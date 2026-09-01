use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use krit::{
    BinaryOperator, BindingId, Builtin, CoreBlock, CoreFunction, CoreModule, CoreOperation,
    FunctionId, OperationKind, Type, UnaryOperator, ValueId, ValueLiteral, VariantFamily,
    VariantName,
};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ElementSection, Elements, EntityType,
    ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction, MemArg, MemorySection, MemoryType, Module, RefType, TableSection, TableType,
    TypeSection,
};

use crate::{
    BuildError,
    compiler::EncodedCore,
    wit::{Scalar, Signature, WitContract},
};

const STATIC_MEMORY_BASE: u32 = 1024;
const MAX_MEMORY_PAGES: u64 = 256;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum StdoutFlavor {
    PrintInt,
    PrintBool,
    PrintUnit,
    PrintlnInt,
    PrintlnBool,
    PrintlnUnit,
}

impl StdoutFlavor {
    fn from_builtin(builtin: Builtin, ty: &Type) -> Result<Self, BuildError> {
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
            .ok_or_else(|| BuildError::unsupported("stdout built-in has no argument", None))?;
        match (builtin, argument) {
            (Builtin::Print, Type::Int) => Ok(Self::PrintInt),
            (Builtin::Print, Type::Bool) => Ok(Self::PrintBool),
            (Builtin::Print, Type::Unit) => Ok(Self::PrintUnit),
            (Builtin::Println, Type::Int) => Ok(Self::PrintlnInt),
            (Builtin::Println, Type::Bool) => Ok(Self::PrintlnBool),
            (Builtin::Println, Type::Unit) => Ok(Self::PrintlnUnit),
            _ => Err(BuildError::unsupported(
                "stdout built-in has no bounded webhook lowering",
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

pub(crate) fn encode_webhook_core(
    module: &CoreModule,
    entrypoint: FunctionId,
    contract: &WitContract,
    minimum_literal_operands: &BTreeSet<ValueId>,
) -> Result<EncodedCore, BuildError> {
    let literals = StaticMemory::collect(module)?;
    let builtin_values = collect_builtin_values(module);
    let stdout_flavors = collect_stdout_flavors(module)?;
    let stdout_slots = stdout_flavors
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
    let static_functions = static_function_bindings(module);

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
    signatures.extend(stdout_flavors.iter().copied().map(StdoutFlavor::signature));
    signatures.insert(contract.entry_signature.clone());
    signatures.insert(contract.post_entry_signature.clone());
    let realloc_signature = Signature {
        params: vec![Scalar::I32; 4],
        results: vec![Scalar::I32],
    };
    signatures.insert(realloc_signature.clone());

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
        imports.import(
            &import.module,
            &import.name,
            EntityType::Function(signature_indices[&import.signature]),
        );
    }
    let import_count = u32::try_from(contract.imports.len())
        .map_err(|_| BuildError::artifact("too many WebAssembly imports"))?;
    let core_count = u32::try_from(module.functions().len())
        .map_err(|_| BuildError::artifact("too many Core functions"))?;
    let wrapper_count = u32::try_from(stdout_flavors.len())
        .map_err(|_| BuildError::artifact("too many stdout wrapper functions"))?;
    let adapter_index = import_count + core_count + wrapper_count;
    let realloc_index = adapter_index + 1;
    let post_index = realloc_index + 1;

    let mut functions = FunctionSection::new();
    for function in module.functions() {
        functions.function(signature_indices[&function_signature(function)?]);
    }
    for flavor in &stdout_flavors {
        functions.function(signature_indices[&flavor.signature()]);
    }
    functions.function(signature_indices[&contract.entry_signature]);
    functions.function(signature_indices[&realloc_signature]);
    functions.function(signature_indices[&contract.post_entry_signature]);

    let table_size = core_count
        .checked_add(wrapper_count)
        .ok_or_else(|| BuildError::artifact("WebAssembly function table is too large"))?;
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: u64::from(table_size),
        maximum: Some(u64::from(table_size)),
        shared: false,
    });

    let minimum_pages = u64::from(literals.heap_start().div_ceil(65_536).max(1));
    if minimum_pages > MAX_MEMORY_PAGES {
        return Err(BuildError::artifact(
            "webhook static data exceeds the bounded guest memory",
        ));
    }
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: minimum_pages,
        maximum: Some(MAX_MEMORY_PAGES),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: wasm_encoder::ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(
            i32::try_from(literals.heap_start())
                .map_err(|_| BuildError::artifact("guest heap offset exceeds i32"))?,
        ),
    );

    let mut exports = ExportSection::new();
    exports.export(&contract.entry_export, ExportKind::Func, adapter_index);
    exports.export(&contract.post_entry_export, ExportKind::Func, post_index);
    exports.export(&contract.memory_export, ExportKind::Memory, 0);
    exports.export(&contract.realloc_export, ExportKind::Func, realloc_index);

    let table_functions = (0..table_size)
        .map(|definition| import_count + definition)
        .collect::<Vec<_>>();
    let mut elements = ElementSection::new();
    elements.active(
        None,
        &ConstExpr::i32_const(0),
        Elements::Functions(Cow::Owned(table_functions)),
    );

    let shared = EncodeContext {
        signature_indices: &signature_indices,
        stdout_slots: &stdout_slots,
        minimum_literal_operands,
        literal_offsets: literals.offsets(),
        builtin_values: &builtin_values,
        static_functions: &static_functions,
        import_indices: &contract.import_indices,
        http_bearer: contract
            .component_imports
            .iter()
            .any(|interface| interface == crate::HTTP_INTERFACE),
        realloc_index,
    };
    let mut code = CodeSection::new();
    for function in module.functions() {
        code.function(&encode_function(function, &shared)?);
    }
    for flavor in &stdout_flavors {
        code.function(&encode_stdout_wrapper(*flavor, contract)?);
    }
    code.function(&encode_webhook_adapter(
        import_count + entrypoint.as_u32(),
        realloc_index,
    )?);
    code.function(&encode_realloc());
    code.function(&encode_post_return(&contract.post_entry_signature));

    let mut data = DataSection::new();
    data.active(
        0,
        &ConstExpr::i32_const(
            i32::try_from(STATIC_MEMORY_BASE)
                .map_err(|_| BuildError::artifact("static memory base exceeds i32"))?,
        ),
        literals.data().iter().copied(),
    );

    let mut wasm = Module::new();
    wasm.section(&types)
        .section(&imports)
        .section(&functions)
        .section(&tables)
        .section(&memories)
        .section(&globals)
        .section(&exports)
        .section(&elements)
        .section(&code)
        .section(&data);

    Ok(EncodedCore {
        bytes: wasm.finish(),
        table_size,
    })
}

struct StaticMemory {
    data: Vec<u8>,
    offsets: BTreeMap<ValueId, u32>,
    heap_start: u32,
}

impl StaticMemory {
    fn collect(module: &CoreModule) -> Result<Self, BuildError> {
        let mut builder = StaticMemoryBuilder {
            data: Vec::new(),
            offsets: BTreeMap::new(),
        };
        for function in module.functions() {
            builder.collect_block(&function.body)?;
        }
        let end = STATIC_MEMORY_BASE
            .checked_add(
                u32::try_from(builder.data.len())
                    .map_err(|_| BuildError::artifact("too much static guest data"))?,
            )
            .ok_or_else(|| BuildError::artifact("static guest data offset overflow"))?;
        Ok(Self {
            data: builder.data,
            offsets: builder.offsets,
            heap_start: align_u32(end, 8)
                .ok_or_else(|| BuildError::artifact("static guest heap offset overflow"))?,
        })
    }

