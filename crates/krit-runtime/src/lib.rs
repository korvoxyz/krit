mod ai;
mod bindings;
mod error;
mod host;
mod limits;
mod network;
mod observability;
mod permissions;
mod policy;
mod state;
mod webhook;

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use krit_wasm::{
    ArtifactMetadata, ComponentInspection, PROGRAM_WORLD, PURE_PROGRAM_WORLD, WEBHOOK_INTERFACE,
    validate_artifact,
};
use serde::Serialize;
use wasmtime::{
    Config, Engine, ProfilingStrategy, Store, StoreLimits, StoreLimitsBuilder, Strategy, Trap,
    component::{Component, HasSelf, Linker, Resource, ResourceTable},
};

use ai::AiAdapter;
use error::HostLimitError;
pub use error::{RuntimeError, RuntimeErrorKind};
pub use host::{HostInputs, MAX_HOST_INPUT_ENTRIES, NetworkPolicy, SecretStore};
pub use limits::{
    DEFAULT_LIMITS, HARD_MAX_LIMITS, HOST_POLICY_VERSION, HOST_STACK_HEADROOM_BYTES, RuntimeLimits,
    STATE_HOST_POLICY_VERSION,
};
pub use observability::{LogEvent, LogField, LogLevel, MAX_LOG_NAME_BYTES, REDACTED_VALUE};
pub use permissions::{ApprovalFact, EffectivePermissions, GrantSet, PermissionFact};
pub use policy::{
    AgentHost, AgentHostPolicy, AiAdapterConfig, ApprovalOperation, ApprovalPolicy,
    ApprovalRequest, CancellationHandle, DenyAllApprovalPolicy, ExplicitApprovalPolicy,
    HttpJsonAdapterConfig, IdempotencyPolicy, MAX_IDEMPOTENCY_BYTES, MAX_IDEMPOTENCY_ENTRIES,
    MAX_IDEMPOTENCY_KEY_BYTES, MAX_IDEMPOTENCY_TTL, MAX_POLICY_RESOURCES, MAX_RATE_CAPACITY,
    MAX_RATE_WINDOW, MAX_RETRY_ATTEMPTS, MAX_RETRY_DELAY, RateLimitPolicy, RetryPolicy,
};
pub use state::{
    Durability, DurableState, DurableStoreDefinition, RetentionPolicy,
    StoreLimits as DurableStoreLimits,
};
pub use webhook::{HttpHeader, HttpRequest, HttpResponse};

#[derive(Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub output: Vec<u8>,
    pub events: Vec<LogEvent>,
    pub stats: ExecutionStats,
}

#[derive(Debug, Eq, PartialEq)]
pub struct WebhookExecutionResult {
    pub response: HttpResponse,
    pub output: Vec<u8>,
    pub events: Vec<LogEvent>,
    pub stats: ExecutionStats,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStats {
    pub policy_version: u32,
    pub fuel_budget: u64,
    pub fuel_consumed: u64,
    pub fuel_remaining: u64,
    pub host_calls: u64,
    pub http_calls: u64,
    pub ai_calls: u64,
    pub network_attempts: u64,
    pub retries: u64,
    pub rate_limit_denials: u64,
    pub idempotency_replayed: bool,
    pub state_operations: u64,
    pub state_reads: u64,
    pub state_writes: u64,
    pub checkpoint_reads: u64,
    pub checkpoint_writes: u64,
    pub replay_hits: u64,
    pub replay_misses: u64,
    pub output_bytes: usize,
    pub elapsed_micros: u128,
}

pub struct Runtime {
    engine: Engine,
    limits: RuntimeLimits,
    epoch_lock: Mutex<()>,
    active_deadline_workers: Arc<AtomicUsize>,
    active_dns_workers: Arc<AtomicUsize>,
}

struct HostState {
    output: Vec<u8>,
    host_calls: u64,
    host_call_limit: u64,
    output_limit: usize,
    store_limits: StoreLimits,
    limits: RuntimeLimits,
    grants: Option<GrantSet>,
    effects: BTreeSet<String>,
    requirements: BTreeSet<(String, String)>,
    agent_host: AgentHost,
    cancellation: CancellationHandle,
    resources: ResourceTable,
    http_calls: u64,
    ai_calls: u64,
    network_attempts: u64,
    retries: u64,
    rate_limit_denials: u64,
    events: Vec<LogEvent>,
    log_bytes: usize,
    invocation_deadline: Instant,
    active_dns_workers: Arc<AtomicUsize>,
    artifact_identity: [u8; 32],
    state: state::InvocationState,
}

struct HostStateConfig {
    grants: Option<GrantSet>,
    effects: BTreeSet<String>,
    requirements: BTreeSet<(String, String)>,
    agent_host: AgentHost,
    cancellation: CancellationHandle,
    started: Instant,
    artifact_identity: [u8; 32],
}

pub struct SecretHandle {
    bytes: Arc<host::SecretBytes>,
}

struct RetryRequest<'a> {
    origin: &'a krit_capability::HttpOrigin,
    request: &'a HttpRequest,
    bearer: Option<&'a host::SecretBytes>,
    rate_resource: policy::RateResource,
    rate_policy: RateLimitPolicy,
    retry_policy: RetryPolicy,
    approval: Option<(ApprovalOperation, String)>,
    approval_prechecked: bool,
    deadline: Instant,
}

