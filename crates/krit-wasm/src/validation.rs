use std::collections::BTreeSet;

use wasm_metadata::Producers;
use wasmparser::{
    ComponentExternalKind, Encoding, ExternalKind, Parser, Payload, TypeRef, Validator,
    WasmFeatures,
};
use wit_component::DecodedWasm;

use crate::{
    ARTIFACT_METADATA_SCHEMA, ARTIFACT_POLICY_VERSION, ArtifactMetadata, BuildError, COMPILER_NAME,
    EmbeddedMetadata, LANGUAGE_NAME, LANGUAGE_VERSION, STDOUT_INTERFACE, WASM_COMPONENT_TARGET,
    wit::{STDOUT_EFFECT, WitContract, contract_from_world, load_contract},
};

pub const EMBEDDED_METADATA_SECTION: &str = "krit.metadata";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentInspection {
    pub world: String,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub effects: Vec<String>,
    pub core_module_count: u32,
    pub table_count: u32,
    pub memory_count: u32,
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

pub(crate) fn validate_core(
    bytes: &[u8],
    contract: &WitContract,
    table_size: u32,
) -> Result<(), BuildError> {
    Validator::new_with_features(WasmFeatures::empty())
        .validate_all(bytes)
        .map_err(|error| {
            BuildError::artifact(format!(
                "core WebAssembly failed policy 1 validation: {error}"
            ))
        })?;

    let mut expected_imports = contract
        .imports
        .iter()
        .map(|import| (import.module.as_str(), import.name.as_str()))
        .collect::<Vec<_>>();
    expected_imports.sort_unstable();
    let mut actual_imports = Vec::new();
    let mut exports = BTreeSet::new();
    let mut tables = Vec::new();
    let mut memory_count = 0u32;
    let mut top_encoding = None;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(parse_error)? {
            Payload::Version { encoding, .. } if top_encoding.is_none() => {
                top_encoding = Some(encoding);
            }
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.map_err(parse_error)?;
                    if !matches!(import.ty, TypeRef::Func(_)) {
                        return Err(BuildError::artifact(
                            "core WebAssembly contains a non-function import",
                        ));
                    }
                    actual_imports.push((import.module, import.name));
                }
            }
            Payload::ExportSection(section) => {
                for export in section {
                    let export = export.map_err(parse_error)?;
                    if export.kind != ExternalKind::Func {
                        return Err(BuildError::artifact(
                            "core WebAssembly contains a non-function export",
                        ));
                    }
                    exports.insert(export.name);
                }
            }
            Payload::TableSection(section) => {
                for table in section {
                    tables.push(table.map_err(parse_error)?.ty);
                }
            }
            Payload::MemorySection(section) => {
                memory_count = memory_count
                    .checked_add(section.count())
                    .ok_or_else(|| BuildError::artifact("too many core WebAssembly memories"))?;
            }
            Payload::StartSection { .. } => {
                return Err(BuildError::artifact(
                    "core WebAssembly start functions are forbidden",
                ));
            }
            Payload::TagSection(_) => {
                return Err(BuildError::artifact(
                    "core WebAssembly exception tags are forbidden",
                ));
            }
            Payload::UnknownSection { .. } => {
                return Err(BuildError::artifact(
                    "core WebAssembly contains an unknown section",
                ));
            }
            _ => {}
        }
    }

    if top_encoding != Some(Encoding::Module) {
        return Err(BuildError::artifact(
            "compiler output is not a core WebAssembly module",
        ));
    }
    actual_imports.sort_unstable();
    if actual_imports != expected_imports {
        return Err(BuildError::artifact(
            "core WebAssembly imports do not match the parsed WIT contract",
        ));
    }
    let expected_exports = BTreeSet::from([
        contract.run_export.as_str(),
        contract.post_run_export.as_str(),
    ]);
    if exports != expected_exports {
        return Err(BuildError::artifact(
            "core WebAssembly exports do not match the parsed WIT contract",
        ));
    }
    if memory_count != 0 {
        return Err(BuildError::artifact(
            "WebAssembly policy 1 does not permit guest linear memory",
        ));
    }
    if tables.len() != 1 {
        return Err(BuildError::artifact(
            "WebAssembly policy 1 requires exactly one bounded function table",
        ));
    }
    let table = tables[0];
    if table.table64
        || table.shared
        || table.element_type != wasmparser::RefType::FUNCREF
        || table.initial != u64::from(table_size)
        || table.maximum != Some(u64::from(table_size))
    {
        return Err(BuildError::artifact(
            "core WebAssembly function table violates policy 1 bounds",
        ));
    }
    Ok(())
}

