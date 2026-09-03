use std::{error::Error, fmt};

use krit_wasm::{BuildError, BuildErrorKind};

use crate::LogEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RuntimeErrorKind {
    InvalidArtifact,
    DigestMismatch,
    Authorization,
    ImportMismatch,
    FuelExhausted,
    DeadlineExceeded,
    Cancelled,
    ResourceLimit,
    HostCallLimit,
    OutputLimit,
    DivisionByZero,
    IntegerOverflow,
    GuestTrap,
    RuntimeSetup,
    DurableState,
    StateConflict,
    Replay,
    DurableIdempotency,
    Delivery,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeError {
    code: &'static str,
    kind: RuntimeErrorKind,
    message: String,
    events: Vec<LogEvent>,
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
            events: Vec::new(),
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

    pub(crate) fn cancelled(message: impl Into<String>) -> Self {
        Self::new("K5106", RuntimeErrorKind::Cancelled, message)
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

    pub(crate) fn durable_state(message: impl Into<String>) -> Self {
        Self::new("K5201", RuntimeErrorKind::DurableState, message)
    }

    pub(crate) fn state_conflict(message: impl Into<String>) -> Self {
        Self::new("K5202", RuntimeErrorKind::StateConflict, message)
    }

    pub(crate) fn replay(message: impl Into<String>) -> Self {
        Self::new("K5203", RuntimeErrorKind::Replay, message)
    }

    pub(crate) fn durable_idempotency(message: impl Into<String>) -> Self {
        Self::new("K5204", RuntimeErrorKind::DurableIdempotency, message)
    }

    pub(crate) fn delivery(message: impl Into<String>) -> Self {
        Self::new("K5205", RuntimeErrorKind::Delivery, message)
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

    pub fn events(&self) -> &[LogEvent] {
        &self.events
    }

    pub(crate) fn with_events(mut self, events: Vec<LogEvent>) -> Self {
        self.events = events;
        self
    }

    pub(crate) fn with_cleanup_failure(mut self, cleanup: &RuntimeError) -> Self {
        self.message
            .push_str("; durable idempotency reservation cleanup also failed: ");
        self.message.push_str(cleanup.message());
        self
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
