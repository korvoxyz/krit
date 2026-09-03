use std::collections::BTreeMap;

use wasm_encoder::ValType;
use wit_parser::{
    ManglingAndAbi, Resolve, WasmExport, WasmExportKind, WasmImport, WorldId, WorldItem, WorldKey,
    abi::{WasmSignature, WasmType},
};

use crate::BuildError;

pub(crate) const WIT_SOURCE: &str = include_str!("../../../wit/runtime.wit");
const WIT_PACKAGE: &str = "krit:runtime@0.2.0";
pub(crate) const STDOUT_EFFECT: &str = "io.stdout";
pub(crate) const AI_EFFECT: &str = "ai.invoke";
pub(crate) const CONFIG_EFFECT: &str = "config.read";
pub(crate) const HTTP_EFFECT: &str = "http.request";
pub(crate) const LOGGING_EFFECT: &str = "observe.log";
pub(crate) const SECRETS_EFFECT: &str = "secret.read";
pub(crate) const STATE_EFFECT: &str = "state.transaction";
pub(crate) const QUEUE_PUBLISH_EFFECT: &str = "queue.publish";
pub(crate) const QUEUE_CONSUME_EFFECT: &str = "queue.consume";
pub(crate) const SCHEDULE_TRIGGER_EFFECT: &str = "schedule.trigger";
pub(crate) const OBJECT_READ_EFFECT: &str = "object.read";
pub(crate) const OBJECT_WRITE_EFFECT: &str = "object.write";
pub const AI_INTERFACE: &str = "krit:runtime/ai@0.2.0";
pub const STDOUT_INTERFACE: &str = "krit:runtime/stdout@0.2.0";
pub const CONFIG_INTERFACE: &str = "krit:runtime/config@0.2.0";
pub const HTTP_INTERFACE: &str = "krit:runtime/http@0.2.0";
pub const HTTP_ANONYMOUS_INTERFACE: &str = "krit:runtime/http-anonymous@0.2.0";
pub const SECRETS_INTERFACE: &str = "krit:runtime/secrets@0.2.0";
pub const LOGGING_INTERFACE: &str = "krit:runtime/logging@0.2.0";
pub const STATE_INTERFACE: &str = "krit:runtime/state@0.2.0";
pub const QUEUE_INTERFACE: &str = "krit:runtime/queue@0.2.0";
pub const OBJECTS_READ_INTERFACE: &str = "krit:runtime/objects-read@0.2.0";
pub const OBJECTS_WRITE_INTERFACE: &str = "krit:runtime/objects-write@0.2.0";
pub const WEBHOOK_INTERFACE: &str = "krit:runtime/webhook@0.2.0";
pub const JOB_INTERFACE: &str = "krit:runtime/job@0.2.0";
pub const SCHEDULE_INTERFACE: &str = "krit:runtime/schedule@0.2.0";
pub const PROGRAM_WORLD: &str = "krit:runtime/program@0.2.0";
pub const PURE_PROGRAM_WORLD: &str = "krit:runtime/pure-program@0.2.0";
pub const WEBHOOK_PROGRAM_WORLD: &str = "krit:runtime/webhook-program@0.2.0";
pub const WEBHOOK_ALL_PROGRAM_WORLD: &str =
    "krit:runtime/webhook-stdout-config-secrets-http-ai-logs-program@0.2.0";
pub const WEBHOOK_STATE_ALL_PROGRAM_WORLD: &str =
    "krit:runtime/webhook-stdout-config-secrets-http-ai-logs-state-program@0.2.0";
pub const JOB_PROGRAM_WORLD: &str = "krit:runtime/job-program@0.2.0";
pub const SCHEDULE_PROGRAM_WORLD: &str = "krit:runtime/schedule-program@0.2.0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProgramKind {
    Module,
    Webhook,
    Job,
    Schedule,
}

impl ProgramKind {
    /// Effect implied by exporting this entrypoint contract.
    pub(crate) const fn export_effect(self) -> Option<&'static str> {
        match self {
            Self::Module | Self::Webhook => None,
            Self::Job => Some(QUEUE_CONSUME_EFFECT),
            Self::Schedule => Some(SCHEDULE_TRIGGER_EFFECT),
        }
    }

    const fn world_prefix(self) -> &'static str {
        match self {
            Self::Module => "program",
            Self::Webhook => "webhook",
            Self::Job => "job",
            Self::Schedule => "schedule",
        }
    }
}

