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
    ARTIFACT_METADATA_SCHEMA, ARTIFACT_POLICY_VERSION, ArtifactMetadata, BuildOptions,
    COMPILER_NAME, LANGUAGE_NAME, LANGUAGE_VERSION, LanguageMetadata, PackageMetadata,
    VersionedTool,
};
pub use support::SUPPORTED_BACKEND_SEMANTICS;
pub use validation::{
    ComponentInspection, EMBEDDED_METADATA_SECTION, digest_bytes, validate_artifact,
    validate_component,
};
pub use wit::{PROGRAM_WORLD, PURE_PROGRAM_WORLD, STDOUT_INTERFACE};

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

    let (resolve, world, contract) = load_contract(&checked.effects)?;
    let mut core = encode_core(module, &contract, &checked.minimum_literal_operands)?;
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

    let embedded =
        EmbeddedMetadata::new(&options.edition, &contract.world, checked.effects.clone());
    let embedded = serde_json::to_vec(&embedded).map_err(|error| {
        BuildError::artifact(format!("could not serialize component metadata: {error}"))
    })?;
    if embedded.len() > 1024 {
        return Err(BuildError::artifact(
            "component metadata exceeds the policy 1 size limit",
        ));
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
        imports: inspection.imports,
        build_profile: options.build_profile.clone(),
        policy_version: ARTIFACT_POLICY_VERSION,
    };
    validate_artifact(&bytes, &metadata)?;
    Ok(BuiltComponent { bytes, metadata })
}
