use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use krit_database::{
    Database, DatabaseError, DatabaseErrorKind, DatabaseLimits, DatabaseMode, OperationBounds,
    StatementRequest, Transaction, TransactionMode,
};

use crate::RuntimeError;

pub use krit_database::{
    MAX_BUSY_TIMEOUT as MAX_DATABASE_BUSY_TIMEOUT, MAX_CATALOG_STATEMENTS,
    MAX_DATABASE_BYTES as MAX_APPLICATION_DATABASE_BYTES, MAX_DATABASES,
    MAX_OPERATIONS_PER_TRANSACTION, MAX_PARAMETER_BYTES, MAX_PARAMETERS, MAX_RESULT_BYTES,
    MAX_RESULT_COLUMNS, MAX_RESULT_ROWS, MAX_TRANSACTION_DURATION,
    MINIMUM_DATABASE_BYTES as MINIMUM_APPLICATION_DATABASE_BYTES, ParameterType, StatementKind,
};

/// Hard bound on transactions one invocation may begin.
pub const MAX_TRANSACTIONS_PER_INVOCATION: usize = 8;

/// One configured logical database before it is opened.
#[derive(Clone, Debug)]
pub struct DatabaseDefinition {
    pub path: PathBuf,
    pub mode: DatabaseMode,
    pub limits: DatabaseLimits,
    pub statements: BTreeMap<String, StatementRequest>,
}

/// Host-owned set of opened application databases.
#[derive(Clone, Default)]
pub struct DatabaseCatalog {
    databases: Arc<BTreeMap<String, Arc<Database>>>,
    max_transactions_per_invocation: usize,
}

