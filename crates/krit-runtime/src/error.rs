use std::{error::Error, fmt};

use krit_wasm::{BuildError, BuildErrorKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RuntimeErrorKind {
    InvalidArtifact,
    DigestMismatch,
    Authorization,
    ImportMismatch,
    FuelExhausted,
    DeadlineExceeded,
    ResourceLimit,
    HostCallLimit,
    OutputLimit,
    DivisionByZero,
    IntegerOverflow,
    GuestTrap,
    RuntimeSetup,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeError {
    code: &'static str,
    kind: RuntimeErrorKind,
    message: String,
}

impl RuntimeError {
    pub(crate) fn new(
        code: &'static str,
        kind: RuntimeErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn authorization(message: impl Into<String>) -> Self {
        Self::new("K5001", RuntimeErrorKind::Authorization, message)
    }

    pub(crate) fn import_mismatch(message: impl Into<String>) -> Self {
        Self::new("K5002", RuntimeErrorKind::ImportMismatch, message)
    }

    pub(crate) fn fuel(message: impl Into<String>) -> Self {
        Self::new("K5101", RuntimeErrorKind::FuelExhausted, message)
    }

    pub(crate) fn deadline(message: impl Into<String>) -> Self {
        Self::new("K5102", RuntimeErrorKind::DeadlineExceeded, message)
    }

    pub(crate) fn resource(message: impl Into<String>) -> Self {
        Self::new("K5103", RuntimeErrorKind::ResourceLimit, message)
    }

    pub(crate) fn host_calls(message: impl Into<String>) -> Self {
        Self::new("K5104", RuntimeErrorKind::HostCallLimit, message)
    }

    pub(crate) fn output(message: impl Into<String>) -> Self {
        Self::new("K5105", RuntimeErrorKind::OutputLimit, message)
    }

    pub(crate) fn guest(code: &'static str, message: impl Into<String>) -> Self {
        let kind = match code {
            "K4004" => RuntimeErrorKind::DivisionByZero,
            "K4005" => RuntimeErrorKind::IntegerOverflow,
            _ => RuntimeErrorKind::GuestTrap,
        };
        Self::new(code, kind, message)
    }

    pub(crate) fn setup(message: impl Into<String>) -> Self {
        Self::new("K7003", RuntimeErrorKind::RuntimeSetup, message)
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn kind(&self) -> RuntimeErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<BuildError> for RuntimeError {
    fn from(error: BuildError) -> Self {
        let kind = match error.kind() {
            BuildErrorKind::DigestMismatch | BuildErrorKind::Metadata => {
                RuntimeErrorKind::DigestMismatch
            }
            _ => RuntimeErrorKind::InvalidArtifact,
        };
        Self::new(error.code(), kind, error.message())
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RuntimeError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostLimitError {
    Calls,
    Output,
}

impl fmt::Display for HostLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Calls => formatter.write_str("host-call limit exceeded"),
            Self::Output => formatter.write_str("buffered output-byte limit exceeded"),
        }
    }
}

impl Error for HostLimitError {}
