use crate::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Identifier(String),
    Integer(String),
    String(String),
    Let,
    Fn,
    If,
    Else,
    Match,
    True,
    False,
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Semicolon,
    Equal,
    EqualEqual,
    Bang,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    DotDot,
    FatArrow,
    Eof,
}

impl TokenKind {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Identifier(_) => "identifier",
            Self::Integer(_) => "integer",
            Self::String(_) => "string",
            Self::Let => "`let`",
            Self::Fn => "`fn`",
            Self::If => "`if`",
            Self::Else => "`else`",
            Self::Match => "`match`",
            Self::True => "`true`",
            Self::False => "`false`",
            Self::LeftParen => "`(`",
            Self::RightParen => "`)`",
            Self::LeftBrace => "`{`",
            Self::RightBrace => "`}`",
            Self::LeftBracket => "`[`",
            Self::RightBracket => "`]`",
            Self::Comma => "`,`",
            Self::Semicolon => "`;`",
            Self::Equal => "`=`",
            Self::EqualEqual => "`==`",
            Self::Bang => "`!`",
            Self::BangEqual => "`!=`",
            Self::Less => "`<`",
            Self::LessEqual => "`<=`",
            Self::Greater => "`>`",
            Self::GreaterEqual => "`>=`",
            Self::Plus => "`+`",
            Self::Minus => "`-`",
            Self::Star => "`*`",
            Self::Slash => "`/`",
            Self::Percent => "`%`",
            Self::AndAnd => "`&&`",
            Self::OrOr => "`||`",
            Self::DotDot => "`..`",
            Self::FatArrow => "`=>`",
            Self::Eof => "end of file",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub const fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}
