use std::{
    collections::BTreeMap,
    fmt, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags, config::DbConfig, types::Value as SqliteValue};

use crate::{
    ParameterType, StatementDefinition, StatementKind,
    catalog::{bind_value, validate_statement},
    error::{DatabaseError, map_sqlite},
    value::{MAX_RESULT_BYTES, RowEncoder},
};

/// Virtual-machine steps between progress-handler checks.
///
/// Small enough that a recursive or data-heavy statement is interrupted
/// promptly, large enough that the callback is not a measurable tax.
const PROGRESS_STEPS: std::os::raw::c_int = 512;

/// Shortest busy wait the connector will ever install.
const MINIMUM_BUSY_TIMEOUT: Duration = Duration::from_millis(1);

/// Hard bound on logical databases one host may configure.
pub const MAX_DATABASES: usize = 8;
/// Smallest byte budget a configured database may declare.
pub const MINIMUM_DATABASE_BYTES: u64 = 64 * 1024;
/// Largest byte budget a configured database may declare.
pub const MAX_DATABASE_BYTES: u64 = 1024 * 1024 * 1024;
/// Hard bound on how long one transaction may stay open.
pub const MAX_TRANSACTION_DURATION: Duration = Duration::from_secs(5);
/// Hard bound on SQLite busy waiting.
pub const MAX_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Hard bound on operations inside one transaction.
pub const MAX_OPERATIONS_PER_TRANSACTION: usize = 256;
/// Hard bound on rows one query may return.
pub const MAX_RESULT_ROWS: usize = 4096;
/// Hard bound on one bound parameter.
pub const MAX_PARAMETER_BYTES: usize = 64 * 1024;
/// Ceiling installed on the rollback journal so a transaction cannot grow the
/// on-disk footprint without bound.
const JOURNAL_SIZE_LIMIT: i64 = 8 * 1024 * 1024;

/// Cooperative bounds applied to one database operation.
///
/// Wasmtime's epoch interruption cannot stop a call that is already inside
/// SQLite, so every operation additionally installs a progress handler that
/// observes the transaction deadline, the invocation deadline, and embedding
/// cancellation.
#[derive(Clone)]
pub struct OperationBounds {
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    invocation_deadline: Instant,
}

impl OperationBounds {
    pub fn new(
        invocation_deadline: Instant,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            cancelled,
            invocation_deadline,
        }
    }

    /// Bounds that never cancel, for configuration-time work.
    pub fn unbounded_for_setup(within: Duration) -> Self {
        Self {
            cancelled: Arc::new(|| false),
            invocation_deadline: Instant::now() + within,
        }
    }

    fn is_cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

impl fmt::Debug for OperationBounds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("OperationBounds").finish()
    }
}

/// Access the host grants to one logical database.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseMode {
    ReadOnly,
    ReadWrite,
}

/// Access one transaction was opened with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionMode {
    Read,
    Write,
}

impl TransactionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// Bounded operational policy for one logical database.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatabaseLimits {
    pub busy_timeout: Duration,
    pub max_database_bytes: u64,
    pub max_transaction_duration: Duration,
    pub max_operations_per_transaction: usize,
    pub max_parameter_bytes: usize,
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_result_bytes: usize,
}

impl DatabaseLimits {
    /// Confirms every declared bound sits inside the Phase 7 envelope.
    ///
    /// Exposed so a host can reject an out-of-range limit during pure
    /// configuration validation, before anything is created or opened.
    pub fn validate(self) -> Result<(), DatabaseError> {
        if self.busy_timeout.is_zero()
            || self.busy_timeout > MAX_BUSY_TIMEOUT
            || self.max_database_bytes < MINIMUM_DATABASE_BYTES
            || self.max_database_bytes > MAX_DATABASE_BYTES
            || self.max_transaction_duration.is_zero()
            || self.max_transaction_duration > MAX_TRANSACTION_DURATION
            || self.max_operations_per_transaction == 0
            || self.max_operations_per_transaction > MAX_OPERATIONS_PER_TRANSACTION
            || self.max_parameter_bytes == 0
            || self.max_parameter_bytes > MAX_PARAMETER_BYTES
            || self.max_rows == 0
            || self.max_rows > MAX_RESULT_ROWS
            || self.max_columns == 0
            || self.max_columns > crate::MAX_RESULT_COLUMNS
            || self.max_result_bytes == 0
            || self.max_result_bytes > MAX_RESULT_BYTES
        {
            return Err(DatabaseError::catalog(
                "application database limits are outside the Phase 7 bounds",
            ));
        }
        Ok(())
    }
}