impl Runtime {
    pub fn new(limits: RuntimeLimits) -> Result<Self, RuntimeError> {
        limits.validate()?;
        curl::init();
        let mut config = Config::new();
        config
            .strategy(Strategy::Cranelift)
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(limits.wasm_stack_bytes())
            .async_stack_size(limits.wasm_stack_bytes())
            .profiler(ProfilingStrategy::None);
        let engine = Engine::new(&config).map_err(|error| {
            RuntimeError::setup(format!("could not create Wasmtime engine: {error}"))
        })?;
        Ok(Self {
            engine,
            limits,
            epoch_lock: Mutex::new(()),
            active_deadline_workers: Arc::new(AtomicUsize::new(0)),
            active_dns_workers: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn permissions(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
    ) -> Result<EffectivePermissions, RuntimeError> {
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        self.preflight_resources(&inspection)?;
        Ok(grants.evaluate(metadata))
    }

    pub fn execute(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
    ) -> Result<ExecutionResult, RuntimeError> {
        self.execute_with_cancellation(bytes, metadata, grants, &CancellationHandle::new())
    }

    pub fn execute_with_cancellation(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
        cancellation: &CancellationHandle,
    ) -> Result<ExecutionResult, RuntimeError> {
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        grants.authorize(metadata)?;
        self.preflight_resources(&inspection)?;
        if cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before guest execution",
            ));
        }

        let _epoch_guard = self
            .epoch_lock
            .lock()
            .map_err(|_| RuntimeError::setup("runtime epoch scheduler lock is poisoned"))?;
        if cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before component instantiation",
            ));
        }
        let component = Component::new(&self.engine, bytes).map_err(|error| {
            RuntimeError::setup(format!(
                "validated component could not be compiled by Wasmtime 47.x: {error}"
            ))
        })?;
        let worker_stack_bytes = self
            .limits
            .wasm_stack_bytes()
            .checked_add(HOST_STACK_HEADROOM_BYTES)
            .ok_or_else(|| RuntimeError::setup("Wasm execution worker stack size overflowed"))?;

        thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("krit-wasm-execution".to_owned())
                .stack_size(worker_stack_bytes)
                .spawn_scoped(scope, || {
                    self.execute_component(&component, metadata, cancellation.clone())
                })
                .map_err(|error| {
                    RuntimeError::setup(format!(
                        "could not start isolated Wasm execution worker: {error}"
                    ))
                })?;
            worker
                .join()
                .map_err(|_| RuntimeError::setup("isolated Wasm execution worker panicked"))?
        })
    }

    pub fn invoke_webhook(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
        inputs: &HostInputs,
        request: HttpRequest,
    ) -> Result<WebhookExecutionResult, RuntimeError> {
        let agent_host = AgentHost::from_inputs(inputs.clone())?;
        self.invoke_webhook_with_host(bytes, metadata, grants, &agent_host, request)
    }

    pub fn invoke_webhook_with_host(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
        agent_host: &AgentHost,
        request: HttpRequest,
    ) -> Result<WebhookExecutionResult, RuntimeError> {
        self.invoke_webhook_with_cancellation(
            bytes,
            metadata,
            grants,
            agent_host,
            &CancellationHandle::new(),
            request,
        )
    }

    pub fn invoke_webhook_with_cancellation(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
        grants: &GrantSet,
        agent_host: &AgentHost,
        cancellation: &CancellationHandle,
        request: HttpRequest,
    ) -> Result<WebhookExecutionResult, RuntimeError> {
        request.validate(self.limits)?;
        self.validate_inputs(bytes, metadata)?;
        let inspection = validate_artifact(bytes, metadata)?;
        grants.authorize(metadata)?;
        self.preflight_resources(&inspection)?;
        self.validate_agent_host(grants, metadata, agent_host)?;
        if inspection.exports != [WEBHOOK_INTERFACE] {
            return Err(RuntimeError::import_mismatch(
                "artifact does not export the typed webhook interface",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before guest execution",
            ));
        }
        let _epoch_guard = self
            .epoch_lock
            .lock()
            .map_err(|_| RuntimeError::setup("runtime epoch scheduler lock is poisoned"))?;
        if cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before component instantiation",
            ));
        }
        let component = Component::new(&self.engine, bytes).map_err(|error| {
            RuntimeError::setup(format!(
                "validated webhook component could not be compiled by Wasmtime 47.x: {error}"
            ))
        })?;
        let worker_stack_bytes = self
            .limits
            .wasm_stack_bytes()
            .checked_add(HOST_STACK_HEADROOM_BYTES)
            .ok_or_else(|| RuntimeError::setup("Wasm execution worker stack size overflowed"))?;
        let idempotency = agent_host.idempotency_decision(metadata, &request)?;
        let policy::IdempotencyDecision::Execute(mut idempotency_token) = idempotency else {
            let (response, replayed) = match idempotency {
                policy::IdempotencyDecision::Replay(response) => (response, true),
                policy::IdempotencyDecision::Conflict => (
                    HttpResponse {
                        status: 409,
                        headers: Vec::new(),
                        body: "idempotency key conflicts with a different request".to_owned(),
                    },
                    false,
                ),
                policy::IdempotencyDecision::Reject(message) => (
                    HttpResponse {
                        status: 400,
                        headers: Vec::new(),
                        body: message,
                    },
                    false,
                ),
                policy::IdempotencyDecision::Execute(_) => {
                    unreachable!("execute decision matched above")
                }
            };
            return Ok(WebhookExecutionResult {
                response,
                output: Vec::new(),
                events: Vec::new(),
                stats: self.empty_stats(replayed, !agent_host.durable_state().is_empty()),
            });
        };

        let execution = thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("krit-webhook-execution".to_owned())
                .stack_size(worker_stack_bytes)
                .spawn_scoped(scope, || {
                    self.invoke_webhook_component(
                        &component,
                        metadata,
                        grants.clone(),
                        agent_host.clone(),
                        cancellation.clone(),
                        request,
                    )
                })
                .map_err(|error| {
                    RuntimeError::setup(format!(
                        "could not start isolated webhook execution worker: {error}"
                    ))
                })?;
            worker
                .join()
                .map_err(|_| RuntimeError::setup("isolated webhook execution worker panicked"))?
        });
        match execution {
            Ok(result) => {
                agent_host
                    .complete_idempotency(idempotency_token.take(), &result.response)
                    .map_err(|error| error.with_events(result.events.clone()))?;
                Ok(result)
            }
            Err(error) => match agent_host.abort_idempotency(idempotency_token.take()) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.with_cleanup_failure(&cleanup)),
            },
        }
    }

    fn invoke_webhook_component(
        &self,
        component: &Component,
        metadata: &ArtifactMetadata,
        grants: GrantSet,
        agent_host: AgentHost,
        cancellation: CancellationHandle,
        request: HttpRequest,
    ) -> Result<WebhookExecutionResult, RuntimeError> {
        let started = Instant::now();
        let mut store = Store::new(
            &self.engine,
            self.new_host_state(HostStateConfig {
                grants: Some(grants),
                effects: metadata.effects.iter().cloned().collect(),
                requirements: metadata
                    .requirements
                    .iter()
                    .map(|requirement| {
                        (requirement.capability.clone(), requirement.resource.clone())
                    })
                    .collect(),
                agent_host,
                cancellation,
                started,
                artifact_identity: policy::artifact_identity(metadata),
            }),
        );
        store.limiter(|state| &mut state.store_limits);
        store
            .set_fuel(self.limits.fuel())
            .map_err(|error| RuntimeError::setup(format!("could not set Wasm fuel: {error}")))?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_trap();
        let deadline = DeadlineWorker::start(
            self.engine.clone(),
            self.limits.deadline(),
            Arc::clone(&self.active_deadline_workers),
        )?;
        let call = self.call_webhook(component, &mut store, request);
        let timed_out = deadline
            .finish()
            .map_err(|error| error.with_events(store.data().events.clone()))?;
        if timed_out {
            return Err(RuntimeError::deadline("Wasm wall deadline exceeded")
                .with_events(store.data().events.clone()));
        }
        let response = call.map_err(|error| error.with_events(store.data().events.clone()))?;
        response
            .validate(self.limits)
            .map_err(|error| error.with_events(store.data().events.clone()))?;
        if store.data().state.touched() && store.data().cancellation.is_cancelled() {
            return Err(RuntimeError::cancelled(
                "embedding cancellation requested before durable state commit",
            )
            .with_events(store.data().events.clone()));
        }
        if let Err(error) = store.data_mut().state.commit() {
            return Err(error.with_events(store.data().events.clone()));
        }
        let remaining = store.get_fuel().map_err(|error| {
            RuntimeError::setup(format!("could not read remaining fuel: {error}"))
                .with_events(store.data().events.clone())
        })?;
        let state = store.into_data();
        Ok(WebhookExecutionResult {
            stats: ExecutionStats {
                policy_version: if state.agent_host.durable_state().is_empty() {
                    HOST_POLICY_VERSION
                } else {
                    STATE_HOST_POLICY_VERSION
                },
                fuel_budget: self.limits.fuel(),
                fuel_consumed: self.limits.fuel().saturating_sub(remaining),
                fuel_remaining: remaining,
                host_calls: state.host_calls,
                http_calls: state.http_calls,
                ai_calls: state.ai_calls,
                network_attempts: state.network_attempts,
                retries: state.retries,
                rate_limit_denials: state.rate_limit_denials,
                idempotency_replayed: false,
                state_operations: state.state.operations(),
                state_reads: state.state.reads(),
                state_writes: state.state.writes(),
                checkpoint_reads: state.state.checkpoint_reads(),
                checkpoint_writes: state.state.checkpoint_writes(),
                replay_hits: state.state.replay_hits(),
                replay_misses: state.state.replay_misses(),
                output_bytes: state.output.len(),
                elapsed_micros: started.elapsed().as_micros(),
            },
            output: state.output,
            events: state.events,
            response,
        })
    }

    fn execute_component(
        &self,
        component: &Component,
        metadata: &ArtifactMetadata,
        cancellation: CancellationHandle,
    ) -> Result<ExecutionResult, RuntimeError> {
        let started = Instant::now();
        let mut store = Store::new(
            &self.engine,
            self.new_host_state(HostStateConfig {
                grants: None,
                effects: BTreeSet::new(),
                requirements: BTreeSet::new(),
                agent_host: AgentHost::from_inputs(HostInputs::default())?,
                cancellation,
                started,
                artifact_identity: [0; 32],
            }),
        );
        store.limiter(|state| &mut state.store_limits);
        store
            .set_fuel(self.limits.fuel())
            .map_err(|error| RuntimeError::setup(format!("could not set Wasm fuel: {error}")))?;
        store.set_epoch_deadline(1);
        store.epoch_deadline_trap();

        let deadline = DeadlineWorker::start(
            self.engine.clone(),
            self.limits.deadline(),
            Arc::clone(&self.active_deadline_workers),
        )?;
        let call = match metadata.world.as_str() {
            PURE_PROGRAM_WORLD => self.call_pure(component, &mut store),
            PROGRAM_WORLD => self.call_stdout(component, &mut store),
            world => Err(RuntimeError::import_mismatch(format!(
                "validated artifact selected unsupported world `{world}`"
            ))),
        };
        let timed_out = deadline.finish()?;
        if timed_out {
            return Err(RuntimeError::deadline("Wasm wall deadline exceeded"));
        }
        call?;

        let remaining = store.get_fuel().map_err(|error| {
            RuntimeError::setup(format!("could not read remaining fuel: {error}"))
        })?;
        let state = store.into_data();
        Ok(ExecutionResult {
            stats: ExecutionStats {
                policy_version: HOST_POLICY_VERSION,
                fuel_budget: self.limits.fuel(),
                fuel_consumed: self.limits.fuel().saturating_sub(remaining),
                fuel_remaining: remaining,
                host_calls: state.host_calls,
                http_calls: state.http_calls,
                ai_calls: state.ai_calls,
                network_attempts: state.network_attempts,
                retries: state.retries,
                rate_limit_denials: state.rate_limit_denials,
                idempotency_replayed: false,
                state_operations: 0,
                state_reads: 0,
                state_writes: 0,
                checkpoint_reads: 0,
                checkpoint_writes: 0,
                replay_hits: 0,
                replay_misses: 0,
                output_bytes: state.output.len(),
                elapsed_micros: started.elapsed().as_micros(),
            },
            output: state.output,
            events: state.events,
        })
    }

    pub fn limits(&self) -> RuntimeLimits {
        self.limits
    }

    pub fn active_deadline_workers(&self) -> usize {
        self.active_deadline_workers.load(Ordering::Acquire)
    }

    pub fn active_dns_workers(&self) -> usize {
        self.active_dns_workers.load(Ordering::Acquire)
    }

    fn empty_stats(&self, idempotency_replayed: bool, durable_state: bool) -> ExecutionStats {
        ExecutionStats {
            policy_version: if durable_state {
                STATE_HOST_POLICY_VERSION
            } else {
                HOST_POLICY_VERSION
            },
            fuel_budget: self.limits.fuel(),
            fuel_consumed: 0,
            fuel_remaining: self.limits.fuel(),
            host_calls: 0,
            http_calls: 0,
            ai_calls: 0,
            network_attempts: 0,
            retries: 0,
            rate_limit_denials: 0,
            idempotency_replayed,
            state_operations: 0,
            state_reads: 0,
            state_writes: 0,
            checkpoint_reads: 0,
            checkpoint_writes: 0,
            replay_hits: 0,
            replay_misses: 0,
            output_bytes: 0,
            elapsed_micros: 0,
        }
    }

    fn validate_inputs(
        &self,
        bytes: &[u8],
        metadata: &ArtifactMetadata,
    ) -> Result<(), RuntimeError> {
        if bytes.len() > self.limits.component_bytes() {
            return Err(RuntimeError::setup(format!(
                "component size {} exceeds the pre-compilation limit {}",
                bytes.len(),
                self.limits.component_bytes()
            )));
        }
        let metadata_size = serde_json::to_vec(metadata)
            .map_err(|error| {
                RuntimeError::setup(format!("could not size artifact metadata: {error}"))
            })?
            .len();
        if metadata_size > self.limits.metadata_bytes() {
            return Err(RuntimeError::setup(format!(
                "artifact metadata size {metadata_size} exceeds the pre-compilation limit {}",
                self.limits.metadata_bytes()
            )));
        }
        Ok(())
    }

    fn preflight_resources(&self, inspection: &ComponentInspection) -> Result<(), RuntimeError> {
        if usize::try_from(inspection.core_module_count)
            .map_or(true, |count| count > self.limits.instances())
        {
            return Err(RuntimeError::resource(
                "component instance count exceeds the host limit",
            ));
        }
        if usize::try_from(inspection.table_count)
            .map_or(true, |count| count > self.limits.tables())
        {
            return Err(RuntimeError::resource(
                "component table count exceeds the host limit",
            ));
        }
        if usize::try_from(inspection.table_elements)
            .map_or(true, |count| count > self.limits.table_elements())
        {
            return Err(RuntimeError::resource(
                "component table elements exceed the host limit",
            ));
        }
        if usize::try_from(inspection.memory_count)
            .map_or(true, |count| count > self.limits.memories())
        {
            return Err(RuntimeError::resource(
                "component memory count exceeds the host limit",
            ));
        }
        if usize::try_from(inspection.memory_minimum_bytes)
            .map_or(true, |bytes| bytes > self.limits.memory_bytes())
        {
            return Err(RuntimeError::resource(
                "component minimum linear memory exceeds the host byte limit",
            ));
        }
        Ok(())
    }

    fn validate_agent_host(
        &self,
        grants: &GrantSet,
        metadata: &ArtifactMetadata,
        agent_host: &AgentHost,
    ) -> Result<(), RuntimeError> {
        agent_host.validate_for_limits(self.limits)?;
        agent_host
            .durable_state()
            .validate_for_runtime(self.limits.deadline())?;
        let inputs = agent_host.inputs();
        let mut config_bytes = 0usize;
        for (name, value) in inputs.config() {
            if !grants.grants("config.read", Some(name)) {
                return Err(RuntimeError::authorization(format!(
                    "host configuration key `{name}` is not granted by the manifest"
                )));
            }
            config_bytes = config_bytes
                .checked_add(name.len())
                .and_then(|bytes| bytes.checked_add(value.len()))
                .ok_or_else(|| RuntimeError::resource("host configuration size overflowed"))?;
            if config_bytes > self.limits.host_config_bytes() {
                return Err(RuntimeError::resource(format!(
                    "host configuration exceeds the {}-byte limit",
                    self.limits.host_config_bytes()
                )));
            }
        }
        for (name, size) in inputs.secrets().iter() {
            if !grants.grants("secret.read", Some(name)) {
                return Err(RuntimeError::authorization(format!(
                    "host secret `{name}` is not granted by the manifest"
                )));
            }
            if size > self.limits.secret_bytes() {
                return Err(RuntimeError::resource(format!(
                    "host secret `{name}` exceeds the {}-byte limit",
                    self.limits.secret_bytes()
                )));
            }
        }
        for (name, adapter) in &agent_host.policy().ai_adapters {
            if !grants.grants("ai.invoke", Some(name)) {
                return Err(RuntimeError::authorization(format!(
                    "AI adapter `{name}` is not granted by the manifest"
                )));
            }
            match adapter {
                AiAdapterConfig::HttpJson(adapter) => {
                    if !grants.grants("http.request", Some(&adapter.origin)) {
                        return Err(RuntimeError::authorization(format!(
                            "AI adapter `{name}` origin `{}` is not granted by the manifest",
                            adapter.origin
                        )));
                    }
                    if let Some(secret) = &adapter.secret
                        && !grants.grants("secret.read", Some(secret))
                    {
                        return Err(RuntimeError::authorization(format!(
                            "AI adapter `{name}` secret `{secret}` is not granted by the manifest"
                        )));
                    }
                }
            }
        }
        for origin in agent_host
            .policy()
            .http_retries
            .keys()
            .chain(agent_host.policy().http_rates.keys())
        {
            if !grants.grants("http.request", Some(origin)) {
                return Err(RuntimeError::authorization(format!(
                    "HTTP policy origin `{origin}` is not granted by the manifest"
                )));
            }
        }
        for adapter in agent_host
            .policy()
            .ai_retries
            .keys()
            .chain(agent_host.policy().ai_rates.keys())
        {
            if !grants.grants("ai.invoke", Some(adapter)) {
                return Err(RuntimeError::authorization(format!(
                    "AI policy adapter `{adapter}` is not granted by the manifest"
                )));
            }
        }
        for requirement in metadata
            .requirements
            .iter()
            .filter(|requirement| requirement.capability == "ai.invoke")
        {
            if !agent_host
                .policy()
                .ai_adapters
                .contains_key(&requirement.resource)
            {
                return Err(RuntimeError::setup(format!(
                    "required AI adapter `{}` is not configured",
                    requirement.resource
                )));
            }
        }
        for name in agent_host.durable_state().store_names() {
            if !grants.grants("state.transaction", Some(name)) {
                return Err(RuntimeError::authorization(format!(
                    "durable state store `{name}` is not granted by the manifest"
                )));
            }
        }
        for requirement in metadata
            .requirements
            .iter()
            .filter(|requirement| requirement.capability == "state.transaction")
        {
            agent_host.durable_state().binding(&requirement.resource)?;
        }
        let rate_resources = metadata
            .requirements
            .iter()
            .filter(|requirement| {
                matches!(
                    requirement.capability.as_str(),
                    "ai.invoke" | "http.request"
                )
            })
            .map(|requirement| {
                (
                    requirement.capability.as_str(),
                    requirement.resource.as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        if rate_resources.len() > agent_host.policy().max_tracked_resources {
            return Err(RuntimeError::resource(format!(
                "artifact requires {} rate-limited resources but AgentHost tracks at most {}",
                rate_resources.len(),
                agent_host.policy().max_tracked_resources
            )));
        }
        Ok(())
    }

    fn new_host_state(&self, config: HostStateConfig) -> HostState {
        let HostStateConfig {
            grants,
            effects,
            requirements,
            agent_host,
            cancellation,
            started,
            artifact_identity,
        } = config;
        let store_limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.memory_bytes())
            .table_elements(self.limits.table_elements())
            .instances(self.limits.instances())
            .tables(self.limits.tables())
            .memories(self.limits.memories())
            .trap_on_grow_failure(true)
            .build();
        HostState {
            output: Vec::new(),
            host_calls: 0,
            host_call_limit: self.limits.host_calls(),
            output_limit: self.limits.output_bytes(),
            store_limits,
            limits: self.limits,
            grants,
            effects,
            requirements,
            agent_host,
            cancellation,
            resources: ResourceTable::new(),
            http_calls: 0,
            ai_calls: 0,
            network_attempts: 0,
            retries: 0,
            rate_limit_denials: 0,
            events: Vec::new(),
            log_bytes: 0,
            invocation_deadline: started + self.limits.deadline(),
            active_dns_workers: Arc::clone(&self.active_dns_workers),
            artifact_identity,
            state: state::InvocationState::default(),
        }
    }

    fn call_pure(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
    ) -> Result<(), RuntimeError> {
        let linker = Linker::new(&self.engine);
        let program = bindings::pure::PureProgram::instantiate(&mut *store, component, &linker)
            .map_err(map_wasmtime_error)?;
        program.call_run(&mut *store).map_err(map_wasmtime_error)
    }

    fn call_stdout(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
    ) -> Result<(), RuntimeError> {
        let mut linker = Linker::new(&self.engine);
        bindings::stdout::Program::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|error| {
                RuntimeError::import_mismatch(format!(
                    "could not link the exact Krit stdout interface: {error}"
                ))
            })?;
        let program = bindings::stdout::Program::instantiate(&mut *store, component, &linker)
            .map_err(map_wasmtime_error)?;
        program.call_run(&mut *store).map_err(map_wasmtime_error)
    }

    fn call_webhook(
        &self,
        component: &Component,
        store: &mut Store<HostState>,
        request: HttpRequest,
    ) -> Result<HttpResponse, RuntimeError> {
        let mut linker = Linker::new(&self.engine);
        bindings::webhook::WebhookHostProgram::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )
        .map_err(|error| {
            RuntimeError::import_mismatch(format!(
                "could not link the exact Krit webhook host interfaces: {error}"
            ))
        })?;
        let program =
            bindings::webhook::WebhookHostProgram::instantiate(&mut *store, component, &linker)
                .map_err(map_wasmtime_error)?;
        let request = bindings::webhook::exports::krit::runtime::webhook::Request {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(
                    |header| bindings::webhook::exports::krit::runtime::webhook::Header {
                        name: header.name,
                        value: header.value,
                    },
                )
                .collect(),
            body: request.body,
        };
        let response = program
            .krit_runtime_webhook()
            .call_handle(&mut *store, &request)
            .map_err(map_wasmtime_error)?;
        Ok(HttpResponse {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        })
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(RuntimeLimits::default()).expect("default runtime limits are valid")
    }
}

