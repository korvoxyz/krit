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
pub const AI_INTERFACE: &str = "krit:runtime/ai@0.2.0";
pub const STDOUT_INTERFACE: &str = "krit:runtime/stdout@0.2.0";
pub const CONFIG_INTERFACE: &str = "krit:runtime/config@0.2.0";
pub const HTTP_INTERFACE: &str = "krit:runtime/http@0.2.0";
pub const HTTP_ANONYMOUS_INTERFACE: &str = "krit:runtime/http-anonymous@0.2.0";
pub const SECRETS_INTERFACE: &str = "krit:runtime/secrets@0.2.0";
pub const LOGGING_INTERFACE: &str = "krit:runtime/logging@0.2.0";
pub const STATE_INTERFACE: &str = "krit:runtime/state@0.2.0";
pub const WEBHOOK_INTERFACE: &str = "krit:runtime/webhook@0.2.0";
pub const PROGRAM_WORLD: &str = "krit:runtime/program@0.2.0";
pub const PURE_PROGRAM_WORLD: &str = "krit:runtime/pure-program@0.2.0";
pub const WEBHOOK_PROGRAM_WORLD: &str = "krit:runtime/webhook-program@0.2.0";
pub const WEBHOOK_ALL_PROGRAM_WORLD: &str =
    "krit:runtime/webhook-stdout-config-secrets-http-ai-logs-program@0.2.0";
pub const WEBHOOK_STATE_ALL_PROGRAM_WORLD: &str =
    "krit:runtime/webhook-stdout-config-secrets-http-ai-logs-state-program@0.2.0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProgramKind {
    Module,
    Webhook,
}

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
}

pub(crate) fn load_contract(
    kind: ProgramKind,
    effects: &[String],
) -> Result<(Resolve, WorldId, WitContract), BuildError> {
    let selection = select_world(kind, effects)?;
    let mut resolve = Resolve::default();
    let source = complete_wit_source();
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
    let mut expected_component_imports = selection.component_imports;
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
        requires_memory: kind == ProgramKind::Webhook,
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
            if identity != WEBHOOK_INTERFACE {
                return Err(BuildError::artifact(
                    "built-in WIT exports an unknown interface",
                ));
            }
            let interface = &resolve.interfaces[*id];
            if interface.functions.len() != 1 {
                return Err(BuildError::artifact(
                    "webhook WIT interface must export exactly one function",
                ));
            }
            let function = interface
                .functions
                .get("handle")
                .ok_or_else(|| BuildError::artifact("webhook WIT interface is missing `handle`"))?;
            Ok((
                key,
                Some(key),
                function,
                WEBHOOK_INTERFACE.to_owned(),
                ProgramKind::Webhook,
            ))
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
            }),
            [effect] if effect == STDOUT_EFFECT => Ok(WorldSelection {
                name: "program".to_owned(),
                id: PROGRAM_WORLD.to_owned(),
                component_imports: vec![STDOUT_INTERFACE.to_owned()],
            }),
            _ => Err(BuildError::artifact(
                "checked module effects do not map to a WebAssembly policy 1 WIT world",
            )),
        };
    }

    let mut mask = 0u16;
    for effect in effects {
        mask |= match effect.as_str() {
            STDOUT_EFFECT => 1 << 0,
            CONFIG_EFFECT => 1 << 1,
            SECRETS_EFFECT => 1 << 2,
            HTTP_EFFECT => 1 << 3,
            AI_EFFECT => 1 << 4,
            LOGGING_EFFECT => 1 << 5,
            STATE_EFFECT => 1 << 6,
            _ => {
                return Err(BuildError::artifact(
                    "checked webhook effects contain an unknown host surface",
                ));
            }
        };
    }
    if mask == 0 {
        return Ok(WorldSelection {
            name: "webhook-program".to_owned(),
            id: WEBHOOK_PROGRAM_WORLD.to_owned(),
            component_imports: Vec::new(),
        });
    }
    let mut tokens = Vec::new();
    let mut component_imports = Vec::new();
    for (bit, token, interface) in [
        (1 << 0, "stdout", STDOUT_INTERFACE),
        (1 << 1, "config", CONFIG_INTERFACE),
        (1 << 2, "secrets", SECRETS_INTERFACE),
        (
            1 << 3,
            "http",
            if mask & (1 << 2) == 0 {
                HTTP_ANONYMOUS_INTERFACE
            } else {
                HTTP_INTERFACE
            },
        ),
        (1 << 4, "ai", AI_INTERFACE),
        (1 << 5, "logs", LOGGING_INTERFACE),
        (1 << 6, "state", STATE_INTERFACE),
    ] {
        if mask & bit != 0 {
            tokens.push(token);
            component_imports.push(interface.to_owned());
        }
    }
    let name = format!("webhook-{}-program", tokens.join("-"));
    Ok(WorldSelection {
        id: format!("krit:runtime/{name}@0.2.0"),
        name,
        component_imports,
    })
}

fn complete_wit_source() -> String {
    let mut source = WIT_SOURCE.to_owned();
    for mask in 0u16..(1 << 6) {
        let mut tokens = Vec::new();
        let mut imports = Vec::new();
        for (bit, token, interface) in [
            (1 << 0, "stdout", "stdout"),
            (1 << 1, "config", "config"),
            (1 << 2, "secrets", "secrets"),
            (
                1 << 3,
                "http",
                if mask & (1 << 2) == 0 {
                    "http-anonymous"
                } else {
                    "http"
                },
            ),
            (1 << 4, "ai", "ai"),
            (1 << 5, "logs", "logging"),
        ] {
            if mask & bit != 0 {
                tokens.push(token);
                imports.push(interface);
            }
        }
        tokens.push("state");
        imports.push("state");
        let name = format!("webhook-{}-program", tokens.join("-"));
        source.push_str("\nworld ");
        source.push_str(&name);
        source.push_str(" {\n");
        for import in imports {
            source.push_str("    import ");
            source.push_str(import);
            source.push_str(";\n");
        }
        source.push_str("    export webhook;\n}\n");
    }
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
