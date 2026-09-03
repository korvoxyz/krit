use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, CancellationHandle, DatabaseCatalog, DatabaseDefinition,
    DatabaseLimits, DatabaseMode, DenyAllApprovalPolicy, DurableState, GrantSet, HostInputs,
    HttpRequest, NetworkPolicy, ParameterType, Runtime, SecretStore, StatementKind,
    StatementRequest,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "krit-runtime-database-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn database(&self) -> PathBuf {
        self.path.join("catalog.db")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn seed(path: &Path) {
    let connection = rusqlite::Connection::open(path).expect("fixture should open");
    connection
        .execute_batch(
            "CREATE TABLE visits(id INTEGER PRIMARY KEY AUTOINCREMENT, path TEXT NOT NULL);",
        )
        .expect("fixture schema should apply");
    drop(connection);
}

fn visit_count(path: &Path) -> i64 {
    let connection = rusqlite::Connection::open(path).expect("fixture should open");
    connection
        .query_row("SELECT COUNT(*) FROM visits", [], |row| row.get(0))
        .expect("count should read")
}

fn limits() -> DatabaseLimits {
    DatabaseLimits {
        busy_timeout: Duration::from_millis(100),
        max_database_bytes: 4 * 1024 * 1024,
        max_transaction_duration: Duration::from_millis(400),
        max_operations_per_transaction: 8,
        max_parameter_bytes: 1024,
        max_rows: 16,
        max_columns: 4,
        max_result_bytes: 4096,
    }
}

fn catalog_statements() -> BTreeMap<String, StatementRequest> {
    BTreeMap::from([
        (
            "record-visit".to_owned(),
            StatementRequest {
                kind: StatementKind::Execute,
                sql: "INSERT INTO visits(path) VALUES(?1)".to_owned(),
                parameters: vec![ParameterType::Text],
                columns: Vec::new(),
            },
        ),
        (
            "count-visits".to_owned(),
            StatementRequest {
                kind: StatementKind::Query,
                sql: "SELECT COUNT(*) AS total FROM visits".to_owned(),
                parameters: Vec::new(),
                columns: vec!["total".to_owned()],
            },
        ),
    ])
}

fn query_only_statements() -> BTreeMap<String, StatementRequest> {
    BTreeMap::from([(
        "count-visits".to_owned(),
        StatementRequest {
            kind: StatementKind::Query,
            sql: "SELECT COUNT(*) AS total FROM visits".to_owned(),
            parameters: Vec::new(),
            columns: vec!["total".to_owned()],
        },
    )])
}

fn catalog(directory: &TestDirectory, mode: DatabaseMode) -> DatabaseCatalog {
    let statements = match mode {
        // A read-only database may only expose query statements.
        DatabaseMode::ReadOnly => query_only_statements(),
        DatabaseMode::ReadWrite => catalog_statements(),
    };
    DatabaseCatalog::open(
        BTreeMap::from([(
            "catalog".to_owned(),
            DatabaseDefinition {
                path: directory.database(),
                mode,
                limits: limits(),
                statements,
            },
        )]),
        1,
    )
    .expect("database catalog should open")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/database"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
    ))
    .expect("manifest should parse")
}

fn compile(source_text: &str, effects: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/database", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("source should compile")
}

fn host(databases: DatabaseCatalog) -> AgentHost {
    AgentHost::new_with_resources(
        HostInputs::new(BTreeMap::new(), SecretStore::default())
            .expect("inputs should be valid")
            .with_network_policy(NetworkPolicy::loopback_for_tests()),
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        DurableState::default(),
        databases,
    )
    .expect("agent host should build")
}

fn request(path: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: path.to_owned(),
        query: String::new(),
        headers: Vec::new(),
        body: String::new(),
    }
}

const WRITE_SOURCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => match db_query(transaction, "count-visits", []) {
                Ok(rows) => match db_commit(transaction) {
                    Ok(committed) => record { status: 200, headers: [], body: rows },
                    Err(error) => record { status: 500, headers: [], body: error },
                },
                Err(error) => match db_rollback(transaction) {
                    Ok(undone) => record { status: 500, headers: [], body: error },
                    Err(fatal) => record { status: 500, headers: [], body: fatal },
                },
            },
            Err(error) => match db_rollback(transaction) {
                Ok(undone) => record { status: 500, headers: [], body: error },
                Err(fatal) => record { status: 500, headers: [], body: fatal },
            },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

const WRITE_CAPABILITIES: &str = "databases = [\"catalog\"]\n";

