use std::collections::BTreeSet;

use krit_package::Manifest;
use krit_wasm::{ArtifactMetadata, PROGRAM_WORLD, PURE_PROGRAM_WORLD, STDOUT_INTERFACE};
use serde::Serialize;

use crate::RuntimeError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantSet {
    package_name: String,
    package_version: String,
    edition: String,
    target: String,
    entry: String,
    requested: Vec<PermissionFact>,
    effects: BTreeSet<String>,
    imports: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PermissionFact {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePermissions {
    pub schema: u32,
    pub package: String,
    pub world: String,
    pub requested: Vec<PermissionFact>,
    pub required: Vec<PermissionFact>,
    pub effective: Vec<PermissionFact>,
    pub denied: Vec<PermissionFact>,
    pub imports: Vec<String>,
    pub denial_reasons: Vec<String>,
    pub local_grant_status: &'static str,
    pub deployment_grant_status: &'static str,
}

impl GrantSet {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        let mut requested = Vec::new();
        let mut effects = BTreeSet::new();
        let mut imports = BTreeSet::new();
        if manifest.capabilities.stdout {
            requested.push(PermissionFact {
                capability: "io.stdout".to_owned(),
                resource: None,
            });
            effects.insert("io.stdout".to_owned());
            imports.insert(STDOUT_INTERFACE.to_owned());
        }
        requested.extend(
            manifest
                .capabilities
                .config
                .iter()
                .cloned()
                .map(|resource| PermissionFact {
                    capability: "config.read".to_owned(),
                    resource: Some(resource),
                }),
        );
        requested.extend(manifest.capabilities.http.iter().cloned().map(|resource| {
            PermissionFact {
                capability: "http.request".to_owned(),
                resource: Some(resource),
            }
        }));
        requested.extend(
            manifest
                .capabilities
                .secrets
                .iter()
                .cloned()
                .map(|resource| PermissionFact {
                    capability: "secret.read".to_owned(),
                    resource: Some(resource),
                }),
        );
        requested.sort();
        Self {
            package_name: manifest.package.name.clone(),
            package_version: manifest.package.version.clone(),
            edition: manifest.package.edition.clone(),
            target: manifest.package.target.clone(),
            entry: manifest.package.entry.to_string_lossy().replace('\\', "/"),
            requested,
            effects,
            imports,
        }
    }

    pub(crate) fn authorize(&self, metadata: &ArtifactMetadata) -> Result<(), RuntimeError> {
        let identity_denials = self.identity_denials(metadata);
        if !identity_denials.is_empty() {
            return Err(RuntimeError::authorization(identity_denials.join("; ")));
        }
        let evaluation = self.evaluate(metadata);
        if !evaluation.denied.is_empty() {
            return Err(RuntimeError::authorization(format!(
                "required capability `{}` is not granted by the manifest",
                evaluation.denied[0].capability
            )));
        }
        if !valid_policy_world(metadata)
            || metadata
                .imports
                .iter()
                .any(|import| !self.imports.contains(import))
        {
            return Err(RuntimeError::import_mismatch(
                "artifact imports are not exactly authorized by the manifest-derived grant set",
            ));
        }
        Ok(())
    }

    pub(crate) fn evaluate(&self, metadata: &ArtifactMetadata) -> EffectivePermissions {
        let required = metadata
            .effects
            .iter()
            .cloned()
            .map(|capability| PermissionFact {
                capability,
                resource: None,
            })
            .collect::<Vec<_>>();
        let effective = required
            .iter()
            .filter(|fact| self.effects.contains(&fact.capability))
            .cloned()
            .collect::<Vec<_>>();
        let denied = required
            .iter()
            .filter(|fact| !self.effects.contains(&fact.capability))
            .cloned()
            .collect::<Vec<_>>();
        let mut denial_reasons = self.identity_denials(metadata);
        if !valid_policy_world(metadata) {
            denial_reasons.push(
                "artifact world, effects, and imports do not form an authorized policy 1 world"
                    .to_owned(),
            );
        }
        let local_grant_status = if denied.is_empty() && denial_reasons.is_empty() {
            "allowed"
        } else {
            "denied"
        };
        EffectivePermissions {
            schema: 1,
            package: metadata.package.name.clone(),
            world: metadata.world.clone(),
            requested: self.requested.clone(),
            required,
            effective,
            denied,
            imports: metadata.imports.clone(),
            denial_reasons,
            local_grant_status,
            deployment_grant_status: "not-evaluated",
        }
    }

    fn identity_denials(&self, metadata: &ArtifactMetadata) -> Vec<String> {
        let mut denial_reasons = Vec::new();
        for (name, expected, actual) in [
            (
                "package name",
                self.package_name.as_str(),
                metadata.package.name.as_str(),
            ),
            (
                "package version",
                self.package_version.as_str(),
                metadata.package.version.as_str(),
            ),
            ("edition", self.edition.as_str(), metadata.edition.as_str()),
            ("target", self.target.as_str(), metadata.target.as_str()),
            ("entry", self.entry.as_str(), metadata.entry.as_str()),
        ] {
            if expected != actual {
                denial_reasons.push(format!(
                    "artifact {name} `{actual}` does not match manifest `{expected}`"
                ));
            }
        }
        denial_reasons
    }
}

impl EffectivePermissions {
    pub fn allowed(&self) -> bool {
        self.local_grant_status == "allowed"
    }

    pub fn render_human(&self) -> String {
        let mut output = format!(
            "Effective permissions for {} (schema {}):\n",
            self.package, self.schema
        );
        render_facts(&mut output, "Requested", &self.requested);
        render_facts(&mut output, "Required", &self.required);
        render_facts(&mut output, "Effective", &self.effective);
        render_facts(&mut output, "Denied", &self.denied);
        output.push_str("Imports:\n");
        if self.imports.is_empty() {
            output.push_str("  (none)\n");
        } else {
            for import in &self.imports {
                output.push_str("  ");
                output.push_str(import);
                output.push('\n');
            }
        }
        if !self.denial_reasons.is_empty() {
            output.push_str("Denial reasons:\n");
            for reason in &self.denial_reasons {
                output.push_str("  ");
                output.push_str(reason);
                output.push('\n');
            }
        }
        output.push_str("Local manifest grants: ");
        output.push_str(self.local_grant_status);
        output.push('\n');
        output.push_str("Deployment grants: not evaluated\n");
        output
    }

    pub fn render_json(&self) -> String {
        serde_json::to_string(self).expect("effective permissions serialization cannot fail")
    }
}

fn render_facts(output: &mut String, heading: &str, facts: &[PermissionFact]) {
    output.push_str(heading);
    output.push_str(":\n");
    if facts.is_empty() {
        output.push_str("  (none)\n");
        return;
    }
    for fact in facts {
        output.push_str("  ");
        output.push_str(&fact.capability);
        if let Some(resource) = &fact.resource {
            output.push_str(": ");
            output.push_str(resource);
        }
        output.push('\n');
    }
}

fn valid_policy_world(metadata: &ArtifactMetadata) -> bool {
    match metadata.effects.as_slice() {
        [] => metadata.world == PURE_PROGRAM_WORLD && metadata.imports.is_empty(),
        [effect] if effect == "io.stdout" => {
            metadata.world == PROGRAM_WORLD && metadata.imports == [STDOUT_INTERFACE]
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use krit_wasm::{
        ARTIFACT_METADATA_SCHEMA, ARTIFACT_POLICY_VERSION, COMPILER_NAME, LANGUAGE_NAME,
        LANGUAGE_VERSION, LanguageMetadata, PackageMetadata, VersionedTool, WASM_COMPONENT_TARGET,
    };

    use super::*;

    #[test]
    fn distinguishes_identity_and_interface_authorization_failures() {
        let grants = GrantSet::from_manifest(
            &Manifest::parse(
                r#"
schema = 1

[package]
name = "test/program"
version = "1.2.3"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
            )
            .expect("manifest should parse"),
        );
        let metadata = metadata();

        let mut identity = metadata.clone();
        identity.package.name = "other/program".to_owned();
        let error = grants.authorize(&identity).expect_err("identity must fail");
        assert_eq!(error.code(), "K5001");

        let mut world = metadata.clone();
        world.world = PURE_PROGRAM_WORLD.to_owned();
        let error = grants.authorize(&world).expect_err("world must fail");
        assert_eq!(error.code(), "K5002");

        let mut imports = metadata;
        imports.imports.clear();
        let error = grants.authorize(&imports).expect_err("imports must fail");
        assert_eq!(error.code(), "K5002");
    }

    fn metadata() -> ArtifactMetadata {
        ArtifactMetadata {
            schema: ARTIFACT_METADATA_SCHEMA,
            compiler: VersionedTool {
                name: COMPILER_NAME.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            language: LanguageMetadata {
                name: LANGUAGE_NAME.to_owned(),
                version: LANGUAGE_VERSION.to_owned(),
            },
            edition: "2026".to_owned(),
            package: PackageMetadata {
                name: "test/program".to_owned(),
                version: "1.2.3".to_owned(),
            },
            target: WASM_COMPONENT_TARGET.to_owned(),
            world: PROGRAM_WORLD.to_owned(),
            entry: "src/main.krit".to_owned(),
            digest: "unused".to_owned(),
            byte_size: 0,
            effects: vec!["io.stdout".to_owned()],
            imports: vec![STDOUT_INTERFACE.to_owned()],
            build_profile: "default".to_owned(),
            policy_version: ARTIFACT_POLICY_VERSION,
        }
    }
}
