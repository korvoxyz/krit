use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    APPLICATION_ID, STORE_SCHEMA_VERSION, StateError, StateErrorKind, map_database, map_transaction,
};

/// `(name, declared SQL)` for one strict table.
type TableDefinition = (&'static str, &'static str);
/// `(index, table, ordered columns, declared SQL)` for one strict index.
type IndexDefinition = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
);
/// `(column, declared type, not null, primary-key position)`.
type ColumnDefinition = (&'static str, &'static str, i64, i64);
/// `(table, ordered columns)` for one strict table.
type TableColumns = (&'static str, &'static [ColumnDefinition]);

const SCHEMA_1_TABLES: &[TableDefinition] = &[
    (
        "meta",
        "CREATE TABLE meta(
            id INTEGER PRIMARY KEY CHECK(id = 1),
            revision INTEGER NOT NULL CHECK(revision >= 0),
            sequence INTEGER NOT NULL CHECK(sequence >= 0)
         ) STRICT",
    ),
    (
        "kv",
        "CREATE TABLE kv(
            key TEXT PRIMARY KEY,
            value BLOB NOT NULL
         ) STRICT",
    ),
    (
        "checkpoints",
        "CREATE TABLE checkpoints(
            name TEXT PRIMARY KEY,
            value BLOB NOT NULL
         ) STRICT",
    ),
    (
        "replay",
        "CREATE TABLE replay(
            artifact BLOB NOT NULL,
            kind INTEGER NOT NULL,
            operation TEXT NOT NULL,
            input_digest BLOB NOT NULL,
            status INTEGER NOT NULL CHECK(status IN (0, 1)),
            owner BLOB,
            lease_until INTEGER,
            result BLOB,
            expires_at INTEGER,
            last_used INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            PRIMARY KEY(artifact, kind, operation)
         ) STRICT",
    ),
    (
        "idempotency",
        "CREATE TABLE idempotency(
            artifact BLOB NOT NULL,
            request_key TEXT NOT NULL,
            request_digest BLOB NOT NULL,
            status INTEGER NOT NULL CHECK(status IN (0, 1)),
            owner BLOB,
            lease_until INTEGER,
            response BLOB,
            expires_at INTEGER,
            last_used INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            PRIMARY KEY(artifact, request_key)
         ) STRICT",
    ),
];

const SCHEMA_1_INDEXES: &[IndexDefinition] = &[
    (
        "replay_cleanup",
        "replay",
        &["status", "expires_at", "last_used"],
        "CREATE INDEX replay_cleanup ON replay(status, expires_at, last_used)",
    ),
    (
        "idempotency_cleanup",
        "idempotency",
        &["status", "expires_at", "last_used"],
        "CREATE INDEX idempotency_cleanup ON idempotency(status, expires_at, last_used)",
    ),
];

const SCHEMA_2_TABLES: &[TableDefinition] = &[
    (
        "queue_jobs",
        "CREATE TABLE queue_jobs(
            id BLOB NOT NULL PRIMARY KEY,
            queue TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            body BLOB NOT NULL,
            attempts INTEGER NOT NULL CHECK(attempts >= 0),
            visible_at INTEGER NOT NULL,
            lease_until INTEGER,
            owner BLOB,
            size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
            enqueued_at INTEGER NOT NULL
         ) STRICT",
    ),
    (
        "queue_dead",
        "CREATE TABLE queue_dead(
            id BLOB NOT NULL PRIMARY KEY,
            queue TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            body BLOB NOT NULL,
            attempts INTEGER NOT NULL CHECK(attempts >= 0),
            reason TEXT NOT NULL,
            size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
            failed_at INTEGER NOT NULL
         ) STRICT",
    ),
    (
        "schedule_fires",
        "CREATE TABLE schedule_fires(
            schedule TEXT NOT NULL,
            due_at INTEGER NOT NULL,
            status INTEGER NOT NULL CHECK(status IN (0, 1, 2)),
            attempts INTEGER NOT NULL CHECK(attempts >= 0),
            visible_at INTEGER NOT NULL,
            lease_until INTEGER,
            owner BLOB,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(schedule, due_at)
         ) STRICT",
    ),
    (
        "schedule_cursors",
        "CREATE TABLE schedule_cursors(
            schedule TEXT NOT NULL PRIMARY KEY,
            last_due_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
         ) STRICT",
    ),
    (
        "objects",
        "CREATE TABLE objects(
            bucket TEXT NOT NULL,
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(bucket, key)
         ) STRICT",
    ),
];

