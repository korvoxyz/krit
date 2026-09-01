use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use krit::{
    BinaryOperator, Builtin, CoreBlock, CoreFunction, CoreModule, CoreOperation, OperationKind,
    Type, UnaryOperator, ValueId, ValueLiteral,
};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ElementSection, Elements, EntityType, ExportKind,
    ExportSection, Function, FunctionSection, ImportSection, Instruction, Module, RefType,
    TableSection, TableType, TypeSection,
};

use crate::{
    BuildError,
    wit::{Scalar, Signature, WitContract},
};

pub(crate) struct EncodedCore {
    pub bytes: Vec<u8>,
    pub table_size: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum BuiltinFlavor {
    PrintInt,
    PrintBool,
    PrintUnit,
    PrintlnInt,
    PrintlnBool,
    PrintlnUnit,
}

impl BuiltinFlavor {
    fn from_operation(builtin: Builtin, ty: &Type) -> Result<Self, BuildError> {
        let Type::Function(function) = ty else {
            return Err(BuildError::unsupported(
                "stdout built-in has a non-function layout",
                None,
            ));
        };
        let argument = function
            .parameters()
            .first()
            .map(Arc::as_ref)
            .ok_or_else(|| {
                BuildError::unsupported("stdout built-in has no argument layout", None)
            })?;
        match (builtin, argument) {
            (Builtin::Print, Type::Int) => Ok(Self::PrintInt),
            (Builtin::Print, Type::Bool) => Ok(Self::PrintBool),
            (Builtin::Print, Type::Unit) => Ok(Self::PrintUnit),
            (Builtin::Println, Type::Int) => Ok(Self::PrintlnInt),
            (Builtin::Println, Type::Bool) => Ok(Self::PrintlnBool),
            (Builtin::Println, Type::Unit) => Ok(Self::PrintlnUnit),
            _ => Err(BuildError::unsupported(
                "stdout built-in argument has no WebAssembly policy 1 lowering",
                None,
            )),
        }
    }

    fn signature(self) -> Signature {
        let params = match self {
            Self::PrintInt | Self::PrintlnInt => vec![Scalar::I64],
            Self::PrintBool | Self::PrintlnBool => vec![Scalar::I32],
            Self::PrintUnit | Self::PrintlnUnit => Vec::new(),
        };
        Signature {
            params,
            results: Vec::new(),
        }
    }

    const fn import_name(self) -> &'static str {
        match self {
            Self::PrintInt | Self::PrintlnInt => "write-int",
            Self::PrintBool | Self::PrintlnBool => "write-bool",
            Self::PrintUnit | Self::PrintlnUnit => "write-unit",
        }
    }

    const fn newline(self) -> bool {
        matches!(
            self,
            Self::PrintlnInt | Self::PrintlnBool | Self::PrintlnUnit
        )
    }

    const fn has_value(self) -> bool {
        !matches!(self, Self::PrintUnit | Self::PrintlnUnit)
    }
}

