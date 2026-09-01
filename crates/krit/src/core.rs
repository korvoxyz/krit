use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    error::Error,
    fmt,
    sync::Arc,
};

use crate::{
    Analysis, BinaryOperator, Block, Builtin, EffectSet, Expression, ExpressionKind, FunctionType,
    MatchKind, Parameter, Program, RequirementSet, ResolvedName, Span, Statement, StatementKind,
    SymbolId, SymbolKind, Type, UnaryOperator, ValueLiteral, VariantFamily, VariantName,
};

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u32);

        impl $name {
            pub const fn as_u32(self) -> u32 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}{}", $prefix, self.0)
            }
        }
    };
}

id_type!(BindingId, "b");
id_type!(FunctionId, "f");
id_type!(BlockId, "bb");
id_type!(ValueId, "v");
id_type!(ParameterId, "p");
id_type!(CaptureId, "c");
id_type!(ClosureId, "cl");
id_type!(MatchBindingId, "m");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EntrypointKind {
    ModuleInit,
    Webhook,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreEntrypoint {
    pub kind: EntrypointKind,
    pub function: FunctionId,
}

impl EntrypointKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModuleInit => "module-init",
            Self::Webhook => "webhook",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingKind {
    Let,
    Function,
    Webhook,
    Parameter,
    Match,
}

impl BindingKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Let => "let",
            Self::Function => "function",
            Self::Webhook => "webhook",
            Self::Parameter => "parameter",
            Self::Match => "match",
        }
    }
}

