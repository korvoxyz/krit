use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use krit_database::{
    Database, DatabaseErrorKind, DatabaseLimits, DatabaseMode, MINIMUM_DATABASE_BYTES,
    OperationBounds, ParameterType, StatementKind, StatementRequest, TransactionMode,
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("krit-database-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn database(&self) -> PathBuf {
        self.path.join("app.db")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn limits() -> DatabaseLimits {
    DatabaseLimits {
        busy_timeout: Duration::from_millis(100),
        max_database_bytes: 4 * 1024 * 1024,
        max_transaction_duration: Duration::from_millis(500),
        max_operations_per_transaction: 8,
        max_parameter_bytes: 128,
        max_rows: 4,
        max_columns: 4,
        max_result_bytes: 1024,
    }
}

fn seed(path: &PathBuf) {
    let connection = rusqlite::Connection::open(path).expect("fixture should open");
    connection
        .execute_batch(
            "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             INSERT INTO users(id, name) VALUES(1, 'alice'), (2, 'bob');",
        )
        .expect("fixture schema should apply");
    drop(connection);
}

fn statement(
    kind: StatementKind,
    sql: &str,
    parameters: &[ParameterType],
    columns: &[&str],
) -> StatementRequest {
    StatementRequest {
        kind,
        sql: sql.to_owned(),
        parameters: parameters.to_vec(),
        columns: columns.iter().map(|column| (*column).to_owned()).collect(),
    }
}

fn catalog() -> BTreeMap<String, StatementRequest> {
    BTreeMap::from([
        (
            "find-user".to_owned(),
            statement(
                StatementKind::Query,
                "SELECT id, name FROM users WHERE name = ?1",
                &[ParameterType::Text],
                &["id", "name"],
            ),
        ),
        (
            "count-users".to_owned(),
            statement(
                StatementKind::Query,
                "SELECT COUNT(*) AS total FROM users",
                &[],
                &["total"],
            ),
        ),
        (
            "insert-user".to_owned(),
            statement(
                StatementKind::Execute,
                "INSERT INTO users(id, name) VALUES(?1, ?2)",
                &[ParameterType::Integer, ParameterType::Text],
                &[],
            ),
        ),
    ])
}

fn open(directory: &TestDirectory, mode: DatabaseMode) -> Database {
    Database::open("app", &directory.database(), mode, limits(), catalog())
        .expect("database should open")
}

fn prepared(directory: &TestDirectory) -> Database {
    seed(&directory.database());
    open(directory, DatabaseMode::ReadWrite)
}

/// Generous setup bounds; deadline behaviour has dedicated tests.
fn bounds() -> OperationBounds {
    OperationBounds::unbounded_for_setup(Duration::from_secs(30))
}

#[test]
fn explicit_transactions_query_execute_and_commit() {
    let directory = TestDirectory::new("lifecycle");
    let database = prepared(&directory);

    let mut transaction = database
        .begin(TransactionMode::Write, &bounds())
        .expect("write transaction should begin");
    let rows = database
        .query(
            &mut transaction,
            "find-user",
            &["alice".to_owned()],
            &bounds(),
        )
        .expect("query should run");
    assert_eq!(
        rows,
        "{\"columns\":[\"id\",\"name\"],\"rows\":[[1,\"alice\"]]}"
    );
    let changed = database
        .execute(
            &mut transaction,
            "insert-user",
            &["3".to_owned(), "carol".to_owned()],
            &bounds(),
        )
        .expect("execute should run");
    assert_eq!(changed, 1);
    database
        .commit(&mut transaction, &bounds())
        .expect("commit should succeed");
    assert!(transaction.is_completed());

    // The commit is durable for a completely new connection.
    let database = open(&directory, DatabaseMode::ReadWrite);
    let mut reader = database
        .begin(TransactionMode::Read, &bounds())
        .expect("read transaction should begin");
    assert_eq!(
        database
            .query(&mut reader, "count-users", &[], &bounds())
            .expect("count should run"),
        "{\"columns\":[\"total\"],\"rows\":[[3]]}"
    );
    database
        .commit(&mut reader, &bounds())
        .expect("commit should succeed");
}

#[test]
fn rollback_discards_every_mutation() {
    let directory = TestDirectory::new("rollback");
    let database = prepared(&directory);

    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();
    database
        .execute(
            &mut transaction,
            "insert-user",
            &["9".to_owned(), "mallory".to_owned()],
            &bounds(),
        )
        .expect("execute should run");
    database
        .rollback(&mut transaction)
        .expect("rollback should succeed");

    let mut reader = database.begin(TransactionMode::Read, &bounds()).unwrap();
    assert_eq!(
        database
            .query(&mut reader, "count-users", &[], &bounds())
            .unwrap(),
        "{\"columns\":[\"total\"],\"rows\":[[2]]}"
    );
    database.commit(&mut reader, &bounds()).unwrap();
}

#[test]
fn completed_handles_reject_every_further_operation() {
    let directory = TestDirectory::new("handle-reuse");
    let database = prepared(&directory);

    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();
    database.commit(&mut transaction, &bounds()).unwrap();

    for error in [
        database
            .query(&mut transaction, "count-users", &[], &bounds())
            .expect_err("query after commit must fail"),
        database
            .execute(
                &mut transaction,
                "insert-user",
                &["4".to_owned(), "dave".to_owned()],
                &bounds(),
            )
            .expect_err("execute after commit must fail"),
    ] {
        assert_eq!(error.kind(), DatabaseErrorKind::Transaction);
    }
    assert_eq!(
        database
            .commit(&mut transaction, &bounds())
            .expect_err("double commit must fail")
            .kind(),
        DatabaseErrorKind::Transaction
    );
    assert_eq!(
        database
            .rollback(&mut transaction)
            .expect_err("rollback after commit must fail")
            .kind(),
        DatabaseErrorKind::Transaction
    );
}

#[test]
fn read_transactions_and_read_only_databases_refuse_mutation() {
    let directory = TestDirectory::new("read-only");
    let database = prepared(&directory);

    let mut reader = database.begin(TransactionMode::Read, &bounds()).unwrap();
    assert_eq!(
        database
            .execute(
                &mut reader,
                "insert-user",
                &["5".to_owned(), "erin".to_owned()],
                &bounds(),
            )
            .expect_err("a read transaction must not mutate")
            .kind(),
        DatabaseErrorKind::Transaction
    );
    database.rollback(&mut reader).unwrap();
    drop(database);

    let query_only = BTreeMap::from([(
        "count-users".to_owned(),
        statement(
            StatementKind::Query,
            "SELECT COUNT(*) AS total FROM users",
            &[],
            &["total"],
        ),
    )]);
    let read_only = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadOnly,
        limits(),
        query_only,
    )
    .expect("a query-only read-only database should open");
    assert_eq!(
        read_only
            .begin(TransactionMode::Write, &bounds())
            .expect_err("a read-only database must refuse write transactions")
            .kind(),
        DatabaseErrorKind::Transaction
    );
    let mut reader = read_only.begin(TransactionMode::Read, &bounds()).unwrap();
    assert!(
        read_only
            .query(&mut reader, "count-users", &[], &bounds())
            .is_ok()
    );
    read_only.commit(&mut reader, &bounds()).unwrap();

    // A read-only database may not even declare a mutating statement.
    assert_eq!(
        Database::open(
            "app",
            &directory.database(),
            DatabaseMode::ReadOnly,
            limits(),
            catalog(),
        )
        .expect_err("a mutating catalog entry must be refused")
        .kind(),
        DatabaseErrorKind::Catalog
    );
}

#[test]
fn parameters_are_bound_never_interpolated() {
    let directory = TestDirectory::new("injection");
    let database = prepared(&directory);

    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();
    let payload = "alice'); DROP TABLE users; --";
    let rows = database
        .query(
            &mut transaction,
            "find-user",
            &[payload.to_owned()],
            &bounds(),
        )
        .expect("an injection payload must be ordinary data");
    assert_eq!(rows, "{\"columns\":[\"id\",\"name\"],\"rows\":[]}");
    database
        .execute(
            &mut transaction,
            "insert-user",
            &["7".to_owned(), payload.to_owned()],
            &bounds(),
        )
        .expect("the payload should be stored literally");
    let stored = database
        .query(
            &mut transaction,
            "find-user",
            &[payload.to_owned()],
            &bounds(),
        )
        .expect("the stored payload should be found by equality");
    assert!(stored.contains("DROP TABLE users"));
    database.commit(&mut transaction, &bounds()).unwrap();

    let connection = rusqlite::Connection::open(directory.database()).unwrap();
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .expect("the table must still exist");
    assert_eq!(total, 3);
}

#[test]
fn parameter_counts_types_and_bytes_are_bounded() {
    let directory = TestDirectory::new("parameters");
    let database = prepared(&directory);
    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();

    for (parameters, expected) in [
        (vec![], DatabaseErrorKind::Limit),
        (
            vec!["alice".to_owned(), "extra".to_owned()],
            DatabaseErrorKind::Limit,
        ),
    ] {
        assert_eq!(
            database
                .query(&mut transaction, "find-user", &parameters, &bounds())
                .expect_err("parameter count must match the catalog")
                .kind(),
            expected
        );
    }
    assert_eq!(
        database
            .execute(
                &mut transaction,
                "insert-user",
                &["not-an-integer".to_owned(), "x".to_owned()],
                &bounds(),
            )
            .expect_err("a declared integer must parse")
            .kind(),
        DatabaseErrorKind::Limit
    );
    assert_eq!(
        database
            .query(
                &mut transaction,
                "find-user",
                &["x".repeat(4096)],
                &bounds()
            )
            .expect_err("parameter bytes must be bounded")
            .kind(),
        DatabaseErrorKind::Limit
    );
    assert_eq!(
        database
            .query(
                &mut transaction,
                "find-user",
                &["a\0b".to_owned()],
                &bounds()
            )
            .expect_err("a NUL byte must be refused")
            .kind(),
        DatabaseErrorKind::Limit
    );
    database.rollback(&mut transaction).unwrap();
}

#[test]
fn unknown_statements_and_mismatched_kinds_fail_closed() {
    let directory = TestDirectory::new("statements");
    let database = prepared(&directory);
    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();

    assert_eq!(
        database
            .query(&mut transaction, "absent", &[], &bounds())
            .expect_err("an uncatalogued statement must fail")
            .kind(),
        DatabaseErrorKind::Catalog
    );
    assert_eq!(
        database
            .query(
                &mut transaction,
                "insert-user",
                &["1".to_owned(), "x".to_owned()],
                &bounds()
            )
            .expect_err("an execute statement must not run as a query")
            .kind(),
        DatabaseErrorKind::Catalog
    );
    assert_eq!(
        database
            .execute(&mut transaction, "count-users", &[], &bounds())
            .expect_err("a query statement must not run as an execute")
            .kind(),
        DatabaseErrorKind::Catalog
    );
    database.rollback(&mut transaction).unwrap();
}

#[test]
fn row_column_and_result_bounds_are_enforced() {
    let directory = TestDirectory::new("results");
    seed(&directory.database());
    let connection = rusqlite::Connection::open(directory.database()).unwrap();
    for index in 3..12 {
        connection
            .execute(
                "INSERT INTO users(id, name) VALUES(?1, ?2)",
                rusqlite::params![index, format!("user-{index}")],
            )
            .unwrap();
    }
    drop(connection);

    let database = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadOnly,
        limits(),
        BTreeMap::from([(
            "all-users".to_owned(),
            statement(
                StatementKind::Query,
                "SELECT id, name FROM users ORDER BY id",
                &[],
                &["id", "name"],
            ),
        )]),
    )
    .expect("database should open");
    let mut transaction = database.begin(TransactionMode::Read, &bounds()).unwrap();
    assert_eq!(
        database
            .query(&mut transaction, "all-users", &[], &bounds())
            .expect_err("the row bound must be enforced")
            .kind(),
        DatabaseErrorKind::Limit
    );
    database.rollback(&mut transaction).unwrap();
}