/// Ordered import surfaces. The bit order fixes every generated world name.
const IMPORT_SURFACES: [(u16, &str, &str, &str); 10] = [
    (1 << 0, "stdout", "stdout", STDOUT_INTERFACE),
    (1 << 1, "config", "config", CONFIG_INTERFACE),
    (1 << 2, "secrets", "secrets", SECRETS_INTERFACE),
    (1 << 3, "http", "http-anonymous", HTTP_ANONYMOUS_INTERFACE),
    (1 << 4, "ai", "ai", AI_INTERFACE),
    (1 << 5, "logs", "logging", LOGGING_INTERFACE),
    (1 << 6, "state", "state", STATE_INTERFACE),
    (1 << 7, "queue", "queue", QUEUE_INTERFACE),
    (1 << 8, "objread", "objects-read", OBJECTS_READ_INTERFACE),
    (1 << 9, "objwrite", "objects-write", OBJECTS_WRITE_INTERFACE),
];

const SECRETS_BIT: u16 = 1 << 2;
const HTTP_BIT: u16 = 1 << 3;
/// Import surfaces whose webhook worlds are checked in to `wit/runtime.wit`.
const CHECKED_IN_MASK: u16 = 0b11_1111;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct WitFunction {
    pub module: String,
    pub name: String,
    pub signature: Signature,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WitContract {
    pub world: String,
    pub component_imports: Vec<String>,
    pub component_export: String,
    pub imports: Vec<WitFunction>,
    pub import_indices: BTreeMap<String, u32>,
    pub entry_export: String,
    pub post_entry_export: String,
    pub entry_signature: Signature,
    pub post_entry_signature: Signature,
    pub kind: ProgramKind,
    pub requires_memory: bool,
    pub memory_export: String,
    pub realloc_export: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Signature {
    pub params: Vec<Scalar>,
    pub results: Vec<Scalar>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Scalar {
    I32,
    I64,
}

impl Scalar {
    pub const fn wasm(self) -> ValType {
        match self {
            Self::I32 => ValType::I32,
            Self::I64 => ValType::I64,
        }
    }
}

#[derive(Clone)]
struct WorldSelection {
    name: String,
    id: String,
    component_imports: Vec<String>,
    generated: bool,
    export: &'static str,
}

pub(crate) fn load_contract(
    kind: ProgramKind,
    effects: &[String],
) -> Result<(Resolve, WorldId, WitContract), BuildError> {
    let selection = select_world(kind, effects)?;
    let mut resolve = Resolve::default();
    let source = complete_wit_source(&selection);
    let package = resolve
        .push_str("krit-runtime.wit", &source)
        .map_err(|error| BuildError::artifact(format!("invalid built-in WIT package: {error}")))?;
    let world = resolve
        .select_world(&[package], Some(&selection.name))
        .map_err(|error| BuildError::artifact(format!("invalid built-in WIT world: {error}")))?;

    let package_name = resolve.packages[package].name.to_string();
    let world_id = resolve.id_of_name(package, &resolve.worlds[world].name);
    if package_name != WIT_PACKAGE || world_id != selection.id {
        return Err(BuildError::artifact(
            "built-in WIT package or world identity does not match policy 1",
        ));
    }

    let contract = contract_from_world(&resolve, world)?;
    let mut expected_component_imports = selection.component_imports.clone();
    expected_component_imports.sort();
    if contract.world != selection.id || contract.component_imports != expected_component_imports {
        return Err(BuildError::artifact(
            "built-in WIT world imports do not match policy 1",
        ));
    }
    if selection.id == PROGRAM_WORLD {
        verify_stdout_signatures(&contract.imports)?;
    } else if kind == ProgramKind::Module && !contract.imports.is_empty() {
        return Err(BuildError::artifact(
            "built-in pure WIT world unexpectedly has core imports",
        ));
    }

    Ok((resolve, world, contract))
}

pub(crate) fn contract_from_world(
    resolve: &Resolve,
    world: WorldId,
) -> Result<WitContract, BuildError> {
    let world_definition = &resolve.worlds[world];
    let package = world_definition
        .package
        .ok_or_else(|| BuildError::artifact("WIT world has no owning package"))?;
    let world_id = resolve.id_of_name(package, &world_definition.name);
    if world_definition.exports.len() != 1 {
        return Err(BuildError::artifact(
            "WIT world must contain exactly one export",
        ));
    }

    let mut component_imports = Vec::new();
    let mut imports = Vec::new();
    let mut import_indices = BTreeMap::new();
    for (interface_key, item) in &world_definition.imports {
        let WorldItem::Interface {
            id: interface_id, ..
        } = item
        else {
            return Err(BuildError::artifact(
                "WIT world contains a non-interface import",
            ));
        };
        let interface_id_string = resolve
            .id_of(*interface_id)
            .ok_or_else(|| BuildError::artifact("WIT import interface has no stable identity"))?;
        component_imports.push(interface_id_string);
        for function in resolve.interfaces[*interface_id].functions.values() {
            let mangling = ManglingAndAbi::Standard32.for_func(function);
            let (module, name) = resolve.wasm_import_name(
                mangling,
                WasmImport::Func {
                    interface: Some(interface_key),
                    func: function,
                },
            );
            let signature =
                convert_signature(resolve.wasm_signature(mangling.import_variant(), function))?;
            let index = u32::try_from(imports.len())
                .map_err(|_| BuildError::artifact("too many WIT imports"))?;
            if import_indices
                .insert(function.name.clone(), index)
                .is_some()
            {
                return Err(BuildError::artifact(
                    "WIT world contains duplicate imported function names",
                ));
            }
            imports.push(WitFunction {
                module,
                name,
                signature,
            });
        }
    }
    component_imports.sort();

    let (_export_key, export_interface, export_function, component_export, kind) =
        export_function(resolve, world)?;
    let mangling = ManglingAndAbi::Standard32.for_func(export_function);
    let entry_signature =
        convert_signature(resolve.wasm_signature(mangling.export_variant(), export_function))?;
    if kind == ProgramKind::Module
        && (!entry_signature.params.is_empty() || !entry_signature.results.is_empty())
    {
        return Err(BuildError::artifact(
            "built-in WIT `run` canonical ABI must be `() -> ()`",
        ));
    }
    let entry_export = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface: export_interface,
            func: export_function,
            kind: WasmExportKind::Normal,
        },
    );
    let post_entry_export = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface: export_interface,
            func: export_function,
            kind: WasmExportKind::PostReturn,
        },
    );
    let post_entry_signature = Signature {
        params: entry_signature.results.clone(),
        results: Vec::new(),
    };

    Ok(WitContract {
        world: world_id,
        component_imports,
        component_export,
        imports,
        import_indices,
        entry_export,
        post_entry_export,
        entry_signature,
        post_entry_signature,
        kind,
        requires_memory: kind != ProgramKind::Module,
        memory_export: resolve.wasm_export_name(ManglingAndAbi::Standard32, WasmExport::Memory),
        realloc_export: resolve.wasm_export_name(ManglingAndAbi::Standard32, WasmExport::Realloc),
    })
}

