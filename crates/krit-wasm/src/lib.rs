mod agent_compiler;
mod compiler;
mod error;
mod metadata;
mod support;
mod validation;
mod wit;

use std::borrow::Cow;

use krit::CoreModule;
use wasm_encoder::{CustomSection, Encode, Section};
use wasm_metadata::{AddMetadata, AddMetadataField};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};

pub use error::{BuildError, BuildErrorKind};
pub use metadata::{
    ARTIFACT_METADATA_SCHEMA, ARTIFACT_POLICY_VERSION, ApprovalRequirementMetadata,
    ArtifactMetadata, BuildOptions, COMPILER_NAME, LANGUAGE_NAME, LANGUAGE_VERSION,
    LanguageMetadata, PackageMetadata, ResourceRequirementMetadata, STATE_ARTIFACT_POLICY_VERSION,
    VersionedTool,
};
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

pub fn build_component(
    module: &CoreModule,
    options: &BuildOptions,
) -> Result<BuiltComponent, BuildError> {
    options.validate()?;
    let checked = support::check_module(module)?;
    for effect in &checked.effects {
        if options.granted_effects.binary_search(effect).is_err() {
            return Err(BuildError::capability(
                format!("required capability `{effect}` is not granted by the package"),
                support::first_effect_span(module),
            ));
        }
    }

    let (resolve, world, contract) = load_contract(checked.kind, &checked.effects)?;
    let mut core = encode_core(
        module,
        checked.entrypoint,
        &contract,
        &checked.minimum_literal_operands,
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
