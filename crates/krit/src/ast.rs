use std::fmt;

use crate::Span;

#[derive(Clone, Debug)]
pub struct Program {
    pub statements: Vec<Statement>,
}

#[derive(Clone, Debug)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

impl Statement {
    /// The doc comment of a function or entrypoint declaration.
    pub fn doc(&self) -> Option<&DocComment> {
        match &self.kind {
            StatementKind::Function { doc, .. }
            | StatementKind::Webhook { doc, .. }
            | StatementKind::QueueConsumer { doc, .. }
            | StatementKind::ScheduleHandler { doc, .. } => doc.as_ref(),
            StatementKind::Let { .. }
            | StatementKind::LetElse { .. }
            | StatementKind::Expression(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum StatementKind {
    Let {
        name: String,
        annotation: Option<TypeAnnotation>,
        value: Expression,
    },
    /// `let PATTERN (: TYPE)? = value else ARM;`: binds the payload of
    /// `value` when it matches `pattern`, and otherwise makes the enclosing
    /// function return the value of `arm`, whose pattern is the other
    /// variant of the same family.
    ///
    /// `annotation` states the type of `value`. Analysis accepts a let-else
    /// only as a direct statement of a named function's body.
    LetElse {
        pattern: VariantPattern,
        annotation: Option<TypeAnnotation>,
        value: Expression,
        /// The `else` keyword.
        else_span: Span,
        arm: Box<VariantArm>,
    },
    Function {
        name: String,
        parameters: Vec<Parameter>,
        return_type: Option<TypeAnnotation>,
        body: Block,
        /// The `///` doc comment written directly before the declaration.
        doc: Option<DocComment>,
    },
    Webhook {
        name: String,
        parameters: Vec<Parameter>,
        return_type: Option<TypeAnnotation>,
        body: Block,
        /// The `///` doc comment written directly before the declaration.
        doc: Option<DocComment>,
    },
    QueueConsumer {
        queue: String,
        queue_span: Span,
        name: String,
        parameters: Vec<Parameter>,
        return_type: Option<TypeAnnotation>,
        body: Block,
        /// The `///` doc comment written directly before the declaration.
        doc: Option<DocComment>,
    },
    ScheduleHandler {
        schedule: String,
        schedule_span: Span,
        name: String,
        parameters: Vec<Parameter>,
        return_type: Option<TypeAnnotation>,
        body: Block,
        /// The `///` doc comment written directly before the declaration.
        doc: Option<DocComment>,
    },
    Expression(Expression),
}

/// A `///` doc comment: the run of `///` lines written directly before an
/// item, with only whitespace between them and the item.
///
/// Analysis ignores it; the formatter preserves it, and `krit explain` and
/// the language server show its text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocComment {
    /// The text of the lines in order, joined by `\n`. Each line is what
    /// follows its `///`, without one leading space and without trailing
    /// whitespace.
    pub text: String,
    /// From the first line's `///` to the end of the last line's text.
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub tail: Option<Box<Expression>>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExpressionKind {
    Literal(ValueLiteral),
    /// A string written with interpolation holes (`"Hi \(name)!"`), or a
    /// `"""` text block, with or without holes: the concatenation of its
    /// parts from left to right, of type `String`.
    ///
    /// It is never a [`ValueLiteral`], so it never counts as a capability
    /// resource literal. Its parts have no syntax nodes apart from each
    /// hole's value, whose spans lie inside the string, so every node keeps a
    /// span of its own.
    Interpolation(Vec<StringPart>),
    /// The unit value `()`.
    Unit,
    Variable(String),
    List(Vec<Expression>),
    /// An entry list `["name": value, ...]`: the list of
    /// `record { name: key, value: value }` records, one per entry, in
    /// source order.
    ///
    /// The records have no syntax node of their own; each is identified by
    /// its entry's span, which covers the key, the `:`, and the value. The
    /// empty list `[]` is always [`ExpressionKind::List`].
    EntryList(Vec<ListEntry>),
    Record(Vec<RecordField>),
    /// Named construction of a built-in contract record:
    /// `HttpResponse { status: 200, body }`.
    ///
    /// Only the written fields are syntax. The fields a construction omits
    /// take their spec-fixed defaults ([`ContractRecord::fields`]), which
    /// analysis, Core lowering, and the evaluator synthesize without any
    /// syntax node, so no analysis fact is keyed by a span the source does
    /// not contain.
    Construct {
        contract: ContractRecord,
        /// The span of the type name.
        contract_span: Span,
        fields: Vec<ConstructField>,
    },
    FieldAccess {
        value: Box<Expression>,
        field: String,
    },
    Block(Block),
    If {
        condition: Box<Expression>,
        consequent: Block,
        alternative: Box<Expression>,
    },
    Function {
        parameters: Vec<Parameter>,
        return_type: Option<TypeAnnotation>,
        body: Block,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
    /// Postfix `?`: `operand?` yields the payload of `Ok` or `Some`, and an
    /// `Err` or `None` operand makes the enclosing function return it.
    ///
    /// The node's span extends past the operand's to the `?` at `question`,
    /// so the two never share a span.
    Propagate {
        operand: Box<Expression>,
        question: Span,
    },
    /// `match subject { arms }`: evaluates `subject` once and the value of
    /// the one arm whose pattern matches it.
    ///
    /// Analysis requires the arms' patterns to name every outcome exactly
    /// once: none matches everything at the top level, no two overlap, and
    /// together they are exhaustive.
    Match {
        subject: Box<Expression>,
        arms: Vec<MatchArm>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
}

#[derive(Clone, Debug)]
pub struct RecordField {
    pub name: String,
    pub value: Expression,
    pub span: Span,
}

/// One part of an interpolated string or text block.
#[derive(Clone, Debug)]
pub enum StringPart {
    /// Literal text with its escapes decoded; in a text block, with each
    /// line's indentation removed and line breaks normalized to `\n`. The
    /// parser never produces an empty text part or two adjacent ones.
    Text { value: String, span: Span },
    /// A hole `\(name.field)`. `value` is the variable named in it, followed
    /// by its field accesses, at the spans of those names inside the hole;
    /// `span` covers the hole from its `\` to its `)`.
    Hole { value: Expression, span: Span },
}

/// One `key: value` entry of an entry list.
#[derive(Clone, Debug)]
pub struct ListEntry {
    pub key: Expression,
    pub value: Expression,
    /// From the start of the key to the end of the value.
    pub span: Span,
}

/// One written field of a named construction.
///
/// A punned field `body` is written as its name alone and means `body: body`:
/// its value is the variable expression at the name's own span.
#[derive(Clone, Debug)]
pub struct ConstructField {
    pub name: String,
    pub name_span: Span,
    pub value: Expression,
    pub span: Span,
}

impl ConstructField {
    /// Whether the field was written as its name alone (`body`).
    pub fn is_punned(&self) -> bool {
        self.value.span == self.name_span
    }
}

/// A built-in contract record type that named construction can build.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ContractRecord {
    HttpRequest,
    HttpResponse,
    HttpHeader,
    LogField,
}

/// The spec-fixed value of a contract field that a construction omits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldDefault {
    /// `""`
    EmptyString,
    /// `[]`
    EmptyList,
}

impl FieldDefault {
    /// The default as it would be written in source.
    pub const fn as_source(self) -> &'static str {
        match self {
            Self::EmptyString => "\"\"",
            Self::EmptyList => "[]",
        }
    }
}

/// One field of a contract record, in the contract's declared order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContractField {
    pub name: &'static str,
    /// `None` for a required field.
    pub default: Option<FieldDefault>,
}

const fn required(name: &'static str) -> ContractField {
    ContractField {
        name,
        default: None,
    }
}

const fn defaulted(name: &'static str, default: FieldDefault) -> ContractField {
    ContractField {
        name,
        default: Some(default),
    }
}

const HTTP_REQUEST_FIELDS: [ContractField; 5] = [
    required("method"),
    required("path"),
    defaulted("query", FieldDefault::EmptyString),
    defaulted("headers", FieldDefault::EmptyList),
    defaulted("body", FieldDefault::EmptyString),
];
const HTTP_RESPONSE_FIELDS: [ContractField; 3] = [
    required("status"),
    defaulted("headers", FieldDefault::EmptyList),
    defaulted("body", FieldDefault::EmptyString),
];
const NAME_VALUE_FIELDS: [ContractField; 2] = [required("name"), required("value")];

impl ContractRecord {
    /// The contract that `name` constructs, if it is one of the four
    /// constructible built-in record types.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "HttpRequest" => Some(Self::HttpRequest),
            "HttpResponse" => Some(Self::HttpResponse),
            "HttpHeader" => Some(Self::HttpHeader),
            "LogField" => Some(Self::LogField),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::HttpRequest => "HttpRequest",
            Self::HttpResponse => "HttpResponse",
            Self::HttpHeader => "HttpHeader",
            Self::LogField => "LogField",
        }
    }

