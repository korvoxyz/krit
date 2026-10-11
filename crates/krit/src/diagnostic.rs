use std::fmt::{self, Write};

use crate::{Source, Span};

/// Whether a diagnostic rejects the program or only reports on it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// The program is rejected and the reporting command fails.
    Error,
    /// The program is accepted. A warning never changes a command's outcome
    /// or exit status.
    Warning,
}

impl Severity {
    /// The stable lowercase spelling used by the human and JSON Lines formats.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    severity: Severity,
    code: &'static str,
    message: String,
    span: Span,
}

impl Diagnostic {
    /// Creates an error diagnostic.
    pub fn new(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            span,
        }
    }

    /// Creates a warning diagnostic.
    pub fn warning(code: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            span,
        }
    }

    pub const fn severity(&self) -> Severity {
        self.severity
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
            "{}:{}:{}: {}[{}]: {}",
            source.name(),
            position.line,
            position.column,
            self.severity,
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
            "{{\"schema\":1,\"severity\":\"{}\",\"code\":\"{}\",\
             \"message\":\"{}\",\"file\":\"{}\",\
             \"span\":{{\"start\":{{\"line\":{},\"column\":{},\"byte\":{}}},\
             \"end\":{{\"line\":{},\"column\":{},\"byte\":{}}}}},\
             \"labels\":[],\"notes\":[]}}",
            self.severity,
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
        assert_eq!(diagnostic.severity(), Severity::Error);
        assert!(
            diagnostic
                .render_json(&source)
                .starts_with("{\"schema\":1,\"severity\":\"error\",\"code\":\"K2001\",")
        );
    }

    #[test]
    fn renders_warnings_with_their_severity_and_the_error_location_style() {
        let source = Source::new("sample.krit", "let _ = 1;\n_");
        let warning = Diagnostic::warning("K2003", "`_` is a discard", Span::new(11, 12));

        assert_eq!(warning.severity(), Severity::Warning);
        assert_eq!(
            warning.render_human(&source),
            "sample.krit:2:1: warning[K2003]: `_` is a discard"
        );
        assert_eq!(
            warning.render_json(&source),
            "{\"schema\":1,\"severity\":\"warning\",\"code\":\"K2003\",\
             \"message\":\"`_` is a discard\",\"file\":\"sample.krit\",\
             \"span\":{\"start\":{\"line\":2,\"column\":1,\"byte\":11},\
             \"end\":{\"line\":2,\"column\":2,\"byte\":12}},\
             \"labels\":[],\"notes\":[]}"
        );
    }
}