pub fn validate_component(bytes: &[u8]) -> Result<ComponentInspection, BuildError> {
    let features = WasmFeatures::COMPONENT_MODEL;
    Validator::new_with_features(features)
        .validate_all(bytes)
        .map_err(|error| {
            BuildError::artifact(format!(
                "WebAssembly component failed policy 1 validation: {error}"
            ))
        })?;

    let mut depth = 0u32;
    let mut top_encoding = None;
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut core_imports = Vec::new();
    let mut core_module_count = 0u32;
    let mut table_count = 0u32;
    let mut memory_count = 0u32;
    let mut embedded = None;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(parse_error)? {
            Payload::Version { encoding, .. } if top_encoding.is_none() => {
                top_encoding = Some(encoding);
            }
            Payload::ModuleSection { .. } => {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| BuildError::artifact("component nesting is too deep"))?;
                core_module_count = core_module_count
                    .checked_add(1)
                    .ok_or_else(|| BuildError::artifact("too many core modules"))?;
            }
            Payload::ComponentSection { .. } => {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| BuildError::artifact("component nesting is too deep"))?;
            }
            Payload::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Payload::ComponentImportSection(section) if depth == 0 => {
                for import in section {
                    let import = import.map_err(parse_error)?;
                    if import.name.implements.is_some()
                        || import.name.version_suffix.is_some()
                        || import.name.external_id.is_some()
                        || !matches!(
                            import.ty,
                            wasmparser::ComponentTypeRef::Instance(_)
                                | wasmparser::ComponentTypeRef::Func(_)
                        )
                    {
                        return Err(BuildError::artifact(
                            "component import uses a forbidden name or type feature",
                        ));
                    }
                    imports.push(import.name.name.to_owned());
                }
            }
            Payload::ComponentExportSection(section) if depth == 0 => {
                for export in section {
                    let export = export.map_err(parse_error)?;
                    if export.name.implements.is_some()
                        || export.name.version_suffix.is_some()
                        || export.name.external_id.is_some()
                        || export.kind != ComponentExternalKind::Func
                    {
                        return Err(BuildError::artifact(
                            "component export uses a forbidden name or type",
                        ));
                    }
                    exports.push(export.name.name.to_owned());
                }
            }
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.map_err(parse_error)?;
                    if !matches!(import.ty, TypeRef::Func(_)) {
                        return Err(BuildError::artifact(
                            "embedded core module contains a non-function import",
                        ));
                    }
                    core_imports.push((import.module.to_owned(), import.name.to_owned()));
                }
            }
            Payload::TableSection(section) => {
                for table in section {
                    let table = table.map_err(parse_error)?.ty;
                    table_count = table_count
                        .checked_add(1)
                        .ok_or_else(|| BuildError::artifact("too many function tables"))?;
                    if table.table64
                        || table.shared
                        || table.maximum.is_none()
                        || table.element_type != wasmparser::RefType::FUNCREF
                    {
                        return Err(BuildError::artifact(
                            "embedded core function table violates policy 1",
                        ));
                    }
                }
            }
            Payload::MemorySection(section) => {
                for memory in section {
                    let memory = memory.map_err(parse_error)?;
                    memory_count = memory_count
                        .checked_add(1)
                        .ok_or_else(|| BuildError::artifact("too many memories"))?;
                    if memory.memory64
                        || memory.shared
                        || memory.maximum.is_none()
                        || memory.page_size_log2.is_some()
                    {
                        return Err(BuildError::artifact(
                            "embedded core memory violates policy 1",
                        ));
                    }
                }
            }
            Payload::StartSection { .. } | Payload::ComponentStartSection { .. } => {
                return Err(BuildError::artifact(
                    "WebAssembly start functions are forbidden by policy 1",
                ));
            }
            Payload::TagSection(_) => {
                return Err(BuildError::artifact(
                    "WebAssembly exception tags are forbidden by policy 1",
                ));
            }
            Payload::CustomSection(section)
                if depth == 0 && section.name() == EMBEDDED_METADATA_SECTION =>
            {
                if embedded.is_some() || section.data().len() > 1024 {
                    return Err(BuildError::artifact(
                        "component metadata section is duplicate or oversized",
                    ));
                }
                embedded = Some(
                    serde_json::from_slice::<EmbeddedMetadata>(section.data()).map_err(
                        |error| {
                            BuildError::artifact(format!(
                                "component metadata section is invalid: {error}"
                            ))
                        },
                    )?,
                );
            }
            Payload::UnknownSection { .. } => {
                return Err(BuildError::artifact(
                    "WebAssembly component contains an unknown section",
                ));
            }
            _ => {}
        }
    }

    if top_encoding != Some(Encoding::Component) {
        return Err(BuildError::artifact(
            "artifact is not a WebAssembly component",
        ));
    }
    imports.sort();
    exports.sort();
    if exports != ["run"] {
        return Err(BuildError::artifact(
            "component exports do not match a policy 1 WIT world",
        ));
    }
    let effects = match imports.as_slice() {
        [] => Vec::new(),
        [import] if import == STDOUT_INTERFACE => vec![STDOUT_EFFECT.to_owned()],
        _ => {
            return Err(BuildError::artifact(
                "component imports do not match WebAssembly policy 1",
            ));
        }
    };
    let (_, _, contract) = load_contract(&effects)?;
    let mut expected_core_imports = contract
        .imports
        .iter()
        .map(|import| (import.module.clone(), import.name.clone()))
        .collect::<Vec<_>>();
    expected_core_imports.sort();
    core_imports.sort();
    if core_imports != expected_core_imports {
        return Err(BuildError::artifact(
            "embedded core imports do not match the selected WIT world",
        ));
    }
    if core_module_count != 1 || table_count != 1 || memory_count != 0 {
        return Err(BuildError::artifact(
            "component core module, table, or memory shape violates policy 1",
        ));
    }

    let embedded =
        embedded.ok_or_else(|| BuildError::artifact("component metadata section is missing"))?;
    if embedded.schema != ARTIFACT_METADATA_SCHEMA
        || embedded.compiler_version != env!("CARGO_PKG_VERSION")
        || embedded.edition != "2026"
        || embedded.world != contract.world
        || embedded.effects != effects
        || embedded.policy_version != ARTIFACT_POLICY_VERSION
        || !sorted_unique(&embedded.effects)
    {
        return Err(BuildError::artifact(
            "component metadata section does not match policy 1",
        ));
    }

    verify_decoded_world(bytes, &contract)?;
    verify_producers(bytes)?;

    Ok(ComponentInspection {
        world: contract.world,
        imports,
        exports,
        effects,
        core_module_count,
        table_count,
        memory_count,
    })
}