/// One opened logical database with its validated statement catalog.
///
/// The connection is host-owned. No path, URI, driver option, or credential is
/// ever exposed to guest code.
pub struct Database {
    name: String,
    mode: DatabaseMode,
    limits: DatabaseLimits,
    statements: BTreeMap<String, StatementDefinition>,
    connection: Mutex<Connection>,
    path: PathBuf,
    /// Set when cleanup could not restore a known-good connection state. A
    /// poisoned database refuses new transactions instead of silently reusing a
    /// connection whose transaction state is unknown.
    poisoned: AtomicBool,
}

impl fmt::Debug for Database {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The configured path is deliberately absent from every rendering.
        formatter
            .debug_struct("Database")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("statements", &self.statements.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// One catalog entry as supplied by host configuration, before validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatementRequest {
    pub kind: StatementKind,
    pub sql: String,
    pub parameters: Vec<ParameterType>,
    pub columns: Vec<String>,
}

impl Database {
    /// Opens an existing database file and validates its whole catalog.
    ///
    /// The file must already exist: Krit never creates, migrates, resets, or
    /// otherwise owns an application schema. That remains the operator's job.
    pub fn open(
        name: &str,
        path: &Path,
        mode: DatabaseMode,
        limits: DatabaseLimits,
        statements: BTreeMap<String, StatementRequest>,
    ) -> Result<Self, DatabaseError> {
        limits.validate()?;
        if statements.is_empty() || statements.len() > crate::MAX_CATALOG_STATEMENTS {
            return Err(DatabaseError::catalog(
                "application database must declare 1..=64 catalog statements",
            ));
        }
        let path = resolve_existing_path(path)?;
        if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return Err(DatabaseError::connection(
                "application database file does not exist",
            ));
        }
        if total_disk_bytes(&path) > limits.max_database_bytes {
            return Err(DatabaseError::limit(
                "application database and its journal exceed the configured byte budget",
            ));
        }
        let mut flags = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        // No CREATE flag: an absent file is an operator error, never an implicit
        // empty database. No URI flag: a path can never carry driver options.
        flags |= match mode {
            DatabaseMode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
            DatabaseMode::ReadWrite => OpenFlags::SQLITE_OPEN_READ_WRITE,
        };
        let connection = Connection::open_with_flags(&path, flags).map_err(map_sqlite)?;
        connection
            .busy_timeout(limits.busy_timeout)
            .map_err(map_sqlite)?;
        configure_safety(&connection, mode, limits)?;