impl HostState {
    fn write(&mut self, rendered: &str, newline: bool) -> wasmtime::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(wasmtime::Error::new(RuntimeError::cancelled(
                "embedding cancellation requested during stdout host call",
            )));
        }
        let next_calls = self
            .host_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next_calls > self.host_call_limit {
            return Err(wasmtime::Error::new(HostLimitError::Calls));
        }
        let additional = rendered
            .len()
            .checked_add(usize::from(newline))
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Output))?;
        let next_bytes = self
            .output
            .len()
            .checked_add(additional)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Output))?;
        if next_bytes > self.output_limit {
            return Err(wasmtime::Error::new(HostLimitError::Output));
        }
        self.output
            .try_reserve(additional)
            .map_err(|_| wasmtime::Error::new(HostLimitError::Output))?;
        self.host_calls = next_calls;
        self.output.extend_from_slice(rendered.as_bytes());
        if newline {
            self.output.push(b'\n');
        }
        Ok(())
    }
}

impl bindings::stdout::krit::runtime::stdout::Host for HostState {
    fn write_int(&mut self, value: i64, newline: bool) -> wasmtime::Result<()> {
        self.write(&value.to_string(), newline)
    }

    fn write_bool(&mut self, value: bool, newline: bool) -> wasmtime::Result<()> {
        self.write(if value { "true" } else { "false" }, newline)
    }