impl DatabaseCatalog {
    pub fn open(
        definitions: BTreeMap<String, DatabaseDefinition>,
        max_transactions_per_invocation: usize,
    ) -> Result<Self, RuntimeError> {
        if definitions.len() > MAX_DATABASES {
            return Err(RuntimeError::database(
                "configured application databases exceed the Phase 7 bound",
            ));
        }
        if max_transactions_per_invocation == 0
            || max_transactions_per_invocation > MAX_TRANSACTIONS_PER_INVOCATION
        {
            return Err(RuntimeError::database(
                "configured transactions per invocation are outside the Phase 7 bounds",
            ));
        }
        let mut databases = BTreeMap::new();
        for (name, definition) in definitions {
            let database = Database::open(
                &name,
                &definition.path,
                definition.mode,
                definition.limits,
                definition.statements,
            )
            .map_err(map_database_error)?;
            databases.insert(name, Arc::new(database));
        }
        Ok(Self {
            databases: Arc::new(databases),
            max_transactions_per_invocation,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.databases.is_empty()
    }

    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.databases.keys().map(String::as_str)
    }

    pub fn mode(&self, name: &str) -> Option<DatabaseMode> {
        self.databases.get(name).map(|database| database.mode())
    }

    pub const fn max_transactions_per_invocation(&self) -> usize {
        self.max_transactions_per_invocation
    }

    /// Confirms every configured transaction bound fits inside one invocation.
    pub(crate) fn validate_for_runtime(&self, deadline: Duration) -> Result<(), RuntimeError> {
        for database in self.databases.values() {
            if database.limits().max_transaction_duration > deadline {
                return Err(RuntimeError::database(
                    "application database transaction bound exceeds the runtime deadline",
                ));
            }
            if database.limits().busy_timeout >= database.limits().max_transaction_duration {
                return Err(RuntimeError::database(
                    "application database busy timeout must stay below its transaction bound",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn database(&self, name: &str) -> Result<Arc<Database>, RuntimeError> {
        self.databases
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::authorization("application database is not configured"))
    }
}

impl std::fmt::Debug for DatabaseCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Paths and SQL are deliberately absent from every rendering.
        formatter
            .debug_struct("DatabaseCatalog")
            .field("databases", &self.databases.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// One guest-visible transaction handle.
///
/// The handle is deliberately inert: it carries only the slot number used to
/// find the live transaction. Ownership of the `Transaction` itself stays with
/// [`InvocationDatabases`] so that every invocation exit path - trap, deadline,
/// cancellation, invalid response, or a failed outcome - can roll back live
/// work without depending on the guest dropping a resource.
pub struct TransactionHandle {
    slot: usize,
}

impl TransactionHandle {
    pub(crate) const fn slot(&self) -> usize {
        self.slot
    }
}

/// One transaction owned by the current invocation.
struct LiveTransaction {
    database: Arc<Database>,
    transaction: Transaction,
    slot: usize,
}

impl Drop for LiveTransaction {
    /// Best-effort fail-safe for a transaction that escaped explicit cleanup.
    ///
    /// Explicit cleanup runs on every known exit path and surfaces its errors.
    /// This only covers an unforeseen unwind, and it never reports success: if
    /// the rollback fails, the connector poisons the database so it refuses
    /// further transactions instead of reusing an unknown connection state.
    fn drop(&mut self) {
        if self.transaction.is_completed() {
            return;
        }
        let _ = self.database.abandon(&mut self.transaction);
    }
}

/// Invocation-local database bookkeeping.
///
/// Krit's outcome model commits state, objects, queues, and the delivery
/// acknowledgement together. An application database is a *separate* durable
/// resource, so this tracker never claims cross-resource atomicity; it only
/// guarantees that every transaction is explicitly completed and that no
/// external effect runs while one is open.
#[derive(Default)]
pub(crate) struct InvocationDatabases {
    open: Vec<LiveTransaction>,
    begun: usize,
    queries: u64,
    executes: u64,
    commits: u64,
    rollbacks: u64,
    abandoned: u64,
    published_write_commit: bool,
}

impl InvocationDatabases {
    pub(crate) fn begin(
        &mut self,
        catalog: &DatabaseCatalog,
        name: &str,
        mode: TransactionMode,
        slot: usize,
        bounds: &OperationBounds,
    ) -> Result<TransactionHandle, RuntimeError> {
        if !self.open.is_empty() {
            return Err(RuntimeError::database_transaction(
                "one invocation may hold at most one open database transaction",
            ));
        }
        let next = self
            .begun
            .checked_add(1)
            .ok_or_else(|| RuntimeError::database("database transaction count overflowed"))?;
        if next > catalog.max_transactions_per_invocation() {
            return Err(RuntimeError::database(
                "invocation exceeded its configured database transaction count",
            ));
        }
        let database = catalog.database(name)?;
        let transaction = database.begin(mode, bounds).map_err(map_database_error)?;
        self.begun = next;
        self.open.push(LiveTransaction {
            database,
            transaction,
            slot,
        });
        Ok(TransactionHandle { slot })
    }

    pub(crate) fn query(
        &mut self,
        slot: usize,
        statement: &str,
        parameters: &[String],
        bounds: &OperationBounds,
    ) -> Result<String, RuntimeError> {
        let live = self.live_mut(slot)?;
        let outcome = live
            .database
            .query(&mut live.transaction, statement, parameters, bounds)
            .map_err(map_database_error);
        self.queries = self.queries.saturating_add(1);
        self.retire_completed();
        outcome
    }

    pub(crate) fn execute(
        &mut self,
        slot: usize,
        statement: &str,
        parameters: &[String],
        bounds: &OperationBounds,
    ) -> Result<i64, RuntimeError> {
        let live = self.live_mut(slot)?;
        let outcome = live
            .database
            .execute(&mut live.transaction, statement, parameters, bounds)
            .map_err(map_database_error);
        self.executes = self.executes.saturating_add(1);
        self.retire_completed();
        outcome
    }

    /// Commits and closes one transaction.
    ///
    /// The commit publishes immediately. Krit does not pretend that a separate
    /// SQLite file can join the invocation's atomic outcome, so the honest
    /// two-resource boundary is recorded here and reported in stats.
    pub(crate) fn commit(
        &mut self,
        slot: usize,
        bounds: &OperationBounds,
    ) -> Result<(), RuntimeError> {
        let live = self.live_mut(slot)?;
        let write = live.transaction.mode() == TransactionMode::Write;
        let outcome = live
            .database
            .commit(&mut live.transaction, bounds)
            .map_err(map_database_error);
        self.commits = self.commits.saturating_add(1);
        if outcome.is_ok() && write {
            self.published_write_commit = true;
        }
        self.retire(slot);
        outcome
    }

    pub(crate) fn rollback(&mut self, slot: usize) -> Result<(), RuntimeError> {
        let live = self.live_mut(slot)?;
        let outcome = live
            .database
            .rollback(&mut live.transaction)
            .map_err(map_database_error);
        self.rollbacks = self.rollbacks.saturating_add(1);
        self.retire(slot);
        outcome
    }

    /// Rolls back every transaction still open at an invocation exit.
    ///
    /// This runs on success and on every failure path before the store is
    /// dropped or reused, so a trapped, cancelled, or timed-out invocation can
    /// never leave a SQLite connection inside a transaction. Failures are
    /// returned rather than hidden.
    pub(crate) fn abandon_all(&mut self) -> Result<(), RuntimeError> {
        let mut failure = None;
        for mut live in std::mem::take(&mut self.open) {
            if live.transaction.is_completed() {
                continue;
            }
            self.abandoned = self.abandoned.saturating_add(1);
            if let Err(error) = live.database.abandon(&mut live.transaction) {
                failure.get_or_insert(map_database_error(error));
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Whether a transaction is currently open.
    ///
    /// Holding a SQLite write transaction across a network call would keep a
    /// database lock for an unbounded time, so external effects are refused
    /// while this is true.
    pub(crate) fn has_open_transaction(&self) -> bool {
        !self.open.is_empty()
    }

    pub(crate) const fn queries(&self) -> u64 {
        self.queries
    }

    pub(crate) const fn executes(&self) -> u64 {
        self.executes
    }

    pub(crate) const fn commits(&self) -> u64 {
        self.commits
    }

    pub(crate) const fn rollbacks(&self) -> u64 {
        self.rollbacks
    }

    pub(crate) const fn abandoned(&self) -> u64 {
        self.abandoned
    }

    pub(crate) const fn published_write_commit(&self) -> bool {
        self.published_write_commit
    }

    fn live_mut(&mut self, slot: usize) -> Result<&mut LiveTransaction, RuntimeError> {
        let live = self
            .open
            .iter_mut()
            .find(|live| live.slot == slot)
            .ok_or_else(|| {
                RuntimeError::database_transaction(
                    "database transaction handle is not open in this invocation",
                )
            })?;
        if live.transaction.is_completed() {
            return Err(RuntimeError::database_transaction(
                "database transaction is already completed",
            ));
        }
        Ok(live)
    }

    /// Rolls back one transaction whose guest handle was dropped.
    pub(crate) fn abandon_slot(&mut self, slot: usize) -> Result<(), RuntimeError> {
        let Some(position) = self.open.iter().position(|live| live.slot == slot) else {
            return Ok(());
        };
        let mut live = self.open.remove(position);
        if live.transaction.is_completed() {
            return Ok(());
        }
        self.abandoned = self.abandoned.saturating_add(1);
        live.database
            .abandon(&mut live.transaction)
            .map_err(map_database_error)
    }

    /// Drops any transaction SQLite already unwound, for example after an
    /// interrupt, so the invocation is not blamed for leaving one open.
    fn retire_completed(&mut self) {
        self.open.retain(|live| !live.transaction.is_completed());
    }

    fn retire(&mut self, slot: usize) {
        self.open.retain(|live| live.slot != slot);
    }
}

pub(crate) fn map_database_error(error: DatabaseError) -> RuntimeError {
    match error.kind() {
        DatabaseErrorKind::Connection | DatabaseErrorKind::Catalog => {
            RuntimeError::database(error.message())
        }
        DatabaseErrorKind::Transaction => RuntimeError::database_transaction(error.message()),
        DatabaseErrorKind::Limit | DatabaseErrorKind::Conflict | DatabaseErrorKind::Interrupted => {
            RuntimeError::database_limit(error.message())
        }
    }
}
