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
