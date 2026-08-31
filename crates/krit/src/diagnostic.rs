use std::fmt::Write;

use crate::{Source, Span};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    code: &'static str,
    message: String,
    span: Span,
}

impl Diagnostic {
    pub fn new(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            message: message.into(),
            span,
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn span(&self) -> Span {
        self.span
    }

    pub fn render_human(&self, source: &Source) -> String {
        let position = source.position(self.span.start);
        format!(
            "{}:{}:{}: error[{}]: {}",
            source.name(),
            position.line,
            position.column,
            self.code,
            self.message
        )
    }

    pub fn render_json(&self, source: &Source) -> String {
        let start = source.position(self.span.start);
        let end = source.position(self.span.end);
        let mut output = String::new();
        write!(
            output,
            "{{\"schema\":1,\"severity\":\"error\",\"code\":\"{}\",\
             \"message\":\"{}\",\"file\":\"{}\",\
             \"span\":{{\"start\":{{\"line\":{},\"column\":{},\"byte\":{}}},\
             \"end\":{{\"line\":{},\"column\":{},\"byte\":{}}}}},\
             \"labels\":[],\"notes\":[]}}",
            self.code,
            escape_json(&self.message),
            escape_json(source.name()),
            start.line,
            start.column,
            start.byte,
            end.line,
            end.column,
            end.byte
        )
        .expect("writing to a String cannot fail");
        output
    }
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            character if character < '\u{20}' => {
                write!(escaped, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_one_based_unicode_positions() {
        let source = Source::new("sample.krit", "🤖\nvalue");
        let diagnostic = Diagnostic::new("K2001", "missing \"name\"", Span::new(5, 10));

        assert_eq!(
            diagnostic.render_human(&source),
            "sample.krit:2:1: error[K2001]: missing \"name\""
        );
        assert!(
            diagnostic
                .render_json(&source)
                .contains("\"line\":2,\"column\":1")
        );
        assert!(
            diagnostic
                .render_json(&source)
                .contains("missing \\\"name\\\"")
        );
    }
}
