mod agent_compiler;
mod compatibility;
mod compiler;
mod error;
mod metadata;
mod strings;
mod support;
mod validation;
mod wit;

use std::borrow::Cow;

use krit::CoreModule;
use wasm_encoder::{CustomSection, Encode, Section};
use wasm_metadata::{AddMetadata, AddMetadataField};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};

pub use compatibility::{
    PREVIOUS_MINOR_TOOLCHAINS, SUPPORTED_EDITIONS, SUPPORTED_METADATA_SCHEMAS,
    SUPPORTED_POLICY_VERSIONS, SUPPORTED_RUNTIME_WIT_VERSIONS, toolchain_in_window,
};
pub use error::{BuildError, BuildErrorKind};
pub use metadata::{
    ARTIFACT_METADATA_SCHEMA, ARTIFACT_POLICY_VERSION, ApprovalRequirementMetadata,
    ArtifactMetadata, BuildOptions, COMPILER_NAME, LANGUAGE_NAME, LANGUAGE_VERSION,
    LanguageMetadata, PackageMetadata, ResourceRequirementMetadata, STATE_ARTIFACT_POLICY_VERSION,
    VersionedTool,
};
pub use strings::JSON_DECODE_FUNCTION_NAME;
pub use support::SUPPORTED_BACKEND_SEMANTICS;
pub use validation::{
    ComponentInspection, EMBEDDED_METADATA_SECTION, MAX_EMBEDDED_METADATA_BYTES, digest_bytes,
    validate_artifact, validate_component,
};
pub use wit::{
    AI_INTERFACE, CACHE_READ_INTERFACE, CACHE_WRITE_INTERFACE, CONFIG_INTERFACE,
    DATABASE_INTERFACE, HTTP_ANONYMOUS_INTERFACE, HTTP_INTERFACE, JOB_INTERFACE, JOB_PROGRAM_WORLD,
    LOGGING_INTERFACE, OBJECTS_READ_INTERFACE, OBJECTS_WRITE_INTERFACE, PROGRAM_WORLD,
    PURE_PROGRAM_WORLD, QUEUE_INTERFACE, SCHEDULE_INTERFACE, SCHEDULE_PROGRAM_WORLD,
    SEARCH_QUERY_INTERFACE, SEARCH_VECTOR_INTERFACE, SECRETS_INTERFACE, STATE_INTERFACE,
    STDOUT_INTERFACE, WEBHOOK_ALL_PROGRAM_WORLD, WEBHOOK_INTERFACE, WEBHOOK_PROGRAM_WORLD,
    WEBHOOK_STATE_ALL_PROGRAM_WORLD,
};

use compiler::encode_core;
use metadata::EmbeddedMetadata;
use validation::validate_core;
use wit::load_contract;

pub const WASM_COMPONENT_TARGET: &str = "wasm-component";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltComponent {
    pub bytes: Vec<u8>,
    pub metadata: ArtifactMetadata,
}

/// Artifact target that a target check verifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckTarget {
    /// The component `build_component` emits for the program: the selected
    /// webhook, queue, or schedule world, or the module-program world.
    Component,
    /// A module program only; exported entrypoints are reported as `K7002`.
    Module,
}

/// Whether a build enforces the package's granted effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Grants {
    Enforce,
    Skip,
}

/// Builds and validates the component for `module`.
///
/// On failure this returns the first target diagnostic in source order; use
/// [`build_component_diagnostics`] to receive every diagnostic.
pub fn build_component(
    module: &CoreModule,
    options: &BuildOptions,
) -> Result<BuiltComponent, BuildError> {
    build_component_diagnostics(module, options).map_err(|errors| {
        errors
            .into_iter()
            .next()
            .unwrap_or_else(|| BuildError::artifact("component build failed without a diagnostic"))
    })
}

/// Builds and validates the component for `module`, reporting every target
/// diagnostic on failure.
///
/// The returned errors are non-empty and sorted by source span. Backend
/// support findings (`K7001`/`K7002`) are collected exhaustively before
/// emission starts; package authority (`K5001`) is checked next, and code
/// generation, componentization, and validation report their first failure.
pub fn build_component_diagnostics(
    module: &CoreModule,
    options: &BuildOptions,
) -> Result<BuiltComponent, Vec<BuildError>> {
    emit_component(module, options, Grants::Enforce)
}