    fn data(&self) -> &[u8] {
        &self.data
    }

    fn offsets(&self) -> &BTreeMap<ValueId, u32> {
        &self.offsets
    }

    const fn heap_start(&self) -> u32 {
        self.heap_start
    }
}

struct StaticMemoryBuilder {
    data: Vec<u8>,
    offsets: BTreeMap<ValueId, u32>,
}

impl StaticMemoryBuilder {
    fn collect_block(&mut self, block: &CoreBlock) -> Result<(), BuildError> {
        for operation in &block.operations {
            if let OperationKind::Literal(ValueLiteral::String(value)) = &operation.kind {
                self.add_string(operation.result, value)?;
            }
            match &operation.kind {
                OperationKind::Block { block } => self.collect_block(block)?,
                OperationKind::If {
                    consequent,
                    alternative,
                    ..
                } => {
                    self.collect_block(consequent)?;
                    self.collect_block(alternative)?;
                }
                OperationKind::MatchList { empty, cons, .. } => {
                    self.collect_block(empty)?;
                    self.collect_block(cons)?;
                }
                OperationKind::MatchVariant { arms, .. } => {
                    for arm in arms {
                        self.collect_block(&arm.block)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn add_string(&mut self, value_id: ValueId, value: &str) -> Result<(), BuildError> {
        self.align(4)?;
        let pair_offset = self.absolute_offset()?;
        self.data.extend_from_slice(&[0; 8]);
        let data_offset = self.absolute_offset()?;
        self.data.extend_from_slice(value.as_bytes());
        let length = u32::try_from(value.len())
            .map_err(|_| BuildError::artifact("string literal is too large"))?;
        let relative = usize::try_from(pair_offset - STATIC_MEMORY_BASE)
            .map_err(|_| BuildError::artifact("string pair offset is invalid"))?;
        self.data[relative..relative + 4].copy_from_slice(&data_offset.to_le_bytes());
        self.data[relative + 4..relative + 8].copy_from_slice(&length.to_le_bytes());
        self.offsets.insert(value_id, pair_offset);
        Ok(())
    }

    fn align(&mut self, alignment: u32) -> Result<(), BuildError> {
        let absolute = self.absolute_offset()?;
        let aligned = align_u32(absolute, alignment)
            .ok_or_else(|| BuildError::artifact("static data alignment overflow"))?;
        let padding = usize::try_from(aligned - absolute)
            .map_err(|_| BuildError::artifact("static data padding overflow"))?;
        self.data.resize(self.data.len() + padding, 0);
        Ok(())
    }

    fn absolute_offset(&self) -> Result<u32, BuildError> {
        STATIC_MEMORY_BASE
            .checked_add(
                u32::try_from(self.data.len())
                    .map_err(|_| BuildError::artifact("too much static guest data"))?,
            )
            .ok_or_else(|| BuildError::artifact("static guest data offset overflow"))
    }
}

struct EncodeContext<'a> {
    signature_indices: &'a BTreeMap<Signature, u32>,
    stdout_slots: &'a BTreeMap<StdoutFlavor, u32>,
    minimum_literal_operands: &'a BTreeSet<ValueId>,
    literal_offsets: &'a BTreeMap<ValueId, u32>,
    builtin_values: &'a BTreeMap<ValueId, Builtin>,
    static_functions: &'a BTreeMap<BindingId, FunctionId>,
    import_indices: &'a BTreeMap<String, u32>,
    http_bearer: bool,
    realloc_index: u32,
}

struct FunctionContext<'a> {
    shared: &'a EncodeContext<'a>,
    locals: BTreeMap<ValueId, u32>,
    value_types: BTreeMap<ValueId, Arc<Type>>,
    scratch_i32: u32,
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
        if value_layout(&parameter.ty)?.is_some() {
            locals.insert(parameter.value, next_parameter);
            next_parameter += 1;
        }
    }

    let mut local_declarations = Vec::new();
    for capture in &core.captures {
        value_types.insert(capture.value, Arc::clone(&capture.ty));
        push_local(
            capture.value,
            &capture.ty,
            next_parameter,
            &mut locals,
            &mut local_declarations,
        )?;
    }
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
    let scratch_i32 = next_parameter
        .checked_add(
            u32::try_from(local_declarations.len())
                .map_err(|_| BuildError::artifact("too many WebAssembly locals"))?,
        )
        .ok_or_else(|| BuildError::artifact("too many WebAssembly locals"))?;
    local_declarations.push(Scalar::I32);

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
        scratch_i32,
    };

    for capture in &core.captures {
        let target = shared
            .static_functions
            .get(&capture.binding)
            .ok_or_else(|| {
                BuildError::unsupported(
                    "webhook helper capture is not a static function",
                    core.source,
                )
            })?;
        function.instruction(&Instruction::I32Const(
            i32::try_from(target.as_u32())
                .map_err(|_| BuildError::artifact("function table slot exceeds i32"))?,
        ));
        store_value(&mut function, &context, capture.value)?;
    }
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
                collect_block_locals(block, parameter_count, locals, value_types, declarations)?;
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
    if let Some(scalar) = value_layout(ty)? {
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
        OperationKind::Literal(ValueLiteral::String(_)) => {
            let offset = context
                .shared
                .literal_offsets
                .get(&operation.result)
                .copied()
                .ok_or_else(|| BuildError::invalid_core("missing static string literal"))?;
            function.instruction(&Instruction::I32Const(
                i32::try_from(offset)
                    .map_err(|_| BuildError::artifact("string offset exceeds i32"))?,
            ));
            store_value(function, context, operation.result)?;
        }
        OperationKind::Unit | OperationKind::Bind { .. } | OperationKind::Discard { .. } => {}
        OperationKind::Builtin(builtin) => {
            if matches!(builtin, Builtin::Print | Builtin::Println) {
                let flavor = StdoutFlavor::from_builtin(*builtin, &operation.ty)?;
                let slot = context.shared.stdout_slots[&flavor];
                function
                    .instruction(&Instruction::I32Const(i32::try_from(slot).map_err(
                        |_| BuildError::artifact("function table slot exceeds i32"),
                    )?));
            } else {
                function.instruction(&Instruction::I32Const(0));
            }
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
            if let Some(builtin) = context.shared.builtin_values.get(callee) {
                encode_builtin_call(function, context, operation, *builtin, arguments)?;
            } else {
                let callee_type = context.value_types.get(callee).ok_or_else(|| {
                    BuildError::invalid_core(format!("missing type for {callee}"))
                })?;
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
        }
        OperationKind::Variant { variant, payload } => {
            encode_variant(function, context, operation, *variant, *payload)?;
        }
        OperationKind::List(values) => encode_header_list(function, context, operation, values)?,
        OperationKind::Record(fields) => {
            encode_record(function, context, operation, fields)?;
        }
        OperationKind::Field { value, field } => {
            encode_field(function, context, operation, *value, field)?;
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
        OperationKind::MatchVariant {
            subject,
            family,
            arms,
        } => encode_variant_match(function, context, operation, *subject, *family, arms)?,
        OperationKind::MatchList { .. } => {
            return Err(BuildError::unsupported(
                "list matching is outside the bounded webhook ABI",
                operation.source,
            ));
        }
        OperationKind::Unary { operator, operand } => {
            encode_unary(function, context, operation.result, *operator, *operand)?;
        }
        OperationKind::Binary {
            left,
            operator,
            right,
        } => encode_binary(
            function,
            context,
            operation.result,
            *left,
            *operator,
            *right,
        )?,
    }
    Ok(())
}

fn encode_builtin_call(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    builtin: Builtin,
    arguments: &[ValueId],
) -> Result<(), BuildError> {
    match builtin {
        Builtin::Print | Builtin::Println => {
            let argument = arguments
                .first()
                .copied()
                .ok_or_else(|| BuildError::invalid_core("stdout call has no argument"))?;
            let argument_type = context
                .value_types
                .get(&argument)
                .ok_or_else(|| BuildError::invalid_core("missing stdout argument type"))?;
            let flavor = match (builtin, argument_type.as_ref()) {
                (Builtin::Print, Type::Int) => StdoutFlavor::PrintInt,
                (Builtin::Print, Type::Bool) => StdoutFlavor::PrintBool,
                (Builtin::Print, Type::Unit) => StdoutFlavor::PrintUnit,
                (Builtin::Println, Type::Int) => StdoutFlavor::PrintlnInt,
                (Builtin::Println, Type::Bool) => StdoutFlavor::PrintlnBool,
                (Builtin::Println, Type::Unit) => StdoutFlavor::PrintlnUnit,
                _ => {
                    return Err(BuildError::unsupported(
                        "stdout argument is outside the bounded webhook ABI",
                        operation.source,
                    ));
                }
            };
            if flavor.has_value() {
                load_value(function, context, argument)?;
            }
            function.instruction(&Instruction::I32Const(i32::from(flavor.newline())));
            call_import(function, context, flavor.import_name())?;
        }
        Builtin::Some | Builtin::Ok | Builtin::Err => {
            let variant = match builtin {
                Builtin::Some => VariantName::Some,
                Builtin::Ok => VariantName::Ok,
                Builtin::Err => VariantName::Err,
                _ => unreachable!("constructor matched above"),
            };
            encode_variant(
                function,
                context,
                operation,
                variant,
                arguments.first().copied(),
            )?;
        }
        Builtin::ConfigString | Builtin::Secret => {
            let argument = arguments
                .first()
                .copied()
                .ok_or_else(|| BuildError::invalid_core("host call has no resource argument"))?;
            allocate_result(function, context, operation.result, 12, 4)?;
            load_string_flat(function, context, argument)?;
            load_value(function, context, operation.result)?;
            call_import(
                function,
                context,
                if builtin == Builtin::ConfigString {
                    "get-string"
                } else {
                    "acquire"
                },
            )?;
        }
        Builtin::HttpRequest => {
            let [origin, request, bearer] = arguments else {
                return Err(BuildError::invalid_core(
                    "http_request call does not have three arguments",
                ));
            };
            allocate_result(function, context, operation.result, 32, 8)?;
            load_string_flat(function, context, *origin)?;
            load_request_flat(function, context, *request)?;
            if context.shared.http_bearer {
                load_option_secret_flat(function, context, *bearer)?;
            }
            load_value(function, context, operation.result)?;
            call_import(function, context, "send")?;
        }
        Builtin::None | Builtin::JsonEncode | Builtin::JsonDecode => {
            return Err(BuildError::unsupported(
                format!(
                    "built-in `{}` has no bounded webhook call lowering",
                    builtin.as_str()
                ),
                operation.source,
            ));
        }
    }
    Ok(())
}

fn encode_variant(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    variant: VariantName,
    payload: Option<ValueId>,
) -> Result<(), BuildError> {
    let layout = variant_layout(&operation.ty)?;
    allocate_result(
        function,
        context,
        operation.result,
        layout.size,
        layout.align,
    )?;
    load_value(function, context, operation.result)?;
    function.instruction(&Instruction::I32Const(i32::from(variant_tag(variant))));
    function.instruction(&Instruction::I32Store8(memarg(0, 0)));
    if let Some(payload) = payload {
        let payload_type = context
            .value_types
            .get(&payload)
            .ok_or_else(|| BuildError::invalid_core("missing variant payload type"))?;
        copy_value_to_memory(
            function,
            context,
            operation.result,
            layout.payload_offset,
            payload,
            payload_type,
        )?;
    }
    Ok(())
}

fn encode_header_list(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    values: &[ValueId],
) -> Result<(), BuildError> {
    let byte_size = u32::try_from(values.len())
        .ok()
        .and_then(|length| length.checked_mul(16))
        .ok_or_else(|| BuildError::artifact("header list is too large"))?;
    if byte_size == 0 {
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::LocalSet(context.scratch_i32));
    } else {
        allocate_to_scratch(function, context, byte_size, 4)?;
        for (index, value) in values.iter().enumerate() {
            let offset = u32::try_from(index)
                .ok()
                .and_then(|index| index.checked_mul(16))
                .ok_or_else(|| BuildError::artifact("header list offset overflow"))?;
            for word in [0u32, 4, 8, 12] {
                function.instruction(&Instruction::LocalGet(context.scratch_i32));
                load_value(function, context, *value)?;
                function.instruction(&Instruction::I32Load(memarg(word, 2)));
                function.instruction(&Instruction::I32Store(memarg(offset + word, 2)));
            }
        }
    }
    allocate_result(function, context, operation.result, 8, 4)?;
    load_value(function, context, operation.result)?;
    function.instruction(&Instruction::LocalGet(context.scratch_i32));
    function.instruction(&Instruction::I32Store(memarg(0, 2)));
    load_value(function, context, operation.result)?;
    function.instruction(&Instruction::I32Const(
        i32::try_from(values.len())
            .map_err(|_| BuildError::artifact("header list length exceeds i32"))?,
    ));
    function.instruction(&Instruction::I32Store(memarg(4, 2)));
    Ok(())
}

fn encode_record(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    fields: &[krit::RecordOperand],
) -> Result<(), BuildError> {
    let layout = record_layout(&operation.ty)?;
    allocate_result(
        function,
        context,
        operation.result,
        layout.size,
        layout.align,
    )?;
    for field in fields {
        let (offset, ty) = record_field(&operation.ty, &field.name)?;
        copy_value_to_memory(
            function,
            context,
            operation.result,
            offset,
            field.value,
            &ty,
        )?;
    }
    Ok(())
}

fn encode_field(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    value: ValueId,
    field: &str,
) -> Result<(), BuildError> {
    let source_type = context
        .value_types
        .get(&value)
        .ok_or_else(|| BuildError::invalid_core("missing record source type"))?;
    let (offset, field_type) = record_field(source_type, field)?;
    match value_layout(&field_type)? {
        None => {}
        Some(Scalar::I64) => {
            load_value(function, context, value)?;
            function.instruction(&Instruction::I64Load(memarg(offset, 3)));
            store_value(function, context, operation.result)?;
        }
        Some(Scalar::I32) if is_pointer_value(&field_type) => {
            let layout = memory_layout(&field_type)?;
            if layout.size == 8 && layout.align == 4 {
                allocate_result(function, context, operation.result, 8, 4)?;
                for word in [0u32, 4] {
                    load_value(function, context, operation.result)?;
                    load_value(function, context, value)?;
                    function.instruction(&Instruction::I32Load(memarg(offset + word, 2)));
                    function.instruction(&Instruction::I32Store(memarg(word, 2)));
                }
            } else {
                load_value(function, context, value)?;
                function.instruction(&Instruction::I32Const(
                    i32::try_from(offset)
                        .map_err(|_| BuildError::artifact("field offset exceeds i32"))?,
                ));
                function.instruction(&Instruction::I32Add);
                store_value(function, context, operation.result)?;
            }
        }
        Some(Scalar::I32) => {
            load_value(function, context, value)?;
            function.instruction(&Instruction::I32Load(memarg(offset, 2)));
            store_value(function, context, operation.result)?;
        }
    }
    Ok(())
}

fn encode_variant_match(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    subject: ValueId,
    family: VariantFamily,
    arms: &[krit::VariantArmBlock],
) -> Result<(), BuildError> {
    let zero_variant = match family {
        VariantFamily::Option => VariantName::None,
        VariantFamily::Result => VariantName::Ok,
    };
    let one_variant = match family {
        VariantFamily::Option => VariantName::Some,
        VariantFamily::Result => VariantName::Err,
    };
    let zero = arms
        .iter()
        .find(|arm| arm.variant == zero_variant)
        .ok_or_else(|| BuildError::invalid_core("variant match is missing its zero arm"))?;
    let one = arms
        .iter()
        .find(|arm| arm.variant == one_variant)
        .ok_or_else(|| BuildError::invalid_core("variant match is missing its one arm"))?;
    load_value(function, context, subject)?;
    function.instruction(&Instruction::I32Load8U(memarg(0, 0)));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    encode_variant_arm(function, context, operation, subject, zero)?;
    function.instruction(&Instruction::Else);
    encode_variant_arm(function, context, operation, subject, one)?;
    function.instruction(&Instruction::End);
    Ok(())
}

fn encode_variant_arm(
    function: &mut Function,
    context: &FunctionContext<'_>,
    operation: &CoreOperation,
    subject: ValueId,
    arm: &krit::VariantArmBlock,
) -> Result<(), BuildError> {
    if let Some(parameter) = arm.block.parameters.first() {
        let subject_type = context
            .value_types
            .get(&subject)
            .ok_or_else(|| BuildError::invalid_core("missing variant subject type"))?;
        let layout = variant_layout(subject_type)?;
        if is_pointer_value(&parameter.ty) {
            load_value(function, context, subject)?;
            function.instruction(&Instruction::I32Const(
                i32::try_from(layout.payload_offset)
                    .map_err(|_| BuildError::artifact("variant payload offset exceeds i32"))?,
            ));
            function.instruction(&Instruction::I32Add);
        } else {
            load_value(function, context, subject)?;
            match value_layout(&parameter.ty)? {
                Some(Scalar::I32) => {
                    function.instruction(&Instruction::I32Load(memarg(layout.payload_offset, 2)));
                }
                Some(Scalar::I64) => {
                    function.instruction(&Instruction::I64Load(memarg(layout.payload_offset, 3)));
                }
                None => {}
            }
        }
        store_value(function, context, parameter.value)?;
    }
    encode_block(function, context, &arm.block)?;
    load_value(function, context, arm.block.result)?;
    store_value(function, context, operation.result)
}

fn copy_value_to_memory(
    function: &mut Function,
    context: &FunctionContext<'_>,
    destination: ValueId,
    destination_offset: u32,
    source: ValueId,
    ty: &Type,
) -> Result<(), BuildError> {
    match value_layout(ty)? {
        None => Ok(()),
        Some(Scalar::I64) => {
            load_value(function, context, destination)?;
            load_value(function, context, source)?;
            function.instruction(&Instruction::I64Store(memarg(destination_offset, 3)));
            Ok(())
        }
        Some(Scalar::I32) if is_pointer_value(ty) => {
            let layout = memory_layout(ty)?;
            let mut offset = 0u32;
            while offset + 8 <= layout.size {
                load_value(function, context, destination)?;
                load_value(function, context, source)?;
                function.instruction(&Instruction::I64Load(memarg(offset, 0)));
                function.instruction(&Instruction::I64Store(memarg(
                    destination_offset + offset,
                    0,
                )));
                offset += 8;
            }
            while offset + 4 <= layout.size {
                load_value(function, context, destination)?;
                load_value(function, context, source)?;
                function.instruction(&Instruction::I32Load(memarg(offset, 0)));
                function.instruction(&Instruction::I32Store(memarg(
                    destination_offset + offset,
                    0,
                )));
                offset += 4;
            }
            if offset != layout.size {
                return Err(BuildError::unsupported(
                    "bounded webhook memory copy encountered an unsupported layout",
                    None,
                ));
            }
            Ok(())
        }
        Some(Scalar::I32) => {
            load_value(function, context, destination)?;
            load_value(function, context, source)?;
            function.instruction(&Instruction::I32Store(memarg(destination_offset, 2)));
            Ok(())
        }
    }
}

fn load_string_flat(
    function: &mut Function,
    context: &FunctionContext<'_>,
    value: ValueId,
) -> Result<(), BuildError> {
    load_value(function, context, value)?;
    function.instruction(&Instruction::I32Load(memarg(0, 2)));
    load_value(function, context, value)?;
    function.instruction(&Instruction::I32Load(memarg(4, 2)));
    Ok(())
}

fn load_request_flat(
    function: &mut Function,
    context: &FunctionContext<'_>,
    value: ValueId,
) -> Result<(), BuildError> {
    for offset in [0u32, 4, 8, 12, 16, 20, 24, 28, 32, 36] {
        load_value(function, context, value)?;
        function.instruction(&Instruction::I32Load(memarg(offset, 2)));
    }
    Ok(())
}

fn load_option_secret_flat(
    function: &mut Function,
    context: &FunctionContext<'_>,
    value: ValueId,
) -> Result<(), BuildError> {
    load_value(function, context, value)?;
    function.instruction(&Instruction::I32Load8U(memarg(0, 0)));
    load_value(function, context, value)?;
    function.instruction(&Instruction::I32Load(memarg(4, 2)));
    Ok(())
}

fn allocate_result(
    function: &mut Function,
    context: &FunctionContext<'_>,
    result: ValueId,
    size: u32,
    align: u32,
) -> Result<(), BuildError> {
    emit_allocate(function, context.shared.realloc_index, size, align);
    store_value(function, context, result)
}

fn allocate_to_scratch(
    function: &mut Function,
    context: &FunctionContext<'_>,
    size: u32,
    align: u32,
) -> Result<(), BuildError> {
    emit_allocate(function, context.shared.realloc_index, size, align);
    function.instruction(&Instruction::LocalSet(context.scratch_i32));
    Ok(())
}

fn emit_allocate(function: &mut Function, realloc_index: u32, size: u32, align: u32) {
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(align as i32));
    function.instruction(&Instruction::I32Const(size as i32));
    function.instruction(&Instruction::Call(realloc_index));
}

fn call_import(
    function: &mut Function,
    context: &FunctionContext<'_>,
    name: &str,
) -> Result<(), BuildError> {
    let import = context
        .shared
        .import_indices
        .get(name)
        .copied()
        .ok_or_else(|| BuildError::artifact(format!("WIT import `{name}` is missing")))?;
    function.instruction(&Instruction::Call(import));
    Ok(())
}

fn encode_webhook_adapter(webhook_index: u32, realloc_index: u32) -> Result<Function, BuildError> {
    let mut function = Function::new([(1, wasm_encoder::ValType::I32)]);
    emit_allocate(&mut function, realloc_index, 40, 4);
    function.instruction(&Instruction::LocalSet(10));
    for (parameter, offset) in (0u32..10).zip([0u32, 4, 8, 12, 16, 20, 24, 28, 32, 36]) {
        function.instruction(&Instruction::LocalGet(10));
        function.instruction(&Instruction::LocalGet(parameter));
        function.instruction(&Instruction::I32Store(memarg(offset, 2)));
    }
    function.instruction(&Instruction::LocalGet(10));
    function.instruction(&Instruction::Call(webhook_index));
    function.instruction(&Instruction::End);
    Ok(function)
}

fn encode_realloc() -> Function {
    let mut function = Function::new([(4, wasm_encoder::ValType::I32)]);
    let allocation = 4;
    let end = 5;
    let scratch = 6;
    let copy_index = 7;

    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Result(
        wasm_encoder::ValType::I32,
    )));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Else);

    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(8));
    function.instruction(&Instruction::I32GtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);

    function.instruction(&Instruction::GlobalGet(0));
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalGet(2));
    function.instruction(&Instruction::I32Const(-1));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::I32And);
    function.instruction(&Instruction::LocalTee(allocation));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalTee(end));
    function.instruction(&Instruction::LocalGet(allocation));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);

    function.instruction(&Instruction::MemorySize(0));
    function.instruction(&Instruction::I32Const(65_536));
    function.instruction(&Instruction::I32Mul);
    function.instruction(&Instruction::LocalTee(scratch));
    function.instruction(&Instruction::LocalGet(end));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(end));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::I32Const(16));
    function.instruction(&Instruction::I32ShrU);
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::MemorySize(0));
    function.instruction(&Instruction::I32Sub);
    function.instruction(&Instruction::MemoryGrow(0));
    function.instruction(&Instruction::I32Const(-1));
    function.instruction(&Instruction::I32Eq);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Unreachable);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    function.instruction(&Instruction::LocalGet(end));
    function.instruction(&Instruction::GlobalSet(0));

    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::I32LtU);
    function.instruction(&Instruction::If(BlockType::Result(
        wasm_encoder::ValType::I32,
    )));
    function.instruction(&Instruction::LocalGet(1));
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::LocalGet(3));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::LocalSet(scratch));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalSet(copy_index));
    function.instruction(&Instruction::Block(BlockType::Empty));
    function.instruction(&Instruction::Loop(BlockType::Empty));
    function.instruction(&Instruction::LocalGet(copy_index));
    function.instruction(&Instruction::LocalGet(scratch));
    function.instruction(&Instruction::I32GeU);
    function.instruction(&Instruction::BrIf(1));
    function.instruction(&Instruction::LocalGet(allocation));
    function.instruction(&Instruction::LocalGet(copy_index));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalGet(0));
    function.instruction(&Instruction::LocalGet(copy_index));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::I32Load8U(memarg(0, 0)));
    function.instruction(&Instruction::I32Store8(memarg(0, 0)));
    function.instruction(&Instruction::LocalGet(copy_index));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Add);
    function.instruction(&Instruction::LocalSet(copy_index));
    function.instruction(&Instruction::Br(0));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);

    function.instruction(&Instruction::LocalGet(allocation));
    function.instruction(&Instruction::End);
    function.instruction(&Instruction::End);
    function
}

