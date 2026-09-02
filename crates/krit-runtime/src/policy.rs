use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use krit_capability::{HttpOrigin, is_valid_resource_name};
use krit_wasm::ArtifactMetadata;

use crate::{
    DurableState, HostInputs, HttpRequest, HttpResponse, RuntimeError, RuntimeLimits,
    state::StoreBinding,
};

pub const MAX_POLICY_RESOURCES: usize = 256;
pub const MAX_RETRY_ATTEMPTS: u8 = 4;
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(2);
pub const MAX_RATE_CAPACITY: u32 = 10_000;
pub const MAX_RATE_WINDOW: Duration = Duration::from_secs(60 * 60);
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 1024;
pub const MAX_IDEMPOTENCY_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_IDEMPOTENCY_TTL: Duration = Duration::from_secs(60 * 60);
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
static NEXT_HOST_INSTANCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Default)]
pub struct CancellationHandle {
    cancelled: Arc<AtomicBool>,
}

impl CancellationHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ApprovalOperation {
    AiInvoke,
    HttpBearer,
}

impl ApprovalOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AiInvoke => "ai.invoke",
            Self::HttpBearer => "http.bearer",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalRequest {
    operation: ApprovalOperation,
    resource: String,
}

impl ApprovalRequest {
    pub fn new(operation: ApprovalOperation, resource: impl Into<String>) -> Self {
        Self {
            operation,
            resource: resource.into(),
        }
    }

    pub const fn operation(&self) -> ApprovalOperation {
        self.operation
    }

    pub fn resource(&self) -> &str {
        &self.resource
    }
}

pub trait ApprovalPolicy: Send + Sync {
    fn approve(&self, request: &ApprovalRequest) -> bool;
}

#[derive(Debug, Default)]
pub struct DenyAllApprovalPolicy;

