use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    sync::Arc,
};

use krit_capability::{HttpOrigin, is_valid_resource_name};

use crate::{
    Builtin, Diagnostic, Span,
    ast::{
        BinaryOperator, Block, Expression, ExpressionKind, MatchKind, Parameter, Program,
        Statement, StatementKind, TypeAnnotation, TypeKind, UnaryOperator, ValueLiteral,
        VariantFamily, VariantName,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Analysis {
    bindings: Vec<BindingAnalysis>,
    effects: EffectSet,
    requirements: RequirementSet,
    symbols: Vec<SymbolAnalysis>,
    symbol_index: Vec<u32>,
    expressions: Vec<ExpressionAnalysis>,
    blocks: Vec<BlockAnalysis>,
}

impl Analysis {
    pub fn bindings(&self) -> &[BindingAnalysis] {
        &self.bindings
    }

    pub const fn effects(&self) -> &EffectSet {
        &self.effects
    }

    pub const fn requirements(&self) -> &RequirementSet {
        &self.requirements
    }

    pub fn symbols(&self) -> &[SymbolAnalysis] {
        &self.symbols
    }

    pub fn expressions(&self) -> &[ExpressionAnalysis] {
        &self.expressions
    }

    pub fn blocks(&self) -> &[BlockAnalysis] {
        &self.blocks
    }

    pub fn symbol(&self, span: Span, name: &str, kind: SymbolKind) -> Option<&SymbolAnalysis> {
        let position = self
            .symbol_index
            .binary_search_by(|index| {
                symbol_key_order(&self.symbols[*index as usize], span, name, kind)
            })
            .ok()?;
        self.symbols.get(self.symbol_index[position] as usize)
    }

    pub fn expression(&self, span: Span) -> Option<&ExpressionAnalysis> {
        self.expressions
            .binary_search_by_key(&span, |expression| expression.span)
            .ok()
            .and_then(|index| self.expressions.get(index))
    }

    pub fn block(&self, span: Span) -> Option<&BlockAnalysis> {
        self.blocks
            .binary_search_by_key(&span, |block| block.span)
            .ok()
            .and_then(|index| self.blocks.get(index))
    }
}

fn symbol_key_order(symbol: &SymbolAnalysis, span: Span, name: &str, kind: SymbolKind) -> Ordering {
    symbol
        .span
        .cmp(&span)
        .then_with(|| symbol.name.as_str().cmp(name))
        .then_with(|| symbol.kind.cmp(&kind))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingAnalysis {
    id: SymbolId,
    name: String,
    ty: Arc<Type>,
    span: Span,
    top_level: bool,
}

impl BindingAnalysis {
    pub const fn id(&self) -> SymbolId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> &Type {
        self.ty.as_ref()
    }

    pub const fn span(&self) -> Span {
        self.span
    }

    pub const fn is_top_level(&self) -> bool {
        self.top_level
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolId(u32);

impl SymbolId {
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolKind {
    Let,
    Function,
    Webhook,
    Parameter,
    Match,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolAnalysis {
    id: SymbolId,
    name: String,
    kind: SymbolKind,
    ty: Arc<Type>,
    span: Span,
    top_level: bool,
}

impl SymbolAnalysis {
    pub const fn id(&self) -> SymbolId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn ty(&self) -> &Type {
        self.ty.as_ref()
    }

    pub(crate) fn shared_type(&self) -> Arc<Type> {
        Arc::clone(&self.ty)
    }

    pub const fn span(&self) -> Span {
        self.span
    }

    pub const fn is_top_level(&self) -> bool {
        self.top_level
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedName {
    Symbol(SymbolId),
    Builtin(Builtin),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionAnalysis {
    span: Span,
    ty: Arc<Type>,
    effects: EffectSet,
    requirements: RequirementSet,
    resolved_name: Option<ResolvedName>,
}

impl ExpressionAnalysis {
    pub const fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> &Type {
        self.ty.as_ref()
    }

    pub(crate) fn shared_type(&self) -> Arc<Type> {
        Arc::clone(&self.ty)
    }

    pub const fn effects(&self) -> &EffectSet {
        &self.effects
    }

    pub const fn requirements(&self) -> &RequirementSet {
        &self.requirements
    }

    pub const fn resolved_name(&self) -> Option<ResolvedName> {
        self.resolved_name
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockAnalysis {
    span: Span,
    ty: Arc<Type>,
    effects: EffectSet,
    requirements: RequirementSet,
}

impl BlockAnalysis {
    pub const fn span(&self) -> Span {
        self.span
    }

    pub fn ty(&self) -> &Type {
        self.ty.as_ref()
    }

    pub(crate) fn shared_type(&self) -> Arc<Type> {
        Arc::clone(&self.ty)
    }

    pub const fn effects(&self) -> &EffectSet {
        &self.effects
    }

    pub const fn requirements(&self) -> &RequirementSet {
        &self.requirements
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Type {
    Int,
    Bool,
    String,
    Unit,
    HttpHeader,
    HttpRequest,
    HttpResponse,
    LogField,
    Secret,
    List(Arc<Self>),
    Record(Vec<RecordType>),
    Option(Arc<Self>),
    Result(Arc<Self>, Arc<Self>),
    Function(FunctionType),
    Variable(u32),
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => formatter.write_str("Int"),
            Self::Bool => formatter.write_str("Bool"),
            Self::String => formatter.write_str("String"),
            Self::Unit => formatter.write_str("Unit"),
            Self::HttpHeader => formatter.write_str("HttpHeader"),
            Self::HttpRequest => formatter.write_str("HttpRequest"),
            Self::HttpResponse => formatter.write_str("HttpResponse"),
            Self::LogField => formatter.write_str("LogField"),
            Self::Secret => formatter.write_str("Secret"),
            Self::List(element) => write!(formatter, "List<{element}>"),
            Self::Record(fields) => {
                formatter.write_str("Record { ")?;
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{}: {}", field.name, field.ty)?;
                }
                formatter.write_str(" }")
            }
            Self::Option(element) => write!(formatter, "Option<{element}>"),
            Self::Result(value, error) => write!(formatter, "Result<{value}, {error}>"),
            Self::Function(function) => function.fmt(formatter),
            Self::Variable(id) => write!(formatter, "'{}", type_variable_name(*id)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordType {
    name: String,
    ty: Arc<Type>,
}

impl RecordType {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> &Type {
        self.ty.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionType {
    parameters: Vec<Arc<Type>>,
    return_type: Arc<Type>,
    effects: EffectSet,
    requirements: RequirementSet,
}

impl FunctionType {
    pub(crate) fn new(
        parameters: Vec<Arc<Type>>,
        return_type: Arc<Type>,
        effects: EffectSet,
        requirements: RequirementSet,
    ) -> Self {
        Self {
            parameters,
            return_type,
            effects,
            requirements,
        }
    }

    pub fn parameters(&self) -> &[Arc<Type>] {
        &self.parameters
    }

    pub fn return_type(&self) -> &Type {
        self.return_type.as_ref()
    }

    pub(crate) fn shared_return_type(&self) -> Arc<Type> {
        Arc::clone(&self.return_type)
    }

    pub const fn effects(&self) -> &EffectSet {
        &self.effects
    }

    pub const fn requirements(&self) -> &RequirementSet {
        &self.requirements
    }
}

impl fmt::Display for FunctionType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("fn(")?;
        for (index, parameter) in self.parameters.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            parameter.fmt(formatter)?;
        }
        write!(
            formatter,
            ") -> {} effects {}",
            self.return_type, self.effects
        )?;
        if !self.requirements.is_empty() {
            write!(formatter, " requirements {}", self.requirements)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Effect {
    AiInvoke,
    ConfigRead,
    HttpRequest,
    IoStdout,
    ObserveLog,
    SecretRead,
}

impl Effect {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AiInvoke => "ai.invoke",
            Self::ConfigRead => "config.read",
            Self::HttpRequest => "http.request",
            Self::IoStdout => "io.stdout",
            Self::ObserveLog => "observe.log",
            Self::SecretRead => "secret.read",
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectSet {
    effects: Vec<Effect>,
}

impl EffectSet {
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Effect> {
        self.effects.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn contains(&self, effect: &Effect) -> bool {
        self.effects.binary_search(effect).is_ok()
    }

    pub fn is_superset(&self, other: &Self) -> bool {
        other.effects.iter().all(|effect| self.contains(effect))
    }

    pub(crate) fn union<'a>(sets: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut effects = BTreeSet::new();
        for set in sets {
            effects.extend(set.effects.iter().cloned());
        }
        effect_set(effects)
    }
}

impl fmt::Display for EffectSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("{")?;
        for (index, effect) in self.effects.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            effect.fmt(formatter)?;
        }
        formatter.write_str("}")
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapabilityRequirement {
    capability: Effect,
    resource: String,
}

impl CapabilityRequirement {
    pub fn new(capability: Effect, resource: impl Into<String>) -> Self {
        Self {
            capability,
            resource: resource.into(),
        }
    }

    pub const fn capability(&self) -> &Effect {
        &self.capability
    }

    pub fn resource(&self) -> &str {
        &self.resource
    }
}

impl fmt::Display for CapabilityRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}({:?})", self.capability, self.resource)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequirementSet {
    requirements: Vec<CapabilityRequirement>,
}

impl RequirementSet {
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &CapabilityRequirement> {
        self.requirements.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty()
    }

    pub fn contains(&self, requirement: &CapabilityRequirement) -> bool {
        self.requirements.binary_search(requirement).is_ok()
    }

    pub fn is_superset(&self, other: &Self) -> bool {
        other
            .requirements
            .iter()
            .all(|requirement| self.contains(requirement))
    }

    pub(crate) fn union<'a>(sets: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut requirements = BTreeSet::new();
        for set in sets {
            requirements.extend(set.requirements.iter().cloned());
        }
        requirement_set(requirements)
    }

    pub(crate) fn from_requirements(
        requirements: impl IntoIterator<Item = CapabilityRequirement>,
    ) -> Self {
        requirement_set(requirements.into_iter().collect())
    }
}

impl fmt::Display for RequirementSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("{")?;
        for (index, requirement) in self.requirements.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            requirement.fmt(formatter)?;
        }
        formatter.write_str("}")
    }
}

pub fn analyze(program: &Program) -> Result<Analysis, Diagnostic> {
    Analyzer::new().program(program)
}

#[derive(Clone, Debug)]
enum InferType {
    Int,
    Bool,
    String,
    Unit,
    HttpHeader,
    HttpRequest,
    HttpResponse,
    LogField,
    Secret,
    List(Box<Self>),
    Record {
        fields: BTreeMap<String, Self>,
        open: bool,
    },
    Option(Box<Self>),
    Result(Box<Self>, Box<Self>),
    Function {
        parameters: Vec<Self>,
        return_type: Box<Self>,
        effect: EffectVariable,
    },
    Variable(TypeVariable),
}

type TypeVariable = u32;
type EffectVariable = u32;

#[derive(Clone, Debug, Default)]
struct InferEffects {
    direct: BTreeSet<Effect>,
    direct_requirements: BTreeSet<CapabilityRequirement>,
    dependencies: BTreeSet<EffectVariable>,
}

impl InferEffects {
    fn union(&mut self, other: Self) {
        self.direct.extend(other.direct);
        self.direct_requirements.extend(other.direct_requirements);
        self.dependencies.extend(other.dependencies);
    }
}

#[derive(Clone)]
struct ExpressionInfo {
    ty: InferType,
    effects: InferEffects,
}

struct PendingBinding {
    id: SymbolId,
    name: String,
    ty: InferType,
    span: Span,
    top_level: bool,
}

struct PendingSymbol {
    id: SymbolId,
    name: String,
    kind: SymbolKind,
    ty: InferType,
    span: Span,
    top_level: bool,
}

struct PendingExpression {
    span: Span,
    ty: InferType,
    effects: InferEffects,
    resolved_name: Option<ResolvedName>,
}

struct PendingBlock {
    span: Span,
    ty: InferType,
    effects: InferEffects,
}

struct TypeConstraint {
    ty: InferType,
    span: Span,
    kind: ConstraintKind,
}

enum ConstraintKind {
    Addable,
    Comparable,
    JsonValue,
    OpaqueArgument,
    Printable,
    StructuralValue,
}

struct Analyzer {
    scopes: Vec<BTreeMap<String, ScopeBinding>>,
    type_parents: Vec<TypeVariable>,
    substitutions: HashMap<TypeVariable, InferType>,
    next_type_variable: TypeVariable,
    effect_definitions: Vec<InferEffects>,
    next_symbol: u32,
    bindings: Vec<PendingBinding>,
    symbols: Vec<PendingSymbol>,
    expressions: Vec<PendingExpression>,
    blocks: Vec<PendingBlock>,
    constraints: Vec<TypeConstraint>,
    host_builtin_references: Vec<(Span, Builtin)>,
    direct_host_builtin_references: BTreeSet<Span>,
    allowed_secret_constructor_spans: BTreeSet<Span>,
}

#[derive(Clone)]
struct ScopeBinding {
    id: SymbolId,
    ty: InferType,
}

impl Analyzer {
    fn new() -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
            type_parents: Vec::new(),
            substitutions: HashMap::new(),
            next_type_variable: 0,
            effect_definitions: Vec::new(),
            next_symbol: 0,
            bindings: Vec::new(),
            symbols: Vec::new(),
            expressions: Vec::new(),
            blocks: Vec::new(),
            constraints: Vec::new(),
            host_builtin_references: Vec::new(),
            direct_host_builtin_references: BTreeSet::new(),
            allowed_secret_constructor_spans: BTreeSet::new(),
        }
    }

    fn program(mut self, program: &Program) -> Result<Analysis, Diagnostic> {
        let mut effects = InferEffects::default();
        for statement in &program.statements {
            effects.union(self.statement(statement)?);
        }
        self.validate_host_builtin_references()?;
        self.validate_constraints()?;

        let expanded_effects = self.expanded_effects();
        let expanded_requirements = self.expanded_requirements();
        let mut normalizer = TypeNormalizer::new(&self, &expanded_effects, &expanded_requirements);
        let bindings = self
            .bindings
            .iter()
            .map(|binding| BindingAnalysis {
                id: binding.id,
                name: binding.name.clone(),
                ty: normalizer.normalize(&binding.ty),
                span: binding.span,
                top_level: binding.top_level,
            })
            .collect();
        let requirements =
            requirement_set(self.resolve_requirements(&effects, &expanded_requirements));
        let effects = effect_set(self.resolve_effects(&effects, &expanded_effects));
        let symbols = self
            .symbols
            .iter()
            .map(|symbol| SymbolAnalysis {
                id: symbol.id,
                name: symbol.name.clone(),
                kind: symbol.kind,
                ty: normalizer.normalize(&symbol.ty),
                span: symbol.span,
                top_level: symbol.top_level,
            })
            .collect::<Vec<_>>();
        let mut symbol_index = (0..symbols.len() as u32).collect::<Vec<_>>();
        symbol_index.sort_unstable_by(|left, right| {
            let left = &symbols[*left as usize];
            let right = &symbols[*right as usize];
            symbol_key_order(left, right.span, &right.name, right.kind)
        });
        assert!(
            symbol_index.windows(2).all(|window| {
                let left = &symbols[window[0] as usize];
                let right = &symbols[window[1] as usize];
                symbol_key_order(left, right.span, &right.name, right.kind) != Ordering::Equal
            }),
            "analysis symbol keys must be unique"
        );
        let mut expressions = self
            .expressions
            .iter()
            .map(|expression| ExpressionAnalysis {
                span: expression.span,
                ty: normalizer.normalize(&expression.ty),
                effects: effect_set(self.resolve_effects(&expression.effects, &expanded_effects)),
                requirements: requirement_set(
                    self.resolve_requirements(&expression.effects, &expanded_requirements),
                ),
                resolved_name: expression.resolved_name,
            })
            .collect::<Vec<_>>();
        expressions.sort_by_key(|expression| expression.span);
        assert!(
            expressions
                .windows(2)
                .all(|window| window[0].span != window[1].span),
            "analysis expression spans must be unique"
        );
        let mut blocks = self
            .blocks
            .iter()
            .map(|block| BlockAnalysis {
                span: block.span,
                ty: normalizer.normalize(&block.ty),
                effects: effect_set(self.resolve_effects(&block.effects, &expanded_effects)),
                requirements: requirement_set(
                    self.resolve_requirements(&block.effects, &expanded_requirements),
                ),
            })
            .collect::<Vec<_>>();
        blocks.sort_by_key(|block| block.span);
        assert!(
            blocks
                .windows(2)
                .all(|window| window[0].span != window[1].span),
            "analysis block spans must be unique"
        );

        Ok(Analysis {
            bindings,
            effects,
            requirements,
            symbols,
            symbol_index,
            expressions,
            blocks,
        })
    }

    fn statement(&mut self, statement: &Statement) -> Result<InferEffects, Diagnostic> {
        match &statement.kind {
            StatementKind::Let {
                name,
                annotation,
                value,
            } => {
                self.ensure_name_available(name, statement.span)?;
                let value = self.expression(value)?;
                let ty = if let Some(annotation) = annotation {
                    let annotated = self.annotation(annotation);
                    self.unify(
                        annotated.clone(),
                        value.ty,
                        annotation.span,
                        "let binding annotation",
                    )?;
                    annotated
                } else {
                    value.ty
                };
                let binding_type = self.fresh_type();
                self.unify(binding_type.clone(), ty, statement.span, "let binding")?;
                let top_level = self.scopes.len() == 1;
                let id = self.define_symbol(
                    name,
                    binding_type.clone(),
                    statement.span,
                    SymbolKind::Let,
                    top_level,
                );
                self.bindings.push(PendingBinding {
                    id,
                    name: name.clone(),
                    ty: binding_type,
                    span: statement.span,
                    top_level,
                });
                Ok(value.effects)
            }
            StatementKind::Function {
                name,
                parameters,
                return_type,
                body,
            }
            | StatementKind::Webhook {
                name,
                parameters,
                return_type,
                body,
            } => {
                let symbol_kind = if matches!(statement.kind, StatementKind::Webhook { .. }) {
                    if self.scopes.len() != 1 {
                        return Err(Diagnostic::new(
                            "K1004",
                            "`webhook` declarations are only allowed at the top level",
                            statement.span,
                        ));
                    }
                    let valid_parameter = parameters.len() == 1
                        && parameters[0].annotation.as_ref().is_some_and(|annotation| {
                            matches!(annotation.kind, TypeKind::HttpRequest)
                        });
                    let valid_return = return_type.as_ref().is_some_and(|annotation| {
                        matches!(annotation.kind, TypeKind::HttpResponse)
                    });
                    if !valid_parameter || !valid_return {
                        return Err(Diagnostic::new(
                            "K3007",
                            "webhook signature must be exactly `(request: HttpRequest) -> HttpResponse`",
                            statement.span,
                        ));
                    }
                    SymbolKind::Webhook
                } else {
                    SymbolKind::Function
                };
                self.ensure_name_available(name, statement.span)?;
                let parameter_types = parameters
                    .iter()
                    .map(|parameter| match &parameter.annotation {
                        Some(annotation) => self.annotation(annotation),
                        None => self.fresh_type(),
                    })
                    .collect::<Vec<_>>();
                let declared_return = match return_type {
                    Some(annotation) => self.annotation(annotation),
                    None => self.fresh_type(),
                };
                let effect = self.fresh_effect();
                let function_type = InferType::Function {
                    parameters: parameter_types.clone(),
                    return_type: Box::new(declared_return.clone()),
                    effect,
                };
                let top_level = self.scopes.len() == 1;
                let id = self.define_symbol(
                    name,
                    function_type.clone(),
                    statement.span,
                    symbol_kind,
                    top_level,
                );

                self.push_scope();
                self.define_parameters(parameters, &parameter_types)?;
                let body_info = self.block(body)?;
                self.pop_scope();
                self.unify(
                    declared_return,
                    body_info.ty,
                    return_type.as_ref().map_or(body.span, |value| value.span),
                    "function return type",
                )?;
                self.effect_definitions[effect as usize].union(body_info.effects);
                self.bindings.push(PendingBinding {
                    id,
                    name: name.clone(),
                    ty: function_type,
                    span: statement.span,
                    top_level,
                });
                Ok(InferEffects::default())
            }
            StatementKind::Expression(expression) => Ok(self.expression(expression)?.effects),
        }
    }

    fn block(&mut self, block: &Block) -> Result<ExpressionInfo, Diagnostic> {
        self.push_scope();
        let mut effects = InferEffects::default();
        for statement in &block.statements {
            effects.union(self.statement(statement)?);
        }
        let ty = if let Some(tail) = block.tail.as_deref() {
            let tail = self.expression(tail)?;
            effects.union(tail.effects);
            tail.ty
        } else {
            InferType::Unit
        };
        self.pop_scope();
        let info = ExpressionInfo { ty, effects };
        self.blocks.push(PendingBlock {
            span: block.span,
            ty: info.ty.clone(),
            effects: info.effects.clone(),
        });
        Ok(info)
    }

    fn expression(&mut self, expression: &Expression) -> Result<ExpressionInfo, Diagnostic> {
        let mut resolved_name = None;
        let info = match &expression.kind {
            ExpressionKind::Literal(literal) => Ok(ExpressionInfo {
                ty: match literal {
                    ValueLiteral::Integer(_) => InferType::Int,
                    ValueLiteral::Boolean(_) => InferType::Bool,
                    ValueLiteral::String(_) => InferType::String,
                },
                effects: InferEffects::default(),
            }),
            ExpressionKind::Variable(name) => {
                let (ty, resolution) = self.lookup(name, expression.span)?;
                if matches!(
                    resolution,
                    ResolvedName::Builtin(
                        Builtin::AiInvoke
                            | Builtin::ConfigString
                            | Builtin::Secret
                            | Builtin::HttpRequest
                            | Builtin::LogInfo
                            | Builtin::LogError
                    )
                ) {
                    let ResolvedName::Builtin(builtin) = resolution else {
                        unreachable!("matched a built-in resolution")
                    };
                    self.host_builtin_references
                        .push((expression.span, builtin));
                }
                resolved_name = Some(resolution);
                Ok(ExpressionInfo {
                    ty,
                    effects: InferEffects::default(),
                })
            }
            ExpressionKind::List(elements) => self.list(elements),
            ExpressionKind::Record(fields) => {
                let mut types = BTreeMap::new();
                let mut effects = InferEffects::default();
                for field in fields {
                    let value = self.expression(&field.value)?;
                    self.constraints.push(TypeConstraint {
                        ty: value.ty.clone(),
                        span: field.value.span,
                        kind: ConstraintKind::StructuralValue,
                    });
                    if types.insert(field.name.clone(), value.ty).is_some() {
                        return Err(Diagnostic::new(
                            "K2002",
                            format!("duplicate record field `{}`", field.name),
                            field.span,
                        ));
                    }
                    effects.union(value.effects);
                }
                Ok(ExpressionInfo {
                    ty: InferType::Record {
                        fields: types,
                        open: false,
                    },
                    effects,
                })
            }
            ExpressionKind::FieldAccess { value, field } => {
                let value = self.expression(value)?;
                let ty = self.require_field(value.ty, field, expression.span)?;
                Ok(ExpressionInfo {
                    ty,
                    effects: value.effects,
                })
            }
            ExpressionKind::Block(block) => self.block(block),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                let condition_source_span = condition.span;
                let condition = self.expression(condition)?;
                self.unify(
                    InferType::Bool,
                    condition.ty,
                    condition_source_span,
                    "if condition",
                )?;
                let consequent = self.block(consequent)?;
                let alternative = self.expression(alternative)?;
                let ty = self.unify(
                    consequent.ty,
                    alternative.ty,
                    expression.span,
                    "if branches",
                )?;
                let mut effects = condition.effects;
                effects.union(consequent.effects);
                effects.union(alternative.effects);
                Ok(ExpressionInfo { ty, effects })
            }
            ExpressionKind::Function {
                parameters,
                return_type,
                body,
            } => self.function(parameters, return_type.as_ref(), body),
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, expression.span)
            }
            ExpressionKind::Match { subject, kind } => {
                self.match_expression(subject, kind, expression.span)
            }
            ExpressionKind::Unary { operator, operand } => {
                let operand = self.expression(operand)?;
                let expected = match operator {
                    UnaryOperator::Not => InferType::Bool,
                    UnaryOperator::Negate => InferType::Int,
                };
                let ty = self.unify(expected, operand.ty, expression.span, "unary operator")?;
                Ok(ExpressionInfo {
                    ty,
                    effects: operand.effects,
                })
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => self.binary(left, *operator, right, expression.span),
        }?;
        self.expressions.push(PendingExpression {
            span: expression.span,
            ty: info.ty.clone(),
            effects: info.effects.clone(),
            resolved_name,
        });
        Ok(info)
    }

    fn list(&mut self, elements: &[Expression]) -> Result<ExpressionInfo, Diagnostic> {
        let element_type = self.fresh_type();
        let mut effects = InferEffects::default();
        for element in elements {
            let value = self.expression(element)?;
            self.constraints.push(TypeConstraint {
                ty: value.ty.clone(),
                span: element.span,
                kind: ConstraintKind::StructuralValue,
            });
            self.unify(element_type.clone(), value.ty, element.span, "list element")?;
            effects.union(value.effects);
        }
        Ok(ExpressionInfo {
            ty: InferType::List(Box::new(element_type)),
            effects,
        })
    }

    fn function(
        &mut self,
        parameters: &[Parameter],
        return_type: Option<&TypeAnnotation>,
        body: &Block,
    ) -> Result<ExpressionInfo, Diagnostic> {
        let parameter_types = parameters
            .iter()
            .map(|parameter| match &parameter.annotation {
                Some(annotation) => self.annotation(annotation),
                None => self.fresh_type(),
            })
            .collect::<Vec<_>>();
        let declared_return = match return_type {
            Some(annotation) => self.annotation(annotation),
            None => self.fresh_type(),
        };
        let effect = self.fresh_effect();

        self.push_scope();
        self.define_parameters(parameters, &parameter_types)?;
        let body_info = self.block(body)?;
        self.pop_scope();
        self.unify(
            declared_return.clone(),
            body_info.ty,
            return_type.map_or(body.span, |value| value.span),
            "function return type",
        )?;
        self.effect_definitions[effect as usize].union(body_info.effects);

        Ok(ExpressionInfo {
            ty: InferType::Function {
                parameters: parameter_types,
                return_type: Box::new(declared_return),
                effect,
            },
            effects: InferEffects::default(),
        })
    }

    fn call(
        &mut self,
        callee: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
        let direct_builtin = direct_host_builtin(callee);
        let direct_resource = if let Some(builtin) = direct_builtin {
            self.direct_host_builtin_references.insert(callee.span);
            match (builtin, arguments) {
                (Builtin::AiInvoke, [adapter, _]) => {
                    let ExpressionKind::Literal(ValueLiteral::String(resource)) = &adapter.kind
                    else {
                        return Err(Diagnostic::new(
                            "K3008",
                            "`ai_invoke` requires a direct string-literal adapter name",
                            adapter.span,
                        ));
                    };
                    if !is_valid_resource_name(resource) {
                        return Err(Diagnostic::new(
                            "K3008",
                            "AI adapter name must use 1-64 lowercase letters, digits, `.` or `-`, without leading/trailing punctuation or `..`/`--`",
                            adapter.span,
                        ));
                    }
                    Some((builtin, resource.clone()))
                }
                (Builtin::ConfigString | Builtin::Secret, [argument]) => {
                    let ExpressionKind::Literal(ValueLiteral::String(resource)) = &argument.kind
                    else {
                        return Err(Diagnostic::new(
                            "K3008",
                            format!(
                                "`{}` requires a direct string-literal resource",
                                builtin.as_str()
                            ),
                            argument.span,
                        ));
                    };
                    if !is_valid_resource_name(resource) {
                        return Err(Diagnostic::new(
                            "K3008",
                            "capability resource must use 1-64 lowercase letters, digits, `.` or `-`, without leading/trailing punctuation or `..`/`--`",
                            argument.span,
                        ));
                    }
                    Some((builtin, resource.clone()))
                }
                (Builtin::HttpRequest, [origin, _, bearer]) => {
                    let ExpressionKind::Literal(ValueLiteral::String(origin_value)) = &origin.kind
                    else {
                        return Err(Diagnostic::new(
                            "K3008",
                            "`http_request` requires a direct normalized exact-origin literal",
                            origin.span,
                        ));
                    };
                    let normalized = HttpOrigin::parse_exact(origin_value).map_err(|error| {
                        Diagnostic::new(
                            "K3008",
                            format!("invalid `http_request` origin: {error}"),
                            origin.span,
                        )
                    })?;
                    match &bearer.kind {
                        ExpressionKind::Variable(name) if name == "None" => {}
                        ExpressionKind::Call {
                            callee: constructor,
                            arguments: constructor_arguments,
                        } if matches!(
                            &constructor.kind,
                            ExpressionKind::Variable(name) if name == "Some"
                        ) && constructor_arguments.len() == 1 =>
                        {
                            self.allowed_secret_constructor_spans
                                .insert(constructor.span);
                        }
                        _ => {
                            return Err(Diagnostic::new(
                                "K3008",
                                "`http_request` bearer must be directly `None` or `Some(secret)`",
                                bearer.span,
                            ));
                        }
                    }
                    Some((builtin, normalized.as_str().to_owned()))
                }
                (Builtin::LogInfo | Builtin::LogError, [event, _]) => {
                    let ExpressionKind::Literal(ValueLiteral::String(name)) = &event.kind else {
                        return Err(Diagnostic::new(
                            "K3008",
                            format!(
                                "`{}` requires a direct string-literal event name",
                                builtin.as_str()
                            ),
                            event.span,
                        ));
                    };
                    if !is_valid_resource_name(name) {
                        return Err(Diagnostic::new(
                            "K3008",
                            "log event name must use 1-64 lowercase letters, digits, `.` or `-`, without leading/trailing punctuation or `..`/`--`",
                            event.span,
                        ));
                    }
                    None
                }
                _ => None,
            }
        } else {
            None
        };
        let callee_info = self.expression(callee)?;
        let mut effects = callee_info.effects;
        let mut argument_types = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let argument = self.expression(argument)?;
            argument_types.push(argument.ty);
            effects.union(argument.effects);
        }

        let callee_type = self.resolve_head(&callee_info.ty);
        let (parameters, return_type, effect) = match callee_type {
            InferType::Function {
                parameters,
                return_type,
                effect,
            } => (parameters, *return_type, effect),
            InferType::Variable(variable) => {
                let parameters = (0..arguments.len())
                    .map(|_| self.fresh_type())
                    .collect::<Vec<_>>();
                let return_type = self.fresh_type();
                let effect = self.fresh_effect();
                self.bind_variable(
                    variable,
                    InferType::Function {
                        parameters: parameters.clone(),
                        return_type: Box::new(return_type.clone()),
                        effect,
                    },
                    span,
                    "call target",
                )?;
                (parameters, return_type, effect)
            }
            other => {
                return Err(Diagnostic::new(
                    "K3002",
                    format!("cannot call value of type `{}`", self.render_type(&other)),
                    callee.span,
                ));
            }
        };

        if parameters.len() != argument_types.len() {
            return Err(Diagnostic::new(
                "K3003",
                format!(
                    "function expects {} argument(s), found {}",
                    parameters.len(),
                    argument_types.len()
                ),
                span,
            ));
        }
        if let Some((builtin, resource)) = direct_resource {
            let capability = match builtin {
                Builtin::AiInvoke => Effect::AiInvoke,
                Builtin::ConfigString => Effect::ConfigRead,
                Builtin::Secret => Effect::SecretRead,
                Builtin::HttpRequest => Effect::HttpRequest,
                _ => unreachable!("only resource host built-ins are returned"),
            };
            effects.direct.insert(capability.clone());
            effects
                .direct_requirements
                .insert(CapabilityRequirement::new(capability, resource));
        }
        for (index, (parameter, argument)) in parameters.into_iter().zip(argument_types).enumerate()
        {
            if matches!(direct_builtin, Some(Builtin::LogInfo | Builtin::LogError))
                && self.contains_secret(&argument, &mut BTreeSet::new())
            {
                return Err(Diagnostic::new(
                    "K3009",
                    "opaque `Secret` values cannot be placed into structured log fields",
                    arguments[index].span,
                ));
            }
            let approved_secret_use =
                matches!(
                    direct_builtin,
                    Some(Builtin::HttpRequest) if index == 2
                ) || self.allowed_secret_constructor_spans.contains(&callee.span);
            if !approved_secret_use {
                self.constraints.push(TypeConstraint {
                    ty: argument.clone(),
                    span: arguments[index].span,
                    kind: ConstraintKind::OpaqueArgument,
                });
            }
            self.unify(parameter, argument, span, "function argument")?;
        }
        effects.dependencies.insert(effect);
        Ok(ExpressionInfo {
            ty: return_type,
            effects,
        })
    }

    fn match_expression(
        &mut self,
        subject: &Expression,
        kind: &MatchKind,
        span: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
        let subject = self.expression(subject)?;
        match kind {
            MatchKind::List {
                empty_case,
                head_name,
                tail_name,
                cons_case,
            } => {
                if head_name == tail_name {
                    return Err(Diagnostic::new(
                        "K2002",
                        format!("duplicate match binding `{tail_name}`"),
                        span,
                    ));
                }
                let element = self.fresh_type();
                self.expect_match_type(
                    subject.ty,
                    InferType::List(Box::new(element.clone())),
                    span,
                    "List",
                )?;
                let empty = self.expression(empty_case)?;
                self.push_scope();
                self.define_symbol(head_name, element.clone(), span, SymbolKind::Match, false);
                self.define_symbol(
                    tail_name,
                    InferType::List(Box::new(element)),
                    span,
                    SymbolKind::Match,
                    false,
                );
                let cons = self.expression(cons_case)?;
                self.pop_scope();
                let ty = self.unify(empty.ty, cons.ty, span, "list match arms")?;
                let mut effects = subject.effects;
                effects.union(empty.effects);
                effects.union(cons.effects);
                Ok(ExpressionInfo { ty, effects })
            }
            MatchKind::Variants { family, arms } => {
                self.variant_match(subject, *family, arms, span)
            }
        }
    }

    fn variant_match(
        &mut self,
        subject: ExpressionInfo,
        family: VariantFamily,
        arms: &[crate::VariantArm],
        span: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
        self.verify_variant_arms(family, arms, span)?;
        let first = self.fresh_type();
        let second = self.fresh_type();
        let expected = match family {
            VariantFamily::Option => InferType::Option(Box::new(first.clone())),
            VariantFamily::Result => {
                InferType::Result(Box::new(first.clone()), Box::new(second.clone()))
            }
        };
        self.expect_match_type(
            subject.ty,
            expected,
            span,
            match family {
                VariantFamily::Option => "Option",
                VariantFamily::Result => "Result",
            },
        )?;

        let mut result_type = None;
        let mut effects = subject.effects;
        for arm in arms {
            self.push_scope();
            if let Some(binding) = &arm.binding {
                let payload = match arm.variant {
                    VariantName::Some | VariantName::Ok => first.clone(),
                    VariantName::Err => second.clone(),
                    VariantName::None => {
                        self.pop_scope();
                        return Err(Diagnostic::new(
                            "K3005",
                            "`None` cannot bind a payload",
                            arm.span,
                        ));
                    }
                };
                self.define_symbol(binding, payload, arm.span, SymbolKind::Match, false);
            }
            let arm_info = self.expression(&arm.value)?;
            self.pop_scope();
            effects.union(arm_info.effects);
            result_type = Some(if let Some(result_type) = result_type {
                self.unify(result_type, arm_info.ty, arm.span, "variant match arms")?
            } else {
                arm_info.ty
            });
        }
        Ok(ExpressionInfo {
            ty: result_type.unwrap_or_else(|| self.fresh_type()),
            effects,
        })
    }

    fn binary(
        &mut self,
        left: &Expression,
        operator: BinaryOperator,
        right: &Expression,
        span: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
        let left = self.expression(left)?;
        let right = self.expression(right)?;
        let mut effects = left.effects;
        effects.union(right.effects);

        let ty = match operator {
            BinaryOperator::Add => {
                let ty = self.unify(left.ty, right.ty, span, "`+` operands")?;
                self.constraints.push(TypeConstraint {
                    ty: ty.clone(),
                    span,
                    kind: ConstraintKind::Addable,
                });
                ty
            }
            BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                self.unify(InferType::Int, left.ty, span, "arithmetic operand")?;
                self.unify(InferType::Int, right.ty, span, "arithmetic operand")?;
                InferType::Int
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                let ty = self.unify(left.ty, right.ty, span, "equality operands")?;
                self.constraints.push(TypeConstraint {
                    ty,
                    span,
                    kind: ConstraintKind::Comparable,
                });
                InferType::Bool
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                self.unify(InferType::Int, left.ty, span, "comparison operand")?;
                self.unify(InferType::Int, right.ty, span, "comparison operand")?;
                InferType::Bool
            }
            BinaryOperator::And | BinaryOperator::Or => {
                self.unify(InferType::Bool, left.ty, span, "boolean operand")?;
                self.unify(InferType::Bool, right.ty, span, "boolean operand")?;
                InferType::Bool
            }
        };
        Ok(ExpressionInfo { ty, effects })
    }

    fn annotation(&mut self, annotation: &TypeAnnotation) -> InferType {
        match &annotation.kind {
            TypeKind::Int => InferType::Int,
            TypeKind::Bool => InferType::Bool,
            TypeKind::String => InferType::String,
            TypeKind::Unit => InferType::Unit,
            TypeKind::HttpHeader => InferType::HttpHeader,
            TypeKind::HttpRequest => InferType::HttpRequest,
            TypeKind::HttpResponse => InferType::HttpResponse,
            TypeKind::LogField => InferType::LogField,
            TypeKind::Secret => InferType::Secret,
            TypeKind::List(element) => InferType::List(Box::new(self.annotation(element))),
            TypeKind::Option(element) => InferType::Option(Box::new(self.annotation(element))),
            TypeKind::Result(value, error) => InferType::Result(
                Box::new(self.annotation(value)),
                Box::new(self.annotation(error)),
            ),
            TypeKind::Record(fields) => InferType::Record {
                fields: fields
                    .iter()
                    .map(|field| (field.name.clone(), self.annotation(&field.annotation)))
                    .collect(),
                open: false,
            },
        }
    }

    fn define_parameters(
        &mut self,
        parameters: &[Parameter],
        types: &[InferType],
    ) -> Result<(), Diagnostic> {
        for (parameter, ty) in parameters.iter().zip(types) {
            self.ensure_name_available(&parameter.name, parameter.span)?;
            self.define_symbol(
                &parameter.name,
                ty.clone(),
                parameter.span,
                SymbolKind::Parameter,
                false,
            );
        }
        Ok(())
    }

    fn verify_variant_arms(
        &self,
        family: VariantFamily,
        arms: &[crate::VariantArm],
        span: Span,
    ) -> Result<(), Diagnostic> {
        let expected = match family {
            VariantFamily::Option => [VariantName::Some, VariantName::None],
            VariantFamily::Result => [VariantName::Ok, VariantName::Err],
        };
        if arms.len() != expected.len()
            || expected
                .iter()
                .any(|variant| arms.iter().filter(|arm| arm.variant == *variant).count() != 1)
            || arms.iter().any(|arm| !expected.contains(&arm.variant))
        {
            return Err(Diagnostic::new(
                "K3005",
                format!(
                    "{} match must contain exactly its two variant arms",
                    match family {
                        VariantFamily::Option => "Option",
                        VariantFamily::Result => "Result",
                    }
                ),
                span,
            ));
        }
        for arm in arms {
            let should_bind = arm.variant != VariantName::None;
            if should_bind != arm.binding.is_some() {
                return Err(Diagnostic::new(
                    "K3005",
                    format!("invalid `{}` match binding", arm.variant.as_str()),
                    arm.span,
                ));
            }
        }
        Ok(())
    }

    fn expect_match_type(
        &mut self,
        actual: InferType,
        expected: InferType,
        span: Span,
        family: &str,
    ) -> Result<(), Diagnostic> {
        let actual_resolved = self.resolve_head(&actual);
        let valid = matches!(
            (&actual_resolved, &expected),
            (InferType::Variable(_), _)
                | (InferType::List(_), InferType::List(_))
                | (InferType::Option(_), InferType::Option(_))
                | (InferType::Result(_, _), InferType::Result(_, _))
        );
        if !valid {
            return Err(Diagnostic::new(
                "K3005",
                format!(
                    "{family} match requires `{family}` subject, found `{}`",
                    self.render_type(&actual_resolved)
                ),
                span,
            ));
        }
        self.unify(expected, actual, span, "match subject")?;
        Ok(())
    }

    fn require_field(
        &mut self,
        ty: InferType,
        field: &str,
        span: Span,
    ) -> Result<InferType, Diagnostic> {
        match ty {
            InferType::Variable(variable) => {
                let variable = self.root_variable(variable);
                if let Some(existing) = self.substitutions.get(&variable).cloned() {
                    let (field_type, widened) =
                        self.require_field_and_widen(existing, field, span)?;
                    self.substitutions.insert(variable, widened);
                    Ok(field_type)
                } else {
                    let field_type = self.fresh_type();
                    self.substitutions.insert(
                        variable,
                        InferType::Record {
                            fields: BTreeMap::from([(field.to_owned(), field_type.clone())]),
                            open: true,
                        },
                    );
                    Ok(field_type)
                }
            }
            other => self
                .require_field_and_widen(other, field, span)
                .map(|(field_type, _)| field_type),
        }
    }

    fn require_field_and_widen(
        &mut self,
        ty: InferType,
        field: &str,
        span: Span,
    ) -> Result<(InferType, InferType), Diagnostic> {
        match ty {
            InferType::Variable(variable) => {
                let variable = self.root_variable(variable);
                let field_type = self.require_field(InferType::Variable(variable), field, span)?;
                Ok((field_type, InferType::Variable(variable)))
            }
            nominal @ (InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField) => {
                let (field_type, _) =
                    self.require_field_and_widen(contract_record(&nominal), field, span)?;
                Ok((field_type, nominal))
            }
            InferType::Record { mut fields, open } => {
                if let Some(field_type) = fields.get(field) {
                    return Ok((field_type.clone(), InferType::Record { fields, open }));
                }
                if open {
                    let field_type = self.fresh_type();
                    fields.insert(field.to_owned(), field_type.clone());
                    Ok((field_type, InferType::Record { fields, open }))
                } else {
                    Err(Diagnostic::new(
                        "K3004",
                        format!("record type has no field `{field}`"),
                        span,
                    ))
                }
            }
            other => Err(Diagnostic::new(
                "K3004",
                format!(
                    "field access requires a record, found `{}`",
                    self.render_type(&other)
                ),
                span,
            )),
        }
    }

    fn unify(
        &mut self,
        expected: InferType,
        actual: InferType,
        span: Span,
        context: &str,
    ) -> Result<InferType, Diagnostic> {
        self.unify_inner(expected.clone(), actual.clone(), span)
            .map_err(|()| {
                Diagnostic::new(
                    "K3001",
                    format!(
                        "{context} type mismatch: expected `{}`, found `{}`",
                        self.render_type(&expected),
                        self.render_type(&actual)
                    ),
                    span,
                )
            })
    }

    fn unify_inner(
        &mut self,
        left: InferType,
        right: InferType,
        span: Span,
    ) -> Result<InferType, ()> {
        match (left, right) {
            (InferType::Variable(left), InferType::Variable(right)) => {
                self.unify_variables(left, right, span)
            }
            (InferType::Variable(variable), other) => self
                .bind_variable(variable, other, span, "type inference")
                .map_err(|_| ()),
            (other, InferType::Variable(variable)) => self
                .bind_variable(variable, other, span, "type inference")
                .map_err(|_| ()),
            (InferType::Int, InferType::Int) => Ok(InferType::Int),
            (InferType::Bool, InferType::Bool) => Ok(InferType::Bool),
            (InferType::String, InferType::String) => Ok(InferType::String),
            (InferType::Unit, InferType::Unit) => Ok(InferType::Unit),
            (InferType::HttpHeader, InferType::HttpHeader) => Ok(InferType::HttpHeader),
            (InferType::HttpRequest, InferType::HttpRequest) => Ok(InferType::HttpRequest),
            (InferType::HttpResponse, InferType::HttpResponse) => Ok(InferType::HttpResponse),
            (InferType::LogField, InferType::LogField) => Ok(InferType::LogField),
            (InferType::Secret, InferType::Secret) => Ok(InferType::Secret),
            (
                nominal @ (InferType::HttpHeader
                | InferType::HttpRequest
                | InferType::HttpResponse
                | InferType::LogField),
                record @ InferType::Record { .. },
            ) => {
                self.unify_inner(contract_record(&nominal), record, span)?;
                Ok(nominal)
            }
            (
                record @ InferType::Record { .. },
                nominal @ (InferType::HttpHeader
                | InferType::HttpRequest
                | InferType::HttpResponse
                | InferType::LogField),
            ) => {
                self.unify_inner(record, contract_record(&nominal), span)?;
                Ok(nominal)
            }
            (InferType::List(left), InferType::List(right)) => self
                .unify_inner(*left, *right, span)
                .map(|element| InferType::List(Box::new(element))),
            (InferType::Option(left), InferType::Option(right)) => self
                .unify_inner(*left, *right, span)
                .map(|element| InferType::Option(Box::new(element))),
            (
                InferType::Result(left_value, left_error),
                InferType::Result(right_value, right_error),
            ) => {
                let value = self.unify_inner(*left_value, *right_value, span)?;
                let error = self.unify_inner(*left_error, *right_error, span)?;
                Ok(InferType::Result(Box::new(value), Box::new(error)))
            }
            (
                InferType::Record {
                    fields: left,
                    open: left_open,
                },
                InferType::Record {
                    fields: right,
                    open: right_open,
                },
            ) => self.unify_records(left, left_open, right, right_open, span),
            (
                InferType::Function {
                    parameters: left_parameters,
                    return_type: left_return,
                    effect: left_effect,
                },
                InferType::Function {
                    parameters: right_parameters,
                    return_type: right_return,
                    effect: right_effect,
                },
            ) => {
                if left_parameters.len() != right_parameters.len() {
                    return Err(());
                }
                let parameters = left_parameters
                    .into_iter()
                    .zip(right_parameters)
                    .map(|(left, right)| self.unify_inner(left, right, span))
                    .collect::<Result<Vec<_>, _>>()?;
                let return_type = self.unify_inner(*left_return, *right_return, span)?;
                self.link_effects(left_effect, right_effect);
                Ok(InferType::Function {
                    parameters,
                    return_type: Box::new(return_type),
                    effect: left_effect,
                })
            }
            _ => Err(()),
        }
    }

    fn unify_variables(
        &mut self,
        left: TypeVariable,
        right: TypeVariable,
        span: Span,
    ) -> Result<InferType, ()> {
        let left = self.root_variable(left);
        let right = self.root_variable(right);
        if left == right {
            return Ok(InferType::Variable(left));
        }

        let left_binding = self.substitutions.get(&left).cloned();
        let right_binding = self.substitutions.get(&right).cloned();
        match (left_binding, right_binding) {
            (None, None) => {
                let (root, alias) = if left < right {
                    (left, right)
                } else {
                    (right, left)
                };
                self.type_parents[alias as usize] = root;
                Ok(InferType::Variable(root))
            }
            (Some(binding), None) => {
                if self.occurs(right, &binding) {
                    return Err(());
                }
                self.type_parents[right as usize] = left;
                Ok(InferType::Variable(left))
            }
            (None, Some(binding)) => {
                if self.occurs(left, &binding) {
                    return Err(());
                }
                self.type_parents[left as usize] = right;
                Ok(InferType::Variable(right))
            }
            (Some(left_binding), Some(right_binding)) => {
                self.substitutions.remove(&left);
                self.substitutions.remove(&right);
                let unified =
                    match self.unify_inner(left_binding.clone(), right_binding.clone(), span) {
                        Ok(unified) => unified,
                        Err(()) => {
                            self.substitutions.insert(left, left_binding);
                            self.substitutions.insert(right, right_binding);
                            return Err(());
                        }
                    };
                if self.occurs(left, &unified) || self.occurs(right, &unified) {
                    self.substitutions.insert(left, left_binding);
                    self.substitutions.insert(right, right_binding);
                    return Err(());
                }
                let (root, alias) = if left < right {
                    (left, right)
                } else {
                    (right, left)
                };
                self.type_parents[alias as usize] = root;
                self.substitutions.insert(root, unified);
                Ok(InferType::Variable(root))
            }
        }
    }

    fn unify_records(
        &mut self,
        left: BTreeMap<String, InferType>,
        left_open: bool,
        right: BTreeMap<String, InferType>,
        right_open: bool,
        span: Span,
    ) -> Result<InferType, ()> {
        if (!left_open && !right_open && left.keys().ne(right.keys()))
            || (!left_open && right.keys().any(|name| !left.contains_key(name)))
            || (!right_open && left.keys().any(|name| !right.contains_key(name)))
        {
            return Err(());
        }

        let mut fields = BTreeMap::new();
        for name in left.keys().chain(right.keys()) {
            if fields.contains_key(name) {
                continue;
            }
            let ty = match (left.get(name), right.get(name)) {
                (Some(left), Some(right)) => self.unify_inner(left.clone(), right.clone(), span)?,
                (Some(left), None) => left.clone(),
                (None, Some(right)) => right.clone(),
                (None, None) => unreachable!("name came from one record"),
            };
            fields.insert(name.clone(), ty);
        }
        Ok(InferType::Record {
            fields,
            open: left_open && right_open,
        })
    }

    fn bind_variable(
        &mut self,
        variable: TypeVariable,
        ty: InferType,
        span: Span,
        context: &str,
    ) -> Result<InferType, Diagnostic> {
        let variable = self.root_variable(variable);
        if let InferType::Variable(other) = ty {
            return self.unify_variables(variable, other, span).map_err(|()| {
                Diagnostic::new(
                    "K3001",
                    format!("{context} would create an infinite type"),
                    span,
                )
            });
        }
        if let Some(existing) = self.substitutions.get(&variable).cloned() {
            let unified = self.unify_inner(existing, ty.clone(), span).map_err(|()| {
                Diagnostic::new(
                    "K3001",
                    format!(
                        "{context} type mismatch: incompatible with `{}`",
                        self.render_type(&ty)
                    ),
                    span,
                )
            })?;
            self.substitutions.insert(variable, unified);
            return Ok(InferType::Variable(variable));
        }
        if self.occurs(variable, &ty) {
            return Err(Diagnostic::new(
                "K3001",
                format!("{context} would create an infinite type"),
                span,
            ));
        }
        self.substitutions.insert(variable, ty);
        Ok(InferType::Variable(variable))
    }

    fn occurs(&self, variable: TypeVariable, ty: &InferType) -> bool {
        self.occurs_inner(self.root_variable(variable), ty, &mut BTreeSet::new())
    }

    fn occurs_inner(
        &self,
        variable: TypeVariable,
        ty: &InferType,
        visited: &mut BTreeSet<TypeVariable>,
    ) -> bool {
        match ty {
            InferType::Variable(other) => {
                let other = self.root_variable(*other);
                if variable == other {
                    return true;
                }
                if !visited.insert(other) {
                    return false;
                }
                self.substitutions
                    .get(&other)
                    .is_some_and(|ty| self.occurs_inner(variable, ty, visited))
            }
            InferType::List(element) | InferType::Option(element) => {
                self.occurs_inner(variable, element, visited)
            }
            InferType::Result(value, error) => {
                self.occurs_inner(variable, value, visited)
                    || self.occurs_inner(variable, error, visited)
            }
            InferType::Record { fields, .. } => fields
                .values()
                .any(|ty| self.occurs_inner(variable, ty, visited)),
            InferType::Function {
                parameters,
                return_type,
                ..
            } => {
                parameters
                    .iter()
                    .any(|ty| self.occurs_inner(variable, ty, visited))
                    || self.occurs_inner(variable, return_type, visited)
            }
            InferType::Int
            | InferType::Bool
            | InferType::String
            | InferType::Unit
            | InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField
            | InferType::Secret => false,
        }
    }

    fn root_variable(&self, variable: TypeVariable) -> TypeVariable {
        let parent = self.type_parents[variable as usize];
        if parent == variable {
            variable
        } else {
            self.root_variable(parent)
        }
    }

    fn resolve_head(&self, ty: &InferType) -> InferType {
        match ty {
            InferType::Variable(variable) => {
                let variable = self.root_variable(*variable);
                self.substitutions
                    .get(&variable)
                    .cloned()
                    .unwrap_or(InferType::Variable(variable))
            }
            _ => ty.clone(),
        }
    }

    fn validate_constraints(&self) -> Result<(), Diagnostic> {
        for constraint in &self.constraints {
            let valid = match constraint.kind {
                ConstraintKind::Addable => matches!(
                    self.resolve_head(&constraint.ty),
                    InferType::Int | InferType::String | InferType::Variable(_)
                ),
                ConstraintKind::Comparable => self.is_comparable(&constraint.ty),
                ConstraintKind::JsonValue => self.is_json_value(&constraint.ty),
                ConstraintKind::OpaqueArgument | ConstraintKind::Printable => {
                    !self.contains_secret(&constraint.ty, &mut BTreeSet::new())
                }
                ConstraintKind::StructuralValue
                    if self
                        .allowed_secret_constructor_spans
                        .contains(&constraint.span) =>
                {
                    matches!(self.resolve_head(&constraint.ty), InferType::Secret)
                }
                ConstraintKind::StructuralValue => {
                    !self.contains_secret(&constraint.ty, &mut BTreeSet::new())
                }
            };
            if !valid {
                if self.contains_secret(&constraint.ty, &mut BTreeSet::new()) {
                    let operation = match constraint.kind {
                        ConstraintKind::Comparable => "compared",
                        ConstraintKind::JsonValue => "encoded as JSON",
                        ConstraintKind::OpaqueArgument => {
                            "passed outside the approved HTTP bearer position"
                        }
                        ConstraintKind::Printable => "printed",
                        ConstraintKind::StructuralValue => "placed into ordinary structural data",
                        ConstraintKind::Addable => "used by `+`",
                    };
                    return Err(Diagnostic::new(
                        "K3009",
                        format!("opaque `Secret` values cannot be {operation}"),
                        constraint.span,
                    ));
                }
                let (code, message) = match constraint.kind {
                    ConstraintKind::Addable => (
                        "K3001",
                        format!(
                            "`+` requires two integers or two strings, found `{}`",
                            self.render_type(&constraint.ty)
                        ),
                    ),
                    ConstraintKind::Comparable => (
                        "K3006",
                        format!(
                            "values of type `{}` cannot be compared",
                            self.render_type(&constraint.ty)
                        ),
                    ),
                    ConstraintKind::JsonValue => (
                        "K3006",
                        format!(
                            "values of type `{}` cannot be encoded as JSON",
                            self.render_type(&constraint.ty)
                        ),
                    ),
                    ConstraintKind::OpaqueArgument
                    | ConstraintKind::Printable
                    | ConstraintKind::StructuralValue => {
                        unreachable!("these constraints reject only Secret")
                    }
                };
                return Err(Diagnostic::new(code, message, constraint.span));
            }
        }
        Ok(())
    }

    fn validate_host_builtin_references(&self) -> Result<(), Diagnostic> {
        for (span, builtin) in &self.host_builtin_references {
            if !self.direct_host_builtin_references.contains(span) {
                return Err(Diagnostic::new(
                    "K3008",
                    format!(
                        "`{}` must be called directly with a string-literal resource",
                        builtin.as_str()
                    ),
                    *span,
                ));
            }
        }
        Ok(())
    }

    fn is_comparable(&self, ty: &InferType) -> bool {
        self.contains_no_function(ty, &mut BTreeSet::new())
    }

    fn is_json_value(&self, ty: &InferType) -> bool {
        self.contains_no_function(ty, &mut BTreeSet::new())
    }

    fn contains_no_function(&self, ty: &InferType, visited: &mut BTreeSet<TypeVariable>) -> bool {
        match ty {
            InferType::Function { .. } => false,
            InferType::List(element) | InferType::Option(element) => {
                self.contains_no_function(element, visited)
            }
            InferType::Record { fields, .. } => fields
                .values()
                .all(|ty| self.contains_no_function(ty, visited)),
            InferType::Result(value, error) => {
                self.contains_no_function(value, visited)
                    && self.contains_no_function(error, visited)
            }
            InferType::Variable(variable) => {
                let variable = self.root_variable(*variable);
                if !visited.insert(variable) {
                    return true;
                }
                self.substitutions
                    .get(&variable)
                    .is_none_or(|ty| self.contains_no_function(ty, visited))
            }
            InferType::Int
            | InferType::Bool
            | InferType::String
            | InferType::Unit
            | InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField => true,
            InferType::Secret => false,
        }
    }

    fn contains_secret(&self, ty: &InferType, visited: &mut BTreeSet<TypeVariable>) -> bool {
        match ty {
            InferType::Secret => true,
            InferType::List(element) | InferType::Option(element) => {
                self.contains_secret(element, visited)
            }
            InferType::Record { fields, .. } => {
                fields.values().any(|ty| self.contains_secret(ty, visited))
            }
            InferType::Result(value, error) => {
                self.contains_secret(value, visited) || self.contains_secret(error, visited)
            }
            InferType::Function {
                parameters,
                return_type,
                ..
            } => {
                parameters
                    .iter()
                    .any(|ty| self.contains_secret(ty, visited))
                    || self.contains_secret(return_type, visited)
            }
            InferType::Variable(variable) => {
                let variable = self.root_variable(*variable);
                if !visited.insert(variable) {
                    return false;
                }
                self.substitutions
                    .get(&variable)
                    .is_some_and(|ty| self.contains_secret(ty, visited))
            }
            InferType::Int
            | InferType::Bool
            | InferType::String
            | InferType::Unit
            | InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField => false,
        }
    }

    fn lookup(&mut self, name: &str, span: Span) -> Result<(InferType, ResolvedName), Diagnostic> {
        for scope in self.scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Ok((binding.ty.clone(), ResolvedName::Symbol(binding.id)));
            }
        }
        self.builtin(name, span)
            .ok_or_else(|| Diagnostic::new("K2001", format!("undefined name `{name}`"), span))
    }

    fn builtin(&mut self, name: &str, span: Span) -> Option<(InferType, ResolvedName)> {
        let builtin = Builtin::from_name(name)?;
        let ty = match builtin {
            Builtin::AiInvoke => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::AiInvoke);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::String),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::Print | Builtin::Println => {
                let value = self.fresh_type();
                self.constraints.push(TypeConstraint {
                    ty: value.clone(),
                    span,
                    kind: ConstraintKind::Printable,
                });
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::IoStdout);
                InferType::Function {
                    parameters: vec![value],
                    return_type: Box::new(InferType::Unit),
                    effect,
                }
            }
            Builtin::Some => {
                let value = self.fresh_type();
                self.constraints.push(TypeConstraint {
                    ty: value.clone(),
                    span,
                    kind: ConstraintKind::StructuralValue,
                });
                InferType::Function {
                    parameters: vec![value.clone()],
                    return_type: Box::new(InferType::Option(Box::new(value))),
                    effect: self.fresh_effect(),
                }
            }
            Builtin::None => InferType::Option(Box::new(self.fresh_type())),
            Builtin::Ok => {
                let value = self.fresh_type();
                self.constraints.push(TypeConstraint {
                    ty: value.clone(),
                    span,
                    kind: ConstraintKind::StructuralValue,
                });
                InferType::Function {
                    parameters: vec![value.clone()],
                    return_type: Box::new(InferType::Result(
                        Box::new(value),
                        Box::new(self.fresh_type()),
                    )),
                    effect: self.fresh_effect(),
                }
            }
            Builtin::Err => {
                let error = self.fresh_type();
                self.constraints.push(TypeConstraint {
                    ty: error.clone(),
                    span,
                    kind: ConstraintKind::StructuralValue,
                });
                InferType::Function {
                    parameters: vec![error.clone()],
                    return_type: Box::new(InferType::Result(
                        Box::new(self.fresh_type()),
                        Box::new(error),
                    )),
                    effect: self.fresh_effect(),
                }
            }
            Builtin::JsonEncode => {
                let value = self.fresh_type();
                self.constraints.push(TypeConstraint {
                    ty: value.clone(),
                    span,
                    kind: ConstraintKind::JsonValue,
                });
                InferType::Function {
                    parameters: vec![value],
                    return_type: Box::new(InferType::String),
                    effect: self.fresh_effect(),
                }
            }
            Builtin::JsonDecode => {
                let value = self.fresh_type();
                self.constraints.push(TypeConstraint {
                    ty: value.clone(),
                    span,
                    kind: ConstraintKind::JsonValue,
                });
                InferType::Function {
                    parameters: vec![InferType::String],
                    return_type: Box::new(value),
                    effect: self.fresh_effect(),
                }
            }
            Builtin::ConfigString => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::ConfigRead);
                InferType::Function {
                    parameters: vec![InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::String),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::Secret => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::SecretRead);
                InferType::Function {
                    parameters: vec![InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Secret),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::HttpRequest => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::HttpRequest);
                InferType::Function {
                    parameters: vec![
                        InferType::String,
                        InferType::HttpRequest,
                        InferType::Option(Box::new(InferType::Secret)),
                    ],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::HttpResponse),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::LogInfo | Builtin::LogError => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::ObserveLog);
                InferType::Function {
                    parameters: vec![
                        InferType::String,
                        InferType::List(Box::new(InferType::LogField)),
                    ],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
        };
        Some((ty, ResolvedName::Builtin(builtin)))
    }

    fn ensure_name_available(&self, name: &str, span: Span) -> Result<(), Diagnostic> {
        if Builtin::from_name(name).is_some()
            || self
                .scopes
                .last()
                .is_some_and(|scope| scope.contains_key(name))
        {
            Err(Diagnostic::new(
                "K2002",
                format!("duplicate declaration `{name}`"),
                span,
            ))
        } else {
            Ok(())
        }
    }

    fn define_symbol(
        &mut self,
        name: &str,
        ty: InferType,
        span: Span,
        kind: SymbolKind,
        top_level: bool,
    ) -> SymbolId {
        let id = SymbolId(self.next_symbol);
        self.next_symbol += 1;
        self.symbols.push(PendingSymbol {
            id,
            name: name.to_owned(),
            kind,
            ty: ty.clone(),
            span,
            top_level,
        });
        self.scopes
            .last_mut()
            .expect("analyzer always has a scope")
            .insert(name.to_owned(), ScopeBinding { id, ty });
        id
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn fresh_type(&mut self) -> InferType {
        let variable = self.next_type_variable;
        self.next_type_variable += 1;
        self.type_parents.push(variable);
        InferType::Variable(variable)
    }

    fn fresh_effect(&mut self) -> EffectVariable {
        let variable = self.effect_definitions.len() as EffectVariable;
        self.effect_definitions.push(InferEffects::default());
        variable
    }

    fn link_effects(&mut self, left: EffectVariable, right: EffectVariable) {
        if left != right {
            self.effect_definitions[left as usize]
                .dependencies
                .insert(right);
            self.effect_definitions[right as usize]
                .dependencies
                .insert(left);
        }
    }

    fn expanded_effects(&self) -> Vec<BTreeSet<Effect>> {
        let mut expanded = self
            .effect_definitions
            .iter()
            .map(|definition| definition.direct.clone())
            .collect::<Vec<_>>();
        loop {
            let previous = expanded.clone();
            for (index, definition) in self.effect_definitions.iter().enumerate() {
                for dependency in &definition.dependencies {
                    expanded[index].extend(previous[*dependency as usize].iter().cloned());
                }
            }
            if expanded == previous {
                return expanded;
            }
        }
    }

    fn expanded_requirements(&self) -> Vec<BTreeSet<CapabilityRequirement>> {
        let mut expanded = self
            .effect_definitions
            .iter()
            .map(|definition| definition.direct_requirements.clone())
            .collect::<Vec<_>>();
        loop {
            let previous = expanded.clone();
            for (index, definition) in self.effect_definitions.iter().enumerate() {
                for dependency in &definition.dependencies {
                    expanded[index].extend(previous[*dependency as usize].iter().cloned());
                }
            }
            if expanded == previous {
                return expanded;
            }
        }
    }

    fn resolve_effects(
        &self,
        effects: &InferEffects,
        expanded: &[BTreeSet<Effect>],
    ) -> BTreeSet<Effect> {
        let mut resolved = effects.direct.clone();
        for dependency in &effects.dependencies {
            resolved.extend(expanded[*dependency as usize].iter().cloned());
        }
        resolved
    }

    fn resolve_requirements(
        &self,
        effects: &InferEffects,
        expanded: &[BTreeSet<CapabilityRequirement>],
    ) -> BTreeSet<CapabilityRequirement> {
        let mut resolved = effects.direct_requirements.clone();
        for dependency in &effects.dependencies {
            resolved.extend(expanded[*dependency as usize].iter().cloned());
        }
        resolved
    }

    fn render_type(&self, ty: &InferType) -> String {
        let expanded_effects = self.expanded_effects();
        let expanded_requirements = self.expanded_requirements();
        let mut normalizer = TypeNormalizer::new(self, &expanded_effects, &expanded_requirements);
        normalizer.normalize(ty).to_string()
    }
}

