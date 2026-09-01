use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, AiAdapterConfig, ApprovalOperation, ExplicitApprovalPolicy,
    HostInputs, HttpJsonAdapterConfig, IdempotencyPolicy, RateLimitPolicy, RetryPolicy,
    RuntimeLimits, SecretStore,
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

    fn input(message: impl Into<String>) -> Self {
        Self {
            kind: HostConfigErrorKind::Input,
            message: message.into(),
        }
    }

    fn authorization(message: impl Into<String>) -> Self {
        Self {
            kind: HostConfigErrorKind::Authorization,
            message: message.into(),
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
            AgentHost::new(inputs, policy, Arc::new(approvals))
                .map_err(|error| HostConfigError::input(error.message().to_owned()))
        }
        unsupported => Err(HostConfigError::input(format!(
            "unsupported host config schema {unsupported}; expected 1 or 2"
        ))),
    }
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