impl ApprovalPolicy for DenyAllApprovalPolicy {
    fn approve(&self, _request: &ApprovalRequest) -> bool {
        false
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExplicitApprovalPolicy {
    allowed: Arc<BTreeSet<(ApprovalOperation, String)>>,
}

impl ExplicitApprovalPolicy {
    pub fn new(
        entries: impl IntoIterator<Item = (ApprovalOperation, String)>,
    ) -> Result<Self, RuntimeError> {
        let mut allowed = BTreeSet::new();
        for (operation, resource) in entries {
            let valid = match operation {
                ApprovalOperation::AiInvoke => is_valid_resource_name(&resource),
                ApprovalOperation::HttpBearer => HttpOrigin::parse_exact(&resource).is_ok(),
            };
            if !valid {
                return Err(RuntimeError::setup(format!(
                    "invalid approval resource `{resource}` for `{}`",
                    operation.as_str()
                )));
            }
            if !allowed.insert((operation, resource.clone())) {
                return Err(RuntimeError::setup(format!(
                    "duplicate approval entry for `{}` resource `{resource}`",
                    operation.as_str()
                )));
            }
        }
        if allowed.len() > MAX_POLICY_RESOURCES {
            return Err(RuntimeError::resource(format!(
                "approval entries exceed the {MAX_POLICY_RESOURCES}-entry limit"
            )));
        }
        Ok(Self {
            allowed: Arc::new(allowed),
        })
    }
}

impl ApprovalPolicy for ExplicitApprovalPolicy {
    fn approve(&self, request: &ApprovalRequest) -> bool {
        self.allowed
            .contains(&(request.operation, request.resource.clone()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u8,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            base_delay: Duration::from_millis(25),
            max_delay: Duration::from_millis(200),
        }
    }
}

impl RetryPolicy {
    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if !(1..=MAX_RETRY_ATTEMPTS).contains(&self.max_attempts)
            || self.base_delay > self.max_delay
            || self.max_delay > MAX_RETRY_DELAY
        {
            return Err(RuntimeError::setup(format!(
                "retry policy must use 1..={MAX_RETRY_ATTEMPTS} attempts and delays no greater than {} ms",
                MAX_RETRY_DELAY.as_millis()
            )));
        }
        Ok(())
    }

    pub(crate) fn delay(self, completed_attempts: u8) -> Duration {
        let exponent = u32::from(completed_attempts.saturating_sub(1));
        self.base_delay
            .saturating_mul(2u32.saturating_pow(exponent))
            .min(self.max_delay)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitPolicy {
    pub capacity: u32,
    pub window: Duration,
}

impl RateLimitPolicy {
    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.capacity == 0
            || self.capacity > MAX_RATE_CAPACITY
            || self.window < Duration::from_millis(1)
            || self.window > MAX_RATE_WINDOW
        {
            return Err(RuntimeError::setup(format!(
                "rate policy must use capacity 1..={MAX_RATE_CAPACITY} and a 1..={} ms window",
                MAX_RATE_WINDOW.as_millis()
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdempotencyPolicy {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub ttl: Duration,
    pub max_key_bytes: usize,
}

impl Default for IdempotencyPolicy {
    fn default() -> Self {
        Self {
            max_entries: 128,
            max_bytes: 16 * 1024 * 1024,
            ttl: Duration::from_secs(5 * 60),
            max_key_bytes: MAX_IDEMPOTENCY_KEY_BYTES,
        }
    }
}

impl IdempotencyPolicy {
    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.max_entries > MAX_IDEMPOTENCY_ENTRIES
            || self.max_bytes > MAX_IDEMPOTENCY_BYTES
            || self.ttl.is_zero()
            || self.ttl > MAX_IDEMPOTENCY_TTL
            || self.max_key_bytes == 0
            || self.max_key_bytes > MAX_IDEMPOTENCY_KEY_BYTES
        {
            return Err(RuntimeError::setup(
                "idempotency policy exceeds host hard maxima",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpJsonAdapterConfig {
    pub origin: String,
    pub path: String,
    pub model: String,
    pub secret: Option<String>,
    pub max_input_bytes: usize,
    pub max_response_bytes: usize,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AiAdapterConfig {
    HttpJson(HttpJsonAdapterConfig),
}

#[derive(Clone, Debug)]
pub struct AgentHostPolicy {
    pub ai_adapters: BTreeMap<String, AiAdapterConfig>,
    pub default_http_retry: RetryPolicy,
    pub default_ai_retry: RetryPolicy,
    pub http_retries: BTreeMap<String, RetryPolicy>,
    pub ai_retries: BTreeMap<String, RetryPolicy>,
    pub default_http_rate: RateLimitPolicy,
    pub default_ai_rate: RateLimitPolicy,
    pub http_rates: BTreeMap<String, RateLimitPolicy>,
    pub ai_rates: BTreeMap<String, RateLimitPolicy>,
    pub max_tracked_resources: usize,
    pub idempotency: IdempotencyPolicy,
}

impl Default for AgentHostPolicy {
    fn default() -> Self {
        Self {
            ai_adapters: BTreeMap::new(),
            default_http_retry: RetryPolicy::default(),
            default_ai_retry: RetryPolicy::default(),
            http_retries: BTreeMap::new(),
            ai_retries: BTreeMap::new(),
            default_http_rate: RateLimitPolicy {
                capacity: 64,
                window: Duration::from_secs(60),
            },
            default_ai_rate: RateLimitPolicy {
                capacity: 16,
                window: Duration::from_secs(60),
            },
            http_rates: BTreeMap::new(),
            ai_rates: BTreeMap::new(),
            max_tracked_resources: 128,
            idempotency: IdempotencyPolicy::default(),
        }
    }
}

#[derive(Clone)]
pub struct AgentHost {
    inner: Arc<AgentHostInner>,
}

struct AgentHostInner {
    inputs: HostInputs,
    policy: AgentHostPolicy,
    approvals: Arc<dyn ApprovalPolicy>,
    instance_nonce: [u8; 32],
    operation_sequence: AtomicU64,
    rates: Mutex<RateState>,
    idempotency: Mutex<IdempotencyState>,
    durable_state: DurableState,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RateResource {
    Ai(String),
    Http(String),
}

struct RateEntry {
    window_started: Instant,
    count: u32,
    last_used: u64,
}

#[derive(Default)]
struct RateState {
    entries: BTreeMap<RateResource, RateEntry>,
    sequence: u64,
}

struct IdempotencyEntry {
    digest: blake3::Hash,
    response: HttpResponse,
    size_bytes: usize,
    expires: Instant,
    last_used: u64,
}

#[derive(Default)]
struct IdempotencyState {
    entries: BTreeMap<([u8; 32], String), IdempotencyEntry>,
    bytes: usize,
    sequence: u64,
}

pub(crate) enum IdempotencyToken {
    Memory {
        key: ([u8; 32], String),
        digest: blake3::Hash,
    },
    Durable {
        binding: Arc<StoreBinding>,
        lease: krit_state::IdempotencyLease,
    },
}

pub(crate) enum IdempotencyDecision {
    Execute(Option<IdempotencyToken>),
    Replay(HttpResponse),
    Conflict,
    Reject(String),
}

impl AgentHost {
    pub fn new(
        inputs: HostInputs,
        policy: AgentHostPolicy,
        approvals: Arc<dyn ApprovalPolicy>,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_state(inputs, policy, approvals, DurableState::default())
    }

    pub fn new_with_state(
        inputs: HostInputs,
        policy: AgentHostPolicy,
        approvals: Arc<dyn ApprovalPolicy>,
        durable_state: DurableState,
    ) -> Result<Self, RuntimeError> {
        validate_policy(&policy)?;
        Ok(Self {
            inner: Arc::new(AgentHostInner {
                inputs,
                policy,
                approvals,
                instance_nonce: host_instance_nonce(),
                operation_sequence: AtomicU64::new(0),
                rates: Mutex::new(RateState::default()),
                idempotency: Mutex::new(IdempotencyState::default()),
                durable_state,
            }),
        })
    }

    pub fn from_inputs(inputs: HostInputs) -> Result<Self, RuntimeError> {
        Self::new(
            inputs,
            AgentHostPolicy::default(),
            Arc::new(DenyAllApprovalPolicy),
        )
    }

    pub(crate) fn inputs(&self) -> &HostInputs {
        &self.inner.inputs
    }

    pub(crate) fn durable_state(&self) -> &DurableState {
        &self.inner.durable_state
    }

    pub(crate) fn next_ai_idempotency_key(&self, adapter: &str) -> Result<String, RuntimeError> {
        let sequence = self
            .inner
            .operation_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| RuntimeError::resource("AI operation sequence exhausted"))?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.inner.instance_nonce);
        hasher.update(&sequence.to_le_bytes());
        hash_part(&mut hasher, adapter.as_bytes());
        Ok(format!("krit-ai-{}", hasher.finalize().to_hex()))
    }

    pub(crate) fn next_lease_owner(&self) -> [u8; 16] {
        let sequence = self
            .inner
            .operation_sequence
            .fetch_add(1, Ordering::Relaxed);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.inner.instance_nonce);
        hasher.update(&sequence.to_le_bytes());
        hasher.update(b"durable-lease");
        let hash = hasher.finalize();
        let mut owner = [0; 16];
        owner.copy_from_slice(&hash.as_bytes()[..16]);
        owner
    }

    pub(crate) fn policy(&self) -> &AgentHostPolicy {
        &self.inner.policy
    }

    pub(crate) fn approve(&self, operation: ApprovalOperation, resource: &str) -> bool {
        self.inner
            .approvals
            .approve(&ApprovalRequest::new(operation, resource))
    }

    pub(crate) fn validate_for_limits(&self, limits: RuntimeLimits) -> Result<(), RuntimeError> {
        for (name, adapter) in &self.inner.policy.ai_adapters {
            match adapter {
                AiAdapterConfig::HttpJson(adapter) => {
                    if adapter.max_input_bytes > limits.ai_input_bytes()
                        || adapter.max_response_bytes > limits.ai_response_bytes()
                        || adapter.timeout > limits.ai_timeout()
                    {
                        return Err(RuntimeError::resource(format!(
                            "AI adapter `{name}` exceeds selected runtime limits"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn check_rate(
        &self,
        resource: RateResource,
        policy: RateLimitPolicy,
    ) -> Result<(), String> {
        let mut state = self
            .inner
            .rates
            .lock()
            .map_err(|_| "rate policy state is unavailable".to_owned())?;
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        if !state.entries.contains_key(&resource) {
            if state.entries.len() >= self.inner.policy.max_tracked_resources {
                let Some(evicted) = state
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(resource, _)| resource.clone())
                else {
                    return Err("rate policy resource tracking is disabled".to_owned());
                };
                state.entries.remove(&evicted);
            }
            state.entries.insert(
                resource.clone(),
                RateEntry {
                    window_started: Instant::now(),
                    count: 0,
                    last_used: sequence,
                },
            );
        }
        let entry = state
            .entries
            .get_mut(&resource)
            .expect("rate entry was inserted above");
        let now = Instant::now();
        if now.saturating_duration_since(entry.window_started) >= policy.window {
            entry.window_started = now;
            entry.count = 0;
        }
        entry.last_used = sequence;
        if entry.count >= policy.capacity {
            return Err(format!(
                "rate limit exceeded for `{}`",
                match &resource {
                    RateResource::Ai(name) | RateResource::Http(name) => name,
                }
            ));
        }
        entry.count += 1;
        Ok(())
    }

    pub(crate) fn idempotency_decision(
        &self,
        metadata: &ArtifactMetadata,
        request: &HttpRequest,
    ) -> Result<IdempotencyDecision, RuntimeError> {
        if matches!(
            request.method.as_str(),
            "GET" | "HEAD" | "OPTIONS" | "TRACE"
        ) {
            return Ok(IdempotencyDecision::Execute(None));
        }
        let mut keys = request
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("idempotency-key"));
        let Some(header) = keys.next() else {
            return Ok(IdempotencyDecision::Execute(None));
        };
        if keys.next().is_some() {
            return Ok(IdempotencyDecision::Reject(
                "Idempotency-Key may appear only once".to_owned(),
            ));
        }
        let durable_binding = self.inner.durable_state.idempotency_binding();
        let max_key_bytes = durable_binding.as_ref().map_or(
            self.inner.policy.idempotency.max_key_bytes,
            |binding| {
                self.inner
                    .policy
                    .idempotency
                    .max_key_bytes
                    .min(binding.store().limits().max_key_bytes)
            },
        );
        if !valid_idempotency_key(&header.value, max_key_bytes) {
            return Ok(IdempotencyDecision::Reject(
                "Idempotency-Key is invalid or exceeds the configured bound".to_owned(),
            ));
        }
        let key = (artifact_identity(metadata), header.value.clone());
        let digest = request_digest(request);
        if let Some(binding) = durable_binding
            && self.inner.policy.idempotency.max_entries > 0
            && self.inner.policy.idempotency.max_bytes > 0
        {
            let owner = self.next_lease_owner();
            let policy = durable_idempotency_policy(
                self.inner.policy.idempotency,
                binding.replay_policy().lease,
                binding.store().limits(),
            );
            let decision = binding
                .store()
                .idempotency_decision(
                    &key.0,
                    &header.value,
                    digest.as_bytes(),
                    &owner,
                    wall_clock_millis()?,
                    policy,
                )
                .map_err(|error| RuntimeError::durable_idempotency(error.message()))?;
            return Ok(match decision {
                krit_state::IdempotencyDecision::Execute(lease) => {
                    IdempotencyDecision::Execute(Some(IdempotencyToken::Durable { binding, lease }))
                }
                krit_state::IdempotencyDecision::Replay(bytes) => {
                    let response = serde_json::from_slice(&bytes).map_err(|_| {
                        RuntimeError::durable_idempotency("durable idempotency response is invalid")
                    })?;
                    IdempotencyDecision::Replay(response)
                }
                krit_state::IdempotencyDecision::Conflict => IdempotencyDecision::Conflict,
                krit_state::IdempotencyDecision::InProgress => IdempotencyDecision::Reject(
                    "request with Idempotency-Key is already in progress".to_owned(),
                ),
            });
        }
        let mut state = self
            .inner
            .idempotency
            .lock()
            .map_err(|_| RuntimeError::setup("idempotency state is unavailable"))?;
        let now = Instant::now();
        state.entries.retain(|_, entry| entry.expires > now);
        state.bytes = state.entries.values().fold(0usize, |total, entry| {
            total.saturating_add(entry.size_bytes)
        });
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        if let Some(entry) = state.entries.get_mut(&key) {
            entry.last_used = sequence;
            if entry.digest == digest {
                return Ok(IdempotencyDecision::Replay(entry.response.clone()));
            }
            return Ok(IdempotencyDecision::Conflict);
        }
        Ok(IdempotencyDecision::Execute(Some(
            IdempotencyToken::Memory { key, digest },
        )))
    }

    pub(crate) fn complete_idempotency(
        &self,
        token: Option<IdempotencyToken>,
        response: &HttpResponse,
    ) -> Result<(), RuntimeError> {
        let Some(token) = token else {
            return Ok(());
        };
        let policy = self.inner.policy.idempotency;
        if let IdempotencyToken::Durable { binding, lease } = token {
            let fail_after_abort =
                |error: RuntimeError| match binding.store().abort_idempotency(&lease) {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.with_cleanup_failure(
                        &RuntimeError::durable_idempotency(cleanup.message()),
                    )),
                };
            if policy.max_entries == 0 || policy.max_bytes == 0 {
                return binding
                    .store()
                    .abort_idempotency(&lease)
                    .map_err(|error| RuntimeError::durable_idempotency(error.message()));
            }
            let bytes = match serde_json::to_vec(response) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return fail_after_abort(RuntimeError::durable_idempotency(
                        "could not encode durable idempotency response",
                    ));
                }
            };
            let retention = durable_idempotency_policy(
                policy,
                binding.replay_policy().lease,
                binding.store().limits(),
            );
            if bytes.len() > retention.max_bytes {
                return binding
                    .store()
                    .abort_idempotency(&lease)
                    .map_err(|error| RuntimeError::durable_idempotency(error.message()));
            }
            let now_millis = match wall_clock_millis() {
                Ok(now_millis) => now_millis,
                Err(error) => return fail_after_abort(error),
            };
            let completion = binding
                .store()
                .complete_idempotency(&lease, &bytes, now_millis, retention)
                .map_err(|error| RuntimeError::durable_idempotency(error.message()));
            return match completion {
                Ok(()) => Ok(()),
                Err(error) => fail_after_abort(error),
            };
        }
        if policy.max_entries == 0 || policy.max_bytes == 0 {
            return Ok(());
        }
        let Some(response_bytes) = cached_response_size(response) else {
            return Ok(());
        };
        if response_bytes > policy.max_bytes {
            return Ok(());
        }
        let IdempotencyToken::Memory { key, digest } = token else {
            unreachable!("durable token returned above")
        };
        let mut state = self
            .inner
            .idempotency
            .lock()
            .map_err(|_| RuntimeError::setup("idempotency state is unavailable"))?;
        if let Some(previous) = state.entries.remove(&key) {
            state.bytes = state.bytes.saturating_sub(previous.size_bytes);
        }
        while state.entries.len() >= policy.max_entries
            || state
                .bytes
                .checked_add(response_bytes)
                .is_none_or(|bytes| bytes > policy.max_bytes)
        {
            let Some(evicted) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = state.entries.remove(&evicted) {
                state.bytes = state.bytes.saturating_sub(entry.size_bytes);
            }
        }
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        state.entries.insert(
            key,
            IdempotencyEntry {
                digest,
                response: response.clone(),
                size_bytes: response_bytes,
                expires: Instant::now() + policy.ttl,
                last_used: sequence,
            },
        );
        state.bytes = state
            .bytes
            .checked_add(response_bytes)
            .ok_or_else(|| RuntimeError::resource("idempotency cache byte count overflowed"))?;
        Ok(())
    }

    pub(crate) fn abort_idempotency(
        &self,
        token: Option<IdempotencyToken>,
    ) -> Result<(), RuntimeError> {
        let Some(IdempotencyToken::Durable { binding, lease }) = token else {
            return Ok(());
        };
        binding
            .store()
            .abort_idempotency(&lease)
            .map_err(|error| RuntimeError::durable_idempotency(error.message()))
    }

    pub fn tracked_rate_resource_count(&self) -> usize {
        self.inner
            .rates
            .lock()
            .map(|state| state.entries.len())
            .unwrap_or_default()
    }

    pub fn idempotency_entry_count(&self) -> usize {
        if let Some(binding) = self.inner.durable_state.idempotency_binding() {
            return binding
                .store()
                .idempotency_counts()
                .map(|counts| counts.0)
                .unwrap_or_default();
        }
        self.inner
            .idempotency
            .lock()
            .map(|state| state.entries.len())
            .unwrap_or_default()
    }

    pub fn idempotency_cached_bytes(&self) -> usize {
        if let Some(binding) = self.inner.durable_state.idempotency_binding() {
            return binding
                .store()
                .idempotency_counts()
                .map(|counts| counts.1)
                .unwrap_or_default();
        }
        self.inner
            .idempotency
            .lock()
            .map(|state| state.bytes)
            .unwrap_or(0)
    }
}

impl fmt::Debug for AgentHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentHost")
            .field("inputs", &self.inner.inputs)
            .field(
                "ai_adapter_names",
                &self.inner.policy.ai_adapters.keys().collect::<Vec<_>>(),
            )
            .field("policy", &"<bounded>")
            .field("approvals", &"<callback>")
            .finish()
    }
}

fn validate_policy(policy: &AgentHostPolicy) -> Result<(), RuntimeError> {
    for count in [
        policy.ai_adapters.len(),
        policy.http_retries.len(),
        policy.ai_retries.len(),
        policy.http_rates.len(),
        policy.ai_rates.len(),
    ] {
        if count > MAX_POLICY_RESOURCES {
            return Err(RuntimeError::resource(format!(
                "host policy map exceeds the {MAX_POLICY_RESOURCES}-entry limit"
            )));
        }
    }
    policy.default_http_retry.validate()?;
    policy.default_ai_retry.validate()?;
    for retry in policy
        .http_retries
        .values()
        .chain(policy.ai_retries.values())
    {
        retry.validate()?;
    }
    policy.default_http_rate.validate()?;
    policy.default_ai_rate.validate()?;
    for rate in policy.http_rates.values().chain(policy.ai_rates.values()) {
        rate.validate()?;
    }
    policy.idempotency.validate()?;
    if policy.max_tracked_resources == 0 || policy.max_tracked_resources > MAX_POLICY_RESOURCES {
        return Err(RuntimeError::setup(format!(
            "max tracked rate resources must be 1..={MAX_POLICY_RESOURCES}"
        )));
    }
    for origin in policy.http_retries.keys().chain(policy.http_rates.keys()) {
        HttpOrigin::parse_exact(origin).map_err(|error| {
            RuntimeError::setup(format!("invalid HTTP policy origin `{origin}`: {error}"))
        })?;
    }
    for adapter in policy.ai_retries.keys().chain(policy.ai_rates.keys()) {
        if !is_valid_resource_name(adapter) || !policy.ai_adapters.contains_key(adapter) {
            return Err(RuntimeError::setup(format!(
                "AI policy references unknown adapter `{adapter}`"
            )));
        }
    }
    for (name, adapter) in &policy.ai_adapters {
        if !is_valid_resource_name(name) {
            return Err(RuntimeError::setup(format!(
                "invalid AI adapter name `{name}`"
            )));
        }
        match adapter {
            AiAdapterConfig::HttpJson(adapter) => validate_http_json_adapter(name, adapter)?,
        }
    }
    Ok(())
}

fn validate_http_json_adapter(
    name: &str,
    adapter: &HttpJsonAdapterConfig,
) -> Result<(), RuntimeError> {
    HttpOrigin::parse_exact(&adapter.origin).map_err(|error| {
        RuntimeError::setup(format!("AI adapter `{name}` has invalid origin: {error}"))
    })?;
    if !safe_origin_path(&adapter.path) {
        return Err(RuntimeError::setup(format!(
            "AI adapter `{name}` has an invalid relative path"
        )));
    }
    if adapter.model.is_empty()
        || adapter.model.len() > 128
        || !adapter.model.is_ascii()
        || adapter.model.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RuntimeError::setup(format!(
            "AI adapter `{name}` has an invalid model identifier"
        )));
    }
    if adapter
        .secret
        .as_deref()
        .is_some_and(|secret| !is_valid_resource_name(secret))
    {
        return Err(RuntimeError::setup(format!(
            "AI adapter `{name}` has an invalid secret name"
        )));
    }
    if adapter.max_input_bytes == 0
        || adapter.max_input_bytes > crate::HARD_MAX_LIMITS.ai_input_bytes()
        || adapter.max_response_bytes == 0
        || adapter.max_response_bytes > crate::HARD_MAX_LIMITS.ai_response_bytes()
        || adapter.timeout.is_zero()
        || adapter.timeout > crate::HARD_MAX_LIMITS.ai_timeout()
    {
        return Err(RuntimeError::setup(format!(
            "AI adapter `{name}` exceeds hard input, response, or timeout bounds"
        )));
    }
    Ok(())
}

fn safe_origin_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("://")
        && !path
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'\\' | b'?' | b'#'))
}

pub(crate) fn valid_idempotency_key(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

pub(crate) fn request_digest(request: &HttpRequest) -> blake3::Hash {
    hash_request(request, true)
}

pub(crate) fn exact_request_digest(request: &HttpRequest) -> blake3::Hash {
    hash_request(request, false)
}

fn hash_request(request: &HttpRequest, omit_idempotency_key: bool) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hash_part(&mut hasher, request.method.as_bytes());
    hash_part(&mut hasher, request.path.as_bytes());
    hash_part(&mut hasher, request.query.as_bytes());
    for header in &request.headers {
        let normalized = header.name.to_ascii_lowercase();
        if omit_idempotency_key && normalized == "idempotency-key" {
            continue;
        }
        hash_part(&mut hasher, normalized.as_bytes());
        hash_part(&mut hasher, header.value.as_bytes());
    }
    hash_part(&mut hasher, request.body.as_bytes());
    hasher.finalize()
}

fn durable_idempotency_policy(
    policy: IdempotencyPolicy,
    lease: Duration,
    limits: krit_state::StoreLimits,
) -> krit_state::RetentionPolicy {
    krit_state::RetentionPolicy {
        max_entries: policy.max_entries.min(limits.max_replay_entries),
        max_bytes: policy.max_bytes.min(limits.max_replay_bytes),
        ttl: policy.ttl,
        lease,
    }
}

fn wall_clock_millis() -> Result<i64, RuntimeError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RuntimeError::durable_idempotency("system clock is before Unix epoch"))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| RuntimeError::durable_idempotency("system clock exceeds timestamp range"))
}

pub(crate) fn artifact_identity(metadata: &ArtifactMetadata) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_part(&mut hasher, metadata.package.name.as_bytes());
    hash_part(&mut hasher, metadata.package.version.as_bytes());
    hash_part(&mut hasher, metadata.world.as_bytes());
    hash_part(&mut hasher, metadata.digest.as_bytes());
    *hasher.finalize().as_bytes()
}

fn hash_part(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn host_instance_nonce() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hasher.update(&timestamp.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(
        &NEXT_HOST_INSTANCE
            .fetch_add(1, Ordering::Relaxed)
            .to_le_bytes(),
    );
    *hasher.finalize().as_bytes()
}

fn cached_response_size(response: &HttpResponse) -> Option<usize> {
    let mut bytes = std::mem::size_of::<HttpResponse>().checked_add(response.body.len())?;
    bytes = bytes.checked_add(
        std::mem::size_of::<crate::HttpHeader>().checked_mul(response.headers.len())?,
    )?;
    for header in &response.headers {
        bytes = bytes.checked_add(header.name.len())?;
        bytes = bytes.checked_add(header.value.len())?;
    }
    Some(bytes)
}
