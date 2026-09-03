use std::{
    error::Error,
    fmt, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use rusqlite::config::DbConfig;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};

mod jobs;
mod schema;

pub use jobs::{
    BucketPolicy, CommitPlan, Completion, DeadLetterEntry, FireDelivery, FireLease, JobDelivery,
    JobDisposition, JobLease, MAX_OBJECT_LIST_KEYS, ObjectEntry, QueuePolicy, ScheduleCatchUp,
    SchedulePolicy,
};

pub(crate) const APPLICATION_ID: i64 = 0x4b52_4954;

/// Current strict store schema. Schema 1 stores migrate forward in place.
pub const STORE_SCHEMA_VERSION: i64 = 2;

/// Hard byte bound for queue, schedule, and bucket resource names.
pub(crate) const MAX_RESOURCE_NAME_BYTES: usize = 64;

/// Smallest configurable database budget.
///
/// An empty schema-2 store occupies 27 SQLite pages (108 KiB at the 4 KiB
/// default page size), so a smaller budget could never hold the schema. Stores
/// whose existing page size makes the schema larger than the configured budget
/// are rejected by the post-schema page check in [`DurableStore::open`].
pub const MINIMUM_DATABASE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Durability {
    Full,
    Normal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreLimits {
    pub busy_timeout: Duration,
    pub max_operations: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
    pub max_transaction_bytes: usize,
    pub max_database_bytes: u64,
    pub max_replay_entries: usize,
    pub max_replay_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionPolicy {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub ttl: Duration,
    pub lease: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateErrorKind {
    Database,
    Conflict,
    Limit,
    Replay,
}

#[derive(Debug)]
pub struct StateError {
    kind: StateErrorKind,
    message: &'static str,
}

impl StateError {
    pub(crate) const fn database(message: &'static str) -> Self {
        Self {
            kind: StateErrorKind::Database,
            message,
        }
    }

    pub(crate) const fn conflict(message: &'static str) -> Self {
        Self {
            kind: StateErrorKind::Conflict,
            message,
        }
    }

    pub(crate) const fn limit(message: &'static str) -> Self {
        Self {
            kind: StateErrorKind::Limit,
            message,
        }
    }

    pub(crate) const fn replay(message: &'static str) -> Self {
        Self {
            kind: StateErrorKind::Replay,
            message,
        }
    }

    pub const fn kind(&self) -> StateErrorKind {
        self.kind
    }

    pub const fn code(&self) -> &'static str {
        match self.kind {
            StateErrorKind::Database => "K5201",
            StateErrorKind::Conflict | StateErrorKind::Limit => "K5202",
            StateErrorKind::Replay => "K5203",
        }
    }

    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for StateError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mutation {
    Put {
        key: String,
        value: Vec<u8>,
    },
    Delete {
        key: String,
    },
    CheckpointPut {
        name: String,
        value: Vec<u8>,
    },
    ObjectPut {
        bucket: String,
        key: String,
        value: Vec<u8>,
    },
    ObjectDelete {
        bucket: String,
        key: String,
    },
    QueuePublish {
        queue: String,
        id: [u8; 16],
        body: Vec<u8>,
    },
}

impl Mutation {
    /// Whether this mutation participates in the store's revision guard.
    ///
    /// Key/value, checkpoint, and object writes are read-modify-write against
    /// an invocation snapshot. Queue publications are append-only and ordered
    /// by `meta.sequence`, so they neither read nor advance the revision.
    pub const fn advances_revision(&self) -> bool {
        match self {
            Self::Put { .. }
            | Self::Delete { .. }
            | Self::CheckpointPut { .. }
            | Self::ObjectPut { .. }
            | Self::ObjectDelete { .. } => true,
            Self::QueuePublish { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayKind {
    Http,
    Ai,
}

impl ReplayKind {
    const fn as_i64(self) -> i64 {
        match self {
            Self::Http => 1,
            Self::Ai => 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayLease {
    artifact: Vec<u8>,
    kind: ReplayKind,
    operation: String,
    input_digest: Vec<u8>,
    owner: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplayDecision {
    Execute(ReplayLease),
    Replay(Vec<u8>),
    Conflict,
    InProgress,
}

#[derive(Clone, Copy, Debug)]
pub struct ReplayRequest<'a> {
    pub artifact: &'a [u8],
    pub kind: ReplayKind,
    pub operation: &'a str,
    pub input_digest: &'a [u8],
    pub owner: &'a [u8],
    pub now_millis: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyLease {
    artifact: Vec<u8>,
    key: String,
    request_digest: Vec<u8>,
    owner: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyDecision {
    Execute(IdempotencyLease),
    Replay(Vec<u8>),
    Conflict,
    InProgress,
}

pub struct DurableStore {
    path: PathBuf,
    limits: StoreLimits,
    connection: Mutex<Connection>,
}

impl fmt::Debug for DurableStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableStore")
            .field("path", &self.path)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DurableStore {
    pub fn open(
        path: &Path,
        durability: Durability,
        limits: StoreLimits,
    ) -> Result<Self, StateError> {
        validate_limits(limits)?;
        let path = resolve_database_path(path)?;
        if path
            .metadata()
            .map(|metadata| metadata.len() > limits.max_database_bytes)
            .unwrap_or(false)
        {
            return Err(StateError::limit("durable database exceeds its byte limit"));
        }
        let mut connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(map_database)?;
        connection
            .busy_timeout(limits.busy_timeout)
            .map_err(map_database)?;
        configure_connection_safety(&connection)?;
        // Install the page ceiling before any schema work so initialization and
        // migration cannot grow an oversized database first and fail afterwards.
        apply_page_limit(&connection, limits.max_database_bytes)?;
        schema::initialize_or_validate(&mut connection)?;
        configure_persistence(&connection, durability, limits.max_database_bytes)?;
        verify_database_budget(&connection, limits.max_database_bytes)?;
        Ok(Self {
            path,
            limits,
            connection: Mutex::new(connection),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn limits(&self) -> StoreLimits {
        self.limits
    }

    pub fn revision(&self) -> Result<u64, StateError> {
        let connection = self.lock()?;
        query_revision(&connection)
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StateError> {
        validate_key(key, self.limits)?;
        let connection = self.lock()?;
        connection
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(map_database)
    }

    pub fn get_at_revision(
        &self,
        key: &str,
        expected_revision: u64,
    ) -> Result<Option<Vec<u8>>, StateError> {
        validate_key(key, self.limits)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_transaction)?;
        if query_revision(&transaction)? != expected_revision {
            return Err(StateError::conflict(
                "durable state revision changed before read",
            ));
        }
        let value = transaction
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(map_database)?;
        transaction.commit().map_err(map_database)?;
        Ok(value)
    }

    pub fn checkpoint(&self, name: &str) -> Result<Option<Vec<u8>>, StateError> {
        validate_key(name, self.limits)?;
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT value FROM checkpoints WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_database)
    }

    pub fn checkpoint_at_revision(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<Option<Vec<u8>>, StateError> {
        validate_key(name, self.limits)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(map_transaction)?;
        if query_revision(&transaction)? != expected_revision {
            return Err(StateError::conflict(
                "durable state revision changed before checkpoint read",
            ));
        }
        let value = transaction
            .query_row(
                "SELECT value FROM checkpoints WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_database)?;
        transaction.commit().map_err(map_database)?;
        Ok(value)
    }

    pub fn replay_decision(
        &self,
        request: ReplayRequest<'_>,
        policy: RetentionPolicy,
    ) -> Result<ReplayDecision, StateError> {
        let ReplayRequest {
            artifact,
            kind,
            operation,
            input_digest,
            owner,
            now_millis,
        } = request;
        validate_retention(policy, self.limits)?;
        validate_identity(operation, self.limits.max_key_bytes)?;
        validate_digest(artifact)?;
        validate_digest(input_digest)?;
        validate_owner(owner)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        cleanup_replay(&transaction, now_millis)?;
        let row = transaction
            .query_row(
                "SELECT input_digest, status, owner, lease_until, result
                 FROM replay
                 WHERE artifact = ?1 AND kind = ?2 AND operation = ?3",
                params![artifact, kind.as_i64(), operation],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(map_database)?;
        if let Some((recorded_digest, status, _recorded_owner, lease_until, result)) = row {
            if recorded_digest != input_digest {
                transaction.commit().map_err(map_database)?;
                return Ok(ReplayDecision::Conflict);
            }
            if status == 1 {
                let sequence = next_sequence(&transaction)?;
                transaction
                    .execute(
                        "UPDATE replay SET last_used = ?1
                         WHERE artifact = ?2 AND kind = ?3 AND operation = ?4",
                        params![sequence, artifact, kind.as_i64(), operation],
                    )
                    .map_err(map_database)?;
                transaction.commit().map_err(map_database)?;
                return result
                    .map(ReplayDecision::Replay)
                    .ok_or_else(|| StateError::database("completed replay record has no result"));
            }
            if lease_until.is_some_and(|lease| lease > now_millis) {
                transaction.commit().map_err(map_database)?;
                return Ok(ReplayDecision::InProgress);
            }
            transaction
                .execute(
                    "UPDATE replay
                     SET owner = ?1, lease_until = ?2, result = NULL,
                         expires_at = NULL, size_bytes = 0
                     WHERE artifact = ?3 AND kind = ?4 AND operation = ?5",
                    params![
                        owner,
                        checked_deadline(now_millis, policy.lease)?,
                        artifact,
                        kind.as_i64(),
                        operation
                    ],
                )
                .map_err(map_database)?;
        } else {
            let sequence = next_sequence(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO replay(
                        artifact, kind, operation, input_digest, status, owner,
                        lease_until, result, expires_at, last_used, size_bytes
                     ) VALUES(?1, ?2, ?3, ?4, 0, ?5, ?6, NULL, NULL, ?7, 0)",
                    params![
                        artifact,
                        kind.as_i64(),
                        operation,
                        input_digest,
                        owner,
                        checked_deadline(now_millis, policy.lease)?,
                        sequence
                    ],
                )
                .map_err(map_database)?;
        }
        transaction.commit().map_err(map_database)?;
        Ok(ReplayDecision::Execute(ReplayLease {
            artifact: artifact.to_vec(),
            kind,
            operation: operation.to_owned(),
            input_digest: input_digest.to_vec(),
            owner: owner.to_vec(),
        }))
    }

    pub fn complete_replay(
        &self,
        lease: &ReplayLease,
        result: &[u8],
        now_millis: i64,
        policy: RetentionPolicy,
    ) -> Result<(), StateError> {
        validate_retention(policy, self.limits)?;
        if result.len() > self.limits.max_replay_bytes || result.len() > policy.max_bytes {
            return Err(StateError::limit(
                "replay result exceeds the store byte limit",
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        let sequence = next_sequence(&transaction)?;
        let changed = transaction
            .execute(
                "UPDATE replay
                 SET status = 1, owner = NULL, lease_until = NULL, result = ?1,
                     expires_at = ?2, last_used = ?3, size_bytes = ?4
                 WHERE artifact = ?5 AND kind = ?6 AND operation = ?7
                   AND input_digest = ?8 AND status = 0 AND owner = ?9",
                params![
                    result,
                    checked_deadline(now_millis, policy.ttl)?,
                    sequence,
                    result.len() as i64,
                    lease.artifact,
                    lease.kind.as_i64(),
                    lease.operation,
                    lease.input_digest,
                    lease.owner
                ],
            )
            .map_err(map_database)?;
        if changed != 1 {
            return Err(StateError::replay("replay lease is stale or not owned"));
        }
        enforce_replay_limits(&transaction, policy)?;
        transaction.commit().map_err(map_database)
    }

    pub fn abort_replay(&self, lease: &ReplayLease) -> Result<(), StateError> {
        let connection = self.lock()?;
        connection
            .execute(
                "DELETE FROM replay
                 WHERE artifact = ?1 AND kind = ?2 AND operation = ?3
                   AND input_digest = ?4 AND status = 0 AND owner = ?5",
                params![
                    lease.artifact,
                    lease.kind.as_i64(),
                    lease.operation,
                    lease.input_digest,
                    lease.owner
                ],
            )
            .map_err(map_database)?;
        Ok(())
    }

    pub fn idempotency_decision(
        &self,
        artifact: &[u8],
        key: &str,
        request_digest: &[u8],
        owner: &[u8],
        now_millis: i64,
        policy: RetentionPolicy,
    ) -> Result<IdempotencyDecision, StateError> {
        validate_retention(policy, self.limits)?;
        validate_identity(key, self.limits.max_key_bytes)?;
        validate_digest(artifact)?;
        validate_digest(request_digest)?;
        validate_owner(owner)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        cleanup_idempotency(&transaction, now_millis)?;
        let row = transaction
            .query_row(
                "SELECT request_digest, status, lease_until, response
                 FROM idempotency WHERE artifact = ?1 AND request_key = ?2",
                params![artifact, key],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_database)?;
        if let Some((recorded_digest, status, lease_until, response)) = row {
            if recorded_digest != request_digest {
                transaction.commit().map_err(map_database)?;
                return Ok(IdempotencyDecision::Conflict);
            }
            if status == 1 {
                let sequence = next_sequence(&transaction)?;
                transaction
                    .execute(
                        "UPDATE idempotency SET last_used = ?1
                         WHERE artifact = ?2 AND request_key = ?3",
                        params![sequence, artifact, key],
                    )
                    .map_err(map_database)?;
                transaction.commit().map_err(map_database)?;
                return response.map(IdempotencyDecision::Replay).ok_or_else(|| {
                    StateError::database("completed idempotency record has no response")
                });
            }
            if lease_until.is_some_and(|lease| lease > now_millis) {
                transaction.commit().map_err(map_database)?;
                return Ok(IdempotencyDecision::InProgress);
            }
            transaction
                .execute(
                    "UPDATE idempotency
                     SET owner = ?1, lease_until = ?2, response = NULL,
                         expires_at = NULL, size_bytes = 0
                     WHERE artifact = ?3 AND request_key = ?4",
                    params![
                        owner,
                        checked_deadline(now_millis, policy.lease)?,
                        artifact,
                        key
                    ],
                )
                .map_err(map_database)?;
        } else {
            let sequence = next_sequence(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO idempotency(
                        artifact, request_key, request_digest, status, owner,
                        lease_until, response, expires_at, last_used, size_bytes
                     ) VALUES(?1, ?2, ?3, 0, ?4, ?5, NULL, NULL, ?6, 0)",
                    params![
                        artifact,
                        key,
                        request_digest,
                        owner,
                        checked_deadline(now_millis, policy.lease)?,
                        sequence
                    ],
                )
                .map_err(map_database)?;
        }
        transaction.commit().map_err(map_database)?;
        Ok(IdempotencyDecision::Execute(IdempotencyLease {
            artifact: artifact.to_vec(),
            key: key.to_owned(),
            request_digest: request_digest.to_vec(),
            owner: owner.to_vec(),
        }))
    }

    pub fn complete_idempotency(
        &self,
        lease: &IdempotencyLease,
        response: &[u8],
        now_millis: i64,
        policy: RetentionPolicy,
    ) -> Result<(), StateError> {
        validate_retention(policy, self.limits)?;
        if response.len() > policy.max_bytes {
            return Err(StateError::limit(
                "idempotency response exceeds the retention byte limit",
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_transaction)?;
        let sequence = next_sequence(&transaction)?;
        let changed = transaction
            .execute(
                "UPDATE idempotency
                 SET status = 1, owner = NULL, lease_until = NULL, response = ?1,
                     expires_at = ?2, last_used = ?3, size_bytes = ?4
                 WHERE artifact = ?5 AND request_key = ?6
                   AND request_digest = ?7 AND status = 0 AND owner = ?8",
                params![
                    response,
                    checked_deadline(now_millis, policy.ttl)?,
                    sequence,
                    response.len() as i64,
                    lease.artifact,
                    lease.key,
                    lease.request_digest,
                    lease.owner
                ],
            )
            .map_err(map_database)?;
        if changed != 1 {
            return Err(StateError::replay(
                "idempotency lease is stale or not owned",
            ));
        }
        enforce_idempotency_limits(&transaction, policy)?;
        transaction.commit().map_err(map_database)
    }

    pub fn abort_idempotency(&self, lease: &IdempotencyLease) -> Result<(), StateError> {
        let connection = self.lock()?;
        connection
            .execute(
                "DELETE FROM idempotency
                 WHERE artifact = ?1 AND request_key = ?2
                   AND request_digest = ?3 AND status = 0 AND owner = ?4",
                params![lease.artifact, lease.key, lease.request_digest, lease.owner],
            )
            .map_err(map_database)?;
        Ok(())
    }

    pub fn replay_counts(&self) -> Result<(usize, usize), StateError> {
        let connection = self.lock()?;
        let entries = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)
                 FROM replay WHERE status = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(map_database)?;
        Ok((entries.0 as usize, entries.1 as usize))
    }

    pub fn idempotency_counts(&self) -> Result<(usize, usize), StateError> {
        let connection = self.lock()?;
        let entries = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)
                 FROM idempotency WHERE status = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(map_database)?;
        Ok((entries.0 as usize, entries.1 as usize))
    }

    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StateError> {
        self.connection
            .lock()
            .map_err(|_| StateError::database("durable database lock is unavailable"))
    }
}

fn configure_connection_safety(connection: &Connection) -> Result<(), StateError> {
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(map_database)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA trusted_schema = OFF;
             PRAGMA recursive_triggers = OFF;",
        )
        .map_err(map_database)
}

fn configure_persistence(
    connection: &Connection,
    durability: Durability,
    max_database_bytes: u64,
) -> Result<(), StateError> {
    connection
        .execute_batch("PRAGMA journal_mode = WAL;")
        .map_err(map_database)?;
    connection
        .pragma_update(
            None,
            "synchronous",
            match durability {
                Durability::Full => "FULL",
                Durability::Normal => "NORMAL",
            },
        )
        .map_err(map_database)?;
    apply_page_limit(connection, max_database_bytes)
}

fn page_size(connection: &Connection) -> Result<u64, StateError> {
    let page_size: i64 = connection
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(map_database)?;
    u64::try_from(page_size).map_err(|_| StateError::database("SQLite page size is invalid"))
}

fn apply_page_limit(connection: &Connection, max_database_bytes: u64) -> Result<(), StateError> {
    let max_pages = database_page_limit(max_database_bytes, page_size(connection)?)?;
    connection
        .pragma_update(None, "max_page_count", max_pages)
        .map_err(map_database)?;
    Ok(())
}

/// Confirms that the materialized schema and the effective page ceiling both
/// stay inside the configured byte budget.
fn verify_database_budget(
    connection: &Connection,
    max_database_bytes: u64,
) -> Result<(), StateError> {
    let page_size = page_size(connection)?;
    let effective_pages: i64 = connection
        .pragma_query_value(None, "max_page_count", |row| row.get(0))
        .map_err(map_database)?;
    let current_pages: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .map_err(map_database)?;
    for pages in [effective_pages, current_pages] {
        let pages = u64::try_from(pages)
            .map_err(|_| StateError::database("SQLite page count is invalid"))?;
        if pages
            .checked_mul(page_size)
            .is_none_or(|bytes| bytes > max_database_bytes)
        {
            return Err(StateError::limit(
                "durable database exceeds its configured byte limit",
            ));
        }
    }
    Ok(())
}

fn database_page_limit(max_database_bytes: u64, page_size: u64) -> Result<i64, StateError> {
    let max_pages = max_database_bytes
        .checked_div(page_size)
        .filter(|pages| *pages > 0)
        .ok_or_else(|| StateError::limit("database byte limit is smaller than one SQLite page"))?;
    i64::try_from(max_pages).map_err(|_| StateError::limit("database page limit exceeds SQLite"))
}

fn resolve_database_path(path: &Path) -> Result<PathBuf, StateError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(StateError::database(
                "durable database path must not be a symbolic link",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => {
            return Err(StateError::database(
                "durable database path could not be inspected",
            ));
        }
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| StateError::database("durable database path has no file name"))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|_| StateError::database("durable database parent is not accessible"))?;
    Ok(parent.join(file_name))
}

pub(crate) fn query_revision(connection: &Connection) -> Result<u64, StateError> {
    let revision: i64 = connection
        .query_row("SELECT revision FROM meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .map_err(map_database)?;
    u64::try_from(revision).map_err(|_| StateError::database("durable state revision is invalid"))
}

pub(crate) fn next_sequence(transaction: &rusqlite::Transaction<'_>) -> Result<i64, StateError> {
    transaction
        .execute("UPDATE meta SET sequence = sequence + 1 WHERE id = 1", [])
        .map_err(map_database)?;
    transaction
        .query_row("SELECT sequence FROM meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .map_err(map_database)
}

fn cleanup_replay(
    transaction: &rusqlite::Transaction<'_>,
    now_millis: i64,
) -> Result<(), StateError> {
    transaction
        .execute(
            "DELETE FROM replay
             WHERE (status = 1 AND expires_at <= ?1)
                OR (status = 0 AND lease_until <= ?1)",
            [now_millis],
        )
        .map_err(map_database)?;
    Ok(())
}

fn cleanup_idempotency(
    transaction: &rusqlite::Transaction<'_>,
    now_millis: i64,
) -> Result<(), StateError> {
    transaction
        .execute(
            "DELETE FROM idempotency
             WHERE (status = 1 AND expires_at <= ?1)
                OR (status = 0 AND lease_until <= ?1)",
            [now_millis],
        )
        .map_err(map_database)?;
    Ok(())
}

fn enforce_replay_limits(
    transaction: &rusqlite::Transaction<'_>,
    policy: RetentionPolicy,
) -> Result<(), StateError> {
    enforce_table_limits(transaction, "replay", policy)
}

fn enforce_idempotency_limits(
    transaction: &rusqlite::Transaction<'_>,
    policy: RetentionPolicy,
) -> Result<(), StateError> {
    enforce_table_limits(transaction, "idempotency", policy)
}

fn enforce_table_limits(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    policy: RetentionPolicy,
) -> Result<(), StateError> {
    loop {
        let query = format!(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)
             FROM {table} WHERE status = 1"
        );
        let (entries, bytes): (i64, i64) = transaction
            .query_row(&query, [], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(map_database)?;
        if entries as usize <= policy.max_entries && bytes as usize <= policy.max_bytes {
            return Ok(());
        }
        let delete = format!(
            "DELETE FROM {table}
             WHERE rowid = (
                SELECT rowid FROM {table}
                WHERE status = 1 ORDER BY last_used ASC LIMIT 1
             )"
        );
        if transaction.execute(&delete, []).map_err(map_database)? != 1 {
            return Err(StateError::database(
                "durable retention cleanup could not evict a record",
            ));
        }
    }
}

fn validate_limits(limits: StoreLimits) -> Result<(), StateError> {
    if limits.busy_timeout.is_zero()
        || limits.max_operations == 0
        || limits.max_key_bytes == 0
        || limits.max_value_bytes == 0
        || limits.max_transaction_bytes == 0
        || limits.max_database_bytes < MINIMUM_DATABASE_BYTES
        || limits.max_replay_entries == 0
        || limits.max_replay_bytes == 0
    {
        return Err(StateError::limit("durable store limits are invalid"));
    }
    Ok(())
}

pub(crate) fn validate_key(key: &str, limits: StoreLimits) -> Result<(), StateError> {
    if key.is_empty() || key.len() > limits.max_key_bytes || key.contains('\0') {
        return Err(StateError::limit(
            "durable state key is invalid or too large",
        ));
    }
    Ok(())
}

pub(crate) fn validate_identity(value: &str, max_bytes: usize) -> Result<(), StateError> {
    if value.is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(StateError::limit(
            "durable record identity is invalid or too large",
        ));
    }
    Ok(())
}

fn validate_digest(digest: &[u8]) -> Result<(), StateError> {
    if digest.len() != 32 {
        return Err(StateError::replay("durable record digest must be 32 bytes"));
    }
    Ok(())
}

fn validate_owner(owner: &[u8]) -> Result<(), StateError> {
    if owner.len() != 16 {
        return Err(StateError::replay("durable lease owner must be 16 bytes"));
    }
    Ok(())
}

pub(crate) fn validate_mutations(
    mutations: &[Mutation],
    limits: StoreLimits,
) -> Result<(), StateError> {
    if mutations.len() > limits.max_operations {
        return Err(StateError::limit(
            "durable state operation count exceeds its limit",
        ));
    }
    let mut total = 0usize;
    for mutation in mutations {
        match mutation {
            Mutation::Put { key, value } => {
                validate_key(key, limits)?;
                validate_value(value, limits)?;
                total = total
                    .checked_add(key.len())
                    .and_then(|total| total.checked_add(value.len()))
                    .ok_or_else(|| {
                        StateError::limit("durable transaction byte count overflowed")
                    })?;
            }
            Mutation::Delete { key } => {
                validate_key(key, limits)?;
                total = total.checked_add(key.len()).ok_or_else(|| {
                    StateError::limit("durable transaction byte count overflowed")
                })?;
            }
            Mutation::CheckpointPut { name, value } => {
                validate_key(name, limits)?;
                validate_value(value, limits)?;
                total = total
                    .checked_add(name.len())
                    .and_then(|total| total.checked_add(value.len()))
                    .ok_or_else(|| {
                        StateError::limit("durable transaction byte count overflowed")
                    })?;
            }
            Mutation::ObjectPut { bucket, key, value } => {
                validate_identity(bucket, MAX_RESOURCE_NAME_BYTES)?;
                validate_key(key, limits)?;
                total = total
                    .checked_add(bucket.len())
                    .and_then(|total| total.checked_add(key.len()))
                    .and_then(|total| total.checked_add(value.len()))
                    .ok_or_else(|| {
                        StateError::limit("durable transaction byte count overflowed")
                    })?;
            }
            Mutation::ObjectDelete { bucket, key } => {
                validate_identity(bucket, MAX_RESOURCE_NAME_BYTES)?;
                validate_key(key, limits)?;
                total = total
                    .checked_add(bucket.len())
                    .and_then(|total| total.checked_add(key.len()))
                    .ok_or_else(|| {
                        StateError::limit("durable transaction byte count overflowed")
                    })?;
            }
            Mutation::QueuePublish { queue, body, .. } => {
                validate_identity(queue, MAX_RESOURCE_NAME_BYTES)?;
                total = total
                    .checked_add(queue.len())
                    .and_then(|total| total.checked_add(body.len()))
                    .ok_or_else(|| {
                        StateError::limit("durable transaction byte count overflowed")
                    })?;
            }
        }
    }
    if total > limits.max_transaction_bytes {
        return Err(StateError::limit(
            "durable transaction bytes exceed their limit",
        ));
    }
    Ok(())
}

pub(crate) fn validate_value(value: &[u8], limits: StoreLimits) -> Result<(), StateError> {
    if value.len() > limits.max_value_bytes {
        return Err(StateError::limit("durable state value exceeds its limit"));
    }
    Ok(())
}

fn validate_retention(policy: RetentionPolicy, limits: StoreLimits) -> Result<(), StateError> {
    if policy.max_entries == 0
        || policy.max_bytes == 0
        || policy.max_entries > limits.max_replay_entries
        || policy.max_bytes > limits.max_replay_bytes
        || policy.ttl.is_zero()
        || policy.lease.is_zero()
    {
        return Err(StateError::limit("durable retention policy is invalid"));
    }
    Ok(())
}

pub(crate) fn checked_deadline(now_millis: i64, duration: Duration) -> Result<i64, StateError> {
    let millis = i64::try_from(duration.as_millis())
        .map_err(|_| StateError::limit("durable deadline exceeds i64"))?;
    now_millis
        .checked_add(millis)
        .ok_or_else(|| StateError::limit("durable deadline overflowed"))
}

pub(crate) fn map_transaction(error: rusqlite::Error) -> StateError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
            StateError::conflict("durable database is busy")
        }
        _ => map_database(error),
    }
}

pub(crate) fn map_database(error: rusqlite::Error) -> StateError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DiskFull | ErrorCode::TooBig) => {
            StateError::limit("durable database limit was exceeded")
        }
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
            StateError::conflict("durable database is busy")
        }
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => {
            StateError::database("durable database is corrupt")
        }
        _ => StateError::database("durable database operation failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_page_limit_never_exceeds_the_byte_budget() {
        let pages = database_page_limit(20_000_000, 4096).unwrap();

        assert_eq!(pages, 4882);
        assert!(u64::try_from(pages).unwrap() * 4096 <= 20_000_000);
        assert!(database_page_limit(4095, 4096).is_err());
    }
}