/// Runs the complete artifact emission for `target` with the output
/// discarded and returns every target diagnostic, or nothing when the
/// program builds.
///
/// This is exactly the pipeline of [`build_component_diagnostics`] except
/// that package grants are not consulted: a target check has no manifest,
/// and `build` still enforces `K5001` against the package.
pub fn check_target(
    module: &CoreModule,
    options: &BuildOptions,
    target: CheckTarget,
) -> Vec<BuildError> {
    if target == CheckTarget::Module
        && let Some(entrypoint) = module
            .entrypoints()
            .iter()
            .find(|entrypoint| entrypoint.kind.is_exported())
    {
        let source = module
            .functions()
            .get(entrypoint.function.as_u32() as usize)
            .and_then(|function| function.source);
        return vec![BuildError::unsupported(
            format!(
                "`{}` entrypoint programs build only as components, not as module programs",
                entrypoint.kind.as_str()
            ),
            source,
        )];
    }
    emit_component(module, options, Grants::Skip)
        .err()
        .unwrap_or_default()
}

fn emit_component(
    module: &CoreModule,
    options: &BuildOptions,
    grants: Grants,
) -> Result<BuiltComponent, Vec<BuildError>> {
    options.validate().map_err(|error| vec![error])?;
    let checked = support::check_module(module)?;
    if grants == Grants::Enforce
        && let Some(effect) = checked
            .effects
            .iter()
            .find(|effect| options.granted_effects.binary_search(effect).is_err())
    {
        return Err(vec![BuildError::capability(
            format!("required capability `{effect}` is not granted by the package"),
            support::effect_span(module, checked.entrypoint, effect),
        )]);
    }
    assemble_component(module, options, &checked).map_err(|error| vec![error])
}