const SCHEMA_2_INDEXES: &[IndexDefinition] = &[
    (
        "queue_jobs_ready",
        "queue_jobs",
        &["queue", "visible_at", "sequence"],
        "CREATE INDEX queue_jobs_ready ON queue_jobs(queue, visible_at, sequence)",
    ),
    (
        "queue_dead_cleanup",
        "queue_dead",
        &["queue", "failed_at", "sequence"],
        "CREATE INDEX queue_dead_cleanup ON queue_dead(queue, failed_at, sequence)",
    ),
    (
        "schedule_fires_ready",
        "schedule_fires",
        &["status", "visible_at", "schedule", "due_at"],
        "CREATE INDEX schedule_fires_ready
         ON schedule_fires(status, visible_at, schedule, due_at)",
    ),
];

const SCHEMA_1_COLUMNS: &[TableColumns] = &[
    (
        "meta",
        &[
            ("id", "INTEGER", 0, 1),
            ("revision", "INTEGER", 1, 0),
            ("sequence", "INTEGER", 1, 0),
        ],
    ),
    ("kv", &[("key", "TEXT", 1, 1), ("value", "BLOB", 1, 0)]),
    (
        "checkpoints",
        &[("name", "TEXT", 1, 1), ("value", "BLOB", 1, 0)],
    ),
    (
        "replay",
        &[
            ("artifact", "BLOB", 1, 1),
            ("kind", "INTEGER", 1, 2),
            ("operation", "TEXT", 1, 3),
            ("input_digest", "BLOB", 1, 0),
            ("status", "INTEGER", 1, 0),
            ("owner", "BLOB", 0, 0),
            ("lease_until", "INTEGER", 0, 0),
            ("result", "BLOB", 0, 0),
            ("expires_at", "INTEGER", 0, 0),
            ("last_used", "INTEGER", 1, 0),
            ("size_bytes", "INTEGER", 1, 0),
        ],
    ),
    (
        "idempotency",
        &[
            ("artifact", "BLOB", 1, 1),
            ("request_key", "TEXT", 1, 2),
            ("request_digest", "BLOB", 1, 0),
            ("status", "INTEGER", 1, 0),
            ("owner", "BLOB", 0, 0),
            ("lease_until", "INTEGER", 0, 0),
            ("response", "BLOB", 0, 0),
            ("expires_at", "INTEGER", 0, 0),
            ("last_used", "INTEGER", 1, 0),
            ("size_bytes", "INTEGER", 1, 0),
        ],
    ),
];

const SCHEMA_2_COLUMNS: &[TableColumns] = &[
    (
        "queue_jobs",
        &[
            ("id", "BLOB", 1, 1),
            ("queue", "TEXT", 1, 0),
            ("sequence", "INTEGER", 1, 0),
            ("body", "BLOB", 1, 0),
            ("attempts", "INTEGER", 1, 0),
            ("visible_at", "INTEGER", 1, 0),
            ("lease_until", "INTEGER", 0, 0),
            ("owner", "BLOB", 0, 0),
            ("size_bytes", "INTEGER", 1, 0),
            ("enqueued_at", "INTEGER", 1, 0),
        ],
    ),
    (
        "queue_dead",
        &[
            ("id", "BLOB", 1, 1),
            ("queue", "TEXT", 1, 0),
            ("sequence", "INTEGER", 1, 0),
            ("body", "BLOB", 1, 0),
            ("attempts", "INTEGER", 1, 0),
            ("reason", "TEXT", 1, 0),
            ("size_bytes", "INTEGER", 1, 0),
            ("failed_at", "INTEGER", 1, 0),
        ],
    ),
    (
        "schedule_fires",
        &[
            ("schedule", "TEXT", 1, 1),
            ("due_at", "INTEGER", 1, 2),
            ("status", "INTEGER", 1, 0),
            ("attempts", "INTEGER", 1, 0),
            ("visible_at", "INTEGER", 1, 0),
            ("lease_until", "INTEGER", 0, 0),
            ("owner", "BLOB", 0, 0),
            ("updated_at", "INTEGER", 1, 0),
        ],
    ),
    (
        "schedule_cursors",
        &[
            ("schedule", "TEXT", 1, 1),
            ("last_due_at", "INTEGER", 1, 0),
            ("updated_at", "INTEGER", 1, 0),
        ],
    ),
    (
        "objects",
        &[
            ("bucket", "TEXT", 1, 1),
            ("key", "TEXT", 1, 2),
            ("value", "BLOB", 1, 0),
            ("size_bytes", "INTEGER", 1, 0),
            ("updated_at", "INTEGER", 1, 0),
        ],
    ),
];