fn encode_post_return(signature: &Signature) -> Function {
    let mut function = Function::new(Vec::new());
    for index in 0..signature.params.len() {
        function.instruction(&Instruction::LocalGet(index as u32));
        function.instruction(&Instruction::Drop);
    }
    function.instruction(&Instruction::End);
    function
}

fn encode_stdout_wrapper(
    flavor: StdoutFlavor,
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
            trap_integer_overflow_if(function);
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
            require_int_operand(context, left, operator)?;
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
            trap_integer_overflow_if(function);
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
            trap_integer_overflow_if(function);
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
            trap_integer_overflow_if(function);
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
            trap_integer_overflow_if(function);
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
                        "composite equality is outside the bounded webhook ABI",
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

fn require_int_operand(
    context: &FunctionContext<'_>,
    value: ValueId,
    operator: BinaryOperator,
) -> Result<(), BuildError> {
    if context.value_types.get(&value).map(Arc::as_ref) == Some(&Type::Int) {
        Ok(())
    } else {
        Err(BuildError::unsupported(
            format!("`{operator:?}` on String is outside the bounded webhook ABI"),
            None,
        ))
    }
}

fn trap_integer_overflow_if(function: &mut Function) {
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I64Const(i64::MIN));
    function.instruction(&Instruction::I64Const(-1));
    function.instruction(&Instruction::I64DivS);
    function.instruction(&Instruction::Drop);
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
    if value_layout(ty)?.is_some() {
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
    if value_layout(ty)?.is_some() {
        let local = context
            .locals
            .get(&value)
            .copied()
            .ok_or_else(|| BuildError::invalid_core(format!("missing local for {value}")))?;
        function.instruction(&Instruction::LocalSet(local));
    }
    Ok(())
}

fn collect_builtin_values(module: &CoreModule) -> BTreeMap<ValueId, Builtin> {
    let mut values = BTreeMap::new();
    for function in module.functions() {
        collect_block_builtins(&function.body, &mut values);
    }
    values
}

fn collect_block_builtins(block: &CoreBlock, values: &mut BTreeMap<ValueId, Builtin>) {
    for operation in &block.operations {
        if let OperationKind::Builtin(builtin) = operation.kind {
            values.insert(operation.result, builtin);
        }
        match &operation.kind {
            OperationKind::Block { block } => collect_block_builtins(block, values),
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_block_builtins(consequent, values);
                collect_block_builtins(alternative, values);
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_block_builtins(empty, values);
                collect_block_builtins(cons, values);
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_block_builtins(&arm.block, values);
                }
            }
            _ => {}
        }
    }
}