fn export_function(
    resolve: &Resolve,
    world: WorldId,
) -> Result<
    (
        &WorldKey,
        Option<&WorldKey>,
        &wit_parser::Function,
        String,
        ProgramKind,
    ),
    BuildError,
> {
    let (key, item) = resolve.worlds[world]
        .exports
        .iter()
        .next()
        .ok_or_else(|| BuildError::artifact("built-in WIT world has no export"))?;
    match item {
        WorldItem::Function(function) if function.name == "run" => {
            Ok((key, None, function, "run".to_owned(), ProgramKind::Module))
        }
        WorldItem::Interface { id, .. } => {
            let identity = resolve
                .id_of(*id)
                .ok_or_else(|| BuildError::artifact("WIT export interface has no identity"))?;
            let kind = match identity.as_str() {
                WEBHOOK_INTERFACE => ProgramKind::Webhook,
                JOB_INTERFACE => ProgramKind::Job,
                SCHEDULE_INTERFACE => ProgramKind::Schedule,
                _ => {
                    return Err(BuildError::artifact(
                        "built-in WIT exports an unknown interface",
                    ));
                }
            };
            let interface = &resolve.interfaces[*id];
            if interface.functions.len() != 1 {
                return Err(BuildError::artifact(
                    "entrypoint WIT interface must export exactly one function",
                ));
            }
            let function = interface.functions.get("handle").ok_or_else(|| {
                BuildError::artifact("entrypoint WIT interface is missing `handle`")
            })?;
            Ok((key, Some(key), function, identity, kind))
        }
        _ => Err(BuildError::artifact(
            "built-in WIT export is not an approved function or interface",
        )),
    }
}