impl From<SymbolKind> for BindingKind {
    fn from(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Let => Self::Let,
            SymbolKind::Function => Self::Function,
            SymbolKind::Webhook => Self::Webhook,
            SymbolKind::Parameter => Self::Parameter,
            SymbolKind::Match => Self::Match,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CoreBinding {
    pub id: BindingId,
    pub kind: BindingKind,
    pub ty: Arc<Type>,
    pub debug_name: Option<String>,
    pub source: Option<Span>,
    pub top_level: bool,
}

#[derive(Clone, Debug)]
pub struct FunctionSignature {
    pub parameters: Vec<Arc<Type>>,
    pub result: Arc<Type>,
    pub effects: EffectSet,
    pub requirements: RequirementSet,
}

impl FunctionSignature {
    fn from_type(ty: &Type) -> Result<Self, IrError> {
        let Type::Function(function) = ty else {
            return Err(IrError::new(format!(
                "expected function type, found `{ty}`"
            )));
        };
        Ok(Self {
            parameters: function.parameters().to_vec(),
            result: function.shared_return_type(),
            effects: function.effects().clone(),
            requirements: function.requirements().clone(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct CoreParameter {
    pub id: ParameterId,
    pub binding: BindingId,
    pub value: ValueId,
    pub ty: Arc<Type>,
    pub debug_name: Option<String>,
    pub source: Option<Span>,
}

#[derive(Clone, Debug)]
pub struct CoreCapture {
    pub id: CaptureId,
    pub binding: BindingId,
    pub value: ValueId,
    pub ty: Arc<Type>,
    pub debug_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RecursiveBinding {
    pub binding: BindingId,
    pub value: ValueId,
    pub ty: Arc<Type>,
}

#[derive(Clone, Debug)]
pub struct CoreFunction {
    pub id: FunctionId,
    pub debug_name: Option<String>,
    pub source: Option<Span>,
    pub signature: FunctionSignature,
    pub parameters: Vec<CoreParameter>,
    pub captures: Vec<CoreCapture>,
    pub recursive: Option<RecursiveBinding>,
    pub body: CoreBlock,
}

#[derive(Clone, Debug)]
pub struct BlockParameter {
    pub match_binding: MatchBindingId,
    pub binding: BindingId,
    pub value: ValueId,
    pub ty: Arc<Type>,
    pub debug_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CoreBlock {
    pub id: BlockId,
    pub parameters: Vec<BlockParameter>,
    pub operations: Vec<CoreOperation>,
    pub result: ValueId,
    pub ty: Arc<Type>,
    pub effects: EffectSet,
    pub requirements: RequirementSet,
    pub source: Option<Span>,
}

#[derive(Clone, Debug)]
pub struct CoreOperation {
    pub result: ValueId,
    pub ty: Arc<Type>,
    pub effects: EffectSet,
    pub requirements: RequirementSet,
    pub source: Option<Span>,
    pub kind: OperationKind,
}

#[derive(Clone, Debug)]
pub struct RecordOperand {
    pub name: String,
    pub value: ValueId,
}

#[derive(Clone, Debug)]
pub struct CaptureArgument {
    pub capture: CaptureId,
    pub binding: BindingId,
    pub value: ValueId,
}

#[derive(Clone, Debug)]
pub struct VariantArmBlock {
    pub variant: VariantName,
    pub binding: Option<MatchBindingId>,
    pub block: CoreBlock,
}

#[derive(Clone, Debug)]
pub enum OperationKind {
    Literal(ValueLiteral),
    Unit,
    Builtin(Builtin),
    Variant {
        variant: VariantName,
        payload: Option<ValueId>,
    },
    List(Vec<ValueId>),
    Record(Vec<RecordOperand>),
    Field {
        value: ValueId,
        field: String,
    },
    Block {
        block: CoreBlock,
    },
    Closure {
        closure: ClosureId,
        function: FunctionId,
        captures: Vec<CaptureArgument>,
    },
    Call {
        callee: ValueId,
        arguments: Vec<ValueId>,
    },
    If {
        condition: ValueId,
        consequent: CoreBlock,
        alternative: CoreBlock,
    },
    MatchList {
        subject: ValueId,
        empty: CoreBlock,
        head: MatchBindingId,
        tail: MatchBindingId,
        cons: CoreBlock,
    },
    MatchVariant {
        subject: ValueId,
        family: VariantFamily,
        arms: Vec<VariantArmBlock>,
    },
    Unary {
        operator: UnaryOperator,
        operand: ValueId,
    },
    Binary {
        left: ValueId,
        operator: BinaryOperator,
        right: ValueId,
    },
    Bind {
        binding: BindingId,
        value: ValueId,
    },
    Discard {
        value: ValueId,
    },
}

#[derive(Clone, Debug)]
pub struct CoreModule {
    entrypoints: Vec<CoreEntrypoint>,
    bindings: Vec<CoreBinding>,
    functions: Vec<CoreFunction>,
}

impl CoreModule {
    pub fn entrypoint(&self) -> FunctionId {
        self.entrypoints[0].function
    }

    pub fn entrypoints(&self) -> &[CoreEntrypoint] {
        &self.entrypoints
    }

    pub fn bindings(&self) -> &[CoreBinding] {
        &self.bindings
    }

    pub fn functions(&self) -> &[CoreFunction] {
        &self.functions
    }

    pub fn entrypoint_function(&self) -> &CoreFunction {
        &self.functions[self.entrypoint().0 as usize]
    }

    pub fn verify(&self) -> Result<(), IrError> {
        Verifier::new(self).verify()
    }

    /// Returns whether any Core boundary or operation still contains an
    /// inference variable that must be specialized before choosing a layout.
    pub fn has_residual_types(&self) -> bool {
        let mut visited = HashSet::new();
        self.bindings
            .iter()
            .any(|binding| type_has_residual(binding.ty.as_ref(), &mut visited))
            || self
                .functions
                .iter()
                .any(|function| function_has_residual(function, &mut visited))
    }

    pub fn render_text(&self) -> String {
        let mut output = String::new();
        output.push_str("core module\n");
        for entrypoint in &self.entrypoints {
            let function = &self.functions[entrypoint.function.0 as usize];
            output.push_str(&format!(
                "entry {} {}",
                function.id,
                entrypoint.kind.as_str()
            ));
            if entrypoint.kind == EntrypointKind::Webhook {
                output.push_str(&format!(
                    " {}",
                    function.debug_name.as_deref().unwrap_or("<anonymous>")
                ));
            }
            output.push_str(&format!(" effects {}", function.signature.effects));
            if !function.signature.requirements.is_empty() {
                output.push_str(&format!(
                    " requirements {}",
                    function.signature.requirements
                ));
            }
            output.push('\n');
        }
        if !self.bindings.is_empty() {
            output.push_str("bindings\n");
            for binding in &self.bindings {
                output.push_str(&format!(
                    "  {} {}{}: {}\n",
                    binding.id,
                    binding.kind.as_str(),
                    binding
                        .debug_name
                        .as_deref()
                        .map_or_else(String::new, |name| format!(" {name}")),
                    binding.ty
                ));
            }
        }
        for function in &self.functions {
            render_function(&mut output, function);
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrError {
    message: String,
}

impl IrError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for IrError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for IrError {}

pub fn lower(program: &Program, analysis: &Analysis) -> Result<CoreModule, IrError> {
    let module = Lowerer::new(analysis)?.program(program)?;
    module.verify()?;
    Ok(module)
}

#[derive(Clone)]
struct CapturePlan {
    capture: CoreCapture,
    source: ValueId,
}

struct FunctionContext {
    scopes: Vec<BTreeMap<BindingId, ValueId>>,
    captures: Vec<CapturePlan>,
    capture_by_binding: BTreeMap<BindingId, usize>,
}

impl FunctionContext {
    fn new() -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
            captures: Vec::new(),
            capture_by_binding: BTreeMap::new(),
        }
    }

    fn lookup(&self, binding: BindingId) -> Option<ValueId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&binding).copied())
    }

    fn define(&mut self, binding: BindingId, value: ValueId) {
        self.scopes
            .last_mut()
            .expect("function context always has a scope")
            .insert(binding, value);
    }
}

struct BlockFacts {
    ty: Arc<Type>,
    effects: EffectSet,
    requirements: RequirementSet,
}

struct Lowerer<'a> {
    analysis: &'a Analysis,
    bindings: Vec<CoreBinding>,
    symbol_bindings: BTreeMap<SymbolId, BindingId>,
    functions: Vec<Option<CoreFunction>>,
    contexts: Vec<FunctionContext>,
    next_block: u32,
    next_value: u32,
    next_parameter: u32,
    next_capture: u32,
    next_closure: u32,
    next_match_binding: u32,
    webhook_entrypoint: Option<CoreEntrypoint>,
}

impl<'a> Lowerer<'a> {
    fn new(analysis: &'a Analysis) -> Result<Self, IrError> {
        let mut bindings = Vec::with_capacity(analysis.symbols().len());
        let mut symbol_bindings = BTreeMap::new();
        for (index, symbol) in analysis.symbols().iter().enumerate() {
            if symbol.id().as_u32() as usize != index {
                return Err(IrError::new("analysis symbol IDs are not contiguous"));
            }
            let id = BindingId(symbol.id().as_u32());
            symbol_bindings.insert(symbol.id(), id);
            bindings.push(CoreBinding {
                id,
                kind: symbol.kind().into(),
                ty: symbol.shared_type(),
                debug_name: Some(symbol.name().to_owned()),
                source: Some(symbol.span()),
                top_level: symbol.is_top_level(),
            });
        }
        Ok(Self {
            analysis,
            bindings,
            symbol_bindings,
            functions: Vec::new(),
            contexts: Vec::new(),
            next_block: 0,
            next_value: 0,
            next_parameter: 0,
            next_capture: 0,
            next_closure: 0,
            next_match_binding: 0,
            webhook_entrypoint: None,
        })
    }

    fn program(mut self, program: &Program) -> Result<CoreModule, IrError> {
        let entrypoint = self.allocate_function();
        debug_assert_eq!(entrypoint, FunctionId(0));
        self.contexts.push(FunctionContext::new());
        let block_id = self.allocate_block();
        let body = self.lower_statements_block(
            block_id,
            &program.statements,
            None,
            BlockFacts {
                ty: Arc::new(Type::Unit),
                effects: self.analysis.effects().clone(),
                requirements: self.analysis.requirements().clone(),
            },
            None,
        )?;
        let context = self
            .contexts
            .pop()
            .expect("module-init context should exist");
        if !context.captures.is_empty() {
            return Err(IrError::new("module-init cannot have captures"));
        }
        let function = CoreFunction {
            id: entrypoint,
            debug_name: Some("$module_init".to_owned()),
            source: None,
            signature: FunctionSignature {
                parameters: Vec::new(),
                result: Arc::new(Type::Unit),
                effects: self.analysis.effects().clone(),
                requirements: self.analysis.requirements().clone(),
            },
            parameters: Vec::new(),
            captures: Vec::new(),
            recursive: None,
            body,
        };
        self.set_function(function)?;

        let mut entrypoints = vec![CoreEntrypoint {
            kind: EntrypointKind::ModuleInit,
            function: entrypoint,
        }];
        if let Some(webhook) = self.webhook_entrypoint.take() {
            entrypoints.push(webhook);
        }
        let functions = self
            .functions
            .into_iter()
            .enumerate()
            .map(|(index, function)| {
                function.ok_or_else(|| IrError::new(format!("function f{index} was not lowered")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CoreModule {
            entrypoints,
            bindings: self.bindings,
            functions,
        })
    }

    fn lower_statements_block(
        &mut self,
        id: BlockId,
        statements: &[Statement],
        tail: Option<&Expression>,
        facts: BlockFacts,
        source: Option<Span>,
    ) -> Result<CoreBlock, IrError> {
        self.push_scope();
        let mut operations = Vec::new();
        for statement in statements {
            self.lower_statement(statement, &mut operations)?;
        }
        let result = if let Some(tail) = tail {
            self.lower_expression(tail, &mut operations)?
        } else {
            self.emit(
                &mut operations,
                Arc::new(Type::Unit),
                EffectSet::default(),
                source,
                OperationKind::Unit,
            )
        };
        self.pop_scope();
        Ok(CoreBlock {
            id,
            parameters: Vec::new(),
            operations,
            result,
            ty: facts.ty,
            effects: facts.effects,
            requirements: facts.requirements,
            source,
        })
    }

    fn lower_block(&mut self, block: &Block) -> Result<CoreBlock, IrError> {
        let fact = self
            .analysis
            .block(block.span)
            .ok_or_else(|| IrError::new(format!("missing analysis for block {:?}", block.span)))?;
        let id = self.allocate_block();
        self.lower_statements_block(
            id,
            &block.statements,
            block.tail.as_deref(),
            BlockFacts {
                ty: fact.shared_type(),
                effects: fact.effects().clone(),
                requirements: fact.requirements().clone(),
            },
            Some(block.span),
        )
    }

    fn lower_expression_block(
        &mut self,
        expression: &Expression,
        parameters: Vec<BlockParameter>,
    ) -> Result<CoreBlock, IrError> {
        let fact = self.expression_fact(expression)?;
        let ty = fact.shared_type();
        let effects = fact.effects().clone();
        let requirements = fact.requirements().clone();
        let id = self.allocate_block();
        self.push_scope();
        for parameter in &parameters {
            self.current_context_mut()
                .define(parameter.binding, parameter.value);
        }
        let mut operations = Vec::new();
        let result = self.lower_expression(expression, &mut operations)?;
        self.pop_scope();
        Ok(CoreBlock {
            id,
            parameters,
            operations,
            result,
            ty,
            effects,
            requirements,
            source: Some(expression.span),
        })
    }

    fn lower_statement(
        &mut self,
        statement: &Statement,
        operations: &mut Vec<CoreOperation>,
    ) -> Result<(), IrError> {
        match &statement.kind {
            StatementKind::Let { name, value, .. } => {
                let value = self.lower_expression(value, operations)?;
                let binding = self.declaration_binding(statement.span, name, SymbolKind::Let)?;
                self.emit(
                    operations,
                    Arc::new(Type::Unit),
                    EffectSet::default(),
                    Some(statement.span),
                    OperationKind::Bind { binding, value },
                );
                self.current_context_mut().define(binding, value);
            }
            StatementKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let binding =
                    self.declaration_binding(statement.span, name, SymbolKind::Function)?;
                let ty = self.binding_type(binding)?;
                let (value, _) = self.lower_function(
                    Some((binding, name.as_str())),
                    parameters,
                    body,
                    ty,
                    Some(statement.span),
                    operations,
                )?;
                self.emit(
                    operations,
                    Arc::new(Type::Unit),
                    EffectSet::default(),
                    Some(statement.span),
                    OperationKind::Bind { binding, value },
                );
                self.current_context_mut().define(binding, value);
            }
            StatementKind::Webhook {
                name,
                parameters,
                body,
                ..
            } => {
                let binding =
                    self.declaration_binding(statement.span, name, SymbolKind::Webhook)?;
                let ty = self.binding_type(binding)?;
                let (value, function) = self.lower_function(
                    Some((binding, name.as_str())),
                    parameters,
                    body,
                    ty,
                    Some(statement.span),
                    operations,
                )?;
                self.emit(
                    operations,
                    Arc::new(Type::Unit),
                    EffectSet::default(),
                    Some(statement.span),
                    OperationKind::Bind { binding, value },
                );
                self.current_context_mut().define(binding, value);
                if self
                    .webhook_entrypoint
                    .replace(CoreEntrypoint {
                        kind: EntrypointKind::Webhook,
                        function,
                    })
                    .is_some()
                {
                    return Err(IrError::new(
                        "multiple webhook entrypoints reached Core lowering",
                    ));
                }
            }
            StatementKind::Expression(expression) => {
                let value = self.lower_expression(expression, operations)?;
                self.emit(
                    operations,
                    Arc::new(Type::Unit),
                    EffectSet::default(),
                    Some(statement.span),
                    OperationKind::Discard { value },
                );
            }
        }
        Ok(())
    }

    fn lower_expression(
        &mut self,
        expression: &Expression,
        operations: &mut Vec<CoreOperation>,
    ) -> Result<ValueId, IrError> {
        let fact = self.expression_fact(expression)?;
        let ty = fact.shared_type();
        let expression_requirements = fact.requirements().clone();
        let source = Some(expression.span);
        match &expression.kind {
            ExpressionKind::Literal(literal) => Ok(self.emit(
                operations,
                ty,
                EffectSet::default(),
                source,
                OperationKind::Literal(literal.clone()),
            )),
            ExpressionKind::Variable(_) => {
                let resolution = fact.resolved_name().ok_or_else(|| {
                    IrError::new(format!(
                        "variable expression {:?} has no resolved name",
                        expression.span
                    ))
                })?;
                match resolution {
                    ResolvedName::Symbol(symbol) => {
                        let binding = self.binding_for_symbol(symbol)?;
                        self.resolve_binding(binding)
                    }
                    ResolvedName::Builtin(Builtin::None) => Ok(self.emit(
                        operations,
                        ty,
                        EffectSet::default(),
                        source,
                        OperationKind::Variant {
                            variant: VariantName::None,
                            payload: None,
                        },
                    )),
                    ResolvedName::Builtin(builtin) => Ok(self.emit(
                        operations,
                        ty,
                        EffectSet::default(),
                        source,
                        OperationKind::Builtin(builtin),
                    )),
                }
            }
            ExpressionKind::List(elements) => {
                let values = elements
                    .iter()
                    .map(|element| self.lower_expression(element, operations))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.emit(
                    operations,
                    ty,
                    EffectSet::default(),
                    source,
                    OperationKind::List(values),
                ))
            }
            ExpressionKind::Record(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        self.lower_expression(&field.value, operations)
                            .map(|value| RecordOperand {
                                name: field.name.clone(),
                                value,
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.emit(
                    operations,
                    ty,
                    EffectSet::default(),
                    source,
                    OperationKind::Record(fields),
                ))
            }
            ExpressionKind::FieldAccess { value, field } => {
                let value = self.lower_expression(value, operations)?;
                Ok(self.emit(
                    operations,
                    ty,
                    EffectSet::default(),
                    source,
                    OperationKind::Field {
                        value,
                        field: field.clone(),
                    },
                ))
            }
            ExpressionKind::Block(block) => {
                let block = self.lower_block(block)?;
                let effects = block.effects.clone();
                let requirements = block.requirements.clone();
                Ok(self.emit_with_requirements(
                    operations,
                    ty,
                    effects,
                    requirements,
                    source,
                    OperationKind::Block { block },
                ))
            }
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                let condition = self.lower_expression(condition, operations)?;
                let consequent = self.lower_block(consequent)?;
                let alternative = self.lower_expression_block(alternative, Vec::new())?;
                let effects = EffectSet::union([&consequent.effects, &alternative.effects]);
                let requirements =
                    RequirementSet::union([&consequent.requirements, &alternative.requirements]);
                Ok(self.emit_with_requirements(
                    operations,
                    ty,
                    effects,
                    requirements,
                    source,
                    OperationKind::If {
                        condition,
                        consequent,
                        alternative,
                    },
                ))
            }
            ExpressionKind::Function {
                parameters, body, ..
            } => self
                .lower_function(None, parameters, body, ty, source, operations)
                .map(|(value, _)| value),
            ExpressionKind::Call { callee, arguments } => {
                let callee_value = self.lower_expression(callee, operations)?;
                let arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument, operations))
                    .collect::<Result<Vec<_>, _>>()?;
                let callee_type = self.expression_fact(callee)?.ty();
                let Type::Function(function_type) = callee_type else {
                    return Err(IrError::new(format!(
                        "analyzed call target has non-function type `{callee_type}`"
                    )));
                };
                Ok(self.emit_with_requirements(
                    operations,
                    ty,
                    function_type.effects().clone(),
                    expression_requirements,
                    source,
                    OperationKind::Call {
                        callee: callee_value,
                        arguments,
                    },
                ))
            }
            ExpressionKind::Match { subject, kind } => {
                let subject_value = self.lower_expression(subject, operations)?;
                match kind {
                    MatchKind::List {
                        empty_case,
                        head_name,
                        tail_name,
                        cons_case,
                    } => {
                        let empty = self.lower_expression_block(empty_case, Vec::new())?;
                        let head_binding = self.declaration_binding(
                            expression.span,
                            head_name,
                            SymbolKind::Match,
                        )?;
                        let tail_binding = self.declaration_binding(
                            expression.span,
                            tail_name,
                            SymbolKind::Match,
                        )?;
                        let head = self.block_parameter(head_binding)?;
                        let tail = self.block_parameter(tail_binding)?;
                        let head_id = head.match_binding;
                        let tail_id = tail.match_binding;
                        let cons = self.lower_expression_block(cons_case, vec![head, tail])?;
                        let effects = EffectSet::union([&empty.effects, &cons.effects]);
                        let requirements =
                            RequirementSet::union([&empty.requirements, &cons.requirements]);
                        Ok(self.emit_with_requirements(
                            operations,
                            ty,
                            effects,
                            requirements,
                            source,
                            OperationKind::MatchList {
                                subject: subject_value,
                                empty,
                                head: head_id,
                                tail: tail_id,
                                cons,
                            },
                        ))
                    }
                    MatchKind::Variants { family, arms } => {
                        let mut lowered_arms = Vec::with_capacity(arms.len());
                        for arm in arms {
                            let (binding, parameters) = if let Some(name) = &arm.binding {
                                let binding =
                                    self.declaration_binding(arm.span, name, SymbolKind::Match)?;
                                let parameter = self.block_parameter(binding)?;
                                (Some(parameter.match_binding), vec![parameter])
                            } else {
                                (None, Vec::new())
                            };
                            lowered_arms.push(VariantArmBlock {
                                variant: arm.variant,
                                binding,
                                block: self.lower_expression_block(&arm.value, parameters)?,
                            });
                        }
                        let effects =
                            EffectSet::union(lowered_arms.iter().map(|arm| &arm.block.effects));
                        let requirements = RequirementSet::union(
                            lowered_arms.iter().map(|arm| &arm.block.requirements),
                        );
                        Ok(self.emit_with_requirements(
                            operations,
                            ty,
                            effects,
                            requirements,
                            source,
                            OperationKind::MatchVariant {
                                subject: subject_value,
                                family: *family,
                                arms: lowered_arms,
                            },
                        ))
                    }
                }
            }
            ExpressionKind::Unary { operator, operand } => {
                let operand = self.lower_expression(operand, operations)?;
                Ok(self.emit(
                    operations,
                    ty,
                    EffectSet::default(),
                    source,
                    OperationKind::Unary {
                        operator: *operator,
                        operand,
                    },
                ))
            }
            ExpressionKind::Binary {
                left,
                operator: BinaryOperator::And,
                right,
            } => self.lower_short_circuit(expression, left, right, false, operations),
            ExpressionKind::Binary {
                left,
                operator: BinaryOperator::Or,
                right,
            } => self.lower_short_circuit(expression, left, right, true, operations),
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.lower_expression(left, operations)?;
                let right = self.lower_expression(right, operations)?;
                Ok(self.emit(
                    operations,
                    ty,
                    EffectSet::default(),
                    source,
                    OperationKind::Binary {
                        left,
                        operator: *operator,
                        right,
                    },
                ))
            }
        }
    }

    fn lower_short_circuit(
        &mut self,
        expression: &Expression,
        left: &Expression,
        right: &Expression,
        short_circuit_value: bool,
        operations: &mut Vec<CoreOperation>,
    ) -> Result<ValueId, IrError> {
        let condition = self.lower_expression(left, operations)?;
        let right_block = self.lower_expression_block(right, Vec::new())?;
        let constant_block = self.boolean_block(short_circuit_value, expression.span);
        let (consequent, alternative) = if short_circuit_value {
            (constant_block, right_block)
        } else {
            (right_block, constant_block)
        };
        let effects = EffectSet::union([&consequent.effects, &alternative.effects]);
        let requirements =
            RequirementSet::union([&consequent.requirements, &alternative.requirements]);
        let ty = self.expression_fact(expression)?.shared_type();
        Ok(self.emit_with_requirements(
            operations,
            ty,
            effects,
            requirements,
            Some(expression.span),
            OperationKind::If {
                condition,
                consequent,
                alternative,
            },
        ))
    }

    fn boolean_block(&mut self, value: bool, span: Span) -> CoreBlock {
        let id = self.allocate_block();
        let mut operations = Vec::new();
        let ty = Arc::new(Type::Bool);
        let result = self.emit(
            &mut operations,
            Arc::clone(&ty),
            EffectSet::default(),
            Some(span),
            OperationKind::Literal(ValueLiteral::Boolean(value)),
        );
        CoreBlock {
            id,
            parameters: Vec::new(),
            operations,
            result,
            ty,
            effects: EffectSet::default(),
            requirements: RequirementSet::default(),
            source: Some(span),
        }
    }

    fn lower_function(
        &mut self,
        named: Option<(BindingId, &str)>,
        parameters: &[Parameter],
        body: &Block,
        ty: Arc<Type>,
        source: Option<Span>,
        operations: &mut Vec<CoreOperation>,
    ) -> Result<(ValueId, FunctionId), IrError> {
        let signature = FunctionSignature::from_type(ty.as_ref())?;
        if signature.parameters.len() != parameters.len() {
            return Err(IrError::new(
                "analysis function parameter count does not match AST",
            ));
        }
        let function_id = self.allocate_function();
        let closure_id = self.allocate_closure();
        self.contexts.push(FunctionContext::new());

        let recursive = if let Some((binding, _)) = named {
            let value = self.allocate_value();
            let recursive = RecursiveBinding {
                binding,
                value,
                ty: Arc::clone(&ty),
            };
            self.current_context_mut().define(binding, value);
            Some(recursive)
        } else {
            None
        };

        let mut core_parameters = Vec::with_capacity(parameters.len());
        for (parameter, parameter_ty) in parameters.iter().zip(&signature.parameters) {
            let binding =
                self.declaration_binding(parameter.span, &parameter.name, SymbolKind::Parameter)?;
            let value = self.allocate_value();
            let core_parameter = CoreParameter {
                id: self.allocate_parameter(),
                binding,
                value,
                ty: Arc::clone(parameter_ty),
                debug_name: Some(parameter.name.clone()),
                source: Some(parameter.span),
            };
            self.current_context_mut().define(binding, value);
            core_parameters.push(core_parameter);
        }

        let core_body = self.lower_block(body)?;
        let context = self
            .contexts
            .pop()
            .expect("function context should exist after lowering");
        let captures = context
            .captures
            .iter()
            .map(|capture| capture.capture.clone())
            .collect::<Vec<_>>();
        let capture_arguments = context
            .captures
            .iter()
            .map(|capture| CaptureArgument {
                capture: capture.capture.id,
                binding: capture.capture.binding,
                value: capture.source,
            })
            .collect::<Vec<_>>();
        let function = CoreFunction {
            id: function_id,
            debug_name: named.map(|(_, name)| name.to_owned()),
            source,
            signature,
            parameters: core_parameters,
            captures,
            recursive,
            body: core_body,
        };
        self.set_function(function)?;
        let value = self.emit(
            operations,
            ty,
            EffectSet::default(),
            source,
            OperationKind::Closure {
                closure: closure_id,
                function: function_id,
                captures: capture_arguments,
            },
        );
        Ok((value, function_id))
    }

    fn block_parameter(&mut self, binding: BindingId) -> Result<BlockParameter, IrError> {
        let metadata = self.binding(binding)?;
        let ty = Arc::clone(&metadata.ty);
        let debug_name = metadata.debug_name.clone();
        Ok(BlockParameter {
            match_binding: self.allocate_match_binding(),
            binding,
            value: self.allocate_value(),
            ty,
            debug_name,
        })
    }

    fn resolve_binding(&mut self, binding: BindingId) -> Result<ValueId, IrError> {
        let current = self
            .contexts
            .len()
            .checked_sub(1)
            .ok_or_else(|| IrError::new("binding resolution outside a function"))?;
        if let Some(value) = self.contexts[current].lookup(binding) {
            return Ok(value);
        }
        let ancestor = (0..current)
            .rev()
            .find(|index| self.contexts[*index].lookup(binding).is_some())
            .ok_or_else(|| IrError::new(format!("resolved binding {binding} is unavailable")))?;
        let mut source = self.contexts[ancestor]
            .lookup(binding)
            .expect("ancestor was selected because the binding is available");
        for index in ancestor + 1..=current {
            if let Some(capture_index) = self.contexts[index]
                .capture_by_binding
                .get(&binding)
                .copied()
            {
                source = self.contexts[index].captures[capture_index].capture.value;
                continue;
            }
            let metadata = self.binding(binding)?;
            let ty = Arc::clone(&metadata.ty);
            let debug_name = metadata.debug_name.clone();
            let capture = CoreCapture {
                id: self.allocate_capture(),
                binding,
                value: self.allocate_value(),
                ty,
                debug_name,
            };
            let value = capture.value;
            let context = &mut self.contexts[index];
            let capture_index = context.captures.len();
            context.capture_by_binding.insert(binding, capture_index);
            context.captures.push(CapturePlan { capture, source });
            context.scopes[0].insert(binding, value);
            source = value;
        }
        Ok(source)
    }

    fn expression_fact(
        &self,
        expression: &Expression,
    ) -> Result<&crate::ExpressionAnalysis, IrError> {
        self.analysis.expression(expression.span).ok_or_else(|| {
            IrError::new(format!(
                "missing typed expression fact for {:?}",
                expression.span
            ))
        })
    }

    fn declaration_binding(
        &self,
        span: Span,
        name: &str,
        kind: SymbolKind,
    ) -> Result<BindingId, IrError> {
        let symbol = self.analysis.symbol(span, name, kind).ok_or_else(|| {
            IrError::new(format!(
                "missing resolved {kind:?} declaration `{name}` at {span:?}"
            ))
        })?;
        self.binding_for_symbol(symbol.id())
    }

    fn binding_for_symbol(&self, symbol: SymbolId) -> Result<BindingId, IrError> {
        self.symbol_bindings
            .get(&symbol)
            .copied()
            .ok_or_else(|| IrError::new(format!("unknown analysis symbol {}", symbol.as_u32())))
    }

    fn binding(&self, binding: BindingId) -> Result<&CoreBinding, IrError> {
        self.bindings
            .get(binding.0 as usize)
            .filter(|metadata| metadata.id == binding)
            .ok_or_else(|| IrError::new(format!("unknown binding {binding}")))
    }

    fn binding_type(&self, binding: BindingId) -> Result<Arc<Type>, IrError> {
        self.binding(binding).map(|binding| Arc::clone(&binding.ty))
    }

    fn current_context_mut(&mut self) -> &mut FunctionContext {
        self.contexts
            .last_mut()
            .expect("lowering always occurs inside a function")
    }

    fn push_scope(&mut self) {
        self.current_context_mut().scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.current_context_mut().scopes.pop();
    }

    fn emit(
        &mut self,
        operations: &mut Vec<CoreOperation>,
        ty: Arc<Type>,
        effects: EffectSet,
        source: Option<Span>,
        kind: OperationKind,
    ) -> ValueId {
        self.emit_with_requirements(
            operations,
            ty,
            effects,
            RequirementSet::default(),
            source,
            kind,
        )
    }

    fn emit_with_requirements(
        &mut self,
        operations: &mut Vec<CoreOperation>,
        ty: Arc<Type>,
        effects: EffectSet,
        requirements: RequirementSet,
        source: Option<Span>,
        kind: OperationKind,
    ) -> ValueId {
        let result = self.allocate_value();
        operations.push(CoreOperation {
            result,
            ty,
            effects,
            requirements,
            source,
            kind,
        });
        result
    }

    fn allocate_function(&mut self) -> FunctionId {
        let id = FunctionId(self.functions.len() as u32);
        self.functions.push(None);
        id
    }

    fn set_function(&mut self, function: CoreFunction) -> Result<(), IrError> {
        let slot = self
            .functions
            .get_mut(function.id.0 as usize)
            .ok_or_else(|| IrError::new(format!("unknown function {}", function.id)))?;
        if slot.is_some() {
            return Err(IrError::new(format!(
                "function {} was lowered twice",
                function.id
            )));
        }
        *slot = Some(function);
        Ok(())
    }

    fn allocate_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        id
    }

    fn allocate_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    fn allocate_parameter(&mut self) -> ParameterId {
        let id = ParameterId(self.next_parameter);
        self.next_parameter += 1;
        id
    }

    fn allocate_capture(&mut self) -> CaptureId {
        let id = CaptureId(self.next_capture);
        self.next_capture += 1;
        id
    }

    fn allocate_closure(&mut self) -> ClosureId {
        let id = ClosureId(self.next_closure);
        self.next_closure += 1;
        id
    }

    fn allocate_match_binding(&mut self) -> MatchBindingId {
        let id = MatchBindingId(self.next_match_binding);
        self.next_match_binding += 1;
        id
    }
}

struct Verifier<'a> {
    module: &'a CoreModule,
    value_types: BTreeMap<ValueId, Arc<Type>>,
    builtin_values: BTreeMap<ValueId, Builtin>,
    string_literals: BTreeMap<ValueId, String>,
    block_ids: BTreeSet<BlockId>,
    parameter_ids: BTreeSet<ParameterId>,
    capture_ids: BTreeSet<CaptureId>,
    closure_ids: BTreeSet<ClosureId>,
    match_binding_ids: BTreeSet<MatchBindingId>,
}

impl<'a> Verifier<'a> {
    fn new(module: &'a CoreModule) -> Self {
        Self {
            module,
            value_types: BTreeMap::new(),
            builtin_values: BTreeMap::new(),
            string_literals: BTreeMap::new(),
            block_ids: BTreeSet::new(),
            parameter_ids: BTreeSet::new(),
            capture_ids: BTreeSet::new(),
            closure_ids: BTreeSet::new(),
            match_binding_ids: BTreeSet::new(),
        }
    }

    fn verify(mut self) -> Result<(), IrError> {
        self.verify_contiguous_bindings()?;
        if self.module.entrypoints.is_empty()
            || self.module.entrypoints.len() > 2
            || self.module.entrypoints[0].kind != EntrypointKind::ModuleInit
            || self.module.entrypoints[0].function != FunctionId(0)
            || self.module.entrypoints[0].function.0 as usize >= self.module.functions.len()
        {
            return Err(IrError::new("invalid module-init entrypoint identity"));
        }
        if let Some(webhook) = self.module.entrypoints.get(1)
            && (webhook.kind != EntrypointKind::Webhook
                || webhook.function == FunctionId(0)
                || webhook.function.0 as usize >= self.module.functions.len()
                || self.module.functions[webhook.function.0 as usize]
                    .debug_name
                    .as_deref()
                    .is_none_or(str::is_empty))
        {
            return Err(IrError::new("invalid webhook entrypoint identity"));
        }
        for (index, function) in self.module.functions.iter().enumerate() {
            if function.id.0 as usize != index {
                return Err(IrError::new("function IDs are not contiguous"));
            }
            self.collect_function(function)?;
        }
        verify_contiguous("block", &self.block_ids)?;
        verify_contiguous("value", &self.value_types.keys().copied().collect())?;
        verify_contiguous("parameter", &self.parameter_ids)?;
        verify_contiguous("capture", &self.capture_ids)?;
        verify_contiguous("closure", &self.closure_ids)?;
        verify_contiguous("match binding", &self.match_binding_ids)?;

        let entrypoint = self.module.entrypoint_function();
        if !entrypoint.parameters.is_empty()
            || !entrypoint.captures.is_empty()
            || entrypoint.recursive.is_some()
            || *entrypoint.signature.result != Type::Unit
        {
            return Err(IrError::new("invalid module-init entrypoint boundary"));
        }
        if let Some(webhook) = self.module.entrypoints.get(1) {
            let function = self.function(webhook.function)?;
            if function.signature.parameters.len() != 1
                || function.signature.parameters[0].as_ref() != &Type::HttpRequest
                || function.signature.result.as_ref() != &Type::HttpResponse
            {
                return Err(IrError::new("invalid webhook entrypoint boundary"));
            }
        }
        for function in &self.module.functions {
            self.verify_function(function)?;
        }
        Ok(())
    }

    fn verify_contiguous_bindings(&self) -> Result<(), IrError> {
        for (index, binding) in self.module.bindings.iter().enumerate() {
            if binding.id.0 as usize != index {
                return Err(IrError::new("binding IDs are not contiguous"));
            }
        }
        Ok(())
    }

    fn collect_function(&mut self, function: &CoreFunction) -> Result<(), IrError> {
        if function.parameters.len() != function.signature.parameters.len() {
            return Err(IrError::new(format!(
                "{} parameter count does not match its signature",
                function.id
            )));
        }
        for (parameter, signature_type) in function
            .parameters
            .iter()
            .zip(&function.signature.parameters)
        {
            insert_unique(&mut self.parameter_ids, parameter.id, "parameter")?;
            self.insert_value(parameter.value, Arc::clone(&parameter.ty))?;
            if parameter.ty.as_ref() != signature_type.as_ref() {
                return Err(IrError::new(format!(
                    "{} has a parameter type inconsistent with its signature",
                    function.id
                )));
            }
            self.binding_type_matches(parameter.binding, &parameter.ty)?;
            self.expect_binding_kind(parameter.binding, BindingKind::Parameter)?;
        }
        for capture in &function.captures {
            insert_unique(&mut self.capture_ids, capture.id, "capture")?;
            self.insert_value(capture.value, Arc::clone(&capture.ty))?;
            self.binding_type_matches(capture.binding, &capture.ty)?;
        }
        if let Some(recursive) = &function.recursive {
            self.insert_value(recursive.value, Arc::clone(&recursive.ty))?;
            self.binding_type_matches(recursive.binding, &recursive.ty)?;
            let binding = self.binding(recursive.binding)?;
            if !matches!(binding.kind, BindingKind::Function | BindingKind::Webhook) {
                return Err(IrError::new(format!(
                    "{} recursive binding has invalid kind {}",
                    function.id,
                    binding.kind.as_str()
                )));
            }
            let expected = function_type(&function.signature);
            if recursive.ty.as_ref() != &expected {
                return Err(IrError::new(format!(
                    "{} recursive value has an inconsistent type",
                    function.id
                )));
            }
        }
        self.collect_block(&function.body)
    }

    fn collect_block(&mut self, block: &CoreBlock) -> Result<(), IrError> {
        insert_unique(&mut self.block_ids, block.id, "block")?;
        for parameter in &block.parameters {
            insert_unique(
                &mut self.match_binding_ids,
                parameter.match_binding,
                "match binding",
            )?;
            self.insert_value(parameter.value, Arc::clone(&parameter.ty))?;
            self.binding_type_matches(parameter.binding, &parameter.ty)?;
            self.expect_binding_kind(parameter.binding, BindingKind::Match)?;
        }
        for operation in &block.operations {
            self.insert_value(operation.result, Arc::clone(&operation.ty))?;
            match &operation.kind {
                OperationKind::Builtin(builtin) => {
                    self.builtin_values.insert(operation.result, *builtin);
                }
                OperationKind::Literal(ValueLiteral::String(value)) => {
                    self.string_literals
                        .insert(operation.result, value.to_owned());
                }
                _ => {}
            }
            match &operation.kind {
                OperationKind::Block { block } => self.collect_block(block)?,
                OperationKind::Closure { closure, .. } => {
                    insert_unique(&mut self.closure_ids, *closure, "closure")?;
                }
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

    fn insert_value(&mut self, id: ValueId, ty: Arc<Type>) -> Result<(), IrError> {
        if self.value_types.insert(id, ty).is_some() {
            Err(IrError::new(format!("duplicate value ID {id}")))
        } else {
            Ok(())
        }
    }

    fn binding_type_matches(&self, id: BindingId, ty: &Type) -> Result<(), IrError> {
        let binding = self.binding(id)?;
        if type_equivalent(binding.ty.as_ref(), ty) {
            Ok(())
        } else {
            Err(IrError::new(format!(
                "binding {id} type `{}` does not match boundary type `{ty}`",
                binding.ty
            )))
        }
    }

    fn expect_binding_kind(&self, id: BindingId, expected: BindingKind) -> Result<(), IrError> {
        let binding = self.binding(id)?;
        if binding.kind == expected {
            Ok(())
        } else {
            Err(IrError::new(format!(
                "binding {id} has kind {}, expected {}",
                binding.kind.as_str(),
                expected.as_str()
            )))
        }
    }

    fn binding(&self, id: BindingId) -> Result<&CoreBinding, IrError> {
        self.module
            .bindings
            .get(id.0 as usize)
            .filter(|binding| binding.id == id)
            .ok_or_else(|| IrError::new(format!("binding {id} is out of range")))
    }

    fn verify_function(&self, function: &CoreFunction) -> Result<(), IrError> {
        if !function
            .signature
            .effects
            .is_superset(&function.body.effects)
        {
            return Err(IrError::new(format!(
                "{} effect summary is not conservative",
                function.id
            )));
        }
        if !function
            .signature
            .requirements
            .is_superset(&function.body.requirements)
        {
            return Err(IrError::new(format!(
                "{} capability requirement summary is not conservative",
                function.id
            )));
        }
        if !type_equivalent(
            function.body.ty.as_ref(),
            function.signature.result.as_ref(),
        ) {
            return Err(IrError::new(format!(
                "{} body result type does not match its signature",
                function.id
            )));
        }
        let mut available = BTreeSet::new();
        let mut bindings = BTreeMap::new();
        available.extend(function.parameters.iter().map(|parameter| parameter.value));
        for parameter in &function.parameters {
            if bindings
                .insert(parameter.binding, parameter.value)
                .is_some()
            {
                return Err(IrError::new(format!(
                    "{} contains duplicate parameter bindings",
                    function.id
                )));
            }
        }
        available.extend(function.captures.iter().map(|capture| capture.value));
        for capture in &function.captures {
            if bindings.insert(capture.binding, capture.value).is_some() {
                return Err(IrError::new(format!(
                    "{} contains duplicate capture bindings",
                    function.id
                )));
            }
        }
        if let Some(recursive) = &function.recursive {
            available.insert(recursive.value);
            if bindings
                .insert(recursive.binding, recursive.value)
                .is_some()
            {
                return Err(IrError::new(format!(
                    "{} recursive binding conflicts with another boundary",
                    function.id
                )));
            }
        }
        self.verify_block(&function.body, &available, &bindings)
    }

    fn verify_block(
        &self,
        block: &CoreBlock,
        incoming: &BTreeSet<ValueId>,
        incoming_bindings: &BTreeMap<BindingId, ValueId>,
    ) -> Result<(), IrError> {
        let mut available = incoming.clone();
        let mut bindings = incoming_bindings.clone();
        available.extend(block.parameters.iter().map(|parameter| parameter.value));
        for parameter in &block.parameters {
            if bindings
                .insert(parameter.binding, parameter.value)
                .is_some()
            {
                return Err(IrError::new(format!(
                    "{} match binding conflicts with an available binding",
                    block.id
                )));
            }
        }
        let mut operation_effects = Vec::new();
        let mut operation_requirements = Vec::new();
        for operation in &block.operations {
            self.verify_operation(operation, &available, &bindings)?;
            operation_effects.push(&operation.effects);
            operation_requirements.push(&operation.requirements);
            available.insert(operation.result);
            if let OperationKind::Bind { binding, value } = &operation.kind
                && bindings.insert(*binding, *value).is_some()
            {
                return Err(IrError::new(format!(
                    "{} defines binding {binding} more than once",
                    block.id
                )));
            }
        }
        require_available(block.result, &available, block.id)?;
        self.expect_value_type(block.result, &block.ty)?;
        let required = EffectSet::union(operation_effects);
        if !block.effects.is_superset(&required) {
            return Err(IrError::new(format!(
                "{} effect summary {} does not cover {}",
                block.id, block.effects, required
            )));
        }
        let required = RequirementSet::union(operation_requirements);
        if !block.requirements.is_superset(&required) {
            return Err(IrError::new(format!(
                "{} capability requirement summary {} does not cover {}",
                block.id, block.requirements, required
            )));
        }
        Ok(())
    }

    fn verify_operation(
        &self,
        operation: &CoreOperation,
        available: &BTreeSet<ValueId>,
        bindings: &BTreeMap<BindingId, ValueId>,
    ) -> Result<(), IrError> {
        let pure = EffectSet::default();
        match &operation.kind {
            OperationKind::Literal(literal) => {
                let expected = match literal {
                    ValueLiteral::Integer(_) => Type::Int,
                    ValueLiteral::Boolean(_) => Type::Bool,
                    ValueLiteral::String(_) => Type::String,
                };
                expect_type(operation, &expected)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Unit => {
                expect_type(operation, &Type::Unit)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Builtin(builtin) => {
                self.verify_builtin_type(*builtin, &operation.ty)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Variant { variant, payload } => {
                if let Some(payload) = payload {
                    require_available(*payload, available, operation.result)?;
                    if type_contains_secret(self.value_type(*payload)?) {
                        return Err(IrError::new("opaque Secret cannot be placed in a variant"));
                    }
                }
                verify_variant_type(*variant, *payload, &operation.ty, &self.value_types)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::List(elements) => {
                let Type::List(element_type) = operation.ty.as_ref() else {
                    return Err(IrError::new(format!(
                        "{} list operation has non-list type",
                        operation.result
                    )));
                };
                for element in elements {
                    require_available(*element, available, operation.result)?;
                    if type_contains_secret(self.value_type(*element)?) {
                        return Err(IrError::new("opaque Secret cannot be placed in a list"));
                    }
                    self.expect_value_type(*element, element_type)?;
                }
                expect_effects(operation, &pure)?;
            }
            OperationKind::Record(fields) => {
                let Type::Record(record_types) = operation.ty.as_ref() else {
                    return Err(IrError::new(format!(
                        "{} record operation has non-record type",
                        operation.result
                    )));
                };
                if record_types
                    .iter()
                    .any(|field| type_contains_secret(field.ty()))
                {
                    return Err(IrError::new("opaque Secret cannot be placed in a record"));
                }
                if fields.len() != record_types.len() {
                    return Err(IrError::new(format!(
                        "{} record field count does not match its type",
                        operation.result
                    )));
                }
                let mut seen = BTreeSet::new();
                for field in fields {
                    require_available(field.value, available, operation.result)?;
                    if !seen.insert(field.name.as_str()) {
                        return Err(IrError::new(format!(
                            "{} contains duplicate record field `{}`",
                            operation.result, field.name
                        )));
                    }
                    let expected = record_types
                        .iter()
                        .find(|record_type| record_type.name() == field.name)
                        .ok_or_else(|| {
                            IrError::new(format!(
                                "{} field `{}` is absent from its record type",
                                operation.result, field.name
                            ))
                        })?;
                    self.expect_value_type(field.value, expected.ty())?;
                }
                expect_effects(operation, &pure)?;
            }
            OperationKind::Field { value, field } => {
                require_available(*value, available, operation.result)?;
                let expected =
                    record_field_type(self.value_type(*value)?, field).ok_or_else(|| {
                        IrError::new(format!(
                            "{} accesses missing field `{field}`",
                            operation.result
                        ))
                    })?;
                expect_type(operation, expected.as_ref())?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Block { block } => {
                self.verify_block(block, available, bindings)?;
                expect_type(operation, &block.ty)?;
                require_effects(operation, &block.effects)?;
            }
            OperationKind::Closure {
                function, captures, ..
            } => {
                let target = self.function(*function)?;
                let expected = function_type(&target.signature);
                expect_type(operation, &expected)?;
                if captures.len() != target.captures.len() {
                    return Err(IrError::new(format!(
                        "{} closure capture count does not match {}",
                        operation.result, function
                    )));
                }
                for (argument, capture) in captures.iter().zip(&target.captures) {
                    if argument.capture != capture.id || argument.binding != capture.binding {
                        return Err(IrError::new(format!(
                            "{} closure capture identities do not match {}",
                            operation.result, function
                        )));
                    }
                    if bindings.get(&argument.binding) != Some(&argument.value) {
                        return Err(IrError::new(format!(
                            "{} closure capture {} is not defined by binding {}",
                            operation.result, argument.capture, argument.binding
                        )));
                    }
                    require_available(argument.value, available, operation.result)?;
                    self.expect_value_type(argument.value, &capture.ty)?;
                }
                expect_effects(operation, &pure)?;
            }
            OperationKind::Call { callee, arguments } => {
                require_available(*callee, available, operation.result)?;
                let Type::Function(function) = self.value_type(*callee)? else {
                    return Err(IrError::new(format!(
                        "{} call target is not a function",
                        operation.result
                    )));
                };
                if function.parameters().len() != arguments.len() {
                    return Err(IrError::new(format!(
                        "{} call arity does not match its signature",
                        operation.result
                    )));
                }
                for (argument, parameter) in arguments.iter().zip(function.parameters()) {
                    require_available(*argument, available, operation.result)?;
                    self.expect_value_type(*argument, parameter)?;
                }
                expect_type(operation, function.return_type())?;
                require_effects(operation, function.effects())?;
            }
            OperationKind::If {
                condition,
                consequent,
                alternative,
            } => {
                require_available(*condition, available, operation.result)?;
                self.expect_value_type(*condition, &Type::Bool)?;
                self.verify_block(consequent, available, bindings)?;
                self.verify_block(alternative, available, bindings)?;
                if !type_equivalent(&consequent.ty, &alternative.ty)
                    || !type_equivalent(&consequent.ty, &operation.ty)
                {
                    return Err(IrError::new(format!(
                        "{} if branch result types are inconsistent",
                        operation.result
                    )));
                }
                let required = EffectSet::union([&consequent.effects, &alternative.effects]);
                require_effects(operation, &required)?;
            }
            OperationKind::MatchList {
                subject,
                empty,
                head,
                tail,
                cons,
            } => {
                require_available(*subject, available, operation.result)?;
                let Type::List(element) = self.value_type(*subject)? else {
                    return Err(IrError::new(format!(
                        "{} list match subject is not a list",
                        operation.result
                    )));
                };
                self.verify_block(empty, available, bindings)?;
                self.verify_block(cons, available, bindings)?;
                if !type_equivalent(&empty.ty, &cons.ty)
                    || !type_equivalent(&empty.ty, &operation.ty)
                {
                    return Err(IrError::new(format!(
                        "{} list match arm types are inconsistent",
                        operation.result
                    )));
                }
                if cons.parameters.len() != 2
                    || cons.parameters[0].match_binding != *head
                    || cons.parameters[1].match_binding != *tail
                    || cons.parameters[0].ty.as_ref() != element.as_ref()
                    || cons.parameters[1].ty.as_ref() != self.value_type(*subject)?
                {
                    return Err(IrError::new(format!(
                        "{} list match bindings are inconsistent",
                        operation.result
                    )));
                }
                let required = EffectSet::union([&empty.effects, &cons.effects]);
                require_effects(operation, &required)?;
            }
            OperationKind::MatchVariant {
                subject,
                family,
                arms,
            } => {
                require_available(*subject, available, operation.result)?;
                let payloads = variant_payload_types(self.value_type(*subject)?, *family)?;
                if arms.len() != 2 {
                    return Err(IrError::new(format!(
                        "{} variant match must contain two arms",
                        operation.result
                    )));
                }
                let mut seen = BTreeSet::new();
                let mut arm_effects = Vec::new();
                for arm in arms {
                    if !seen.insert(arm.variant) {
                        return Err(IrError::new(format!(
                            "{} has duplicate variant arms",
                            operation.result
                        )));
                    }
                    let expected_payload = payloads.get(&arm.variant).ok_or_else(|| {
                        IrError::new(format!(
                            "{} variant arm does not belong to its family",
                            operation.result
                        ))
                    })?;
                    match (expected_payload, arm.binding) {
                        (Some(expected), Some(binding)) => {
                            if arm.block.parameters.len() != 1
                                || arm.block.parameters[0].match_binding != binding
                                || arm.block.parameters[0].ty.as_ref() != expected.as_ref()
                            {
                                return Err(IrError::new(format!(
                                    "{} variant payload binding is inconsistent",
                                    operation.result
                                )));
                            }
                        }
                        (None, None) if arm.block.parameters.is_empty() => {}
                        _ => {
                            return Err(IrError::new(format!(
                                "{} variant arm binding shape is inconsistent",
                                operation.result
                            )));
                        }
                    }
                    self.verify_block(&arm.block, available, bindings)?;
                    if !type_equivalent(&arm.block.ty, &operation.ty) {
                        return Err(IrError::new(format!(
                            "{} variant arm result type is inconsistent",
                            operation.result
                        )));
                    }
                    arm_effects.push(&arm.block.effects);
                }
                require_effects(operation, &EffectSet::union(arm_effects))?;
            }
            OperationKind::Unary { operator, operand } => {
                require_available(*operand, available, operation.result)?;
                let expected = match operator {
                    UnaryOperator::Not => Type::Bool,
                    UnaryOperator::Negate => Type::Int,
                };
                self.expect_value_type(*operand, &expected)?;
                expect_type(operation, &expected)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Binary {
                left,
                operator,
                right,
            } => {
                require_available(*left, available, operation.result)?;
                require_available(*right, available, operation.result)?;
                self.verify_binary(operation, *left, *operator, *right)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Bind { binding, value } => {
                require_available(*value, available, operation.result)?;
                self.binding_type_matches(*binding, self.value_type(*value)?)?;
                if !matches!(
                    self.binding(*binding)?.kind,
                    BindingKind::Let | BindingKind::Function | BindingKind::Webhook
                ) {
                    return Err(IrError::new(format!(
                        "{} binds non-declaration {}",
                        operation.result, binding
                    )));
                }
                expect_type(operation, &Type::Unit)?;
                expect_effects(operation, &pure)?;
            }
            OperationKind::Discard { value } => {
                require_available(*value, available, operation.result)?;
                expect_type(operation, &Type::Unit)?;
                expect_effects(operation, &pure)?;
            }
        }
        self.verify_operation_requirements(operation)?;
        Ok(())
    }

    fn verify_operation_requirements(&self, operation: &CoreOperation) -> Result<(), IrError> {
        match &operation.kind {
            OperationKind::Block { block } => require_requirements(operation, &block.requirements),
            OperationKind::Call { callee, arguments } => {
                let Type::Function(function) = self.value_type(*callee)? else {
                    return Err(IrError::new(format!(
                        "{} call target is not a function",
                        operation.result
                    )));
                };
                let mut requirements = function.requirements().iter().cloned().collect::<Vec<_>>();
                if let Some(builtin) = self.builtin_values.get(callee)
                    && matches!(builtin, Builtin::ConfigString | Builtin::Secret)
                {
                    let argument = arguments.first().ok_or_else(|| {
                        IrError::new(format!(
                            "{} resource host call has no argument",
                            operation.result
                        ))
                    })?;
                    let resource = self.string_literals.get(argument).ok_or_else(|| {
                        IrError::new(format!(
                            "{} resource host call argument is not a string literal",
                            operation.result
                        ))
                    })?;
                    let capability = match builtin {
                        Builtin::ConfigString => crate::Effect::ConfigRead,
                        Builtin::Secret => crate::Effect::SecretRead,
                        _ => unreachable!("matched resource host built-ins"),
                    };
                    requirements.push(crate::CapabilityRequirement::new(
                        capability,
                        resource.clone(),
                    ));
                }
                require_requirements(operation, &RequirementSet::from_requirements(requirements))
            }
            OperationKind::If {
                consequent,
                alternative,
                ..
            } => require_requirements(
                operation,
                &RequirementSet::union([&consequent.requirements, &alternative.requirements]),
            ),
            OperationKind::MatchList { empty, cons, .. } => require_requirements(
                operation,
                &RequirementSet::union([&empty.requirements, &cons.requirements]),
            ),
            OperationKind::MatchVariant { arms, .. } => require_requirements(
                operation,
                &RequirementSet::union(arms.iter().map(|arm| &arm.block.requirements)),
            ),
            OperationKind::Literal(_)
            | OperationKind::Unit
            | OperationKind::Builtin(_)
            | OperationKind::Variant { .. }
            | OperationKind::List(_)
            | OperationKind::Record(_)
            | OperationKind::Field { .. }
            | OperationKind::Closure { .. }
            | OperationKind::Unary { .. }
            | OperationKind::Binary { .. }
            | OperationKind::Bind { .. }
            | OperationKind::Discard { .. } => {
                expect_requirements(operation, &RequirementSet::default())
            }
        }
    }

    fn verify_builtin_type(&self, builtin: Builtin, ty: &Type) -> Result<(), IrError> {
        match builtin {
            Builtin::None => Err(IrError::new(
                "None must lower as a variant value, not a builtin function",
            )),
            Builtin::Print | Builtin::Println => {
                let Type::Function(function) = ty else {
                    return Err(IrError::new(format!("{builtin} has non-function type")));
                };
                if function.parameters().len() == 1
                    && function.return_type() == &Type::Unit
                    && function.effects().contains(&crate::Effect::IoStdout)
                    && !type_contains_secret(function.parameters()[0].as_ref())
                {
                    Ok(())
                } else {
                    Err(IrError::new(format!("{builtin} has invalid signature")))
                }
            }
            Builtin::Some => verify_constructor_function(ty, VariantFamily::Option, true),
            Builtin::Ok => verify_constructor_function(ty, VariantFamily::Result, true),
            Builtin::Err => verify_constructor_function(ty, VariantFamily::Result, false),
            Builtin::JsonEncode => {
                let Type::Function(function) = ty else {
                    return Err(IrError::new("json_encode has non-function type"));
                };
                if function.parameters().len() == 1
                    && function.return_type() == &Type::String
                    && type_contains_no_function(function.parameters()[0].as_ref())
                {
                    Ok(())
                } else {
                    Err(IrError::new("json_encode has invalid signature"))
                }
            }
            Builtin::JsonDecode => {
                let Type::Function(function) = ty else {
                    return Err(IrError::new("json_decode has non-function type"));
                };
                if function.parameters().len() == 1
                    && function.parameters()[0].as_ref() == &Type::String
                    && type_contains_no_function(function.return_type())
                {
                    Ok(())
                } else {
                    Err(IrError::new("json_decode has invalid signature"))
                }
            }
            Builtin::ConfigString => {
                let Type::Function(function) = ty else {
                    return Err(IrError::new("config_string has non-function type"));
                };
                let expected = Type::Result(Arc::new(Type::String), Arc::new(Type::String));
                if function.parameters() == [Arc::new(Type::String)]
                    && function.return_type() == &expected
                    && function.effects().contains(&crate::Effect::ConfigRead)
                {
                    Ok(())
                } else {
                    Err(IrError::new("config_string has invalid signature"))
                }
            }
            Builtin::Secret => {
                let Type::Function(function) = ty else {
                    return Err(IrError::new("secret has non-function type"));
                };
                let expected = Type::Result(Arc::new(Type::Secret), Arc::new(Type::String));
                if function.parameters() == [Arc::new(Type::String)]
                    && function.return_type() == &expected
                    && function.effects().contains(&crate::Effect::SecretRead)
                {
                    Ok(())
                } else {
                    Err(IrError::new("secret has invalid signature"))
                }
            }
        }
    }

    fn verify_binary(
        &self,
        operation: &CoreOperation,
        left: ValueId,
        operator: BinaryOperator,
        right: ValueId,
    ) -> Result<(), IrError> {
        let left_type = self.value_type(left)?;
        let right_type = self.value_type(right)?;
        match operator {
            BinaryOperator::Add => {
                if left_type != right_type
                    || !matches!(left_type, Type::Int | Type::String | Type::Variable(_))
                    || operation.ty.as_ref() != left_type
                {
                    return Err(IrError::new(format!(
                        "{} has invalid addition types",
                        operation.result
                    )));
                }
            }
            BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                if left_type != &Type::Int
                    || right_type != &Type::Int
                    || operation.ty.as_ref() != &Type::Int
                {
                    return Err(IrError::new(format!(
                        "{} has invalid arithmetic types",
                        operation.result
                    )));
                }
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                if left_type != right_type
                    || !type_contains_no_function(left_type)
                    || operation.ty.as_ref() != &Type::Bool
                {
                    return Err(IrError::new(format!(
                        "{} has invalid equality types",
                        operation.result
                    )));
                }
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if left_type != &Type::Int
                    || right_type != &Type::Int
                    || operation.ty.as_ref() != &Type::Bool
                {
                    return Err(IrError::new(format!(
                        "{} has invalid comparison types",
                        operation.result
                    )));
                }
            }
            BinaryOperator::And | BinaryOperator::Or => {
                return Err(IrError::new(
                    "short-circuit boolean operators must lower as explicit branches",
                ));
            }
        }
        Ok(())
    }

    fn value_type(&self, value: ValueId) -> Result<&Type, IrError> {
        self.value_types
            .get(&value)
            .map(AsRef::as_ref)
            .ok_or_else(|| IrError::new(format!("value {value} is out of range")))
    }

    fn expect_value_type(&self, value: ValueId, expected: &Type) -> Result<(), IrError> {
        let actual = self.value_type(value)?;
        if type_equivalent(actual, expected) {
            Ok(())
        } else {
            Err(IrError::new(format!(
                "value {value} has type `{actual}`, expected `{expected}`"
            )))
        }
    }

    fn function(&self, function: FunctionId) -> Result<&CoreFunction, IrError> {
        self.module
            .functions
            .get(function.0 as usize)
            .filter(|candidate| candidate.id == function)
            .ok_or_else(|| IrError::new(format!("function {function} is out of range")))
    }
}

fn function_type(signature: &FunctionSignature) -> Type {
    Type::Function(FunctionType::new(
        signature.parameters.clone(),
        Arc::clone(&signature.result),
        signature.effects.clone(),
        signature.requirements.clone(),
    ))
}

fn verify_constructor_function(
    ty: &Type,
    family: VariantFamily,
    first_payload: bool,
) -> Result<(), IrError> {
    let Type::Function(function) = ty else {
        return Err(IrError::new("constructor has non-function type"));
    };
    if function.parameters().len() != 1 {
        return Err(IrError::new("constructor has invalid function boundary"));
    }
    let parameter = function.parameters()[0].as_ref();
    if type_contains_secret(parameter) {
        return Err(IrError::new(
            "opaque Secret cannot be placed in a constructed variant",
        ));
    }
    let valid = match (family, function.return_type()) {
        (VariantFamily::Option, Type::Option(element)) => element.as_ref() == parameter,
        (VariantFamily::Result, Type::Result(value, _)) if first_payload => {
            value.as_ref() == parameter
        }
        (VariantFamily::Result, Type::Result(_, error)) => error.as_ref() == parameter,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(IrError::new("constructor has invalid variant signature"))
    }
}

fn verify_variant_type(
    variant: VariantName,
    payload: Option<ValueId>,
    ty: &Type,
    value_types: &BTreeMap<ValueId, Arc<Type>>,
) -> Result<(), IrError> {
    let expected_payload = match (variant, ty) {
        (VariantName::Some, Type::Option(element)) => Some(element.as_ref()),
        (VariantName::None, Type::Option(_)) => None,
        (VariantName::Ok, Type::Result(value, _)) => Some(value.as_ref()),
        (VariantName::Err, Type::Result(_, error)) => Some(error.as_ref()),
        _ => return Err(IrError::new("variant operation has invalid result type")),
    };
    match (expected_payload, payload) {
        (None, None) => Ok(()),
        (Some(expected), Some(value)) => {
            let actual = value_types
                .get(&value)
                .map(AsRef::as_ref)
                .ok_or_else(|| IrError::new(format!("variant payload {value} is out of range")))?;
            if actual == expected {
                Ok(())
            } else {
                Err(IrError::new("variant payload type is inconsistent"))
            }
        }
        _ => Err(IrError::new("variant payload shape is inconsistent")),
    }
}

fn type_equivalent(left: &Type, right: &Type) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (Type::HttpHeader, Type::Record(fields)) | (Type::Record(fields), Type::HttpHeader) => {
            record_matches(fields, &[("name", Type::String), ("value", Type::String)])
        }
        (Type::HttpRequest, Type::Record(fields)) | (Type::Record(fields), Type::HttpRequest) => {
            record_matches(
                fields,
                &[
                    ("body", Type::String),
                    ("headers", Type::List(Arc::new(Type::HttpHeader))),
                    ("method", Type::String),
                    ("path", Type::String),
                    ("query", Type::String),
                ],
            )
        }
        (Type::HttpResponse, Type::Record(fields)) | (Type::Record(fields), Type::HttpResponse) => {
            record_matches(
                fields,
                &[
                    ("body", Type::String),
                    ("headers", Type::List(Arc::new(Type::HttpHeader))),
                    ("status", Type::Int),
                ],
            )
        }
        (Type::List(left), Type::List(right)) | (Type::Option(left), Type::Option(right)) => {
            type_equivalent(left, right)
        }
        (Type::Result(left_value, left_error), Type::Result(right_value, right_error)) => {
            type_equivalent(left_value, right_value) && type_equivalent(left_error, right_error)
        }
        (Type::Record(left_fields), Type::Record(right_fields)) => {
            left_fields.len() == right_fields.len()
                && left_fields.iter().all(|left| {
                    right_fields.iter().any(|right| {
                        left.name() == right.name() && type_equivalent(left.ty(), right.ty())
                    })
                })
        }
        (Type::Function(left), Type::Function(right)) => {
            left.parameters().len() == right.parameters().len()
                && left
                    .parameters()
                    .iter()
                    .zip(right.parameters())
                    .all(|(left, right)| type_equivalent(left, right))
                && type_equivalent(left.return_type(), right.return_type())
                && left.effects() == right.effects()
                && left.requirements() == right.requirements()
        }
        _ => false,
    }
}

fn record_matches(fields: &[crate::RecordType], expected: &[(&str, Type)]) -> bool {
    fields.len() == expected.len()
        && expected.iter().all(|(name, ty)| {
            fields
                .iter()
                .find(|field| field.name() == *name)
                .is_some_and(|field| type_equivalent(field.ty(), ty))
        })
}

fn record_field_type(ty: &Type, name: &str) -> Option<Arc<Type>> {
    match ty {
        Type::Record(fields) => fields
            .iter()
            .find(|field| field.name() == name)
            .map(|field| Arc::new(field.ty().clone())),
        Type::HttpHeader => match name {
            "name" | "value" => Some(Arc::new(Type::String)),
            _ => None,
        },
        Type::HttpRequest => match name {
            "method" | "path" | "query" | "body" => Some(Arc::new(Type::String)),
            "headers" => Some(Arc::new(Type::List(Arc::new(Type::HttpHeader)))),
            _ => None,
        },
        Type::HttpResponse => match name {
            "status" => Some(Arc::new(Type::Int)),
            "headers" => Some(Arc::new(Type::List(Arc::new(Type::HttpHeader)))),
            "body" => Some(Arc::new(Type::String)),
            _ => None,
        },
        _ => None,
    }
}

fn type_contains_no_function(ty: &Type) -> bool {
    match ty {
        Type::Function(_) => false,
        Type::List(element) | Type::Option(element) => type_contains_no_function(element),
        Type::Record(fields) => fields
            .iter()
            .all(|field| type_contains_no_function(field.ty())),
        Type::Result(value, error) => {
            type_contains_no_function(value) && type_contains_no_function(error)
        }
        Type::Int
        | Type::Bool
        | Type::String
        | Type::Unit
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::Variable(_) => true,
        Type::Secret => false,
    }
}

fn type_contains_secret(ty: &Type) -> bool {
    match ty {
        Type::Secret => true,
        Type::List(element) | Type::Option(element) => type_contains_secret(element),
        Type::Record(fields) => fields.iter().any(|field| type_contains_secret(field.ty())),
        Type::Result(value, error) => type_contains_secret(value) || type_contains_secret(error),
        Type::Function(function) => {
            function
                .parameters()
                .iter()
                .any(|parameter| type_contains_secret(parameter))
                || type_contains_secret(function.return_type())
        }
        Type::Int
        | Type::Bool
        | Type::String
        | Type::Unit
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::Variable(_) => false,
    }
}

fn function_has_residual(function: &CoreFunction, visited: &mut HashSet<*const Type>) -> bool {
    function
        .signature
        .parameters
        .iter()
        .any(|ty| type_has_residual(ty.as_ref(), visited))
        || type_has_residual(function.signature.result.as_ref(), visited)
        || function
            .parameters
            .iter()
            .any(|parameter| type_has_residual(parameter.ty.as_ref(), visited))
        || function
            .captures
            .iter()
            .any(|capture| type_has_residual(capture.ty.as_ref(), visited))
        || function
            .recursive
            .as_ref()
            .is_some_and(|recursive| type_has_residual(recursive.ty.as_ref(), visited))
        || block_has_residual(&function.body, visited)
}

fn block_has_residual(block: &CoreBlock, visited: &mut HashSet<*const Type>) -> bool {
    type_has_residual(block.ty.as_ref(), visited)
        || block
            .parameters
            .iter()
            .any(|parameter| type_has_residual(parameter.ty.as_ref(), visited))
        || block.operations.iter().any(|operation| {
            type_has_residual(operation.ty.as_ref(), visited)
                || match &operation.kind {
                    OperationKind::Block { block } => block_has_residual(block, visited),
                    OperationKind::If {
                        consequent,
                        alternative,
                        ..
                    } => {
                        block_has_residual(consequent, visited)
                            || block_has_residual(alternative, visited)
                    }
                    OperationKind::MatchList { empty, cons, .. } => {
                        block_has_residual(empty, visited) || block_has_residual(cons, visited)
                    }
                    OperationKind::MatchVariant { arms, .. } => arms
                        .iter()
                        .any(|arm| block_has_residual(&arm.block, visited)),
                    OperationKind::Literal(_)
                    | OperationKind::Unit
                    | OperationKind::Builtin(_)
                    | OperationKind::Variant { .. }
                    | OperationKind::List(_)
                    | OperationKind::Record(_)
                    | OperationKind::Field { .. }
                    | OperationKind::Closure { .. }
                    | OperationKind::Call { .. }
                    | OperationKind::Unary { .. }
                    | OperationKind::Binary { .. }
                    | OperationKind::Bind { .. }
                    | OperationKind::Discard { .. } => false,
                }
        })
}

fn type_has_residual(ty: &Type, visited: &mut HashSet<*const Type>) -> bool {
    if !visited.insert(std::ptr::from_ref(ty)) {
        return false;
    }
    match ty {
        Type::Variable(_) => true,
        Type::List(element) | Type::Option(element) => type_has_residual(element.as_ref(), visited),
        Type::Record(fields) => fields
            .iter()
            .any(|field| type_has_residual(field.ty(), visited)),
        Type::Result(value, error) => {
            type_has_residual(value.as_ref(), visited) || type_has_residual(error.as_ref(), visited)
        }
        Type::Function(function) => {
            function
                .parameters()
                .iter()
                .any(|parameter| type_has_residual(parameter.as_ref(), visited))
                || type_has_residual(function.return_type(), visited)
        }
        Type::Int
        | Type::Bool
        | Type::String
        | Type::Unit
        | Type::HttpHeader
        | Type::HttpRequest
        | Type::HttpResponse
        | Type::Secret => false,
    }
}

fn variant_payload_types(
    ty: &Type,
    family: VariantFamily,
) -> Result<BTreeMap<VariantName, Option<Arc<Type>>>, IrError> {
    match (family, ty) {
        (VariantFamily::Option, Type::Option(element)) => Ok(BTreeMap::from([
            (VariantName::Some, Some(Arc::clone(element))),
            (VariantName::None, None),
        ])),
        (VariantFamily::Result, Type::Result(value, error)) => Ok(BTreeMap::from([
            (VariantName::Ok, Some(Arc::clone(value))),
            (VariantName::Err, Some(Arc::clone(error))),
        ])),
        _ => Err(IrError::new(
            "variant match family does not match subject type",
        )),
    }
}

fn require_available(
    value: ValueId,
    available: &BTreeSet<ValueId>,
    user: impl fmt::Display,
) -> Result<(), IrError> {
    if available.contains(&value) {
        Ok(())
    } else {
        Err(IrError::new(format!(
            "{user} uses unavailable value {value}"
        )))
    }
}

fn expect_type(operation: &CoreOperation, expected: &Type) -> Result<(), IrError> {
    if type_equivalent(operation.ty.as_ref(), expected) {
        Ok(())
    } else {
        Err(IrError::new(format!(
            "{} has type `{}`, expected `{expected}`",
            operation.result, operation.ty
        )))
    }
}

fn expect_effects(operation: &CoreOperation, expected: &EffectSet) -> Result<(), IrError> {
    if &operation.effects == expected {
        Ok(())
    } else {
        Err(IrError::new(format!(
            "{} has effects {}, expected {}",
            operation.result, operation.effects, expected
        )))
    }
}

fn require_effects(operation: &CoreOperation, required: &EffectSet) -> Result<(), IrError> {
    if operation.effects.is_superset(required) {
        Ok(())
    } else {
        Err(IrError::new(format!(
            "{} effects {} do not cover {}",
            operation.result, operation.effects, required
        )))
    }
}

fn expect_requirements(
    operation: &CoreOperation,
    expected: &RequirementSet,
) -> Result<(), IrError> {
    if &operation.requirements == expected {
        Ok(())
    } else {
        Err(IrError::new(format!(
            "{} has capability requirements {}, expected {}",
            operation.result, operation.requirements, expected
        )))
    }
}

fn require_requirements(
    operation: &CoreOperation,
    required: &RequirementSet,
) -> Result<(), IrError> {
    if operation.requirements.is_superset(required) {
        Ok(())
    } else {
        Err(IrError::new(format!(
            "{} capability requirements {} do not cover {}",
            operation.result, operation.requirements, required
        )))
    }
}

fn insert_unique<T: Copy + fmt::Display + Ord>(
    set: &mut BTreeSet<T>,
    id: T,
    kind: &str,
) -> Result<(), IrError> {
    if set.insert(id) {
        Ok(())
    } else {
        Err(IrError::new(format!("duplicate {kind} ID {id}")))
    }
}

trait NumericId {
    fn number(self) -> u32;
}

macro_rules! numeric_id {
    ($($name:ident),+ $(,)?) => {
        $(
            impl NumericId for $name {
                fn number(self) -> u32 {
                    self.0
                }
            }
        )+
    };
}

numeric_id!(
    BlockId,
    ValueId,
    ParameterId,
    CaptureId,
    ClosureId,
    MatchBindingId,
);

fn verify_contiguous<T: Copy + NumericId + Ord>(
    kind: &str,
    ids: &BTreeSet<T>,
) -> Result<(), IrError> {
    for (index, id) in ids.iter().copied().enumerate() {
        if id.number() as usize != index {
            return Err(IrError::new(format!("{kind} IDs are not contiguous")));
        }
    }
    Ok(())
}

fn render_function(output: &mut String, function: &CoreFunction) {
    output.push_str(&format!(
        "function {}{} (",
        function.id,
        function
            .debug_name
            .as_deref()
            .map_or_else(String::new, |name| format!(" {name}"))
    ));
    for (index, parameter) in function.parameters.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&format!(
            "{} {} {}: {}",
            parameter.id, parameter.binding, parameter.value, parameter.ty
        ));
    }
    output.push_str(&format!(
        ") -> {} effects {}",
        function.signature.result, function.signature.effects
    ));
    if !function.signature.requirements.is_empty() {
        output.push_str(&format!(
            " requirements {}",
            function.signature.requirements
        ));
    }
    output.push('\n');
    if let Some(recursive) = &function.recursive {
        output.push_str(&format!(
            "  self {} {}: {}\n",
            recursive.binding, recursive.value, recursive.ty
        ));
    }
    for capture in &function.captures {
        output.push_str(&format!(
            "  capture {} {} {}: {}\n",
            capture.id, capture.binding, capture.value, capture.ty
        ));
    }
    render_block(output, &function.body, 1);
}

fn render_block(output: &mut String, block: &CoreBlock, indent: usize) {
    let padding = "  ".repeat(indent);
    output.push_str(&format!("{padding}block {}", block.id,));
    if !block.parameters.is_empty() {
        output.push('(');
        for (index, parameter) in block.parameters.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            output.push_str(&format!(
                "{} {} {} {}: {}",
                parameter.match_binding,
                parameter.binding,
                parameter.value,
                parameter.debug_name.as_deref().unwrap_or("<anonymous>"),
                parameter.ty
            ));
        }
        output.push(')');
    }
    output.push_str(&format!(" -> {} effects {}", block.ty, block.effects));
    if !block.requirements.is_empty() {
        output.push_str(&format!(" requirements {}", block.requirements));
    }
    output.push('\n');
    for operation in &block.operations {
        render_operation(output, operation, indent + 1);
    }
    output.push_str(&format!("{padding}  return {}\n", block.result));
}

fn render_operation(output: &mut String, operation: &CoreOperation, indent: usize) {
    let padding = "  ".repeat(indent);
    output.push_str(&format!(
        "{padding}{}: {} = ",
        operation.result, operation.ty
    ));
    match &operation.kind {
        OperationKind::Literal(ValueLiteral::Integer(value)) => {
            output.push_str(&format!("int {value}"));
        }
        OperationKind::Literal(ValueLiteral::Boolean(value)) => {
            output.push_str(&format!("bool {value}"));
        }
        OperationKind::Literal(ValueLiteral::String(value)) => {
            output.push_str(&format!("string {value:?}"));
        }
        OperationKind::Unit => output.push_str("unit"),
        OperationKind::Builtin(builtin) => output.push_str(&format!(
            "builtin {} [{}]",
            builtin,
            builtin.category().as_str()
        )),
        OperationKind::Variant { variant, payload } => {
            output.push_str(variant.as_str());
            if let Some(payload) = payload {
                output.push_str(&format!(" {payload}"));
            }
        }
        OperationKind::List(elements) => {
            output.push_str("list [");
            render_values(output, elements);
            output.push(']');
        }
        OperationKind::Record(fields) => {
            output.push_str("record {");
            for (index, field) in fields.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!("{}: {}", field.name, field.value));
            }
            output.push('}');
        }
        OperationKind::Field { value, field } => {
            output.push_str(&format!("field {value}.{field}"));
        }
        OperationKind::Block { block } => {
            output.push_str(&format!("block effects {}\n", operation.effects));
            render_block(output, block, indent + 1);
            return;
        }
        OperationKind::Closure {
            closure,
            function,
            captures,
        } => {
            output.push_str(&format!("closure {closure} {function} ["));
            for (index, capture) in captures.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!(
                    "{}:{}={}",
                    capture.capture, capture.binding, capture.value
                ));
            }
            output.push(']');
        }
        OperationKind::Call { callee, arguments } => {
            output.push_str(&format!("call {callee}("));
            render_values(output, arguments);
            output.push(')');
        }
        OperationKind::If {
            condition,
            consequent,
            alternative,
        } => {
            output.push_str(&format!("if {condition} effects {}\n", operation.effects));
            render_block(output, consequent, indent + 1);
            output.push_str(&format!("{padding}else\n"));
            render_block(output, alternative, indent + 1);
            return;
        }
        OperationKind::MatchList {
            subject,
            empty,
            head,
            tail,
            cons,
        } => {
            output.push_str(&format!(
                "match-list {subject} effects {}\n",
                operation.effects
            ));
            output.push_str(&format!("{padding}empty\n"));
            render_block(output, empty, indent + 1);
            output.push_str(&format!("{padding}cons {head} {tail}\n"));
            render_block(output, cons, indent + 1);
            return;
        }
        OperationKind::MatchVariant {
            subject,
            family,
            arms,
        } => {
            output.push_str(&format!(
                "match-{} {subject} effects {}\n",
                variant_family_name(*family),
                operation.effects
            ));
            for arm in arms {
                output.push_str(&format!("{padding}{}\n", arm.variant.as_str()));
                render_block(output, &arm.block, indent + 1);
            }
            return;
        }
        OperationKind::Unary { operator, operand } => {
            output.push_str(&format!("{} {operand}", unary_name(*operator)));
        }
        OperationKind::Binary {
            left,
            operator,
            right,
        } => {
            output.push_str(&format!("{} {left} {right}", binary_name(*operator)));
        }
        OperationKind::Bind { binding, value } => {
            output.push_str(&format!("bind {binding} = {value}"));
        }
        OperationKind::Discard { value } => {
            output.push_str(&format!("discard {value}"));
        }
    }
    if !operation.effects.is_empty() {
        output.push_str(&format!(" effects {}", operation.effects));
    }
    if !operation.requirements.is_empty() {
        output.push_str(&format!(" requirements {}", operation.requirements));
    }
    output.push('\n');
}

fn render_values(output: &mut String, values: &[ValueId]) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_str(", ");
        }
        output.push_str(&value.to_string());
    }
}