        let mut validated = BTreeMap::new();
        for (statement_name, request) in statements {
            if mode == DatabaseMode::ReadOnly && request.kind != StatementKind::Query {
                return Err(DatabaseError::catalog(
                    "a read-only application database may only declare `query` statements",
                ));
            }
            let definition = validate_statement(
                &connection,
                request.kind,
                &request.sql,
                &request.parameters,
                &request.columns,
                limits.max_columns,
            )?;
            validated.insert(statement_name, definition);
        }
        Ok(Self {
            name: name.to_owned(),
            mode,
            limits,
            statements: validated,
            connection: Mutex::new(connection),
            path,
            poisoned: AtomicBool::new(false),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn mode(&self) -> DatabaseMode {
        self.mode
    }

    pub const fn limits(&self) -> DatabaseLimits {
        self.limits
    }

    pub fn statement_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.statements.keys().map(String::as_str)
    }

    pub fn statement(&self, name: &str) -> Option<&StatementDefinition> {
        self.statements.get(name)
    }

    /// Begins one bounded transaction.
    ///
    /// A read transaction uses `BEGIN DEFERRED`; a write transaction uses
    /// `BEGIN IMMEDIATE` so write conflicts surface at begin rather than at
    /// commit. A write transaction on a read-only database is refused.
    pub fn begin(
        &self,
        mode: TransactionMode,
        bounds: &OperationBounds,
    ) -> Result<Transaction, DatabaseError> {
        if mode == TransactionMode::Write && self.mode == DatabaseMode::ReadOnly {
            return Err(DatabaseError::transaction(
                "application database is configured read only",
            ));
        }
        self.require_healthy()?;
        let started = Instant::now();
        let deadline = started
            .checked_add(self.limits.max_transaction_duration)
            .ok_or_else(|| DatabaseError::limit("database transaction deadline overflowed"))?;
        let connection = self.lock()?;
        if !connection.is_autocommit() {
            // A previous invocation left this connection inside a transaction.
            // Continuing would silently join unrelated work, so fail closed.
            self.poisoned.store(true, Ordering::SeqCst);
            return Err(DatabaseError::transaction(
                "application database connection was left inside a transaction",
            ));
        }
        let guard = InterruptGuard::install(&connection, deadline, bounds, self.limits)?;
        let behavior = match mode {
            TransactionMode::Read => "BEGIN DEFERRED",
            TransactionMode::Write => "BEGIN IMMEDIATE",
        };
        let outcome = connection.execute_batch(behavior).map_err(map_sqlite);
        guard.remove(&connection);
        outcome?;
        drop(connection);
        Ok(Transaction {
            mode,
            started,
            deadline,
            operations: 0,
            outcome: None,
        })
    }

    /// Runs one catalog `query` statement inside an open transaction.
    ///
    /// Rows are encoded incrementally while stepping, so native memory stays
    /// bounded by the configured result budget plus one in-flight value even
    /// when the underlying statement would produce far more data.
    pub fn query(
        &self,
        transaction: &mut Transaction,
        statement: &str,
        parameters: &[String],
        bounds: &OperationBounds,
    ) -> Result<String, DatabaseError> {
        let definition = self.prepare_operation(transaction, statement, StatementKind::Query)?;
        let bound = self.bind_parameters(definition, parameters)?;
        let deadline = transaction.deadline;
        let connection = self.lock()?;
        let guard = InterruptGuard::install(&connection, deadline, bounds, self.limits)?;
        let outcome = self.run_query(&connection, definition, &bound);
        guard.remove(&connection);
        drop(connection);
        self.finish_operation(transaction, outcome)
    }

    fn run_query(
        &self,
        connection: &Connection,
        definition: &StatementDefinition,
        bound: &[SqliteValue],
    ) -> Result<String, DatabaseError> {
        let mut prepared = connection.prepare(&definition.sql).map_err(map_sqlite)?;
        for (index, value) in bound.iter().enumerate() {
            prepared
                .raw_bind_parameter(index + 1, value)
                .map_err(map_sqlite)?;
        }
        let mut encoder = RowEncoder::new(
            &definition.columns,
            self.limits.max_result_bytes,
            self.limits.max_rows,
        )?;
        let mut rows = prepared.raw_query();
        while let Some(row) = rows.next().map_err(map_sqlite)? {
            encoder.begin_row()?;
            for index in 0..definition.columns.len() {
                encoder.push_column(row.get_ref(index).map_err(map_sqlite)?)?;
            }
            encoder.end_row()?;
        }
        drop(rows);
        drop(prepared);
        encoder.finish()
    }

    /// Runs one catalog `execute` statement inside an open write transaction.
    pub fn execute(
        &self,
        transaction: &mut Transaction,
        statement: &str,
        parameters: &[String],
        bounds: &OperationBounds,
    ) -> Result<i64, DatabaseError> {
        if transaction.mode != TransactionMode::Write {
            return Err(DatabaseError::transaction(
                "a read transaction cannot run a mutating statement",
            ));
        }
        let definition = self.prepare_operation(transaction, statement, StatementKind::Execute)?;
        let bound = self.bind_parameters(definition, parameters)?;
        let deadline = transaction.deadline;
        let connection = self.lock()?;
        let guard = InterruptGuard::install(&connection, deadline, bounds, self.limits)?;
        let outcome = self
            .run_execute(&connection, definition, &bound)
            .and_then(|changed| {
                // The on-disk footprint is checked while the transaction is
                // still open so an over-budget write is rolled back rather than
                // published.
                self.require_disk_budget()?;
                Ok(changed)
            });
        guard.remove(&connection);
        drop(connection);
        self.finish_operation(transaction, outcome)
    }

    fn run_execute(
        &self,
        connection: &Connection,
        definition: &StatementDefinition,
        bound: &[SqliteValue],
    ) -> Result<i64, DatabaseError> {
        let mut prepared = connection.prepare(&definition.sql).map_err(map_sqlite)?;
        for (index, value) in bound.iter().enumerate() {
            prepared
                .raw_bind_parameter(index + 1, value)
                .map_err(map_sqlite)?;
        }
        let changed = prepared.raw_execute().map_err(map_sqlite)?;
        drop(prepared);
        i64::try_from(changed)
            .map_err(|_| DatabaseError::limit("database affected-row count exceeds i64"))
    }

    /// Commits and closes one open transaction.
    pub fn commit(
        &self,
        transaction: &mut Transaction,
        bounds: &OperationBounds,
    ) -> Result<(), DatabaseError> {
        transaction.require_open()?;
        transaction.require_within_deadline()?;
        let deadline = transaction.deadline;
        let connection = self.lock()?;
        let guard = InterruptGuard::install(&connection, deadline, bounds, self.limits)?;
        let outcome = connection.execute_batch("COMMIT").map_err(map_sqlite);
        let outcome = match outcome {
            Ok(()) => self.checkpoint_and_verify(&connection),
            Err(error) => {
                // A failed commit must not leave the connection inside a
                // transaction; the rollback failure is surfaced, never hidden.
                let undo = connection.execute_batch("ROLLBACK").map_err(map_sqlite);
                if undo.is_err() {
                    self.poisoned.store(true, Ordering::SeqCst);
                }
                undo.and(Err(error))
            }
        };
        guard.remove(&connection);
        drop(connection);
        transaction.outcome = Some(if outcome.is_ok() {
            Completion::Committed
        } else {
            Completion::RolledBack
        });
        outcome
    }

    /// Truncates the rollback journal and confirms the disk contract still
    /// holds after a commit publishes.
    fn checkpoint_and_verify(&self, connection: &Connection) -> Result<(), DatabaseError> {
        if self.mode == DatabaseMode::ReadWrite {
            connection
                .pragma_update(None, "journal_size_limit", 0i64)
                .map_err(map_sqlite)?;
            connection
                .pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)
                .map_err(map_sqlite)?;
        }
        self.require_disk_budget()
    }