    fn write_unit(&mut self, newline: bool) -> wasmtime::Result<()> {
        self.write("()", newline)
    }
}

impl bindings::webhook::krit::runtime::stdout::Host for HostState {
    fn write_int(&mut self, value: i64, newline: bool) -> wasmtime::Result<()> {
        self.write(&value.to_string(), newline)
    }

    fn write_bool(&mut self, value: bool, newline: bool) -> wasmtime::Result<()> {
        self.write(if value { "true" } else { "false" }, newline)
    }

    fn write_unit(&mut self, newline: bool) -> wasmtime::Result<()> {
        self.write("()", newline)
    }
}

impl bindings::webhook::krit::runtime::config::Host for HostState {
    fn get_string(&mut self, key: String) -> wasmtime::Result<Result<String, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("config.read", &key)?;
        Ok(self
            .agent_host
            .inputs()
            .config()
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("configuration key `{key}` is not configured")))
    }
}

impl bindings::webhook::krit::runtime::secrets::HostSecret for HostState {
    fn drop(&mut self, secret: Resource<SecretHandle>) -> wasmtime::Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(wasmtime::Error::new(RuntimeError::cancelled(
                "embedding cancellation requested during secret handle drop",
            )));
        }
        self.resources
            .delete(secret)
            .map(|_| ())
            .map_err(|error| wasmtime::Error::msg(format!("secret handle drop failed: {error}")))
    }
}

