use std::collections::BTreeMap;

use wasm_encoder::ValType;
use wit_parser::{
    ManglingAndAbi, Resolve, WasmExport, WasmExportKind, WasmImport, WorldId, WorldItem,
    abi::{WasmSignature, WasmType},
};

use crate::BuildError;

pub(crate) const WIT_SOURCE: &str = include_str!("../../../wit/runtime.wit");
const WIT_PACKAGE: &str = "krit:runtime@0.2.0";
pub(crate) const STDOUT_EFFECT: &str = "io.stdout";
pub const STDOUT_INTERFACE: &str = "krit:runtime/stdout@0.2.0";
pub const CONFIG_INTERFACE: &str = "krit:runtime/config@0.2.0";
pub const SECRETS_INTERFACE: &str = "krit:runtime/secrets@0.2.0";
pub const WEBHOOK_INTERFACE: &str = "krit:runtime/webhook@0.2.0";
pub const PROGRAM_WORLD: &str = "krit:runtime/program@0.2.0";
pub const PURE_PROGRAM_WORLD: &str = "krit:runtime/pure-program@0.2.0";
pub const WEBHOOK_PROGRAM_WORLD: &str = "krit:runtime/webhook-program@0.2.0";

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
    pub imports: Vec<WitFunction>,
    pub import_indices: BTreeMap<String, u32>,
    pub run_export: String,
    pub post_run_export: String,
    pub run_signature: Signature,
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

#[derive(Clone, Copy)]
struct WorldSelection {
    name: &'static str,
    id: &'static str,
    component_imports: &'static [&'static str],
}

pub(crate) fn load_contract(
    effects: &[String],
) -> Result<(Resolve, WorldId, WitContract), BuildError> {
    let selection = select_world(effects)?;
    let mut resolve = Resolve::default();
    let package = resolve
        .push_str("krit-runtime.wit", WIT_SOURCE)
        .map_err(|error| BuildError::artifact(format!("invalid built-in WIT package: {error}")))?;
    let world = resolve
        .select_world(&[package], Some(selection.name))
        .map_err(|error| BuildError::artifact(format!("invalid built-in WIT world: {error}")))?;

    let package_name = resolve.packages[package].name.to_string();
    let world_id = resolve.id_of_name(package, &resolve.worlds[world].name);
    if package_name != WIT_PACKAGE || world_id != selection.id {
        return Err(BuildError::artifact(
            "built-in WIT package or world identity does not match policy 1",
        ));
    }

    let contract = contract_from_world(&resolve, world)?;
    let expected_component_imports = selection
        .component_imports
        .iter()
        .map(|import| (*import).to_owned())
        .collect::<Vec<_>>();
    if contract.world != selection.id || contract.component_imports != expected_component_imports {
        return Err(BuildError::artifact(
            "built-in WIT world imports do not match policy 1",
        ));
    }
    if selection.id == PROGRAM_WORLD {
        verify_stdout_signatures(&contract.imports)?;
    } else if !contract.imports.is_empty() {
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

    let run = world_definition
        .exports
        .iter()
        .next()
        .and_then(|(key, item)| match item {
            WorldItem::Function(function) => Some((key, function)),
            _ => None,
        })
        .ok_or_else(|| BuildError::artifact("built-in WIT run export is not a function"))?;
    if run.1.name != "run" {
        return Err(BuildError::artifact("built-in WIT world must export `run`"));
    }
    let mangling = ManglingAndAbi::Standard32.for_func(run.1);
    let run_signature =
        convert_signature(resolve.wasm_signature(mangling.export_variant(), run.1))?;
    if !run_signature.params.is_empty() || !run_signature.results.is_empty() {
        return Err(BuildError::artifact(
            "built-in WIT `run` canonical ABI must be `() -> ()`",
        ));
    }
    let run_export = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface: None,
            func: run.1,
            kind: WasmExportKind::Normal,
        },
    );
    let post_run_export = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface: None,
            func: run.1,
            kind: WasmExportKind::PostReturn,
        },
    );

    Ok(WitContract {
        world: world_id,
        component_imports,
        imports,
        import_indices,
        run_export,
        post_run_export,
        run_signature,
    })
}

fn select_world(effects: &[String]) -> Result<WorldSelection, BuildError> {
    match effects {
        [] => Ok(WorldSelection {
            name: "pure-program",
            id: PURE_PROGRAM_WORLD,
            component_imports: &[],
        }),
        [effect] if effect == STDOUT_EFFECT => Ok(WorldSelection {
            name: "program",
            id: PROGRAM_WORLD,
            component_imports: &[STDOUT_INTERFACE],
        }),
        _ => Err(BuildError::artifact(
            "checked effects do not map to a WebAssembly policy 1 WIT world",
        )),
    }
}

fn convert_signature(signature: WasmSignature) -> Result<Signature, BuildError> {
    if signature.indirect_params || signature.retptr {
        return Err(BuildError::artifact(
            "built-in WIT unexpectedly requires guest memory",
        ));
    }
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
        WasmType::F32
        | WasmType::F64
        | WasmType::Pointer
        | WasmType::PointerOrI64
        | WasmType::Length => Err(BuildError::artifact(
            "built-in WIT uses a canonical ABI type outside policy 1",
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
    fn parses_the_versioned_webhook_config_and_secret_contracts() {
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
            "resource secret",
            "handle: func",
        ] {
            assert!(
                source.contains(contract),
                "missing WIT contract `{contract}`"
            );
        }
    }
}
