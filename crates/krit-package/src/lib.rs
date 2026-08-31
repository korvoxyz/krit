use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use semver::{Version, VersionReq};
use serde::Deserialize;

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
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub stdout: bool,
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
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        let entry = root.join(&manifest.package.entry);
        if !entry.is_file() {
            return Err(ManifestError::new(format!(
                "package entry `{}` does not exist or is not a file",
                manifest.package.entry.display()
            )));
        }
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
        Ok(())
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
    "#;

    #[test]
    fn validates_a_strict_manifest() {
        let manifest = Manifest::parse(VALID).expect("manifest should be valid");
        assert_eq!(manifest.package.name, "akshay/krit");
        assert!(manifest.capabilities.stdout);
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
}
