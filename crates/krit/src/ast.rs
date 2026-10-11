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
            StatementKind::Let { .. } | StatementKind::Expression(_) => None,
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
    Match {
        subject: Box<Expression>,
        kind: MatchKind,
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

#[derive(Clone, Debug)]
pub enum MatchKind {
    List {
        empty_case: Box<Expression>,
        head_name: String,
        head_span: Span,
        tail_name: String,
        tail_span: Span,
        cons_case: Box<Expression>,
    },
    Variants {
        family: VariantFamily,
        arms: Vec<VariantArm>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VariantFamily {
    Option,
    Result,
}

#[derive(Clone, Debug)]
pub struct VariantArm {
    pub variant: VariantName,
    pub binding: Option<String>,
    pub value: Expression,
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