    /// Every field of the contract in its declared order, which is also the
    /// order of the constructed record's fields.
    pub const fn fields(self) -> &'static [ContractField] {
        match self {
            Self::HttpRequest => &HTTP_REQUEST_FIELDS,
            Self::HttpResponse => &HTTP_RESPONSE_FIELDS,
            Self::HttpHeader | Self::LogField => &NAME_VALUE_FIELDS,
        }
    }

    /// The field named `name`, if the contract has one.
    pub fn field(self, name: &str) -> Option<&'static ContractField> {
        self.fields().iter().find(|field| field.name == name)
    }
}

/// One `match` arm: `pattern => value`.
#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub value: Expression,
    /// From the start of the pattern to the end of the value.
    pub span: Span,
}

/// A `match` pattern.
///
/// A pattern constrains one path into its value: a variant pattern's payload
/// is itself a pattern, while a list pattern's parts are only names or `_`.
#[derive(Clone, Debug)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PatternKind {
    /// A name, which matches any value and binds it.
    Binder(String),
    /// `_`, which matches any value without naming an outcome. In edition
    /// 2026 it binds the value as the discard name `_`, whose reads are
    /// warning `K2003`.
    Wildcard,
    /// `Some(p)`, `Ok(p)`, or `Err(p)`, whose payload matches `p`, or
    /// `None`, which has no payload.
    Variant {
        variant: VariantName,
        payload: Option<Box<Pattern>>,
    },
    /// `[]`, the empty list.
    EmptyList,
    /// `[head, ..tail]`, a non-empty list; `head` and `tail` are each a
    /// [`PatternKind::Binder`] or a [`PatternKind::Wildcard`].
    Cons {
        head: Box<Pattern>,
        tail: Box<Pattern>,
    },
}