#[test]
fn operation_count_per_transaction_is_bounded() {
    let directory = TestDirectory::new("operations");
    let database = prepared(&directory);
    let mut transaction = database.begin(TransactionMode::Read, &bounds()).unwrap();

    for _ in 0..limits().max_operations_per_transaction {
        database
            .query(&mut transaction, "count-users", &[], &bounds())
            .expect("operations within the bound should run");
    }
    assert_eq!(
        database
            .query(&mut transaction, "count-users", &[], &bounds())
            .expect_err("the operation bound must be enforced")
            .kind(),
        DatabaseErrorKind::Limit
    );
    database.rollback(&mut transaction).unwrap();
}

#[test]
fn catalog_validation_rejects_dangerous_and_malformed_sql() {
    let directory = TestDirectory::new("catalog-guard");
    seed(&directory.database());

    let cases: [(StatementKind, &str, Vec<ParameterType>, Vec<&str>); 12] = [
        (
            StatementKind::Query,
            "PRAGMA journal_mode = WAL",
            vec![],
            vec![],
        ),
        (
            StatementKind::Execute,
            "ATTACH DATABASE 'x' AS y",
            vec![],
            vec![],
        ),
        (StatementKind::Execute, "DROP TABLE users", vec![], vec![]),
        (StatementKind::Execute, "CREATE TABLE t(a)", vec![], vec![]),
        (StatementKind::Execute, "VACUUM", vec![], vec![]),
        (StatementKind::Execute, "BEGIN", vec![], vec![]),
        (
            StatementKind::Execute,
            "INSERT INTO users(id, name) VALUES(1, 'x'); DROP TABLE users",
            vec![],
            vec![],
        ),
        // read/write classification
        (
            StatementKind::Query,
            "INSERT INTO users(id, name) VALUES(9, 'x')",
            vec![],
            vec![],
        ),
        (
            StatementKind::Execute,
            "SELECT COUNT(*) AS total FROM users",
            vec![],
            vec!["total"],
        ),
        // placeholder and column contracts
        (
            StatementKind::Query,
            "SELECT id FROM users WHERE name = ?1",
            vec![],
            vec!["id"],
        ),
        (
            StatementKind::Query,
            "SELECT id FROM users WHERE name = :name",
            vec![ParameterType::Text],
            vec!["id"],
        ),
        (
            StatementKind::Query,
            "SELECT id, name FROM users",
            vec![],
            vec!["id"],
        ),
    ];

    for (kind, sql, parameters, columns) in cases {
        let error = Database::open(
            "app",
            &directory.database(),
            DatabaseMode::ReadWrite,
            limits(),
            BTreeMap::from([(
                "entry".to_owned(),
                statement(kind, sql, &parameters, &columns),
            )]),
        )
        .expect_err(&format!("`{sql}` must be rejected"));
        assert_eq!(error.kind(), DatabaseErrorKind::Catalog, "sql: {sql}");
    }

    // A read-only database may not declare a mutating statement at all.
    let error = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadOnly,
        limits(),
        catalog(),
    )
    .expect_err("a read-only database must refuse mutating catalog entries");
    assert_eq!(error.kind(), DatabaseErrorKind::Catalog);
}

