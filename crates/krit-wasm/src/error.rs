use std::{error::Error, fmt};

use krit::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BuildErrorKind {
    ResidualType,
    UnsupportedSemantics,
    Capability,
    InvalidCore,
    InvalidArtifact,
    DigestMismatch,
    Metadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildError {
    code: &'static str,
    kind: BuildErrorKind,
    message: String,
    span: Option<Span>,
}

impl BuildError {
    pub(crate) fn residual(message: impl Into<String>, span: Option<Span>) -> Self {
        Self::new("K7001", BuildErrorKind::ResidualType, message, span)
    }

    pub(crate) fn unsupported(message: impl Into<String>, span: Option<Span>) -> Self {
        Self::new("K7002", BuildErrorKind::UnsupportedSemantics, message, span)
    }

    pub(crate) fn capability(message: impl Into<String>, span: Option<Span>) -> Self {
        Self::new("K5001", BuildErrorKind::Capability, message, span)
    }

    pub(crate) fn invalid_core(message: impl Into<String>) -> Self {
        Self::new("K7002", BuildErrorKind::InvalidCore, message, None)
    }

    pub(crate) fn artifact(message: impl Into<String>) -> Self {
        Self::new("K7003", BuildErrorKind::InvalidArtifact, message, None)
    }

    pub(crate) fn digest(message: impl Into<String>) -> Self {
        Self::new("K7004", BuildErrorKind::DigestMismatch, message, None)
    }

    pub(crate) fn metadata(message: impl Into<String>) -> Self {
        Self::new("K7004", BuildErrorKind::Metadata, message, None)
    }

    /// Attaches `span` when the error does not already name a source location.
    ///
    /// Emission helpers that only see a type or a value raise spanless errors;
    /// the enclosing Core operation or function supplies its own span here so
    /// that no source construct is reported without a location.
    #[must_use]
    pub(crate) fn or_span(mut self, span: Option<Span>) -> Self {
        if self.span.is_none() {
            self.span = span;
        }
        self
    }

    fn new(
        code: &'static str,
        kind: BuildErrorKind,
        message: impl Into<String>,
        span: Option<Span>,
    ) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
            span,
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn kind(&self) -> BuildErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn span(&self) -> Option<Span> {
        self.span
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BuildError {}

/// Orders target diagnostics by source position and removes repeated reports.
///
/// Located diagnostics sort by span start, span end, then code; diagnostics
/// without a location (internal artifact failures) follow them in the order
/// they were raised. One code is reported at most once per source position:
/// when nested constructs that start together (a built-in and its call, for
/// example) both fail, the narrowest span is kept, and among equal spans the
/// first finding in deterministic traversal order.
pub(crate) fn normalize(errors: &mut Vec<BuildError>) {
    errors.sort_by(|left, right| {
        (left.span.is_none(), left.span, left.code).cmp(&(
            right.span.is_none(),
            right.span,
            right.code,
        ))
    });
    let mut seen = std::collections::BTreeSet::new();
    errors.retain(|error| match error.span {
        Some(span) => seen.insert((span.start, error.code)),
        None => true,
    });
}

#[cfg(test)]
mod tests {
    use krit::Span;

    use super::{BuildError, normalize};

    #[test]
    fn spans_are_only_filled_when_missing() {
        let located = BuildError::unsupported("located", Some(Span::new(4, 9)));
        assert_eq!(
            located.or_span(Some(Span::new(0, 1))).span(),
            Some(Span::new(4, 9))
        );
        let spanless = BuildError::unsupported("spanless", None);
        assert_eq!(
            spanless.or_span(Some(Span::new(2, 3))).span(),
            Some(Span::new(2, 3))
        );
    }

    #[test]
    fn normalization_sorts_by_span_and_keeps_one_code_per_position() {
        let mut errors = vec![
            BuildError::unsupported("late", Some(Span::new(20, 25))),
            BuildError::artifact("internal"),
            BuildError::unsupported("outer", Some(Span::new(10, 18))),
            BuildError::residual("residual", Some(Span::new(10, 18))),
            BuildError::unsupported("inner", Some(Span::new(10, 12))),
            BuildError::unsupported("repeat", Some(Span::new(20, 25))),
        ];
        normalize(&mut errors);
        assert_eq!(
            errors
                .iter()
                .map(|error| (error.code(), error.message()))
                .collect::<Vec<_>>(),
            [
                ("K7002", "inner"),
                ("K7001", "residual"),
                ("K7002", "late"),
                ("K7003", "internal"),
            ]
        );
    }
}