struct TypeNormalizer<'a> {
    analyzer: &'a Analyzer,
    expanded_effects: &'a [BTreeSet<Effect>],
    expanded_requirements: &'a [BTreeSet<CapabilityRequirement>],
    variables: BTreeMap<TypeVariable, u32>,
    normalized: HashMap<TypeVariable, Arc<Type>>,
}

impl<'a> TypeNormalizer<'a> {
    fn new(
        analyzer: &'a Analyzer,
        expanded_effects: &'a [BTreeSet<Effect>],
        expanded_requirements: &'a [BTreeSet<CapabilityRequirement>],
    ) -> Self {
        Self {
            analyzer,
            expanded_effects,
            expanded_requirements,
            variables: BTreeMap::new(),
            normalized: HashMap::new(),
        }
    }

    fn normalize(&mut self, ty: &InferType) -> Arc<Type> {
        match ty {
            InferType::Int => Arc::new(Type::Int),
            InferType::Bool => Arc::new(Type::Bool),
            InferType::String => Arc::new(Type::String),
            InferType::Unit => Arc::new(Type::Unit),
            InferType::HttpHeader => Arc::new(Type::HttpHeader),
            InferType::HttpRequest => Arc::new(Type::HttpRequest),
            InferType::HttpResponse => Arc::new(Type::HttpResponse),
            InferType::LogField => Arc::new(Type::LogField),
            InferType::Secret => Arc::new(Type::Secret),
            InferType::List(element) => Arc::new(Type::List(self.normalize(element))),
            InferType::Record { fields, .. } => Arc::new(Type::Record(
                fields
                    .iter()
                    .map(|(name, ty)| RecordType {
                        name: name.clone(),
                        ty: self.normalize(ty),
                    })
                    .collect(),
            )),
            InferType::Option(element) => Arc::new(Type::Option(self.normalize(element))),
            InferType::Result(value, error) => {
                Arc::new(Type::Result(self.normalize(value), self.normalize(error)))
            }
            InferType::Function {
                parameters,
                return_type,
                effect,
            } => Arc::new(Type::Function(FunctionType {
                parameters: parameters
                    .iter()
                    .map(|parameter| self.normalize(parameter))
                    .collect(),
                return_type: self.normalize(return_type),
                effects: effect_set(self.expanded_effects[*effect as usize].clone()),
                requirements: requirement_set(self.expanded_requirements[*effect as usize].clone()),
            })),
            InferType::Variable(variable) => {
                let variable = self.analyzer.root_variable(*variable);
                if let Some(ty) = self.normalized.get(&variable) {
                    return ty.clone();
                }
                if let Some(ty) = self.analyzer.substitutions.get(&variable).cloned() {
                    let normalized = self.normalize(&ty);
                    self.normalized.insert(variable, normalized.clone());
                    return normalized;
                }
                let next = self.variables.len() as u32;
                Arc::new(Type::Variable(
                    *self.variables.entry(variable).or_insert(next),
                ))
            }
        }
    }
}