impl bindings::webhook::krit::runtime::secrets::Host for HostState {
    fn acquire(
        &mut self,
        name: String,
    ) -> wasmtime::Result<Result<Resource<SecretHandle>, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("secret.read", &name)?;
        let Some(bytes) = self.agent_host.inputs().secrets().get(&name) else {
            return Ok(Err(format!("secret `{name}` is not configured")));
        };
        let handle = self
            .resources
            .push(SecretHandle { bytes })
            .map_err(|error| {
                wasmtime::Error::new(RuntimeError::resource(format!(
                    "secret handle table is full: {error}"
                )))
            })?;
        Ok(Ok(handle))
    }
}

impl bindings::webhook::krit::runtime::http::Host for HostState {
    fn send(
        &mut self,
        origin: String,
        request: bindings::webhook::krit::runtime::http::Request,
        bearer: Option<Resource<SecretHandle>>,
    ) -> wasmtime::Result<Result<bindings::webhook::krit::runtime::http::Response, String>> {
        let request = HttpRequest {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: request.body,
        };
        let response = match self.perform_http(origin, request, bearer)? {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        Ok(Ok(bindings::webhook::krit::runtime::http::Response {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| bindings::webhook::krit::runtime::http::Header {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        }))
    }
}

impl bindings::webhook::krit::runtime::http_anonymous::Host for HostState {
    fn send(
        &mut self,
        origin: String,
        request: bindings::webhook::krit::runtime::http_anonymous::Request,
    ) -> wasmtime::Result<Result<bindings::webhook::krit::runtime::http_anonymous::Response, String>>
    {
        let request = HttpRequest {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: request.body,
        };
        let response = match self.perform_http(origin, request, None)? {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        Ok(Ok(
            bindings::webhook::krit::runtime::http_anonymous::Response {
                status: response.status,
                headers: response
                    .headers
                    .into_iter()
                    .map(
                        |header| bindings::webhook::krit::runtime::http_anonymous::Header {
                            name: header.name,
                            value: header.value,
                        },
                    )
                    .collect(),
                body: response.body,
            },
        ))
    }
}

impl bindings::webhook::krit::runtime::ai::Host for HostState {
    fn invoke(
        &mut self,
        adapter: String,
        input: String,
    ) -> wasmtime::Result<Result<String, String>> {
        self.perform_ai(adapter, input)
    }
}

impl bindings::webhook::krit::runtime::logging::Host for HostState {
    fn info(
        &mut self,
        event: String,
        fields: Vec<bindings::webhook::krit::runtime::logging::Field>,
    ) -> wasmtime::Result<Result<(), String>> {
        self.record_log(
            LogLevel::Info,
            event,
            fields
                .into_iter()
                .map(|field| LogField {
                    name: field.name,
                    value: field.value,
                })
                .collect(),
        )
    }

    fn error(
        &mut self,
        event: String,
        fields: Vec<bindings::webhook::krit::runtime::logging::Field>,
    ) -> wasmtime::Result<Result<(), String>> {
        self.record_log(
            LogLevel::Error,
            event,
            fields
                .into_iter()
                .map(|field| LogField {
                    name: field.name,
                    value: field.value,
                })
                .collect(),
        )
    }
}

impl bindings::webhook::krit::runtime::state::Host for HostState {
    fn get(
        &mut self,
        store: String,
        key: String,
    ) -> wasmtime::Result<Result<Option<String>, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        let durable = self.agent_host.durable_state().clone();
        let value = self
            .state
            .get(&durable, &store, &key)
            .map_err(wasmtime::Error::new)?;
        Ok(Ok(value))
    }

    fn put(
        &mut self,
        store: String,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        let durable = self.agent_host.durable_state().clone();
        self.state
            .put(&durable, &store, key, value)
            .map_err(wasmtime::Error::new)?;
        Ok(Ok(()))
    }

    fn delete(&mut self, store: String, key: String) -> wasmtime::Result<Result<(), String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        let durable = self.agent_host.durable_state().clone();
        self.state
            .delete(&durable, &store, key)
            .map_err(wasmtime::Error::new)?;
        Ok(Ok(()))
    }

    fn checkpoint_get(
        &mut self,
        store: String,
        name: String,
    ) -> wasmtime::Result<Result<Option<String>, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        if !krit_capability::is_valid_resource_name(&name) {
            return Ok(Err("workflow checkpoint name is invalid".to_owned()));
        }
        let durable = self.agent_host.durable_state().clone();
        let value = self
            .state
            .checkpoint_get(&durable, &store, &name)
            .map_err(wasmtime::Error::new)?;
        Ok(Ok(value))
    }