#[test]
fn missing_corrupt_and_symlinked_files_fail_closed_without_reset() {
    let directory = TestDirectory::new("files");

    let missing = Database::open(
        "app",
        &directory.path.join("absent.db"),
        DatabaseMode::ReadWrite,
        limits(),
        catalog(),
    )
    .expect_err("an absent database must be refused");
    assert_eq!(missing.kind(), DatabaseErrorKind::Connection);
    assert!(
        !directory.path.join("absent.db").exists(),
        "Krit must never create an application database"
    );

    let corrupt = directory.path.join("corrupt.db");
    fs::write(&corrupt, b"not a sqlite database at all").unwrap();
    let before = fs::read(&corrupt).unwrap();
    let error = Database::open(
        "app",
        &corrupt,
        DatabaseMode::ReadWrite,
        limits(),
        catalog(),
    )
    .expect_err("a corrupt database must be refused");
    assert_eq!(error.kind(), DatabaseErrorKind::Connection);
    assert_eq!(
        fs::read(&corrupt).unwrap(),
        before,
        "a refused open must not rewrite the file"
    );

    #[cfg(unix)]
    {
        seed(&directory.database());
        let link = directory.path.join("linked.db");
        std::os::unix::fs::symlink(directory.database(), &link).unwrap();
        assert_eq!(
            Database::open("app", &link, DatabaseMode::ReadWrite, limits(), catalog())
                .expect_err("a symlinked database must be refused")
                .kind(),
            DatabaseErrorKind::Connection
        );
    }
}

