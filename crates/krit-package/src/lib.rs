use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use krit_capability::{HttpOrigin, is_valid_resource_name};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

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
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct PermissionPlan {
    pub schema: u32,
    pub package: String,
    pub requested: Vec<PermissionRequest>,
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
    message: String,
}

impl ManifestError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
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
        for (name, requirement) in &self.dependencies {
            validate_package_name(name)?;
            VersionReq::parse(requirement).map_err(|error| {
                ManifestError::new(format!(
                    "invalid version requirement `{requirement}` for `{name}`: {error}"
                ))
            })?;
        }
        validate_unique_names("configuration key", &self.capabilities.config)?;
        validate_capability_count("configuration key", &self.capabilities.config)?;
        for name in &self.capabilities.config {
            validate_resource_name("configuration key", name)?;
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
        Ok(())
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
        requested.extend(self.capabilities.secrets.iter().cloned().map(|resource| {
            PermissionRequest {
                capability: "secret.read",
                resource: Some(resource),
            }
        }));
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
        requested.sort();
        PermissionPlan {
            schema: 1,
            package: self.package.name.clone(),
            requested,
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
        output.push_str("Deployment grants: not evaluated\n");
        output
    }

    pub fn render_json(&self) -> String {
        serde_json::to_string(self).expect("permission plan serialization cannot fail")
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
        "krit/json" = "1.2.3"

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
        assert!(error.to_string().contains("unknown field"));
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
            "{\"schema\":1,\"package\":\"akshay/krit\",\"requested\":[{\"capability\":\"config.read\",\"resource\":\"agent.model\"},{\"capability\":\"config.read\",\"resource\":\"agent.timeout-ms\"},{\"capability\":\"http.request\",\"resource\":\"https://api.github.com\"},{\"capability\":\"http.request\",\"resource\":\"https://slack.com:8443\"},{\"capability\":\"io.stdout\"},{\"capability\":\"secret.read\",\"resource\":\"github-token\"},{\"capability\":\"secret.read\",\"resource\":\"slack-token\"}],\"grantStatus\":\"not-evaluated\"}"
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
}
