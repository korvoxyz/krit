use std::collections::BTreeSet;

use wasm_metadata::Producers;
use wasmparser::{
    ComponentExternalKind, Encoding, ExternalKind, Parser, Payload, TypeRef, Validator,
    WasmFeatures,
};
use wit_component::DecodedWasm;

use crate::{
    AI_INTERFACE, ARTIFACT_METADATA_SCHEMA, ApprovalRequirementMetadata, ArtifactMetadata,
    BuildError, COMPILER_NAME, CONFIG_INTERFACE, EmbeddedMetadata, HTTP_ANONYMOUS_INTERFACE,
    HTTP_INTERFACE, LANGUAGE_NAME, LANGUAGE_VERSION, LOGGING_INTERFACE,
    ResourceRequirementMetadata, SECRETS_INTERFACE, STATE_INTERFACE, STDOUT_INTERFACE,
    WASM_COMPONENT_TARGET, WEBHOOK_INTERFACE, artifact_policy_version,
    wit::{
        AI_EFFECT, CONFIG_EFFECT, HTTP_EFFECT, LOGGING_EFFECT, ProgramKind, SECRETS_EFFECT,
        STATE_EFFECT, STDOUT_EFFECT, WitContract, contract_from_world, load_contract,
    },
};

pub const EMBEDDED_METADATA_SECTION: &str = "krit.metadata";
pub const MAX_EMBEDDED_METADATA_BYTES: usize = 48 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentInspection {
    pub world: String,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub effects: Vec<String>,
    pub requirements: Vec<ResourceRequirementMetadata>,
    pub approvals: Vec<ApprovalRequirementMetadata>,
    pub core_module_count: u32,
    pub table_count: u32,
    pub table_elements: u64,
    pub memory_count: u32,
    pub memory_minimum_bytes: u64,
    pub memory_maximum_bytes: u64,
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
    let mut memories = Vec::new();
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
                    match export.kind {
                        ExternalKind::Func => {
                            exports.insert((export.name, 0u8));
                        }
                        ExternalKind::Memory if export.name == contract.memory_export => {
                            exports.insert((export.name, 1u8));
                        }
                        _ => {
                            return Err(BuildError::artifact(
                                "core WebAssembly contains an unexpected non-function export",
                            ));
                        }
                    }
                }
            }
            Payload::TableSection(section) => {
                for table in section {
                    tables.push(table.map_err(parse_error)?.ty);
                }
            }
            Payload::MemorySection(section) => {
                for memory in section {
                    let memory = memory.map_err(parse_error)?;
                    memory_count = memory_count.checked_add(1).ok_or_else(|| {
                        BuildError::artifact("too many core WebAssembly memories")
                    })?;
                    memories.push(memory);
                }
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
    let mut expected_exports = BTreeSet::from([
        (contract.entry_export.as_str(), 0u8),
        (contract.post_entry_export.as_str(), 0u8),
    ]);
    if contract.requires_memory {
        expected_exports.insert((contract.memory_export.as_str(), 1u8));
        expected_exports.insert((contract.realloc_export.as_str(), 0u8));
    }
    if exports != expected_exports {
        return Err(BuildError::artifact(
            "core WebAssembly exports do not match the parsed WIT contract",
        ));
    }
    if memory_count != u32::from(contract.requires_memory) {
        return Err(BuildError::artifact(
            "WebAssembly policy 1 core memory count does not match the selected WIT world",
        ));
    }
    if contract.requires_memory {
        let memory = memories[0];
        if memory.memory64
            || memory.shared
            || memory.initial == 0
            || memory.maximum != Some(256)
            || memory.page_size_log2.is_some()
        {
            return Err(BuildError::artifact(
                "bounded webhook core memory violates policy 1",
            ));
        }
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
    let mut table_elements = 0u64;
    let mut memory_count = 0u32;
    let mut memory_minimum_bytes = 0u64;
    let mut memory_maximum_bytes = 0u64;
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
                        || !matches!(
                            export.kind,
                            ComponentExternalKind::Func | ComponentExternalKind::Instance
                        )
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
                    if core_module_count == 1 {
                        if !matches!(import.ty, TypeRef::Func(_)) {
                            return Err(BuildError::artifact(format!(
                                "main core module contains a non-function import `{}::{}`",
                                import.module, import.name
                            )));
                        }
                        core_imports.push((import.module.to_owned(), import.name.to_owned()));
                    }
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
                    table_elements = table_elements
                        .checked_add(table.maximum.expect("bounded table checked above"))
                        .ok_or_else(|| BuildError::artifact("too many function table elements"))?;
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
                        || memory.maximum > Some(256)
                        || memory.page_size_log2.is_some()
                    {
                        return Err(BuildError::artifact(
                            "embedded core memory violates policy 1",
                        ));
                    }
                    memory_minimum_bytes = memory_minimum_bytes
                        .checked_add(memory.initial.saturating_mul(65_536))
                        .ok_or_else(|| BuildError::artifact("minimum memory size overflowed"))?;
                    memory_maximum_bytes = memory_maximum_bytes
                        .checked_add(
                            memory
                                .maximum
                                .expect("bounded memory checked above")
                                .saturating_mul(65_536),
                        )
                        .ok_or_else(|| BuildError::artifact("maximum memory size overflowed"))?;
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
                if embedded.is_some() {
                    return Err(BuildError::artifact(
                        "component metadata section is duplicate",
                    ));
                }
                if section.data().len() > MAX_EMBEDDED_METADATA_BYTES {
                    return Err(BuildError::artifact(format!(
                        "component metadata section exceeds the {}-byte policy limit",
                        MAX_EMBEDDED_METADATA_BYTES
                    )));
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
    let kind = match exports.as_slice() {
        [export] if export == "run" => ProgramKind::Module,
        [export] if export == WEBHOOK_INTERFACE => ProgramKind::Webhook,
        _ => {
            return Err(BuildError::artifact(
                "component exports do not match a policy 1 WIT world",
            ));
        }
    };
    if !sorted_unique(&imports) {
        return Err(BuildError::artifact(
            "component imports must be sorted and unique",
        ));
    }
    let mut effects = Vec::with_capacity(imports.len());
    for import in &imports {
        effects.push(
            match import.as_str() {
                STDOUT_INTERFACE => STDOUT_EFFECT,
                AI_INTERFACE => AI_EFFECT,
                CONFIG_INTERFACE => CONFIG_EFFECT,
                SECRETS_INTERFACE => SECRETS_EFFECT,
                HTTP_INTERFACE => HTTP_EFFECT,
                HTTP_ANONYMOUS_INTERFACE => HTTP_EFFECT,
                LOGGING_INTERFACE => LOGGING_EFFECT,
                STATE_INTERFACE => STATE_EFFECT,
                _ => {
                    return Err(BuildError::artifact(
                        "component imports do not match WebAssembly policy 1",
                    ));
                }
            }
            .to_owned(),
        );
    }
    effects.sort();
    let (_, _, contract) = load_contract(kind, &effects)?;
    if contract.component_imports != imports || contract.component_export != exports[0] {
        return Err(BuildError::artifact(
            "component surface does not match its deterministic WIT world",
        ));
    }
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
    if !(1..=8).contains(&core_module_count)
        || !(1..=8).contains(&table_count)
        || memory_count != u32::from(contract.requires_memory)
    {
        return Err(BuildError::artifact(format!(
            "component core shape violates policy 1 (modules={core_module_count}, tables={table_count}, memories={memory_count})"
        )));
    }

    let embedded =
        embedded.ok_or_else(|| BuildError::artifact("component metadata section is missing"))?;
    if embedded.schema != ARTIFACT_METADATA_SCHEMA
        || embedded.compiler_version != env!("CARGO_PKG_VERSION")
        || embedded.edition != "2026"
        || embedded.world != contract.world
        || embedded.effects != effects
        || !valid_requirements(&embedded.requirements, &effects)
        || !valid_approvals(&embedded.approvals, &embedded.requirements, &effects)
        || embedded.policy_version != artifact_policy_version(&effects)
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
        requirements: embedded.requirements,
        approvals: embedded.approvals,
        core_module_count,
        table_count,
        table_elements,
        memory_count,
        memory_minimum_bytes,
        memory_maximum_bytes,
    })
}