    /// Rolls back and closes one open transaction.
    pub fn rollback(&self, transaction: &mut Transaction) -> Result<(), DatabaseError> {
        if transaction.outcome == Some(Completion::Interrupted) {
            // The transaction was already rolled back when it was interrupted;
            // reporting success here states a fact rather than hiding an error.
            transaction.outcome = Some(Completion::RolledBack);
            return Ok(());
        }
        transaction.require_open()?;
        let outcome = self.rollback_now();
        transaction.outcome = Some(Completion::RolledBack);
        outcome
    }

    fn rollback_now(&self) -> Result<(), DatabaseError> {
        let connection = self.lock()?;
        if connection.is_autocommit() {
            // SQLite already unwound the transaction, for example after an
            // interrupt; there is nothing left to roll back.
            return Ok(());
        }
        let outcome = connection.execute_batch("ROLLBACK").map_err(map_sqlite);
        if outcome.is_err() {
            self.poisoned.store(true, Ordering::SeqCst);
        }
        drop(connection);
        outcome
    }

    /// Rolls back an abandoned transaction during invocation cleanup.
    ///
    /// Cleanup never reports success for work the guest did not complete: the
    /// caller still fails the invocation.
    pub fn abandon(&self, transaction: &mut Transaction) -> Result<(), DatabaseError> {
        if transaction.is_completed() {
            return Ok(());
        }
        self.rollback(transaction)
    }

