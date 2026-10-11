use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use krit_capability::{HttpOrigin, is_valid_config_key, is_valid_resource_name};
use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub package: Package,
    /// Declared dependencies. Resolution is not implemented, so validation
    /// rejects any entry with `K6003`. Values are deliberately untyped so that
    /// every entry shape (version string or table) reaches that diagnostic
    /// rather than a TOML type error.
    #[serde(default)]
    pub dependencies: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// `[secrets.<name>]` tables. Each one grants the secret and binds it to
    /// the exact HTTP origins it may be injected into.
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretBinding>,
}

/// One `[secrets.<name>]` table.
///
/// The table both grants the secret and restricts it to `origins`, each of
/// which must also be a granted `http` origin. A secret named only by the
/// legacy `[capabilities] secrets` list stays unbound and may reach any granted
/// origin; a name present in both forms is bound by its table.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SecretBinding {
    pub origins: Vec<String>,
    #[serde(default)]
    pub inject: SecretInjection,
}

/// How the host presents a secret to an origin.
///
/// Bearer injection is the only supported form; any other spelling is a
/// manifest error rather than a silently ignored request.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretInjection {
    #[default]
    Bearer,
}

impl SecretInjection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
        }
    }
}

/// Whether a granted secret is restricted to a declared set of origins.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretBindingStatus {
    /// Declared by a `[secrets.<name>]` table; only its origins may receive it.
    Bound,
    /// Declared only by the legacy list; any granted origin may receive it.
    Unbound,
}

/// The reviewable binding of one granted secret.
///
/// `origins` is sorted and is empty for an unbound secret, whose reach is
/// every granted `http` origin.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SecretBindingFact {
    pub secret: String,
    pub inject: SecretInjection,
    pub binding: SecretBindingStatus,
    pub origins: Vec<String>,
}

