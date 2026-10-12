use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::{self, Write as _},
    sync::Arc,
};

use krit_capability::{HttpOrigin, is_valid_config_key, is_valid_resource_name};

use crate::{
    Builtin, Diagnostic, Span,
    ast::{
        BinaryOperator, Block, ConstructField, ContractRecord, Expression, ExpressionKind,
        ListEntry, MatchArm, Parameter, Pattern, PatternKind, Program, Statement, StatementKind,
        StringPart, TypeAnnotation, TypeKind, UnaryOperator, ValueLiteral, VariantArm,
        VariantFamily, VariantName, VariantPattern,
    },
    habits::{MethodReceiver, closest_name, method_hint, undefined_name_note},
    limits::{MAX_NESTING_DEPTH, MAX_TREE_DEPTH, tree_depth_error},
    patterns,
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

    pub(crate) fn shared_type(&self) -> Arc<Type> {
        Arc::clone(&self.ty)
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
/// warnings found, whether analysis succeeds or fails. The lints
/// (`K9000`-`K9999`) need the complete analysis, so they run only when
/// analysis succeeds.
pub fn analyze_with_warnings(program: &Program) -> AnalysisReport {
    let mut analyzer = Analyzer::new();
    let result = analyzer.program(program);
    let mut warnings = analyzer.warnings;
    if let Ok(analysis) = &result {
        warnings.extend(crate::lint::lint(program, analysis));
    }
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
    Orderable,
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
    /// The span of the field access `call` is about to analyze as its callee,
    /// so a failed access reports the method call it is part of (`K3011`).
    method_callee: Option<Span>,
    /// What a postfix `?` may exit, one entry per enclosing function body,
    /// innermost last; empty at the top level.
    exits: Vec<ExitContext>,
    /// Where a let-else may exit, one entry per enclosing function body,
    /// innermost last; empty at the top level.
    let_else_bodies: Vec<LetElseBody>,
}

/// Whether a let-else may return from the function whose body it is in.
#[derive(Clone)]
enum LetElseBody {
    /// The body block at `block` of a named function (or entrypoint) whose
    /// result type is `result`: a let-else that is a direct statement of
    /// that block returns its arm's value from the function.
    Named { block: Span, result: InferType },
    /// The body of an anonymous function, which no let-else may exit.
    Closure,
}

/// Whether a postfix `?` in a function body may return from that function.
#[derive(Clone)]
enum ExitContext {
    Propagates(Propagation),
    Refused(Refusal),
}

/// A named function (or a `queue`/`schedule` entrypoint) declared to return
/// `Result<_, E>` or `Option<_>`, which a postfix `?` in its body returns
/// from.
#[derive(Clone)]
struct Propagation {
    name: String,
    family: VariantFamily,
    /// The declared error type `E` of a `Result` family.
    error: Option<InferType>,
    /// The declared return type as written.
    return_type: String,
}

/// Why a postfix `?` cannot return from where it stands (`K3012`).
#[derive(Clone)]
enum Refusal {
    /// Outside every function body.
    TopLevel,
    /// In an anonymous function `fn(...) { ... }`.
    Closure,
    /// In a webhook entrypoint, which returns `HttpResponse`.
    Webhook,
    /// In a named function without a declared return type.
    Undeclared { name: String },
    /// In a named function declared to return a type that is neither a
    /// `Result` nor an `Option`.
    Other { name: String, return_type: String },
}

#[derive(Clone)]
struct ScopeBinding {
    id: SymbolId,
    ty: InferType,
}

/// A name or `_` at the end of a pattern's path, with its span and the type
/// of the value at its position.
struct PatternBinding {
    name: String,
    span: Span,
    ty: InferType,
}

/// The positions that the patterns of one `match` reach and their types: the
/// subject, and below each typed variant position, its payloads.
///
/// A position is typed against its family once, by the first pattern that
/// reaches it, and the patterns of every later arm reuse its payload types,
/// so typing a match costs one unification per position rather than one per
/// position of every arm.
struct PatternPositions {
    /// The type of the value at each position.
    types: Vec<InferType>,
    /// What each position's family gave the patterns below it, once a
    /// pattern at the position has been typed.
    payloads: Vec<Option<PositionPayloads>>,
}

impl PatternPositions {
    /// The position of the match's subject.
    const SUBJECT: usize = 0;

    fn new(subject: InferType) -> Self {
        Self {
            types: vec![subject],
            payloads: vec![None],
        }
    }

    /// Adds a position whose value has type `ty` and returns it.
    fn add(&mut self, ty: InferType) -> usize {
        self.types.push(ty);
        self.payloads.push(None);
        self.types.len() - 1
    }
}

/// What a typed position gives the patterns below it.
#[derive(Clone)]
enum PositionPayloads {
    /// An `Option<T>` position: the position of a `Some` payload, of type `T`.
    Option { some: usize },
    /// A `Result<T, E>` position: the positions of an `Ok` payload, of type
    /// `T`, and of an `Err` payload, of type `E`.
    Result { ok: usize, err: usize },
    /// A `List<T>` position: the types of a list pattern's head, `T`, and
    /// tail, `List<T>`, which are names or `_`.
    List { element: InferType, list: InferType },
}

impl PositionPayloads {
    const fn family(&self) -> patterns::Family {
        match self {
            Self::Option { .. } => patterns::Family::Option,
            Self::Result { .. } => patterns::Family::Result,
            Self::List { .. } => patterns::Family::List,
        }
    }
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
            method_callee: None,
            exits: Vec::new(),
            let_else_bodies: Vec::new(),
        }
    }

    fn program(&mut self, program: &Program) -> Result<Analysis, Diagnostic> {
        let mut effects = InferEffects::default();
        for statement in &program.statements {
            effects.union(self.statement(statement, None)?);
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

    /// Analyzes `statement`, a direct statement of the block at `block`, or
    /// of the program when `block` is `None`.
    fn statement(
        &mut self,
        statement: &Statement,
        block: Option<Span>,
    ) -> Result<InferEffects, Diagnostic> {
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
            StatementKind::LetElse {
                pattern,
                annotation,
                value,
                arm,
                ..
            } => self.let_else(
                statement.span,
                block,
                pattern,
                annotation.as_ref(),
                value,
                arm,
            ),
            StatementKind::Function {
                name,
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::Webhook {
                name,
                parameters,
                return_type,
                body,
                ..
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
                let exit = if matches!(statement.kind, StatementKind::Webhook { .. }) {
                    ExitContext::Refused(Refusal::Webhook)
                } else {
                    named_exit_context(name, return_type.as_ref(), &declared_return)
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
                self.exits.push(exit);
                self.let_else_bodies.push(LetElseBody::Named {
                    block: body.span,
                    result: declared_return.clone(),
                });
                let body_info = self.block(body);
                self.let_else_bodies.pop();
                self.exits.pop();
                let body_info = body_info?;
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

    /// Analyzes the let-else `let PATTERN (: TYPE)? = value else ARM;` at
    /// `span`, a direct statement of the block at `block` (`None` at the top
    /// level).
    ///
    /// It is valid only as a direct statement of a named function's body,
    /// with the two complementary patterns of one family (`K3014`). The value
    /// has the annotated type and belongs to the pattern's family. The arm's
    /// binder is in scope only in the arm, whose value is the function's
    /// return value and so has its result type. The pattern's binder is then
    /// declared like a `let` binding for the rest of the block. A binder `_`
    /// is the discard name, declared like any other binder, so reading it is
    /// warning `K2003`, as for every `_`. The effects are those of the value
    /// and the arm.
    fn let_else(
        &mut self,
        span: Span,
        block: Option<Span>,
        pattern: &VariantPattern,
        annotation: Option<&TypeAnnotation>,
        value: &Expression,
        arm: &VariantArm,
    ) -> Result<InferEffects, Diagnostic> {
        let head = Span::new(span.start, pattern.span.end);
        let result = self.let_else_result(head, block)?;
        let complement = pattern.variant.complement();
        if arm.variant != complement {
            return Err(Diagnostic::new(
                "K3014",
                format!(
                    "the arm of a let-else on `{}` must match `{}`, found `{}`",
                    pattern.variant.as_str(),
                    complement.as_str(),
                    arm.variant.as_str()
                ),
                arm.span,
            )
            .with_note(format!(
                "the two patterns of a let-else are the two variants of one family, `Ok` and \
                 `Err` or `Some` and `None`: write `else {} => ...`",
                complement.example_pattern()
            )));
        }
        for (variant, binding, at) in [
            (pattern.variant, &pattern.binding, pattern.span),
            (arm.variant, &arm.binding, arm.span),
        ] {
            if (variant != VariantName::None) != binding.is_some() {
                return Err(Diagnostic::new(
                    "K3005",
                    format!("invalid `{}` let-else binding", variant.as_str()),
                    at,
                ));
            }
        }
        if let Some(name) = &pattern.binding {
            self.ensure_name_available(name, span)?;
        }

        let value_info = self.expression(value)?;
        let subject = if let Some(annotation) = annotation {
            let annotated = self.annotation(annotation);
            self.unify(
                annotated.clone(),
                value_info.ty,
                annotation.span,
                "let-else annotation",
            )?;
            annotated
        } else {
            value_info.ty
        };
        let family = pattern.variant.family();
        let family_name = match family {
            VariantFamily::Option => "an `Option`",
            VariantFamily::Result => "a `Result`",
        };
        let found = self.resolve_head(&subject);
        if !matches!(
            (&found, family),
            (InferType::Variable(_), _)
                | (InferType::Option(_), VariantFamily::Option)
                | (InferType::Result(_, _), VariantFamily::Result)
        ) {
            return Err(Diagnostic::new(
                "K3005",
                format!(
                    "let-else on `{}` requires {family_name} value, found `{}`",
                    pattern.variant.as_str(),
                    self.render_type(&found)
                ),
                value.span,
            ));
        }
        let payload = self.fresh_type();
        let error = self.fresh_type();
        let expected = match family {
            VariantFamily::Option => InferType::Option(Box::new(payload.clone())),
            VariantFamily::Result => {
                InferType::Result(Box::new(payload.clone()), Box::new(error.clone()))
            }
        };
        self.unify(expected, subject, value.span, "let-else value")?;
        let payload_of = |variant| match variant {
            VariantName::Err => error.clone(),
            _ => payload.clone(),
        };

        self.push_scope();
        if let Some(binding) = &arm.binding {
            self.define_symbol(
                binding,
                payload_of(arm.variant),
                arm.span,
                SymbolKind::Match,
                false,
            );
        }
        let arm_info = self.expression(&arm.value)?;
        self.pop_scope();
        self.unify(result, arm_info.ty, arm.value.span, "let-else arm")?;

        if let Some(name) = &pattern.binding {
            let binding_type = self.fresh_type();
            self.unify(
                binding_type.clone(),
                payload_of(pattern.variant),
                span,
                "let-else binding",
            )?;
            let id = self.define_symbol(name, binding_type.clone(), span, SymbolKind::Let, false);
            self.bindings.push(PendingBinding {
                id,
                name: name.clone(),
                ty: binding_type,
                span,
                top_level: false,
            });
        }
        let mut effects = value_info.effects;
        effects.union(arm_info.effects);
        Ok(effects)
    }

    /// The result type of the function a let-else whose `let` and pattern
    /// are at `head` returns from, when it is a direct statement of that
    /// function's body block; `block` is the block it is a statement of.
    fn let_else_result(&self, head: Span, block: Option<Span>) -> Result<InferType, Diagnostic> {
        let (message, note) = match self.let_else_bodies.last() {
            Some(LetElseBody::Named {
                block: body,
                result,
            }) if block == Some(*body) => return Ok(result.clone()),
            Some(LetElseBody::Named { .. }) => (
                "let-else must be a statement of the function body itself, not of a nested \
                 block",
                "let-else returns from the enclosing function, so it may only appear at the top \
                 level of a named function's body; handle the value with `match` here",
            ),
            Some(LetElseBody::Closure) => (
                "let-else is not allowed in an anonymous function",
                "let-else returns only from a named function; handle the value with `match` \
                 here, or move the code into a named `fn`",
            ),
            None => (
                "let-else is not allowed at the top level",
                "let-else returns early from the enclosing named function; at the top level, \
                 handle the value with `match`",
            ),
        };
        Err(Diagnostic::new("K3014", message, head).with_note(note))
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
            effects.union(self.statement(statement, Some(block.span))?);
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
            ExpressionKind::Interpolation(parts) => self.interpolation(parts),
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
            ExpressionKind::Unit => Ok(ExpressionInfo {
                ty: InferType::Unit,
                effects: InferEffects::default(),
            }),
            ExpressionKind::List(elements) => self.list(elements),
            ExpressionKind::EntryList(entries) => self.entry_list(entries),
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
            ExpressionKind::Construct {
                contract,
                contract_span,
                fields,
            } => self.construction(*contract, *contract_span, fields),
            ExpressionKind::FieldAccess { value, field } => {
                let called = self.method_callee.take() == Some(expression.span);
                let receiver = value;
                let value = self.expression(receiver)?;
                let receiver_type = called.then(|| value.ty.clone());
                let ty = self
                    .require_field(value.ty, field, expression.span)
                    .map_err(|error| match receiver_type {
                        Some(receiver_type) if error.code() == "K3004" => {
                            self.method_call_error(&receiver_type, receiver, field, expression.span)
                        }
                        _ => error,
                    })?;
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
            ExpressionKind::Propagate { operand, question } => self.propagate(operand, *question),
            ExpressionKind::Match { subject, arms } => {
                self.match_expression(subject, arms, expression.span)
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

    /// Analyzes a named construction such as `HttpResponse { status: 200 }`.
    ///
    /// The field set is checked first (`K3016`). Each written value is then
    /// analyzed in source order and must have its contract field's type. The
    /// construction has exactly the closed structural record type that
    /// `record { ... }` with every contract field would have; the omitted
    /// fields take their spec-fixed defaults, whose types are the contract
    /// field types, so they need no syntax node and no expression fact.
    fn construction(
        &mut self,
        contract: ContractRecord,
        contract_span: Span,
        fields: &[ConstructField],
    ) -> Result<ExpressionInfo, Diagnostic> {
        check_construction_fields(contract, contract_span, fields)?;
        let InferType::Record {
            fields: contract_fields,
            ..
        } = contract_record(&contract_nominal(contract))
        else {
            unreachable!("built-in record contracts normalize to records")
        };
        let mut effects = InferEffects::default();
        for field in fields {
            let value = self.expression(&field.value)?;
            self.constraints.push(TypeConstraint {
                ty: value.ty.clone(),
                span: field.value.span,
                kind: ConstraintKind::StructuralValue,
            });
            let expected = contract_fields
                .get(&field.name)
                .cloned()
                .expect("checked construction fields belong to their contract");
            self.unify(
                expected,
                value.ty,
                field.value.span,
                &format!("`{}` field `{}`", contract.name(), field.name),
            )?;
            effects.union(value.effects);
        }
        Ok(ExpressionInfo {
            ty: InferType::Record {
                fields: contract_fields,
                open: false,
            },
            effects,
        })
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

    /// Analyzes an interpolated string or text block.
    ///
    /// Each hole's value is analyzed in source order and must be a `String`
    /// (`K3015` at the hole otherwise). The string has type `String`, and its
    /// effects and literal requirements are its holes', which are names and
    /// field accesses and so have none.
    fn interpolation(&mut self, parts: &[StringPart]) -> Result<ExpressionInfo, Diagnostic> {
        let mut effects = InferEffects::default();
        for part in parts {
            let StringPart::Hole { value, span } = part else {
                continue;
            };
            let info = self.expression(value)?;
            if self
                .unify_inner(InferType::String, info.ty.clone(), *span)
                .is_err()
            {
                return Err(self
                    .type_depth_error
                    .take()
                    .unwrap_or_else(|| self.hole_type_error(value, &info.ty, *span)));
            }
            effects.union(info.effects);
        }
        Ok(ExpressionInfo {
            ty: InferType::String,
            effects,
        })
    }

    /// The error for the interpolation hole at `span` whose value `value` has
    /// type `ty`, which is not `String` (`K3015`, always with one note).
    fn hole_type_error(&self, value: &Expression, ty: &InferType, span: Span) -> Diagnostic {
        let path = receiver_path(value);
        let message = format!(
            "{} has type `{}`, but a hole must be a `String`",
            path.as_ref().map_or_else(
                || "this interpolation hole".to_owned(),
                |path| format!("interpolation hole `{path}`")
            ),
            self.render_type(ty)
        );
        let note = match self.resolve_head(ty) {
            InferType::Int => {
                let path = path.as_deref().unwrap_or("value");
                let name = path.rsplit('.').next().unwrap_or(path);
                format!(
                    "convert it first, as in `let {name}_text = int_to_string({path});`, then \
                     write `\\({name}_text)`"
                )
            }
            InferType::Secret => "a `Secret` is opaque and never becomes text".to_owned(),
            InferType::Option(_) | InferType::Result(_, _) => {
                "match the value first and interpolate the `String` it holds".to_owned()
            }
            _ => "a hole inserts text: bind a `String` to a name and interpolate that name"
                .to_owned(),
        };
        Diagnostic::new("K3015", message, span).with_note(note)
    }

    /// Analyzes an entry list `["k": v, ...]`.
    ///
    /// Every key and value must be a `String`; they are analyzed in source
    /// order (`k1`, `v1`, `k2`, ...). The list has exactly the type of
    /// `[record { name: k, value: v }, ...]` with `String` fields, the closed
    /// structural record that `HttpHeader` and `LogField` name.
    fn entry_list(&mut self, entries: &[ListEntry]) -> Result<ExpressionInfo, Diagnostic> {
        let mut effects = InferEffects::default();
        for entry in entries {
            for (part, context) in [(&entry.key, "entry key"), (&entry.value, "entry value")] {
                let info = self.expression(part)?;
                self.constraints.push(TypeConstraint {
                    ty: info.ty.clone(),
                    span: part.span,
                    kind: ConstraintKind::StructuralValue,
                });
                self.unify(InferType::String, info.ty, part.span, context)?;
                effects.union(info.effects);
            }
        }
        Ok(ExpressionInfo {
            ty: InferType::List(Box::new(InferType::Record {
                fields: BTreeMap::from([
                    ("name".to_owned(), InferType::String),
                    ("value".to_owned(), InferType::String),
                ]),
                open: false,
            })),
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
        self.exits.push(ExitContext::Refused(Refusal::Closure));
        self.let_else_bodies.push(LetElseBody::Closure);
        let body_info = self.block(body);
        self.let_else_bodies.pop();
        self.exits.pop();
        let body_info = body_info?;
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
                    if builtin == Builtin::ConfigString {
                        if !is_valid_config_key(resource) {
                            return Err(Diagnostic::new(
                                "K3008",
                                "configuration key must use 1-64 lowercase letters, digits, `.`, `-`, or `_`, without leading/trailing punctuation or `..`/`--`/`__`",
                                argument.span,
                            ));
                        }
                    } else if !is_valid_resource_name(resource) {
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
        if matches!(callee.kind, ExpressionKind::FieldAccess { .. }) {
            self.method_callee = Some(callee.span);
        }
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
                let diagnostic = Diagnostic::new(
                    "K3002",
                    format!("cannot call value of type `{}`", self.render_type(&other)),
                    callee.span,
                );
                // `None()`: the reserved name always means the built-in.
                return Err(
                    if matches!(&callee.kind, ExpressionKind::Variable(name) if name == "None") {
                        diagnostic.with_note(
                            "`None` is a value, not a function: write `None` without \
                             parentheses, or `Some(value)` for a present value",
                        )
                    } else {
                        diagnostic
                    },
                );
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

    /// Analyzes `operand?`, whose `?` is at `question`.
    ///
    /// `?` is valid only in the body of a named function declared to return
    /// `Result<_, E>` (for a `Result<T, E>` operand with the identical `E`)
    /// or `Option<_>` (for an `Option<T>` operand); its value is the `T`. It
    /// adds no effect of its own.
    fn propagate(
        &mut self,
        operand: &Expression,
        question: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
        let operand_info = self.expression(operand)?;
        let Propagation {
            name,
            family,
            error,
            return_type,
        } = match self.exits.last().cloned() {
            Some(ExitContext::Propagates(propagation)) => propagation,
            Some(ExitContext::Refused(refusal)) => {
                return Err(propagation_refused(&refusal, question));
            }
            None => return Err(propagation_refused(&Refusal::TopLevel, question)),
        };
        let found = self.resolve_head(&operand_info.ty);
        let ty = match (family, found, error) {
            (VariantFamily::Option, InferType::Option(value), _) => *value,
            (VariantFamily::Result, InferType::Result(value, found_error), Some(error)) => {
                // The failure arm rebuilds the `Err` around the error value,
                // so an opaque error type is rejected as it is for `Err(x)`.
                self.constraints.push(TypeConstraint {
                    ty: error.clone(),
                    span: question,
                    kind: ConstraintKind::StructuralValue,
                });
                self.unify(error, *found_error, question, "`?` error")
                    .map_err(|mismatch| {
                        if mismatch.code() != "K3001" {
                            return mismatch;
                        }
                        Diagnostic::new(
                            "K3013",
                            format!(
                                "`?` cannot return this `Err` from `{name}`: its error type \
                                 differs from the declared `{return_type}`"
                            ),
                            question,
                        )
                        .with_note(
                            "`?` passes the error on unchanged, so both error types must be \
                             identical; `match` the value to convert the error",
                        )
                    })?;
                *value
            }
            (family, InferType::Variable(_), error) => {
                let value = self.fresh_type();
                let expected = match (family, error) {
                    (VariantFamily::Result, Some(error)) => {
                        self.constraints.push(TypeConstraint {
                            ty: error.clone(),
                            span: question,
                            kind: ConstraintKind::StructuralValue,
                        });
                        InferType::Result(Box::new(value.clone()), Box::new(error))
                    }
                    _ => InferType::Option(Box::new(value.clone())),
                };
                self.unify(expected, operand_info.ty, question, "`?` operand")?;
                value
            }
            (family, found @ (InferType::Option(_) | InferType::Result(_, _)), _) => {
                let (found_family, expected_family) = match family {
                    VariantFamily::Option => ("a `Result`", "an `Option`"),
                    VariantFamily::Result => ("an `Option`", "a `Result`"),
                };
                return Err(Diagnostic::new(
                    "K3013",
                    format!(
                        "`?` on {found_family} value cannot return from `{name}`, which \
                         returns `{return_type}`",
                    ),
                    question,
                )
                .with_note(format!(
                    "`?` returns the family it unwraps; `match` the `{}` to build \
                     {expected_family}, or declare {found_family} return type",
                    self.render_type(&found)
                )));
            }
            (_, found, _) => {
                return Err(Diagnostic::new(
                    "K3001",
                    format!(
                        "`?` applies to a `Result` or `Option` value, found `{}`",
                        self.render_type(&found)
                    ),
                    question,
                ));
            }
        };
        Ok(ExpressionInfo {
            ty,
            effects: operand_info.effects,
        })
    }

    /// Analyzes the `match` at `span`.
    ///
    /// After the subject, the arms' patterns are checked against each other
    /// ([`patterns::check_arms`]), typed against the subject's type in arm
    /// order, and checked for exhaustiveness ([`patterns::check_exhaustive`]).
    /// Each arm's value is then analyzed in a scope of its own that holds the
    /// pattern's binders, and every arm's value has the match's type.
    fn match_expression(
        &mut self,
        subject: &Expression,
        arms: &[MatchArm],
        span: Span,
    ) -> Result<ExpressionInfo, Diagnostic> {
        let subject = self.expression(subject)?;
        let known_family = match self.resolve_head(&subject.ty) {
            InferType::Option(_) => Some(patterns::Family::Option),
            InferType::Result(_, _) => Some(patterns::Family::Result),
            InferType::List(_) => Some(patterns::Family::List),
            _ => None,
        };
        patterns::check_arms(arms, known_family)?;
        let mut positions = PatternPositions::new(subject.ty);
        let mut arm_bindings = Vec::with_capacity(arms.len());
        for arm in arms {
            arm_bindings.push(self.pattern_bindings(&arm.pattern, &mut positions, span)?);
        }
        patterns::check_exhaustive(arms, span)?;

        let mut result_type = None;
        let mut effects = subject.effects;
        for (arm, bindings) in arms.iter().zip(arm_bindings) {
            self.push_scope();
            for PatternBinding { name, span, ty } in bindings {
                self.define_symbol(&name, ty, span, SymbolKind::Match, false);
            }
            let arm_info = self.expression(&arm.value);
            self.pop_scope();
            let arm_info = arm_info?;
            effects.union(arm_info.effects);
            result_type = Some(if let Some(result_type) = result_type {
                self.unify(result_type, arm_info.ty, arm.span, "match arms")?
            } else {
                arm_info.ty
            });
        }
        Ok(ExpressionInfo {
            ty: result_type.unwrap_or_else(|| self.fresh_type()),
            effects,
        })
    }

    /// Types `pattern`, an arm of the match at `span`, against the types of
    /// the match's `positions` and returns the names and `_` it binds, in
    /// source order, each with the type of the value at its position.
    ///
    /// Patterns are paths, so this walks one iteratively, from the subject's
    /// position down through each variant pattern's payload.
    fn pattern_bindings(
        &mut self,
        pattern: &Pattern,
        positions: &mut PatternPositions,
        span: Span,
    ) -> Result<Vec<PatternBinding>, Diagnostic> {
        let mut position = PatternPositions::SUBJECT;
        let mut current = pattern;
        let mut nested = false;
        loop {
            match &current.kind {
                PatternKind::Binder(name) => {
                    return Ok(vec![PatternBinding {
                        name: name.clone(),
                        span: current.span,
                        ty: positions.types[position].clone(),
                    }]);
                }
                PatternKind::Wildcard => {
                    return Ok(vec![PatternBinding {
                        name: DISCARD.to_owned(),
                        span: current.span,
                        ty: positions.types[position].clone(),
                    }]);
                }
                PatternKind::EmptyList => {
                    self.type_position(
                        positions,
                        position,
                        patterns::Family::List,
                        current,
                        nested,
                        span,
                    )?;
                    return Ok(Vec::new());
                }
                PatternKind::Cons { head, tail } => {
                    let PositionPayloads::List { element, list } = self.type_position(
                        positions,
                        position,
                        patterns::Family::List,
                        current,
                        nested,
                        span,
                    )?
                    else {
                        unreachable!("a `List` position has list payloads");
                    };
                    if let (PatternKind::Binder(head_name), PatternKind::Binder(tail_name)) =
                        (&head.kind, &tail.kind)
                        && head_name == tail_name
                    {
                        return Err(Diagnostic::new(
                            "K2002",
                            format!("duplicate match binding `{tail_name}`"),
                            current.span,
                        ));
                    }
                    // `patterns::check_arms` made the head and tail names or
                    // `_`.
                    return Ok([(head, element), (tail, list)]
                        .into_iter()
                        .map(|(part, ty)| PatternBinding {
                            name: match &part.kind {
                                PatternKind::Binder(name) => name.clone(),
                                _ => DISCARD.to_owned(),
                            },
                            span: part.span,
                            ty,
                        })
                        .collect());
                }
                PatternKind::Variant { variant, payload } => {
                    let family = match variant.family() {
                        VariantFamily::Option => patterns::Family::Option,
                        VariantFamily::Result => patterns::Family::Result,
                    };
                    let payloads =
                        self.type_position(positions, position, family, current, nested, span)?;
                    let Some(payload) = payload else {
                        return Ok(Vec::new());
                    };
                    position = match (payloads, variant) {
                        (PositionPayloads::Option { some }, VariantName::Some)
                        | (PositionPayloads::Result { ok: some, .. }, VariantName::Ok)
                        | (PositionPayloads::Result { err: some, .. }, VariantName::Err) => some,
                        _ => unreachable!("a variant's position has its family's payloads"),
                    };
                    current = payload;
                    nested = true;
                }
            }
        }
    }

    /// Requires `family` at `position` for the variant or list pattern
    /// `pattern` of the match at `span`, and returns what that family gives
    /// the patterns below it.
    ///
    /// The first pattern of a family to reach a position unifies the type
    /// there with the family over fresh payload types (`K3005` for another
    /// type: at the match for the subject, or at `pattern` when it is
    /// `nested` in a payload); every later pattern of that family at the
    /// position reuses them, so a position is typed once however many arms
    /// reach it. A pattern of another family is typed afresh and fails.
    fn type_position(
        &mut self,
        positions: &mut PatternPositions,
        position: usize,
        family: patterns::Family,
        pattern: &Pattern,
        nested: bool,
        span: Span,
    ) -> Result<PositionPayloads, Diagnostic> {
        if let Some(payloads) = &positions.payloads[position]
            && payloads.family() == family
        {
            return Ok(payloads.clone());
        }
        let (expected, payloads, name) = match family {
            patterns::Family::Option => {
                let some = self.fresh_type();
                (
                    InferType::Option(Box::new(some.clone())),
                    PositionPayloads::Option {
                        some: positions.add(some),
                    },
                    "Option",
                )
            }
            patterns::Family::Result => {
                let ok = self.fresh_type();
                let err = self.fresh_type();
                (
                    InferType::Result(Box::new(ok.clone()), Box::new(err.clone())),
                    PositionPayloads::Result {
                        ok: positions.add(ok),
                        err: positions.add(err),
                    },
                    "Result",
                )
            }
            patterns::Family::List => {
                let element = self.fresh_type();
                let list = InferType::List(Box::new(element.clone()));
                (
                    list.clone(),
                    PositionPayloads::List { element, list },
                    "List",
                )
            }
        };
        self.expect_pattern_type(
            positions.types[position].clone(),
            expected,
            name,
            nested.then_some(pattern),
            span,
        )?;
        positions.payloads[position] = Some(payloads.clone());
        Ok(payloads)
    }

    /// Requires a pattern of `family` to match a value of type `actual`,
    /// unifying it with `expected` (`K3005` for another type): the subject of
    /// the match at `span`, or the payload that `nested` matches.
    fn expect_pattern_type(
        &mut self,
        actual: InferType,
        expected: InferType,
        family: &str,
        nested: Option<&Pattern>,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let Some(pattern) = nested else {
            return self.expect_match_type(actual, expected, span, family);
        };
        let resolved = self.resolve_head(&actual);
        if !matches!(
            (&resolved, &expected),
            (InferType::Variable(_), _)
                | (InferType::List(_), InferType::List(_))
                | (InferType::Option(_), InferType::Option(_))
                | (InferType::Result(_, _), InferType::Result(_, _))
        ) {
            let article = if family == "Option" { "an" } else { "a" };
            return Err(Diagnostic::new(
                "K3005",
                format!(
                    "the pattern `{pattern}` requires {article} `{family}` value, found `{}`",
                    self.render_type(&resolved)
                ),
                pattern.span,
            ));
        }
        self.unify(expected, actual, pattern.span, "match pattern")?;
        Ok(())
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
                let ty = self.unify(left.ty, right.ty, span, "comparison operands")?;
                self.constraints.push(TypeConstraint {
                    ty,
                    span,
                    kind: ConstraintKind::Orderable,
                });
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
                ConstraintKind::Addable | ConstraintKind::Orderable => matches!(
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
                        ConstraintKind::Orderable => "ordered",
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
                        ConstraintKind::Orderable => "ordered",
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
                    ConstraintKind::Orderable => (
                        "K3001",
                        format!(
                            "`<`, `<=`, `>`, and `>=` require two integers or two strings, found `{}`",
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
        if let Some(builtin) = self.builtin(name, span) {
            return Ok(builtin);
        }
        let diagnostic = Diagnostic::new("K2001", format!("undefined name `{name}`"), span);
        // Suggestions come from the names visible here: bindings in scope,
        // built-ins, and the Boolean literals (`True` -> `true`).
        let candidates = self
            .scopes
            .iter()
            .flat_map(|scope| scope.keys().map(String::as_str))
            .chain(Builtin::ALL.iter().map(|builtin| builtin.as_str()))
            .chain(["true", "false"]);
        Err(match undefined_name_note(name, candidates) {
            Some(note) => diagnostic.with_note(note),
            None => diagnostic,
        })
    }

    /// The error for calling `.method()` on `receiver`, a value of
    /// `receiver_type` that has no field of that name (`K3011`).
    ///
    /// Krit values have no methods; `value.name(...)` is valid only as a call
    /// of a function stored in a record field. The note names the standard
    /// library function that does the job when one exists.
    fn method_call_error(
        &self,
        receiver_type: &InferType,
        receiver: &Expression,
        method: &str,
        span: Span,
    ) -> Diagnostic {
        let head = self.resolve_head(receiver_type);
        let rendered = self.render_type(&head);
        let (kind, message) = match head {
            InferType::Record { .. }
            | InferType::HttpHeader
            | InferType::HttpRequest
            | InferType::HttpResponse
            | InferType::LogField
            | InferType::QueueJob
            | InferType::ScheduleEvent => (
                MethodReceiver::Other,
                format!(
                    "cannot call `.{method}()`: `{rendered}` has no field `{method}`, and Krit \
                     values have no methods"
                ),
            ),
            head => (
                match head {
                    InferType::String => MethodReceiver::String,
                    InferType::Int => MethodReceiver::Int,
                    InferType::List(_) => MethodReceiver::List,
                    InferType::Option(_) | InferType::Result(..) => MethodReceiver::Variant,
                    _ => MethodReceiver::Other,
                },
                format!(
                    "cannot call `.{method}()` on a value of type `{rendered}`: Krit values have \
                     no methods"
                ),
            ),
        };
        let value = receiver_path(receiver).unwrap_or_else(|| "value".to_owned());
        Diagnostic::new("K3011", message, span).with_note(method_hint(kind, method, &value))
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
            // The standard library is pure: no effect and no requirement.
            Builtin::IntToString => InferType::Function {
                parameters: vec![InferType::Int],
                return_type: Box::new(InferType::String),
                effect: self.fresh_effect(),
            },
            Builtin::UrlPathSegment => InferType::Function {
                parameters: vec![InferType::String],
                return_type: Box::new(InferType::Result(
                    Box::new(InferType::String),
                    Box::new(InferType::String),
                )),
                effect: self.fresh_effect(),
            },
            Builtin::UrlQuery => InferType::Function {
                parameters: vec![InferType::List(Box::new(InferType::Record {
                    fields: BTreeMap::from([
                        ("name".to_owned(), InferType::String),
                        ("value".to_owned(), InferType::String),
                    ]),
                    open: false,
                }))],
                return_type: Box::new(InferType::String),
                effect: self.fresh_effect(),
            },
            Builtin::StringByteLen => InferType::Function {
                parameters: vec![InferType::String],
                return_type: Box::new(InferType::Int),
                effect: self.fresh_effect(),
            },
            Builtin::StringIsEmpty => InferType::Function {
                parameters: vec![InferType::String],
                return_type: Box::new(InferType::Bool),
                effect: self.fresh_effect(),
            },
            Builtin::StringContains | Builtin::StringStartsWith | Builtin::StringEndsWith => {
                InferType::Function {
                    parameters: vec![InferType::String, InferType::String],
                    return_type: Box::new(InferType::Bool),
                    effect: self.fresh_effect(),
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

/// Decides whether `?` may exit the named function `name`, declared with
/// `return_type` (analyzed as `declared`).
fn named_exit_context(
    name: &str,
    return_type: Option<&TypeAnnotation>,
    declared: &InferType,
) -> ExitContext {
    let Some(annotation) = return_type else {
        return ExitContext::Refused(Refusal::Undeclared {
            name: name.to_owned(),
        });
    };
    let (family, error) = match declared {
        InferType::Result(_, error) => (VariantFamily::Result, Some(error.as_ref().clone())),
        InferType::Option(_) => (VariantFamily::Option, None),
        _ => {
            return ExitContext::Refused(Refusal::Other {
                name: name.to_owned(),
                return_type: annotation.to_string(),
            });
        }
    };
    ExitContext::Propagates(Propagation {
        name: name.to_owned(),
        family,
        error,
        return_type: annotation.to_string(),
    })
}

/// `K3012`: the `?` at `question` has no function it may return from, for
/// the reason `refusal`.
fn propagation_refused(refusal: &Refusal, question: Span) -> Diagnostic {
    let (message, note) = match refusal {
        Refusal::TopLevel => (
            "`?` is not allowed at the top level".to_owned(),
            "`?` returns early from the enclosing named function; at the top level, \
             handle the value with `match`"
                .to_owned(),
        ),
        Refusal::Closure => (
            "`?` is not allowed in an anonymous function".to_owned(),
            "`?` returns only from a named function; handle the value with `match` here, \
             or move the code into a named `fn` that returns `Result` or `Option`"
                .to_owned(),
        ),
        Refusal::Webhook => (
            "`?` is not allowed in a webhook entrypoint, which returns `HttpResponse`".to_owned(),
            "bind each fallible step with let-else here, as in \
             `let Ok(value) = step else Err(error) => response;`, or move the steps into a \
             helper `fn` that returns `Result<HttpResponse, String>` and use `?` there"
                .to_owned(),
        ),
        Refusal::Undeclared { name } => (
            format!("`?` is not allowed in `{name}`, which has no declared return type"),
            format!(
                "declare the return type of `{name}`, as in `-> Result<T, E>` or \
                 `-> Option<T>`; `?` returns its `Err` or `None` from there"
            ),
        ),
        Refusal::Other { name, return_type } => (
            format!("`?` is not allowed in `{name}`, which returns `{return_type}`"),
            format!(
                "`?` returns an `Err` or `None` early, so `{name}` must return `Result<_, E>` \
                 or `Option<_>`; otherwise handle the value with `match`"
            ),
        ),
    };
    Diagnostic::new("K3012", message, question).with_note(note)
}

/// The source spelling of a method call's receiver when it is a name or a
/// short field path (`s`, `request.body`), for use in a fix; `None` for any
/// other expression.
fn receiver_path(receiver: &Expression) -> Option<String> {
    const MAX_FIELDS: usize = 4;
    let mut fields = Vec::new();
    let mut current = receiver;
    loop {
        match &current.kind {
            ExpressionKind::Variable(name) => {
                fields.push(name.as_str());
                break;
            }
            ExpressionKind::FieldAccess { value, field } if fields.len() < MAX_FIELDS => {
                fields.push(field);
                current = value;
            }
            _ => return None,
        }
    }
    fields.reverse();
    Some(fields.join("."))
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

/// The built-in nominal type that `contract` constructs a record of.
const fn contract_nominal(contract: ContractRecord) -> InferType {
    match contract {
        ContractRecord::HttpRequest => InferType::HttpRequest,
        ContractRecord::HttpResponse => InferType::HttpResponse,
        ContractRecord::HttpHeader => InferType::HttpHeader,
        ContractRecord::LogField => InferType::LogField,
    }
}

/// Checks the written field names of a named construction (`K3016`): each
/// must belong to the contract and be written once, and every required field
/// must be written. Fields are checked in source order, and a missing field
/// is reported at the type name after every written field has been checked.
pub(crate) fn check_construction_fields(
    contract: ContractRecord,
    contract_span: Span,
    fields: &[ConstructField],
) -> Result<(), Diagnostic> {
    let name = contract.name();
    let all_fields = || {
        format!(
            "`{name}` has the fields {}",
            code_list(contract.fields().iter().map(|field| field.name))
        )
    };
    let mut written = BTreeSet::new();
    for field in fields {
        if contract.field(&field.name).is_none() {
            let note = match closest_name(
                &field.name,
                contract.fields().iter().map(|field| field.name),
            ) {
                Some(candidate) => format!("did you mean `{candidate}`? {}", all_fields()),
                None => all_fields(),
            };
            return Err(Diagnostic::new(
                "K3016",
                format!("`{name}` has no field `{}`", field.name),
                field.name_span,
            )
            .with_note(note));
        }
        if !written.insert(field.name.as_str()) {
            return Err(Diagnostic::new(
                "K3016",
                format!(
                    "field `{}` is written more than once in `{name} {{ ... }}`",
                    field.name
                ),
                field.name_span,
            )
            .with_note(format!("write each field once; {}", all_fields())));
        }
    }
    let missing = contract
        .fields()
        .iter()
        .filter(|field| field.default.is_none() && !written.contains(field.name))
        .map(|field| field.name)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    let required = contract
        .fields()
        .iter()
        .filter(|field| field.default.is_none())
        .map(|field| field.name);
    let defaults = contract
        .fields()
        .iter()
        .filter_map(|field| {
            field
                .default
                .map(|default| format!("`{}` defaults to `{}`", field.name, default.as_source()))
        })
        .collect::<Vec<_>>();
    let note = if defaults.is_empty() {
        format!(
            "`{name}` requires {} and has no defaults",
            code_list(required)
        )
    } else {
        format!(
            "`{name}` requires {}; {}",
            code_list(required),
            prose_list(defaults)
        )
    };
    Err(Diagnostic::new(
        "K3016",
        format!(
            "`{name} {{ ... }}` is missing required {} {}",
            if missing.len() == 1 {
                "field"
            } else {
                "fields"
            },
            code_list(missing.iter().copied())
        ),
        contract_span,
    )
    .with_note(note))
}

/// Joins names as code spans in an English list: "`a`", "`a` and `b`", or
/// "`a`, `b`, and `c`".
fn code_list<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    prose_list(names.into_iter().map(|name| format!("`{name}`")).collect())
}

/// Joins phrases in an English list with a serial comma.
fn prose_list(items: Vec<String>) -> String {
    match items.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
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
    use crate::{
        Source,
        habits::Habit,
        limits::deepest::{self, Level},
        parse_source,
    };

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
    fn orders_two_integers_or_two_strings_only() {
        let analysis = check(
            r#"
            fn before(left: String, right: String) {
                left < right
            }
            fn later(left, right) {
                left >= right
            }
            let numbers = later(2, 1);
            let words = before("a", "b") && "b" > "a" && "a" <= "a";
            "#,
        )
        .expect("String and Int ordering should check");
        assert_eq!(
            analysis.bindings()[0].ty().to_string(),
            "fn(String, String) -> Bool effects {}"
        );
        assert_eq!(
            analysis.bindings()[1].ty().to_string(),
            "fn(Int, Int) -> Bool effects {}"
        );

        for (source, code) in [
            ("let value = 1 < \"one\";", "K3001"),
            ("let value = true < false;", "K3001"),
            ("let value = [1] <= [2];", "K3001"),
            ("let value = record { a: 1 } > record { a: 2 };", "K3001"),
            (
                "fn later(left, right) { left > right }\nlet value = later(1, \"one\");",
                "K3001",
            ),
        ] {
            let error = check(source).expect_err("ordering should reject other operands");
            assert_eq!(error.code(), code, "{source}");
        }
        let secret = check(
            r#"
            fn compare(left: Secret, right: Secret) -> Bool {
                left < right
            }
            "#,
        )
        .expect_err("secrets must not be ordered");
        assert_eq!(secret.code(), "K3009");
        assert!(secret.message().contains("ordered"), "{}", secret.message());
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
    fn rejects_non_exhaustive_and_malformed_pattern_ast() {
        let program_with = |pattern: Pattern| {
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
                    arms: vec![MatchArm {
                        pattern,
                        value: arm_value,
                        span: Span::new(13, 22),
                    }],
                },
                span: Span::new(0, 24),
            };
            Program {
                statements: vec![Statement {
                    kind: StatementKind::Expression(expression),
                    span: Span::new(0, 25),
                }],
            }
        };
        let leaf = |kind| Pattern {
            kind,
            span: Span::new(13, 17),
        };

        let none = leaf(PatternKind::Variant {
            variant: VariantName::None,
            payload: None,
        });
        let error = analyze(&program_with(none)).expect_err("one arm is not exhaustive");
        assert_eq!(error.code(), "K3005");
        assert_eq!(error.message(), "non-exhaustive match; missing: `Some(_)`");

        for malformed in [
            PatternKind::Variant {
                variant: VariantName::None,
                payload: Some(Box::new(leaf(PatternKind::Wildcard))),
            },
            PatternKind::Variant {
                variant: VariantName::Some,
                payload: None,
            },
            PatternKind::Cons {
                head: Box::new(leaf(PatternKind::EmptyList)),
                tail: Box::new(leaf(PatternKind::Wildcard)),
            },
        ] {
            let error = analyze(&program_with(leaf(malformed)))
                .expect_err("a pattern the parser never builds should fail defensively");
            assert_eq!(error.code(), "K1003");
        }
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
        // A block expression counts two levels of analysis recursion for its
        // one syntactic level and an `else` block three, so 256 nested `else`
        // blocks at the structural limit need `MAX_TREE_DEPTH - 1` levels.
        for level in [Level::Else, Level::Block] {
            check_on_compiler_stack(deepest::source(level, 0)).unwrap_or_else(|error| {
                panic!("{level:?}: the deepest accepted tree should analyze: {error:?}")
            });
            let error = check_on_compiler_stack(deepest::source(level, 1))
                .expect_err("one more link should exceed the structural limit");
            assert_eq!(error.code(), "K1005", "{level:?}");
            assert!(
                error.message().contains("4096"),
                "{level:?}: {}",
                error.message()
            );
        }
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

    fn error_notes(text: &str) -> (&'static str, String, Vec<String>) {
        let error = check(text).expect_err("program should not check");
        (
            error.code(),
            error.message().to_owned(),
            error.notes().to_vec(),
        )
    }

    #[test]
    fn suggests_the_closest_visible_name_for_an_undefined_name() {
        let (code, message, notes) = error_notes("let total = 1;\nprintln(totl);");
        assert_eq!(code, "K2001");
        assert_eq!(message, "undefined name `totl`");
        assert_eq!(notes, ["did you mean `total`?"]);

        // Built-ins, inner scopes, and the Boolean literals are candidates;
        // a binding that has gone out of scope is not.
        let (_, _, notes) = error_notes("pritnln(1);");
        assert_eq!(notes, ["did you mean `println`?"]);
        let (_, _, notes) = error_notes("fn f(count) { let counter = count; conuter }");
        assert_eq!(notes, ["did you mean `counter`?"]);
        let (_, _, notes) = error_notes("fn f() { let inner = 1; inner }\nprintln(innr);");
        assert!(notes.is_empty(), "{notes:?}");
        let (_, _, notes) = error_notes("let flag = True;");
        assert_eq!(notes, ["did you mean `true`?"]);
        let (_, _, notes) = error_notes("let value = some(1);");
        assert_eq!(notes, ["did you mean `Some`?"]);

        // Ties go to the fewest edits, then the bytewise smallest name.
        let (_, _, notes) = error_notes("let tokb = 1;\nlet toka = 2;\ntok;");
        assert_eq!(notes, ["did you mean `toka`?"]);
        let (_, _, notes) = error_notes("let missing_value = 1;\nlet value = missing;");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn explains_null_and_foreign_keywords_used_as_names() {
        for name in ["null", "nil", "undefined"] {
            let (code, _, notes) = error_notes(&format!("let value = {name};"));
            assert_eq!(code, "K2001");
            assert_eq!(
                notes,
                [format!(
                    "Krit has no `{name}`; write `None` for an absent `Option` value"
                )]
            );
        }
        for (text, habit) in [
            ("fn f() { return; }", Habit::Return),
            ("fn f() { return -1 }", Habit::Return),
            ("fn f(g) { await(g) }", Habit::Async),
            ("let json = require(\"json\");", Habit::Import),
        ] {
            let (code, _, notes) = error_notes(text);
            assert_eq!(code, "K2001", "{text}");
            assert_eq!(notes, [habit.hint()], "{text}");
        }

        let (code, _, notes) = error_notes("let value = None();");
        assert_eq!(code, "K3002");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("`None` is a value, not a function"));
        let (code, _, notes) = error_notes("let value = 1;\nvalue();");
        assert_eq!(code, "K3002");
        assert!(notes.is_empty());
    }

    #[test]
    fn reports_method_calls_with_a_standard_library_hint() {
        for (text, message, note) in [
            (
                "let name = \"krit\";\nname.len();",
                "cannot call `.len()` on a value of type `String`: Krit values have no methods",
                "call `string_byte_len(name)` instead",
            ),
            (
                "fn f(path: String) { path.startsWith(\"/\") }",
                "cannot call `.startsWith()` on a value of type `String`: Krit values have no \
                 methods",
                "call `string_starts_with(path, prefix)` instead",
            ),
            (
                "let count = 1;\ncount.toString();",
                "cannot call `.toString()` on a value of type `Int`: Krit values have no methods",
                "call `int_to_string(count)` instead",
            ),
            (
                "let items = [1];\nitems.len();",
                "cannot call `.len()` on a value of type `List<Int>`: Krit values have no methods",
                "match the list with `[]` and `[head, ..tail]` patterns instead",
            ),
            (
                "let found = Some(1);\nfound.unwrap();",
                "cannot call `.unwrap()` on a value of type `Option<Int>`: Krit values have no \
                 methods",
                "handle both outcomes with `match` instead",
            ),
            (
                "let item = record { body: \"x\" };\nitem.body.len();",
                "cannot call `.len()` on a value of type `String`: Krit values have no methods",
                "call `string_byte_len(item.body)` instead",
            ),
            (
                "let item = record { a: 1 };\nitem.send(1);",
                "cannot call `.send()`: `Record { a: Int }` has no field `send`, and Krit values \
                 have no methods",
                "pass the value to a function instead, as in `f(item)`",
            ),
            (
                "webhook fn handle(request: HttpRequest) -> HttpResponse {\n\
                 request.json()\n}",
                "cannot call `.json()`: `HttpRequest` has no field `json`, and Krit values have \
                 no methods",
                "pass the value to a function instead, as in `f(request)`",
            ),
        ] {
            let (code, actual, notes) = error_notes(text);
            assert_eq!(code, "K3011", "{text}");
            assert_eq!(actual, message, "{text}");
            assert_eq!(notes.len(), 1, "{text}");
            assert!(notes[0].starts_with(note), "{text}: {}", notes[0]);
        }
    }

    #[test]
    fn field_access_and_calls_of_function_fields_keep_their_meaning() {
        // A call of a function stored in a record field is not a method call.
        check("let item = record { run: fn() { 1 } };\nitem.run();")
            .expect("a function field should be callable");
        check("fn f(item) { item.run() }").expect("an open record may hold a function field");

        // A plain field access is still K3004, and so is a failing access in
        // the receiver of a method call.
        let (code, _, notes) = error_notes("let name = \"krit\";\nname.len;");
        assert_eq!(code, "K3004");
        assert!(notes.is_empty());
        let (code, message, _) = error_notes("let item = record { a: 1 };\nitem.b.c();");
        assert_eq!(code, "K3004");
        assert_eq!(message, "record type has no field `b`");

        // A field that exists but does not hold a function is a call error.
        let (code, _, _) = error_notes("let item = record { a: 1 };\nitem.a();");
        assert_eq!(code, "K3002");
    }

    /// The type and effects analysis recorded for the expression spelled
    /// `snippet` in `text`.
    fn expression_fact(text: &str, snippet: &str) -> (String, String) {
        let analysis = check(text).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        let start = text
            .find(snippet)
            .expect("snippet should occur in the text");
        let fact = analysis
            .expression(Span::new(start, start + snippet.len()))
            .expect("the snippet should be an analyzed expression");
        (fact.ty().to_string(), fact.effects().to_string())
    }

    #[test]
    fn postfix_question_marks_unwrap_results_and_options() {
        let text = "fn digit(text: String) -> Result<Int, String> { Err(text) }\n\
                    fn total(text: String) -> Result<Int, String> { Ok(digit(text)? + 1) }";
        assert_eq!(
            expression_fact(text, "digit(text)?"),
            ("Int".to_owned(), "{}".to_owned())
        );
        let text = "fn first(items: List<String>) -> Option<String> {\n\
                    match items { [] => None, [head, .._] => Some(head) }\n}\n\
                    fn twice(items: List<String>) -> Option<String> { Some(first(items)? + \"!\") }";
        assert_eq!(
            expression_fact(text, "first(items)?"),
            ("String".to_owned(), "{}".to_owned())
        );

        // `?` adds no effect: the operand's effects are its effects.
        let text = "fn logged() -> Option<Int> { println(1); Some(1) }\n\
                    fn use_it() -> Option<Int> { Some(logged()?) }";
        assert_eq!(
            expression_fact(text, "logged()?"),
            ("Int".to_owned(), "{io.stdout}".to_owned())
        );

        // An operand of unknown type takes the declared family and error type.
        let text = "fn pass(value) -> Result<Bool, String> { Ok(value?) }";
        let analysis = check(text).expect("an inferred operand should check");
        assert_eq!(
            analysis.bindings()[0].ty().to_string(),
            "fn(Result<Bool, String>) -> Result<Bool, String> effects {}"
        );
        check("fn pass(value) -> Option<Int> { Some(value? + 1) }")
            .expect("an inferred Option operand should check");

        // Entrypoints that return `Result<String, String>` accept `?`, and the
        // innermost named function decides.
        check(
            "queue \"jobs\" fn work(job: QueueJob) -> Result<String, String> {\n\
             Ok(config_string(\"agent.model\")? + job.body)\n}",
        )
        .expect("a queue entrypoint should accept `?`");
        check(
            "schedule \"tick\" fn tick(event: ScheduleEvent) -> Result<String, String> {\n\
             Ok(config_string(\"agent.model\")?)\n}",
        )
        .expect("a schedule entrypoint should accept `?`");
        check(
            "fn outer() -> Int {\n\
             fn inner(value: Option<Int>) -> Option<Int> { Some(value? + 1) }\n\
             match inner(Some(1)) { Some(x) => x, None => 0 }\n}",
        )
        .expect("a nested named function returning Option should accept `?`");
    }

    #[test]
    fn postfix_question_marks_need_a_function_that_returns_their_family() {
        for (text, code, message) in [
            (
                "let x = Some(1)?;",
                "K3012",
                "`?` is not allowed at the top level",
            ),
            (
                "fn f(x: Option<Int>) -> Option<Int> { let g = fn() -> Option<Int> { Some(x?) };\n\
                 g() }",
                "K3012",
                "`?` is not allowed in an anonymous function",
            ),
            (
                "webhook fn handle(request: HttpRequest) -> HttpResponse {\n\
                 let model = config_string(\"agent.model\")?;\n\
                 record { status: 200, headers: [], body: model }\n}",
                "K3012",
                "`?` is not allowed in a webhook entrypoint, which returns `HttpResponse`",
            ),
            (
                "fn f(x: Option<Int>) { x? }",
                "K3012",
                "`?` is not allowed in `f`, which has no declared return type",
            ),
            (
                "fn f(x: Option<Int>) -> Int { x? }",
                "K3012",
                "`?` is not allowed in `f`, which returns `Int`",
            ),
            (
                "fn f(x: Option<Int>) -> Option<Int> { fn g(y: Option<Int>) -> Int { y? }\n\
                 Some(g(x)) }",
                "K3012",
                "`?` is not allowed in `g`, which returns `Int`",
            ),
            (
                "fn f(x: Result<Int, Int>) -> Result<Int, String> { Ok(x?) }",
                "K3013",
                "`?` cannot return this `Err` from `f`: its error type differs from the \
                 declared `Result<Int, String>`",
            ),
            (
                "fn f(x: Option<Int>) -> Result<Int, String> { Ok(x?) }",
                "K3013",
                "`?` on an `Option` value cannot return from `f`, which returns \
                 `Result<Int, String>`",
            ),
            (
                "fn f(x: Result<Int, String>) -> Option<Int> { Some(x?) }",
                "K3013",
                "`?` on a `Result` value cannot return from `f`, which returns `Option<Int>`",
            ),
        ] {
            let error = check(text).expect_err("misplaced `?` should not check");
            assert_eq!(error.code(), code, "{text}");
            assert_eq!(error.message(), message, "{text}");
            assert_eq!(error.notes().len(), 1, "{text}");
            let question = text.rfind('?').expect("text has a `?`");
            assert_eq!(error.span(), Span::new(question, question + 1), "{text}");
        }

        let error = check("fn f(x: Int) -> Option<Int> { Some(x?) }").expect_err("not a variant");
        assert_eq!(error.code(), "K3001");
        // Propagating rebuilds the `Err`, so an opaque error type is refused.
        let error = check(
            "fn g() -> Result<Int, Secret> { Ok(1) }\n\
             fn f() -> Result<Int, Secret> { Ok(g()?) }",
        )
        .expect_err("a Secret error should not propagate");
        assert_eq!(error.code(), "K3009");
        // The operand is checked before the `?`.
        let error = check("let x = missing?;").expect_err("undefined operand");
        assert_eq!(error.code(), "K2001");
    }

    /// The span of `snippet`, which occurs once in `text`.
    fn span_of(text: &str, snippet: &str) -> Span {
        let start = text
            .find(snippet)
            .expect("snippet should occur in the text");
        assert_eq!(
            text.rfind(snippet),
            Some(start),
            "snippet should occur once"
        );
        Span::new(start, start + snippet.len())
    }

    #[test]
    fn let_else_binds_the_payload_and_types_its_arm_as_the_function_result() {
        let statement = "let Ok(value) = digit(text) else Err(error) => 0;";
        let text = format!(
            "fn digit(text: String) -> Result<Int, String> {{ Err(text) }}\n\
             fn total(text: String) -> Int {{\n    {statement}\n    value + 1\n}}"
        );
        let analysis = check(&text).expect("let-else should check");
        // The pattern's binder is a `let` binding at the statement, and the
        // arm's binder a match binding at the arm.
        let value = analysis
            .symbol(span_of(&text, statement), "value", SymbolKind::Let)
            .expect("the binder should be a let symbol");
        assert_eq!(value.ty().to_string(), "Int");
        assert!(!value.is_top_level());
        let error = analysis
            .symbol(
                span_of(&text, "Err(error) => 0"),
                "error",
                SymbolKind::Match,
            )
            .expect("the arm binder should be a match symbol");
        assert_eq!(error.ty().to_string(), "String");
        assert!(
            analysis
                .bindings()
                .iter()
                .any(|binding| binding.name() == "value" && binding.ty().to_string() == "Int")
        );

        // The arm's value is the function's return value, so it decides an
        // undeclared result type, and the annotation types the value.
        let function_type = |text: &str, name: &str| {
            check(text)
                .unwrap_or_else(|error| panic!("{text} should check: {error:?}"))
                .bindings()
                .iter()
                .find(|binding| binding.name() == name)
                .expect("the function should have a binding")
                .ty()
                .to_string()
        };
        let text =
            "fn pick(value) {\n    let Some(found) = value else None => \"none\";\n    found\n}";
        assert_eq!(
            function_type(text, "pick"),
            "fn(Option<String>) -> String effects {}"
        );
        let text = "fn decode(raw: String) -> String {\n    \
                    let Ok(text): Result<String, String> = json_decode(raw) else Err(e) => e;\n    \
                    text\n}";
        check(text).expect("an annotated value should check");

        // The effects of the value and the arm are the statement's effects.
        let text = "fn noisy(x: Option<Int>) -> Int {\n    \
                    let Some(v) = x else None => { println(0); 0 };\n    v\n}";
        assert_eq!(
            function_type(text, "noisy"),
            "fn(Option<Int>) -> Int effects {io.stdout}"
        );

        // Either variant may come first, `_` may repeat, and every named
        // function body accepts let-else, including entrypoints and nested
        // named functions.
        for text in [
            "fn f(r: Result<Int, String>) -> String {\n    let Err(e) = r else Ok(v) => \"ok\";\n    e\n}",
            "fn f(o: Option<Int>) -> Int {\n    let None = o else Some(v) => v;\n    0\n}",
            "fn f(r: Result<Int, String>) -> Int {\n    let Ok(_) = r else Err(_) => 0;\n    let Ok(_) = r else Err(_) => 1;\n    2\n}",
            "webhook fn handle(request: HttpRequest) -> HttpResponse {\n    \
             let Ok(model) = config_string(\"agent.model\") else\n        \
             Err(error) => record { status: 500, headers: [], body: error };\n    \
             record { status: 200, headers: [], body: model }\n}",
            "queue \"jobs\" fn work(job: QueueJob) -> Result<String, String> {\n    \
             let Ok(model) = config_string(\"agent.model\") else Err(error) => Err(error);\n    \
             Ok(model + job.body)\n}",
            "fn outer() -> Int {\n    fn inner(o: Option<Int>) -> Int {\n        \
             let Some(v) = o else None => 0;\n        v\n    }\n    inner(Some(1))\n}",
            "fn f(x: Option<Int>) -> Int {\n    let Some(x) = x else None => 0;\n    x\n}",
        ] {
            let report = analyze_with_warnings(
                &parse_source(&Source::new("test.krit", text)).expect("text should parse"),
            );
            report
                .result
                .unwrap_or_else(|error| panic!("{text} should check: {error:?}"));
            assert!(report.warnings.is_empty(), "{text}");
        }
    }

    #[test]
    fn let_else_is_a_direct_statement_of_a_named_function_body() {
        for (text, head, message) in [
            (
                "let Ok(x) = Ok(1) else Err(e) => 0;",
                "let Ok(x)",
                "let-else is not allowed at the top level",
            ),
            (
                "let v = {\n    let Some(x) = Some(1) else None => 0;\n    x\n};",
                "let Some(x)",
                "let-else is not allowed at the top level",
            ),
            (
                "fn f(r: Result<Int, String>) -> Int {\n    let g = fn() -> Int {\n        \
                 let Ok(x) = r else Err(e) => 0;\n        x\n    };\n    g()\n}",
                "let Ok(x)",
                "let-else is not allowed in an anonymous function",
            ),
            (
                "fn f(r: Result<Int, String>) -> Int {\n    let y = {\n        \
                 let Ok(x) = r else Err(e) => 0;\n        x\n    };\n    y\n}",
                "let Ok(x)",
                "let-else must be a statement of the function body itself, not of a nested block",
            ),
            (
                "fn f(r: Result<Int, String>, c: Bool) -> Int {\n    if c {\n        \
                 let Ok(x) = r else Err(e) => 0;\n        x\n    } else {\n        1\n    }\n}",
                "let Ok(x)",
                "let-else must be a statement of the function body itself, not of a nested block",
            ),
            (
                "fn f(r: Result<Int, String>) -> Int {\n    let Ok(x) = r else None => 0;\n    x\n}",
                "None => 0",
                "the arm of a let-else on `Ok` must match `Err`, found `None`",
            ),
            (
                "fn f(o: Option<Int>) -> Int {\n    let Some(x) = o else Some(y) => y;\n    x\n}",
                "Some(y) => y",
                "the arm of a let-else on `Some` must match `None`, found `Some`",
            ),
        ] {
            let error = check(text).expect_err("misplaced let-else should not check");
            assert_eq!(error.code(), "K3014", "{text}");
            assert_eq!(error.message(), message, "{text}");
            assert_eq!(error.notes().len(), 1, "{text}");
            assert_eq!(error.span(), span_of(text, head), "{text}");
        }

        for (text, code, at) in [
            // The value belongs to the pattern's family.
            (
                "fn f(r: Int) -> Int {\n    let Ok(x) = r else Err(e) => 0;\n    x\n}",
                "K3005",
                "r else",
            ),
            (
                "fn f(r: Option<Int>) -> Int {\n    let Ok(x) = r else Err(e) => 0;\n    x\n}",
                "K3005",
                "r else",
            ),
            // The arm has the function's result type.
            (
                "fn f(r: Result<Int, String>) -> Int {\n    let Ok(x) = r else Err(e) => e;\n    x\n}",
                "K3001",
                "e;",
            ),
            (
                "fn f(r: Result<Int, String>) -> Int {\n    \
                 let Ok(x): Option<Int> = r else Err(e) => 0;\n    x\n}",
                "K3001",
                "Option<Int>",
            ),
            // The pattern's binder is declared in the body's scope, after the arm.
            (
                "fn f(r: Result<Int, String>) -> Int {\n    let x = 1;\n    \
                 let Ok(x) = r else Err(e) => 0;\n    x\n}",
                "K2002",
                "let Ok(x)",
            ),
            (
                "fn f(r: Result<Int, String>) -> Int {\n    let Ok(x) = r else Err(e) => x;\n    x\n}",
                "K2001",
                "x;\n    x",
            ),
            (
                "fn f(r: Result<Int, String>) -> String {\n    let Ok(x) = r else Err(e) => e;\n    e\n}",
                "K2001",
                "e\n}",
            ),
        ] {
            let error = check(text).expect_err("ill-typed let-else should not check");
            assert_eq!(error.code(), code, "{text}");
            assert_eq!(error.span().start, span_of(text, at).start, "{text}");
        }
    }

    #[test]
    fn nested_patterns_bind_payload_types_in_any_arm_order() {
        let text = "fn describe(value: Result<Option<String>, Int>) -> String {\n    \
                    match value {\n        Err(code) => \"error\",\n        \
                    Ok(None) => \"missing\",\n        Ok(Some(text)) => text,\n    }\n}\n\
                    fn first(items) {\n    match items {\n        [_, ..rest] => rest,\n        \
                    [] => [1],\n    }\n}";
        let analysis = check(text).unwrap_or_else(|error| panic!("{error:?}"));
        for (snippet, name, ty) in [
            ("code", "code", "Int"),
            ("text)", "text", "String"),
            ("rest]", "rest", "List<Int>"),
        ] {
            let start = text.find(snippet).expect("binder should occur");
            let span = Span::new(start, start + name.len());
            let symbol = analysis
                .symbol(span, name, SymbolKind::Match)
                .unwrap_or_else(|| panic!("`{name}` should be a match symbol at its own span"));
            assert_eq!(symbol.ty().to_string(), ty, "{name}");
        }
        // `_` is a wildcard for the arm rules, and it binds the discard name
        // at its own span like any binder.
        let start = text.find("_,").expect("the head `_` should occur");
        let discard = analysis
            .symbol(Span::new(start, start + 1), "_", SymbolKind::Match)
            .expect("`_` should be a match symbol at its own span");
        assert_eq!(discard.ty().to_string(), "Int");
    }

    #[test]
    fn non_exhaustive_matches_list_every_missing_pattern() {
        for (text, missing) in [
            (
                "fn f(v: Result<Option<String>, String>) -> Int { match v { Ok(Some(t)) => 1, Err(e) => 2 } }",
                "`Ok(None)`",
            ),
            (
                "fn f(v: Result<Option<String>, String>) -> Int { match v { Ok(Some(t)) => 1 } }",
                "`Ok(None)`, `Err(_)`",
            ),
            (
                "fn f(v: Option<Result<List<Int>, String>>) -> Int { match v { Some(Ok([])) => 1, None => 0 } }",
                "`Some(Ok([_, .._]))`, `Some(Err(_))`",
            ),
            (
                "fn f(v: List<Int>) -> Int { match v { [_, ..rest] => 1 } }",
                "`[]`",
            ),
            ("fn f(v) -> Int { match v { None => 1 } }", "`Some(_)`"),
        ] {
            let (code, message, notes) = error_notes(text);
            assert_eq!(code, "K3005", "{text}");
            assert_eq!(
                message,
                format!("non-exhaustive match; missing: {missing}"),
                "{text}"
            );
            assert_eq!(notes.len(), 1, "{text}");
            assert!(
                notes[0].starts_with("add an arm for each missing pattern: `"),
                "{notes:?}"
            );
        }
    }

    #[test]
    fn catch_all_arms_are_rejected_with_the_outcomes_to_name() {
        for (text, at, note) in [
            (
                "fn f(v: Result<Option<String>, String>) -> Int { match v { Ok(Some(t)) => 1, _ => 2 } }",
                "_ =>",
                "name every outcome: replace `_ => ...` with `Ok(None) => ...` and `Err(_) => ...`",
            ),
            (
                "fn f(v: Result<Int, String>) -> Int { match v { other => 2 } }",
                "other",
                "name every outcome: replace `other => ...` with `Ok(_) => ...` and `Err(_) => ...`",
            ),
            (
                "fn f(v: Option<Int>) -> Int { match v { _ => 0, Some(x) => x, None => 1 } }",
                "_ =>",
                "name every outcome: the other arms already do, so remove `_ => ...`",
            ),
            (
                "fn f(v) -> Int { match v { _ => 2 } }",
                "_ =>",
                "name every outcome with its own arm, such as `Some(value)` and `None`, \
                 `Ok(value)` and `Err(error)`, or `[]` and `[head, ..tail]`",
            ),
        ] {
            let error = check(text).expect_err(text);
            assert_eq!(error.code(), "K3005", "{text}");
            assert_eq!(error.span().start, span_of(text, at).start, "{text}");
            assert!(
                error
                    .message()
                    .starts_with("a match arm cannot match every value")
            );
            assert_eq!(error.notes(), [note], "{text}");
        }
        // A catch-all inside a payload names its variant and is allowed.
        check("fn f(v: Result<Option<Int>, String>) -> Int { match v { Ok(Some(x)) => x, Ok(None) => 0, Err(_) => 1 } }")
            .expect("a payload `_` is not a catch-all arm");
    }

    #[test]
    fn duplicate_unreachable_overlapping_and_mixed_arms_are_rejected() {
        for (text, at, message, note) in [
            (
                "fn f(v: Option<Int>) -> Int { match v { Some(t) => 1, Some(u) => 3, None => 2 } }",
                "Some(u)",
                "duplicate match arm: `Some(u)` matches exactly what the earlier arm `Some(t)` matches",
                Some("remove one of the two arms"),
            ),
            (
                "fn f(v: Result<Option<Int>, Int>) -> Int { match v { Ok(x) => 1, Ok(Some(t)) => 3, Err(e) => 2 } }",
                "Ok(Some(t))",
                "unreachable match arm: the earlier arm `Ok(x)` already matches every value `Ok(Some(t))` matches",
                Some("remove this arm"),
            ),
            (
                "fn f(v: Option<Option<Int>>) -> Int { match v { Some(Some(t)) => 1, Some(None) => 3, Some(x) => 4, None => 2 } }",
                "Some(x)",
                "unreachable match arm: earlier arms already match every value `Some(x)` matches",
                Some("remove this arm"),
            ),
            (
                "fn f(v: Result<Option<Option<Int>>, Int>) -> Int { match v { Ok(Some(Some(t))) => 1, Ok(x) => 3, Err(e) => 2 } }",
                "Ok(x)",
                "match arm `Ok(x)` overlaps the earlier arm `Ok(Some(Some(t)))`: arms may not overlap",
                Some(
                    "name each remaining outcome instead: replace `Ok(x) => ...` with `Ok(Some(None)) => ...` and `Ok(None) => ...`",
                ),
            ),
            (
                "fn f(v: Option<Int>) -> Int { match v { Some(value) => value, Err(error) => error } }",
                "Err(error)",
                "`Err(error)` is a `Result` pattern, but the earlier arm `Some(value)` matches an `Option` value here",
                None,
            ),
            (
                "fn f(v) -> Int { match v { Some(Ok(t)) => 1, Some(None) => 3, None => 2 } }",
                "None) =>",
                "`None` is an `Option` pattern, but the earlier arm `Some(Ok(t))` matches a `Result` value here",
                None,
            ),
            (
                "fn f(v) -> Int { match v { [] => 1, None => 3 } }",
                "None",
                "`None` is an `Option` pattern, but the earlier arm `[]` matches a `List` value here",
                None,
            ),
        ] {
            let error = check(text).expect_err(text);
            assert_eq!(error.code(), "K1003", "{text}");
            assert_eq!(error.span().start, span_of(text, at).start, "{text}");
            assert_eq!(error.message(), message, "{text}");
            match note {
                Some(note) => assert_eq!(error.notes(), [note], "{text}"),
                None => assert_eq!(error.notes().len(), 1, "{text}"),
            }
        }
    }

    #[test]
    fn typing_a_match_unifies_each_position_once_however_many_arms_reach_it() {
        // An exhaustive match over `Result<Result<..>, Result<..>>` nested
        // `depth` deep has 2^depth arms, each `depth` patterns long, but only
        // 2^(depth + 1) - 1 positions; typing creates the payload types of
        // each position once, not once per arm that passes through it.
        let depth = 8;
        let mut ty = "Int".to_owned();
        for _ in 0..depth {
            ty = format!("Result<{ty}, {ty}>");
        }
        let arms = (0..1_u32 << depth)
            .map(|index| {
                let mut pattern = "x".to_owned();
                for level in 0..depth {
                    let variant = if index >> level & 1 == 0 { "Ok" } else { "Err" };
                    pattern = format!("{variant}({pattern})");
                }
                format!("{pattern} => {index}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!("fn f(v: {ty}) -> Int {{ match v {{ {arms} }} }}");
        let program = parse_source(&Source::new("wide.krit", text.as_str()))
            .expect("the wide match should parse");
        let mut analyzer = Analyzer::new();
        analyzer
            .program(&program)
            .expect("the wide match should check");
        let positions = (1_u32 << (depth + 1)) - 1;
        let variables = analyzer.next_type_variable;
        assert!(
            variables <= 2 * positions,
            "{variables} type variables for {positions} positions"
        );
    }

    #[test]
    fn nested_patterns_are_typed_against_the_payload() {
        for (text, code, at) in [
            (
                "fn f(v: Option<Int>) -> Int { match v { Some(Ok(t)) => 1, Some(Err(e)) => 3, None => 2 } }",
                "K3005",
                "Ok(t)",
            ),
            (
                "fn f(v: Option<Int>) -> Int { match v { Some([]) => 1, Some([h, ..t]) => 3, None => 2 } }",
                "K3005",
                "[]",
            ),
            (
                "fn f(v: Int) -> Int { match v { Some(x) => 1, None => 2 } }",
                "K3005",
                "match",
            ),
            (
                "fn f(v: Result<Option<String>, Int>) -> Int { match v { Ok(Some(t)) => t, Ok(None) => 0, Err(e) => e } }",
                "K3001",
                "Ok(None)",
            ),
        ] {
            let error = check(text).expect_err(text);
            assert_eq!(error.code(), code, "{text}");
            assert_eq!(error.span().start, span_of(text, at).start, "{text}");
        }
        // An unresolved subject takes the patterns' families.
        let text = "fn f(v) { match v { Ok(Some(t)) => t + 1, Ok(None) => 0, Err(e) => e } }";
        let analysis = check(text).expect("an unresolved subject should check");
        let parameter = analysis
            .symbols()
            .iter()
            .find(|symbol| symbol.name() == "v")
            .expect("parameter");
        assert_eq!(parameter.ty().to_string(), "Result<Option<Int>, Int>");
    }

    #[test]
    fn reading_a_pattern_discard_warns_and_reads_the_value_at_its_position() {
        // Edition 2026 keeps 0.2's meaning: a pattern `_` binds the value at
        // its position like `let _`, so reading it sees the most recent `_`
        // (the tail of `[_, .._]`, and the pattern's `_` over an outer one),
        // with K2003. Each annotation holds only for that binding.
        for (text, warnings) in [
            (
                "fn f(v: Option<Int>) -> Int { match v { Some(_) => _ + 1, None => 0 } }",
                1,
            ),
            (
                "fn f(_: String, v: Option<Int>) -> Int { match v { Some(_) => _, None => 0 } }",
                1,
            ),
            (
                "let _ = 1;\nlet x: List<String> = match [\"a\"] { [] => [], [_, .._] => _ };",
                1,
            ),
            (
                "fn f(r: Result<Bool, Int>) -> Int {\n    let Ok(_) = r else Err(_) => _;\n    \
                 if _ { 1 } else { 0 }\n}",
                2,
            ),
            (
                "fn f(_: String, r: Result<Int, Int>) -> Int {\n    \
                 let Ok(_) = r else Err(e) => e;\n    _\n}",
                1,
            ),
            (
                "fn f(_: String, v: Option<Int>) -> String { match v { Some(x) => _, None => \"\" } }",
                1,
            ),
        ] {
            let report = check_with_warnings(text);
            report
                .result
                .unwrap_or_else(|error| panic!("{text} should check: {error:?}"));
            assert_eq!(
                warning_codes(&report.warnings),
                vec!["K2003"; warnings],
                "{text}"
            );
        }

        // The read has the type of the value at the `_`'s position.
        let text = "fn f(v: Result<Option<String>, Int>) -> String {\n    match v {\n        \
                    Ok(Some(_)) => _,\n        Ok(None) => \"\",\n        Err(_) => \"\",\n    }\n}";
        let analysis = check(text).expect("reading a nested `_` should check");
        let read = text.find("=> _").expect("the read should occur") + 3;
        assert_eq!(
            analysis
                .expression(Span::new(read, read + 1))
                .expect("the read should have facts")
                .ty()
                .to_string(),
            "String"
        );
    }

    #[test]
    fn named_construction_has_the_structural_type_of_the_full_record() {
        let analysis = check(
            r#"
            let body = "ready";
            let response = HttpResponse { status: 200, body };
            let explicit = record { status: 200, headers: [], body: "ready" };
            let same = response == explicit;
            let request = HttpRequest { method: "GET", path: "/x" };
            let header = HttpHeader { name: "accept", value: "text/plain" };
            let field = LogField { name: "k", value: "v" };
            let full = HttpRequest {
                body,
                headers: [HttpHeader { name: "a", value: "b" }],
                query: "",
                path: "/",
                method: "POST",
            };
            fn status(value: HttpResponse) -> Int { value.status }
            let code = status(HttpResponse { status: 204 });
            let accepted: List<HttpHeader> = [LogField { name: "k", value: "v" }];
            "#,
        )
        .expect("constructions should check");
        let types = analysis
            .bindings()
            .iter()
            .map(|binding| (binding.name().to_owned(), binding.ty().to_string()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            types["response"],
            "Record { body: String, headers: List<HttpHeader>, status: Int }"
        );
        assert_eq!(
            types["request"],
            "Record { body: String, headers: List<HttpHeader>, method: String, path: String, query: String }"
        );
        assert_eq!(types["full"], types["request"]);
        assert_eq!(types["header"], "Record { name: String, value: String }");
        assert_eq!(types["field"], types["header"]);
        assert_eq!(types["same"], "Bool");

        // The punned field's value is a resolved reference to the binding.
        let text = "let body = \"x\";\nlet r = HttpResponse { status: 1, body };";
        let analysis = check(text).expect("punning should check");
        let start = text.rfind("body").expect("punned field");
        let fact = analysis
            .expression(Span::new(start, start + 4))
            .expect("the punned value has an expression fact");
        assert_eq!(
            fact.resolved_name(),
            Some(ResolvedName::Symbol(analysis.bindings()[0].id()))
        );
    }

    #[test]
    fn named_construction_values_carry_effects_and_requirements() {
        let analysis = check(
            r#"
            fn respond() -> HttpResponse {
                HttpResponse {
                    status: 200,
                    body: match config_string("agent.greeting") {
                        Ok(text) => text,
                        Err(error) => error,
                    },
                }
            }
            "#,
        )
        .expect("an effectful field value should check");
        let Type::Function(function) = analysis.bindings()[0].ty() else {
            panic!("respond should be a function");
        };
        assert!(function.effects().contains(&Effect::ConfigRead));
        assert!(
            function
                .requirements()
                .contains(&CapabilityRequirement::new(
                    Effect::ConfigRead,
                    "agent.greeting"
                ))
        );
    }

    #[test]
    fn reports_unknown_duplicate_and_missing_construction_fields() {
        for (text, message, note) in [
            (
                "let r = HttpResponse { stauts: 200 };",
                "`HttpResponse` has no field `stauts`",
                "did you mean `status`? `HttpResponse` has the fields `status`, `headers`, and `body`",
            ),
            (
                "let r = HttpHeader { name: \"a\", value: \"b\", extra: 1 };",
                "`HttpHeader` has no field `extra`",
                "`HttpHeader` has the fields `name` and `value`",
            ),
            (
                "let r = HttpResponse { status: 1, body: \"a\", body: \"b\" };",
                "field `body` is written more than once in `HttpResponse { ... }`",
                "write each field once; `HttpResponse` has the fields `status`, `headers`, and `body`",
            ),
            (
                "let r = HttpRequest { method: \"GET\" };",
                "`HttpRequest { ... }` is missing required field `path`",
                "`HttpRequest` requires `method` and `path`; `query` defaults to `\"\"`, \
                 `headers` defaults to `[]`, and `body` defaults to `\"\"`",
            ),
            (
                "let r = HttpResponse { body: \"x\" };",
                "`HttpResponse { ... }` is missing required field `status`",
                "`HttpResponse` requires `status`; `headers` defaults to `[]` and `body` \
                 defaults to `\"\"`",
            ),
            (
                "let r = LogField {};",
                "`LogField { ... }` is missing required fields `name` and `value`",
                "`LogField` requires `name` and `value` and has no defaults",
            ),
        ] {
            let (code, actual, notes) = error_notes(text);
            assert_eq!(code, "K3016", "{text}");
            assert_eq!(actual, message, "{text}");
            assert_eq!(notes, [note], "{text}");
        }

        // The unknown field is reported at its name, a missing field at the
        // type name, and the field set before any value is analyzed.
        let error = check("let r = HttpResponse { status: missing, stauts: 1 };")
            .expect_err("unknown field");
        assert_eq!(error.code(), "K3016");
        assert_eq!(error.span(), Span::new(40, 46));
        let error = check("let r = HttpRequest {\n    method: \"GET\",\n};").expect_err("missing");
        assert_eq!(error.span(), Span::new(8, 19));
    }

    #[test]
    fn construction_values_must_have_their_contract_field_types() {
        let error = check("let r = HttpResponse { status: \"200\" };").expect_err("mismatch");
        assert_eq!(error.code(), "K3001");
        assert_eq!(error.span(), Span::new(31, 36));
        assert_eq!(
            error.message(),
            "`HttpResponse` field `status` type mismatch: expected `Int`, found `String`"
        );
        let error = check("let r = HttpRequest { method: \"GET\", path: \"/\", headers: [1] };")
            .expect_err("mismatch");
        assert_eq!(error.code(), "K3001");
        let error =
            check("let s = secret(\"token\");\nlet r = HttpResponse { status: 1, body: s };")
                .expect_err("a secret result is not a body");
        assert_eq!(error.code(), "K3001");
    }

    #[test]
    fn interpolation_is_a_string_and_records_each_hole_at_its_own_span() {
        let text = "fn line(user, id: String) {\n    \"\\(user.name) #\\(id)\"\n}\n\
                    let shown = line(record { name: \"Ada\" }, \"7\");\n";
        let analysis = check(text).expect("interpolation should analyze");
        let line = &analysis.bindings()[0];
        assert_eq!(
            line.ty().to_string(),
            "fn(Record { name: String }, String) -> String effects {}"
        );
        let span_of = |needle: &str| {
            let start = text.find(needle).expect("needle should occur");
            Span::new(start, start + needle.len())
        };
        let string = span_of("\"\\(user.name) #\\(id)\"");
        assert_eq!(
            analysis.expression(string).map(|fact| fact.ty()),
            Some(&Type::String)
        );
        for (needle, ty) in [("user.name", "String"), ("id", "String")] {
            let start = text[string.start..].find(needle).expect("hole") + string.start;
            let fact = analysis
                .expression(Span::new(start, start + needle.len()))
                .expect("each hole value has a fact");
            assert_eq!(fact.ty().to_string(), ty);
        }
        assert!(
            analysis
                .expressions()
                .windows(2)
                .all(|window| window[0].span() < window[1].span())
        );
    }

    #[test]
    fn a_hole_that_is_not_a_string_is_k3015_with_one_note() {
        for (text, message, note, hole) in [
            (
                "let count = 3;\nlet s = \"n=\\(count)\";",
                "interpolation hole `count` has type `Int`",
                "let count_text = int_to_string(count);",
                "\\(count)",
            ),
            (
                "let issue = record { number: 7 };\nlet s = \"\\(issue.number)\";",
                "interpolation hole `issue.number` has type `Int`",
                "let number_text = int_to_string(issue.number);",
                "\\(issue.number)",
            ),
            (
                "let title = Some(\"a\");\nlet s = \"\\(title)\";",
                "has type `Option<String>`",
                "match the value first",
                "\\(title)",
            ),
            (
                "let s = secret(\"token\");\nlet t = \"\\(s)\";",
                "has type `Result<Secret, String>`",
                "match the value first",
                "\\(s)",
            ),
            (
                "fn f(token: Secret) -> String { \"\\(token)\" }",
                "has type `Secret`",
                "opaque",
                "\\(token)",
            ),
            (
                "let flag = true;\nlet s = \"\\(flag)\";",
                "has type `Bool`",
                "bind a `String` to a name",
                "\\(flag)",
            ),
            (
                "let s = \"\\(println)\";",
                "has type `fn(",
                "bind a `String` to a name",
                "\\(println)",
            ),
            (
                "fn f(n) { let a = n + 1; \"\\(n)\" }",
                "interpolation hole `n` has type `Int`",
                "let n_text = int_to_string(n);",
                "\\(n)",
            ),
        ] {
            let error = check(text).expect_err("a non-String hole should fail");
            assert_eq!(error.code(), "K3015", "{text}");
            assert!(
                error.message().contains(message),
                "{text}: {}",
                error.message()
            );
            assert_eq!(error.notes().len(), 1, "{text}");
            assert!(
                error.notes()[0].contains(note),
                "{text}: {:?}",
                error.notes()
            );
            assert_eq!(&text[error.span().start..error.span().end], hole, "{text}");
        }
    }

    #[test]
    fn a_hole_fixes_an_inferred_type_to_string() {
        let error = check("fn f(n) { \"\\(n)\" }\nlet x = f(1);")
            .expect_err("calling with an Int should fail");
        assert_eq!(error.code(), "K3001");
        let error = check("fn f(r) { let a = r.count + 1; \"\\(r.count)\" }")
            .expect_err("an Int field should fail");
        assert_eq!(error.code(), "K3015");
    }

    #[test]
    fn interpolated_strings_and_text_blocks_are_never_resource_literals() {
        for text in [
            "let key = \"model\";\nlet v = config_string(\"agent.\\(key)\");",
            "let v = config_string(\"\"\"\n    agent.model\n    \"\"\");",
            "let id = \"reviewer\";\nfn f(x: String) { ai_invoke(\"\\(id)\", x) }",
            "fn f() { log_info(\"\"\"\n    review.started\n    \"\"\", []) }",
            "fn f() { state_get(\"\"\"\n    agent-work\n    \"\"\", \"k\") }",
            "let o = \"https://example.com\";\n\
             fn f(r: HttpRequest) { http_request(\"\\(o)\", r, None) }",
        ] {
            let error = check(text).expect_err("a template resource should fail");
            assert_eq!(error.code(), "K3008", "{text}");
            let template = text.rfind("(\"").map(|index| index + 1).expect("call");
            assert_eq!(error.span().start, template, "{text}");
        }
        check("let v = config_string(\"agent.model\");").expect("a literal resource is valid");
    }

    #[test]
    fn types_the_standard_library_as_pure_functions() {
        let analysis = check(
            r#"
            let text = int_to_string(-7);
            let segment = url_path_segment(text);
            let query = url_query(["q": text]);
            let headers: List<HttpHeader> = [HttpHeader { name: "a", value: "b" }];
            let from_headers = url_query(headers);
            let fields = url_query([LogField { name: "k", value: "v" }]);
            let length = string_byte_len(text);
            let empty = string_is_empty(text);
            let found = string_contains(text, "7") && string_starts_with(text, "-")
                && string_ends_with(text, "7");
            let search = string_contains;
            "#,
        )
        .expect("standard library calls should check");
        let binding = |name: &str| {
            analysis
                .bindings()
                .iter()
                .find(|binding| binding.name() == name)
                .unwrap_or_else(|| panic!("binding `{name}`"))
                .ty()
                .to_string()
        };
        for (name, ty) in [
            ("text", "String"),
            ("segment", "Result<String, String>"),
            ("query", "String"),
            ("from_headers", "String"),
            ("fields", "String"),
            ("length", "Int"),
            ("empty", "Bool"),
            ("found", "Bool"),
            ("search", "fn(String, String) -> Bool effects {}"),
        ] {
            assert_eq!(binding(name), ty, "{name}");
        }
        assert!(analysis.effects().is_empty());
        assert!(analysis.requirements().is_empty());
        for builtin in Builtin::STANDARD_LIBRARY {
            assert_eq!(builtin.category(), crate::BuiltinCategory::StandardLibrary);
        }
    }

    #[test]
    fn standard_library_arguments_must_have_their_types() {
        for (text, code) in [
            ("int_to_string(\"1\");", "K3001"),
            ("url_path_segment(1);", "K3001"),
            ("url_query([\"a\"]);", "K3001"),
            ("url_query([record { name: \"a\" }]);", "K3001"),
            ("url_query([record { name: \"a\", value: 1 }]);", "K3001"),
            ("string_byte_len([]);", "K3001"),
            ("string_contains(\"a\", 1);", "K3001"),
            ("let n: String = string_byte_len(\"a\");", "K3001"),
            ("string_starts_with(\"a\");", "K3003"),
            (
                "let secret_text = match secret(\"token\") { Ok(value) => string_byte_len(value), Err(error) => 0 };",
                "K3001",
            ),
        ] {
            let error = check(text).expect_err("a mistyped call should fail");
            assert_eq!(error.code(), code, "{text}");
        }
    }

    #[test]
    fn standard_library_names_are_built_in() {
        for text in [
            "let int_to_string = 1;",
            "fn url_query(items) { items }",
            "fn f(string_contains) { string_contains }",
        ] {
            let error = check(text).expect_err("a built-in name cannot be declared");
            assert_eq!(error.code(), "K2002", "{text}");
        }
        // As for every built-in the parser does not reserve, a match binder
        // may reuse the name within its arm.
        check("let n = match Some(1) { Some(string_byte_len) => string_byte_len, None => 0 };")
            .expect("a match binder may shadow a standard-library name");
        let error = check("let n = string_byte_lenn(\"a\");").expect_err("a typo is undefined");
        assert_eq!(error.code(), "K2001");
        assert_eq!(error.notes(), ["did you mean `string_byte_len`?"]);
    }
}