pub fn validate_artifact(
    bytes: &[u8],
    metadata: &ArtifactMetadata,
) -> Result<ComponentInspection, BuildError> {
    if metadata.schema != ARTIFACT_METADATA_SCHEMA
        || metadata.policy_version != artifact_policy_version(&metadata.effects)
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
        || !valid_requirements(&metadata.requirements, &metadata.effects)
        || !valid_approvals(
            &metadata.approvals,
            &metadata.requirements,
            &metadata.effects,
        )
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
        || metadata.requirements != inspection.requirements
        || metadata.approvals != inspection.approvals
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
        || decoded.component_export != expected.component_export
        || decoded.entry_export != expected.entry_export
        || decoded.post_entry_export != expected.post_entry_export
        || decoded.entry_signature != expected.entry_signature
        || decoded.post_entry_signature != expected.post_entry_signature
        || decoded.kind != expected.kind
    {
        return Err(BuildError::artifact(
            "decoded component does not implement the selected WIT world",
        ));
    }

    Ok(())
}

fn valid_requirements(requirements: &[ResourceRequirementMetadata], effects: &[String]) -> bool {
    requirements.windows(2).all(|pair| pair[0] < pair[1])
        && effects.iter().all(|effect| {
            effect == STDOUT_EFFECT
                || effect == LOGGING_EFFECT
                || requirements
                    .iter()
                    .any(|requirement| &requirement.capability == effect)
        })
        && requirements.iter().all(|requirement| {
            (effects.binary_search(&requirement.capability).is_ok()
                || (effects
                    .binary_search_by(|effect| effect.as_str().cmp(STATE_EFFECT))
                    .is_ok()
                    && matches!(requirement.capability.as_str(), AI_EFFECT | HTTP_EFFECT)))
                && match requirement.capability.as_str() {
                    AI_EFFECT | CONFIG_EFFECT | SECRETS_EFFECT | STATE_EFFECT => {
                        krit_capability::is_valid_resource_name(&requirement.resource)
                    }
                    HTTP_EFFECT => {
                        krit_capability::HttpOrigin::parse_exact(&requirement.resource).is_ok()
                    }
                    _ => false,
                }
        })
}

fn valid_approvals(
    approvals: &[ApprovalRequirementMetadata],
    requirements: &[ResourceRequirementMetadata],
    effects: &[String],
) -> bool {
    approvals.windows(2).all(|pair| pair[0] < pair[1])
        && requirements
            .iter()
            .filter(|requirement| requirement.capability == AI_EFFECT)
            .all(|requirement| {
                approvals.iter().any(|approval| {
                    approval.operation == "ai.invoke" && approval.resource == requirement.resource
                })
            })
        && approvals
            .iter()
            .all(|approval| match approval.operation.as_str() {
                "ai.invoke" => requirements.iter().any(|requirement| {
                    requirement.capability == AI_EFFECT && requirement.resource == approval.resource
                }),
                "http.bearer" => {
                    effects
                        .binary_search_by(|effect| effect.as_str().cmp(SECRETS_EFFECT))
                        .is_ok()
                        && requirements.iter().any(|requirement| {
                            requirement.capability == HTTP_EFFECT
                                && requirement.resource == approval.resource
                        })
                }
                _ => false,
            })
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