/// Sorted table names of the schema-1 store.
const SCHEMA_1_TABLE_NAMES: &[&str] = &["checkpoints", "idempotency", "kv", "meta", "replay"];

/// Sorted explicit index names of the schema-1 store.
const SCHEMA_1_INDEX_NAMES: &[&str] = &["idempotency_cleanup", "replay_cleanup"];

/// Sorted table names of the current strict schema.
const EXPECTED_TABLES: &[&str] = &[
    "checkpoints",
    "idempotency",
    "kv",
    "meta",
    "objects",
    "queue_dead",
    "queue_jobs",
    "replay",
    "schedule_cursors",
    "schedule_fires",
];

/// Sorted explicit index names of the current strict schema.
const EXPECTED_INDEXES: &[&str] = &[
    "idempotency_cleanup",
    "queue_dead_cleanup",
    "queue_jobs_ready",
    "replay_cleanup",
    "schedule_fires_ready",
];

fn definition_batch(tables: &[TableDefinition], indexes: &[IndexDefinition]) -> String {
    let mut batch = String::new();
    for (_, sql) in tables {
        batch.push_str(sql);
        batch.push_str(";\n");
    }
    for (_, _, _, sql) in indexes {
        batch.push_str(sql);
        batch.push_str(";\n");
    }
    batch
}

/// Bounded retries for a concurrent first open or migration.
///
/// Initialization and migration are idempotent, so a busy exclusive
/// transaction means another opener is finishing the same work.
const MAX_SCHEMA_ATTEMPTS: usize = 4;

pub(crate) fn initialize_or_validate(connection: &mut Connection) -> Result<(), StateError> {
    let integrity: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(map_database)?;
    if integrity != "ok" {
        return Err(StateError::database(
            "durable database failed its integrity check",
        ));
    }
    for attempt in 0..MAX_SCHEMA_ATTEMPTS {
        let (application_id, version) = read_identity(connection)?;
        if application_id != 0 && application_id != APPLICATION_ID {
            return Err(StateError::database(
                "database is not a Krit durable-state store",
            ));
        }
        if version > STORE_SCHEMA_VERSION {
            return Err(StateError::database(
                "durable database schema is newer than this runtime",
            ));
        }
        if version == STORE_SCHEMA_VERSION {
            if application_id != APPLICATION_ID {
                return Err(StateError::database(
                    "durable database identity or schema is invalid",
                ));
            }
            return validate_schema(connection);
        }
        let outcome = if version == 0 {
            initialize(connection)
        } else {
            migrate(connection, version)
        };
        match outcome {
            Ok(()) => return validate_schema(connection),
            Err(error)
                if error.kind() == StateErrorKind::Conflict
                    && attempt + 1 < MAX_SCHEMA_ATTEMPTS => {}
            Err(error) => return Err(error),
        }
    }
    Err(StateError::conflict(
        "durable database stayed busy during initialization",
    ))
}

fn read_identity(connection: &mut Connection) -> Result<(i64, i64), StateError> {
    let preflight = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_transaction)?;
    let application_id: i64 = preflight
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(map_database)?;
    let version: i64 = preflight
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_database)?;
    preflight.commit().map_err(map_database)?;
    Ok((application_id, version))
}

fn initialize(connection: &mut Connection) -> Result<(), StateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(map_transaction)?;
    let locked_application_id: i64 = transaction
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(map_database)?;
    let locked_version: i64 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_database)?;
    if locked_application_id == APPLICATION_ID && locked_version == STORE_SCHEMA_VERSION {
        transaction.commit().map_err(map_database)?;
        return Ok(());
    }
    if locked_application_id != 0 || locked_version != 0 {
        return Err(StateError::database(
            "durable database identity changed during initialization",
        ));
    }
    let object_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(map_database)?;
    if object_count != 0 {
        return Err(StateError::database(
            "unversioned durable database is not empty",
        ));
    }
    transaction
        .execute_batch(&definition_batch(SCHEMA_1_TABLES, SCHEMA_1_INDEXES))
        .map_err(map_database)?;
    transaction
        .execute(
            "INSERT INTO meta(id, revision, sequence) VALUES(1, 0, 0)",
            [],
        )
        .map_err(map_database)?;
    transaction
        .execute_batch(&definition_batch(SCHEMA_2_TABLES, SCHEMA_2_INDEXES))
        .map_err(map_database)?;
    transaction
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(map_database)?;
    transaction
        .pragma_update(None, "user_version", STORE_SCHEMA_VERSION)
        .map_err(map_database)?;
    transaction.commit().map_err(map_database)
}