const fn unary_name(operator: UnaryOperator) -> &'static str {
    match operator {
        UnaryOperator::Not => "not",
        UnaryOperator::Negate => "neg",
    }
}

const fn binary_name(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Add => "add",
        BinaryOperator::Subtract => "sub",
        BinaryOperator::Multiply => "mul",
        BinaryOperator::Divide => "div",
        BinaryOperator::Remainder => "rem",
        BinaryOperator::Equal => "eq",
        BinaryOperator::NotEqual => "ne",
        BinaryOperator::Less => "lt",
        BinaryOperator::LessEqual => "le",
        BinaryOperator::Greater => "gt",
        BinaryOperator::GreaterEqual => "ge",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
    }
}

const fn variant_family_name(family: VariantFamily) -> &'static str {
    match family {
        VariantFamily::Option => "option",
        VariantFamily::Result => "result",
    }
}

#[cfg(test)]
mod tests {
    use crate::{Source, analyze, parse_source};

    use super::*;

    fn lowered(text: &str) -> CoreModule {
        let source = Source::new("test.krit", text);
        let program = parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("source should analyze");
        lower(&program, &analysis).expect("source should lower and verify")
    }

    fn visit_operations(block: &CoreBlock, visit: &mut impl FnMut(&CoreOperation)) {
        for operation in &block.operations {
            visit(operation);
            match &operation.kind {
                OperationKind::Block { block } => visit_operations(block, visit),
                OperationKind::If {
                    consequent,
                    alternative,
                    ..
                } => {
                    visit_operations(consequent, visit);
                    visit_operations(alternative, visit);
                }
                OperationKind::MatchList { empty, cons, .. } => {
                    visit_operations(empty, visit);
                    visit_operations(cons, visit);
                }
                OperationKind::MatchVariant { arms, .. } => {
                    for arm in arms {
                        visit_operations(&arm.block, visit);
                    }
                }
                OperationKind::Literal(_)
                | OperationKind::Unit
                | OperationKind::Builtin(_)
                | OperationKind::Variant { .. }
                | OperationKind::List(_)
                | OperationKind::Record(_)
                | OperationKind::Field { .. }
                | OperationKind::Closure { .. }
                | OperationKind::Call { .. }
                | OperationKind::Unary { .. }
                | OperationKind::Binary { .. }
                | OperationKind::Bind { .. }
                | OperationKind::Discard { .. } => {}
            }
        }
    }