    fn checkpoint_put(
        &mut self,
        store: String,
        name: String,
        value: String,
    ) -> wasmtime::Result<Result<(), String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        if !krit_capability::is_valid_resource_name(&name) {
            return Ok(Err("workflow checkpoint name is invalid".to_owned()));
        }
        let durable = self.agent_host.durable_state().clone();
        self.state
            .checkpoint_put(&durable, &store, name, value)
            .map_err(wasmtime::Error::new)?;
        Ok(Ok(()))
    }

    fn replay_http(
        &mut self,
        store: String,
        operation: String,
        origin: String,
        request: bindings::webhook::krit::runtime::state::Request,
    ) -> wasmtime::Result<Result<bindings::webhook::krit::runtime::state::Response, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        self.require_grant("http.request", &origin)?;
        if !krit_capability::is_valid_resource_name(&operation) {
            return Ok(Err("replay operation name is invalid".to_owned()));
        }
        let request = HttpRequest {
            method: request.method,
            path: request.path,
            query: request.query,
            headers: request
                .headers
                .into_iter()
                .map(|header| HttpHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: request.body,
        };
        request
            .validate(self.limits)
            .map_err(wasmtime::Error::new)?;
        if !replay_safe_http(&request, self.agent_host.policy().idempotency.max_key_bytes) {
            return Err(wasmtime::Error::new(RuntimeError::replay(
                "durable HTTP replay requires GET, HEAD, or one valid Idempotency-Key",
            )));
        }
        let durable = self.agent_host.durable_state().clone();
        let binding = self.state.replay_binding(&durable, &store)?;
        let input_digest = replay_http_digest(&origin, &request);
        let owner = self.agent_host.next_lease_owner();
        let decision = binding
            .store()
            .replay_decision(
                krit_state::ReplayRequest {
                    artifact: &self.artifact_identity,
                    kind: krit_state::ReplayKind::Http,
                    operation: &operation,
                    input_digest: &input_digest,
                    owner: &owner,
                    now_millis: wall_clock_millis()?,
                },
                binding.replay_policy(),
            )
            .map_err(state::map_state_error)?;
        let response = match decision {
            krit_state::ReplayDecision::Replay(bytes) => {
                self.state.record_replay(true);
                serde_json::from_slice::<HttpResponse>(&bytes).map_err(|_| {
                    wasmtime::Error::new(RuntimeError::durable_state(
                        "durable HTTP replay result is invalid",
                    ))
                })?
            }
            krit_state::ReplayDecision::Execute(lease) => {
                self.state.record_replay(false);
                let response = match self.perform_http_inner(origin, request, None)? {
                    Ok(response) => response,
                    Err(error) => {
                        binding
                            .store()
                            .abort_replay(&lease)
                            .map_err(state::map_state_error)?;
                        return Ok(Err(error));
                    }
                };
                let bytes = serde_json::to_vec(&response).map_err(|_| {
                    wasmtime::Error::new(RuntimeError::durable_state(
                        "could not encode durable HTTP replay result",
                    ))
                })?;
                binding
                    .store()
                    .complete_replay(
                        &lease,
                        &bytes,
                        wall_clock_millis()?,
                        binding.replay_policy(),
                    )
                    .map_err(state::map_state_error)?;
                response
            }
            krit_state::ReplayDecision::Conflict => {
                return Err(wasmtime::Error::new(RuntimeError::replay(
                    "durable HTTP replay input conflicts with its completed operation",
                )));
            }
            krit_state::ReplayDecision::InProgress => {
                return Err(wasmtime::Error::new(RuntimeError::replay(
                    "durable HTTP replay operation is already in progress",
                )));
            }
        };
        response
            .validate(self.limits)
            .map_err(wasmtime::Error::new)?;
        Ok(Ok(bindings::webhook::krit::runtime::state::Response {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| bindings::webhook::krit::runtime::state::Header {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        }))
    }

    fn replay_ai(
        &mut self,
        store: String,
        operation: String,
        adapter: String,
        input: String,
    ) -> wasmtime::Result<Result<String, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_grant("state.transaction", &store)?;
        self.require_grant("ai.invoke", &adapter)?;
        if !krit_capability::is_valid_resource_name(&operation) {
            return Ok(Err("replay operation name is invalid".to_owned()));
        }
        let Some(config) = self.agent_host.policy().ai_adapters.get(&adapter) else {
            return Ok(Err(format!("AI adapter `{adapter}` is not configured")));
        };
        let configured = ai::Adapter::from_config(config).map_err(wasmtime::Error::new)?;
        if input.len() > configured.max_input_bytes() || input.len() > self.limits.ai_input_bytes()
        {
            return Ok(Err("AI input exceeded the configured size limit".to_owned()));
        }
        if !self
            .agent_host
            .approve(ApprovalOperation::AiInvoke, &adapter)
        {
            return Ok(Err(format!("approval denied for AI adapter `{adapter}`")));
        }
        if configured
            .secret_name()
            .is_some_and(|name| self.agent_host.inputs().secrets().get(name).is_none())
        {
            return Ok(Err(format!(
                "AI adapter `{adapter}` secret is not configured"
            )));
        }
        let durable = self.agent_host.durable_state().clone();
        let binding = self.state.replay_binding(&durable, &store)?;
        let input_digest = replay_ai_digest(&adapter, &input);
        let owner = self.agent_host.next_lease_owner();
        let decision = binding
            .store()
            .replay_decision(
                krit_state::ReplayRequest {
                    artifact: &self.artifact_identity,
                    kind: krit_state::ReplayKind::Ai,
                    operation: &operation,
                    input_digest: &input_digest,
                    owner: &owner,
                    now_millis: wall_clock_millis()?,
                },
                binding.replay_policy(),
            )
            .map_err(state::map_state_error)?;
        match decision {
            krit_state::ReplayDecision::Replay(bytes) => {
                self.state.record_replay(true);
                String::from_utf8(bytes).map(Ok).map_err(|_| {
                    wasmtime::Error::new(RuntimeError::durable_state(
                        "durable AI replay result is not UTF-8",
                    ))
                })
            }
            krit_state::ReplayDecision::Execute(lease) => {
                self.state.record_replay(false);
                let stable_key = stable_replay_idempotency_key(
                    &self.artifact_identity,
                    &store,
                    &operation,
                    &input_digest,
                );
                let result = match self.perform_ai_inner(adapter, input, Some(&stable_key))? {
                    Ok(result) => result,
                    Err(error) => {
                        binding
                            .store()
                            .abort_replay(&lease)
                            .map_err(state::map_state_error)?;
                        return Ok(Err(error));
                    }
                };
                binding
                    .store()
                    .complete_replay(
                        &lease,
                        result.as_bytes(),
                        wall_clock_millis()?,
                        binding.replay_policy(),
                    )
                    .map_err(state::map_state_error)?;
                Ok(Ok(result))
            }
            krit_state::ReplayDecision::Conflict => {
                Err(wasmtime::Error::new(RuntimeError::replay(
                    "durable AI replay input conflicts with its completed operation",
                )))
            }
            krit_state::ReplayDecision::InProgress => Err(wasmtime::Error::new(
                RuntimeError::replay("durable AI replay operation is already in progress"),
            )),
        }
    }
}

impl HostState {
    fn record_host_call(&mut self) -> wasmtime::Result<()> {
        let next = self
            .host_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next > self.host_call_limit {
            return Err(wasmtime::Error::new(HostLimitError::Calls));
        }
        self.host_calls = next;
        Ok(())
    }

    fn record_fallible_host_call(&mut self) -> wasmtime::Result<Option<String>> {
        self.record_host_call()?;
        Ok(self
            .cancellation
            .is_cancelled()
            .then(|| "operation cancelled by embedding host".to_owned()))
    }

    fn require_grant(&self, capability: &str, resource: &str) -> wasmtime::Result<()> {
        if self
            .grants
            .as_ref()
            .is_some_and(|grants| grants.grants(capability, Some(resource)))
            && self
                .requirements
                .contains(&(capability.to_owned(), resource.to_owned()))
        {
            Ok(())
        } else {
            Err(wasmtime::Error::new(RuntimeError::authorization(format!(
                "guest requested ungranted capability `{capability}` for resource `{resource}`"
            ))))
        }
    }

    fn require_effect(&self, capability: &str) -> wasmtime::Result<()> {
        if self
            .grants
            .as_ref()
            .is_some_and(|grants| grants.grants(capability, None))
            && self.effects.contains(capability)
        {
            Ok(())
        } else {
            Err(wasmtime::Error::new(RuntimeError::authorization(format!(
                "guest requested ungranted capability `{capability}`"
            ))))
        }
    }

