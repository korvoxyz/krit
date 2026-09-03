use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, AiAdapterConfig, ApprovalOperation, BucketDefinition, BucketPolicy,
    DatabaseCatalog, DatabaseDefinition, DatabaseLimits, DatabaseMode, Durability, DurableState,
    DurableStoreDefinition, DurableStoreLimits, ExplicitApprovalPolicy, HostInputs,
    HttpJsonAdapterConfig, IdempotencyPolicy, MAX_CATALOG_STATEMENTS, MAX_DATABASES,
    MAX_PARAMETERS, MAX_RESULT_COLUMNS, ParameterType, QueueDefinition, QueuePolicy,
    RateLimitPolicy, RetentionPolicy, RetryPolicy, RuntimeLimits, ScheduleDefinition,
    SchedulePolicy, SecretStore, StatementKind, StatementRequest,
};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostConfigErrorKind {
    Authorization,
    Input,
}

#[derive(Debug)]
pub(crate) struct HostConfigError {
    kind: HostConfigErrorKind,
    code: &'static str,
    message: String,
}

#[derive(Deserialize)]
struct HostConfigSchema {
    schema: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostConfigV1 {
    schema: u32,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostConfigV2 {
    schema: u32,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
    #[serde(default)]
    ai_adapters: BTreeMap<String, AiAdapterFile>,
    #[serde(default)]
    approvals: Vec<ApprovalFile>,
    #[serde(default)]
    retries: RetriesFile,
    #[serde(default)]
    rate_limits: RateLimitsFile,
    #[serde(default)]
    idempotency: IdempotencyFile,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostConfigV3 {
    schema: u32,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
    #[serde(default)]
    ai_adapters: BTreeMap<String, AiAdapterFile>,
    #[serde(default)]
    approvals: Vec<ApprovalFile>,
    #[serde(default)]
    retries: RetriesFile,
    #[serde(default)]
    rate_limits: RateLimitsFile,
    #[serde(default)]
    idempotency: IdempotencyFile,
    state: StateFile,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostConfigV4 {
    schema: u32,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
    #[serde(default)]
    ai_adapters: BTreeMap<String, AiAdapterFile>,
    #[serde(default)]
    approvals: Vec<ApprovalFile>,
    #[serde(default)]
    retries: RetriesFile,
    #[serde(default)]
    rate_limits: RateLimitsFile,
    #[serde(default)]
    idempotency: IdempotencyFile,
    state: StateFile,
    #[serde(default)]
    jobs: JobsFile,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostConfigV5 {
    schema: u32,
    #[serde(default)]
    config: BTreeMap<String, String>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretReference>,
    #[serde(default)]
    ai_adapters: BTreeMap<String, AiAdapterFile>,
    #[serde(default)]
    approvals: Vec<ApprovalFile>,
    #[serde(default)]
    retries: RetriesFile,
    #[serde(default)]
    rate_limits: RateLimitsFile,
    #[serde(default)]
    idempotency: IdempotencyFile,
    state: StateFile,
    #[serde(default)]
    jobs: JobsFile,
    #[serde(default)]
    databases: BTreeMap<String, DatabaseFile>,
    #[serde(default = "default_max_transactions")]
    max_transactions_per_invocation: usize,
}

const fn default_max_transactions() -> usize {
    1
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DatabaseFile {
    path: PathBuf,
    mode: DatabaseModeFile,
    busy_timeout_ms: u64,
    max_database_bytes: u64,
    max_transaction_millis: u64,
    max_operations_per_transaction: usize,
    max_parameter_bytes: usize,
    max_rows: usize,
    max_columns: usize,
    max_result_bytes: usize,
    statements: BTreeMap<String, StatementFile>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DatabaseModeFile {
    ReadOnly,
    ReadWrite,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatementFile {
    kind: StatementKindFile,
    sql: String,
    #[serde(default)]
    parameters: Vec<ParameterTypeFile>,
    #[serde(default)]
    columns: Vec<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum StatementKindFile {
    Query,
    Execute,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ParameterTypeFile {
    Text,
    Integer,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JobsFile {
    #[serde(default)]
    queues: BTreeMap<String, QueueFile>,
    #[serde(default)]
    schedules: BTreeMap<String, ScheduleFile>,
    #[serde(default)]
    buckets: BTreeMap<String, BucketFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueueFile {
    store: String,
    max_depth: usize,
    max_job_bytes: usize,
    max_queue_bytes: usize,
    max_attempts: u32,
    lease_seconds: u64,
    backoff_seconds: u64,
    max_backoff_seconds: u64,
    dead_letter_max_entries: usize,
    dead_letter_retention_seconds: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleFile {
    store: String,
    interval_seconds: u64,
    start_epoch_millis: i64,
    max_catch_up: u32,
    max_attempts: u32,
    lease_seconds: u64,
    backoff_seconds: u64,
    max_backoff_seconds: u64,
    retention_seconds: u64,
    max_retained_fires: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BucketFile {
    store: String,
    max_objects: usize,
    max_key_bytes: usize,
    max_object_bytes: usize,
    max_bucket_bytes: usize,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateFile {
    #[serde(default)]
    stores: BTreeMap<String, StateStoreFile>,
    #[serde(default)]
    durable_idempotency_store: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StateStoreFile {
    path: PathBuf,
    durability: DurabilityFile,
    busy_timeout_ms: u64,
    max_operations: usize,
    max_key_bytes: usize,
    max_value_bytes: usize,
    max_transaction_bytes: usize,
    max_database_bytes: u64,
    max_replay_entries: usize,
    max_replay_bytes: usize,
    replay_ttl_seconds: u64,
    lease_seconds: u64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DurabilityFile {
    Full,
    Normal,
}

const MAX_STATE_STORES: usize = 16;
const MAX_STATE_OPERATIONS: usize = 1024;
const MAX_STATE_KEY_BYTES: usize = 4 * 1024;
const MAX_STATE_VALUE_BYTES: usize = 1024 * 1024;
const MAX_STATE_TRANSACTION_BYTES: usize = 16 * 1024 * 1024;
const MAX_STATE_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;
/// Smallest budget that can hold the strict schema-2 store.
const MIN_STATE_DATABASE_BYTES: u64 = krit_runtime::MINIMUM_DATABASE_BYTES;
const MAX_STATE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REPLAY_ENTRIES: usize = 65_536;
const MAX_REPLAY_BYTES: usize = 256 * 1024 * 1024;
const MAX_REPLAY_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MAX_REPLAY_LEASE: Duration = Duration::from_secs(5 * 60);
const MAX_QUEUES: usize = 16;
const MAX_SCHEDULES: usize = 16;
const MAX_BUCKETS: usize = 16;
const MAX_QUEUE_DEPTH: usize = 65_536;
const MAX_QUEUE_JOB_BYTES: usize = 1024 * 1024;
const MAX_QUEUE_BYTES: usize = 256 * 1024 * 1024;
const MAX_DELIVERY_ATTEMPTS: u32 = 16;
const MAX_DELIVERY_LEASE: Duration = Duration::from_secs(5 * 60);
const MAX_DELIVERY_BACKOFF: Duration = Duration::from_secs(60 * 60);
const MAX_DEAD_LETTER_ENTRIES: usize = 4096;
const MAX_DEAD_LETTER_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MIN_SCHEDULE_INTERVAL: Duration = Duration::from_secs(1);
const MAX_SCHEDULE_INTERVAL: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const MAX_SCHEDULE_CATCH_UP: u32 = 64;
const MAX_SCHEDULE_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MAX_RETAINED_FIRES: usize = 4096;
const MAX_BUCKET_OBJECTS: usize = 65_536;
const MAX_OBJECT_KEY_BYTES: usize = 1024;
const MAX_OBJECT_BYTES: usize = 4 * 1024 * 1024;
const MAX_BUCKET_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretReference {
    file: PathBuf,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum AiAdapterFile {
    HttpJson {
        origin: String,
        path: String,
        model: String,
        #[serde(default)]
        secret: Option<String>,
        #[serde(rename = "maxInputBytes")]
        max_input_bytes: usize,
        #[serde(rename = "maxResponseBytes")]
        max_response_bytes: usize,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u64,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalFile {
    operation: ApprovalOperationFile,
    resource: String,
}

#[derive(Clone, Copy, Deserialize)]
enum ApprovalOperationFile {
    #[serde(rename = "ai.invoke")]
    AiInvoke,
    #[serde(rename = "http.bearer")]
    HttpBearer,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryPolicyFile {
    #[serde(default = "default_retry_attempts")]
    max_attempts: u8,
    #[serde(default = "default_retry_base_ms")]
    base_delay_ms: u64,
    #[serde(default = "default_retry_max_ms")]
    max_delay_ms: u64,
}

impl Default for RetryPolicyFile {
    fn default() -> Self {
        Self {
            max_attempts: default_retry_attempts(),
            base_delay_ms: default_retry_base_ms(),
            max_delay_ms: default_retry_max_ms(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetriesFile {
    #[serde(default)]
    default_http: RetryPolicyFile,
    #[serde(default)]
    default_ai: RetryPolicyFile,
    #[serde(default)]
    http: BTreeMap<String, RetryPolicyFile>,
    #[serde(default)]
    ai: BTreeMap<String, RetryPolicyFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RatePolicyFile {
    capacity: u32,
    window_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RateLimitsFile {
    #[serde(default = "default_http_rate")]
    default_http: RatePolicyFile,
    #[serde(default = "default_ai_rate")]
    default_ai: RatePolicyFile,
    #[serde(default)]
    http: BTreeMap<String, RatePolicyFile>,
    #[serde(default)]
    ai: BTreeMap<String, RatePolicyFile>,
    #[serde(default = "default_max_tracked_resources")]
    max_tracked_resources: usize,
}

impl Default for RateLimitsFile {
    fn default() -> Self {
        Self {
            default_http: default_http_rate(),
            default_ai: default_ai_rate(),
            http: BTreeMap::new(),
            ai: BTreeMap::new(),
            max_tracked_resources: default_max_tracked_resources(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdempotencyFile {
    #[serde(default = "default_idempotency_entries")]
    max_entries: usize,
    #[serde(default = "default_idempotency_bytes")]
    max_bytes: usize,
    #[serde(default = "default_idempotency_ttl_ms")]
    ttl_ms: u64,
    #[serde(default = "default_idempotency_key_bytes")]
    max_key_bytes: usize,
}

impl Default for IdempotencyFile {
    fn default() -> Self {
        Self {
            max_entries: default_idempotency_entries(),
            max_bytes: default_idempotency_bytes(),
            ttl_ms: default_idempotency_ttl_ms(),
            max_key_bytes: default_idempotency_key_bytes(),
        }
    }
}

impl HostConfigError {
    pub(crate) const fn kind(&self) -> HostConfigErrorKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) const fn code(&self) -> &'static str {
        self.code
    }

    fn input(message: impl Into<String>) -> Self {
        Self {
            kind: HostConfigErrorKind::Input,
            code: "K7003",
            message: message.into(),
        }
    }

    fn authorization(message: impl Into<String>) -> Self {
        Self {
            kind: HostConfigErrorKind::Authorization,
            code: "K5001",
            message: message.into(),
        }
    }

    fn durable(error: krit_runtime::RuntimeError) -> Self {
        Self {
            kind: HostConfigErrorKind::Input,
            code: error.code(),
            message: error.message().to_owned(),
        }
    }
}

pub(crate) fn load(
    path: Option<&Path>,
    manifest: &Manifest,
    limits: RuntimeLimits,
) -> Result<AgentHost, HostConfigError> {
    let Some(path) = path else {
        let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
            .map_err(|error| HostConfigError::input(error.message()))?;
        return AgentHost::from_inputs(inputs)
            .map_err(|error| HostConfigError::input(error.message()));
    };
    let bytes = read_bounded(path, limits.host_config_bytes()).map_err(|error| {
        HostConfigError::input(format!(
            "could not read host config {}: {error}",
            path.display()
        ))
    })?;
    let schema: HostConfigSchema = serde_json::from_slice(&bytes).map_err(|error| {
        HostConfigError::input(format!(
            "invalid strict host config JSON {}: {error}",
            path.display()
        ))
    })?;
    match schema.schema {
        1 => {
            let file: HostConfigV1 = serde_json::from_slice(&bytes).map_err(|error| {
                HostConfigError::input(format!(
                    "invalid strict schema-1 host config {}: {error}",
                    path.display()
                ))
            })?;
            debug_assert_eq!(file.schema, 1);
            let inputs = load_inputs(path, manifest, limits, file.config, file.secrets)?;
            AgentHost::from_inputs(inputs)
                .map_err(|error| HostConfigError::input(error.message().to_owned()))
        }
        2 => {
            let file: HostConfigV2 = serde_json::from_slice(&bytes).map_err(|error| {
                HostConfigError::input(format!(
                    "invalid strict schema-2 host config {}: {error}",
                    path.display()
                ))
            })?;
            debug_assert_eq!(file.schema, 2);
            load_configured_host(path, manifest, limits, file, || {
                Ok((DurableState::default(), DatabaseCatalog::default()))
            })
        }
        3 => {
            let file: HostConfigV3 = serde_json::from_slice(&bytes).map_err(|error| {
                HostConfigError::input(format!(
                    "invalid strict schema-3 host config {}: {error}",
                    path.display()
                ))
            })?;
            debug_assert_eq!(file.schema, 3);
            let resolved = resolve_durable_state(path, manifest, file.state, &BTreeSet::new())?;
            let compatible = HostConfigV2 {
                schema: 2,
                config: file.config,
                secrets: file.secrets,
                ai_adapters: file.ai_adapters,
                approvals: file.approvals,
                retries: file.retries,
                rate_limits: file.rate_limits,
                idempotency: file.idempotency,
            };
            load_configured_host(path, manifest, limits, compatible, || {
                Ok((open_durable_state(resolved)?, DatabaseCatalog::default()))
            })
        }
        4 => {
            let file: HostConfigV4 = serde_json::from_slice(&bytes).map_err(|error| {
                HostConfigError::input(format!(
                    "invalid strict schema-4 host config {}: {error}",
                    path.display()
                ))
            })?;
            debug_assert_eq!(file.schema, 4);
            let configured_stores = file.state.stores.keys().cloned().collect();
            let jobs = validate_jobs(manifest, file.jobs, &configured_stores)?;
            let host_owned = jobs.store_names();
            let resolved = resolve_durable_state(path, manifest, file.state, &host_owned)?;
            let compatible = HostConfigV2 {
                schema: 2,
                config: file.config,
                secrets: file.secrets,
                ai_adapters: file.ai_adapters,
                approvals: file.approvals,
                retries: file.retries,
                rate_limits: file.rate_limits,
                idempotency: file.idempotency,
            };
            load_configured_host(path, manifest, limits, compatible, || {
                let durable = open_durable_state(resolved)?
                    .with_jobs(jobs.queues, jobs.schedules, jobs.buckets)
                    .map_err(HostConfigError::durable)?;
                Ok((durable, DatabaseCatalog::default()))
            })
        }
        5 => {
            let file: HostConfigV5 = serde_json::from_slice(&bytes).map_err(|error| {
                HostConfigError::input(format!(
                    "invalid strict schema-5 host config {}: {error}",
                    path.display()
                ))
            })?;
            debug_assert_eq!(file.schema, 5);
            // Every database and job definition is resolved and validated
            // before any store or application database is created or opened.
            let prepared = validate_databases(manifest, &file.databases)?;
            let configured_stores = file.state.stores.keys().cloned().collect();
            let jobs = validate_jobs(manifest, file.jobs, &configured_stores)?;
            let host_owned = jobs.store_names();
            let resolved = resolve_durable_state(path, manifest, file.state, &host_owned)?;
            let root = config_root(path)?;
            reject_aliased_paths(&resolved.definitions, &prepared, &root)?;
            let max_transactions = file.max_transactions_per_invocation;
            let compatible = HostConfigV2 {
                schema: 2,
                config: file.config,
                secrets: file.secrets,
                ai_adapters: file.ai_adapters,
                approvals: file.approvals,
                retries: file.retries,
                rate_limits: file.rate_limits,
                idempotency: file.idempotency,
            };
            load_configured_host(path, manifest, limits, compatible, || {
                // Application databases open first. Opening one never creates a
                // file, so a catalog or live-schema failure cannot leave a
                // freshly created or migrated durable state store behind.
                let databases = open_databases(path, prepared, max_transactions)?;
                let durable = open_durable_state(resolved)?
                    .with_jobs(jobs.queues, jobs.schedules, jobs.buckets)
                    .map_err(HostConfigError::durable)?;
                Ok((durable, databases))
            })
        }
        unsupported => Err(HostConfigError::input(format!(
            "unsupported host config schema {unsupported}; expected 1, 2, 3, 4, or 5"
        ))),
    }
}

/// Validates every host policy and only then performs the durable side effects.
///
/// `open_durable` is a thunk deliberately: config inputs, secrets, AI adapters,
/// retries, rate limits, idempotency, and approvals are all validated *before*
/// any durable store or application database is created, opened, or migrated.
/// An invalid policy therefore leaves the filesystem untouched.
fn load_configured_host(
    path: &Path,
    manifest: &Manifest,
    limits: RuntimeLimits,
    file: HostConfigV2,
    open_durable: impl FnOnce() -> Result<(DurableState, DatabaseCatalog), HostConfigError>,
) -> Result<AgentHost, HostConfigError> {
    let inputs = load_inputs(path, manifest, limits, file.config, file.secrets)?;
    let mut policy = AgentHostPolicy::default();
    for (name, adapter) in file.ai_adapters {
        require_manifest_ai(manifest, &name)?;
        let adapter = match adapter {
            AiAdapterFile::HttpJson {
                origin,
                path,
                model,
                secret,
                max_input_bytes,
                max_response_bytes,
                timeout_ms,
            } => {
                require_manifest_http(manifest, &origin)?;
                if let Some(secret) = &secret {
                    require_manifest_secret(manifest, secret)?;
                }
                AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
                    origin,
                    path,
                    model,
                    secret,
                    max_input_bytes,
                    max_response_bytes,
                    timeout: milliseconds(timeout_ms, "AI adapter timeout")?,
                })
            }
        };
        policy.ai_adapters.insert(name, adapter);
    }
    policy.default_http_retry = retry_policy(file.retries.default_http)?;
    policy.default_ai_retry = retry_policy(file.retries.default_ai)?;
    for (origin, retry) in file.retries.http {
        require_manifest_http(manifest, &origin)?;
        policy.http_retries.insert(origin, retry_policy(retry)?);
    }
    for (adapter, retry) in file.retries.ai {
        require_manifest_ai(manifest, &adapter)?;
        policy.ai_retries.insert(adapter, retry_policy(retry)?);
    }
    policy.default_http_rate = rate_policy(file.rate_limits.default_http)?;
    policy.default_ai_rate = rate_policy(file.rate_limits.default_ai)?;
    for (origin, rate) in file.rate_limits.http {
        require_manifest_http(manifest, &origin)?;
        policy.http_rates.insert(origin, rate_policy(rate)?);
    }
    for (adapter, rate) in file.rate_limits.ai {
        require_manifest_ai(manifest, &adapter)?;
        policy.ai_rates.insert(adapter, rate_policy(rate)?);
    }
    policy.max_tracked_resources = file.rate_limits.max_tracked_resources;
    policy.idempotency = IdempotencyPolicy {
        max_entries: file.idempotency.max_entries,
        max_bytes: file.idempotency.max_bytes,
        ttl: milliseconds(file.idempotency.ttl_ms, "idempotency TTL")?,
        max_key_bytes: file.idempotency.max_key_bytes,
    };

    let mut approvals = Vec::new();
    for approval in file.approvals {
        let operation = match approval.operation {
            ApprovalOperationFile::AiInvoke => {
                require_manifest_ai(manifest, &approval.resource)?;
                ApprovalOperation::AiInvoke
            }
            ApprovalOperationFile::HttpBearer => {
                require_manifest_http(manifest, &approval.resource)?;
                ApprovalOperation::HttpBearer
            }
        };
        approvals.push((operation, approval.resource));
    }
    let approvals = ExplicitApprovalPolicy::new(approvals)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))?;
    // The assembled policy is validated here rather than inside the host
    // constructor, so an invalid retry, rate, or idempotency bound is rejected
    // before anything durable is touched.
    krit_runtime::validate_policy(&policy)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))?;
    // Everything above this line is pure validation; the durable side effects
    // happen only once the whole configuration is known to be valid.
    let (durable, databases) = open_durable()?;
    AgentHost::new_with_resources(inputs, policy, Arc::new(approvals), durable, databases)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))
}

/// Fully validated job bindings.
///
/// Building this value performs no I/O, so an invalid `jobs` section is
/// rejected before any database is created, opened, or migrated.
struct PreparedJobs {
    queues: BTreeMap<String, QueueDefinition>,
    schedules: BTreeMap<String, ScheduleDefinition>,
    buckets: BTreeMap<String, BucketDefinition>,
}

impl PreparedJobs {
    /// Stores that back at least one job resource and are therefore host-owned.
    fn store_names(&self) -> BTreeSet<String> {
        self.queues
            .values()
            .map(|definition| definition.store.clone())
            .chain(
                self.schedules
                    .values()
                    .map(|definition| definition.store.clone()),
            )
            .chain(
                self.buckets
                    .values()
                    .map(|definition| definition.store.clone()),
            )
            .collect()
    }
}

/// Validates every queue, schedule, and bucket definition, grant, limit, and
/// store reference. Purely a parsing and checking step.
fn validate_jobs(
    manifest: &Manifest,
    jobs: JobsFile,
    configured_stores: &BTreeSet<String>,
) -> Result<PreparedJobs, HostConfigError> {
    if jobs.queues.len() > MAX_QUEUES
        || jobs.schedules.len() > MAX_SCHEDULES
        || jobs.buckets.len() > MAX_BUCKETS
    {
        return Err(HostConfigError::input(
            "configured queues, schedules, or buckets exceed the Phase 6 bounds",
        ));
    }
    let require_store = |store: &str| -> Result<(), HostConfigError> {
        if configured_stores.contains(store) {
            Ok(())
        } else {
            Err(HostConfigError::input(
                "durable job resource names a store that `state.stores` does not configure",
            ))
        }
    };
    let mut queues = BTreeMap::new();
    for (name, file) in jobs.queues {
        if !manifest.grants_permission("queue.publish", Some(&name))
            && !manifest.grants_permission("queue.consume", Some(&name))
        {
            return Err(HostConfigError::authorization(format!(
                "durable queue `{name}` is not granted by the package manifest"
            )));
        }
        require_store(&file.store)?;
        let lease = seconds(file.lease_seconds, "queue lease")?;
        let backoff = seconds(file.backoff_seconds, "queue backoff")?;
        let max_backoff = seconds(file.max_backoff_seconds, "queue maximum backoff")?;
        let dead_letter_ttl = seconds(file.dead_letter_retention_seconds, "dead-letter retention")?;
        if file.max_depth == 0
            || file.max_depth > MAX_QUEUE_DEPTH
            || file.max_job_bytes == 0
            || file.max_job_bytes > MAX_QUEUE_JOB_BYTES
            || file.max_queue_bytes < file.max_job_bytes
            || file.max_queue_bytes > MAX_QUEUE_BYTES
            || file.max_attempts == 0
            || file.max_attempts > MAX_DELIVERY_ATTEMPTS
            || lease.is_zero()
            || lease > MAX_DELIVERY_LEASE
            || backoff.is_zero()
            || backoff > max_backoff
            || max_backoff > MAX_DELIVERY_BACKOFF
            || file.dead_letter_max_entries == 0
            || file.dead_letter_max_entries > MAX_DEAD_LETTER_ENTRIES
            || dead_letter_ttl.is_zero()
            || dead_letter_ttl > MAX_DEAD_LETTER_RETENTION
        {
            return Err(HostConfigError::input(
                "durable queue limits exceed the Phase 6 bounds",
            ));
        }
        queues.insert(
            name,
            QueueDefinition {
                store: file.store,
                policy: QueuePolicy {
                    max_depth: file.max_depth,
                    max_job_bytes: file.max_job_bytes,
                    max_queue_bytes: file.max_queue_bytes,
                    max_attempts: file.max_attempts,
                    lease,
                    backoff,
                    max_backoff,
                    dead_letter_max_entries: file.dead_letter_max_entries,
                    dead_letter_ttl,
                },
            },
        );
    }
    let mut schedules = BTreeMap::new();
    for (name, file) in jobs.schedules {
        if !manifest.grants_permission("schedule.trigger", Some(&name)) {
            return Err(HostConfigError::authorization(format!(
                "durable schedule `{name}` is not granted by the package manifest"
            )));
        }
        require_store(&file.store)?;
        let interval = seconds(file.interval_seconds, "schedule interval")?;
        let lease = seconds(file.lease_seconds, "schedule lease")?;
        let backoff = seconds(file.backoff_seconds, "schedule backoff")?;
        let max_backoff = seconds(file.max_backoff_seconds, "schedule maximum backoff")?;
        let retention = seconds(file.retention_seconds, "schedule retention")?;
        if interval < MIN_SCHEDULE_INTERVAL
            || interval > MAX_SCHEDULE_INTERVAL
            || file.start_epoch_millis < 0
            || file.max_catch_up == 0
            || file.max_catch_up > MAX_SCHEDULE_CATCH_UP
            || file.max_attempts == 0
            || file.max_attempts > MAX_DELIVERY_ATTEMPTS
            || lease.is_zero()
            || lease > MAX_DELIVERY_LEASE
            || backoff.is_zero()
            || backoff > max_backoff
            || max_backoff > MAX_DELIVERY_BACKOFF
            || retention.is_zero()
            || retention > MAX_SCHEDULE_RETENTION
            || file.max_retained_fires == 0
            || file.max_retained_fires > MAX_RETAINED_FIRES
        {
            return Err(HostConfigError::input(
                "durable schedule limits exceed the Phase 6 bounds",
            ));
        }
        schedules.insert(
            name,
            ScheduleDefinition {
                store: file.store,
                policy: SchedulePolicy {
                    interval,
                    start_millis: file.start_epoch_millis,
                    max_catch_up: file.max_catch_up,
                    max_attempts: file.max_attempts,
                    lease,
                    backoff,
                    max_backoff,
                    retention,
                    max_retained_fires: file.max_retained_fires,
                },
            },
        );
    }
    let mut buckets = BTreeMap::new();
    for (name, file) in jobs.buckets {
        if !manifest.grants_permission("object.read", Some(&name))
            && !manifest.grants_permission("object.write", Some(&name))
        {
            return Err(HostConfigError::authorization(format!(
                "object bucket `{name}` is not granted by the package manifest"
            )));
        }
        require_store(&file.store)?;
        if file.max_objects == 0
            || file.max_objects > MAX_BUCKET_OBJECTS
            || file.max_key_bytes == 0
            || file.max_key_bytes > MAX_OBJECT_KEY_BYTES
            || file.max_object_bytes == 0
            || file.max_object_bytes > MAX_OBJECT_BYTES
            || file.max_bucket_bytes < file.max_object_bytes
            || u64::try_from(file.max_bucket_bytes).unwrap_or(u64::MAX) > MAX_BUCKET_BYTES
        {
            return Err(HostConfigError::input(
                "object bucket limits exceed the Phase 6 bounds",
            ));
        }
        buckets.insert(
            name,
            BucketDefinition {
                store: file.store,
                policy: BucketPolicy {
                    max_objects: file.max_objects,
                    max_key_bytes: file.max_key_bytes,
                    max_object_bytes: file.max_object_bytes,
                    max_bucket_bytes: file.max_bucket_bytes,
                },
            },
        );
    }
    Ok(PreparedJobs {
        queues,
        schedules,
        buckets,
    })
}

/// Canonical directory that every configured relative path resolves against.
fn config_root(host_config_path: &Path) -> Result<PathBuf, HostConfigError> {
    host_config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|_| HostConfigError::input("host config directory is not accessible"))
}

/// Fully resolved durable state, before any file is created or migrated.
struct ResolvedState {
    definitions: BTreeMap<String, DurableStoreDefinition>,
    idempotency_store: Option<String>,
}

/// Resolves and validates every durable store without touching the filesystem
/// beyond reading what already exists.
fn resolve_durable_state(
    host_config_path: &Path,
    manifest: &Manifest,
    state: StateFile,
    host_owned: &BTreeSet<String>,
) -> Result<ResolvedState, HostConfigError> {
    if state.stores.len() > MAX_STATE_STORES {
        return Err(HostConfigError::input(format!(
            "too many durable stores; maximum is {MAX_STATE_STORES}"
        )));
    }
    let root = config_root(host_config_path)?;
    let mut definitions = BTreeMap::new();
    let mut paths = BTreeMap::<PathBuf, String>::new();
    // Pass one validates and resolves every store without creating a file.
    for (name, file) in state.stores {
        if !host_owned.contains(&name) {
            require_manifest_state(manifest, &name)?;
        }
        validate_state_store_file(&file)?;
        validate_relative_state_path(&file.path)?;
        let database = resolve_state_database_path(&root, &file.path, file.max_database_bytes)?;
        if let Some(previous) = paths.insert(database.clone(), name.clone()) {
            return Err(HostConfigError::input(format!(
                "durable stores `{previous}` and `{name}` use the same database path"
            )));
        }
        let busy_timeout = milliseconds(file.busy_timeout_ms, "state busy timeout")?;
        let replay_ttl = seconds(file.replay_ttl_seconds, "state replay TTL")?;
        let lease = seconds(file.lease_seconds, "state replay lease")?;
        definitions.insert(
            name,
            DurableStoreDefinition {
                path: database,
                durability: match file.durability {
                    DurabilityFile::Full => Durability::Full,
                    DurabilityFile::Normal => Durability::Normal,
                },
                limits: DurableStoreLimits {
                    busy_timeout,
                    max_operations: file.max_operations,
                    max_key_bytes: file.max_key_bytes,
                    max_value_bytes: file.max_value_bytes,
                    max_transaction_bytes: file.max_transaction_bytes,
                    max_database_bytes: file.max_database_bytes,
                    max_replay_entries: file.max_replay_entries,
                    max_replay_bytes: file.max_replay_bytes,
                },
                replay: RetentionPolicy {
                    max_entries: file.max_replay_entries,
                    max_bytes: file.max_replay_bytes,
                    ttl: replay_ttl,
                    lease,
                },
            },
        );
    }
    if let Some(name) = &state.durable_idempotency_store {
        require_manifest_state(manifest, name)?;
        if !definitions.contains_key(name) {
            return Err(HostConfigError::input(
                "durable idempotency store is not present in `state.stores`",
            ));
        }
    }
    Ok(ResolvedState {
        definitions,
        idempotency_store: state.durable_idempotency_store,
    })
}

/// Creates, opens, validates, and migrates every resolved durable store.
///
/// This is the first mutating step in loading a host configuration and runs
/// only after every definition, limit, grant, and policy has been validated.
fn open_durable_state(resolved: ResolvedState) -> Result<DurableState, HostConfigError> {
    for definition in resolved.definitions.values() {
        create_state_database_file(&definition.path)?;
    }
    DurableState::open(resolved.definitions, resolved.idempotency_store)
        .map_err(HostConfigError::durable)
}

/// Rejects any durable store and application database that would share a file.
///
/// A `database.write` grant must never reach Krit's own internal schema, and an
/// application database must never be silently migrated by the state store. The
/// comparison uses the canonical path plus, on Unix, the device and inode so
/// that relative aliases, case aliases on case-insensitive filesystems, and
/// hard links are all caught.
fn reject_aliased_paths(
    state: &BTreeMap<String, DurableStoreDefinition>,
    databases: &[PreparedDatabase],
    root: &Path,
) -> Result<(), HostConfigError> {
    let mut seen = BTreeMap::<FileIdentity, String>::new();
    for definition in state.values() {
        for identity in file_identities(&definition.path) {
            if seen.insert(identity, "durable state".to_owned()).is_some() {
                return Err(HostConfigError::input(
                    "two durable stores resolve to the same file",
                ));
            }
        }
    }
    for database in databases {
        let path = root.join(&database.relative_path);
        for identity in file_identities(&path) {
            if let Some(previous) = seen.insert(identity, "application database".to_owned())
                && previous == "durable state"
            {
                return Err(HostConfigError::input(
                    "an application database and a durable state store resolve to the same file; \
                     Krit's internal schema is never reachable through `database` grants",
                ));
            }
        }
    }
    Ok(())
}

/// Strongest available identity for one file path.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum FileIdentity {
    /// Device and inode: catches hard links and every path alias.
    Unix(u64, u64),
    /// Canonical path: the strongest portable identity.
    Canonical(PathBuf),
    /// Lowercased canonical path, for case-insensitive filesystems.
    CaseFolded(String),
}

/// Every identity a path is known by. A collision on any one is a collision.
fn file_identities(path: &Path) -> Vec<FileIdentity> {
    let mut identities = Vec::new();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    identities.push(FileIdentity::Canonical(canonical.clone()));
    if let Some(text) = canonical.to_str() {
        let folded = text.to_lowercase();
        if folded != text {
            // Only meaningful when the filesystem folds case; an extra
            // identity is harmless because it is compared against other
            // folded identities only.
            identities.push(FileIdentity::CaseFolded(folded));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if let Ok(metadata) = fs::metadata(&canonical) {
            identities.push(FileIdentity::Unix(metadata.dev(), metadata.ino()));
        }
    }
    identities
}

/// One validated database definition awaiting its file path.
struct PreparedDatabase {
    name: String,
    relative_path: PathBuf,
    mode: DatabaseMode,
    limits: DatabaseLimits,
    statements: BTreeMap<String, StatementRequest>,
}

/// Validates every database definition, grant, and limit with no I/O.
fn validate_databases(
    manifest: &Manifest,
    databases: &BTreeMap<String, DatabaseFile>,
) -> Result<Vec<PreparedDatabase>, HostConfigError> {
    if databases.len() > MAX_DATABASES {
        return Err(HostConfigError::input(
            "configured application databases exceed the Phase 7 bound",
        ));
    }
    let mut prepared = Vec::with_capacity(databases.len());
    for (name, file) in databases {
        let mode = match file.mode {
            DatabaseModeFile::ReadOnly => DatabaseMode::ReadOnly,
            DatabaseModeFile::ReadWrite => DatabaseMode::ReadWrite,
        };
        let granted = match mode {
            DatabaseMode::ReadOnly => manifest.grants_permission("database.read", Some(name)),
            DatabaseMode::ReadWrite => manifest.grants_permission("database.write", Some(name)),
        };
        if !granted {
            return Err(HostConfigError::authorization(format!(
                "application database `{name}` is not granted by the package manifest"
            )));
        }
        validate_relative_database_path(&file.path)?;
        let busy_timeout = milliseconds(file.busy_timeout_ms, "database busy timeout")?;
        let max_transaction_duration =
            milliseconds(file.max_transaction_millis, "database transaction bound")?;
        if file.statements.is_empty() || file.statements.len() > MAX_CATALOG_STATEMENTS {
            return Err(HostConfigError::input(
                "application database must declare 1..=64 catalog statements",
            ));
        }
        let database_limits = DatabaseLimits {
            busy_timeout,
            max_database_bytes: file.max_database_bytes,
            max_transaction_duration,
            max_operations_per_transaction: file.max_operations_per_transaction,
            max_parameter_bytes: file.max_parameter_bytes,
            max_rows: file.max_rows,
            max_columns: file.max_columns,
            max_result_bytes: file.max_result_bytes,
        };
        // Rejected here, in the pure phase, rather than at open time.
        database_limits
            .validate()
            .map_err(|error| HostConfigError::input(error.message().to_owned()))?;
        let mut statements = BTreeMap::new();
        for (statement_name, statement) in &file.statements {
            if !krit_capability::is_valid_resource_name(statement_name) {
                return Err(HostConfigError::input(
                    "database statement name must use the canonical resource grammar",
                ));
            }
            if statement.parameters.len() > MAX_PARAMETERS
                || statement.columns.len() > MAX_RESULT_COLUMNS
            {
                return Err(HostConfigError::input(
                    "database statement parameters or columns exceed the Phase 7 bounds",
                ));
            }
            statements.insert(
                statement_name.clone(),
                StatementRequest {
                    kind: match statement.kind {
                        StatementKindFile::Query => StatementKind::Query,
                        StatementKindFile::Execute => StatementKind::Execute,
                    },
                    sql: statement.sql.clone(),
                    parameters: statement
                        .parameters
                        .iter()
                        .map(|parameter| match parameter {
                            ParameterTypeFile::Text => ParameterType::Text,
                            ParameterTypeFile::Integer => ParameterType::Integer,
                        })
                        .collect(),
                    columns: statement.columns.clone(),
                },
            );
        }
        prepared.push(PreparedDatabase {
            name: name.clone(),
            relative_path: file.path.clone(),
            mode,
            limits: database_limits,
            statements,
        });
    }
    Ok(prepared)
}

/// Resolves paths and opens each validated database.
fn open_databases(
    host_config_path: &Path,
    prepared: Vec<PreparedDatabase>,
    max_transactions_per_invocation: usize,
) -> Result<DatabaseCatalog, HostConfigError> {
    if prepared.is_empty() {
        return Ok(DatabaseCatalog::default());
    }
    let root = config_root(host_config_path)?;
    let mut definitions = BTreeMap::new();
    let mut paths = BTreeMap::<PathBuf, String>::new();
    for database in prepared {
        let path = resolve_state_database_path(
            &root,
            &database.relative_path,
            database.limits.max_database_bytes,
        )?;
        if !path.exists() {
            return Err(HostConfigError::input(
                "application database file must already exist; Krit never creates or migrates an application schema",
            ));
        }
        if let Some(previous) = paths.insert(path.clone(), database.name.clone()) {
            return Err(HostConfigError::input(format!(
                "application databases `{previous}` and `{}` use the same file",
                database.name
            )));
        }
        definitions.insert(
            database.name,
            DatabaseDefinition {
                path,
                mode: database.mode,
                limits: database.limits,
                statements: database.statements,
            },
        );
    }
    DatabaseCatalog::open(definitions, max_transactions_per_invocation)
        .map_err(HostConfigError::durable)
}

/// Application database paths follow the same strict relative-path policy as
/// durable state files.
fn validate_relative_database_path(path: &Path) -> Result<(), HostConfigError> {
    validate_relative_state_path(path)
}

fn validate_state_store_file(file: &StateStoreFile) -> Result<(), HostConfigError> {
    let busy = Duration::from_millis(file.busy_timeout_ms);
    let replay_ttl = Duration::from_secs(file.replay_ttl_seconds);
    let lease = Duration::from_secs(file.lease_seconds);
    if busy.is_zero()
        || busy > MAX_STATE_BUSY_TIMEOUT
        || file.max_operations == 0
        || file.max_operations > MAX_STATE_OPERATIONS
        || file.max_key_bytes == 0
        || file.max_key_bytes > MAX_STATE_KEY_BYTES
        || file.max_value_bytes == 0
        || file.max_value_bytes > MAX_STATE_VALUE_BYTES
        || file.max_transaction_bytes == 0
        || file.max_transaction_bytes > MAX_STATE_TRANSACTION_BYTES
        || file.max_database_bytes < MIN_STATE_DATABASE_BYTES
        || file.max_database_bytes > MAX_STATE_DATABASE_BYTES
        || file.max_replay_entries == 0
        || file.max_replay_entries > MAX_REPLAY_ENTRIES
        || file.max_replay_bytes == 0
        || file.max_replay_bytes > MAX_REPLAY_BYTES
        || replay_ttl.is_zero()
        || replay_ttl > MAX_REPLAY_TTL
        || lease.is_zero()
        || lease > MAX_REPLAY_LEASE
    {
        return Err(HostConfigError::input(
            "durable state store limits exceed the Phase 6 bounds",
        ));
    }
    Ok(())
}

fn validate_relative_state_path(path: &Path) -> Result<(), HostConfigError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.to_string_lossy().contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.extension().and_then(|extension| extension.to_str()) != Some("db")
    {
        return Err(HostConfigError::input(
            "durable state path must be a host-config-relative `.db` path without `.` or `..`",
        ));
    }
    Ok(())
}

/// Resolves a store path without creating anything.
///
/// Every directory, symlink, ownership, and sidecar check runs here so that a
/// later configuration error cannot leave a half-created store behind.
fn resolve_state_database_path(
    root: &Path,
    relative: &Path,
    max_bytes: u64,
) -> Result<PathBuf, HostConfigError> {
    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut lexical = root.to_owned();
    for component in parent_relative.components() {
        let Component::Normal(name) = component else {
            return Err(HostConfigError::input(
                "durable state parent path is invalid",
            ));
        };
        lexical.push(name);
        let metadata = fs::symlink_metadata(&lexical)
            .map_err(|_| HostConfigError::input("durable state directory is not accessible"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(HostConfigError::input(
                "durable state directory components must be real directories",
            ));
        }
    }
    let parent = root
        .join(parent_relative)
        .canonicalize()
        .map_err(|_| HostConfigError::input("durable state directory is not accessible"))?;
    if !parent.starts_with(root) {
        return Err(HostConfigError::input(
            "durable state directory escapes the host config root",
        ));
    }
    validate_owner_only_directory(&parent)?;
    let file_name = relative
        .file_name()
        .ok_or_else(|| HostConfigError::input("durable state path has no file name"))?;
    let database = parent.join(file_name);
    if database.exists() {
        validate_owner_only_state_file(&database, max_bytes)?;
    }
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = database.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            validate_owner_only_state_file(&sidecar, max_bytes)?;
        }
    }
    Ok(database)
}

/// Creates a missing owner-only database file. Called only after every
/// configuration value has already been validated.
fn create_state_database_file(database: &Path) -> Result<(), HostConfigError> {
    if database.exists() {
        return Ok(());
    }
    create_owner_only_state_file(database)
}

#[cfg(unix)]
fn validate_owner_only_directory(path: &Path) -> Result<(), HostConfigError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .map_err(|_| HostConfigError::input("could not inspect durable state directory"))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(HostConfigError::input(
            "durable state directory must be owner-only on Unix",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_only_directory(_path: &Path) -> Result<(), HostConfigError> {
    Ok(())
}

fn validate_owner_only_state_file(path: &Path, max_bytes: u64) -> Result<(), HostConfigError> {
    let link = fs::symlink_metadata(path)
        .map_err(|_| HostConfigError::input("could not inspect durable state file"))?;
    if link.file_type().is_symlink() || !link.is_file() || link.len() > max_bytes {
        return Err(HostConfigError::input(
            "durable state file is unsafe or exceeds its configured byte limit",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if link.permissions().mode() & 0o077 != 0 {
            return Err(HostConfigError::input(
                "durable state files must be owner-only on Unix",
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_owner_only_state_file(path: &Path) -> Result<(), HostConfigError> {
    use rustix::fs::{Mode, OFlags};

    rustix::fs::open(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map(|_| ())
    .map_err(|_| HostConfigError::input("could not create owner-only durable state file"))
}

#[cfg(not(unix))]
fn create_owner_only_state_file(path: &Path) -> Result<(), HostConfigError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|_| HostConfigError::input("could not create durable state file"))
}

fn load_inputs(
    path: &Path,
    manifest: &Manifest,
    limits: RuntimeLimits,
    config: BTreeMap<String, String>,
    secret_references: BTreeMap<String, SecretReference>,
) -> Result<HostInputs, HostConfigError> {
    for name in config.keys() {
        if !manifest.capabilities.config.contains(name) {
            return Err(HostConfigError::authorization(format!(
                "host configuration key `{name}` is not granted by the package manifest"
            )));
        }
    }
    for name in secret_references.keys() {
        require_manifest_secret(manifest, name)?;
    }

    let root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_root = root.canonicalize().map_err(|error| {
        HostConfigError::input(format!(
            "could not resolve host config directory {}: {error}",
            root.display()
        ))
    })?;
    let mut secrets = BTreeMap::new();
    for (name, reference) in secret_references {
        validate_secret_reference(&reference.file)?;
        let unresolved = canonical_root.join(&reference.file);
        let link_metadata = fs::symlink_metadata(&unresolved).map_err(|error| {
            HostConfigError::input(format!(
                "could not inspect secret file for `{name}`: {error}"
            ))
        })?;
        if link_metadata.file_type().is_symlink() {
            return Err(HostConfigError::input(format!(
                "secret file for `{name}` must not be a symbolic link"
            )));
        }
        let canonical = unresolved.canonicalize().map_err(|error| {
            HostConfigError::input(format!(
                "could not resolve secret file for `{name}`: {error}"
            ))
        })?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
            return Err(HostConfigError::input(format!(
                "secret file for `{name}` must be a regular file inside the host config directory"
            )));
        }
        let bytes =
            read_secret_bounded(&canonical, limits.secret_bytes(), &name).map_err(|error| {
                HostConfigError::input(format!("could not read secret file for `{name}`: {error}"))
            })?;
        secrets.insert(name, bytes);
    }
    let secrets = SecretStore::new(secrets)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))?;
    HostInputs::new(config, secrets)
        .map_err(|error| HostConfigError::input(error.message().to_owned()))
}

fn require_manifest_ai(manifest: &Manifest, name: &str) -> Result<(), HostConfigError> {
    if manifest.capabilities.ai.iter().any(|entry| entry == name) {
        Ok(())
    } else {
        Err(HostConfigError::authorization(format!(
            "AI adapter `{name}` is not granted by the package manifest"
        )))
    }
}

fn require_manifest_state(manifest: &Manifest, name: &str) -> Result<(), HostConfigError> {
    if manifest
        .capabilities
        .state
        .iter()
        .any(|entry| entry == name)
    {
        Ok(())
    } else {
        Err(HostConfigError::authorization(format!(
            "durable state store `{name}` is not granted by the package manifest"
        )))
    }
}

fn require_manifest_http(manifest: &Manifest, origin: &str) -> Result<(), HostConfigError> {
    if manifest
        .capabilities
        .http
        .iter()
        .any(|entry| entry == origin)
    {
        Ok(())
    } else {
        Err(HostConfigError::authorization(format!(
            "HTTP origin `{origin}` is not granted by the package manifest"
        )))
    }
}

fn require_manifest_secret(manifest: &Manifest, name: &str) -> Result<(), HostConfigError> {
    if manifest
        .capabilities
        .secrets
        .iter()
        .any(|entry| entry == name)
    {
        Ok(())
    } else {
        Err(HostConfigError::authorization(format!(
            "host secret `{name}` is not granted by the package manifest"
        )))
    }
}

fn retry_policy(file: RetryPolicyFile) -> Result<RetryPolicy, HostConfigError> {
    Ok(RetryPolicy {
        max_attempts: file.max_attempts,
        base_delay: Duration::from_millis(file.base_delay_ms),
        max_delay: Duration::from_millis(file.max_delay_ms),
    })
}

fn rate_policy(file: RatePolicyFile) -> Result<RateLimitPolicy, HostConfigError> {
    Ok(RateLimitPolicy {
        capacity: file.capacity,
        window: milliseconds(file.window_ms, "rate limit window")?,
    })
}

fn milliseconds(value: u64, name: &str) -> Result<Duration, HostConfigError> {
    if value == 0 {
        return Err(HostConfigError::input(format!("{name} must be nonzero")));
    }

    Ok(Duration::from_millis(value))
}

fn seconds(value: u64, name: &str) -> Result<Duration, HostConfigError> {
    if value == 0 {
        return Err(HostConfigError::input(format!("{name} must be nonzero")));
    }
    Ok(Duration::from_secs(value))
}

const fn default_retry_attempts() -> u8 {
    1
}

const fn default_retry_base_ms() -> u64 {
    25
}

const fn default_retry_max_ms() -> u64 {
    200
}

fn default_http_rate() -> RatePolicyFile {
    RatePolicyFile {
        capacity: 64,
        window_ms: 60_000,
    }
}

fn default_ai_rate() -> RatePolicyFile {
    RatePolicyFile {
        capacity: 16,
        window_ms: 60_000,
    }
}

const fn default_max_tracked_resources() -> usize {
    128
}

const fn default_idempotency_entries() -> usize {
    128
}

const fn default_idempotency_bytes() -> usize {
    16 * 1024 * 1024
}

const fn default_idempotency_ttl_ms() -> u64 {
    300_000
}

const fn default_idempotency_key_bytes() -> usize {
    128
}

fn validate_secret_reference(path: &Path) -> Result<(), HostConfigError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.to_string_lossy().contains('\\')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(HostConfigError::input(
            "secret file references must be relative paths without `.` or `..`",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_secret_bounded(path: &Path, limit: usize, name: &str) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::PermissionsExt;

    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| error.to_string())?;
    let file = fs::File::from(descriptor);
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("not a regular file".to_owned());
    }
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "secret file for `{name}` must not grant group or other permissions"
        ));
    }
    read_file_bounded(file, limit)
}

#[cfg(not(unix))]
fn read_secret_bounded(path: &Path, limit: usize, _name: &str) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    read_file_bounded(file, limit)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    read_file_bounded(file, limit)
}

fn read_file_bounded(file: fs::File, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err(format!("file exceeds the {limit}-byte host input limit"));
    }
    Ok(bytes)
}