#[test]
fn database_budgets_and_empty_catalogs_are_rejected() {
    let directory = TestDirectory::new("budgets");
    seed(&directory.database());

    assert_eq!(
        Database::open(
            "app",
            &directory.database(),
            DatabaseMode::ReadWrite,
            DatabaseLimits {
                max_database_bytes: MINIMUM_DATABASE_BYTES - 1,
                ..limits()
            },
            catalog(),
        )
        .expect_err("a sub-minimum budget must be refused")
        .kind(),
        DatabaseErrorKind::Catalog
    );
    assert_eq!(
        Database::open(
            "app",
            &directory.database(),
            DatabaseMode::ReadWrite,
            limits(),
            BTreeMap::new(),
        )
        .expect_err("an empty catalog must be refused")
        .kind(),
        DatabaseErrorKind::Catalog
    );
    assert_eq!(
        Database::open(
            "app",
            &directory.database(),
            DatabaseMode::ReadWrite,
            DatabaseLimits {
                max_transaction_duration: Duration::from_secs(600),
                ..limits()
            },
            catalog(),
        )
        .expect_err("an unbounded transaction window must be refused")
        .kind(),
        DatabaseErrorKind::Catalog
    );
}

#[test]
fn concurrent_writers_serialize_and_surface_busy_conflicts() {
    let directory = TestDirectory::new("concurrent");
    seed(&directory.database());
    let first = Arc::new(open(&directory, DatabaseMode::ReadWrite));
    let second = Arc::new(open(&directory, DatabaseMode::ReadWrite));

    let mut holder = first.begin(TransactionMode::Write, &bounds()).unwrap();
    // A second immediate writer cannot enter while the first holds the lock.
    let error = second
        .begin(TransactionMode::Write, &bounds())
        .expect_err("a competing writer must not enter the transaction");
    assert_eq!(error.kind(), DatabaseErrorKind::Conflict);
    first.commit(&mut holder, &bounds()).unwrap();

    // After the first writer commits, the second succeeds.
    let mut later = second
        .begin(TransactionMode::Write, &bounds())
        .expect("the lock should be released");
    second.commit(&mut later, &bounds()).unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let handles = [Arc::clone(&first), Arc::clone(&second)]
        .into_iter()
        .enumerate()
        .map(|(index, database)| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let mut transaction = database.begin(TransactionMode::Write, &bounds()).ok()?;
                let outcome = database.execute(
                    &mut transaction,
                    "insert-user",
                    &[format!("{}", 20 + index), format!("user-{index}")],
                    &bounds(),
                );
                let committed =
                    outcome.is_ok() && database.commit(&mut transaction, &bounds()).is_ok();
                if !committed {
                    let _ = database.abandon(&mut transaction);
                }
                committed.then_some(index)
            })
        })
        .collect::<Vec<_>>();
    let winners = handles
        .into_iter()
        .filter_map(|handle| handle.join().expect("writer should finish"))
        .count();
    assert!(
        winners >= 1,
        "at least one concurrent writer must make progress"
    );
    let connection = rusqlite::Connection::open(directory.database()).unwrap();
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 2 + winners as i64);
}