    fn perform_http(
        &mut self,
        origin: String,
        request: HttpRequest,
        bearer: Option<Resource<SecretHandle>>,
    ) -> wasmtime::Result<Result<HttpResponse, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.perform_http_inner(origin, request, bearer)
    }

    fn perform_http_inner(
        &mut self,
        origin: String,
        request: HttpRequest,
        bearer: Option<Resource<SecretHandle>>,
    ) -> wasmtime::Result<Result<HttpResponse, String>> {
        self.require_grant("http.request", &origin)?;
        let parsed = krit_capability::HttpOrigin::parse_exact(&origin).map_err(|error| {
            wasmtime::Error::new(RuntimeError::authorization(format!(
                "guest requested invalid HTTP origin `{origin}`: {error}"
            )))
        })?;
        let next_http_calls = self
            .http_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next_http_calls > self.limits.http_calls() {
            return Err(wasmtime::Error::new(RuntimeError::host_calls(
                "outbound HTTP call limit exceeded",
            )));
        }
        self.http_calls = next_http_calls;
        request
            .validate(self.limits)
            .map_err(wasmtime::Error::new)?;
        if bearer.is_some()
            && !self
                .agent_host
                .approve(ApprovalOperation::HttpBearer, &origin)
        {
            return Ok(Err(format!(
                "approval denied for bearer HTTP origin `{origin}`"
            )));
        }
        let bearer = bearer
            .as_ref()
            .map(|handle| {
                self.resources
                    .get(handle)
                    .map(|secret| Arc::clone(&secret.bytes))
            })
            .transpose()
            .map_err(|error| wasmtime::Error::msg(format!("invalid secret handle: {error}")))?;
        let retry = self
            .agent_host
            .policy()
            .http_retries
            .get(&origin)
            .copied()
            .unwrap_or(self.agent_host.policy().default_http_retry);
        let rate = self
            .agent_host
            .policy()
            .http_rates
            .get(&origin)
            .copied()
            .unwrap_or(self.agent_host.policy().default_http_rate);
        let approval = bearer
            .is_some()
            .then(|| (ApprovalOperation::HttpBearer, origin.clone()));
        Ok(self.send_with_retry(RetryRequest {
            origin: &parsed,
            request: &request,
            bearer: bearer.as_deref(),
            rate_resource: policy::RateResource::Http(origin),
            rate_policy: rate,
            retry_policy: retry,
            approval,
            approval_prechecked: bearer.is_some(),
            deadline: self.invocation_deadline,
        }))
    }

    fn perform_ai(
        &mut self,
        adapter_name: String,
        input: String,
    ) -> wasmtime::Result<Result<String, String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.perform_ai_inner(adapter_name, input, None)
    }

    fn perform_ai_inner(
        &mut self,
        adapter_name: String,
        input: String,
        stable_idempotency_key: Option<&str>,
    ) -> wasmtime::Result<Result<String, String>> {
        self.require_grant("ai.invoke", &adapter_name)?;
        let next_ai_calls = self
            .ai_calls
            .checked_add(1)
            .ok_or_else(|| wasmtime::Error::new(HostLimitError::Calls))?;
        if next_ai_calls > self.limits.ai_calls() {
            return Err(wasmtime::Error::new(RuntimeError::host_calls(
                "outbound AI call limit exceeded",
            )));
        }
        self.ai_calls = next_ai_calls;
        let Some(config) = self
            .agent_host
            .policy()
            .ai_adapters
            .get(&adapter_name)
            .cloned()
        else {
            return Ok(Err(format!(
                "AI adapter `{adapter_name}` is not configured"
            )));
        };
        let adapter = ai::Adapter::from_config(&config).map_err(wasmtime::Error::new)?;
        if input.len() > adapter.max_input_bytes() || input.len() > self.limits.ai_input_bytes() {
            return Ok(Err("AI input exceeded the configured size limit".to_owned()));
        }
        if !self
            .agent_host
            .approve(ApprovalOperation::AiInvoke, &adapter_name)
        {
            return Ok(Err(format!(
                "approval denied for AI adapter `{adapter_name}`"
            )));
        }
        let bearer = match adapter.secret_name() {
            Some(secret_name) => {
                let Some(secret) = self.agent_host.inputs().secrets().get(secret_name) else {
                    return Ok(Err(format!(
                        "AI adapter `{adapter_name}` secret is not configured"
                    )));
                };
                Some(secret)
            }
            None => None,
        };
        let generated_key;
        let idempotency_key = if let Some(key) = stable_idempotency_key {
            key
        } else {
            generated_key = self
                .agent_host
                .next_ai_idempotency_key(&adapter_name)
                .map_err(wasmtime::Error::new)?;
            &generated_key
        };
        let request = match adapter.build_request(&input, idempotency_key) {
            Ok(request) => request,
            Err(error) => return Ok(Err(error)),
        };
        if let Err(error) = request.validate(self.limits) {
            return Ok(Err(error.message().to_owned()));
        }
        let origin = krit_capability::HttpOrigin::parse_exact(adapter.origin()).map_err(|_| {
            wasmtime::Error::new(RuntimeError::setup(
                "validated AI adapter origin became invalid",
            ))
        })?;
        let retry = self
            .agent_host
            .policy()
            .ai_retries
            .get(&adapter_name)
            .copied()
            .unwrap_or(self.agent_host.policy().default_ai_retry);
        let rate = self
            .agent_host
            .policy()
            .ai_rates
            .get(&adapter_name)
            .copied()
            .unwrap_or(self.agent_host.policy().default_ai_rate);
        let adapter_deadline = Instant::now()
            .checked_add(adapter.timeout())
            .unwrap_or(self.invocation_deadline)
            .min(self.invocation_deadline);
        let response = match self.send_with_retry(RetryRequest {
            origin: &origin,
            request: &request,
            bearer: bearer.as_deref(),
            rate_resource: policy::RateResource::Ai(adapter_name.clone()),
            rate_policy: rate,
            retry_policy: retry,
            approval: Some((ApprovalOperation::AiInvoke, adapter_name)),
            approval_prechecked: true,
            deadline: adapter_deadline,
        }) {
            Ok(response) => response,
            Err(error) => return Ok(Err(error)),
        };
        Ok(adapter.parse_response(response))
    }

    fn record_log(
        &mut self,
        level: LogLevel,
        event: String,
        fields: Vec<LogField>,
    ) -> wasmtime::Result<Result<(), String>> {
        if let Some(error) = self.record_fallible_host_call()? {
            return Ok(Err(error));
        }
        self.require_effect("observe.log")?;
        if self.events.len() >= self.limits.log_events() {
            return Ok(Err(format!(
                "structured log event count exceeds the {}-event limit",
                self.limits.log_events()
            )));
        }
        let sequence = u64::try_from(self.events.len())
            .map_err(|_| wasmtime::Error::new(RuntimeError::resource("too many log events")))?;
        let (event, total) = match observability::validate_and_redact(
            sequence,
            level,
            event,
            fields,
            self.agent_host.inputs().secrets(),
            self.limits,
            self.log_bytes,
        ) {
            Ok(event) => event,
            Err(error) => return Ok(Err(error)),
        };
        self.events.push(event);
        self.log_bytes = total;
        Ok(Ok(()))
    }

    fn send_with_retry(&mut self, retry: RetryRequest<'_>) -> Result<HttpResponse, String> {
        let RetryRequest {
            origin,
            request,
            bearer,
            rate_resource,
            rate_policy,
            retry_policy,
            approval,
            approval_prechecked,
            deadline,
        } = retry;
        let retry_eligible = retry_eligible(request);
        let mut attempt = 0u8;
        loop {
            attempt = attempt.saturating_add(1);
            if self.cancellation.is_cancelled() {
                return Err("operation cancelled by embedding host".to_owned());
            }
            if Instant::now() >= deadline {
                return Err("outbound operation deadline expired".to_owned());
            }
            if let Some((operation, resource)) = &approval
                && (attempt > 1 || !approval_prechecked)
                && !self.agent_host.approve(*operation, resource)
            {
                return Err(format!(
                    "approval denied for `{}` resource `{resource}`",
                    operation.as_str()
                ));
            }
            if let Err(error) = self
                .agent_host
                .check_rate(rate_resource.clone(), rate_policy)
            {
                self.rate_limit_denials = self.rate_limit_denials.saturating_add(1);
                return Err(error);
            }
            if attempt > 1 {
                self.retries = self.retries.saturating_add(1);
            }
            let next_network_attempts = self
                .network_attempts
                .checked_add(1)
                .ok_or_else(|| "outbound HTTP attempt count overflowed".to_owned())?;
            self.network_attempts = next_network_attempts;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let outcome = network::send(
                origin,
                request,
                bearer,
                network::SendContext {
                    policy: self.agent_host.inputs().network_policy(),
                    limits: self.limits,
                    remaining,
                    cancellation: &self.cancellation,
                    active_dns_workers: &self.active_dns_workers,
                },
            );
            match outcome {
                Ok(response) => {
                    if retry_eligible
                        && retryable_status(response.status)
                        && attempt < retry_policy.max_attempts
                    {
                        let delay =
                            retry_delay(retry_policy, attempt, retry_after(&response.headers));
                        self.wait_for_retry(delay, deadline)?;
                        continue;
                    }
                    return Ok(response);
                }
                Err(error)
                    if error.retryable()
                        && retry_eligible
                        && attempt < retry_policy.max_attempts =>
                {
                    self.wait_for_retry(retry_policy.delay(attempt), deadline)?;
                }
                Err(error) => return Err(error.message),
            }
        }
    }

    fn wait_for_retry(&self, delay: Duration, deadline: Instant) -> Result<(), String> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if delay > remaining {
            return Err("retry delay would exceed the outbound operation deadline".to_owned());
        }
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if self.cancellation.is_cancelled() {
                return Err("operation cancelled during retry backoff".to_owned());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("outbound operation deadline expired during retry backoff".to_owned());
            }
            let slice = until
                .saturating_duration_since(now)
                .min(deadline.saturating_duration_since(now))
                .min(Duration::from_millis(10));
            if slice.is_zero() {
                break;
            }
            thread::sleep(slice);
        }
        Ok(())
    }
}

