use crate::{
    Diagnostic, Source, Span,
    token::{Token, TokenKind},
};

pub fn lex(source: &Source) -> Result<Vec<Token>, Diagnostic> {
    Ok(lex_with_comments(source)?.tokens)
}

pub(crate) fn lex_with_comments(source: &Source) -> Result<LexedSource, Diagnostic> {
    Lexer::new(source.text()).scan()
}

#[derive(Clone, Debug)]
pub(crate) struct Comment {
    pub(crate) text: String,
    pub(crate) span: Span,
    pub(crate) inline: bool,
}

pub(crate) struct LexedSource {
    pub(crate) tokens: Vec<Token>,
    pub(crate) comments: Vec<Comment>,
}

struct Lexer<'a> {
    text: &'a str,
    cursor: usize,
    tokens: Vec<Token>,
    comments: Vec<Comment>,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            cursor: 0,
            tokens: Vec::new(),
            comments: Vec::new(),
        }
    }

    fn scan(mut self) -> Result<LexedSource, Diagnostic> {
        while self.cursor < self.text.len() {
            self.skip_trivia();
            if self.cursor >= self.text.len() {
                break;
            }

            let start = self.cursor;
            let character = self.advance().expect("cursor is within source");
            match character {
                'a'..='z' | 'A'..='Z' | '_' => self.identifier(start),
                '0'..='9' => self.integer(start)?,
                '"' => self.string(start)?,
                '(' => self.push(TokenKind::LeftParen, start),
                ')' => self.push(TokenKind::RightParen, start),
                '{' => self.push(TokenKind::LeftBrace, start),
                '}' => self.push(TokenKind::RightBrace, start),
                '[' => self.push(TokenKind::LeftBracket, start),
                ']' => self.push(TokenKind::RightBracket, start),
                ',' => self.push(TokenKind::Comma, start),
                ':' => self.push(TokenKind::Colon, start),
                ';' => self.push(TokenKind::Semicolon, start),
                '+' => self.push(TokenKind::Plus, start),
                '-' if self.consume('>') => self.push(TokenKind::ThinArrow, start),
                '-' => self.push(TokenKind::Minus, start),
                '*' => self.push(TokenKind::Star, start),
                '%' => self.push(TokenKind::Percent, start),
                '/' => self.push(TokenKind::Slash, start),
                '=' if self.consume('=') => self.push(TokenKind::EqualEqual, start),
                '=' if self.consume('>') => self.push(TokenKind::FatArrow, start),
                '=' => self.push(TokenKind::Equal, start),
                '!' if self.consume('=') => self.push(TokenKind::BangEqual, start),
                '!' => self.push(TokenKind::Bang, start),
                '<' if self.consume('=') => self.push(TokenKind::LessEqual, start),
                '<' => self.push(TokenKind::Less, start),
                '>' if self.consume('=') => self.push(TokenKind::GreaterEqual, start),
                '>' => self.push(TokenKind::Greater, start),
                '&' if self.consume('&') => self.push(TokenKind::AndAnd, start),
                '|' if self.consume('|') => self.push(TokenKind::OrOr, start),
                '.' if self.consume('.') => self.push(TokenKind::DotDot, start),
                '.' => self.push(TokenKind::Dot, start),
                other => {
                    return Err(Diagnostic::new(
                        "K0001",
                        format!("invalid source character `{other}`"),
                        Span::new(start, self.cursor),
                    ));
                }
            }
        }

        self.tokens.push(Token::new(
            TokenKind::Eof,
            Span::new(self.text.len(), self.text.len()),
        ));
        Ok(LexedSource {
            tokens: self.tokens,
            comments: self.comments,
        })
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.advance();
            }

            if self.peek() == Some('/') && self.peek_next() == Some('/') {
                let start = self.cursor;
                let line_start = self.text[..start]
                    .rfind(['\n', '\r'])
                    .map_or(0, |index| index + 1);
                let inline = self.text[line_start..start]
                    .chars()
                    .any(|character| !character.is_whitespace());
                while self
                    .peek()
                    .is_some_and(|character| character != '\n' && character != '\r')
                {
                    self.advance();
                }
                self.comments.push(Comment {
                    text: self.text[start..self.cursor].to_owned(),
                    span: Span::new(start, self.cursor),
                    inline,
                });
            } else {
                break;
            }
        }
    }

    fn identifier(&mut self, start: usize) {
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            self.advance();
        }

        let value = &self.text[start..self.cursor];
        let kind = match value {
            "let" => TokenKind::Let,
            "fn" => TokenKind::Fn,
            "webhook" => TokenKind::Webhook,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "match" => TokenKind::Match,
            "record" => TokenKind::Record,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => TokenKind::Identifier(value.to_owned()),
        };
        self.tokens
            .push(Token::new(kind, Span::new(start, self.cursor)));
    }

    fn integer(&mut self, start: usize) -> Result<(), Diagnostic> {
        let mut previous_separator = false;
        while let Some(character) = self.peek() {
            match character {
                '0'..='9' => {
                    previous_separator = false;
                    self.advance();
                }
                '_' if !previous_separator => {
                    previous_separator = true;
                    self.advance();
                }
                '_' => {
                    return Err(Diagnostic::new(
                        "K0001",
                        "integer separators cannot be repeated",
                        Span::new(start, self.cursor + 1),
                    ));
                }
                character if character.is_ascii_alphabetic() => {
                    self.advance();
                    return Err(Diagnostic::new(
                        "K0001",
                        "an integer and identifier must be separated",
                        Span::new(start, self.cursor),
                    ));
                }
                _ => break,
            }
        }

        if previous_separator {
            return Err(Diagnostic::new(
                "K0001",
                "an integer separator cannot be trailing",
                Span::new(start, self.cursor),
            ));
        }

        let normalized = self.text[start..self.cursor].replace('_', "");
        self.tokens.push(Token::new(
            TokenKind::Integer(normalized),
            Span::new(start, self.cursor),
        ));
        Ok(())
    }

    fn string(&mut self, start: usize) -> Result<(), Diagnostic> {
        let mut value = String::new();
        loop {
            let Some(character) = self.advance() else {
                return Err(Diagnostic::new(
                    "K0002",
                    "unterminated string",
                    Span::new(start, self.cursor),
                ));
            };

            match character {
                '"' => {
                    self.tokens.push(Token::new(
                        TokenKind::String(value),
                        Span::new(start, self.cursor),
                    ));
                    return Ok(());
                }
                '\n' | '\r' => {
                    return Err(Diagnostic::new(
                        "K0002",
                        "unterminated string",
                        Span::new(start, self.cursor),
                    ));
                }
                '\\' => value.push(self.escape(start)?),
                character => value.push(character),
            }
        }
    }

    fn escape(&mut self, string_start: usize) -> Result<char, Diagnostic> {
        let escape_start = self.cursor.saturating_sub(1);
        let Some(character) = self.advance() else {
            return Err(Diagnostic::new(
                "K0002",
                "unterminated string",
                Span::new(string_start, self.cursor),
            ));
        };

        let escaped = match character {
            '"' => '"',
            '\\' => '\\',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            'u' => return self.unicode_escape(escape_start),
            _ => {
                return Err(Diagnostic::new(
                    "K0003",
                    format!("invalid string escape `\\{character}`"),
                    Span::new(escape_start, self.cursor),
                ));
            }
        };
        Ok(escaped)
    }

    fn unicode_escape(&mut self, escape_start: usize) -> Result<char, Diagnostic> {
        if !self.consume('{') {
            return Err(Diagnostic::new(
                "K0003",
                "a Unicode escape must use `\\u{...}`",
                Span::new(escape_start, self.cursor),
            ));
        }

        let digits_start = self.cursor;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_hexdigit())
            && self.cursor - digits_start < 6
        {
            self.advance();
        }

        if digits_start == self.cursor || !self.consume('}') {
            return Err(Diagnostic::new(
                "K0003",
                "invalid Unicode escape",
                Span::new(escape_start, self.cursor),
            ));
        }

        let digits = &self.text[digits_start..self.cursor - 1];
        let scalar = u32::from_str_radix(digits, 16).map_err(|_| {
            Diagnostic::new(
                "K0003",
                "invalid Unicode escape",
                Span::new(escape_start, self.cursor),
            )
        })?;
        char::from_u32(scalar).ok_or_else(|| {
            Diagnostic::new(
                "K0003",
                "Unicode escape is not a valid scalar value",
                Span::new(escape_start, self.cursor),
            )
        })
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens
            .push(Token::new(kind, Span::new(start, self.cursor)));
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.cursor += character.len_utf8();
        Some(character)
    }

    fn peek(&self) -> Option<char> {
        self.text[self.cursor..].chars().next()
    }

    fn peek_next(&self) -> Option<char> {
        let mut characters = self.text[self.cursor..].chars();
        characters.next()?;
        characters.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_keywords_operators_comments_and_strings() {
        let source = Source::new(
            "test.krit",
            "let value = \"open \\u{1f916}\"; // note\nvalue != 1_000 && true",
        );
        let tokens = lex(&source).expect("source should lex");

        assert!(matches!(tokens[0].kind, TokenKind::Let));
        assert!(matches!(tokens[3].kind, TokenKind::String(ref value) if value == "open 🤖"));
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::BangEqual)
        );
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::Integer("1000".to_owned()))
        );
        assert!(tokens.iter().any(|token| token.kind == TokenKind::AndAnd));
    }

    #[test]
    fn rejects_invalid_integer_separator() {
        let source = Source::new("test.krit", "1__0;");
        let error = lex(&source).expect_err("source should fail");
        assert_eq!(error.code(), "K0001");
    }

    #[test]
    fn lexes_data_and_annotation_punctuation() {
        let source = Source::new(
            "test.krit",
            "let item: Record { value: Int } = record { value: 1 }; item.value; fn(x) -> Int { x };",
        );
        let tokens = lex(&source).expect("source should lex");

        assert!(tokens.iter().any(|token| token.kind == TokenKind::Record));
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Colon));
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Dot));
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::ThinArrow)
        );
    }

    #[test]
    fn reserves_the_webhook_keyword() {
        let source = Source::new(
            "test.krit",
            "webhook fn handle(request: HttpRequest) -> HttpResponse {}",
        );
        let tokens = lex(&source).expect("webhook declaration should lex");

        assert!(matches!(tokens[0].kind, TokenKind::Webhook));
        assert!(matches!(tokens[1].kind, TokenKind::Fn));
    }
}
