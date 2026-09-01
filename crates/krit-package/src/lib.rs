use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub package: Package,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Default, Deserialize)]
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
        for name in &self.capabilities.config {
            validate_resource_name("configuration key", name)?;
        }
        validate_unique_names("HTTP origin", &self.capabilities.http)?;
        for origin in &self.capabilities.http {
            validate_http_origin(origin)?;
        }
        validate_unique_names("secret name", &self.capabilities.secrets)?;
        for name in &self.capabilities.secrets {
            validate_resource_name("secret name", name)?;
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
        requested.sort();
        PermissionPlan {
            schema: 1,
            package: self.package.name.clone(),
            requested,
            grant_status: "not-evaluated",
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

fn validate_resource_name(kind: &str, name: &str) -> Result<(), ManifestError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.ends_with('-')
        && !name.contains("..")
        && !name.contains("--")
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        });
    if !valid {
        return Err(ManifestError::new(format!(
            "{kind} `{name}` must use 1-64 lowercase letters, digits, `.` or `-`"
        )));
    }
    Ok(())
}

fn validate_http_origin(origin: &str) -> Result<(), ManifestError> {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return Err(invalid_origin(origin));
    };
    if !matches!(scheme, "http" | "https")
        || authority.is_empty()
        || authority
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'?' | b'#' | b'@'))
    {
        return Err(invalid_origin(origin));
    }

    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    if let Some(port) = port {
        let port = port.parse::<u16>().map_err(|_| invalid_origin(origin))?;
        if port == 0 {
            return Err(invalid_origin(origin));
        }
    }

    let valid_host = host.len() <= 253
        && host.contains('.')
        && host != "localhost"
        && !host.ends_with(".local")
        && !host
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
    if !valid_host {
        return Err(invalid_origin(origin));
    }
    Ok(())
}

fn invalid_origin(origin: &str) -> ManifestError {
    ManifestError::new(format!(
        "HTTP origin `{origin}` must be `http[s]://lowercase.dns.name[:port]` without a path"
    ))
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
        http = ["https://api.github.com", "https://slack.com:443"]
        secrets = ["github-token", "slack-token"]
    "#;

    #[test]
    fn validates_a_strict_manifest() {
        let manifest = Manifest::parse(VALID).expect("manifest should be valid");
        assert_eq!(manifest.package.name, "akshay/krit");
        assert!(manifest.capabilities.stdout);
        assert_eq!(manifest.permission_plan().requested.len(), 7);
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
            "{\"schema\":1,\"package\":\"akshay/krit\",\"requested\":[{\"capability\":\"config.read\",\"resource\":\"agent.model\"},{\"capability\":\"config.read\",\"resource\":\"agent.timeout-ms\"},{\"capability\":\"http.request\",\"resource\":\"https://api.github.com\"},{\"capability\":\"http.request\",\"resource\":\"https://slack.com:443\"},{\"capability\":\"io.stdout\"},{\"capability\":\"secret.read\",\"resource\":\"github-token\"},{\"capability\":\"secret.read\",\"resource\":\"slack-token\"}],\"grantStatus\":\"not-evaluated\"}"
        );
    }

    #[test]
    fn rejects_unsafe_http_origins() {
        for origin in [
            "https://api.github.com/path",
            "https://user@api.github.com",
            "https://127.0.0.1",
            "https://metadata.local",
            "HTTPS://api.github.com",
        ] {
            let manifest = VALID.replace(
                "https://api.github.com\", \"https://slack.com:443",
                &format!("{origin}\", \"https://slack.com:443"),
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
}