pub(crate) fn encode_core(
    module: &CoreModule,
    contract: &WitContract,
    minimum_literal_operands: &BTreeSet<ValueId>,
) -> Result<EncodedCore, BuildError> {
    let builtin_flavors = collect_builtin_flavors(module)?;
    let builtin_slots = builtin_flavors
        .iter()
        .copied()
        .enumerate()
        .map(|(index, flavor)| {
            let index = u32::try_from(index)
                .map_err(|_| BuildError::artifact("too many stdout wrapper functions"))?;
            let base = u32::try_from(module.functions().len())
                .map_err(|_| BuildError::artifact("too many Core functions"))?;
            Ok((flavor, base + index))
        })
        .collect::<Result<BTreeMap<_, _>, BuildError>>()?;

    let mut signatures = BTreeSet::new();
    signatures.extend(
        contract
            .imports
            .iter()
            .map(|function| function.signature.clone()),
    );
    for function in module.functions() {
        signatures.insert(function_signature(function)?);
    }
    signatures.extend(
        builtin_flavors
            .iter()
            .copied()
            .map(BuiltinFlavor::signature),
    );
    signatures.insert(contract.run_signature.clone());

    let signature_indices = signatures
        .into_iter()
        .enumerate()
        .map(|(index, signature)| {
            u32::try_from(index)
                .map(|index| (signature, index))
                .map_err(|_| BuildError::artifact("too many WebAssembly function types"))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    let mut types = TypeSection::new();
    for signature in signature_indices.keys() {
        types.ty().function(
            signature.params.iter().copied().map(Scalar::wasm),
            signature.results.iter().copied().map(Scalar::wasm),
        );
    }

    let mut imports = ImportSection::new();
    for import in &contract.imports {
        let type_index = signature_indices[&import.signature];
        imports.import(
            &import.module,
            &import.name,
            EntityType::Function(type_index),
        );
    }

    let mut functions = FunctionSection::new();
    for function in module.functions() {
        functions.function(signature_indices[&function_signature(function)?]);
    }
    for flavor in &builtin_flavors {
        functions.function(signature_indices[&flavor.signature()]);
    }
    functions.function(signature_indices[&contract.run_signature]);

    let table_size = u32::try_from(module.functions().len() + builtin_flavors.len())
        .map_err(|_| BuildError::artifact("WebAssembly function table is too large"))?;
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: u64::from(table_size),
        maximum: Some(u64::from(table_size)),
        shared: false,
    });

    let import_count = u32::try_from(contract.imports.len())
        .map_err(|_| BuildError::artifact("too many WebAssembly imports"))?;
    let mut exports = ExportSection::new();
    let entrypoint = module.entrypoint().as_u32();
    exports.export(
        &contract.run_export,
        ExportKind::Func,
        import_count + entrypoint,
    );
    let post_run_index = import_count + table_size;
    exports.export(&contract.post_run_export, ExportKind::Func, post_run_index);

    let table_functions = (0..table_size)
        .map(|definition| import_count + definition)
        .collect::<Vec<_>>();
    let mut elements = ElementSection::new();
    elements.active(
        None,
        &ConstExpr::i32_const(0),
        Elements::Functions(Cow::Owned(table_functions)),
    );

    let context = EncodeContext {
        signature_indices: &signature_indices,
        builtin_slots: &builtin_slots,
        minimum_literal_operands,
    };
    let mut code = CodeSection::new();
    for function in module.functions() {
        code.function(&encode_function(function, &context)?);
    }
    for flavor in &builtin_flavors {
        code.function(&encode_builtin_wrapper(*flavor, contract)?);
    }
    let mut post_run = Function::new(Vec::new());
    post_run.instruction(&Instruction::End);
    code.function(&post_run);

    let mut wasm = Module::new();
    wasm.section(&types)
        .section(&imports)
        .section(&functions)
        .section(&tables)
        .section(&exports)
        .section(&elements)
        .section(&code);

    Ok(EncodedCore {
        bytes: wasm.finish(),
        table_size,
    })
}

struct EncodeContext<'a> {
    signature_indices: &'a BTreeMap<Signature, u32>,
    builtin_slots: &'a BTreeMap<BuiltinFlavor, u32>,
    minimum_literal_operands: &'a BTreeSet<ValueId>,
}

struct FunctionContext<'a> {
    shared: &'a EncodeContext<'a>,
    locals: BTreeMap<ValueId, u32>,
    value_types: BTreeMap<ValueId, Arc<Type>>,
}

fn encode_function(
    core: &CoreFunction,
    shared: &EncodeContext<'_>,
) -> Result<Function, BuildError> {
    let mut locals = BTreeMap::new();
    let mut value_types = BTreeMap::new();
    let mut next_parameter = 0u32;
    for parameter in &core.parameters {
        value_types.insert(parameter.value, Arc::clone(&parameter.ty));
        if scalar_layout(&parameter.ty)?.is_some() {
            locals.insert(parameter.value, next_parameter);
            next_parameter += 1;
        }
    }

    let mut local_declarations = Vec::new();
    if let Some(recursive) = &core.recursive {
        value_types.insert(recursive.value, Arc::clone(&recursive.ty));
        push_local(
            recursive.value,
            &recursive.ty,
            next_parameter,
            &mut locals,
            &mut local_declarations,
        )?;
    }
    collect_block_locals(
        &core.body,
        next_parameter,
        &mut locals,
        &mut value_types,
        &mut local_declarations,
    )?;

    let mut function = Function::new(
        local_declarations
            .iter()
            .copied()
            .map(|scalar| (1, scalar.wasm())),
    );
    let context = FunctionContext {
        shared,
        locals,
        value_types,
    };

    if let Some(recursive) = &core.recursive {
        function.instruction(&Instruction::I32Const(
            i32::try_from(core.id.as_u32())
                .map_err(|_| BuildError::artifact("function table slot exceeds i32"))?,
        ));
        store_value(&mut function, &context, recursive.value)?;
    }

    encode_block(&mut function, &context, &core.body)?;
    load_value(&mut function, &context, core.body.result)?;
    function.instruction(&Instruction::End);
    Ok(function)
}