fn collect_stdout_flavors(module: &CoreModule) -> Result<Vec<StdoutFlavor>, BuildError> {
    let mut flavors = BTreeSet::new();
    for function in module.functions() {
        collect_block_stdout_flavors(&function.body, &mut flavors)?;
    }
    Ok(flavors.into_iter().collect())
}

fn collect_block_stdout_flavors(
    block: &CoreBlock,
    flavors: &mut BTreeSet<StdoutFlavor>,
) -> Result<(), BuildError> {
    for operation in &block.operations {
        if let OperationKind::Builtin(builtin @ (Builtin::Print | Builtin::Println)) =
            &operation.kind
        {
            flavors.insert(StdoutFlavor::from_builtin(*builtin, &operation.ty)?);
        }
        match &operation.kind {
            OperationKind::Block { block } => collect_block_stdout_flavors(block, flavors)?,
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => {
                collect_block_stdout_flavors(consequent, flavors)?;
                collect_block_stdout_flavors(alternative, flavors)?;
            }
            OperationKind::MatchList { empty, cons, .. } => {
                collect_block_stdout_flavors(empty, flavors)?;
                collect_block_stdout_flavors(cons, flavors)?;
            }
            OperationKind::MatchVariant { arms, .. } => {
                for arm in arms {
                    collect_block_stdout_flavors(&arm.block, flavors)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn static_function_bindings(module: &CoreModule) -> BTreeMap<BindingId, FunctionId> {
    let mut values = BTreeMap::new();
    let mut bindings = BTreeMap::new();
    for operation in &module.entrypoint_function().body.operations {
        match &operation.kind {
            OperationKind::Closure {
                function, captures, ..
            } => {
                values.insert(
                    operation.result,
                    (
                        *function,
                        captures
                            .iter()
                            .map(|capture| capture.binding)
                            .collect::<Vec<_>>(),
                    ),
                );
            }
            OperationKind::Bind { binding, value } => {
                if let Some((function, captures)) = values.get(value)
                    && captures
                        .iter()
                        .all(|capture| bindings.contains_key(capture))
                {
                    bindings.insert(*binding, *function);
                }
            }
            _ => {}
        }
    }
    bindings
}

fn function_signature(function: &CoreFunction) -> Result<Signature, BuildError> {
    type_signature(&function.signature.parameters, &function.signature.result)
}

fn type_signature(parameters: &[Arc<Type>], result: &Type) -> Result<Signature, BuildError> {
    let mut params = Vec::new();
    for parameter in parameters {
        if let Some(parameter) = value_layout(parameter)? {
            params.push(parameter);
        }
    }
    Ok(Signature {
        params,
        results: value_layout(result)?.into_iter().collect(),
    })
}

fn value_layout(ty: &Type) -> Result<Option<Scalar>, BuildError> {
    match ty {
        Type::Int => Ok(Some(Scalar::I64)),
        Type::Unit => Ok(None),
        Type::Bool
        | Type::String
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::Secret
        | Type::List(_)
        | Type::Record(_)
        | Type::Option(_)
        | Type::Result(_, _)
        | Type::Function(_) => Ok(Some(Scalar::I32)),
        Type::Variable(_) => Err(BuildError::residual(
            "bounded webhook ABI requires specialization of a parametric type",
            None,
        )),
    }
}

#[derive(Clone, Copy)]
struct Layout {
    size: u32,
    align: u32,
    payload_offset: u32,
}

fn memory_layout(ty: &Type) -> Result<Layout, BuildError> {
    if is_string(ty) || is_header_list(ty) {
        return Ok(Layout {
            size: 8,
            align: 4,
            payload_offset: 0,
        });
    }
    if is_header(ty) {
        return Ok(Layout {
            size: 16,
            align: 4,
            payload_offset: 0,
        });
    }
    if is_request(ty) {
        return Ok(Layout {
            size: 40,
            align: 4,
            payload_offset: 0,
        });
    }
    if is_response(ty) {
        return Ok(Layout {
            size: 24,
            align: 8,
            payload_offset: 0,
        });
    }
    variant_layout(ty)
}

fn variant_layout(ty: &Type) -> Result<Layout, BuildError> {
    match ty {
        Type::Option(element) if element.as_ref() == &Type::Secret => Ok(Layout {
            size: 8,
            align: 4,
            payload_offset: 4,
        }),
        Type::Result(value, error)
            if error.as_ref() == &Type::String
                && matches!(value.as_ref(), Type::String | Type::Secret) =>
        {
            Ok(Layout {
                size: 12,
                align: 4,
                payload_offset: 4,
            })
        }
        Type::Result(value, error)
            if value.as_ref() == &Type::HttpResponse && error.as_ref() == &Type::String =>
        {
            Ok(Layout {
                size: 32,
                align: 8,
                payload_offset: 8,
            })
        }
        _ => Err(BuildError::unsupported(
            format!("type `{ty}` has no bounded webhook variant layout"),
            None,
        )),
    }
}

fn record_layout(ty: &Type) -> Result<Layout, BuildError> {
    if is_header(ty) || is_request(ty) || is_response(ty) {
        memory_layout(ty)
    } else {
        Err(BuildError::unsupported(
            format!("record `{ty}` is outside the bounded webhook ABI"),
            None,
        ))
    }
}

fn record_field(ty: &Type, field: &str) -> Result<(u32, Type), BuildError> {
    if is_header(ty) {
        return match field {
            "name" => Ok((0, Type::String)),
            "value" => Ok((8, Type::String)),
            _ => Err(BuildError::invalid_core("unknown HttpHeader field")),
        };
    }
    if is_request(ty) {
        return match field {
            "method" => Ok((0, Type::String)),
            "path" => Ok((8, Type::String)),
            "query" => Ok((16, Type::String)),
            "headers" => Ok((24, Type::List(Arc::new(Type::HttpHeader)))),
            "body" => Ok((32, Type::String)),
            _ => Err(BuildError::invalid_core("unknown HttpRequest field")),
        };
    }
    if is_response(ty) {
        return match field {
            "status" => Ok((0, Type::Int)),
            "headers" => Ok((8, Type::List(Arc::new(Type::HttpHeader)))),
            "body" => Ok((16, Type::String)),
            _ => Err(BuildError::invalid_core("unknown HttpResponse field")),
        };
    }
    Err(BuildError::unsupported(
        format!("record field `{field}` is outside the bounded webhook ABI"),
        None,
    ))
}

fn is_pointer_value(ty: &Type) -> bool {
    matches!(
        ty,
        Type::String
            | Type::HttpHeader
            | Type::HttpRequest
            | Type::HttpResponse
            | Type::List(_)
            | Type::Record(_)
            | Type::Option(_)
            | Type::Result(_, _)
    )
}

fn is_string(ty: &Type) -> bool {
    ty == &Type::String
}

fn is_header_list(ty: &Type) -> bool {
    matches!(ty, Type::List(element) if is_header(element))
}

fn is_header(ty: &Type) -> bool {
    match ty {
        Type::HttpHeader => true,
        Type::Record(fields) => {
            record_matches(fields, &[("name", Type::String), ("value", Type::String)])
        }
        _ => false,
    }
}

fn is_request(ty: &Type) -> bool {
    match ty {
        Type::HttpRequest => true,
        Type::Record(fields) => record_matches(
            fields,
            &[
                ("method", Type::String),
                ("path", Type::String),
                ("query", Type::String),
                ("headers", Type::List(Arc::new(Type::HttpHeader))),
                ("body", Type::String),
            ],
        ),
        _ => false,
    }
}

fn is_response(ty: &Type) -> bool {
    match ty {
        Type::HttpResponse => true,
        Type::Record(fields) => record_matches(
            fields,
            &[
                ("status", Type::Int),
                ("headers", Type::List(Arc::new(Type::HttpHeader))),
                ("body", Type::String),
            ],
        ),
        _ => false,
    }
}

fn record_matches(fields: &[krit::RecordType], expected: &[(&str, Type)]) -> bool {
    fields.len() == expected.len()
        && expected.iter().all(|(name, ty)| {
            fields
                .iter()
                .find(|field| field.name() == *name)
                .is_some_and(|field| equivalent(field.ty(), ty))
        })
}

fn equivalent(left: &Type, right: &Type) -> bool {
    left == right
        || (is_header(left) && is_header(right))
        || (is_request(left) && is_request(right))
        || (is_response(left) && is_response(right))
        || matches!(
            (left, right),
            (Type::List(left), Type::List(right)) if equivalent(left, right)
        )
}

const fn variant_tag(variant: VariantName) -> u8 {
    match variant {
        VariantName::None | VariantName::Ok => 0,
        VariantName::Some | VariantName::Err => 1,
    }
}

const fn memarg(offset: u32, align: u32) -> MemArg {
    MemArg {
        offset: offset as u64,
        align,
        memory_index: 0,
    }
}

fn align_u32(value: u32, alignment: u32) -> Option<u32> {
    value
        .checked_add(alignment.checked_sub(1)?)
        .map(|value| value & !(alignment - 1))
}