    #[test]
    fn accepts_conservative_effects_on_intrinsically_pure_builtins() {
        for (builtin, text) in [
            (
                Builtin::Some,
                r#"
                let wrap = if true {
                    Some
                } else {
                    fn(value) {
                        println(value);
                        Some(value)
                    }
                };
                let result = wrap(1);
                "#,
            ),
            (
                Builtin::Ok,
                r#"
                let wrap = if true {
                    Ok
                } else {
                    fn(value) {
                        println(value);
                        Ok(value)
                    }
                };
                let result = wrap(1);
                "#,
            ),
            (
                Builtin::Err,
                r#"
                let wrap = if true {
                    Err
                } else {
                    fn(value) {
                        println(value);
                        Err(value)
                    }
                };
                let result = wrap(1);
                "#,
            ),
            (
                Builtin::Some,
                r#"
                fn apply(constructor, value) {
                    constructor(value)
                }
                fn loud_some(value) {
                    println(value);
                    Some(value)
                }
                let first = apply(Some, 1);
                let second = apply(loud_some, 2);
                "#,
            ),
            (
                Builtin::JsonEncode,
                r#"
                let encode = if true {
                    json_encode
                } else {
                    fn(value) {
                        println(value);
                        json_encode(value)
                    }
                };
                let encoded = encode(1);
                "#,
            ),
            (
                Builtin::JsonDecode,
                r#"
                let decode = if true {
                    json_decode
                } else {
                    fn(value) {
                        println(value);
                        json_decode(value)
                    }
                };
                let decoded: Int = decode("1");
                "#,
            ),
        ] {
            let module = lowered(text);
            let mut found_builtin = false;
            let mut found_effectful_call = false;
            for function in module.functions() {
                visit_operations(&function.body, &mut |operation| match &operation.kind {
                    OperationKind::Builtin(candidate) if *candidate == builtin => {
                        let Type::Function(function) = operation.ty.as_ref() else {
                            panic!("{builtin} should retain a function type");
                        };
                        assert!(
                            operation.effects.is_empty(),
                            "creating builtin {builtin} must remain pure"
                        );
                        found_builtin |= function.effects().contains(&crate::Effect::IoStdout);
                    }
                    OperationKind::Call { .. }
                        if operation.effects.contains(&crate::Effect::IoStdout) =>
                    {
                        found_effectful_call = true;
                    }
                    _ => {}
                });
            }
            assert!(found_builtin, "expected a lowered {builtin} value");
            assert!(
                found_effectful_call,
                "a later call should carry the inferred conservative effect"
            );
            module
                .verify()
                .expect("conservative builtin type should verify");
        }
    }