#[test]
fn explicit_transactions_commit_and_persist_across_runtimes() {
    let directory = TestDirectory::new("commit");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);
    let grants = GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect("write invocation should succeed");

    assert_eq!(result.response.status, 200);
    assert_eq!(
        result.response.body,
        "{\"columns\":[\"total\"],\"rows\":[[1]]}"
    );
    assert_eq!(result.stats.database_executes, 1);
    assert_eq!(result.stats.database_queries, 1);
    assert_eq!(result.stats.database_commits, 1);
    assert!(result.stats.database_write_committed);
    assert_eq!(visit_count(&directory.database()), 1);

    // A completely fresh runtime, host, and catalog observes the commit.
    let second = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect("second invocation should succeed");
    assert_eq!(
        second.response.body,
        "{\"columns\":[\"total\"],\"rows\":[[2]]}"
    );
}

#[test]
fn explicit_rollback_discards_the_mutation() {
    let directory = TestDirectory::new("rollback");
    seed(&directory.database());
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => match db_rollback(transaction) {
                Ok(undone) => record { status: 200, headers: [], body: request.path },
                Err(error) => record { status: 500, headers: [], body: error },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.write"],
    );

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect("rollback invocation should succeed");

    assert_eq!(result.response.status, 200);
    assert_eq!(result.stats.database_rollbacks, 1);
    assert!(!result.stats.database_write_committed);
    assert_eq!(visit_count(&directory.database()), 0);
}

#[test]
fn an_unclosed_transaction_rolls_back_and_fails_the_invocation() {
    let directory = TestDirectory::new("unclosed");
    seed(&directory.database());
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => record { status: 200, headers: [], body: request.path },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.write"],
    );

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect_err("an unclosed transaction must fail the invocation");

    assert_eq!(error.code(), "K5302");
    assert_eq!(
        visit_count(&directory.database()),
        0,
        "the abandoned transaction must roll back"
    );
}

#[test]
fn a_trapped_invocation_rolls_the_transaction_back() {
    let directory = TestDirectory::new("trap");
    seed(&directory.database());
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => record { status: 200 / (changed - 1), headers: [], body: request.path },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.write"],
    );

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect_err("a guest trap must fail the invocation");

    assert_eq!(error.code(), "K4004");
    assert_eq!(
        visit_count(&directory.database()),
        0,
        "a trapped invocation must not publish a mutation"
    );
}

#[test]
fn cancellation_before_execution_leaves_the_database_untouched() {
    let directory = TestDirectory::new("cancel");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);
    let cancellation = CancellationHandle::new();
    cancellation.cancel();

    let error = Runtime::default()
        .invoke_webhook_with_cancellation(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            &cancellation,
            request("/orders"),
        )
        .expect_err("a cancelled invocation must fail closed");

    assert_eq!(error.code(), "K5106");
    assert_eq!(visit_count(&directory.database()), 0);
}

#[test]
fn a_completed_handle_cannot_be_reused() {
    let directory = TestDirectory::new("reuse");
    seed(&directory.database());
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_commit(transaction) {
            Ok(committed) => match db_execute(transaction, "record-visit", [request.path]) {
                Ok(changed) => record { status: 200, headers: [], body: request.path },
                Err(error) => record { status: 409, headers: [], body: error },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.write"],
    );

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect_err("reusing a completed handle must fail closed");

    assert_eq!(error.code(), "K5302");
    assert_eq!(visit_count(&directory.database()), 0);
}

#[test]
fn a_second_transaction_is_refused_while_one_is_open() {
    let directory = TestDirectory::new("nested");
    seed(&directory.database());
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(first) => match db_begin_write("catalog") {
            Ok(second) => record { status: 200, headers: [], body: request.path },
            Err(error) => match db_rollback(first) {
                Ok(undone) => record { status: 409, headers: [], body: error },
                Err(fatal) => record { status: 500, headers: [], body: fatal },
            },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.write"],
    );

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect_err("a nested transaction must fail closed");

    assert_eq!(error.code(), "K5302");
}

#[test]
fn external_effects_are_refused_while_a_transaction_is_open() {
    let directory = TestDirectory::new("external");
    seed(&directory.database());
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_read("catalog") {
        Ok(transaction) => match http_request("https://api.example.com", request, None) {
            Ok(response) => record { status: 200, headers: [], body: response.body },
            Err(error) => match db_rollback(transaction) {
                Ok(undone) => record { status: 502, headers: [], body: error },
                Err(fatal) => record { status: 500, headers: [], body: fatal },
            },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.read", "http.request"],
    );

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(
                "readOnlyDatabases = [\"catalog\"]\nhttp = [\"https://api.example.com\"]\n",
            )),
            &host(catalog(&directory, DatabaseMode::ReadOnly)),
            request("/orders"),
        )
        .expect_err("an external call inside a transaction must fail closed");

    assert_eq!(error.code(), "K5302");
    assert!(error.message().contains("outbound HTTP"));
}