    /// Whether the connection is known to be outside any transaction.
    pub fn is_idle(&self) -> Result<bool, DatabaseError> {
        Ok(self.lock()?.is_autocommit())
    }

    /// Whether cleanup previously failed to restore a known-good state.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }

    /// Total bytes the database occupies on disk, including its sidecars.
    pub fn disk_bytes(&self) -> u64 {
        total_disk_bytes(&self.path)
    }

    fn require_healthy(&self) -> Result<(), DatabaseError> {
        if self.is_poisoned() {
            return Err(DatabaseError::connection(
                "application database connection is poisoned and refuses new transactions",
            ));
        }
        Ok(())
    }

    /// Enforces the declared budget across the main file and its sidecars.
    fn require_disk_budget(&self) -> Result<(), DatabaseError> {
        if total_disk_bytes(&self.path) > self.limits.max_database_bytes {
            return Err(DatabaseError::limit(
                "application database and its journal exceed the configured byte budget",
            ));
        }
        Ok(())
    }

    /// Applies an operation outcome, rolling back on interruption.
    fn finish_operation<T>(
        &self,
        transaction: &mut Transaction,
        outcome: Result<T, DatabaseError>,
    ) -> Result<T, DatabaseError> {
        match outcome {
            Ok(value) => Ok(value),
            Err(error) => {
                if error.is_interrupted() {
                    // An interrupted statement leaves no partial mutation
                    // behind: unwind immediately and mark the handle finished.
                    let undo = self.rollback_now();
                    transaction.outcome = Some(Completion::Interrupted);
                    undo?;
                }
                Err(error)
            }
        }
    }

    fn prepare_operation(
        &self,
        transaction: &mut Transaction,
        statement: &str,
        expected: StatementKind,
    ) -> Result<&StatementDefinition, DatabaseError> {
        self.require_healthy()?;
        transaction.require_open()?;
        transaction.require_within_deadline()?;
        transaction.record_operation(self.limits.max_operations_per_transaction)?;
        let definition = self
            .statements
            .get(statement)
            .ok_or_else(|| DatabaseError::catalog("named statement is not in the catalog"))?;
        if definition.kind != expected {
            return Err(DatabaseError::catalog(
                "named statement kind does not match the requested operation",
            ));
        }
        Ok(definition)
    }

    fn bind_parameters(
        &self,
        definition: &StatementDefinition,
        parameters: &[String],
    ) -> Result<Vec<SqliteValue>, DatabaseError> {
        if parameters.len() != definition.parameters.len() {
            return Err(DatabaseError::limit(
                "database parameter count does not match the catalog statement",
            ));
        }
        let mut bound = Vec::with_capacity(parameters.len());
        for (declared, value) in definition.parameters.iter().zip(parameters) {
            if value.len() > self.limits.max_parameter_bytes {
                return Err(DatabaseError::limit(
                    "database parameter exceeds its configured byte bound",
                ));
            }
            if value.contains('\0') {
                return Err(DatabaseError::limit(
                    "database parameter contains a NUL byte",
                ));
            }
            bound.push(bind_value(*declared, value)?);
        }
        Ok(bound)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, DatabaseError> {
        self.connection
            .lock()
            .map_err(|_| DatabaseError::connection("application database lock is unavailable"))
    }
}

/// How one transaction finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Completion {
    Committed,
    RolledBack,
    /// SQLite unwound the transaction when the operation was interrupted.
    Interrupted,
}

/// One open transaction handle.
///
/// The value is host-owned and never serialized, compared, logged, or stored.
#[derive(Debug)]
pub struct Transaction {
    mode: TransactionMode,
    started: Instant,
    deadline: Instant,
    operations: usize,
    outcome: Option<Completion>,
}

impl Transaction {
    pub const fn mode(&self) -> TransactionMode {
        self.mode
    }

