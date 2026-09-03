use std::{error::Error, fmt};

/// Classification of every cache failure the host can surface.
///
/// Messages are fixed operator-facing strings. They never carry a key, a value,
/// a namespace payload, or a credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheErrorKind {
    /// The namespace is not configured, or the operation is not permitted on it.
    Namespace,
    /// A configured count, byte, or time bound was exceeded.
    Limit,
    /// The backing store is unusable, for example after a poisoned lock.
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheError {
    kind: CacheErrorKind,
    message: &'static str,
}

impl CacheError {
    pub(crate) const fn namespace(message: &'static str) -> Self {
        Self {
            kind: CacheErrorKind::Namespace,
            message,
        }
    }

    pub(crate) const fn limit(message: &'static str) -> Self {
        Self {
            kind: CacheErrorKind::Limit,
            message,
        }
    }

    pub(crate) const fn unavailable(message: &'static str) -> Self {
        Self {
            kind: CacheErrorKind::Unavailable,
            message,
        }
    }

    pub const fn kind(&self) -> CacheErrorKind {
        self.kind
    }

    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// Stable diagnostic code for each failure class.
    pub const fn code(&self) -> &'static str {
        match self.kind {
            CacheErrorKind::Namespace => "K5401",
            CacheErrorKind::Limit => "K5402",
            CacheErrorKind::Unavailable => "K5403",
        }
    }
}

impl fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for CacheError {}
