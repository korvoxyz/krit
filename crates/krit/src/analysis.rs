use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::{self, Write as _},
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
    limits::{MAX_NESTING_DEPTH, MAX_TREE_DEPTH, tree_depth_error},
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
    QueueConsumer,
    ScheduleHandler,
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
    QueueJob,
    ScheduleEvent,
    DatabaseTransaction,
    Secret,
    List(Arc<Self>),
    Record(Vec<RecordType>),
    Option(Arc<Self>),
    Result(Arc<Self>, Arc<Self>),
    Function(FunctionType),
    Variable(u32),
}

impl Type {
    pub fn record_fields(&self) -> Option<Vec<RecordType>> {
        match self {
            Self::Record(fields) => Some(fields.clone()),
            Self::HttpHeader => Some(public_contract_fields(&InferType::HttpHeader)),
            Self::HttpRequest => Some(public_contract_fields(&InferType::HttpRequest)),
            Self::HttpResponse => Some(public_contract_fields(&InferType::HttpResponse)),
            Self::LogField => Some(public_contract_fields(&InferType::LogField)),
            Self::QueueJob => Some(public_contract_fields(&InferType::QueueJob)),
            Self::ScheduleEvent => Some(public_contract_fields(&InferType::ScheduleEvent)),
            Self::Int
            | Self::Bool
            | Self::String
            | Self::Unit
            | Self::DatabaseTransaction
            | Self::Secret
            | Self::List(_)
            | Self::Option(_)
            | Self::Result(_, _)
            | Self::Function(_)
            | Self::Variable(_) => None,
        }
    }

    pub fn render_bounded(&self, max_bytes: usize) -> Option<String> {
        let mut writer = BoundedTypeWriter::new(max_bytes);
        render_type_bounded(self, &mut writer, 0).ok()?;
        Some(writer.finish())
    }
}

const MAX_TYPE_RENDER_DEPTH: usize = 256;

struct BoundedTypeWriter {
    output: String,
    max_bytes: usize,
}

impl BoundedTypeWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            output: String::new(),
            max_bytes,
        }
    }

    fn finish(self) -> String {
        self.output
    }
}

impl fmt::Write for BoundedTypeWriter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.output.len().saturating_add(value.len()) > self.max_bytes {
            return Err(fmt::Error);
        }
        self.output.push_str(value);
        Ok(())
    }
}

fn render_type_bounded(ty: &Type, writer: &mut BoundedTypeWriter, depth: usize) -> fmt::Result {
    if depth > MAX_TYPE_RENDER_DEPTH {
        return Err(fmt::Error);
    }
    match ty {
        Type::Int => writer.write_str("Int"),
        Type::Bool => writer.write_str("Bool"),
        Type::String => writer.write_str("String"),
        Type::Unit => writer.write_str("Unit"),
        Type::HttpHeader => writer.write_str("HttpHeader"),
        Type::HttpRequest => writer.write_str("HttpRequest"),
        Type::HttpResponse => writer.write_str("HttpResponse"),
        Type::LogField => writer.write_str("LogField"),
        Type::QueueJob => writer.write_str("QueueJob"),
        Type::ScheduleEvent => writer.write_str("ScheduleEvent"),
        Type::DatabaseTransaction => writer.write_str("DatabaseTransaction"),
        Type::Secret => writer.write_str("Secret"),
        Type::List(element) => {
            writer.write_str("List<")?;
            render_type_bounded(element, writer, depth + 1)?;
            writer.write_str(">")
        }
        Type::Record(fields) => {
            writer.write_str("Record { ")?;
            for (index, field) in fields.iter().enumerate() {
                if index > 0 {
                    writer.write_str(", ")?;
                }
                writer.write_str(field.name())?;
                writer.write_str(": ")?;
                render_type_bounded(field.ty(), writer, depth + 1)?;
            }
            writer.write_str(" }")
        }
        Type::Option(element) => {
            writer.write_str("Option<")?;
            render_type_bounded(element, writer, depth + 1)?;
            writer.write_str(">")
        }
        Type::Result(value, error) => {
            writer.write_str("Result<")?;
            render_type_bounded(value, writer, depth + 1)?;
            writer.write_str(", ")?;
            render_type_bounded(error, writer, depth + 1)?;
            writer.write_str(">")
        }
        Type::Function(function) => {
            writer.write_str("fn(")?;
            for (index, parameter) in function.parameters().iter().enumerate() {
                if index > 0 {
                    writer.write_str(", ")?;
                }
                render_type_bounded(parameter, writer, depth + 1)?;
            }
            writer.write_str(") -> ")?;
            render_type_bounded(function.return_type(), writer, depth + 1)?;
            writer.write_str(" effects {")?;
            for (index, effect) in function.effects().iter().enumerate() {
                if index > 0 {
                    writer.write_str(", ")?;
                }
                writer.write_str(effect.as_str())?;
            }
            writer.write_str("}")?;
            if !function.requirements().is_empty() {
                writer.write_str(" requirements {")?;
                for (index, requirement) in function.requirements().iter().enumerate() {
                    if index > 0 {
                        writer.write_str(", ")?;
                    }
                    writer.write_str(requirement.capability().as_str())?;
                    writer.write_str("(")?;
                    write!(writer, "{:?}", requirement.resource())?;
                    writer.write_str(")")?;
                }
                writer.write_str("}")?;
            }
            Ok(())
        }
        Type::Variable(id) => write!(writer, "'{}", type_variable_name(*id)),
    }
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
            Self::QueueJob => formatter.write_str("QueueJob"),
            Self::ScheduleEvent => formatter.write_str("ScheduleEvent"),
            Self::DatabaseTransaction => formatter.write_str("DatabaseTransaction"),
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
    fn new(name: impl Into<String>, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty: Arc::new(ty),
        }
    }

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
    CacheRead,
    CacheWrite,
    ConfigRead,
    DatabaseRead,
    DatabaseWrite,
    HttpRequest,
    IoStdout,
    ObjectRead,
    ObjectWrite,
    ObserveLog,
    QueueConsume,
    QueuePublish,
    ScheduleTrigger,
    SearchQuery,
    SearchVector,
    SecretRead,
    StateTransaction,
}

impl Effect {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AiInvoke => "ai.invoke",
            Self::CacheRead => "cache.read",
            Self::CacheWrite => "cache.write",
            Self::ConfigRead => "config.read",
            Self::DatabaseRead => "database.read",
            Self::DatabaseWrite => "database.write",
            Self::HttpRequest => "http.request",
            Self::IoStdout => "io.stdout",
            Self::ObjectRead => "object.read",
            Self::ObjectWrite => "object.write",
            Self::ObserveLog => "observe.log",
            Self::QueueConsume => "queue.consume",
            Self::QueuePublish => "queue.publish",
            Self::ScheduleTrigger => "schedule.trigger",
            Self::SearchQuery => "search.query",
            Self::SearchVector => "search.vector",
            Self::SecretRead => "secret.read",
            Self::StateTransaction => "state.transaction",
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

/// Analyzes `program` and returns its analysis or the first error.
///
/// Warnings are discarded; [`analyze_with_warnings`] performs the same
/// analysis and also returns them.
pub fn analyze(program: &Program) -> Result<Analysis, Diagnostic> {
    analyze_with_warnings(program).result
}

/// The outcome of analyzing a program together with its warnings.
///
/// Warnings never decide the outcome: a program that analysis accepts still
/// produces an [`Analysis`] when it has warnings, and a program that analysis
/// rejects still reports every warning found before its error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisReport {
    /// The analysis, or the first error that rejected the program.
    pub result: Result<Analysis, Diagnostic>,
    /// Every warning analysis found, ordered by source span and then code.
    pub warnings: Vec<Diagnostic>,
}

/// Analyzes `program` exactly as [`analyze`] does and also returns the
/// warnings found, whether analysis succeeds or fails.
pub fn analyze_with_warnings(program: &Program) -> AnalysisReport {
    let mut analyzer = Analyzer::new();
    let result = analyzer.program(program);
    let mut warnings = analyzer.warnings;
    warnings.sort_by(|left, right| {
        left.span()
            .cmp(&right.span())
            .then_with(|| left.code().cmp(right.code()))
    });
    AnalysisReport { result, warnings }
}