#[test]
fn read_only_grants_and_databases_refuse_write_transactions() {
    let directory = TestDirectory::new("read-only");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);
    let read_only_statements = query_only_statements();
    let read_only = DatabaseCatalog::open(
        BTreeMap::from([(
            "catalog".to_owned(),
            DatabaseDefinition {
                path: directory.database(),
                mode: DatabaseMode::ReadOnly,
                limits: limits(),
                statements: read_only_statements,
            },
        )]),
        1,
    )
    .expect("read-only catalog should open");

    // The artifact requires write authority the read-only database cannot give.
    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(read_only),
            request("/orders"),
        )
        .expect_err("write authority on a read-only database must fail closed");
    assert_eq!(error.code(), "K5001");

    // The manifest alone cannot grant write authority either.
    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest("readOnlyDatabases = [\"catalog\"]\n")),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect_err("an ungranted write must fail closed");
    assert_eq!(error.code(), "K5001");
}

#[test]
fn unconfigured_databases_and_statements_fail_closed() {
    let directory = TestDirectory::new("unconfigured");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(DatabaseCatalog::default()),
            request("/orders"),
        )
        .expect_err("an unconfigured database must fail closed");
    assert_eq!(error.code(), "K5001");

    let unknown_statement = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "absent-statement", [request.path]) {
            Ok(changed) => record { status: 200, headers: [], body: request.path },
            Err(error) => match db_rollback(transaction) {
                Ok(undone) => record { status: 404, headers: [], body: error },
                Err(fatal) => record { status: 500, headers: [], body: fatal },
            },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["database.write"],
    );
    let error = Runtime::default()
        .invoke_webhook_with_host(
            &unknown_statement.bytes,
            &unknown_statement.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect_err("an uncatalogued statement must fail closed");
    assert_eq!(error.code(), "K5301");
}

#[test]
fn injection_payloads_stay_ordinary_parameter_data() {
    let directory = TestDirectory::new("injection");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            HttpRequest {
                method: "POST".to_owned(),
                path: "/orders".to_owned(),
                query: String::new(),
                headers: Vec::new(),
                body: String::new(),
            },
        )
        .expect("invocation should succeed");
    assert_eq!(result.response.status, 200);

    let connection = rusqlite::Connection::open(directory.database()).unwrap();
    connection
        .execute(
            "INSERT INTO visits(path) VALUES(?1)",
            rusqlite::params!["x'); DROP TABLE visits; --"],
        )
        .expect("literal payloads are ordinary data");
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM visits", [], |row| row.get(0))
        .expect("the table must survive");
    assert_eq!(total, 2);
}

#[test]
fn database_transaction_bounds_must_fit_the_invocation_deadline() {
    let directory = TestDirectory::new("bounds");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);
    let runtime = Runtime::default();
    let oversized = DatabaseCatalog::open(
        BTreeMap::from([(
            "catalog".to_owned(),
            DatabaseDefinition {
                path: directory.database(),
                mode: DatabaseMode::ReadWrite,
                limits: DatabaseLimits {
                    max_transaction_duration: runtime.limits().deadline()
                        + Duration::from_millis(1),
                    ..limits()
                },
                statements: catalog_statements(),
            },
        )]),
        1,
    )
    .expect("catalog should open");

    let error = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(oversized),
            request("/orders"),
        )
        .expect_err("a transaction bound above the deadline must fail closed");

    assert_eq!(error.code(), "K5301");
}

