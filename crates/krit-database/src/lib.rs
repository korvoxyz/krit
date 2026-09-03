//! Capability-scoped parameterized database access for Krit.
//!
//! This crate owns general database semantics and deliberately shares no schema,
//! table, or migration logic with `krit-state`. Guest code never supplies SQL, a
//! path, a DSN, driver options, or credentials: the host binds a logical
//! database name to a file and a strict catalog of named parameterized
//! statements, and the guest may only name those.

mod catalog;
mod connection;
mod error;
mod value;

pub use catalog::{
    ColumnName, MAX_CATALOG_STATEMENTS, MAX_PARAMETERS, MAX_RESULT_COLUMNS, ParameterType,
    StatementDefinition, StatementKind,
};
pub use connection::{
    Database, DatabaseLimits, DatabaseMode, MAX_BUSY_TIMEOUT, MAX_DATABASE_BYTES, MAX_DATABASES,
    MAX_OPERATIONS_PER_TRANSACTION, MAX_PARAMETER_BYTES, MAX_RESULT_ROWS, MAX_TRANSACTION_DURATION,
    MINIMUM_DATABASE_BYTES, OperationBounds, StatementRequest, Transaction, TransactionMode,
};
pub use error::{DatabaseError, DatabaseErrorKind};
pub use value::MAX_RESULT_BYTES;
