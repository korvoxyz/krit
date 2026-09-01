use std::{error::Error, fmt};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssistErrorKind {
    Disabled,
    Context,
    Provider,
    Proposal,
    Candidate,
    Permission,
    Io,
}

#[derive(Debug)]
pub struct AssistError {
    code: &'static str,
    kind: AssistErrorKind,
    message: String,
}

impl AssistError {
    pub(crate) fn disabled(message: impl Into<String>) -> Self {
        Self::new("K8101", AssistErrorKind::Disabled, message)
    }

    pub(crate) fn context(message: impl Into<String>) -> Self {
        Self::new("K8102", AssistErrorKind::Context, message)
    }

    pub(crate) fn provider(message: impl Into<String>) -> Self {
        Self::new("K8103", AssistErrorKind::Provider, message)
    }

    pub(crate) fn proposal(message: impl Into<String>) -> Self {
        Self::new("K8104", AssistErrorKind::Proposal, message)
    }

    pub(crate) fn candidate(message: impl Into<String>) -> Self {
        Self::new("K8105", AssistErrorKind::Candidate, message)
    }

    pub(crate) fn permission(message: impl Into<String>) -> Self {
        Self::new("K8106", AssistErrorKind::Permission, message)
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new("K8107", AssistErrorKind::Io, message)
    }

    fn new(code: &'static str, kind: AssistErrorKind, message: impl Into<String>) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn kind(&self) -> AssistErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn exit_status(&self) -> u8 {
        match self.kind {
            AssistErrorKind::Permission => 4,
            AssistErrorKind::Disabled
            | AssistErrorKind::Context
            | AssistErrorKind::Provider
            | AssistErrorKind::Proposal
            | AssistErrorKind::Candidate
            | AssistErrorKind::Io => 1,
        }
    }

    pub fn render_json(&self) -> String {
        #[derive(Serialize)]
        struct JsonError<'a> {
            schema: u32,
            severity: &'static str,
            code: &'a str,
            message: &'a str,
        }

        serde_json::to_string(&JsonError {
            schema: 1,
            severity: "error",
            code: self.code,
            message: &self.message,
        })
        .expect("assist error serialization cannot fail")
    }
}

impl fmt::Display for AssistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "error[{}]: {}", self.code, self.message)
    }
}

impl Error for AssistError {}