fn collect_block_locals(
    block: &CoreBlock,
    parameter_count: u32,
    locals: &mut BTreeMap<ValueId, u32>,
    value_types: &mut BTreeMap<ValueId, Arc<Type>>,
    declarations: &mut Vec<Scalar>,
) -> Result<(), BuildError> {
    for parameter in &block.parameters {
        value_types.insert(parameter.value, Arc::clone(&parameter.ty));
        push_local(
            parameter.value,
            &parameter.ty,
            parameter_count,
            locals,
            declarations,
        )?;
    }
    for operation in &block.operations {
        value_types.insert(operation.result, Arc::clone(&operation.ty));
        push_local(
            operation.result,
            &operation.ty,
            parameter_count,
            locals,
            declarations,
        )?;
        match &operation.kind {
            OperationKind::Block { block } => {
                collect_block_locals(block, parameter_count, locals, value_types, declarations)?
            }
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_block_locals(
                    consequent,
                    parameter_count,
                    locals,
                    value_types,
                    declarations,
                )?;
                collect_block_locals(
                    alternative,
                    parameter_count,
                    locals,
                    value_types,
                    declarations,
                )?;
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_block_locals(empty, parameter_count, locals, value_types, declarations)?;
                collect_block_locals(cons, parameter_count, locals, value_types, declarations)?;
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_block_locals(
                        &arm.block,
                        parameter_count,
                        locals,
                        value_types,
                        declarations,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn push_local(
    value: ValueId,
    ty: &Type,
    parameter_count: u32,
    locals: &mut BTreeMap<ValueId, u32>,
    declarations: &mut Vec<Scalar>,
) -> Result<(), BuildError> {
    if let Some(scalar) = scalar_layout(ty)? {
        let declaration_count = u32::try_from(declarations.len())
            .map_err(|_| BuildError::artifact("too many WebAssembly locals"))?;
        if locals
            .insert(value, parameter_count + declaration_count)
            .is_some()
        {
            return Err(BuildError::invalid_core(format!(
                "duplicate Core value {value}"
            )));
        }
        declarations.push(scalar);
    }
    Ok(())
}

fn encode_block(
    function: &mut Function,
    context: &FunctionContext<'_>,
    block: &CoreBlock,
) -> Result<(), BuildError> {
    for operation in &block.operations {
        encode_operation(function, context, operation)?;
    }
    Ok(())
}

fn encode_operation(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
) -> Result<(), BuildError> {
    match &operation.kind {
        OperationKind::Literal(ValueLiteral::Integer(value)) => {
            let value = if context
                .shared
                .minimum_literal_operands
                .contains(&operation.result)
            {
                0
            } else {
                i64::try_from(*value).map_err(|_| {
                    BuildError::unsupported(
                        "integer literal is outside the concrete i64 WebAssembly layout",
                        operation.source,
                    )
                })?
            };
            function.instruction(&Instruction::I64Const(value));
            store_value(function, context, operation.result)?;
        }
        OperationKind::Literal(ValueLiteral::Boolean(value)) => {
            function.instruction(&Instruction::I32Const(i32::from(*value)));
            store_value(function, context, operation.result)?;
        }
        OperationKind::Unit | OperationKind::Bind { .. } | OperationKind::Discard { .. } => {}
        OperationKind::Builtin(builtin) => {
            let flavor = BuiltinFlavor::from_operation(*builtin, &operation.ty)?;
            let slot = context.shared.builtin_slots[&flavor];
            function
                .instruction(&Instruction::I32Const(i32::try_from(slot).map_err(
                    |_| BuildError::artifact("function table slot exceeds i32"),
                )?));
            store_value(function, context, operation.result)?;
        }
        OperationKind::Closure {
            function: target, ..
        } => {
            function.instruction(&Instruction::I32Const(
                i32::try_from(target.as_u32())
                    .map_err(|_| BuildError::artifact("function table slot exceeds i32"))?,
            ));
            store_value(function, context, operation.result)?;
        }
        OperationKind::Call { callee, arguments } => {
            let callee_type = context
                .value_types
                .get(callee)
                .ok_or_else(|| BuildError::invalid_core(format!("missing type for {callee}")))?;
            let Type::Function(function_type) = callee_type.as_ref() else {
                return Err(BuildError::invalid_core(format!(
                    "call target {callee} is not a function"
                )));
            };
            for argument in arguments {
                load_value(function, context, *argument)?;
            }
            load_value(function, context, *callee)?;
            let signature =
                type_signature(function_type.parameters(), function_type.return_type())?;
            let type_index = context.shared.signature_indices[&signature];
            function.instruction(&Instruction::CallIndirect {
                type_index,
                table_index: 0,
            });
            store_value(function, context, operation.result)?;
        }
        OperationKind::Block { block } => {
            encode_block(function, context, block)?;
            load_value(function, context, block.result)?;
            store_value(function, context, operation.result)?;
        }
        OperationKind::If {
            condition,
            consequent,
            alternative,
        } => {
            load_value(function, context, *condition)?;
            function.instruction(&Instruction::If(BlockType::Empty));
            encode_block(function, context, consequent)?;
            load_value(function, context, consequent.result)?;
            store_value(function, context, operation.result)?;
            function.instruction(&Instruction::Else);
            encode_block(function, context, alternative)?;
            load_value(function, context, alternative.result)?;
            store_value(function, context, operation.result)?;
            function.instruction(&Instruction::End);
        }
        OperationKind::Unary { operator, operand } => {
            encode_unary(function, context, operation.result, *operator, *operand)?;
        }
        OperationKind::Binary {
            left,
            operator,
            right,
        } => {
            encode_binary(
                function,
                context,
                operation.result,
                *left,
                *operator,
                *right,
            )?;
        }
        OperationKind::Literal(ValueLiteral::String(_))
        | OperationKind::Variant { .. }
        | OperationKind::List(_)
        | OperationKind::Record(_)
        | OperationKind::Field { .. }
        | OperationKind::MatchList { .. }
        | OperationKind::MatchVariant { .. } => {
            return Err(BuildError::unsupported(
                "operation passed WebAssembly support checking without a lowering",
                operation.source,
            ));
        }
    }
    Ok(())
}

fn encode_unary(
    function: &mut Function,
    context: &FunctionContext<'_>,
    result: ValueId,
    operator: UnaryOperator,
    operand: ValueId,
) -> Result<(), BuildError> {
    match operator {
        UnaryOperator::Not => {
            load_value(function, context, operand)?;
            function.instruction(&Instruction::I32Eqz);
            store_value(function, context, result)?;
        }
        UnaryOperator::Negate => {
            if context.shared.minimum_literal_operands.contains(&operand) {
                function.instruction(&Instruction::I64Const(i64::MIN));
                store_value(function, context, result)?;
                return Ok(());
            }
            load_value(function, context, operand)?;
            function.instruction(&Instruction::I64Const(i64::MIN));
            function.instruction(&Instruction::I64Eq);
            trap_if(function);
            function.instruction(&Instruction::I64Const(0));
            load_value(function, context, operand)?;
            function.instruction(&Instruction::I64Sub);
            store_value(function, context, result)?;
        }
    }
    Ok(())
}

fn encode_binary(
    function: &mut Function,
    context: &FunctionContext<'_>,
    result: ValueId,
    left: ValueId,
    operator: BinaryOperator,
    right: ValueId,
) -> Result<(), BuildError> {
    match operator {
        BinaryOperator::Add => {
            load_pair(function, context, left, right)?;
            function.instruction(&Instruction::I64Add);
            store_value(function, context, result)?;
            load_value(function, context, left)?;
            load_value(function, context, result)?;
            function.instruction(&Instruction::I64Xor);
            load_value(function, context, right)?;
            load_value(function, context, result)?;
            function.instruction(&Instruction::I64Xor);
            function.instruction(&Instruction::I64And);
            function.instruction(&Instruction::I64Const(0));
            function.instruction(&Instruction::I64LtS);
            trap_if(function);
        }
        BinaryOperator::Subtract => {
            load_pair(function, context, left, right)?;
            function.instruction(&Instruction::I64Sub);
            store_value(function, context, result)?;
            load_value(function, context, left)?;
            load_value(function, context, right)?;
            function.instruction(&Instruction::I64Xor);
            load_value(function, context, left)?;
            load_value(function, context, result)?;
            function.instruction(&Instruction::I64Xor);
            function.instruction(&Instruction::I64And);
            function.instruction(&Instruction::I64Const(0));
            function.instruction(&Instruction::I64LtS);
            trap_if(function);
        }
        BinaryOperator::Multiply => {
            load_pair(function, context, left, right)?;
            function.instruction(&Instruction::I64Mul);
            store_value(function, context, result)?;
            load_value(function, context, right)?;
            function.instruction(&Instruction::I64Eqz);
            function.instruction(&Instruction::If(BlockType::Empty));
            function.instruction(&Instruction::Else);
            load_value(function, context, result)?;
            load_value(function, context, right)?;
            function.instruction(&Instruction::I64DivS);
            load_value(function, context, left)?;
            function.instruction(&Instruction::I64Ne);
            trap_if(function);
            function.instruction(&Instruction::End);
        }
        BinaryOperator::Divide => {
            load_pair(function, context, left, right)?;
            function.instruction(&Instruction::I64DivS);
            store_value(function, context, result)?;
        }
        BinaryOperator::Remainder => {
            load_value(function, context, left)?;
            function.instruction(&Instruction::I64Const(i64::MIN));
            function.instruction(&Instruction::I64Eq);
            load_value(function, context, right)?;
            function.instruction(&Instruction::I64Const(-1));
            function.instruction(&Instruction::I64Eq);
            function.instruction(&Instruction::I32And);
            trap_if(function);
            load_pair(function, context, left, right)?;
            function.instruction(&Instruction::I64RemS);
            store_value(function, context, result)?;
        }
        BinaryOperator::Equal | BinaryOperator::NotEqual => {
            let operand_type = context
                .value_types
                .get(&left)
                .ok_or_else(|| BuildError::invalid_core(format!("missing type for {left}")))?;
            match operand_type.as_ref() {
                Type::Int => {
                    load_pair(function, context, left, right)?;
                    function.instruction(if operator == BinaryOperator::Equal {
                        &Instruction::I64Eq
                    } else {
                        &Instruction::I64Ne
                    });
                }
                Type::Bool | Type::Function(_) => {
                    load_pair(function, context, left, right)?;
                    function.instruction(if operator == BinaryOperator::Equal {
                        &Instruction::I32Eq
                    } else {
                        &Instruction::I32Ne
                    });
                }
                Type::Unit => {
                    function.instruction(&Instruction::I32Const(i32::from(
                        operator == BinaryOperator::Equal,
                    )));
                }
                _ => {
                    return Err(BuildError::unsupported(
                        "equality operand has no WebAssembly policy 1 lowering",
                        None,
                    ));
                }
            }
            store_value(function, context, result)?;
        }
        BinaryOperator::Less
        | BinaryOperator::LessEqual
        | BinaryOperator::Greater
        | BinaryOperator::GreaterEqual => {
            load_pair(function, context, left, right)?;
            function.instruction(match operator {
                BinaryOperator::Less => &Instruction::I64LtS,
                BinaryOperator::LessEqual => &Instruction::I64LeS,
                BinaryOperator::Greater => &Instruction::I64GtS,
                BinaryOperator::GreaterEqual => &Instruction::I64GeS,
                _ => unreachable!("operator matched above"),
            });
            store_value(function, context, result)?;
        }
        BinaryOperator::And | BinaryOperator::Or => {
            return Err(BuildError::unsupported(
                "short-circuit operator was not lowered to a Core conditional",
                None,
            ));
        }
    }
    Ok(())
}

fn trap_if(function: &mut Function) {
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
}

fn load_pair(
    function: &mut Function,
    context: &FunctionContext<'_>,
    left: ValueId,
    right: ValueId,
) -> Result<(), BuildError> {
    load_value(function, context, left)?;
    load_value(function, context, right)
}

fn load_value(
    function: &mut Function,
    context: &FunctionContext<'_>,
    value: ValueId,
) -> Result<(), BuildError> {
    let ty = context
        .value_types
        .get(&value)
        .ok_or_else(|| BuildError::invalid_core(format!("missing type for {value}")))?;
    if scalar_layout(ty)?.is_some() {
        let local = context
            .locals
            .get(&value)
            .copied()
            .ok_or_else(|| BuildError::invalid_core(format!("missing local for {value}")))?;
        function.instruction(&Instruction::LocalGet(local));
    }
    Ok(())
}

fn store_value(
    function: &mut Function,
    context: &FunctionContext<'_>,
    value: ValueId,
) -> Result<(), BuildError> {
    let ty = context
        .value_types
        .get(&value)
        .ok_or_else(|| BuildError::invalid_core(format!("missing type for {value}")))?;
    if scalar_layout(ty)?.is_some() {
        let local = context
            .locals
            .get(&value)
            .copied()
            .ok_or_else(|| BuildError::invalid_core(format!("missing local for {value}")))?;
        function.instruction(&Instruction::LocalSet(local));
    }
    Ok(())
}

fn collect_builtin_flavors(module: &CoreModule) -> Result<Vec<BuiltinFlavor>, BuildError> {
    let mut flavors = BTreeSet::new();
    for function in module.functions() {
        collect_block_builtins(&function.body, &mut flavors)?;
    }
    Ok(flavors.into_iter().collect())
}

fn collect_block_builtins(
    block: &CoreBlock,
    flavors: &mut BTreeSet<BuiltinFlavor>,
) -> Result<(), BuildError> {
    for operation in &block.operations {
        match &operation.kind {
            OperationKind::Builtin(builtin) => {
                flavors.insert(BuiltinFlavor::from_operation(*builtin, &operation.ty)?);
            }
            OperationKind::Block { block } => collect_block_builtins(block, flavors)?,
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_block_builtins(consequent, flavors)?;
                collect_block_builtins(alternative, flavors)?;
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_block_builtins(empty, flavors)?;
                collect_block_builtins(cons, flavors)?;
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_block_builtins(&arm.block, flavors)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn encode_builtin_wrapper(
    flavor: BuiltinFlavor,
    contract: &WitContract,
) -> Result<Function, BuildError> {
    let mut function = Function::new(Vec::new());
    if flavor.has_value() {
        function.instruction(&Instruction::LocalGet(0));
    }
    function.instruction(&Instruction::I32Const(i32::from(flavor.newline())));
    let import = contract
        .import_indices
        .get(flavor.import_name())
        .copied()
        .ok_or_else(|| BuildError::artifact("stdout WIT import is missing"))?;
    function.instruction(&Instruction::Call(import));
    function.instruction(&Instruction::End);
    Ok(function)
}

fn function_signature(function: &CoreFunction) -> Result<Signature, BuildError> {
    type_signature(&function.signature.parameters, &function.signature.result)
}

fn type_signature(parameters: &[Arc<Type>], result: &Type) -> Result<Signature, BuildError> {
    let mut params = Vec::new();
    for parameter in parameters {
        if let Some(parameter) = scalar_layout(parameter)? {
            params.push(parameter);
        }
    }
    Ok(Signature {
        params,
        results: scalar_layout(result)?.into_iter().collect(),
    })
}

fn scalar_layout(ty: &Type) -> Result<Option<Scalar>, BuildError> {
    match ty {
        Type::Int => Ok(Some(Scalar::I64)),
        Type::Bool | Type::Function(_) => Ok(Some(Scalar::I32)),
        Type::Unit => Ok(None),
        unsupported => Err(BuildError::unsupported(
            format!("type `{unsupported}` has no scalar WebAssembly policy 1 layout"),
            None,
        )),
    }
}