pub fn validate_artifact(
    bytes: &[u8],
    metadata: &ArtifactMetadata,
) -> Result<ComponentInspection, BuildError> {
    if metadata.schema != ARTIFACT_METADATA_SCHEMA
        || metadata.policy_version != ARTIFACT_POLICY_VERSION
        || metadata.compiler.name != COMPILER_NAME
        || metadata.compiler.version != env!("CARGO_PKG_VERSION")
        || metadata.language.name != LANGUAGE_NAME
        || metadata.language.version != LANGUAGE_VERSION
        || metadata.edition != "2026"
        || metadata.target != WASM_COMPONENT_TARGET
        || metadata.package.name.is_empty()
        || metadata.package.version.is_empty()
        || !safe_entry(&metadata.entry)
        || !sorted_unique(&metadata.effects)
        || !sorted_unique(&metadata.imports)
    {
        return Err(BuildError::metadata(
            "artifact metadata does not match schema 1 policy",
        ));
    }
    if metadata.byte_size != bytes.len() as u64 {
        return Err(BuildError::digest(
            "artifact byte size does not match metadata",
        ));
    }
    let digest = digest_bytes(bytes);
    if metadata.digest != digest {
        return Err(BuildError::digest(
            "artifact BLAKE3 digest does not match metadata",
        ));
    }

    let inspection = validate_component(bytes)?;
    if metadata.world != inspection.world
        || metadata.imports != inspection.imports
        || metadata.effects != inspection.effects
    {
        return Err(BuildError::metadata(
            "artifact metadata world, imports, or effects do not match the component",
        ));
    }
    Ok(inspection)
}

fn verify_decoded_world(bytes: &[u8], expected: &WitContract) -> Result<(), BuildError> {
    let decoded = wit_component::decode(bytes).map_err(|error| {
        BuildError::artifact(format!("could not decode component WIT world: {error}"))
    })?;
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(BuildError::artifact(
            "artifact does not decode as a component WIT world",
        ));
    };
    let decoded = contract_from_world(&resolve, world)?;
    let mut decoded_imports = decoded.imports;
    decoded_imports.sort();
    let mut expected_imports = expected.imports.clone();
    expected_imports.sort();
    if decoded.component_imports != expected.component_imports
        || decoded_imports != expected_imports
        || decoded.run_export != expected.run_export
        || decoded.post_run_export != expected.post_run_export
        || decoded.run_signature != expected.run_signature
    {
        return Err(BuildError::artifact(
            "decoded component does not implement the selected WIT world",
        ));
    }
    Ok(())
}

fn verify_producers(bytes: &[u8]) -> Result<(), BuildError> {
    let producers = Producers::from_wasm(bytes)
        .map_err(|error| BuildError::artifact(format!("invalid producers metadata: {error}")))?
        .ok_or_else(|| BuildError::artifact("component producers metadata is missing"))?;
    let language = producers
        .get("language")
        .and_then(|field| field.get(LANGUAGE_NAME));
    let compiler = producers
        .get("processed-by")
        .and_then(|field| field.get(COMPILER_NAME));
    if language.map(String::as_str) != Some(LANGUAGE_VERSION)
        || compiler.map(String::as_str) != Some(env!("CARGO_PKG_VERSION"))
    {
        return Err(BuildError::artifact(
            "component producers metadata does not identify Krit",
        ));
    }
    Ok(())
}

fn sorted_unique(values: &[String]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn safe_entry(entry: &str) -> bool {
    !entry.starts_with('/')
        && !entry.contains('\\')
        && entry
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn parse_error(error: wasmparser::BinaryReaderError) -> BuildError {
    BuildError::artifact(format!("could not inspect WebAssembly artifact: {error}"))
}