    pub const fn is_completed(&self) -> bool {
        self.outcome.is_some()
    }

    /// Whether the transaction committed durable work.
    pub fn is_committed(&self) -> bool {
        self.outcome == Some(Completion::Committed)
    }

    pub const fn operations(&self) -> usize {
        self.operations
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn require_open(&self) -> Result<(), DatabaseError> {
        if self.is_completed() {
            return Err(DatabaseError::transaction(
                "database transaction is already completed",
            ));
        }
        Ok(())
    }

    fn require_within_deadline(&self) -> Result<(), DatabaseError> {
        if Instant::now() >= self.deadline {
            return Err(DatabaseError::limit(
                "database transaction exceeded its configured time bound",
            ));
        }
        Ok(())
    }

    fn record_operation(&mut self, maximum: usize) -> Result<(), DatabaseError> {
        let next = self
            .operations
            .checked_add(1)
            .ok_or_else(|| DatabaseError::limit("database operation count overflowed"))?;
        if next > maximum {
            return Err(DatabaseError::limit(
                "database transaction exceeded its configured operation bound",
            ));
        }
        self.operations = next;
        Ok(())
    }
}

/// Installs, and then removes, the per-operation interrupt and busy bounds.
///
/// Wasmtime's epoch deadline only interrupts guest code. A statement already
/// executing inside SQLite - a recursive CTE, a large scan, a lock wait - would
/// otherwise run to completion. The progress handler below converts the
/// transaction deadline, the invocation deadline, and embedding cancellation
/// into a prompt `SQLITE_INTERRUPT`.
struct InterruptGuard;

impl InterruptGuard {
    fn install(
        connection: &Connection,
        transaction_deadline: Instant,
        bounds: &OperationBounds,
        limits: DatabaseLimits,
    ) -> Result<Self, DatabaseError> {
        if bounds.is_cancelled() {
            return Err(DatabaseError::interrupted(
                "database operation cancelled by the embedding host",
            ));
        }
        let deadline = transaction_deadline.min(bounds.invocation_deadline);
        let now = Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return Err(DatabaseError::interrupted(
                "database transaction exceeded its configured time bound",
            ));
        };
        // Busy waiting can never outlast the work it is waiting for.
        let busy = limits.busy_timeout.min(remaining).max(MINIMUM_BUSY_TIMEOUT);
        connection.busy_timeout(busy).map_err(map_sqlite)?;
        let cancelled = Arc::clone(&bounds.cancelled);
        connection
            .progress_handler(
                PROGRESS_STEPS,
                Some(move || Instant::now() >= deadline || cancelled()),
            )
            .map_err(map_sqlite)?;
        Ok(Self)
    }

    /// Removes the handler so it cannot observe a stale deadline later.
    fn remove(self, connection: &Connection) {
        // Clearing a progress handler cannot fail in SQLite; the result is
        // ignored only because there is no error to report.
        let _ = connection.progress_handler(0, None::<fn() -> bool>);
    }
}

/// Sums the database file and every sidecar SQLite may create for it.
fn total_disk_bytes(path: &Path) -> u64 {
    let mut total = fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        total = total.saturating_add(
            fs::metadata(PathBuf::from(sidecar))
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        );
    }
    total
}