fn select_world(kind: ProgramKind, effects: &[String]) -> Result<WorldSelection, BuildError> {
    if effects.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(BuildError::artifact(
            "checked effects must be sorted and unique",
        ));
    }
    if kind == ProgramKind::Module {
        return match effects {
            [] => Ok(WorldSelection {
                name: "pure-program".to_owned(),
                id: PURE_PROGRAM_WORLD.to_owned(),
                component_imports: Vec::new(),
                generated: false,
                export: "run",
            }),
            [effect] if effect == STDOUT_EFFECT => Ok(WorldSelection {
                name: "program".to_owned(),
                id: PROGRAM_WORLD.to_owned(),
                component_imports: vec![STDOUT_INTERFACE.to_owned()],
                generated: false,
                export: "run",
            }),
            _ => Err(BuildError::artifact(
                "checked module effects do not map to a WebAssembly policy 1 WIT world",
            )),
        };
    }

    let export_effect = kind.export_effect();
    let mut saw_export_effect = false;
    let mut mask = 0u16;
    for effect in effects {
        if Some(effect.as_str()) == export_effect {
            saw_export_effect = true;
            continue;
        }
        mask |= match effect.as_str() {
            STDOUT_EFFECT => 1 << 0,
            CONFIG_EFFECT => 1 << 1,
            SECRETS_EFFECT => SECRETS_BIT,
            HTTP_EFFECT => HTTP_BIT,
            AI_EFFECT => 1 << 4,
            LOGGING_EFFECT => 1 << 5,
            STATE_EFFECT => 1 << 6,
            QUEUE_PUBLISH_EFFECT => 1 << 7,
            OBJECT_READ_EFFECT => 1 << 8,
            OBJECT_WRITE_EFFECT => 1 << 9,
            _ => {
                return Err(BuildError::artifact(
                    "checked entrypoint effects contain an unknown host surface",
                ));
            }
        };
    }
    if export_effect.is_some() != saw_export_effect {
        return Err(BuildError::artifact(
            "checked entrypoint effects do not match the exported host contract",
        ));
    }
    let (tokens, component_imports) = surfaces(mask);
    let name = if tokens.is_empty() {
        format!("{}-program", kind.world_prefix())
    } else {
        format!("{}-{}-program", kind.world_prefix(), tokens.join("-"))
    };
    let generated = kind != ProgramKind::Webhook || mask & !CHECKED_IN_MASK != 0;
    Ok(WorldSelection {
        id: format!("krit:runtime/{name}@0.2.0"),
        name,
        component_imports,
        generated,
        export: kind.world_prefix(),
    })
}

/// Returns the ordered world-name tokens and component import identities for
/// one import mask. Bearer HTTP is selected only alongside secret authority.
fn surfaces(mask: u16) -> (Vec<&'static str>, Vec<String>) {
    let mut tokens = Vec::new();
    let mut imports = Vec::new();
    for (bit, token, _, interface) in IMPORT_SURFACES {
        if mask & bit == 0 {
            continue;
        }
        tokens.push(token);
        if bit == HTTP_BIT && mask & SECRETS_BIT != 0 {
            imports.push(HTTP_INTERFACE.to_owned());
        } else {
            imports.push(interface.to_owned());
        }
    }
    imports.sort();
    (tokens, imports)
}

/// Returns the checked-in package plus, when required, the one deterministic
/// least-authority world selected for this build.
fn complete_wit_source(selection: &WorldSelection) -> String {
    let mut source = WIT_SOURCE.to_owned();
    if !selection.generated {
        return source;
    }
    let mut mask = 0u16;
    for (bit, _, _, interface) in IMPORT_SURFACES {
        let selected = selection
            .component_imports
            .iter()
            .any(|import| import == interface || (bit == HTTP_BIT && import == HTTP_INTERFACE));
        if selected {
            mask |= bit;
        }
    }
    source.push_str("\nworld ");
    source.push_str(&selection.name);
    source.push_str(" {\n");
    for (bit, _, wit_name, _) in IMPORT_SURFACES {
        if mask & bit == 0 {
            continue;
        }
        source.push_str("    import ");
        if bit == HTTP_BIT && mask & SECRETS_BIT != 0 {
            source.push_str("http");
        } else {
            source.push_str(wit_name);
        }
        source.push_str(";\n");
    }
    source.push_str("    export ");
    source.push_str(selection.export);
    source.push_str(";\n}\n");
    source
}

fn convert_signature(signature: WasmSignature) -> Result<Signature, BuildError> {
    Ok(Signature {
        params: signature
            .params
            .into_iter()
            .map(convert_wasm_type)
            .collect::<Result<_, _>>()?,
        results: signature
            .results
            .into_iter()
            .map(convert_wasm_type)
            .collect::<Result<_, _>>()?,
    })
}