fn assemble_component(
    module: &CoreModule,
    options: &BuildOptions,
    checked: &support::CheckedModule,
) -> Result<BuiltComponent, BuildError> {
    let (resolve, world, contract) = load_contract(checked.kind, &checked.effects)?;
    let mut core = encode_core(
        module,
        checked.entrypoint,
        &contract,
        &checked.minimum_literal_operands,
        &checked.live,
    )?;
    validate_core(&core.bytes, &contract, core.table_size)?;
    embed_component_metadata(&mut core.bytes, &resolve, world, StringEncoding::UTF8).map_err(
        |error| BuildError::artifact(format!("could not embed the parsed WIT world: {error}")),
    )?;
    validate_core(&core.bytes, &contract, core.table_size)?;

    let mut encoder = ComponentEncoder::default();
    encoder
        .module(&core.bytes)
        .map_err(|error| BuildError::artifact(format!("could not register core module: {error}")))?
        .reject_legacy_names(true)
        .validate(true);
    let component = encoder.encode().map_err(|error| {
        BuildError::artifact(format!("could not componentize core WebAssembly: {error}"))
    })?;

    let mut standard_metadata = AddMetadata::default();
    standard_metadata.name = AddMetadataField::Set("krit-program".to_owned());
    standard_metadata.language = vec![(LANGUAGE_NAME.to_owned(), LANGUAGE_VERSION.to_owned())];
    standard_metadata.processed_by = vec![(
        COMPILER_NAME.to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    )];
    let mut bytes = standard_metadata.to_wasm(&component).map_err(|error| {
        BuildError::artifact(format!(
            "could not attach standard component metadata: {error}"
        ))
    })?;

    let policy_version = artifact_policy_version(&checked.effects);
    let embedded = EmbeddedMetadata::new(
        &options.edition,
        &contract.world,
        checked.effects.clone(),
        checked.requirements.clone(),
        checked.approvals.clone(),
        policy_version,
    );
    let embedded = serde_json::to_vec(&embedded).map_err(|error| {
        BuildError::artifact(format!("could not serialize component metadata: {error}"))
    })?;
    if embedded.len() > MAX_EMBEDDED_METADATA_BYTES {
        return Err(BuildError::artifact(format!(
            "component metadata exceeds the {}-byte policy limit",
            MAX_EMBEDDED_METADATA_BYTES
        )));
    }
    let section = CustomSection {
        name: Cow::Borrowed(EMBEDDED_METADATA_SECTION),
        data: Cow::Borrowed(&embedded),
    };
    bytes.push(section.id());
    section.encode(&mut bytes);

    let inspection = validate_component(&bytes)?;
    if inspection.effects != checked.effects || inspection.world != contract.world {
        return Err(BuildError::artifact(
            "validated component world or effects differ from checked Core",
        ));
    }

    let digest = digest_bytes(&bytes);
    let metadata = ArtifactMetadata {
        schema: ARTIFACT_METADATA_SCHEMA,
        compiler: VersionedTool {
            name: COMPILER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        language: LanguageMetadata {
            name: LANGUAGE_NAME.to_owned(),
            version: LANGUAGE_VERSION.to_owned(),
        },
        edition: options.edition.clone(),
        package: PackageMetadata {
            name: options.package_name.clone(),
            version: options.package_version.clone(),
        },
        target: options.target.clone(),
        world: inspection.world,
        entry: options.source_entry.clone(),
        digest,
        byte_size: bytes.len() as u64,
        effects: inspection.effects,
        requirements: inspection.requirements,
        approvals: inspection.approvals,
        imports: inspection.imports,
        build_profile: options.build_profile.clone(),
        policy_version,
    };
    validate_artifact(&bytes, &metadata)?;
    Ok(BuiltComponent { bytes, metadata })
}

/// Durable Phase 6 surfaces raise artifact validation to policy 2.
/// Host surfaces introduced after artifact policy 1.
///
/// An artifact that uses any of them selects policy 2. The list is *not* a
/// durability claim: `cache.read` and `cache.write` are explicitly
/// non-durable, and are here only because they postdate policy 1.
pub(crate) const POLICY_TWO_EFFECTS: [&str; 12] = [
    "cache.read",
    "cache.write",
    "database.read",
    "database.write",
    "object.read",
    "object.write",
    "queue.consume",
    "queue.publish",
    "schedule.trigger",
    "search.query",
    "search.vector",
    "state.transaction",
];

pub(crate) fn artifact_policy_version(effects: &[String]) -> u32 {
    if effects
        .iter()
        .any(|effect| POLICY_TWO_EFFECTS.contains(&effect.as_str()))
    {
        STATE_ARTIFACT_POLICY_VERSION
    } else {
        ARTIFACT_POLICY_VERSION
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use krit::{Source, analyze, lower, parse_source};

    use super::{BuildOptions, Grants, emit_component, support};

    fn corpus() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        // The CLI agreement test owns the target probes; they are read here
        // to prove the support check, not emission, explains each failure.
        for directory in ["examples", "crates/krit-cli/tests/target-probes"] {
            for entry in fs::read_dir(root.join(directory)).expect("corpus directory should exist")
            {
                let path = entry.expect("directory entry should be readable").path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "krit")
                {
                    files.push(path);
                }
            }
        }
        let mut pending = vec![root.join("conformance")];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).expect("conformance should be readable") {
                let path = entry.expect("directory entry should be readable").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.file_name().is_some_and(|name| name == "program.krit") {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    #[test]
    fn emission_never_fails_after_the_support_check_passes() {
        let options = BuildOptions::new("2026", "test/corpus", "1.0.0", "main.krit");
        let mut checked = 0;
        for path in corpus() {
            let text = fs::read_to_string(&path).expect("program should be readable");
            let source = Source::new("main.krit", text);
            let Ok(program) = parse_source(&source) else {
                continue;
            };
            let Ok(analysis) = analyze(&program) else {
                continue;
            };
            let module = lower(&program, &analysis).expect("analyzed program should lower");
            if support::check_module(&module).is_err() {
                continue;
            }
            checked += 1;
            if let Err(errors) = emit_component(&module, &options, Grants::Skip) {
                panic!(
                    "{}: emission failed after the support check passed: {errors:?}",
                    path.display()
                );
            }
        }
        assert!(
            checked >= 15,
            "the corpus should contain buildable programs"
        );
    }
}