#[test]
fn error_messages_never_disclose_sql_paths_or_values() {
    let directory = TestDirectory::new("privacy");
    let database = prepared(&directory);
    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();

    let secret_value = "super-secret-parameter";
    let error = database
        .query(
            &mut transaction,
            "find-user",
            &[secret_value.repeat(64)],
            &bounds(),
        )
        .expect_err("an oversized parameter must fail");
    let rendered = format!("{error} {:?}", error);
    assert!(!rendered.contains(secret_value));
    assert!(!rendered.contains("SELECT"));
    assert!(!rendered.contains("users"));
    assert!(!rendered.contains(&directory.path.to_string_lossy().to_string()));

    let catalog_error = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadWrite,
        limits(),
        BTreeMap::from([(
            "bad".to_owned(),
            statement(
                StatementKind::Query,
                "SELECT missing_column FROM users",
                &[],
                &[],
            ),
        )]),
    )
    .expect_err("an invalid catalog entry must fail");
    let rendered = format!("{catalog_error} {:?}", catalog_error);
    assert!(!rendered.contains("missing_column"));
    assert!(!rendered.contains("SELECT"));
    database.rollback(&mut transaction).unwrap();
}

/// Bounds that report cancellation on demand.
fn cancellable(cancelled: &Arc<std::sync::atomic::AtomicBool>) -> OperationBounds {
    let flag = Arc::clone(cancelled);
    OperationBounds::new(
        std::time::Instant::now() + Duration::from_secs(30),
        Arc::new(move || flag.load(Ordering::SeqCst)),
    )
}