fn contract_record(nominal: &InferType) -> InferType {
    let fields = match nominal {
        InferType::HttpHeader => BTreeMap::from([
            ("name".to_owned(), InferType::String),
            ("value".to_owned(), InferType::String),
        ]),
        InferType::LogField => BTreeMap::from([
            ("name".to_owned(), InferType::String),
            ("value".to_owned(), InferType::String),
        ]),
        InferType::HttpRequest => BTreeMap::from([
            ("body".to_owned(), InferType::String),
            (
                "headers".to_owned(),
                InferType::List(Box::new(InferType::HttpHeader)),
            ),
            ("method".to_owned(), InferType::String),
            ("path".to_owned(), InferType::String),
            ("query".to_owned(), InferType::String),
        ]),
        InferType::HttpResponse => BTreeMap::from([
            ("body".to_owned(), InferType::String),
            (
                "headers".to_owned(),
                InferType::List(Box::new(InferType::HttpHeader)),
            ),
            ("status".to_owned(), InferType::Int),
        ]),
        _ => unreachable!("only built-in record contract types have structural aliases"),
    };
    InferType::Record {
        fields,
        open: false,
    }
}

fn direct_host_builtin(callee: &Expression) -> Option<Builtin> {
    let ExpressionKind::Variable(name) = &callee.kind else {
        return None;
    };
    match Builtin::from_name(name) {
        Some(
            builtin @ (Builtin::AiInvoke
            | Builtin::ConfigString
            | Builtin::Secret
            | Builtin::HttpRequest
            | Builtin::LogInfo
            | Builtin::LogError),
        ) => Some(builtin),
        _ => None,
    }
}