impl Pattern {
    /// The number of variant and list patterns on the pattern's path: 0 for
    /// a name or `_`, 1 for `None`, `[]`, `[head, ..tail]`, or a variant
    /// whose payload is a name or `_`, and one more for each variant pattern
    /// it is the payload of (`Ok(Some(v))` is 2).
    pub fn constructor_depth(&self) -> usize {
        let mut depth = 0;
        let mut current = self;
        loop {
            match &current.kind {
                PatternKind::Binder(_) | PatternKind::Wildcard => return depth,
                PatternKind::EmptyList | PatternKind::Cons { .. } => return depth + 1,
                PatternKind::Variant { payload, .. } => {
                    depth += 1;
                    match payload {
                        Some(payload) => current = payload,
                        None => return depth,
                    }
                }
            }
        }
    }

    /// Whether the pattern matches every value: a name or `_`.
    pub const fn is_catch_all(&self) -> bool {
        matches!(self.kind, PatternKind::Binder(_) | PatternKind::Wildcard)
    }
}

/// Renders the pattern in its canonical spelling.
impl fmt::Display for Pattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Patterns are paths, so the closing parentheses are counted rather
        // than produced by recursion.
        let mut closing = 0;
        let mut current = self;
        loop {
            match &current.kind {
                PatternKind::Binder(name) => formatter.write_str(name)?,
                PatternKind::Wildcard => formatter.write_str("_")?,
                PatternKind::EmptyList => formatter.write_str("[]")?,
                PatternKind::Cons { head, tail } => write!(formatter, "[{head}, ..{tail}]")?,
                PatternKind::Variant { variant, payload } => {
                    formatter.write_str(variant.as_str())?;
                    if let Some(payload) = payload {
                        formatter.write_str("(")?;
                        closing += 1;
                        current = payload;
                        continue;
                    }
                }
            }
            break;
        }
        for _ in 0..closing {
            formatter.write_str(")")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VariantFamily {
    Option,
    Result,
}

/// The arm of a let-else: `PATTERN => value`, whose pattern is a flat
/// variant pattern as in [`VariantPattern`].
#[derive(Clone, Debug)]
pub struct VariantArm {
    pub variant: VariantName,
    /// The payload binder of `Some`, `Ok`, or `Err`, possibly the discard
    /// name `_`; `None` binds nothing.
    pub binding: Option<String>,
    pub value: Expression,
    pub span: Span,
}

/// A flat variant pattern: `Some(name)`, `Ok(name)`, `Err(name)`, or
/// `None`, as written after `let` in a let-else.
#[derive(Clone, Debug)]
pub struct VariantPattern {
    pub variant: VariantName,
    /// The binder of `Some`, `Ok`, or `Err`, possibly the discard name `_`;
    /// `None` binds nothing.
    pub binding: Option<String>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum VariantName {
    Some,
    None,
    Ok,
    Err,
}

impl VariantName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Some => "Some",
            Self::None => "None",
            Self::Ok => "Ok",
            Self::Err => "Err",
        }
    }

    /// The family the variant belongs to.
    pub const fn family(self) -> VariantFamily {
        match self {
            Self::Some | Self::None => VariantFamily::Option,
            Self::Ok | Self::Err => VariantFamily::Result,
        }
    }

    /// The other variant of the same family.
    pub const fn complement(self) -> Self {
        match self {
            Self::Some => Self::None,
            Self::None => Self::Some,
            Self::Ok => Self::Err,
            Self::Err => Self::Ok,
        }
    }

    /// The variant as a pattern, with an example binder when it binds one:
    /// `Ok(value)`, `Err(error)`, `Some(value)`, or `None`.
    pub const fn example_pattern(self) -> &'static str {
        match self {
            Self::Some => "Some(value)",
            Self::None => "None",
            Self::Ok => "Ok(value)",
            Self::Err => "Err(error)",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Parameter {
    pub name: String,
    pub annotation: Option<TypeAnnotation>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeAnnotation {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeKind {
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
    List(Box<TypeAnnotation>),
    Option(Box<TypeAnnotation>),
    Result(Box<TypeAnnotation>, Box<TypeAnnotation>),
    Record(Vec<RecordTypeField>),
}

impl fmt::Display for TypeAnnotation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)
    }
}

impl fmt::Display for TypeKind {
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
            Self::Option(element) => write!(formatter, "Option<{element}>"),
            Self::Result(value, error) => write!(formatter, "Result<{value}, {error}>"),
            Self::Record(fields) => {
                formatter.write_str("Record { ")?;
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{}: {}", field.name, field.annotation)?;
                }
                formatter.write_str(" }")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecordTypeField {
    pub name: String,
    pub annotation: TypeAnnotation,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ValueLiteral {
    Integer(i128),
    Boolean(bool),
    String(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}