fn recursive_statements() -> BTreeMap<String, StatementRequest> {
    BTreeMap::from([
        (
            "runaway".to_owned(),
            StatementRequest {
                kind: StatementKind::Query,
                // An unbounded recursive CTE that yields a single aggregate
                // row: the row and byte bounds can never fire, so only an
                // interrupt can stop it.
                sql: "SELECT COUNT(*) AS n FROM \
                      (WITH RECURSIVE spin(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM spin) \
                       SELECT x FROM spin)"
                    .to_owned(),
                parameters: Vec::new(),
                columns: vec!["n".to_owned()],
            },
        ),
        (
            "runaway-write".to_owned(),
            StatementRequest {
                kind: StatementKind::Execute,
                sql: "INSERT INTO users(id, name) \
                      SELECT n, 'x' FROM (WITH RECURSIVE spin(n) AS \
                      (SELECT 100 UNION ALL SELECT n + 1 FROM spin) SELECT n FROM spin)"
                    .to_owned(),
                parameters: Vec::new(),
                columns: Vec::new(),
            },
        ),
        (
            "count-users".to_owned(),
            StatementRequest {
                kind: StatementKind::Query,
                sql: "SELECT COUNT(*) AS total FROM users".to_owned(),
                parameters: Vec::new(),
                columns: vec!["total".to_owned()],
            },
        ),
    ])
}

fn recursive_database(directory: &TestDirectory, transaction_bound: Duration) -> Database {
    seed(&directory.database());
    let mut limits = limits();
    limits.max_transaction_duration = transaction_bound;
    Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadWrite,
        limits,
        recursive_statements(),
    )
    .expect("database should open")
}

#[test]
fn a_runaway_query_is_interrupted_at_the_transaction_deadline() {
    let directory = TestDirectory::new("interrupt-query");
    let database = recursive_database(&directory, Duration::from_millis(150));
    let mut transaction = database.begin(TransactionMode::Read, &bounds()).unwrap();

    let started = std::time::Instant::now();
    let error = database
        .query(&mut transaction, "runaway", &[], &bounds())
        .expect_err("an unbounded recursive query must be interrupted");
    let elapsed = started.elapsed();

    assert_eq!(error.kind(), DatabaseErrorKind::Interrupted);
    assert_eq!(error.code(), "K5303");
    assert!(
        elapsed < Duration::from_secs(5),
        "interruption took {elapsed:?}"
    );
    // The connection is usable again immediately: no transaction was left open.
    assert!(database.is_idle().unwrap());
    assert!(!database.is_poisoned());
    let mut next = database.begin(TransactionMode::Read, &bounds()).unwrap();
    assert!(
        database
            .query(&mut next, "count-users", &[], &bounds())
            .is_ok()
    );
    database.commit(&mut next, &bounds()).unwrap();
}