    #[test]
    fn reports_residual_types_without_rejecting_parametric_core() {
        let generic = lowered(
            r#"
            fn generic_add(left, right) {
                left + right
            }
            fn generic_equal(left, right) {
                left == right
            }
            "#,
        );
        assert!(generic.has_residual_types());
        generic
            .verify()
            .expect("operation constraints make parametric Core valid");

        let factorial = lowered(
            r#"
            fn factorial(number) {
                if number == 0 {
                    1
                } else {
                    number * factorial(number - 1)
                }
            }
            println(factorial(6));
            "#,
        );
        assert!(!factorial.has_residual_types());
    }

    #[test]
    fn rejects_unavailable_value_use() {
        let mut module = lowered("let value = 1; println(value);");
        let entrypoint = &mut module.functions[0];
        let call = entrypoint
            .body
            .operations
            .iter_mut()
            .find(|operation| matches!(operation.kind, OperationKind::Call { .. }))
            .expect("program should contain a call");
        let OperationKind::Call { callee, .. } = &mut call.kind else {
            unreachable!("operation was selected as a call");
        };
        *callee = ValueId(u32::MAX);
        assert!(
            module
                .verify()
                .expect_err("malformed use should fail")
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut module = lowered("let first = 1; let second = 2;");
        let operations = &mut module.functions[0].body.operations;
        operations[1].result = operations[0].result;
        assert!(
            module
                .verify()
                .expect_err("duplicate values should fail")
                .to_string()
                .contains("duplicate value ID")
        );
    }

    #[test]
    fn rejects_call_arity_and_effect_understatement() {
        let mut arity = lowered("println(1);");
        let call = arity.functions[0]
            .body
            .operations
            .iter_mut()
            .find(|operation| matches!(operation.kind, OperationKind::Call { .. }))
            .expect("program should contain a call");
        let OperationKind::Call { arguments, .. } = &mut call.kind else {
            unreachable!("operation was selected as a call");
        };
        arguments.clear();
        assert!(
            arity
                .verify()
                .expect_err("bad arity should fail")
                .to_string()
                .contains("arity")
        );

        let mut effects = lowered("println(1);");
        let call = effects.functions[0]
            .body
            .operations
            .iter_mut()
            .find(|operation| matches!(operation.kind, OperationKind::Call { .. }))
            .expect("program should contain a call");
        call.effects = EffectSet::default();
        assert!(
            effects
                .verify()
                .expect_err("missing effects should fail")
                .to_string()
                .contains("do not cover")
        );

        let mut requirements = lowered("config_string(\"agent.model\");");
        let call = requirements.functions[0]
            .body
            .operations
            .iter_mut()
            .find(|operation| matches!(operation.kind, OperationKind::Call { .. }))
            .expect("program should contain a call");
        call.requirements = RequirementSet::default();
        assert!(
            requirements
                .verify()
                .expect_err("missing capability requirement should fail")
                .to_string()
                .contains("capability requirements")
        );
    }

    #[test]
    fn rejects_branch_type_mismatch_and_bad_capture() {
        let mut branch = lowered("if true { 1 } else { 2 };");
        let if_operation = branch.functions[0]
            .body
            .operations
            .iter_mut()
            .find(|operation| matches!(operation.kind, OperationKind::If { .. }))
            .expect("program should contain a branch");
        let OperationKind::If { alternative, .. } = &mut if_operation.kind else {
            unreachable!("operation was selected as an if");
        };
        alternative.ty = Arc::new(Type::Bool);
        assert!(
            branch
                .verify()
                .expect_err("bad branch type should fail")
                .to_string()
                .contains("type")
        );

        let mut capture = lowered("let offset = 1; let add = fn(value) { value + offset };");
        let closure = capture.functions[0]
            .body
            .operations
            .iter_mut()
            .find(|operation| matches!(operation.kind, OperationKind::Closure { .. }))
            .expect("program should contain a closure");
        let OperationKind::Closure { captures, .. } = &mut closure.kind else {
            unreachable!("operation was selected as a closure");
        };
        captures.clear();
        assert!(
            capture
                .verify()
                .expect_err("bad capture should fail")
                .to_string()
                .contains("capture count")
        );
    }
}