fn effect_set(effects: BTreeSet<Effect>) -> EffectSet {
    EffectSet {
        effects: effects.into_iter().collect(),
    }
}

fn requirement_set(requirements: BTreeSet<CapabilityRequirement>) -> RequirementSet {
    RequirementSet {
        requirements: requirements.into_iter().collect(),
    }
}

fn type_variable_name(id: u32) -> String {
    let letter = char::from(b'a' + (id % 26) as u8);
    if id < 26 {
        letter.to_string()
    } else {
        format!("{letter}{}", id / 26)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Source, parse_source};

    use super::*;

    fn check(text: &str) -> Result<Analysis, Diagnostic> {
        let source = Source::new("test.krit", text);
        analyze(&parse_source(&source)?)
    }

    #[test]
    fn infers_recursion_lists_closures_empty_values_and_effects() {
        let analysis = check(
            r#"
            fn factorial(number) {
                if number == 0 { 1 } else { number * factorial(number - 1) }
            }
            fn sum(items) {
                match items {
                    [] => 0,
                    [head, ..tail] => head + sum(tail),
                }
            }
            let empty = [];
            let absent = None;
            let offset = 2;
            let add_offset = fn(value) { value + offset };
            println(add_offset(factorial(sum([1, 2, 3]))));
            "#,
        )
        .expect("valid program should analyze");

        assert_eq!(analysis.effects().to_string(), "{io.stdout}");
        assert_eq!(
            analysis.bindings()[0].ty().to_string(),
            "fn(Int) -> Int effects {}"
        );
        assert_eq!(
            analysis.bindings()[1].ty().to_string(),
            "fn(List<Int>) -> Int effects {}"
        );
        assert_eq!(analysis.bindings()[2].ty().to_string(), "List<'a>");
        assert_eq!(analysis.bindings()[3].ty().to_string(), "Option<'b>");
    }

    #[test]
    fn propagates_effects_through_functions_and_higher_order_calls() {
        let analysis = check(
            r#"
            fn invoke(callback) {
                callback("ready")
            }
            let emit = fn(value) {
                println(value);
            };
            invoke(emit);
            "#,
        )
        .expect("effects should infer");

        assert_eq!(analysis.effects().to_string(), "{io.stdout}");
        assert_eq!(
            analysis.bindings()[0].ty().to_string(),
            "fn(fn(String) -> Unit effects {io.stdout}) -> Unit effects {io.stdout}"
        );
    }

    #[test]
    fn keeps_json_conversion_pure_and_effect_rendering_stable() {
        let source = r#"
            let encoded = json_encode(record { value: 42 });
            let decoded = json_decode(encoded);
            decoded;
        "#;
        let first = check(source).expect("JSON conversion should check");
        let second = check(source).expect("analysis should be repeatable");

        assert!(first.effects().is_empty());
        assert_eq!(first, second);
        assert_eq!(first.effects().to_string(), "{}");
    }

    #[test]
    fn reports_name_and_scope_errors() {
        let undefined = check("let value = missing;").expect_err("undefined name should fail");
        assert_eq!(undefined.code(), "K2001");

        let duplicate = check("let value = 1; let value = 2;").expect_err("duplicate should fail");
        assert_eq!(duplicate.code(), "K2002");

        check("let value = 1; { let value = 2; value; };")
            .expect("nested shadowing should remain valid");
        check(
            r#"
            let value = Some(1);
            match value {
                Some(value) => value,
                None => 0,
            };
            "#,
        )
        .expect("match bindings should shadow in their arm");

        let duplicate_nested =
            check("{ let value = 1; let value = 2; };").expect_err("same block should fail");
        assert_eq!(duplicate_nested.code(), "K2002");
    }

    #[test]
    fn rejects_known_type_contradictions() {
        for text in [
            "1 + \"two\";",
            "if 1 { true } else { false };",
            "record { value: 1 }.missing;",
            "1();",
            "let identity = fn(value) { value }; identity(1, 2);",
            "let values: List<Int> = [1, true];",
            "let value: Bool = 1;",
            "fn value() -> Bool { 1 }",
            "match 1 { [] => 0, [head, ..tail] => head };",
            "match Some(1) { Ok(value) => value, Err(error) => error };",
        ] {
            let error = check(text).expect_err(text);
            assert!(
                error.code().starts_with("K300"),
                "{text} produced {}",
                error.code()
            );
        }
    }

    #[test]
    fn checks_annotations_and_structural_records() {
        let analysis = check(
            r#"
            let request: Record { path: String, retry: Option<Int> } =
                record { path: "/events", retry: None };
            fn get_path(value: Record { path: String, retry: Option<Int> }) -> String {
                value.path
            }
            get_path(request);
            "#,
        )
        .expect("annotations should check");

        assert_eq!(
            analysis.bindings()[0].ty().to_string(),
            "Record { path: String, retry: Option<Int> }"
        );
    }

    #[test]
    fn preserves_open_record_requirements_through_function_returns() {
        for text in [
            r#"
            fn wrap(value) { value.a; value }
            fn use2(value) { wrap(value).b }
            println(use2(record { a: 1 }));
            "#,
            r#"
            fn wrap(value) { value.a; value }
            fn use2(value) { wrap(value).b + 1 }
            println(use2(record { a: 1, b: "wrong" }));
            "#,
        ] {
            let error = check(text).expect_err("invalid returned record should be rejected");
            assert_eq!(error.code(), "K3001");
        }

        for text in [
            r#"
            fn wrap(value) { value.a; value }
            fn use2(value) { wrap(value).b }
            println(use2(record { a: 1, b: true }));
            "#,
            r#"
            fn wrap(value) { value.a; value }
            fn use2(value) { wrap(value).b + 1 }
            println(use2(record { a: 1, b: 2 }));
            "#,
        ] {
            check(text).expect("valid returned record should be accepted");
        }
    }

    #[test]
    fn preserves_open_record_requirements_through_lets_branches_and_matches() {
        for body in [
            "let returned = wrap(value); returned.b",
            "(if true { wrap(value) } else { value }).b",
            r#"
            let returned = match [true] {
                [] => value,
                [head, ..tail] => wrap(value),
            };
            returned.b
            "#,
            r#"
            let returned = match Some(true) {
                Some(payload) => wrap(value),
                None => value,
            };
            returned.b
            "#,
        ] {
            let invalid = format!(
                "fn wrap(value) {{ value.a; value }}\n\
                 fn use2(value) {{ {body} }}\n\
                 println(use2(record {{ a: 1 }}));"
            );
            let error = check(&invalid).expect_err("missing field should remain required");
            assert_eq!(error.code(), "K3001", "{body}");

            let valid = format!(
                "fn wrap(value) {{ value.a; value }}\n\
                 fn use2(value) {{ {body} }}\n\
                 println(use2(record {{ a: 1, b: true }}));"
            );
            check(&valid).unwrap_or_else(|error| panic!("{body}: {error:?}"));
        }
    }

    #[test]
    fn keeps_deep_repeated_let_types_as_a_shared_dag() {
        let mut source = String::from("let x0 = record { a: 1 };\n");
        for depth in 1..=22 {
            source.push_str(&format!(
                "let x{depth} = record {{ l: x{}, r: x{} }};\n",
                depth - 1,
                depth - 1
            ));
        }
        source.push_str("println(1);\n");

        let analysis = check(&source).expect("deep shared record types should analyze");
        let Type::Record(fields) = analysis.bindings()[22].ty() else {
            panic!("last binding should be a record");
        };
        assert_eq!(fields.len(), 2);
        assert!(Arc::ptr_eq(&fields[0].ty, &fields[1].ty));
    }

    #[test]
    fn builds_sorted_unique_indexes_for_analysis_facts() {
        let analysis = check(
            r#"
            let first = 1;
            let second = {
                let nested = first + 1;
                nested
            };
            println(second);
            "#,
        )
        .expect("program should analyze");

        assert!(
            analysis
                .expressions
                .windows(2)
                .all(|window| window[0].span < window[1].span)
        );
        assert!(
            analysis
                .blocks
                .windows(2)
                .all(|window| window[0].span < window[1].span)
        );
        assert_eq!(analysis.symbol_index.len(), analysis.symbols.len());
        assert!(analysis.symbol_index.windows(2).all(|window| {
            let left = &analysis.symbols[window[0] as usize];
            let right = &analysis.symbols[window[1] as usize];
            symbol_key_order(left, right.span, &right.name, right.kind) == Ordering::Less
        }));

        for expression in &analysis.expressions {
            assert_eq!(analysis.expression(expression.span), Some(expression));
        }
        for block in &analysis.blocks {
            assert_eq!(analysis.block(block.span), Some(block));
        }
        for symbol in &analysis.symbols {
            assert_eq!(
                analysis.symbol(symbol.span, &symbol.name, symbol.kind),
                Some(symbol)
            );
        }
    }

    #[test]
    fn defensively_rejects_non_exhaustive_variant_ast() {
        let subject = Expression {
            kind: ExpressionKind::Variable("None".to_owned()),
            span: Span::new(6, 10),
        };
        let arm_value = Expression {
            kind: ExpressionKind::Literal(ValueLiteral::Integer(0)),
            span: Span::new(21, 22),
        };
        let expression = Expression {
            kind: ExpressionKind::Match {
                subject: Box::new(subject),
                kind: MatchKind::Variants {
                    family: VariantFamily::Option,
                    arms: vec![crate::VariantArm {
                        variant: VariantName::None,
                        binding: None,
                        value: arm_value,
                        span: Span::new(13, 22),
                    }],
                },
            },
            span: Span::new(0, 24),
        };
        let program = Program {
            statements: vec![Statement {
                kind: StatementKind::Expression(expression),
                span: Span::new(0, 25),
            }],
        };

        let error = analyze(&program).expect_err("invalid AST should fail defensively");
        assert_eq!(error.code(), "K3005");
    }
}