#[test]
fn a_runaway_mutation_is_interrupted_and_rolls_back() {
    let directory = TestDirectory::new("interrupt-execute");
    let database = recursive_database(&directory, Duration::from_millis(150));
    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();

    let error = database
        .execute(&mut transaction, "runaway-write", &[], &bounds())
        .expect_err("an unbounded recursive mutation must be interrupted");

    assert_eq!(error.kind(), DatabaseErrorKind::Interrupted);
    assert!(database.is_idle().unwrap());

    // Nothing the interrupted statement inserted survived.
    let mut reader = database.begin(TransactionMode::Read, &bounds()).unwrap();
    let rows = database
        .query(&mut reader, "count-users", &[], &bounds())
        .unwrap();
    database.commit(&mut reader, &bounds()).unwrap();
    assert_eq!(rows, "{\"columns\":[\"total\"],\"rows\":[[2]]}");
}

#[test]
fn cancellation_interrupts_an_operation_already_inside_sqlite() {
    let directory = TestDirectory::new("interrupt-cancel");
    let database = recursive_database(&directory, Duration::from_secs(4));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bounds = cancellable(&cancelled);
    let mut transaction = database.begin(TransactionMode::Read, &bounds).unwrap();

    let flag = Arc::clone(&cancelled);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(120));
        flag.store(true, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let error = database
        .query(&mut transaction, "runaway", &[], &bounds)
        .expect_err("cancellation must interrupt the running statement");

    assert_eq!(error.kind(), DatabaseErrorKind::Interrupted);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancellation was not observed promptly"
    );
    assert!(database.is_idle().unwrap());
}

#[test]
fn an_already_cancelled_invocation_never_enters_sqlite() {
    let directory = TestDirectory::new("interrupt-precancel");
    let database = recursive_database(&directory, Duration::from_secs(4));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let error = database
        .begin(TransactionMode::Read, &cancellable(&cancelled))
        .expect_err("a cancelled invocation must not begin a transaction");

    assert_eq!(error.kind(), DatabaseErrorKind::Interrupted);
    assert!(database.is_idle().unwrap());
}

#[test]
fn write_ahead_logging_databases_are_refused() {
    let directory = TestDirectory::new("wal");
    seed(&directory.database());
    let connection = rusqlite::Connection::open(directory.database()).unwrap();
    let mode: String = connection
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    drop(connection);

    let error = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadWrite,
        limits(),
        catalog(),
    )
    .expect_err("a WAL database has an unbounded on-disk footprint");

    assert_eq!(error.kind(), DatabaseErrorKind::Connection);
    assert!(error.message().contains("write-ahead logging"));
}