/// Deterministically migrates an existing store forward without discarding data.
///
/// Every step runs inside one exclusive transaction; a failed step leaves the
/// previous schema version untouched.
fn migrate(connection: &mut Connection, from: i64) -> Result<(), StateError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(map_transaction)?;
    let locked_application_id: i64 = transaction
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(map_database)?;
    let locked_version: i64 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_database)?;
    if locked_application_id != APPLICATION_ID {
        return Err(StateError::database(
            "durable database identity or schema is invalid",
        ));
    }
    if locked_version == STORE_SCHEMA_VERSION {
        transaction.commit().map_err(map_database)?;
        return Ok(());
    }
    if locked_version != from || !(1..=STORE_SCHEMA_VERSION).contains(&locked_version) {
        return Err(StateError::database(
            "durable database schema changed during migration",
        ));
    }
    // Reject a foreign, partially migrated, or extended schema-1 store before
    // emitting any DDL, so a rejected open leaves the database untouched.
    validate_schema_one(&transaction)?;
    if locked_version == 1 {
        transaction
            .execute_batch(&definition_batch(SCHEMA_2_TABLES, SCHEMA_2_INDEXES))
            .map_err(map_database)?;
    }
    transaction
        .pragma_update(None, "user_version", STORE_SCHEMA_VERSION)
        .map_err(map_database)?;
    validate_schema(&transaction)?;
    transaction.commit().map_err(map_database)
}

/// Exact schema-1 validation used as the migration precondition.
fn validate_schema_one(connection: &Connection) -> Result<(), StateError> {
    validate_object_set(connection, SCHEMA_1_TABLE_NAMES, SCHEMA_1_INDEX_NAMES)?;
    validate_definitions(connection, SCHEMA_1_TABLES, SCHEMA_1_INDEXES)?;
    validate_columns(connection, SCHEMA_1_COLUMNS)?;
    validate_meta_row(connection)
}

/// Rejects triggers, views, and any table or index outside the expected set.
fn validate_object_set(
    connection: &Connection,
    tables: &[&str],
    indexes: &[&str],
) -> Result<(), StateError> {
    let extra_objects: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type IN ('trigger', 'view')",
            [],
            |row| row.get(0),
        )
        .map_err(map_database)?;
    if extra_objects != 0 {
        return Err(StateError::database(
            "durable database contains unsupported schema objects",
        ));
    }
    let mut table_statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(map_database)?;
    let actual_tables = table_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_database)?;
    if actual_tables != tables {
        return Err(StateError::database(
            "durable database table set is invalid",
        ));
    }
    let mut index_statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'index' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(map_database)?;
    let actual_indexes = index_statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_database)?;
    if actual_indexes != indexes {
        return Err(StateError::database(
            "durable database index set is invalid",
        ));
    }
    Ok(())
}