/// Human label for a legacy secret that any granted origin may receive.
pub const UNBOUND_SECRET_LABEL: &str = "UNBOUND (any granted origin)";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub entry: PathBuf,
    pub license: String,
    #[serde(default = "default_target")]
    pub target: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub stdout: bool,
    #[serde(default)]
    pub config: Vec<String>,
    #[serde(default)]
    pub http: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub ai: Vec<String>,
    #[serde(default)]
    pub logs: bool,
    #[serde(default)]
    pub state: Vec<String>,
    #[serde(default)]
    pub queues: Vec<String>,
    #[serde(default)]
    pub consumes: Vec<String>,
    #[serde(default)]
    pub schedules: Vec<String>,
    #[serde(default)]
    pub buckets: Vec<String>,
    #[serde(default, rename = "readOnlyBuckets")]
    pub read_only_buckets: Vec<String>,
    #[serde(default)]
    pub databases: Vec<String>,
    #[serde(default, rename = "readOnlyDatabases")]
    pub read_only_databases: Vec<String>,
    #[serde(default, rename = "cacheNamespaces")]
    pub cache_namespaces: Vec<String>,
    #[serde(default, rename = "readOnlyCacheNamespaces")]
    pub read_only_cache_namespaces: Vec<String>,
    #[serde(default, rename = "searchIndexes")]
    pub search_indexes: Vec<String>,
    #[serde(default, rename = "vectorIndexes")]
    pub vector_indexes: Vec<String>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct PermissionPlan {
    pub schema: u32,
    pub package: String,
    pub requested: Vec<PermissionRequest>,
    #[serde(rename = "secretBindings", skip_serializing_if = "Vec::is_empty")]
    pub secret_bindings: Vec<SecretBindingFact>,
    #[serde(rename = "grantStatus")]
    pub grant_status: &'static str,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PermissionRequest {
    pub capability: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

#[derive(Debug)]
pub struct ManifestError {
    code: &'static str,
    message: String,
}

impl ManifestError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            code: "K6001",
            message: message.into(),
        }
    }

    fn unsupported_dependencies() -> Self {
        Self {
            code: "K6003",
            message: "dependency resolution is not supported; remove [dependencies]".to_owned(),
        }
    }

    /// Stable diagnostic code: `K6001` for an invalid manifest or entry, and
    /// `K6003` for a manifest that declares dependencies.
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ManifestError {}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let contents = fs::read_to_string(path).map_err(|error| {
            ManifestError::new(format!("could not read {}: {error}", path.display()))
        })?;
        let manifest = Self::parse(&contents)?;
        manifest.resolve_entry(path)?;
        Ok(manifest)
    }

    pub fn parse(contents: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(contents)
            .map_err(|error| ManifestError::new(format!("invalid manifest: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != 1 {
            return Err(ManifestError::new(format!(
                "unsupported manifest schema {}; expected 1",
                self.schema
            )));
        }
        validate_package_name(&self.package.name)?;
        Version::parse(&self.package.version).map_err(|error| {
            ManifestError::new(format!(
                "invalid package version `{}`: {error}",
                self.package.version
            ))
        })?;
        if self.package.edition != "2026" {
            return Err(ManifestError::new(format!(
                "unsupported Krit edition `{}`; expected `2026`",
                self.package.edition
            )));
        }
        validate_entry_path(&self.package.entry)?;
        if self.package.target != "wasm-component" {
            return Err(ManifestError::new(format!(
                "unsupported package target `{}`; expected `wasm-component`",
                self.package.target
            )));
        }
        if self.package.license.trim().is_empty() {
            return Err(ManifestError::new("package license cannot be empty"));
        }
        // Fail closed: a declared dependency would otherwise be silently
        // ignored, so any entry is rejected until resolution and locking exist.
        if !self.dependencies.is_empty() {
            return Err(ManifestError::unsupported_dependencies());
        }
        validate_unique_names("configuration key", &self.capabilities.config)?;
        validate_capability_count("configuration key", &self.capabilities.config)?;
        for name in &self.capabilities.config {
            validate_config_key(name)?;
        }
        validate_unique_names("HTTP origin", &self.capabilities.http)?;
        validate_capability_count("HTTP origin", &self.capabilities.http)?;
        for origin in &self.capabilities.http {
            validate_http_origin(origin)?;
        }
        validate_unique_names("secret name", &self.capabilities.secrets)?;
        validate_capability_count("secret name", &self.capabilities.secrets)?;
        for name in &self.capabilities.secrets {
            validate_resource_name("secret name", name)?;
        }
        self.validate_secret_bindings()?;
        validate_sorted_unique_names("AI adapter", &self.capabilities.ai)?;
        validate_capability_count("AI adapter", &self.capabilities.ai)?;
        for name in &self.capabilities.ai {
            validate_resource_name("AI adapter", name)?;
        }
        validate_sorted_unique_names("durable state store", &self.capabilities.state)?;
        validate_capability_count("durable state store", &self.capabilities.state)?;
        for name in &self.capabilities.state {
            validate_resource_name("durable state store", name)?;
        }
        for (label, names) in [
            ("durable queue", &self.capabilities.queues),
            ("durable queue consumer", &self.capabilities.consumes),
            ("durable schedule", &self.capabilities.schedules),
            ("object bucket", &self.capabilities.buckets),
            (
                "read-only object bucket",
                &self.capabilities.read_only_buckets,
            ),
            ("application database", &self.capabilities.databases),
            (
                "read-only application database",
                &self.capabilities.read_only_databases,
            ),
            ("cache namespace", &self.capabilities.cache_namespaces),
            (
                "read-only cache namespace",
                &self.capabilities.read_only_cache_namespaces,
            ),
            ("search index", &self.capabilities.search_indexes),
            ("vector index", &self.capabilities.vector_indexes),
        ] {
            validate_sorted_unique_names(label, names)?;
            validate_capability_count(label, names)?;
            for name in names {
                validate_resource_name(label, name)?;
            }
        }
        for name in &self.capabilities.read_only_buckets {
            if self.capabilities.buckets.contains(name) {
                return Err(ManifestError::new(format!(
                    "object bucket `{name}` cannot request both read-only and write authority"
                )));
            }
        }
        for name in &self.capabilities.read_only_databases {
            if self.capabilities.databases.contains(name) {
                return Err(ManifestError::new(format!(
                    "application database `{name}` cannot request both read-only and write authority"
                )));
            }
        }
        for name in &self.capabilities.read_only_cache_namespaces {
            if self.capabilities.cache_namespaces.contains(name) {
                return Err(ManifestError::new(format!(
                    "cache namespace `{name}` cannot request both read-only and write authority"
                )));
            }
        }
        Ok(())
    }

    /// Validates every `[secrets.<name>]` table.
    ///
    /// A binding names at least one origin, each origin is canonical, unique,
    /// and already granted under `[capabilities] http`, and only bearer
    /// injection is accepted. A binding can therefore narrow where a secret
    /// goes but never adds network authority of its own.
    fn validate_secret_bindings(&self) -> Result<(), ManifestError> {
        validate_capability_count("secret name", &self.secret_names())?;
        for (name, binding) in &self.secrets {
            validate_resource_name("secret name", name)?;
            if binding.origins.is_empty() {
                return Err(ManifestError::new(format!(
                    "secret `{name}` must bind at least one origin"
                )));
            }
            let mut seen = Vec::with_capacity(binding.origins.len());
            for origin in &binding.origins {
                validate_http_origin(origin)?;
                if seen.contains(&origin) {
                    return Err(ManifestError::new(format!(
                        "secret `{name}` binds origin `{origin}` more than once"
                    )));
                }
                seen.push(origin);
                if !self.capabilities.http.contains(origin) {
                    return Err(ManifestError::new(format!(
                        "secret `{name}` is bound to origin `{origin}`, which is not a granted `http` origin"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every granted secret name, sorted and unique: the legacy
    /// `[capabilities] secrets` list together with every `[secrets.<name>]`
    /// table.
    pub fn secret_names(&self) -> Vec<String> {
        let mut names = self
            .capabilities
            .secrets
            .iter()
            .chain(self.secrets.keys())
            .cloned()
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// The binding of every granted secret, sorted by secret name.
    pub fn secret_bindings(&self) -> Vec<SecretBindingFact> {
        self.secret_names()
            .into_iter()
            .map(|secret| match self.secrets.get(&secret) {
                Some(binding) => {
                    let mut origins = binding.origins.clone();
                    origins.sort();
                    SecretBindingFact {
                        secret,
                        inject: binding.inject,
                        binding: SecretBindingStatus::Bound,
                        origins,
                    }
                }
                None => SecretBindingFact {
                    secret,
                    inject: SecretInjection::Bearer,
                    binding: SecretBindingStatus::Unbound,
                    origins: Vec::new(),
                },
            })
            .collect()
    }

    /// Whether the host may inject `secret` as the bearer credential of a
    /// request to `origin`.
    ///
    /// Both must be granted, and a secret bound by a `[secrets.<name>]` table
    /// may reach only the origins that table lists.
    pub fn permits_bearer(&self, secret: &str, origin: &str) -> bool {
        self.grants_permission("secret.read", Some(secret))
            && self.grants_permission("http.request", Some(origin))
            && self
                .secrets
                .get(secret)
                .is_none_or(|binding| binding.origins.iter().any(|bound| bound == origin))
    }

    pub fn resolve_entry(&self, manifest_path: &Path) -> Result<PathBuf, ManifestError> {
        let root = manifest_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_root = root.canonicalize().map_err(|error| {
            ManifestError::new(format!(
                "could not resolve manifest directory {}: {error}",
                root.display()
            ))
        })?;
        let entry = canonical_root.join(&self.package.entry);
        let canonical_entry = entry.canonicalize().map_err(|error| {
            ManifestError::new(format!(
                "package entry `{}` does not exist or is not accessible: {error}",
                self.package.entry.display()
            ))
        })?;
        if !canonical_entry.starts_with(&canonical_root) || !canonical_entry.is_file() {
            return Err(ManifestError::new(format!(
                "package entry `{}` must resolve to a file inside the package",
                self.package.entry.display()
            )));
        }
        Ok(canonical_entry)
    }

    pub fn permission_plan(&self) -> PermissionPlan {
        let mut requested = Vec::new();
        if self.capabilities.stdout {
            requested.push(PermissionRequest {
                capability: "io.stdout",
                resource: None,
            });
        }

        requested.extend(self.capabilities.config.iter().cloned().map(|resource| {
            PermissionRequest {
                capability: "config.read",
                resource: Some(resource),
            }
        }));
        requested.extend(self.capabilities.http.iter().cloned().map(|resource| {
            PermissionRequest {
                capability: "http.request",
                resource: Some(resource),
            }
        }));
        requested.extend(
            self.secret_names()
                .into_iter()
                .map(|resource| PermissionRequest {
                    capability: "secret.read",
                    resource: Some(resource),
                }),
        );
        requested.extend(
            self.capabilities
                .ai
                .iter()
                .cloned()
                .map(|resource| PermissionRequest {
                    capability: "ai.invoke",
                    resource: Some(resource),
                }),
        );
        if self.capabilities.logs {
            requested.push(PermissionRequest {
                capability: "observe.log",
                resource: None,
            });
        }
        requested.extend(self.capabilities.state.iter().cloned().map(|resource| {
            PermissionRequest {
                capability: "state.transaction",
                resource: Some(resource),
            }
        }));
        for (capability, names) in [
            ("queue.publish", &self.capabilities.queues),
            ("queue.consume", &self.capabilities.consumes),
            ("schedule.trigger", &self.capabilities.schedules),
            ("object.write", &self.capabilities.buckets),
            ("database.write", &self.capabilities.databases),
            ("cache.write", &self.capabilities.cache_namespaces),
            ("search.query", &self.capabilities.search_indexes),
            ("search.vector", &self.capabilities.vector_indexes),
        ] {
            requested.extend(names.iter().cloned().map(|resource| PermissionRequest {
                capability,
                resource: Some(resource),
            }));
        }
        requested.extend(
            self.capabilities
                .buckets
                .iter()
                .chain(&self.capabilities.read_only_buckets)
                .cloned()
                .map(|resource| PermissionRequest {
                    capability: "object.read",
                    resource: Some(resource),
                }),
        );
        requested.extend(
            self.capabilities
                .databases
                .iter()
                .chain(&self.capabilities.read_only_databases)
                .cloned()
                .map(|resource| PermissionRequest {
                    capability: "database.read",
                    resource: Some(resource),
                }),
        );
        requested.extend(
            self.capabilities
                .cache_namespaces
                .iter()
                .chain(&self.capabilities.read_only_cache_namespaces)
                .cloned()
                .map(|resource| PermissionRequest {
                    capability: "cache.read",
                    resource: Some(resource),
                }),
        );
        requested.sort();
        PermissionPlan {
            schema: 1,
            package: self.package.name.clone(),
            requested,
            secret_bindings: self.secret_bindings(),
            grant_status: "not-evaluated",
        }
    }

    pub fn grants_permission(&self, capability: &str, resource: Option<&str>) -> bool {
        match capability {
            "io.stdout" => resource.is_none() && self.capabilities.stdout,
            "config.read" => resource.is_some_and(|resource| {
                self.capabilities
                    .config
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "http.request" => resource.is_some_and(|resource| {
                self.capabilities
                    .http
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "secret.read" => resource.is_some_and(|resource| {
                self.capabilities
                    .secrets
                    .iter()
                    .any(|granted| granted == resource)
                    || self.secrets.contains_key(resource)
            }),
            "ai.invoke" => resource.is_some_and(|resource| {
                self.capabilities
                    .ai
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "observe.log" => resource.is_none() && self.capabilities.logs,
            "state.transaction" => resource.is_some_and(|resource| {
                self.capabilities
                    .state
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "queue.publish" => resource.is_some_and(|resource| {
                self.capabilities
                    .queues
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "queue.consume" => resource.is_some_and(|resource| {
                self.capabilities
                    .consumes
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "schedule.trigger" => resource.is_some_and(|resource| {
                self.capabilities
                    .schedules
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "object.write" => resource.is_some_and(|resource| {
                self.capabilities
                    .buckets
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "object.read" => resource.is_some_and(|resource| {
                self.capabilities
                    .buckets
                    .iter()
                    .chain(&self.capabilities.read_only_buckets)
                    .any(|granted| granted == resource)
            }),
            "database.write" => resource.is_some_and(|resource| {
                self.capabilities
                    .databases
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "database.read" => resource.is_some_and(|resource| {
                self.capabilities
                    .databases
                    .iter()
                    .chain(&self.capabilities.read_only_databases)
                    .any(|granted| granted == resource)
            }),
            "cache.write" => resource.is_some_and(|resource| {
                self.capabilities
                    .cache_namespaces
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "cache.read" => resource.is_some_and(|resource| {
                self.capabilities
                    .cache_namespaces
                    .iter()
                    .chain(&self.capabilities.read_only_cache_namespaces)
                    .any(|granted| granted == resource)
            }),
            "search.query" => resource.is_some_and(|resource| {
                self.capabilities
                    .search_indexes
                    .iter()
                    .any(|granted| granted == resource)
            }),
            "search.vector" => resource.is_some_and(|resource| {
                self.capabilities
                    .vector_indexes
                    .iter()
                    .any(|granted| granted == resource)
            }),
            _ => false,
        }
    }
}

fn default_target() -> String {
    "wasm-component".to_owned()
}

impl PermissionPlan {
    pub fn render_human(&self) -> String {
        let mut output = format!("Requested capabilities for {}:\n", self.package);
        if self.requested.is_empty() {
            output.push_str("  (none)\n");
        } else {
            for request in &self.requested {
                output.push_str("  ");
                output.push_str(request.capability);
                if let Some(resource) = &request.resource {
                    output.push_str(": ");
                    output.push_str(resource);
                }
                output.push('\n');
            }
        }
        render_secret_bindings(&mut output, &self.secret_bindings);
        output.push_str("Deployment grants: not evaluated\n");
        output
    }

    pub fn render_json(&self) -> String {
        serde_json::to_string(self).expect("permission plan serialization cannot fail")
    }
}

/// Appends the human `Secret bindings:` section shared by every permission
/// report. Nothing is written when no secret is granted.
pub fn render_secret_bindings(output: &mut String, bindings: &[SecretBindingFact]) {
    if bindings.is_empty() {
        return;
    }
    output.push_str("Secret bindings:\n");
    for binding in bindings {
        output.push_str("  ");
        output.push_str(&binding.secret);
        output.push_str(" (");
        output.push_str(binding.inject.as_str());
        output.push_str("): ");
        match binding.binding {
            SecretBindingStatus::Bound => output.push_str(&binding.origins.join(", ")),
            SecretBindingStatus::Unbound => output.push_str(UNBOUND_SECRET_LABEL),
        }
        output.push('\n');
    }
}

fn validate_package_name(name: &str) -> Result<(), ManifestError> {
    let mut segments = name.split('/');
    let namespace = segments.next().unwrap_or_default();
    let package = segments.next().unwrap_or_default();
    if namespace.is_empty() || package.is_empty() || segments.next().is_some() {
        return Err(ManifestError::new(format!(
            "package name `{name}` must be `namespace/name`"
        )));
    }
    if !valid_name_segment(namespace) || !valid_name_segment(package) {
        return Err(ManifestError::new(format!(
            "package name `{name}` may contain lowercase ASCII letters, digits, and `-`"
        )));
    }
    Ok(())
}

fn valid_name_segment(segment: &str) -> bool {
    !segment.starts_with('-')
        && !segment.ends_with('-')
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_entry_path(path: &Path) -> Result<(), ManifestError> {
    if path.as_os_str().is_empty()
        || path.to_string_lossy().contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ManifestError::new(
            "package entry must be a package-relative path without `.` or `..`",
        ));
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("krit") {
        return Err(ManifestError::new(
            "package entry must have the `.krit` extension",
        ));
    }
    Ok(())
}

fn validate_unique_names(kind: &str, values: &[String]) -> Result<(), ManifestError> {
    let mut sorted = values.to_vec();
    sorted.sort();
    if let Some(duplicate) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(ManifestError::new(format!(
            "duplicate {kind} `{}`",
            duplicate[0]
        )));
    }
    Ok(())
}

fn validate_sorted_unique_names(kind: &str, values: &[String]) -> Result<(), ManifestError> {
    if let Some(pair) = values.windows(2).find(|pair| pair[0] >= pair[1]) {
        return Err(ManifestError::new(if pair[0] == pair[1] {
            format!("duplicate {kind} `{}`", pair[0])
        } else {
            format!("{kind} entries must be sorted and unique")
        }));
    }
    Ok(())
}

fn validate_capability_count(kind: &str, values: &[String]) -> Result<(), ManifestError> {
    if values.len() > 256 {
        return Err(ManifestError::new(format!(
            "too many {kind} entries; maximum is 256"
        )));
    }
    Ok(())
}

fn validate_resource_name(kind: &str, name: &str) -> Result<(), ManifestError> {
    if !is_valid_resource_name(name) {
        return Err(ManifestError::new(format!(
            "{kind} `{name}` must use 1-64 lowercase letters, digits, `.` or `-`, without leading/trailing punctuation or `..`/`--`"
        )));
    }
    Ok(())
}

fn validate_config_key(name: &str) -> Result<(), ManifestError> {
    if !is_valid_config_key(name) {
        return Err(ManifestError::new(format!(
            "configuration key `{name}` must use 1-64 lowercase letters, digits, `.`, `-`, or `_`, without leading/trailing punctuation or `..`/`--`/`__`"
        )));
    }
    Ok(())
}

fn validate_http_origin(origin: &str) -> Result<(), ManifestError> {
    HttpOrigin::parse_exact(origin)
        .map(|_| ())
        .map_err(|error| ManifestError::new(format!("invalid HTTP origin `{origin}`: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        schema = 1

        [package]
        name = "akshay/krit"
        version = "0.2.0"
        edition = "2026"
        entry = "examples/factorial.krit"
        license = "Apache-2.0"

        [dependencies]

        [capabilities]
        stdout = true
        config = ["agent.model", "agent.timeout-ms"]
        http = ["https://api.github.com", "https://slack.com:8443"]
        secrets = ["github-token", "slack-token"]
    "#;

    #[test]
    fn validates_a_strict_manifest() {
        let manifest = Manifest::parse(VALID).expect("manifest should be valid");
        assert_eq!(manifest.package.name, "akshay/krit");
        assert!(manifest.capabilities.stdout);
        assert_eq!(manifest.permission_plan().requested.len(), 7);
        assert!(manifest.grants_permission("io.stdout", None));
        assert!(manifest.grants_permission("config.read", Some("agent.model")));
        assert!(manifest.grants_permission("http.request", Some("https://api.github.com")));
        assert!(manifest.grants_permission("secret.read", Some("github-token")));
        assert!(!manifest.grants_permission("config.read", Some("missing")));
        assert!(!manifest.grants_permission("unknown", None));
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = Manifest::parse(&VALID.replace("stdout = true", "network = true"))
            .expect_err("unknown capabilities should fail");
        assert_eq!(error.code(), "K6001");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn accepts_an_absent_or_empty_dependency_table() {
        let manifest = Manifest::parse(VALID).expect("an empty table should be accepted");
        assert!(manifest.dependencies.is_empty());

        let manifest = Manifest::parse(&VALID.replace("[dependencies]\n", ""))
            .expect("an absent table should be accepted");
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn rejects_every_declared_dependency_shape_with_k6003() {
        for entries in [
            "\"krit/json\" = \"1.2.3\"",
            "\"krit/json\" = \"^1\"\n\"krit/http\" = \"1.0.0\"",
            "\"krit/json\" = { path = \"../json\" }",
            "\"krit/json\" = { git = \"https://example.com/json.git\", rev = \"0123abc\" }",
            // Unresolvable names and requirements are refused for the same
            // reason rather than reaching name or semver validation.
            "\"Not A Package\" = \"not semver\"",
            "\"krit/json\" = 1",
        ] {
            let manifest =
                VALID.replace("[dependencies]\n", &format!("[dependencies]\n{entries}\n"));
            let error = Manifest::parse(&manifest).expect_err("dependencies should fail closed");
            assert_eq!(error.code(), "K6003", "{entries}");
            assert_eq!(
                error.to_string(),
                "dependency resolution is not supported; remove [dependencies]"
            );
        }

        let manifest = VALID.replace(
            "[dependencies]\n",
            "[dependencies.\"krit/json\"]\npath = \"../json\"\n",
        );
        let error = Manifest::parse(&manifest).expect_err("dependency sub-tables should fail");
        assert_eq!(error.code(), "K6003");
    }

    #[test]
    fn dependency_refusal_has_a_fixed_validation_position() {
        let declared = VALID.replace(
            "[dependencies]\n",
            "[dependencies]\n\"krit/json\" = \"1.2.3\"\n",
        );

        let error = Manifest::parse(&declared.replace("stdout = true", "network = true"))
            .expect_err("unknown fields should fail");
        assert_eq!(error.code(), "K6001");
        assert!(error.to_string().contains("unknown field"));

        let error = Manifest::parse(&declared.replace("edition = \"2026\"", "edition = \"2024\""))
            .expect_err("unsupported edition should fail");
        assert_eq!(error.code(), "K6001");
        assert!(error.to_string().contains("unsupported Krit edition"));

        let error = Manifest::parse(&declared.replace(
            "secrets = [\"github-token\", \"slack-token\"]",
            "secrets = [\"github-token\", \"github-token\"]",
        ))
        .expect_err("dependencies should fail before capability lists");
        assert_eq!(error.code(), "K6003");
    }

    #[test]
    fn rejects_parent_entry_paths() {
        let error = Manifest::parse(
            &VALID.replace("examples/factorial.krit", "../examples/factorial.krit"),
        )
        .expect_err("parent traversal should fail");
        assert!(error.to_string().contains("package-relative"));
    }

    #[test]
    fn defaults_and_validates_the_component_target() {
        let manifest = Manifest::parse(VALID).expect("manifest should be valid");
        assert_eq!(manifest.package.target, "wasm-component");

        let error = Manifest::parse(&VALID.replace(
            "license = \"Apache-2.0\"",
            "license = \"Apache-2.0\"\ntarget = \"native\"",
        ))
        .expect_err("unknown package targets should fail");
        assert!(error.to_string().contains("unsupported package target"));
    }

    #[test]
    fn renders_a_sorted_permission_plan() {
        let manifest = Manifest::parse(VALID).expect("manifest should be valid");
        assert_eq!(
            manifest.permission_plan().render_json(),
            "{\"schema\":1,\"package\":\"akshay/krit\",\"requested\":[{\"capability\":\"config.read\",\"resource\":\"agent.model\"},{\"capability\":\"config.read\",\"resource\":\"agent.timeout-ms\"},{\"capability\":\"http.request\",\"resource\":\"https://api.github.com\"},{\"capability\":\"http.request\",\"resource\":\"https://slack.com:8443\"},{\"capability\":\"io.stdout\"},{\"capability\":\"secret.read\",\"resource\":\"github-token\"},{\"capability\":\"secret.read\",\"resource\":\"slack-token\"}],\"secretBindings\":[{\"secret\":\"github-token\",\"inject\":\"bearer\",\"binding\":\"unbound\",\"origins\":[]},{\"secret\":\"slack-token\",\"inject\":\"bearer\",\"binding\":\"unbound\",\"origins\":[]}],\"grantStatus\":\"not-evaluated\"}"
        );
    }

    #[test]
    fn rejects_unsafe_http_origins() {
        for origin in [
            "https://api.github.com/path",
            "https://user@api.github.com",
            "https://api.github.com/",
            "https://api.github.com:443",
            "HTTPS://api.github.com",
        ] {
            let manifest = VALID.replace(
                "https://api.github.com\", \"https://slack.com:8443",
                &format!("{origin}\", \"https://slack.com:8443"),
            );
            let error = Manifest::parse(&manifest).expect_err("unsafe origin should fail");
            assert!(error.to_string().contains("HTTP origin"));
        }
    }

    #[test]
    fn configuration_keys_alone_accept_underscores() {
        let manifest = Manifest::parse(&VALID.replace(
            "config = [\"agent.model\", \"agent.timeout-ms\"]",
            "config = [\"agent.model\", \"agent.timeout_ms\", \"max_retries\"]",
        ))
        .expect("underscored configuration keys should validate");
        assert!(manifest.grants_permission("config.read", Some("agent.timeout_ms")));
        assert!(manifest.grants_permission("config.read", Some("max_retries")));
        assert!(!manifest.grants_permission("config.read", Some("agent.timeout-ms")));

        for key in ["_agent", "agent_", "agent__model", "Agent_Model"] {
            let error = Manifest::parse(&VALID.replace(
                "config = [\"agent.model\", \"agent.timeout-ms\"]",
                &format!("config = [\"{key}\"]"),
            ))
            .expect_err("a non-canonical configuration key should fail");
            assert_eq!(error.code(), "K6001");
            assert!(
                error
                    .to_string()
                    .contains(&format!("configuration key `{key}` must use")),
                "{error}"
            );
        }

        // Every other resource identity keeps the narrower grammar.
        for (field, value) in [
            ("secrets", "[\"github_token\"]"),
            ("ai", "[\"code_reviewer\"]"),
            ("state", "[\"agent_work\"]"),
            ("queues", "[\"render_jobs\"]"),
            ("buckets", "[\"render_output\"]"),
        ] {
            let manifest = VALID.replace(
                "secrets = [\"github-token\", \"slack-token\"]",
                &format!("{field} = {value}"),
            );
            let error = Manifest::parse(&manifest).expect_err("underscored resource should fail");
            assert_eq!(error.code(), "K6001");
            assert!(error.to_string().contains("must use"), "{error}");
        }
    }

    #[test]
    fn rejects_duplicate_capability_resources() {
        let manifest = VALID.replace(
            "secrets = [\"github-token\", \"slack-token\"]",
            "secrets = [\"github-token\", \"github-token\"]",
        );
        let error = Manifest::parse(&manifest).expect_err("duplicate secret should fail");
        assert!(error.to_string().contains("duplicate secret name"));
    }

    #[test]
    fn validates_sorted_ai_and_structured_log_permissions() {
        let manifest = Manifest::parse(&VALID.replace(
            "stdout = true",
            "stdout = true\nai = [\"reviewer\", \"summarizer\"]\nlogs = true",
        ))
        .expect("AI and logging requests should validate");
        assert_eq!(
            manifest
                .permission_plan()
                .requested
                .iter()
                .filter(|request| { matches!(request.capability, "ai.invoke" | "observe.log") })
                .count(),
            3
        );

        let unsorted = VALID.replace(
            "stdout = true",
            "stdout = true\nai = [\"summarizer\", \"reviewer\"]",
        );
        let error = Manifest::parse(&unsorted).expect_err("AI names must be sorted");
        assert!(error.to_string().contains("sorted and unique"));
    }

    #[test]
    fn validates_sorted_durable_state_permissions() {
        let manifest = Manifest::parse(&VALID.replace(
            "secrets = [\"github-token\", \"slack-token\"]",
            "secrets = [\"github-token\", \"slack-token\"]\nstate = [\"agent-work\", \"replay\"]",
        ))
        .expect("sorted state stores should validate");
        assert!(manifest.grants_permission("state.transaction", Some("agent-work")));
        assert!(manifest.permission_plan().requested.iter().any(|request| {
            request.capability == "state.transaction"
                && request.resource.as_deref() == Some("replay")
        }));

        let error = Manifest::parse(&VALID.replace(
            "secrets = [\"github-token\", \"slack-token\"]",
            "secrets = [\"github-token\", \"slack-token\"]\nstate = [\"replay\", \"agent-work\"]",
        ))
        .expect_err("unsorted state stores should fail");
        assert!(error.to_string().contains("sorted and unique"));
    }

    const BOUND: &str = r#"
        schema = 1

        [package]
        name = "akshay/krit"
        version = "0.2.0"
        edition = "2026"
        entry = "examples/factorial.krit"
        license = "Apache-2.0"

        [capabilities]
        http = ["https://api.github.com", "https://slack.com:8443"]
        secrets = ["github-token", "legacy-token"]

        [secrets.github-token]
        origins = ["https://api.github.com"]

        [secrets.slack-token]
        origins = ["https://slack.com:8443", "https://api.github.com"]
        inject = "bearer"
    "#;

    #[test]
    fn secret_tables_grant_and_bind_while_the_legacy_list_stays_unbound() {
        let manifest = Manifest::parse(BOUND).expect("bound manifest should be valid");
        assert_eq!(
            manifest.secret_names(),
            ["github-token", "legacy-token", "slack-token"]
        );
        for name in ["github-token", "legacy-token", "slack-token"] {
            assert!(manifest.grants_permission("secret.read", Some(name)));
        }
        assert!(!manifest.grants_permission("secret.read", Some("missing-token")));

        // A name in both forms is bound by its table.
        assert!(manifest.permits_bearer("github-token", "https://api.github.com"));
        assert!(!manifest.permits_bearer("github-token", "https://slack.com:8443"));
        assert!(manifest.permits_bearer("slack-token", "https://slack.com:8443"));
        assert!(manifest.permits_bearer("slack-token", "https://api.github.com"));
        // A legacy secret reaches any granted origin, and nothing else.
        assert!(manifest.permits_bearer("legacy-token", "https://api.github.com"));
        assert!(manifest.permits_bearer("legacy-token", "https://slack.com:8443"));
        assert!(!manifest.permits_bearer("legacy-token", "https://other.example"));
        assert!(!manifest.permits_bearer("missing-token", "https://api.github.com"));

        // A dotted resource name is a quoted TOML key.
        let dotted =
            Manifest::parse(&BOUND.replace("[secrets.slack-token]", "[secrets.\"slack.token\"]"))
                .expect("a quoted dotted secret name should be valid");
        assert!(dotted.permits_bearer("slack.token", "https://slack.com:8443"));

        assert_eq!(
            manifest.secret_bindings(),
            [
                SecretBindingFact {
                    secret: "github-token".to_owned(),
                    inject: SecretInjection::Bearer,
                    binding: SecretBindingStatus::Bound,
                    origins: vec!["https://api.github.com".to_owned()],
                },
                SecretBindingFact {
                    secret: "legacy-token".to_owned(),
                    inject: SecretInjection::Bearer,
                    binding: SecretBindingStatus::Unbound,
                    origins: Vec::new(),
                },
                SecretBindingFact {
                    secret: "slack-token".to_owned(),
                    inject: SecretInjection::Bearer,
                    binding: SecretBindingStatus::Bound,
                    origins: vec![
                        "https://api.github.com".to_owned(),
                        "https://slack.com:8443".to_owned(),
                    ],
                },
            ]
        );
    }

    #[test]
    fn permission_plans_report_every_secret_binding() {
        let plan = Manifest::parse(BOUND)
            .expect("bound manifest should be valid")
            .permission_plan();
        assert_eq!(
            plan.render_json(),
            "{\"schema\":1,\"package\":\"akshay/krit\",\"requested\":[{\"capability\":\"http.request\",\"resource\":\"https://api.github.com\"},{\"capability\":\"http.request\",\"resource\":\"https://slack.com:8443\"},{\"capability\":\"secret.read\",\"resource\":\"github-token\"},{\"capability\":\"secret.read\",\"resource\":\"legacy-token\"},{\"capability\":\"secret.read\",\"resource\":\"slack-token\"}],\"secretBindings\":[{\"secret\":\"github-token\",\"inject\":\"bearer\",\"binding\":\"bound\",\"origins\":[\"https://api.github.com\"]},{\"secret\":\"legacy-token\",\"inject\":\"bearer\",\"binding\":\"unbound\",\"origins\":[]},{\"secret\":\"slack-token\",\"inject\":\"bearer\",\"binding\":\"bound\",\"origins\":[\"https://api.github.com\",\"https://slack.com:8443\"]}],\"grantStatus\":\"not-evaluated\"}"
        );
        assert_eq!(
            plan.render_human(),
            "Requested capabilities for akshay/krit:\n  \
             http.request: https://api.github.com\n  \
             http.request: https://slack.com:8443\n  \
             secret.read: github-token\n  \
             secret.read: legacy-token\n  \
             secret.read: slack-token\n\
             Secret bindings:\n  \
             github-token (bearer): https://api.github.com\n  \
             legacy-token (bearer): UNBOUND (any granted origin)\n  \
             slack-token (bearer): https://api.github.com, https://slack.com:8443\n\
             Deployment grants: not evaluated\n"
        );

        // A plan without secrets keeps its exact pre-binding shape.
        let plain =
            Manifest::parse(&VALID.replace("secrets = [\"github-token\", \"slack-token\"]", ""))
                .expect("secret-free manifest should be valid")
                .permission_plan();
        assert!(!plain.render_json().contains("secretBindings"));
        assert!(!plain.render_human().contains("Secret bindings"));
    }

    #[test]
    fn secret_bindings_fail_closed() {
        for (from, to, expected) in [
            (
                "origins = [\"https://api.github.com\"]",
                "origins = [\"https://other.example\"]",
                "not a granted `http` origin",
            ),
            (
                "origins = [\"https://api.github.com\"]",
                "origins = []",
                "must bind at least one origin",
            ),
            (
                "origins = [\"https://api.github.com\"]",
                "origins = [\"https://api.github.com\", \"https://api.github.com\"]",
                "more than once",
            ),
            (
                "origins = [\"https://api.github.com\"]",
                "origins = [\"https://api.github.com/\"]",
                "invalid HTTP origin",
            ),
            (
                "inject = \"bearer\"",
                "inject = \"basic\"",
                "unknown variant",
            ),
            (
                "inject = \"bearer\"",
                "inject = \"Bearer\"",
                "unknown variant",
            ),
            (
                "inject = \"bearer\"",
                "header = \"x-api-key\"",
                "unknown field",
            ),
            (
                "[secrets.github-token]",
                "[secrets.GitHub-Token]",
                "must use 1-64 lowercase",
            ),
            (
                "[secrets.github-token]\n        origins = [\"https://api.github.com\"]",
                "[secrets.github-token]",
                "missing field `origins`",
            ),
        ] {
            assert!(BOUND.contains(from), "fixture must contain `{from}`");
            let error = Manifest::parse(&BOUND.replacen(from, to, 1))
                .expect_err("invalid secret binding must fail");
            assert!(
                error.to_string().contains(expected),
                "`{to}` produced `{error}`"
            );
        }
    }
}