fn retry_eligible(request: &HttpRequest) -> bool {
    if request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD") {
        return true;
    }
    let mut keys = request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("idempotency-key"));
    let Some(key) = keys.next() else {
        return false;
    };
    keys.next().is_none() && policy::valid_idempotency_key(&key.value, MAX_IDEMPOTENCY_KEY_BYTES)
}

fn retryable_status(status: i64) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

fn retry_after(headers: &[HttpHeader]) -> Option<Duration> {
    let mut values = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("retry-after"));
    let value = values.next()?;
    if values.next().is_some() || !value.value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.value.parse::<u64>().ok().map(Duration::from_secs)
}

fn retry_delay(
    policy: RetryPolicy,
    completed_attempts: u8,
    retry_after: Option<Duration>,
) -> Duration {
    retry_after
        .map(|delay| delay.min(policy.max_delay))
        .unwrap_or_default()
        .max(policy.delay(completed_attempts))
}

fn map_wasmtime_error(error: wasmtime::Error) -> RuntimeError {
    if let Some(runtime) = error.downcast_ref::<RuntimeError>() {
        return runtime.clone();
    }
    if let Some(host) = error.downcast_ref::<HostLimitError>() {
        return match host {
            HostLimitError::Calls => RuntimeError::host_calls("host-call limit exceeded"),
            HostLimitError::Output => RuntimeError::output("buffered output-byte limit exceeded"),
        };
    }
    if let Some(trap) = error.downcast_ref::<Trap>() {
        return match trap {
            Trap::OutOfFuel => RuntimeError::fuel("Wasm fuel budget exhausted"),
            Trap::Interrupt => RuntimeError::deadline("Wasm wall deadline exceeded"),
            Trap::StackOverflow
            | Trap::MemoryOutOfBounds
            | Trap::TableOutOfBounds
            | Trap::AllocationTooLarge => {
                RuntimeError::resource(format!("Wasm resource limit exceeded: {trap}"))
            }
            Trap::IntegerDivisionByZero => {
                RuntimeError::guest("K4004", "division or remainder by zero")
            }
            Trap::IntegerOverflow => RuntimeError::guest("K4005", "integer overflow"),
            Trap::UnreachableCodeReached => {
                RuntimeError::guest("K4001", "guest executed an unreachable instruction")
            }
            Trap::IndirectCallToNull | Trap::BadSignature => {
                RuntimeError::guest("K4001", "invalid indirect function call")
            }
            _ => RuntimeError::guest("K4001", format!("guest trapped: {trap}")),
        };
    }
    let message = error.to_string();
    if message.starts_with("resource limit exceeded:")
        || message.starts_with("forcing trap when growing ")
    {
        RuntimeError::resource(message)
    } else {
        RuntimeError::guest("K4001", format!("component execution failed: {message}"))
    }
}

fn replay_safe_http(request: &HttpRequest, max_key_bytes: usize) -> bool {
    if matches!(request.method.as_str(), "GET" | "HEAD") {
        return true;
    }
    let mut keys = request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("idempotency-key"));
    let Some(key) = keys.next() else {
        return false;
    };
    keys.next().is_none() && policy::valid_idempotency_key(&key.value, max_key_bytes)
}

fn replay_http_digest(origin: &str, request: &HttpRequest) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_replay_part(&mut hasher, origin.as_bytes());
    hasher.update(policy::exact_request_digest(request).as_bytes());
    *hasher.finalize().as_bytes()
}

fn replay_ai_digest(adapter: &str, input: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_replay_part(&mut hasher, adapter.as_bytes());
    hash_replay_part(&mut hasher, input.as_bytes());
    *hasher.finalize().as_bytes()
}

fn stable_replay_idempotency_key(
    artifact: &[u8; 32],
    store: &str,
    operation: &str,
    input_digest: &[u8; 32],
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(artifact);
    hash_replay_part(&mut hasher, store.as_bytes());
    hash_replay_part(&mut hasher, operation.as_bytes());
    hasher.update(input_digest);
    format!("krit-replay-{}", hasher.finalize().to_hex())
}

fn hash_replay_part(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn wall_clock_millis() -> wasmtime::Result<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            wasmtime::Error::new(RuntimeError::durable_state(
                "system clock is before the Unix epoch",
            ))
        })?
        .as_millis();
    i64::try_from(millis).map_err(|_| {
        wasmtime::Error::new(RuntimeError::durable_state(
            "system clock exceeds durable timestamp range",
        ))
    })
}

struct DeadlineWorker {
    cancel: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
    expired: Arc<AtomicBool>,
}

impl DeadlineWorker {
    fn start(
        engine: Engine,
        timeout: std::time::Duration,
        active: Arc<AtomicUsize>,
    ) -> Result<Self, RuntimeError> {
        let (cancel, receiver) = mpsc::channel();
        let expired = Arc::new(AtomicBool::new(false));
        let worker_expired = Arc::clone(&expired);
        let handle = thread::Builder::new()
            .name("krit-wasm-deadline".to_owned())
            .spawn(move || {
                active.fetch_add(1, Ordering::AcqRel);
                if matches!(
                    receiver.recv_timeout(timeout),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    worker_expired.store(true, Ordering::Release);
                    engine.increment_epoch();
                }
                active.fetch_sub(1, Ordering::AcqRel);
            })
            .map_err(|error| {
                RuntimeError::deadline(format!("could not start deadline worker: {error}"))
            })?;
        Ok(Self {
            cancel: Some(cancel),
            handle: Some(handle),
            expired,
        })
    }

    fn expired(&self) -> bool {
        self.expired.load(Ordering::Acquire)
    }

    fn finish(mut self) -> Result<bool, RuntimeError> {
        self.stop()?;
        Ok(self.expired())
    }

    fn stop(&mut self) -> Result<(), RuntimeError> {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| RuntimeError::deadline("deadline worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for DeadlineWorker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod phase4_policy_tests {
    use super::*;

    #[test]
    fn retry_after_accepts_one_decimal_value_and_caps_the_schedule() {
        let policy = RetryPolicy {
            max_attempts: 4,
            base_delay: Duration::from_millis(25),
            max_delay: Duration::from_millis(200),
        };
        let headers = vec![HttpHeader {
            name: "retry-after".to_owned(),
            value: "10".to_owned(),
        }];
        assert_eq!(
            retry_delay(policy, 1, retry_after(&headers)),
            Duration::from_millis(200)
        );

        for headers in [
            vec![HttpHeader {
                name: "retry-after".to_owned(),
                value: "Wed, 21 Oct 2015 07:28:00 GMT".to_owned(),
            }],
            vec![
                HttpHeader {
                    name: "retry-after".to_owned(),
                    value: "1".to_owned(),
                },
                HttpHeader {
                    name: "Retry-After".to_owned(),
                    value: "2".to_owned(),
                },
            ],
        ] {
            assert_eq!(retry_after(&headers), None);
            assert_eq!(
                retry_delay(policy, 1, retry_after(&headers)),
                Duration::from_millis(25)
            );
        }
    }
}
