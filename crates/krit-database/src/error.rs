use std::{error::Error, fmt};

/// Classification of every database failure the host can surface.
///
/// Messages are fixed operator-facing strings. They never carry SQL, paths,
/// parameter values, row data, or credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseErrorKind {
    /// The configured file, schema, or connection is unusable.
    Connection,
    /// The statement catalog violates the strict host policy.
    Catalog,
    /// A transaction handle, lifecycle step, or ordering rule was violated.
    Transaction,
    /// A configured count, byte, or time bound was exceeded.
    Limit,
    /// The operation was stopped at a deadline or on cancellation.
    Interrupted,
    /// The database is busy or a write conflicted.
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseError {
    kind: DatabaseErrorKind,
    message: &'static str,
}

impl DatabaseError {
    pub(crate) const fn connection(message: &'static str) -> Self {
        Self {
            kind: DatabaseErrorKind::Connection,
            message,
        }
    }

    pub(crate) const fn catalog(message: &'static str) -> Self {
        Self {
            kind: DatabaseErrorKind::Catalog,
            message,
        }
    }

    pub(crate) const fn transaction(message: &'static str) -> Self {
        Self {
            kind: DatabaseErrorKind::Transaction,
            message,
        }
    }

    pub(crate) const fn limit(message: &'static str) -> Self {
        Self {
            kind: DatabaseErrorKind::Limit,
            message,
        }
    }

    pub(crate) const fn interrupted(message: &'static str) -> Self {
        Self {
            kind: DatabaseErrorKind::Interrupted,
            message,
        }
    }

    pub(crate) const fn conflict(message: &'static str) -> Self {
        Self {
            kind: DatabaseErrorKind::Conflict,
            message,
        }
    }

    pub const fn kind(&self) -> DatabaseErrorKind {
        self.kind
    }

    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// Whether SQLite stopped mid-operation at a deadline or on cancellation.
    pub const fn is_interrupted(&self) -> bool {
        matches!(self.kind, DatabaseErrorKind::Interrupted)
    }

    /// Stable diagnostic code for each failure class.
    pub const fn code(&self) -> &'static str {
        match self.kind {
            DatabaseErrorKind::Connection | DatabaseErrorKind::Catalog => "K5301",
            DatabaseErrorKind::Transaction => "K5302",
            DatabaseErrorKind::Limit
            | DatabaseErrorKind::Conflict
            | DatabaseErrorKind::Interrupted => "K5303",
        }
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for DatabaseError {}

/// Maps a SQLite failure onto a fixed message.
///
/// The underlying driver text is intentionally discarded: it can echo SQL,
/// column names, and parameter values.
pub(crate) fn map_sqlite(error: rusqlite::Error) -> DatabaseError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::OperationInterrupted) => DatabaseError::interrupted(
            "database operation was interrupted at its deadline or on cancellation",
        ),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            DatabaseError::conflict("application database is busy")
        }
        Some(rusqlite::ErrorCode::ReadOnly) => {
            DatabaseError::conflict("application database is read only")
        }
        Some(rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::TooBig) => {
            DatabaseError::limit("application database limit was exceeded")
        }
        Some(rusqlite::ErrorCode::ConstraintViolation) => {
            DatabaseError::conflict("application database constraint was violated")
        }
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            DatabaseError::connection("application database is corrupt")
        }
        _ => DatabaseError::connection("application database operation failed"),
    }
}