#[test]
fn stats_and_errors_never_disclose_paths_sql_or_values() {
    let directory = TestDirectory::new("privacy");
    seed(&directory.database());
    let artifact = compile(WRITE_SOURCE, &["database.write"]);

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(catalog(&directory, DatabaseMode::ReadWrite)),
            request("/orders"),
        )
        .expect("invocation should succeed");

    let stats = serde_json::to_string(&result.stats).expect("stats should serialize");
    assert!(!stats.contains("catalog.db"));
    assert!(!stats.contains("INSERT"));
    assert!(!stats.contains("visits"));
    assert!(!stats.contains(&directory.path.to_string_lossy().to_string()));
    assert!(stats.contains("databaseWriteCommitted"));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES)),
            &host(DatabaseCatalog::default()),
            request("/orders"),
        )
        .expect_err("an unconfigured database fails");
    let rendered = format!("{} {}", error.code(), error.message());
    assert!(!rendered.contains("catalog.db"));
    assert!(!rendered.contains(&directory.path.to_string_lossy().to_string()));
}

/// Writes a row, then traps before the transaction is ever completed.
const TRAP_AFTER_WRITE_SOURCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => record { status: 200 / (changed - 1), headers: [], body: "unreachable" },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

/// Writes a row and returns successfully while the transaction is still open.
const UNCLOSED_AFTER_WRITE_SOURCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match db_begin_write("catalog") {
        Ok(transaction) => match db_execute(transaction, "record-visit", [request.path]) {
            Ok(changed) => record { status: 200, headers: [], body: "left open" },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#;

#[test]
fn a_persistent_host_recovers_from_trapped_and_unclosed_transactions() {
    let directory = TestDirectory::new("host-reuse");
    seed(&directory.database());
    let grants = GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES));
    // One host, one open connection, reused across every invocation below.
    let host = host(catalog(&directory, DatabaseMode::ReadWrite));
    let runtime = Runtime::default();

    let trapping = compile(TRAP_AFTER_WRITE_SOURCE, &["database.write"]);
    let trapped = runtime.invoke_webhook_with_host(
        &trapping.bytes,
        &trapping.metadata,
        &grants,
        &host,
        request("/trap"),
    );
    assert!(trapped.is_err(), "the guest must trap");
    assert_eq!(
        visit_count(&directory.database()),
        0,
        "a trapped invocation must not publish its write"
    );

    let unclosed = compile(UNCLOSED_AFTER_WRITE_SOURCE, &["database.write"]);
    let left_open = runtime
        .invoke_webhook_with_host(
            &unclosed.bytes,
            &unclosed.metadata,
            &grants,
            &host,
            request("/unclosed"),
        )
        .expect_err("an unclosed transaction must fail the invocation");
    assert_eq!(left_open.code(), "K5302");
    assert_eq!(
        visit_count(&directory.database()),
        0,
        "an unclosed transaction must not publish its write"
    );

    // The reused connection is clean: a later invocation begins, writes, and
    // commits normally.
    let writer = compile(WRITE_SOURCE, &["database.write"]);
    let recovered = runtime
        .invoke_webhook_with_host(
            &writer.bytes,
            &writer.metadata,
            &grants,
            &host,
            request("/after"),
        )
        .expect("the host must be reusable after failed invocations");

    assert_eq!(recovered.response.status, 200);
    assert_eq!(
        recovered.response.body,
        "{\"columns\":[\"total\"],\"rows\":[[1]]}"
    );
    assert_eq!(recovered.stats.database_transactions_abandoned, 0);
    assert_eq!(
        visit_count(&directory.database()),
        1,
        "only the committed write survives"
    );
}

#[test]
fn cleanup_is_reported_and_repeatable_across_many_failed_invocations() {
    let directory = TestDirectory::new("host-reuse-repeat");
    seed(&directory.database());
    let grants = GrantSet::from_manifest(&manifest(WRITE_CAPABILITIES));
    let host = host(catalog(&directory, DatabaseMode::ReadWrite));
    let runtime = Runtime::default();
    let unclosed = compile(UNCLOSED_AFTER_WRITE_SOURCE, &["database.write"]);

    for round in 0..8 {
        let error = runtime
            .invoke_webhook_with_host(
                &unclosed.bytes,
                &unclosed.metadata,
                &grants,
                &host,
                request("/unclosed"),
            )
            .expect_err("an unclosed transaction always fails closed");
        assert_eq!(error.code(), "K5302", "round {round} reported {error}");
    }

    assert_eq!(visit_count(&directory.database()), 0);
    let writer = compile(WRITE_SOURCE, &["database.write"]);
    let recovered = runtime
        .invoke_webhook_with_host(
            &writer.bytes,
            &writer.metadata,
            &grants,
            &host,
            request("/after"),
        )
        .expect("the host must still be usable");
    assert_eq!(recovered.response.status, 200);
    assert_eq!(visit_count(&directory.database()), 1);
}