#[test]
fn repeated_writes_and_a_pinned_reader_stay_inside_the_disk_budget() {
    let directory = TestDirectory::new("disk-budget");
    seed(&directory.database());
    let mut limits = limits();
    limits.max_database_bytes = MINIMUM_DATABASE_BYTES;
    limits.max_operations_per_transaction = 64;
    // Short busy waits keep the whole run inside the reader's own time bound.
    limits.busy_timeout = Duration::from_millis(5);
    let database = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadWrite,
        limits,
        catalog(),
    )
    .expect("database should open");

    // A reader is pinned open for the whole run; under write-ahead logging this
    // is exactly the shape that lets a `-wal` sidecar grow without bound. With
    // a rollback journal the reader instead blocks writers, which is bounded by
    // the busy timeout and cannot grow the on-disk footprint.
    let mut reader_limits = limits;
    reader_limits.max_transaction_duration = Duration::from_secs(4);
    let reader = Database::open(
        "reader",
        &directory.database(),
        DatabaseMode::ReadOnly,
        reader_limits,
        BTreeMap::from([(
            "count-users".to_owned(),
            StatementRequest {
                kind: StatementKind::Query,
                sql: "SELECT COUNT(*) AS total FROM users".to_owned(),
                parameters: Vec::new(),
                columns: vec!["total".to_owned()],
            },
        )]),
    )
    .expect("reader should open");
    let mut pinned = reader.begin(TransactionMode::Read, &bounds()).unwrap();
    reader
        .query(&mut pinned, "count-users", &[], &bounds())
        .unwrap();

    let mut blocked = 0;
    for round in 0..20 {
        // While a reader is pinned, a write either blocks out to a bounded
        // busy conflict - at begin, at execute, or at commit - or succeeds.
        // Either way the disk contract holds.
        let attempt =
            database
                .begin(TransactionMode::Write, &bounds())
                .and_then(|mut transaction| {
                    for index in 0..8 {
                        database.execute(
                            &mut transaction,
                            "insert-user",
                            &[
                                (round * 8 + index + 100).to_string(),
                                format!("user-{round}-{index}"),
                            ],
                            &bounds(),
                        )?;
                    }
                    database.commit(&mut transaction, &bounds())
                });
        if let Err(error) = attempt {
            assert_eq!(
                error.kind(),
                DatabaseErrorKind::Conflict,
                "unexpected failure: {}",
                error.message()
            );
            blocked += 1;
        }
        assert!(
            database.disk_bytes() <= limits.max_database_bytes,
            "round {round} grew the on-disk footprint to {} bytes, past the {} byte contract",
            database.disk_bytes(),
            limits.max_database_bytes
        );
        assert!(
            database.is_idle().unwrap(),
            "round {round} leaked a transaction"
        );
    }
    assert!(
        blocked > 0,
        "a pinned reader must serialise writers rather than let a log grow"
    );

    reader.commit(&mut pinned, &bounds()).unwrap();
    assert!(database.disk_bytes() <= limits.max_database_bytes);

    // Once the reader releases, writes proceed again inside the same budget.
    let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();
    database
        .execute(
            &mut transaction,
            "insert-user",
            &[9001.to_string(), "after".to_owned()],
            &bounds(),
        )
        .expect("writes resume once the reader releases");
    database.commit(&mut transaction, &bounds()).unwrap();
    assert!(database.disk_bytes() <= limits.max_database_bytes);
}

#[test]
fn a_write_that_would_breach_the_disk_budget_fails_and_rolls_back() {
    let directory = TestDirectory::new("disk-overflow");
    seed(&directory.database());
    let mut limits = limits();
    // Just above the floor, so a handful of pages exhausts it.
    limits.max_database_bytes = MINIMUM_DATABASE_BYTES;
    limits.max_operations_per_transaction = 256;
    limits.max_parameter_bytes = 32 * 1024;
    let database = Database::open(
        "app",
        &directory.database(),
        DatabaseMode::ReadWrite,
        limits,
        catalog(),
    )
    .expect("database should open");

    let mut failed = false;
    for round in 0..64 {
        let mut transaction = database.begin(TransactionMode::Write, &bounds()).unwrap();
        let outcome = database.execute(
            &mut transaction,
            "insert-user",
            &[(round + 100).to_string(), "x".repeat(16 * 1024)],
            &bounds(),
        );
        if outcome.is_err() {
            failed = true;
            let _ = database.rollback(&mut transaction);
            break;
        }
        database.commit(&mut transaction, &bounds()).unwrap();
        assert!(database.disk_bytes() <= limits.max_database_bytes);
    }

    assert!(failed, "the byte budget was never enforced");
    assert!(database.disk_bytes() <= limits.max_database_bytes);
    assert!(database.is_idle().unwrap());
}
