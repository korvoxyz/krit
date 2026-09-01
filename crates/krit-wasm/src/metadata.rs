use serde::{Deserialize, Serialize};

use crate::{BuildError, WASM_COMPONENT_TARGET};

pub const ARTIFACT_METADATA_SCHEMA: u32 = 1;
pub const ARTIFACT_POLICY_VERSION: u32 = 1;
pub const COMPILER_NAME: &str = "krit";
pub const LANGUAGE_NAME: &str = "Krit";
pub const LANGUAGE_VERSION: &str = "0.2.0";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactMetadata {
    pub schema: u32,
    pub compiler: VersionedTool,
    pub language: LanguageMetadata,
    pub edition: String,
    pub package: PackageMetadata,
    pub target: String,
    pub world: String,
    pub entry: String,
    pub digest: String,
    pub byte_size: u64,
    pub effects: Vec<String>,
    pub imports: Vec<String>,
    pub build_profile: String,
    pub policy_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionedTool {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanguageMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildOptions {
    pub edition: String,
    pub package_name: String,
    pub package_version: String,
    pub source_entry: String,
    pub target: String,
    pub build_profile: String,
    pub granted_effects: Vec<String>,
}

impl BuildOptions {
    pub fn new(
        edition: impl Into<String>,
        package_name: impl Into<String>,
        package_version: impl Into<String>,
        source_entry: impl Into<String>,
    ) -> Self {
        Self {
            edition: edition.into(),
            package_name: package_name.into(),
            package_version: package_version.into(),
            source_entry: source_entry.into(),
            target: WASM_COMPONENT_TARGET.to_owned(),
            build_profile: "default".to_owned(),
            granted_effects: Vec::new(),
        }
    }

    pub fn grant_effect(&mut self, effect: impl Into<String>) {
        let effect = effect.into();
        if !self.granted_effects.contains(&effect) {
            self.granted_effects.push(effect);
            self.granted_effects.sort();
        }
    }

    pub(crate) fn validate(&self) -> Result<(), BuildError> {
        if self.edition != "2026" {
            return Err(BuildError::metadata(format!(
                "unsupported artifact edition `{}`",
                self.edition
            )));
        }
        if self.target != WASM_COMPONENT_TARGET {
            return Err(BuildError::metadata(format!(
                "unsupported artifact target `{}`",
                self.target
            )));
        }
        for (description, value, maximum) in [
            ("package name", self.package_name.as_str(), 128),
            ("package version", self.package_version.as_str(), 64),
            ("source entry", self.source_entry.as_str(), 512),
            ("build profile", self.build_profile.as_str(), 32),
        ] {
            if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
                return Err(BuildError::metadata(format!(
                    "invalid artifact {description}"
                )));
            }
        }
        if !is_safe_relative_entry(&self.source_entry) {
            return Err(BuildError::metadata(
                "artifact source entry must be a forward-slash package-relative path",
            ));
        }
        if self
            .granted_effects
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(BuildError::metadata(
                "artifact granted effects must be sorted and unique",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EmbeddedMetadata {
    pub schema: u32,
    pub compiler_version: String,
    pub edition: String,
    pub world: String,
    pub effects: Vec<String>,
    pub policy_version: u32,
}

impl EmbeddedMetadata {
    pub(crate) fn new(edition: &str, world: &str, effects: Vec<String>) -> Self {
        Self {
            schema: ARTIFACT_METADATA_SCHEMA,
            compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
            edition: edition.to_owned(),
            world: world.to_owned(),
            effects,
            policy_version: ARTIFACT_POLICY_VERSION,
        }
    }
}

fn is_safe_relative_entry(entry: &str) -> bool {
    !entry.starts_with('/')
        && !entry.contains('\\')
        && entry
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}