/// The discard name. Any number of bindings in one scope may use it, and
/// reading it is warning K2003 in edition 2026.
const DISCARD: &str = "_";

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
    QueueJob,
    ScheduleEvent,
    DatabaseTransaction,
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
    /// Nested expression and block analyses currently open.
    depth: usize,
    /// The resolved height of each root type variable: how many type
    /// constructors its binding nests with every type variable inside it
    /// resolved, or 0 while it is unbound. Kept current whenever a binding
    /// changes and bounded by [`MAX_NESTING_DEPTH`].
    type_heights: Vec<usize>,
    /// For each type variable, the type variables whose bindings mention it,
    /// so a change to its height reaches every type that contains it.
    type_dependents: Vec<BTreeSet<TypeVariable>>,
    /// A binding that would have nested an inferred type too deeply; it takes
    /// precedence over the generic mismatch reported by the failing
    /// unification.
    type_depth_error: Option<Diagnostic>,
    /// Warnings found so far, in analysis order. They never stop analysis.
    warnings: Vec<Diagnostic>,
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
            depth: 0,
            type_heights: Vec::new(),
            type_dependents: Vec::new(),
            type_depth_error: None,
            warnings: Vec::new(),
        }
    }

    fn program(&mut self, program: &Program) -> Result<Analysis, Diagnostic> {
        let mut effects = InferEffects::default();
        for statement in &program.statements {
            effects.union(self.statement(statement)?);
        }
        self.validate_host_builtin_references()?;
        self.validate_constraints()?;

        let expanded_effects = self.expanded_effects();
        let expanded_requirements = self.expanded_requirements();
        let mut normalizer = TypeNormalizer::new(self, &expanded_effects, &expanded_requirements);
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

    /// Validates one top-level entrypoint declaration and returns its symbol
    /// kind plus the exact capability requirement the host must grant.
    fn entrypoint_declaration(
        &self,
        statement: &Statement,
        parameters: &[Parameter],
        return_type: Option<&TypeAnnotation>,
    ) -> Result<Option<(SymbolKind, Option<CapabilityRequirement>)>, Diagnostic> {
        let (keyword, symbol_kind, parameter_kind, resource) = match &statement.kind {
            StatementKind::Webhook { .. } => (
                "webhook",
                SymbolKind::Webhook,
                EntrypointParameter::Request,
                None,
            ),
            StatementKind::QueueConsumer {
                queue, queue_span, ..
            } => (
                "queue",
                SymbolKind::QueueConsumer,
                EntrypointParameter::QueueJob,
                Some((Effect::QueueConsume, queue.as_str(), *queue_span)),
            ),
            StatementKind::ScheduleHandler {
                schedule,
                schedule_span,
                ..
            } => (
                "schedule",
                SymbolKind::ScheduleHandler,
                EntrypointParameter::ScheduleEvent,
                Some((Effect::ScheduleTrigger, schedule.as_str(), *schedule_span)),
            ),
            _ => return Ok(None),
        };
        if self.scopes.len() != 1 {
            return Err(Diagnostic::new(
                "K1004",
                format!("`{keyword}` declarations are only allowed at the top level"),
                statement.span,
            ));
        }
        let valid_parameter = parameters.len() == 1
            && parameters[0]
                .annotation
                .as_ref()
                .is_some_and(|annotation| parameter_kind.matches(&annotation.kind));
        let valid_return =
            return_type.is_some_and(|annotation| parameter_kind.matches_return(annotation));
        if !valid_parameter || !valid_return {
            return Err(Diagnostic::new(
                "K3007",
                format!(
                    "{keyword} signature must be exactly `{}`",
                    parameter_kind.signature()
                ),
                statement.span,
            ));
        }
        let requirement = match resource {
            Some((effect, name, span)) => {
                if !is_valid_resource_name(name) {
                    return Err(Diagnostic::new(
                        "K3008",
                        format!(
                            "{keyword} name must use 1-64 lowercase letters, digits, `.` or `-`, without leading/trailing punctuation or `..`/`--`"
                        ),
                        span,
                    ));
                }
                Some(CapabilityRequirement::new(effect, name))
            }
            None => None,
        };
        Ok(Some((symbol_kind, requirement)))
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
            }
            | StatementKind::QueueConsumer {
                name,
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::ScheduleHandler {
                name,
                parameters,
                return_type,
                body,
                ..
            } => {
                let entrypoint =
                    self.entrypoint_declaration(statement, parameters, return_type.as_ref())?;
                let symbol_kind = entrypoint
                    .as_ref()
                    .map_or(SymbolKind::Function, |(kind, _)| *kind);
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
                if let Some(requirement) = entrypoint.and_then(|(_, requirement)| requirement) {
                    let definition = &mut self.effect_definitions[effect as usize];
                    definition.direct.insert(requirement.capability().clone());
                    definition.direct_requirements.insert(requirement);
                }
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

    /// Counts one level of analyzer recursion.
    ///
    /// The parser's structural limit keeps every source program within
    /// [`MAX_TREE_DEPTH`], and `else if` and binary-operator chains are
    /// analyzed iteratively, so this only rejects hand-built syntax trees that
    /// bypassed the parser.
    fn enter(&mut self, span: Span) -> Result<(), Diagnostic> {
        if self.depth >= MAX_TREE_DEPTH {
            return Err(tree_depth_error(span));
        }
        self.depth += 1;
        Ok(())
    }

    fn block(&mut self, block: &Block) -> Result<ExpressionInfo, Diagnostic> {
        self.enter(block.span)?;
        let info = self.block_contents(block);
        self.depth -= 1;
        info
    }

    fn block_contents(&mut self, block: &Block) -> Result<ExpressionInfo, Diagnostic> {
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
        self.enter(expression.span)?;
        let info = self.expression_contents(expression);
        self.depth -= 1;
        info
    }

    fn expression_contents(
        &mut self,
        expression: &Expression,
    ) -> Result<ExpressionInfo, Diagnostic> {
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
                if name == DISCARD && matches!(resolution, ResolvedName::Symbol(_)) {
                    self.warnings.push(Diagnostic::warning(
                        "K2003",
                        "`_` is a discard; reading it will be an error in edition 2027",
                        expression.span,
                    ));
                }
                if matches!(
                    resolution,
                    ResolvedName::Builtin(
                        Builtin::AiInvoke
                            | Builtin::ConfigString
                            | Builtin::Secret
                            | Builtin::HttpRequest
                            | Builtin::LogInfo
                            | Builtin::LogError
                            | Builtin::StateGet
                            | Builtin::StatePut
                            | Builtin::StateDelete
                            | Builtin::CheckpointGet
                            | Builtin::CheckpointPut
                            | Builtin::ReplayHttp
                            | Builtin::ReplayAi
                            | Builtin::QueuePublish
                            | Builtin::ObjectGet
                            | Builtin::ObjectPut
                            | Builtin::ObjectDelete
                            | Builtin::DatabaseBeginRead
                            | Builtin::DatabaseBeginWrite
                            | Builtin::DatabaseQuery
                            | Builtin::DatabaseExecute
                            | Builtin::DatabaseCommit
                            | Builtin::DatabaseRollback
                            | Builtin::CacheGet
                            | Builtin::CachePut
                            | Builtin::CacheDelete
                            | Builtin::SearchQuery
                            | Builtin::VectorSearch
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
            ExpressionKind::If { .. } => self.if_chain(expression),
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
            ExpressionKind::Binary { .. } => self.binary_chain(expression),
        }?;
        self.record_expression(expression.span, &info, resolved_name);
        Ok(info)
    }

    fn record_expression(
        &mut self,
        span: Span,
        info: &ExpressionInfo,
        resolved_name: Option<ResolvedName>,
    ) {
        self.expressions.push(PendingExpression {
            span,
            ty: info.ty.clone(),
            effects: info.effects.clone(),
            resolved_name,
        });
    }

    /// Analyzes an `if` expression and its whole `else if` chain.
    ///
    /// The chain is walked iteratively so its length never deepens analyzer
    /// recursion. Conditions and consequents are analyzed in source order and
    /// branch types are unified from the innermost link outward, exactly as a
    /// recursive walk would.
    fn if_chain(&mut self, expression: &Expression) -> Result<ExpressionInfo, Diagnostic> {
        let mut links = Vec::new();
        let mut current = expression;
        let mut info = loop {
            let ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } = &current.kind
            else {
                break self.expression(current)?;
            };
            let condition_source_span = condition.span;
            let condition = self.expression(condition)?;
            self.unify(
                InferType::Bool,
                condition.ty,
                condition_source_span,
                "if condition",
            )?;
            let consequent = self.block(consequent)?;
            links.push((current.span, condition.effects, consequent));
            current = alternative;
        };
        while let Some((span, condition_effects, consequent)) = links.pop() {
            let ty = self.unify(consequent.ty, info.ty, span, "if branches")?;
            let mut effects = condition_effects;
            effects.union(consequent.effects);
            effects.union(info.effects);
            info = ExpressionInfo { ty, effects };
            // The outermost link is recorded by `expression`.
            if !links.is_empty() {
                self.record_expression(span, &info, None);
            }
        }
        Ok(info)
    }

    /// Analyzes a left-nested chain of binary operators such as `a + b + c`.
    ///
    /// The left spine is walked iteratively so a long chain never deepens
    /// analyzer recursion; operands are still analyzed left to right.
    fn binary_chain(&mut self, expression: &Expression) -> Result<ExpressionInfo, Diagnostic> {
        let mut spine = Vec::new();
        let mut current = expression;
        while let ExpressionKind::Binary { left, .. } = &current.kind {
            spine.push(current);
            current = left;
        }
        let mut info = self.expression(current)?;
        while let Some(node) = spine.pop() {
            let ExpressionKind::Binary {
                operator, right, ..
            } = &node.kind
            else {
                unreachable!("the binary spine holds only binary expressions")
            };
            let right = self.expression(right)?;
            info = self.binary(info, *operator, right, node.span)?;
            // The outermost operator is recorded by `expression`.
            if !spine.is_empty() {
                self.record_expression(node.span, &info, None);
            }
        }
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
        let direct_requirements = if let Some(builtin) = direct_builtin {
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
                    vec![CapabilityRequirement::new(
                        Effect::AiInvoke,
                        resource.clone(),
                    )]
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
                    vec![CapabilityRequirement::new(
                        if builtin == Builtin::ConfigString {
                            Effect::ConfigRead
                        } else {
                            Effect::SecretRead
                        },
                        resource.clone(),
                    )]
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
                    vec![CapabilityRequirement::new(
                        Effect::HttpRequest,
                        normalized.as_str(),
                    )]
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
                    Vec::new()
                }
                (Builtin::StateGet | Builtin::StateDelete, [store, _])
                | (Builtin::StatePut, [store, _, _]) => {
                    let store = require_canonical_literal(store, builtin, "durable state store")?;
                    vec![CapabilityRequirement::new(Effect::StateTransaction, store)]
                }
                (Builtin::CheckpointGet, [store, checkpoint])
                | (Builtin::CheckpointPut, [store, checkpoint, _]) => {
                    let store = require_canonical_literal(store, builtin, "durable state store")?;
                    require_canonical_literal(checkpoint, builtin, "workflow checkpoint")?;
                    vec![CapabilityRequirement::new(Effect::StateTransaction, store)]
                }
                (Builtin::ReplayHttp, [store, operation, origin, _]) => {
                    let store = require_canonical_literal(store, builtin, "durable state store")?;
                    require_canonical_literal(operation, builtin, "replay operation")?;
                    let ExpressionKind::Literal(ValueLiteral::String(origin_value)) = &origin.kind
                    else {
                        return Err(Diagnostic::new(
                            "K3008",
                            "`replay_http` requires a direct normalized exact-origin literal",
                            origin.span,
                        ));
                    };
                    let normalized = HttpOrigin::parse_exact(origin_value).map_err(|error| {
                        Diagnostic::new(
                            "K3008",
                            format!("invalid `replay_http` origin: {error}"),
                            origin.span,
                        )
                    })?;
                    vec![
                        CapabilityRequirement::new(Effect::StateTransaction, store),
                        CapabilityRequirement::new(Effect::HttpRequest, normalized.as_str()),
                    ]
                }
                (Builtin::QueuePublish, [queue, _]) => {
                    let queue = require_canonical_literal(queue, builtin, "durable queue")?;
                    vec![CapabilityRequirement::new(Effect::QueuePublish, queue)]
                }
                (Builtin::ObjectGet | Builtin::ObjectDelete, [bucket, _])
                | (Builtin::ObjectPut, [bucket, _, _]) => {
                    let effect = if builtin == Builtin::ObjectGet {
                        Effect::ObjectRead
                    } else {
                        Effect::ObjectWrite
                    };
                    let bucket = require_canonical_literal(bucket, builtin, "object bucket")?;
                    vec![CapabilityRequirement::new(effect, bucket)]
                }
                (Builtin::CacheGet | Builtin::CacheDelete, [namespace, _])
                | (Builtin::CachePut, [namespace, _, _, _]) => {
                    let effect = if builtin == Builtin::CacheGet {
                        Effect::CacheRead
                    } else {
                        Effect::CacheWrite
                    };
                    let namespace =
                        require_canonical_literal(namespace, builtin, "cache namespace")?;
                    vec![CapabilityRequirement::new(effect, namespace)]
                }
                (Builtin::SearchQuery | Builtin::VectorSearch, [index, _, _]) => {
                    let effect = if builtin == Builtin::SearchQuery {
                        Effect::SearchQuery
                    } else {
                        Effect::SearchVector
                    };
                    let index = require_canonical_literal(index, builtin, "search index")?;
                    vec![CapabilityRequirement::new(effect, index)]
                }
                (Builtin::DatabaseBeginRead | Builtin::DatabaseBeginWrite, [database]) => {
                    let effect = if builtin == Builtin::DatabaseBeginRead {
                        Effect::DatabaseRead
                    } else {
                        Effect::DatabaseWrite
                    };
                    let database =
                        require_canonical_literal(database, builtin, "application database")?;
                    vec![CapabilityRequirement::new(effect, database)]
                }
                (Builtin::DatabaseQuery | Builtin::DatabaseExecute, [_, statement, _]) => {
                    // The database authority comes from the transaction handle;
                    // the statement identity must still be a direct literal so
                    // the catalog entry is auditable in source.
                    require_canonical_literal(statement, builtin, "database statement")?;
                    Vec::new()
                }
                (Builtin::ReplayAi, [store, operation, adapter, _]) => {
                    let store = require_canonical_literal(store, builtin, "durable state store")?;
                    require_canonical_literal(operation, builtin, "replay operation")?;
                    let adapter = require_canonical_literal(adapter, builtin, "AI adapter")?;
                    vec![
                        CapabilityRequirement::new(Effect::StateTransaction, store),
                        CapabilityRequirement::new(Effect::AiInvoke, adapter),
                    ]
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
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
        let replay_builtin = matches!(
            direct_builtin,
            Some(Builtin::ReplayHttp | Builtin::ReplayAi)
        );
        for requirement in direct_requirements {
            if !replay_builtin || requirement.capability() == &Effect::StateTransaction {
                effects.direct.insert(requirement.capability().clone());
            }
            effects.direct_requirements.insert(requirement);
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
            // A transaction handle may appear only as the first argument of a
            // database operation; every other position rejects it.
            let approved_transaction_use = matches!(
                direct_builtin,
                Some(
                    Builtin::DatabaseQuery
                        | Builtin::DatabaseExecute
                        | Builtin::DatabaseCommit
                        | Builtin::DatabaseRollback
                ) if index == 0
            );
            if !approved_secret_use && !approved_transaction_use {
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
                head_span,
                tail_name,
                tail_span,
                cons_case,
            } => {
                if head_name == tail_name && head_name != DISCARD {
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
                self.define_symbol(
                    head_name,
                    element.clone(),
                    *head_span,
                    SymbolKind::Match,
                    false,
                );
                self.define_symbol(
                    tail_name,
                    InferType::List(Box::new(element)),
                    *tail_span,
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
        left: ExpressionInfo,
        operator: BinaryOperator,
        right: ExpressionInfo,
        span: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
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
            TypeKind::QueueJob => InferType::QueueJob,
            TypeKind::ScheduleEvent => InferType::ScheduleEvent,
            TypeKind::DatabaseTransaction => InferType::DatabaseTransaction,
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
                    self.set_binding(variable, widened, span)?;
                    Ok(field_type)
                } else {
                    let field_type = self.fresh_type();
                    self.set_binding(
                        variable,
                        InferType::Record {
                            fields: BTreeMap::from([(field.to_owned(), field_type.clone())]),
                            open: true,
                        },
                        span,
                    )?;
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
            | InferType::LogField
            | InferType::QueueJob
            | InferType::ScheduleEvent) => {
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
        match self.unify_inner(expected.clone(), actual.clone(), span) {
            Ok(unified) => Ok(unified),
            Err(()) => Err(self.type_depth_error.take().unwrap_or_else(|| {
                Diagnostic::new(
                    "K3001",
                    format!(
                        "{context} type mismatch: expected `{}`, found `{}`",
                        self.render_type(&expected),
                        self.render_type(&actual)
                    ),
                    span,
                )
            })),
        }
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
            (InferType::DatabaseTransaction, InferType::DatabaseTransaction) => {
                Ok(InferType::DatabaseTransaction)
            }
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
                self.alias_variable(alias, root, span).map_err(|_| ())?;
                Ok(InferType::Variable(root))
            }
            (Some(binding), None) => {
                if self.occurs(right, &binding) {
                    return Err(());
                }
                self.alias_variable(right, left, span).map_err(|_| ())?;
                Ok(InferType::Variable(left))
            }
            (None, Some(binding)) => {
                if self.occurs(left, &binding) {
                    return Err(());
                }
                self.alias_variable(left, right, span).map_err(|_| ())?;
                Ok(InferType::Variable(right))
            }
            (Some(left_binding), Some(right_binding)) => {
                // Both bindings are detached while they are unified, so the
                // occurs checks made meanwhile cannot see through `left` or
                // `right`. A binding that mentions the other variable would
                // make the merged type infinite, and unifying it could bind a
                // variable inside one binding to a type containing the other,
                // leaving a cycle that every later walk over types would
                // follow forever (`let e = []; let w = [[e]]; e == w;`).
                // Reject it first; otherwise neither variable is reachable
                // from either binding, and the checks below stay exact.
                if self.occurs(left, &right_binding) || self.occurs(right, &left_binding) {
                    return Err(());
                }
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
                self.alias_variable(alias, root, span).map_err(|_| ())?;
                self.set_binding(root, unified, span).map_err(|_| ())?;
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
        // A depth failure stays recorded in `type_depth_error` (cloned, not
        // taken) so an enclosing `unify` still reports it instead of a
        // generic mismatch.
        if let InferType::Variable(other) = ty {
            return self.unify_variables(variable, other, span).map_err(|()| {
                self.type_depth_error.clone().unwrap_or_else(|| {
                    Diagnostic::new(
                        "K3001",
                        format!("{context} would create an infinite type"),
                        span,
                    )
                })
            });
        }
        if let Some(existing) = self.substitutions.get(&variable).cloned() {
            let Ok(unified) = self.unify_inner(existing, ty.clone(), span) else {
                return Err(self.type_depth_error.clone().unwrap_or_else(|| {
                    Diagnostic::new(
                        "K3001",
                        format!(
                            "{context} type mismatch: incompatible with `{}`",
                            self.render_type(&ty)
                        ),
                        span,
                    )
                }));
            };
            self.set_binding(variable, unified, span)?;
            return Ok(InferType::Variable(variable));
        }
        if self.occurs(variable, &ty) {
            return Err(Diagnostic::new(
                "K3001",
                format!("{context} would create an infinite type"),
                span,
            ));
        }
        self.set_binding(variable, ty, span)?;
        Ok(InferType::Variable(variable))
    }

    /// Makes `ty` the binding of the root type variable `variable`.
    ///
    /// Every inferred type deeper than its syntax grows through bindings, so
    /// this is where inferred types are bounded. Binding a variable can deepen
    /// not only its own type but every type already containing it (`top`
    /// below grows when `next` is bound later), so the resolved heights of
    /// `variable` and of everything depending on it are refreshed, and the
    /// binding fails with `K1005` if any of them would nest more than
    /// [`MAX_NESTING_DEPTH`] type constructors:
    ///
    /// ```text
    /// let next = [];
    /// let top = [[next]];
    /// let deeper = next == [[[]]];
    /// ```
    fn set_binding(
        &mut self,
        variable: TypeVariable,
        ty: InferType,
        span: Span,
    ) -> Result<(), Diagnostic> {
        for mentioned in self.mentioned_roots(&ty) {
            self.type_dependents[mentioned as usize].insert(variable);
        }
        self.substitutions.insert(variable, ty);
        self.refresh_type_heights(vec![variable], span)
    }

    /// Makes `alias` an alias of the root `root`, so every type mentioning
    /// `alias` now resolves through `root`'s binding.
    ///
    /// `root` keeps the larger of the two cached heights, and the types that
    /// mention either variable are rechecked against it. Rechecking the types
    /// that mention `root` matters when both variables were solved:
    /// `unify_variables` detaches both bindings while it unifies them, so
    /// refreshes during that unification stop at both variables, and their
    /// merged binding may be no taller than `alias` already was. Those types
    /// would otherwise keep a stale height and could grow past
    /// [`MAX_NESTING_DEPTH`] unnoticed (`top` below gains seven levels):
    ///
    /// ```text
    /// let shallow = [[[]]];
    /// let top = [[[[shallow]]]];
    /// let deep = [[[[[[[[[[1]]]]]]]]]];
    /// let grows = shallow == deep;
    /// ```
    fn alias_variable(
        &mut self,
        alias: TypeVariable,
        root: TypeVariable,
        span: Span,
    ) -> Result<(), Diagnostic> {
        self.type_parents[alias as usize] = root;
        let alias_height = self.type_heights[alias as usize];
        let mut pending = Vec::new();
        if alias_height > self.type_heights[root as usize] {
            self.type_heights[root as usize] = alias_height;
            pending.extend(self.type_dependents[root as usize].iter().copied());
        }
        let dependents = std::mem::take(&mut self.type_dependents[alias as usize]);
        pending.extend(dependents.iter().copied());
        self.type_dependents[root as usize].extend(dependents);
        self.refresh_type_heights(pending, span)
    }

    /// Recomputes the resolved heights of the `pending` type variables and,
    /// whenever one grows, of every type variable whose binding mentions it.
    ///
    /// Heights only grow and are bounded, so each variable grows at most
    /// [`MAX_NESTING_DEPTH`] times and the propagation stays linear in the
    /// size of the bindings. Fails with `K1005` at `span`, the inference step
    /// being checked, as soon as any height would exceed the bound.
    fn refresh_type_heights(
        &mut self,
        mut pending: Vec<TypeVariable>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        while let Some(variable) = pending.pop() {
            let root = self.root_variable(variable);
            let Some(binding) = self.substitutions.get(&root) else {
                continue;
            };
            let height = self.resolved_height(binding, 0);
            if height <= self.type_heights[root as usize] {
                continue;
            }
            if height > MAX_NESTING_DEPTH {
                let error = Diagnostic::new(
                    "K1005",
                    format!(
                        "nesting depth limit exceeded: inferred types may nest at most \
                         {MAX_NESTING_DEPTH} levels"
                    ),
                    span,
                );
                self.type_depth_error = Some(error.clone());
                return Err(error);
            }
            self.type_heights[root as usize] = height;
            pending.extend(self.type_dependents[root as usize].iter().copied());
        }
        Ok(())
    }

    /// Returns how many type constructors `ty` nests, with each type variable
    /// counted at its cached resolved height, once `depth` constructors
    /// already enclose it.
    ///
    /// The walk stops below `MAX_NESTING_DEPTH + 1` enclosing constructors and
    /// reports at least that height for anything it cuts off, so its recursion
    /// is bounded and a result above the limit is exact enough to reject.
    fn resolved_height(&self, ty: &InferType, depth: usize) -> usize {
        const CUTOFF: usize = MAX_NESTING_DEPTH + 1;
        if depth >= CUTOFF {
            return CUTOFF;
        }
        let nested = |children: &mut dyn Iterator<Item = &InferType>| {
            1 + children
                .map(|child| self.resolved_height(child, depth + 1))
                .max()
                .unwrap_or(0)
        };
        match ty {
            InferType::Variable(variable) => {
                self.type_heights[self.root_variable(*variable) as usize]
            }
            InferType::List(element) | InferType::Option(element) => {
                nested(&mut std::iter::once(element.as_ref()))
            }
            InferType::Result(value, error) => {
                nested(&mut [value.as_ref(), error.as_ref()].into_iter())
            }
            InferType::Record { fields, .. } => nested(&mut fields.values()),
            InferType::Function {
                parameters,
                return_type,
                ..
            } => nested(
                &mut parameters
                    .iter()
                    .chain(std::iter::once(return_type.as_ref())),
            ),
            InferType::Int
            | InferType::Bool
            | InferType::String
            | InferType::Unit
            | InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField
            | InferType::QueueJob
            | InferType::ScheduleEvent
            | InferType::DatabaseTransaction
            | InferType::Secret => 0,
        }
    }

    /// Returns the root of every type variable `ty` mentions directly, without
    /// following bindings.
    fn mentioned_roots(&self, ty: &InferType) -> BTreeSet<TypeVariable> {
        let mut roots = BTreeSet::new();
        let mut pending = vec![ty];
        while let Some(ty) = pending.pop() {
            match ty {
                InferType::Variable(variable) => {
                    roots.insert(self.root_variable(*variable));
                }
                InferType::List(element) | InferType::Option(element) => pending.push(element),
                InferType::Result(value, error) => {
                    pending.push(value);
                    pending.push(error);
                }
                InferType::Record { fields, .. } => pending.extend(fields.values()),
                InferType::Function {
                    parameters,
                    return_type,
                    ..
                } => {
                    pending.extend(parameters);
                    pending.push(return_type);
                }
                InferType::Int
                | InferType::Bool
                | InferType::String
                | InferType::Unit
                | InferType::HttpHeader
                | InferType::HttpRequest
                | InferType::HttpResponse
                | InferType::LogField
                | InferType::QueueJob
                | InferType::ScheduleEvent
                | InferType::DatabaseTransaction
                | InferType::Secret => {}
            }
        }
        roots
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
            | InferType::QueueJob
            | InferType::ScheduleEvent
            | InferType::DatabaseTransaction
            | InferType::Secret => false,
        }
    }

    fn root_variable(&self, variable: TypeVariable) -> TypeVariable {
        // Iterative: alias chains grow with flat source such as a long list
        // of distinct unannotated parameters.
        let mut current = variable;
        loop {
            let parent = self.type_parents[current as usize];
            if parent == current {
                return current;
            }
            current = parent;
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
                        && !self.contains_transaction(&constraint.ty, &mut BTreeSet::new())
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
                        && !self.contains_transaction(&constraint.ty, &mut BTreeSet::new())
                }
            };
            if !valid {
                if self.contains_transaction(&constraint.ty, &mut BTreeSet::new()) {
                    let operation = match constraint.kind {
                        ConstraintKind::Comparable => "compared",
                        ConstraintKind::JsonValue => "encoded as JSON",
                        ConstraintKind::OpaqueArgument => {
                            "passed outside the first argument of a database operation"
                        }
                        ConstraintKind::Printable => "printed",
                        ConstraintKind::StructuralValue => "placed into ordinary structural data",
                        ConstraintKind::Addable => "used by `+`",
                    };
                    return Err(Diagnostic::new(
                        "K3010",
                        format!("opaque `DatabaseTransaction` values cannot be {operation}"),
                        constraint.span,
                    ));
                }
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
            | InferType::LogField
            | InferType::QueueJob
            | InferType::ScheduleEvent => true,
            InferType::DatabaseTransaction | InferType::Secret => false,
        }
    }

    /// Whether an opaque database transaction handle is reachable in `ty`.
    ///
    /// Handles mirror `Secret`: they are host-owned, never serializable, and
    /// may appear only in the approved argument position of a database
    /// built-in.
    fn contains_transaction(&self, ty: &InferType, visited: &mut BTreeSet<TypeVariable>) -> bool {
        match ty {
            InferType::DatabaseTransaction => true,
            InferType::List(element) | InferType::Option(element) => {
                self.contains_transaction(element, visited)
            }
            InferType::Record { fields, .. } => fields
                .values()
                .any(|ty| self.contains_transaction(ty, visited)),
            InferType::Result(value, error) => {
                self.contains_transaction(value, visited)
                    || self.contains_transaction(error, visited)
            }
            InferType::Function {
                parameters,
                return_type,
                ..
            } => {
                parameters
                    .iter()
                    .any(|ty| self.contains_transaction(ty, visited))
                    || self.contains_transaction(return_type, visited)
            }
            InferType::Variable(variable) => {
                let variable = self.root_variable(*variable);
                if !visited.insert(variable) {
                    return false;
                }
                self.substitutions
                    .get(&variable)
                    .is_some_and(|ty| self.contains_transaction(ty, visited))
            }
            InferType::Int
            | InferType::Bool
            | InferType::String
            | InferType::Unit
            | InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField
            | InferType::QueueJob
            | InferType::ScheduleEvent
            | InferType::Secret => false,
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
            | InferType::LogField
            | InferType::QueueJob
            | InferType::ScheduleEvent
            | InferType::DatabaseTransaction => false,
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
            Builtin::StateGet | Builtin::CheckpointGet => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::StateTransaction);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Option(Box::new(InferType::String))),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::StatePut | Builtin::CheckpointPut => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::StateTransaction);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::StateDelete => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::StateTransaction);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::ReplayHttp => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::StateTransaction);
                InferType::Function {
                    parameters: vec![
                        InferType::String,
                        InferType::String,
                        InferType::String,
                        InferType::HttpRequest,
                    ],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::HttpResponse),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::ReplayAi => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::StateTransaction);
                InferType::Function {
                    parameters: vec![
                        InferType::String,
                        InferType::String,
                        InferType::String,
                        InferType::String,
                    ],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::String),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::QueuePublish => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::QueuePublish);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::String),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::CacheGet => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::CacheRead);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Option(Box::new(InferType::String))),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::CachePut => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::CacheWrite);
                InferType::Function {
                    parameters: vec![
                        InferType::String,
                        InferType::String,
                        InferType::String,
                        InferType::Int,
                    ],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::CacheDelete => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::CacheWrite);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::SearchQuery | Builtin::VectorSearch => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize].direct.insert(
                    if builtin == Builtin::SearchQuery {
                        Effect::SearchQuery
                    } else {
                        Effect::SearchVector
                    },
                );
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String, InferType::Int],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::String),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::ObjectGet => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::ObjectRead);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Option(Box::new(InferType::String))),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::ObjectPut => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::ObjectWrite);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::ObjectDelete => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize]
                    .direct
                    .insert(Effect::ObjectWrite);
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::Unit),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::DatabaseBeginRead | Builtin::DatabaseBeginWrite => {
                let effect = self.fresh_effect();
                self.effect_definitions[effect as usize].direct.insert(
                    if builtin == Builtin::DatabaseBeginRead {
                        Effect::DatabaseRead
                    } else {
                        Effect::DatabaseWrite
                    },
                );
                InferType::Function {
                    parameters: vec![InferType::String],
                    return_type: Box::new(InferType::Result(
                        Box::new(InferType::DatabaseTransaction),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::DatabaseQuery | Builtin::DatabaseExecute => {
                // Authority already came from the transaction, so these carry no
                // additional effect of their own.
                let effect = self.fresh_effect();
                InferType::Function {
                    parameters: vec![
                        InferType::DatabaseTransaction,
                        InferType::String,
                        InferType::List(Box::new(InferType::String)),
                    ],
                    return_type: Box::new(InferType::Result(
                        Box::new(if builtin == Builtin::DatabaseQuery {
                            InferType::String
                        } else {
                            InferType::Int
                        }),
                        Box::new(InferType::String),
                    )),
                    effect,
                }
            }
            Builtin::DatabaseCommit | Builtin::DatabaseRollback => {
                let effect = self.fresh_effect();
                InferType::Function {
                    parameters: vec![InferType::DatabaseTransaction],
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

    /// Rejects a declaration that reuses a built-in name or a name already
    /// declared in the current scope. The discard name `_` may be declared any
    /// number of times; reading it refers to the most recent declaration.
    fn ensure_name_available(&self, name: &str, span: Span) -> Result<(), Diagnostic> {
        if Builtin::from_name(name).is_some()
            || (name != DISCARD
                && self
                    .scopes
                    .last()
                    .is_some_and(|scope| scope.contains_key(name)))
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
        self.type_heights.push(0);
        self.type_dependents.push(BTreeSet::new());
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
            InferType::QueueJob => Arc::new(Type::QueueJob),
            InferType::ScheduleEvent => Arc::new(Type::ScheduleEvent),
            InferType::DatabaseTransaction => Arc::new(Type::DatabaseTransaction),
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
        InferType::QueueJob => BTreeMap::from([
            ("attempt".to_owned(), InferType::Int),
            ("body".to_owned(), InferType::String),
            ("id".to_owned(), InferType::String),
            ("maxAttempts".to_owned(), InferType::Int),
            ("queue".to_owned(), InferType::String),
        ]),
        InferType::ScheduleEvent => BTreeMap::from([
            ("attempt".to_owned(), InferType::Int),
            ("firedAtMillis".to_owned(), InferType::Int),
            ("id".to_owned(), InferType::String),
            ("maxAttempts".to_owned(), InferType::Int),
            ("schedule".to_owned(), InferType::String),
            ("scheduledAtMillis".to_owned(), InferType::Int),
        ]),
        _ => unreachable!("only built-in record contract types have structural aliases"),
    };
    InferType::Record {
        fields,
        open: false,
    }
}

fn public_contract_fields(nominal: &InferType) -> Vec<RecordType> {
    let InferType::Record { fields, .. } = contract_record(nominal) else {
        unreachable!("built-in record contracts normalize to records")
    };
    fields
        .into_iter()
        .map(|(name, ty)| RecordType::new(name, public_contract_type(ty)))
        .collect()
}

fn public_contract_type(ty: InferType) -> Type {
    match ty {
        InferType::Int => Type::Int,
        InferType::String => Type::String,
        InferType::HttpHeader => Type::HttpHeader,
        InferType::List(element) => Type::List(Arc::new(public_contract_type(*element))),
        _ => unreachable!("built-in record fields use concrete public contract types"),
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
            | Builtin::LogError
            | Builtin::StateGet
            | Builtin::StatePut
            | Builtin::StateDelete
            | Builtin::CheckpointGet
            | Builtin::CheckpointPut
            | Builtin::ReplayHttp
            | Builtin::ReplayAi
            | Builtin::QueuePublish
            | Builtin::ObjectGet
            | Builtin::ObjectPut
            | Builtin::ObjectDelete
            | Builtin::DatabaseBeginRead
            | Builtin::DatabaseBeginWrite
            | Builtin::DatabaseQuery
            | Builtin::DatabaseExecute
            | Builtin::DatabaseCommit
            | Builtin::DatabaseRollback
            | Builtin::CacheGet
            | Builtin::CachePut
            | Builtin::CacheDelete
            | Builtin::SearchQuery
            | Builtin::VectorSearch),
        ) => Some(builtin),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntrypointParameter {
    Request,
    QueueJob,
    ScheduleEvent,
}

impl EntrypointParameter {
    const fn matches(self, kind: &TypeKind) -> bool {
        match self {
            Self::Request => matches!(kind, TypeKind::HttpRequest),
            Self::QueueJob => matches!(kind, TypeKind::QueueJob),
            Self::ScheduleEvent => matches!(kind, TypeKind::ScheduleEvent),
        }
    }

    fn matches_return(self, annotation: &TypeAnnotation) -> bool {
        match self {
            Self::Request => matches!(annotation.kind, TypeKind::HttpResponse),
            Self::QueueJob | Self::ScheduleEvent => matches!(
                &annotation.kind,
                TypeKind::Result(value, error)
                    if matches!(value.kind, TypeKind::String)
                        && matches!(error.kind, TypeKind::String)
            ),
        }
    }

    const fn signature(self) -> &'static str {
        match self {
            Self::Request => "(request: HttpRequest) -> HttpResponse",
            Self::QueueJob => "(job: QueueJob) -> Result<String, String>",
            Self::ScheduleEvent => "(event: ScheduleEvent) -> Result<String, String>",
        }
    }
}

fn require_canonical_literal(
    expression: &Expression,
    builtin: Builtin,
    kind: &str,
) -> Result<String, Diagnostic> {
    let ExpressionKind::Literal(ValueLiteral::String(value)) = &expression.kind else {
        return Err(Diagnostic::new(
            "K3008",
            format!(
                "`{}` requires a direct string-literal {kind}",
                builtin.as_str()
            ),
            expression.span,
        ));
    };
    if !is_valid_resource_name(value) {
        return Err(Diagnostic::new(
            "K3008",
            format!(
                "{kind} must use 1-64 lowercase letters, digits, `.` or `-`, without leading/trailing punctuation or `..`/`--`"
            ),
            expression.span,
        ));
    }
    Ok(value.clone())
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

    fn check_with_warnings(text: &str) -> AnalysisReport {
        let source = Source::new("test.krit", text);
        analyze_with_warnings(&parse_source(&source).expect("source should parse"))
    }

    fn warning_codes(warnings: &[Diagnostic]) -> Vec<&'static str> {
        warnings.iter().map(Diagnostic::code).collect()
    }

    #[test]
    fn reading_a_discard_warns_without_rejecting_the_program() {
        let text = "let _ = 1;\nprintln(_);";
        let report = check_with_warnings(text);

        assert_eq!(report.warnings.len(), 1);
        let warning = &report.warnings[0];
        assert_eq!(warning.severity(), crate::Severity::Warning);
        assert_eq!(warning.code(), "K2003");
        assert_eq!(
            warning.message(),
            "`_` is a discard; reading it will be an error in edition 2027"
        );
        assert_eq!(warning.span(), Span::new(19, 20));
        let analysis = report.result.expect("reading `_` should still check");
        assert_eq!(
            check(text).expect("analyze should accept the same program"),
            analysis
        );

        let unbound = check_with_warnings("println(_);");
        assert_eq!(
            unbound
                .result
                .expect_err("an unbound `_` is undefined")
                .code(),
            "K2001"
        );
        assert!(unbound.warnings.is_empty());

        let unread =
            check_with_warnings("let _ = 1; let _ = 2; fn ignore(_, _) { 0 } ignore(1, 2);");
        unread
            .result
            .expect("binding `_` without reading it should check");
        assert!(unread.warnings.is_empty());
    }

    #[test]
    fn discard_may_be_bound_repeatedly_and_reads_the_most_recent_binding() {
        // Each annotation holds only if `_` refers to the most recent binding.
        let report = check_with_warnings(
            r#"
            let _ = 1;
            let _ = "two";
            let text: String = _;
            fn pick(_, _: Bool) { _ }
            let flag: Bool = pick(1, true);
            let rest: List<Int> = match [1, 2] {
                [] => [],
                [_, .._] => _,
            };
            let ignore = fn(_, _) { 0 };
            fn _() { 3 }
            fn _() { 4 }
            "#,
        );

        report.result.expect("repeated discards should check");
        assert_eq!(warning_codes(&report.warnings), ["K2003", "K2003", "K2003"]);
        assert!(
            report
                .warnings
                .windows(2)
                .all(|pair| pair[0].span() < pair[1].span()),
            "warnings should be in source order"
        );

        for (text, message) in [
            (
                "let value = 1; let value = 2;",
                "duplicate declaration `value`",
            ),
            (
                "fn pair(value, value) { value }",
                "duplicate parameter `value`",
            ),
            (
                "match [1] { [] => 0, [value, ..value] => 1 };",
                "duplicate match binding `value`",
            ),
        ] {
            let error = check(text).expect_err("non-discard duplicates should still fail");
            assert_eq!(error.code(), "K2002", "{text}");
            assert_eq!(error.message(), message, "{text}");
        }
    }

    #[test]
    fn reports_warnings_found_before_an_error() {
        let report = check_with_warnings("let _ = 1;\nlet text = _ + \"a\";");

        assert_eq!(
            report
                .result
                .expect_err("the type error should still reject the program")
                .code(),
            "K3001"
        );
        assert_eq!(warning_codes(&report.warnings), ["K2003"]);
        assert_eq!(report.warnings[0].span(), Span::new(22, 23));
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

    #[test]
    fn analyzes_long_else_if_and_operator_chains_iteratively() {
        // Runs on the default test stack: recursive analysis of either chain
        // would need far more.
        let mut text = String::from("fn pick(n) {\n    if n == 0 {\n        0\n    }");
        for branch in 1..1_000 {
            text.push_str(&format!(
                " else if n == {branch} {{\n        {branch}\n    }}"
            ));
        }
        text.push_str(" else {\n        -1\n    }\n}\n\nlet total = 0");
        text.push_str(&" + pick(1)".repeat(4_000));
        text.push_str(";\nprintln(total);\n");
        let source = Source::new("test.krit", text);
        let program = parse_source(&source).expect("long chains should parse");
        let analysis = analyze(&program).expect("long chains should analyze");

        let StatementKind::Function { body, .. } = &program.statements[0].kind else {
            panic!("first statement should be a function");
        };
        let mut current = body.tail.as_deref().expect("function should have a tail");
        let mut links = 0;
        while let ExpressionKind::If { alternative, .. } = &current.kind {
            let fact = analysis
                .expression(current.span)
                .expect("every else-if link should have a fact");
            assert_eq!(fact.ty(), &Type::Int);
            links += 1;
            current = alternative;
        }
        assert_eq!(links, 1_000);

        let StatementKind::Let { value, .. } = &program.statements[1].kind else {
            panic!("second statement should be a let");
        };
        let mut current = value;
        let mut operators = 0;
        while let ExpressionKind::Binary { left, .. } = &current.kind {
            let fact = analysis
                .expression(current.span)
                .expect("every chained operator should have a fact");
            assert_eq!(fact.ty(), &Type::Int);
            operators += 1;
            current = left;
        }
        assert_eq!(operators, 4_000);
    }

    #[test]
    fn analyzes_every_tree_the_parser_accepts() {
        // A block used as a callee counts twice in analysis recursion but
        // once in the structural depth: 127 levels of `{ ... }` each followed
        // by 31 calls is 4,064 structural levels and 4,191 levels of analysis
        // recursion. Analysis reaches the undefined innermost name instead of
        // reporting the tree as too deep.
        let mut expression = String::from("g");
        for _ in 0..127 {
            expression = format!("{{ {expression} }}{}", "(1)".repeat(31));
        }
        let error = check_on_compiler_stack(format!("let value = {expression};\n"))
            .expect_err("the innermost name is undefined");
        assert_eq!(error.code(), "K2001");
    }

    #[test]
    fn rejects_hand_built_syntax_trees_beyond_the_tree_limit() {
        let depth = MAX_TREE_DEPTH + 10;
        let error = crate::run_on_thread("analysis-test", crate::COMPILER_STACK_BYTES, move || {
            let mut expression = Expression {
                kind: ExpressionKind::Literal(ValueLiteral::Boolean(true)),
                span: Span::new(depth, depth + 4),
            };
            for level in 1..=depth {
                expression = Expression {
                    kind: ExpressionKind::Unary {
                        operator: UnaryOperator::Not,
                        operand: Box::new(expression),
                    },
                    span: Span::new(depth - level, depth + 4),
                };
            }
            let program = Program {
                statements: vec![Statement {
                    kind: StatementKind::Expression(expression),
                    span: Span::new(0, depth + 5),
                }],
            };
            analyze(&program).expect_err("an over-deep syntax tree should be rejected")
        })
        .expect("analysis test thread should start");
        assert_eq!(error.code(), "K1005");
        assert!(
            error.message().contains("syntax tree"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn rejects_inferred_types_nested_beyond_the_limit() {
        let chain = |levels: usize| {
            let mut text = String::from("let level0 = 0;\n");
            for level in 1..=levels {
                text.push_str(&format!("let level{level} = [level{}];\n", level - 1));
            }
            text
        };
        let analysis = check(&chain(MAX_NESTING_DEPTH)).expect("types at the limit should analyze");
        assert!(
            analysis
                .bindings()
                .last()
                .expect("chain should bind")
                .ty()
                .to_string()
                .starts_with("List<List<")
        );

        let text = chain(MAX_NESTING_DEPTH + 1);
        let error = check(&text).expect_err("one more level should be rejected");
        assert_eq!(error.code(), "K1005");
        assert!(
            error.message().contains("inferred types"),
            "{}",
            error.message()
        );
        let crossing = format!("let level{} =", MAX_NESTING_DEPTH + 1);
        assert_eq!(
            error.span().start,
            text.find(&crossing).expect("crossing binding should exist")
        );

        // Long chains fail at the same binding instead of slowing analysis.
        let error = check(&chain(5_000)).expect_err("a long chain should be rejected");
        assert_eq!(error.code(), "K1005");
    }

    fn list_depth(ty: &Type) -> usize {
        ty.to_string().matches("List<").count()
    }

    /// Checks on the stack the CLI gives the compiler pipeline; unoptimized
    /// builds need more than a default test thread to parse 200 levels.
    fn check_on_compiler_stack(text: String) -> Result<Analysis, Diagnostic> {
        crate::run_on_thread("analysis-test", crate::COMPILER_STACK_BYTES, move || {
            check(&text)
        })
        .expect("analysis test thread should start")
    }

    #[test]
    fn rejects_inferred_types_that_grow_after_they_are_bound() {
        // `top` is bound first around the still-unknown element type of `e0`;
        // each later comparison resolves the previous element type to a deeper
        // list, so `top` grows by `width` levels per step.
        let program = |steps: usize, width: usize| {
            let wrap = |inner: &str| format!("{}{inner}{}", "[".repeat(width), "]".repeat(width));
            let mut text = format!("let e0 = [];\nlet top = {};\n", wrap("e0"));
            for step in 1..=steps {
                text.push_str(&format!(
                    "let e{step} = [];\nlet c{step} = e{} == {};\n",
                    step - 1,
                    wrap(&format!("e{step}"))
                ));
            }
            text
        };
        let analysis = check(&program(4, 50)).expect("251 levels should analyze");
        let top = analysis
            .bindings()
            .iter()
            .find(|binding| binding.name() == "top")
            .expect("top should be bound");
        assert_eq!(list_depth(top.ty()), 251);

        let text = program(5, 50);
        let error = check(&text).expect_err("301 levels should be rejected");
        assert_eq!(error.code(), "K1005");
        assert!(
            error.message().contains("inferred types"),
            "{}",
            error.message()
        );
        // Reported at the step that deepens `top` past the limit.
        let step = text.find("let c5 =").expect("fifth step should exist");
        let line_end = step + text[step..].find('\n').expect("step should end");
        assert!(
            (step..line_end).contains(&error.span().start),
            "{:?}",
            error.span()
        );

        // The reviewer's shape: 200 levels per step fails at the first step
        // instead of growing `top` by 200 levels per binding.
        let error =
            check_on_compiler_stack(program(100, 200)).expect_err("deep growth should be rejected");
        assert_eq!(error.code(), "K1005");
    }

    #[test]
    fn rejects_inferred_types_that_grow_through_an_alias() {
        // The list match makes the unknown element type inside `top` the type
        // of `head`; unifying the arms then aliases it to the type of `deep`,
        // so `top` resolves through `deep` without being bound again.
        let program = |deep: usize| {
            format!(
                "let e = [];\nlet top = {}e{};\nlet deep = {}0{};\n\
                 let h = match e {{ [] => deep, [head, ..rest] => head }};\n",
                "[".repeat(100),
                "]".repeat(100),
                "[".repeat(deep),
                "]".repeat(deep)
            )
        };
        let analysis = check_on_compiler_stack(program(150)).expect("251 levels should analyze");
        let top = analysis
            .bindings()
            .iter()
            .find(|binding| binding.name() == "top")
            .expect("top should be bound");
        assert_eq!(list_depth(top.ty()), 251);

        let error =
            check_on_compiler_stack(program(200)).expect_err("301 levels should be rejected");
        assert_eq!(error.code(), "K1005");
        assert!(
            error.message().contains("inferred types"),
            "{}",
            error.message()
        );
    }

    /// Asserts that `error` is the inferred-type `K1005` reported on the line
    /// of `text` that starts with `line`.
    fn assert_inferred_type_limit_at(text: &str, error: &Diagnostic, line: &str) {
        assert_eq!(error.code(), "K1005", "{}", error.message());
        assert!(
            error.message().contains("inferred types"),
            "{}",
            error.message()
        );
        let start = text.find(line).expect("the crossing line should exist");
        let end = start + text[start..].find('\n').expect("the line should end");
        assert!(
            (start..end).contains(&error.span().start),
            "expected the error on `{line}`, found {:?}",
            error.span()
        );
    }

    #[test]
    fn rejects_inferred_types_that_grow_when_two_solved_types_unify() {
        // `shallow` and `deep` both have solved types. Unifying them solves
        // the element type inside `shallow` to a seven-level list, which
        // deepens `top` although the merged type of `shallow` and `deep` is no
        // deeper than `deep` already was.
        let program = |width: usize| {
            format!(
                "let shallow = [[[]]];\nlet top = {}shallow{};\n\
                 let deep = [[[[[[[[[[1]]]]]]]]]];\nlet grows = shallow == deep;\n",
                "[".repeat(width),
                "]".repeat(width)
            )
        };
        let analysis = check_on_compiler_stack(program(246)).expect("256 levels should analyze");
        let top = analysis
            .bindings()
            .iter()
            .find(|binding| binding.name() == "top")
            .expect("top should be bound");
        assert_eq!(list_depth(top.ty()), 256);

        let text = program(247);
        let error = check_on_compiler_stack(text.clone()).expect_err("257 levels should fail");
        assert_inferred_type_limit_at(&text, &error, "let grows =");

        // Repeating the step would stack about 252 levels per step; it fails
        // at the second step instead of accepting ever deeper types.
        let mut text = String::from("let s0 = 1;\n");
        for step in 1..=100 {
            text.push_str(&format!(
                "let r{step} = [[]];\nlet top{step} = {}r{step}{};\n\
                 let a{step} = [[s{}]];\nlet c{step} = r{step} == a{step};\n\
                 let s{step} = top{step};\n",
                "[".repeat(250),
                "]".repeat(250),
                step - 1
            ));
        }
        let error = check_on_compiler_stack(text.clone()).expect_err("stacked steps should fail");
        assert_inferred_type_limit_at(&text, &error, "let c2 =");
    }

    #[test]
    fn rejects_infinite_types_between_two_solved_types_without_cycles() {
        // `w` contains `e`, so unifying their solved types would make the
        // element type of `e` contain itself. Rejecting it must not leave a
        // cyclic binding for the error message to render forever.
        for unify in [
            "e == w",
            "w == e",
            "match [e] { [] => w, [head, ..rest] => head }",
        ] {
            let text = format!("let e = [];\nlet w = [[e]];\nlet c = {unify};\n");
            let error = check_on_compiler_stack(text).expect_err("the type would be infinite");
            assert_eq!(error.code(), "K3001", "{unify}: {}", error.message());
        }
    }

    /// Recomputes the resolved height of `ty` from the solved bindings alone,
    /// without the analyzer's cached heights.
    fn exact_height(
        analyzer: &Analyzer,
        ty: &InferType,
        memo: &mut BTreeMap<TypeVariable, usize>,
    ) -> usize {
        let children: Vec<&InferType> = match ty {
            InferType::Variable(variable) => {
                let root = analyzer.root_variable(*variable);
                if let Some(height) = memo.get(&root) {
                    return *height;
                }
                let height = analyzer
                    .substitutions
                    .get(&root)
                    .map_or(0, |binding| exact_height(analyzer, binding, memo));
                memo.insert(root, height);
                return height;
            }
            InferType::List(element) | InferType::Option(element) => vec![element],
            InferType::Result(value, error) => vec![value, error],
            InferType::Record { fields, .. } => fields.values().collect(),
            InferType::Function {
                parameters,
                return_type,
                ..
            } => parameters
                .iter()
                .chain(std::iter::once(return_type.as_ref()))
                .collect(),
            _ => return 0,
        };
        1 + children
            .into_iter()
            .map(|child| exact_height(analyzer, child, memo))
            .max()
            .unwrap_or(0)
    }

    /// Generates a short program of list bindings, comparisons, and list
    /// matches whose inferred types sometimes grow past the limit: either
    /// random statements over earlier bindings, or steps that each unify a
    /// shallow solved type, already wrapped by another binding, with a deeper
    /// one built from the previous step.
    fn random_unification_program(state: &mut u64) -> String {
        let mut next = |bound: u64| {
            // xorshift64: deterministic, so failures reproduce.
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state % bound
        };
        let wrap = |width: u64, inner: &str| {
            let width = width as usize;
            format!("{}{inner}{}", "[".repeat(width), "]".repeat(width))
        };
        if next(2) == 0 {
            let mut text = format!("let s0 = {};\n", wrap(next(4), "1"));
            for step in 1..=1 + next(4) {
                let unify = match next(3) {
                    0 => format!("r{step} == d{step}"),
                    1 => format!("d{step} == r{step}"),
                    _ => format!("match [r{step}] {{ [] => d{step}, [head, ..rest] => head }}"),
                };
                text.push_str(&format!(
                    "let r{step} = {};\nlet top{step} = {};\nlet d{step} = {};\n\
                     let u{step} = {unify};\nlet s{step} = top{step};\n",
                    wrap(next(4), "[]"),
                    wrap(next(120), &format!("r{step}")),
                    wrap(next(4), &format!("s{}", step - 1)),
                ));
            }
            return text;
        }
        let mut text = String::from("let v0 = [];\n");
        let mut values = 1;
        for statement in 0..4 + next(12) {
            let width = if next(4) == 0 { 30 + next(90) } else { next(5) };
            let value = match next(20) {
                0..=5 => wrap(width, "[]"),
                6 => wrap(width, "1"),
                7..=11 => wrap(width, &format!("v{}", next(values))),
                12..=16 => {
                    let (left, right) = (next(values), next(values));
                    text.push_str(&format!("let c{statement} = v{left} == v{right};\n"));
                    continue;
                }
                _ => {
                    let (list, other) = (next(values), next(values));
                    format!("match v{list} {{ [] => v{other}, [head, ..rest] => head }}")
                }
            };
            text.push_str(&format!("let v{values} = {value};\n"));
            values += 1;
        }
        text
    }

    #[test]
    fn cached_type_heights_stay_exact_through_every_unification() {
        // Every accepted program must leave each type variable's cached
        // height equal to the height of its solved type; a stale, too-low
        // height is how an inferred type escapes the limit. A cyclic binding
        // left behind by a rejected program would instead overflow the stack.
        let (accepted, too_deep) = crate::run_on_thread(
            "analysis-test",
            crate::COMPILER_STACK_BYTES,
            check_type_heights,
        )
        .expect("analysis test thread should start");
        // The generator is deterministic; these guard its coverage.
        assert!(accepted >= 200, "only {accepted} programs were accepted");
        assert!(too_deep >= 20, "only {too_deep} programs were too deep");
    }

    /// Analyzes generated programs, checks the cached heights of each one
    /// that is accepted, and returns how many were accepted and how many
    /// failed with `K1005`.
    fn check_type_heights() -> (usize, usize) {
        let mut state = 0x9e37_79b9_7f4a_7c15;
        let (mut accepted, mut too_deep) = (0, 0);
        for _ in 0..500 {
            let text = random_unification_program(&mut state);
            let source = Source::new("heights.krit", text.as_str());
            let program = parse_source(&source).expect("generated programs should parse");
            let mut analyzer = Analyzer::new();
            match analyzer.program(&program) {
                Ok(_) => {
                    accepted += 1;
                    let mut memo = BTreeMap::new();
                    for variable in 0..analyzer.next_type_variable {
                        if analyzer.root_variable(variable) != variable {
                            continue;
                        }
                        let exact =
                            exact_height(&analyzer, &InferType::Variable(variable), &mut memo);
                        assert_eq!(
                            analyzer.type_heights[variable as usize], exact,
                            "type variable {variable} of:\n{text}"
                        );
                        assert!(exact <= MAX_NESTING_DEPTH, "{text}");
                    }
                }
                Err(error) if error.code() == "K1005" => too_deep += 1,
                Err(_) => {}
            }
        }
        (accepted, too_deep)
    }

    #[test]
    fn bounded_type_rendering_stops_shared_dag_expansion() {
        let mut source = "let value0 = record { leaf: 0 };\n".to_owned();
        for level in 1..=20 {
            source.push_str(&format!(
                "let value{level} = record {{ left: value{}, right: value{} }};\n",
                level - 1,
                level - 1
            ));
        }
        let source = Source::new("<bounded-type-render>", source);
        let program = crate::parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("source should analyze");
        let small = analysis.bindings()[0].ty();
        let large = analysis.bindings().last().expect("last binding").ty();
        let expected = small.to_string();

        assert_eq!(
            small.render_bounded(1024).as_deref(),
            Some(expected.as_str())
        );
        assert!(large.render_bounded(16 * 1024).is_none());
    }
}
