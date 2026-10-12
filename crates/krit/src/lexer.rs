use crate::{
    Diagnostic, Source, Span,
    token::{Template, TemplatePart, Token, TokenKind},
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
                '"' if self.text[self.cursor..].starts_with("\"\"") => self.text_block(start)?,
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
                '?' => self.push(TokenKind::Question, start),
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
        let kind = keyword(value).unwrap_or_else(|| TokenKind::Identifier(value.to_owned()));
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

    /// Reads a one-line string literal whose opening `"` starts at `start`.
    ///
    /// A string without interpolation holes is a [`TokenKind::String`]; one
    /// with at least one hole is a [`TokenKind::Template`].
    fn string(&mut self, start: usize) -> Result<(), Diagnostic> {
        let mut parts = PartsBuilder::default();
        loop {
            let character_start = self.cursor;
            let Some(character) = self.advance() else {
                return Err(Diagnostic::new(
                    "K0002",
                    "unterminated string",
                    Span::new(start, self.cursor),
                ));
            };

            match character {
                '"' => {
                    let kind = if parts.has_holes() {
                        TokenKind::Template(Template {
                            parts: parts.finish(),
                            text_block: None,
                        })
                    } else {
                        TokenKind::String(parts.into_text())
                    };
                    self.tokens
                        .push(Token::new(kind, Span::new(start, self.cursor)));
                    return Ok(());
                }
                '\n' | '\r' => {
                    return Err(Diagnostic::new(
                        "K0002",
                        "unterminated string",
                        Span::new(start, self.cursor),
                    ));
                }
                '\\' if self.peek() == Some('(') => {
                    let (path, span) = self.hole(character_start)?;
                    parts.push_hole(path, span);
                }
                '\\' => {
                    let escaped = self.escape(start)?;
                    parts.push_text(escaped, Span::new(character_start, self.cursor));
                }
                character => parts.push_text(character, Span::new(character_start, self.cursor)),
            }
        }
    }

    /// Reads a text block whose opening `"""` starts at `start`.
    ///
    /// The opening delimiter must end its line, and the closing `"""` is the
    /// first one that starts a line after spaces and tabs. That indentation
    /// is removed from every content line; a content line that does not
    /// start with it is an error unless it holds only spaces and tabs, which
    /// makes it empty. The content lines are joined by `\n`, whatever line
    /// breaks the source uses, and the line break before the closing
    /// delimiter is not part of the text. Escapes and holes work as in a
    /// one-line string, within one line.
    fn text_block(&mut self, start: usize) -> Result<(), Diagnostic> {
        let text = self.text;
        let opening = Span::new(start, start + 3);
        self.cursor = opening.end;
        let Some(break_length) = line_break_at(text, self.cursor) else {
            if self.cursor == text.len() {
                return Err(unterminated_text_block(opening));
            }
            return Err(Diagnostic::new(
                "K0004",
                "a text block's opening `\"\"\"` must end its line",
                opening,
            )
            .with_note(
                "end the line right after the opening `\"\"\"`, without trailing spaces, and \
                 start the text on the next line; a one-line string is written `\"...\"`",
            ));
        };

        let mut lines = Vec::new();
        let mut line_start = self.cursor + break_length;
        let (indentation, close_start) = loop {
            let line_end = line_end(text, line_start);
            let line = &text[line_start..line_end];
            let indentation = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
            if line[indentation.len()..].starts_with("\"\"\"") {
                break (indentation, line_start + indentation.len());
            }
            lines.push((line_start, line_end));
            match line_break_at(text, line_end) {
                Some(length) => line_start = line_end + length,
                None => return Err(unclosed_text_block(text, opening, &lines)),
            }
        };

        let mut parts = PartsBuilder::default();
        let mut raw_lines = Vec::with_capacity(lines.len());
        for (index, &(line_start, line_end)) in lines.iter().enumerate() {
            if let Some(&(_, previous_end)) = index.checked_sub(1).and_then(|i| lines.get(i)) {
                parts.push_text('\n', Span::new(previous_end, line_start));
            }
            let line = &text[line_start..line_end];
            let content_start = if line.starts_with(indentation) {
                line_start + indentation.len()
            } else if line.bytes().all(|byte| byte == b' ' || byte == b'\t') {
                line_end
            } else {
                let content = line.trim_start_matches([' ', '\t']);
                let first = content.chars().next().map_or(0, char::len_utf8);
                return Err(Diagnostic::new(
                    "K0004",
                    "a text block line must start with the indentation of the closing `\"\"\"`",
                    Span::new(line_start, line_end - content.len() + first),
                )
                .with_note(
                    "indent every line at least as far as the closing `\"\"\"`, with the same \
                     spaces and tabs; that indentation is removed from each line",
                ));
            };
            raw_lines.push(text[content_start..line_end].to_owned());
            self.cursor = content_start;
            while self.cursor < line_end {
                let character_start = self.cursor;
                let character = self.advance().expect("cursor is within the line");
                match character {
                    '"' if text[self.cursor..line_end].starts_with("\"\"") => {
                        return Err(misplaced_closing_delimiter(character_start));
                    }
                    '\\' if self.peek() == Some('(') => {
                        let (path, span) = self.hole(character_start)?;
                        parts.push_hole(path, span);
                    }
                    '\\' if self.cursor == line_end => {
                        return Err(Diagnostic::new(
                            "K0003",
                            "invalid string escape: a line of a text block cannot end with `\\`",
                            Span::new(character_start, self.cursor),
                        ));
                    }
                    '\\' => {
                        let escaped = self.escape(start)?;
                        parts.push_text(escaped, Span::new(character_start, self.cursor));
                    }
                    character => {
                        parts.push_text(character, Span::new(character_start, self.cursor));
                    }
                }
            }
        }

        self.cursor = close_start + 3;
        self.tokens.push(Token::new(
            TokenKind::Template(Template {
                parts: parts.finish(),
                text_block: Some(raw_lines),
            }),
            Span::new(start, self.cursor),
        ));
        Ok(())
    }

    /// Reads an interpolation hole whose `\` starts at `hole_start`; the
    /// cursor is at its `(`.
    ///
    /// The hole ends at the `)` that closes its `(` on the same line;
    /// parentheses inside it nest, and a quoted string inside it is skipped,
    /// so `\(f("x"))` is one (invalid) hole. Its content must be an
    /// identifier followed by any number of `.field` accesses, with no
    /// whitespace (`K3015` otherwise).
    fn hole(&mut self, hole_start: usize) -> Result<(Vec<(String, Span)>, Span), Diagnostic> {
        let text = self.text;
        self.advance();
        let content_start = self.cursor;
        let not_closed = || {
            Diagnostic::new(
                "K3015",
                "interpolation hole is not closed",
                Span::new(hole_start, content_start),
            )
            .with_note("end the hole with `)` right after the name, as in `\\(name)`")
        };
        let mut depth = 0usize;
        let content_end = loop {
            match self.peek() {
                None | Some('\n' | '\r') => return Err(not_closed()),
                Some(')') if depth == 0 => {
                    let end = self.cursor;
                    self.advance();
                    break end;
                }
                Some('"') => {
                    self.advance();
                    loop {
                        match self.advance() {
                            None | Some('\n' | '\r') => return Err(not_closed()),
                            Some('"') => break,
                            Some('\\') if !matches!(self.peek(), None | Some('\n' | '\r')) => {
                                self.advance();
                            }
                            Some(_) => {}
                        }
                    }
                }
                Some(character) => {
                    if character == '(' {
                        depth += 1;
                    } else if character == ')' {
                        depth -= 1;
                    }
                    self.advance();
                }
            }
        };
        let span = Span::new(hole_start, self.cursor);
        hole_path(&text[content_start..content_end], content_start)
            .map(|path| (path, span))
            .map_err(|problem| problem.diagnostic(span))
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

/// The token kind of keyword `word`, if it is one.
fn keyword(word: &str) -> Option<TokenKind> {
    Some(match word {
        "let" => TokenKind::Let,
        "fn" => TokenKind::Fn,
        "webhook" => TokenKind::Webhook,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "match" => TokenKind::Match,
        "record" => TokenKind::Record,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        _ => return None,
    })
}

/// The length of the line break at byte `index` of `text`: `\n`, `\r\n`,
/// or a lone `\r`.
fn line_break_at(text: &str, index: usize) -> Option<usize> {
    let rest = &text.as_bytes()[index..];
    match rest {
        [b'\r', b'\n', ..] => Some(2),
        [b'\n' | b'\r', ..] => Some(1),
        _ => None,
    }
}

/// The end of the line that starts at byte `start` of `text`, before its
/// line break.
fn line_end(text: &str, start: usize) -> usize {
    text[start..]
        .find(['\n', '\r'])
        .map_or(text.len(), |offset| start + offset)
}

/// A text block opened at `opening` that no line closes.
fn unterminated_text_block(opening: Span) -> Diagnostic {
    Diagnostic::new("K0002", "unterminated text block", opening)
        .with_note("close the text block with `\"\"\"` on a line of its own")
}

/// A `"""` at byte `start` that is not at the start of a line.
fn misplaced_closing_delimiter(start: usize) -> Diagnostic {
    Diagnostic::new(
        "K0004",
        "the closing `\"\"\"` of a text block must be on a line of its own",
        Span::new(start, start + 3),
    )
    .with_note(
        "move the `\"\"\"` to the next line, or write `\\\"\"\"` for three quote characters in \
         the text",
    )
}

/// The error for a text block opened at `opening` that no line closes, whose
/// content lines are `lines`: the first `"""` inside a line, which was most
/// likely meant to close it, or else the opening delimiter.
fn unclosed_text_block(text: &str, opening: Span, lines: &[(usize, usize)]) -> Diagnostic {
    for &(start, end) in lines {
        let mut characters = text[start..end].char_indices();
        while let Some((offset, character)) = characters.next() {
            match character {
                '\\' => {
                    characters.next();
                }
                '"' if text[start + offset..end].starts_with("\"\"\"") => {
                    return misplaced_closing_delimiter(start + offset);
                }
                _ => {}
            }
        }
    }
    unterminated_text_block(opening)
}

/// Why the content of an interpolation hole is not a name path.
enum HoleProblem {
    Empty,
    Whitespace,
    NotAPath,
}

impl HoleProblem {
    fn diagnostic(self, span: Span) -> Diagnostic {
        let (message, note) = match self {
            Self::Empty => (
                "interpolation hole is empty",
                "write the name of a `String` value between `\\(` and `)`, as in `\\(name)`",
            ),
            Self::Whitespace => (
                "an interpolation hole cannot contain whitespace",
                "write the hole without spaces, as in `\\(request.path)`",
            ),
            Self::NotAPath => (
                "an interpolation hole holds only a name and its field accesses, such as \
                 `\\(name)` or `\\(request.path)`",
                "bind the value to a name first (`let text = ...;`), then write `\\(text)`",
            ),
        };
        Diagnostic::new("K3015", message, span).with_note(note)
    }
}

/// The identifier and field names of the hole content `content`, which starts
/// at byte `start`, each with its span.
fn hole_path(content: &str, start: usize) -> Result<Vec<(String, Span)>, HoleProblem> {
    if content.trim().is_empty() {
        return Err(HoleProblem::Empty);
    }
    let mut path = Vec::new();
    let mut offset = 0;
    for segment in content.split('.') {
        let is_identifier = segment
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && keyword(segment).is_none();
        if !is_identifier {
            // Whitespace around the names and dots of an otherwise valid
            // path, as in `\( name )` or `\(a. b)`.
            let trimmed = content
                .split('.')
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(".");
            let spaced_path = trimmed.len() < content.len() && hole_path(&trimmed, 0).is_ok();
            return Err(if spaced_path {
                HoleProblem::Whitespace
            } else {
                HoleProblem::NotAPath
            });
        }
        let segment_start = start + offset;
        path.push((
            segment.to_owned(),
            Span::new(segment_start, segment_start + segment.len()),
        ));
        offset += segment.len() + 1;
    }
    Ok(path)
}

/// Collects the parts of a string literal or text block as it is read.
#[derive(Default)]
struct PartsBuilder {
    parts: Vec<TemplatePart>,
    /// Text read since the last hole.
    text: String,
    /// The source span of `text`, once it holds anything.
    text_span: Option<Span>,
}

impl PartsBuilder {
    /// Appends `character`, read from the source at `span`.
    fn push_text(&mut self, character: char, span: Span) {
        self.text.push(character);
        self.text_span = Some(self.text_span.map_or(span, |text| text.join(span)));
    }

    fn push_hole(&mut self, path: Vec<(String, Span)>, span: Span) {
        self.flush_text();
        self.parts.push(TemplatePart::Hole { path, span });
    }

    fn has_holes(&self) -> bool {
        !self.parts.is_empty()
    }

    fn flush_text(&mut self) {
        if let Some(span) = self.text_span.take() {
            self.parts.push(TemplatePart::Text {
                value: std::mem::take(&mut self.text),
                span,
            });
        }
    }

    /// The text of a string without holes.
    fn into_text(self) -> String {
        debug_assert!(self.parts.is_empty());
        self.text
    }

    fn finish(mut self) -> Vec<TemplatePart> {
        self.flush_text();
        self.parts
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
    fn lexes_the_question_mark_as_a_token() {
        let source = Source::new("test.krit", "ready ? 1 : 2");
        let tokens = lex(&source).expect("`?` should lex");

        assert_eq!(tokens[1].kind, TokenKind::Question);
        assert_eq!(tokens[1].span, Span::new(6, 7));
        assert_eq!(tokens[3].kind, TokenKind::Colon);
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

    fn lex_template(text: &str) -> (Template, Span) {
        let source = Source::new("test.krit", text);
        let tokens = lex(&source).expect("source should lex");
        let token = tokens
            .into_iter()
            .find(|token| matches!(token.kind, TokenKind::Template(_)))
            .expect("source should contain a template");
        let TokenKind::Template(template) = token.kind else {
            unreachable!("found a template token")
        };
        (template, token.span)
    }

    /// The parts of `template` as `text` and `\(path)` strings.
    fn rendered_parts(template: &Template) -> Vec<String> {
        template
            .parts
            .iter()
            .map(|part| match part {
                TemplatePart::Text { value, .. } => value.clone(),
                TemplatePart::Hole { path, .. } => format!(
                    "\\({})",
                    path.iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(".")
                ),
            })
            .collect()
    }

    fn lex_error(text: &str) -> Diagnostic {
        lex(&Source::new("test.krit", text)).expect_err("source should fail to lex")
    }

    #[test]
    fn lexes_interpolation_holes_with_spans() {
        let text = r#"let s = "a\t\(user.name)\(id)\\(x)\"\(b)";"#;
        let (template, span) = lex_template(text);
        assert_eq!(span, Span::new(8, text.len() - 1));
        assert_eq!(template.text_block, None);
        assert_eq!(
            rendered_parts(&template),
            ["a\t", "\\(user.name)", "\\(id)", "\\(x)\"", "\\(b)"]
        );
        let TemplatePart::Hole { path, span } = &template.parts[1] else {
            panic!("the second part should be a hole");
        };
        assert_eq!(&text[span.start..span.end], "\\(user.name)");
        assert_eq!(&text[path[0].1.start..path[0].1.end], "user");
        assert_eq!(&text[path[1].1.start..path[1].1.end], "name");
        let TemplatePart::Text { span, .. } = &template.parts[0] else {
            panic!("the first part should be text");
        };
        assert_eq!(&text[span.start..span.end], "a\\t");
    }

    #[test]
    fn a_string_without_holes_stays_a_plain_string_token() {
        let tokens = lex(&Source::new("test.krit", r#""a \\(b) \u{41}""#)).expect("should lex");
        assert_eq!(tokens[0].kind, TokenKind::String("a \\(b) A".to_owned()));
    }

    #[test]
    fn rejects_holes_that_are_not_name_paths() {
        for (text, message, hole) in [
            (r#""\(f(x))""#, "holds only a name", r"\(f(x))"),
            (r#""\(f("y"))""#, "holds only a name", r#"\(f("y"))"#),
            (r#""\(a + b)""#, "holds only a name", r"\(a + b)"),
            (r#""\(1)""#, "holds only a name", r"\(1)"),
            (r#""\(true)""#, "holds only a name", r"\(true)"),
            (r#""\(a.if)""#, "holds only a name", r"\(a.if)"),
            (r#""\(a..b)""#, "holds only a name", r"\(a..b)"),
            (r#""\(a.)""#, "holds only a name", r"\(a.)"),
            (r#""\(.a)""#, "holds only a name", r"\(.a)"),
            (r#""\(1a)""#, "holds only a name", r"\(1a)"),
            (r#""\(é)""#, "holds only a name", r"\(é)"),
            (r#""\( name )""#, "whitespace", r"\( name )"),
            (r#""\(a. b)""#, "whitespace", r"\(a. b)"),
            (r#""\()""#, "is empty", r"\()"),
            (r#""\( )""#, "is empty", r"\( )"),
            (r#""\(name""#, "not closed", r"\("),
            ("\"\\(name\n\"", "not closed", r"\("),
            (r#""\(name"#, "not closed", r"\("),
            (r#""\(f("x)""#, "not closed", r"\("),
        ] {
            let error = lex_error(text);
            assert_eq!(error.code(), "K3015", "{text}");
            assert!(
                error.message().contains(message),
                "{text}: {}",
                error.message()
            );
            assert_eq!(&text[error.span().start..error.span().end], hole, "{text}");
            assert_eq!(error.notes().len(), 1, "{text}");
        }
    }

    #[test]
    fn lexes_text_blocks_with_swift_indentation_rules() {
        let text = "let s = \"\"\"\n    first \\(name)\n      deeper \"quoted\" \\t\n\n  \n        \n    last\n    \"\"\";";
        let (template, span) = lex_template(text);
        assert_eq!(&text[span.start..span.end], &text[8..text.len() - 1]);
        assert_eq!(
            rendered_parts(&template),
            [
                "first ",
                "\\(name)",
                "\n  deeper \"quoted\" \t\n\n\n    \nlast"
            ]
        );
        assert_eq!(
            template.text_block,
            Some(vec![
                "first \\(name)".to_owned(),
                "  deeper \"quoted\" \\t".to_owned(),
                String::new(),
                String::new(),
                "    ".to_owned(),
                "last".to_owned(),
            ])
        );
    }

    #[test]
    fn text_blocks_normalize_every_line_break_to_a_line_feed() {
        for text in [
            "\"\"\"\n  a\n\n  b\n  \"\"\"",
            "\"\"\"\r\n  a\r\n\r\n  b\r\n  \"\"\"",
            "\"\"\"\r  a\r\r  b\r  \"\"\"",
        ] {
            let (template, _) = lex_template(text);
            assert_eq!(rendered_parts(&template), ["a\n\nb"], "{text:?}");
            assert_eq!(
                template.text_block,
                Some(vec!["a".to_owned(), String::new(), "b".to_owned()]),
                "{text:?}"
            );
        }
    }

    #[test]
    fn empty_text_blocks_and_unindented_closers() {
        let (template, _) = lex_template("\"\"\"\n\"\"\"");
        assert!(template.parts.is_empty());
        assert_eq!(template.text_block, Some(Vec::new()));

        let (template, _) = lex_template("\"\"\"\n\n    \"\"\"");
        assert!(template.parts.is_empty());
        assert_eq!(template.text_block, Some(vec![String::new()]));

        let (template, _) = lex_template("\"\"\"\n\tx\n  y\n\"\"\"");
        assert_eq!(rendered_parts(&template), ["\tx\n  y"]);
    }

    #[test]
    fn text_blocks_end_at_the_closing_delimiter_and_lex_what_follows() {
        let source = Source::new("test.krit", "f(\"\"\"\n    a\n    \"\"\"); \"b\"");
        let tokens = lex(&source).expect("source should lex");
        assert!(matches!(tokens[2].kind, TokenKind::Template(_)));
        assert_eq!(tokens[3].kind, TokenKind::RightParen);
        assert_eq!(tokens[4].kind, TokenKind::Semicolon);
        assert_eq!(tokens[5].kind, TokenKind::String("b".to_owned()));
    }

    #[test]
    fn rejects_malformed_text_blocks() {
        for (text, code, message, at) in [
            ("\"\"\"a\n\"\"\"", "K0004", "must end its line", "\"\"\""),
            ("\"\"\" \n\"\"\"", "K0004", "must end its line", "\"\"\""),
            (
                "\"\"\"\n    a\n  b\n    \"\"\"",
                "K0004",
                "indentation",
                "  b",
            ),
            ("\"\"\"\n\ta\n    \"\"\"", "K0004", "indentation", "\ta"),
            (
                "\"\"\"\n  a \"\"\" b\n  \"\"\"",
                "K0004",
                "line of its own",
                "\"\"\"",
            ),
            ("\"\"\"\n  a\"\"\"\n", "K0004", "line of its own", "\"\"\""),
            (
                "\"\"\"\n  a\n",
                "K0002",
                "unterminated text block",
                "\"\"\"",
            ),
            ("\"\"\"", "K0002", "unterminated text block", "\"\"\""),
            ("\"\"\"\n  a \\\n  \"\"\"", "K0003", "cannot end with", "\\"),
            (
                "\"\"\"\n  \\q\n  \"\"\"",
                "K0003",
                "invalid string escape",
                "\\q",
            ),
            (
                "\"\"\"\n  \\(a b)\n  \"\"\"",
                "K3015",
                "holds only a name",
                "\\(a b)",
            ),
            (
                "\"\"\"\n  \\(a\n  )\n  \"\"\"",
                "K3015",
                "not closed",
                "\\(",
            ),
        ] {
            let error = lex_error(text);
            assert_eq!(error.code(), code, "{text:?}");
            assert!(
                error.message().contains(message),
                "{text:?}: {}",
                error.message()
            );
            assert_eq!(&text[error.span().start..error.span().end], at, "{text:?}");
        }
    }

    #[test]
    fn escaped_quotes_never_close_a_text_block() {
        let (template, _) = lex_template("\"\"\"\n  say \\\"\"\" and \\\"\\\"\\\"\n  \"\"\"");
        assert_eq!(rendered_parts(&template), ["say \"\"\" and \"\"\""]);
    }
}