fn validate_meta_row(connection: &Connection) -> Result<(), StateError> {
    let meta_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM meta
             WHERE id = 1 AND revision >= 0 AND sequence >= 0",
            [],
            |row| row.get(0),
        )
        .map_err(map_database)?;
    if meta_rows != 1 {
        return Err(StateError::database(
            "durable database metadata row is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn validate_schema(connection: &Connection) -> Result<(), StateError> {
    validate_object_set(connection, EXPECTED_TABLES, EXPECTED_INDEXES)?;
    validate_definitions(connection, SCHEMA_1_TABLES, SCHEMA_1_INDEXES)?;
    validate_definitions(connection, SCHEMA_2_TABLES, SCHEMA_2_INDEXES)?;
    validate_columns(connection, SCHEMA_1_COLUMNS)?;
    validate_columns(connection, SCHEMA_2_COLUMNS)?;
    validate_meta_row(connection)
}

fn validate_definitions(
    connection: &Connection,
    tables: &[TableDefinition],
    indexes: &[IndexDefinition],
) -> Result<(), StateError> {
    for (name, sql) in tables {
        validate_schema_sql(connection, "table", name, sql)?;
    }
    for (index, table, columns, sql) in indexes {
        validate_index(connection, table, index, columns)?;
        validate_schema_sql(connection, "index", index, sql)?;
    }
    Ok(())
}

fn validate_columns(
    connection: &Connection,
    definitions: &[TableColumns],
) -> Result<(), StateError> {
    for (table, columns) in definitions {
        validate_table(connection, table, columns)?;
    }
    Ok(())
}

fn validate_schema_sql(
    connection: &Connection,
    object_type: &str,
    name: &str,
    expected: &str,
) -> Result<(), StateError> {
    let actual: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = ?1 AND name = ?2",
            params![object_type, name],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_database)?;
    let Some(actual) = actual else {
        return Err(StateError::database(
            "durable database schema object is missing",
        ));
    };
    if normalize_schema_sql(&actual) != normalize_schema_sql(expected) {
        return Err(StateError::database(
            "durable database schema definition is invalid",
        ));
    }
    Ok(())
}

fn normalize_schema_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn validate_table(
    connection: &Connection,
    table: &str,
    expected: &[ColumnDefinition],
) -> Result<(), StateError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(map_database)?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(map_database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_database)?;
    if columns.len() != expected.len()
        || columns.iter().zip(expected).any(|(actual, expected)| {
            actual.0 != expected.0
                || actual.1 != expected.1
                || actual.2 != expected.2
                || actual.3 != expected.3
        })
    {
        return Err(StateError::database(
            "durable database table schema is invalid",
        ));
    }
    let flags: Option<(i64, i64)> = connection
        .query_row(
            "SELECT wr, strict FROM pragma_table_list
             WHERE schema = 'main' AND name = ?1",
            [table],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_database)?;
    if flags != Some((0, 1)) {
        return Err(StateError::database(
            "durable database table options are invalid",
        ));
    }
    Ok(())
}

fn validate_index(
    connection: &Connection,
    table: &str,
    index: &str,
    expected_columns: &[&str],
) -> Result<(), StateError> {
    let mut list = connection
        .prepare(&format!("PRAGMA index_list({table})"))
        .map_err(map_database)?;
    let attributes = list
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(map_database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_database)?;
    if !attributes
        .iter()
        .any(|value| value == &(index.to_owned(), 0, "c".to_owned(), 0))
    {
        return Err(StateError::database(
            "durable database required index is invalid",
        ));
    }
    let mut info = connection
        .prepare(&format!("PRAGMA index_info({index})"))
        .map_err(map_database)?;
    let columns = info
        .query_map([], |row| row.get::<_, String>(2))
        .map_err(map_database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_database)?;
    if !columns
        .iter()
        .map(String::as_str)
        .eq(expected_columns.iter().copied())
    {
        return Err(StateError::database(
            "durable database required index columns are invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_schema_names_stay_sorted_and_complete() {
        assert!(EXPECTED_TABLES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(EXPECTED_INDEXES.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            EXPECTED_TABLES.len(),
            SCHEMA_1_TABLES.len() + SCHEMA_2_TABLES.len()
        );
        assert_eq!(
            EXPECTED_INDEXES.len(),
            SCHEMA_1_INDEXES.len() + SCHEMA_2_INDEXES.len()
        );
        assert_eq!(SCHEMA_1_COLUMNS.len(), SCHEMA_1_TABLES.len());
        assert_eq!(SCHEMA_2_COLUMNS.len(), SCHEMA_2_TABLES.len());
        assert!(
            SCHEMA_1_TABLE_NAMES
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert!(
            SCHEMA_1_INDEX_NAMES
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert_eq!(SCHEMA_1_TABLE_NAMES.len(), SCHEMA_1_TABLES.len());
        assert_eq!(SCHEMA_1_INDEX_NAMES.len(), SCHEMA_1_INDEXES.len());
        assert!(
            SCHEMA_1_TABLE_NAMES
                .iter()
                .all(|name| EXPECTED_TABLES.contains(name))
        );
        assert!(
            SCHEMA_1_INDEX_NAMES
                .iter()
                .all(|name| EXPECTED_INDEXES.contains(name))
        );
    }
}