fn convert_wasm_type(ty: WasmType) -> Result<Scalar, BuildError> {
    match ty {
        WasmType::I32 => Ok(Scalar::I32),
        WasmType::I64 => Ok(Scalar::I64),
        WasmType::Pointer | WasmType::Length => Ok(Scalar::I32),
        WasmType::PointerOrI64 => Ok(Scalar::I64),
        WasmType::F32 | WasmType::F64 => Err(BuildError::artifact(
            "built-in WIT uses floating-point canonical ABI values",
        )),
    }
}

fn verify_stdout_signatures(imports: &[WitFunction]) -> Result<(), BuildError> {
    let expected = [
        ("write-int", vec![Scalar::I64, Scalar::I32]),
        ("write-bool", vec![Scalar::I32, Scalar::I32]),
        ("write-unit", vec![Scalar::I32]),
    ];
    if imports.len() != expected.len() {
        return Err(BuildError::artifact(
            "built-in WIT stdout interface has an unexpected function count",
        ));
    }

    for (name, parameters) in expected {
        let Some(function) = imports.iter().find(|function| function.name == name) else {
            return Err(BuildError::artifact(format!(
                "built-in WIT stdout interface is missing `{name}`"
            )));
        };
        if function.signature.params != parameters || !function.signature.results.is_empty() {
            return Err(BuildError::artifact(format!(
                "built-in WIT stdout function `{name}` has an unexpected canonical ABI"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_versioned_phase_four_host_contracts() {
        let mut resolve = Resolve::default();
        let package = resolve
            .push_str("krit-runtime.wit", WIT_SOURCE)
            .expect("checked-in WIT should parse");
        let world = resolve
            .select_world(&[package], Some("webhook-program"))
            .expect("webhook world should exist");

        assert_eq!(
            resolve.id_of_name(package, &resolve.worlds[world].name),
            WEBHOOK_PROGRAM_WORLD
        );
        assert!(resolve.worlds[world].imports.is_empty());
        assert_eq!(resolve.worlds[world].exports.len(), 1);
        let Some(WorldItem::Interface { id, .. }) = resolve.worlds[world].exports.values().next()
        else {
            panic!("webhook world should export an interface");
        };
        assert_eq!(resolve.id_of(*id).as_deref(), Some(WEBHOOK_INTERFACE));
        assert!(resolve.interfaces[*id].functions.contains_key("handle"));
        let source = WIT_SOURCE;
        for contract in [
            "interface webhook",
            "interface config",
            "interface secrets",
            "interface http",
            "interface http-anonymous",
            "interface ai",
            "interface logging",
            "interface state",
            "resource secret",
            "option<borrow<secret>>",
            "handle: func",
        ] {
            assert!(
                source.contains(contract),
                "missing WIT contract `{contract}`"
            );
        }
    }

    #[test]
    fn selects_anonymous_and_bearer_http_surfaces_without_implicit_secret_authority() {
        let (_, _, anonymous) = load_contract(ProgramKind::Webhook, &[HTTP_EFFECT.to_owned()])
            .expect("anonymous HTTP world should load");
        assert_eq!(anonymous.component_imports, [HTTP_ANONYMOUS_INTERFACE]);

        let (_, _, bearer) = load_contract(
            ProgramKind::Webhook,
            &[HTTP_EFFECT.to_owned(), SECRETS_EFFECT.to_owned()],
        )
        .expect("bearer HTTP world should load");
        assert_eq!(
            bearer.component_imports,
            [HTTP_INTERFACE, SECRETS_INTERFACE]
        );

        let (_, _, ai_only) = load_contract(ProgramKind::Webhook, &[AI_EFFECT.to_owned()])
            .expect("AI-only world should load");
        assert_eq!(ai_only.component_imports, [AI_INTERFACE]);

        let (_, _, logs_only) = load_contract(ProgramKind::Webhook, &[LOGGING_EFFECT.to_owned()])
            .expect("log-only world should load");
        assert_eq!(logs_only.component_imports, [LOGGING_INTERFACE]);

        let (_, _, state_only) = load_contract(ProgramKind::Webhook, &[STATE_EFFECT.to_owned()])
            .expect("state-only world should load");
        assert_eq!(state_only.component_imports, [STATE_INTERFACE]);
        assert_eq!(state_only.world, "krit:runtime/webhook-state-program@0.2.0");

        let (_, _, state_ai) = load_contract(
            ProgramKind::Webhook,
            &[AI_EFFECT.to_owned(), STATE_EFFECT.to_owned()],
        )
        .expect("AI plus state world should load");
        assert_eq!(state_ai.component_imports, [AI_INTERFACE, STATE_INTERFACE]);
    }
}