/// Applies the same defensive SQLite posture the durable state store uses.
fn configure_safety(
    connection: &Connection,
    mode: DatabaseMode,
    limits: DatabaseLimits,
) -> Result<(), DatabaseError> {
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(map_sqlite)?;
    // Extension loading is compiled out of the bundled build and stays off
    // through the defensive posture below; the schema is never writable and
    // double-quoted strings never silently become literals.
    for (option, enabled) in [
        (DbConfig::SQLITE_DBCONFIG_WRITABLE_SCHEMA, false),
        (DbConfig::SQLITE_DBCONFIG_DQS_DML, false),
        (DbConfig::SQLITE_DBCONFIG_DQS_DDL, false),
        (DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
    ] {
        connection
            .set_db_config(option, enabled)
            .map_err(map_sqlite)?;
    }
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA trusted_schema = OFF;
             PRAGMA recursive_triggers = OFF;
             PRAGMA cell_size_check = ON;",
        )
        .map_err(map_sqlite)?;
    configure_journal(connection, mode)?;
    let page_size: i64 = connection
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(map_sqlite)?;
    let page_size = u64::try_from(page_size)
        .map_err(|_| DatabaseError::connection("SQLite page size is invalid"))?;
    let max_pages = limits
        .max_database_bytes
        .checked_div(page_size)
        .filter(|pages| *pages > 0)
        .ok_or_else(|| {
            DatabaseError::limit("application database budget is smaller than one page")
        })?;
    let max_pages = i64::try_from(max_pages)
        .map_err(|_| DatabaseError::limit("application database page limit exceeds SQLite"))?;
    connection
        .pragma_update(None, "max_page_count", max_pages)
        .map_err(map_sqlite)?;
    let current_pages: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .map_err(map_sqlite)?;
    if u64::try_from(current_pages)
        .ok()
        .and_then(|pages| pages.checked_mul(page_size))
        .is_none_or(|bytes| bytes > limits.max_database_bytes)
    {
        return Err(DatabaseError::limit(
            "application database exceeds its configured byte limit",
        ));
    }
    let integrity: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(map_sqlite)?;
    if integrity != "ok" {
        return Err(DatabaseError::connection(
            "application database failed its integrity check",
        ));
    }
    Ok(())
}

/// Refuses write-ahead logging and bounds the rollback journal.
///
/// A WAL database's on-disk footprint is not bounded by the main file: a pinned
/// reader can hold back checkpointing until the `-wal` sidecar grows without
/// limit, so the declared byte budget would become unenforceable. Krit refuses
/// WAL application databases outright and uses a rollback journal, whose peak
/// size is bounded by the pages one bounded transaction touches and which is
/// truncated at every commit.
fn configure_journal(connection: &Connection, mode: DatabaseMode) -> Result<(), DatabaseError> {
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(map_sqlite)?;
    if journal.eq_ignore_ascii_case("wal") {
        return Err(DatabaseError::connection(
            "application database uses write-ahead logging, whose on-disk size Krit cannot bound",
        ));
    }
    if mode == DatabaseMode::ReadOnly {
        // A read-only connection cannot change journal settings, and a
        // rollback-journal database is already bounded for readers.
        return Ok(());
    }
    let selected: String = connection
        .pragma_update_and_check(None, "journal_mode", "TRUNCATE", |row| row.get(0))
        .map_err(map_sqlite)?;
    if !selected.eq_ignore_ascii_case("truncate") {
        return Err(DatabaseError::connection(
            "application database refused a bounded rollback journal",
        ));
    }
    connection
        .pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)
        .map_err(map_sqlite)?;
    Ok(())
}

/// Resolves an already-existing database file without following symlinks.
fn resolve_existing_path(path: &Path) -> Result<PathBuf, DatabaseError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(DatabaseError::connection(
                "application database path must not be a symbolic link",
            ));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(DatabaseError::connection(
                "application database path is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(DatabaseError::connection(
                "application database file does not exist",
            ));
        }
        Err(_) => {
            return Err(DatabaseError::connection(
                "application database path could not be inspected",
            ));
        }
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| DatabaseError::connection("application database path has no file name"))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|_| DatabaseError::connection("application database parent is not accessible"))?;
    Ok(parent.join(file_name))
}
