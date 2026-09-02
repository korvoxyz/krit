use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use krit_state::{
    Durability, DurableStore, IdempotencyDecision, Mutation, ReplayDecision, ReplayKind,
    ReplayRequest, RetentionPolicy, StateErrorKind, StoreLimits,
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("krit-state-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn database(&self) -> PathBuf {
        self.path.join("state.db")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn limits() -> StoreLimits {
    StoreLimits {
        busy_timeout: Duration::from_millis(100),
        max_operations: 16,
        max_key_bytes: 64,
        max_value_bytes: 1024,
        max_transaction_bytes: 4096,
        max_database_bytes: 4 * 1024 * 1024,
        max_replay_entries: 8,
        max_replay_bytes: 4096,
    }
}

fn retention(max_entries: usize, max_bytes: usize) -> RetentionPolicy {
    RetentionPolicy {
        max_entries,
        max_bytes,
        ttl: Duration::from_secs(60),
        lease: Duration::from_secs(5),
    }
}

fn digest(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn owner(byte: u8) -> [u8; 16] {
    [byte; 16]
}

#[test]
fn commits_reopens_and_rejects_stale_revisions() {
    let directory = TestDirectory::new("commit");
    let path = directory.database();
    let store = DurableStore::open(&path, Durability::Full, limits()).expect("store should open");
    let revision = store.revision().expect("revision should read");
    assert_eq!(revision, 0);
    let next = store
        .commit(
            revision,
            &[
                Mutation::Put {
                    key: "issue".to_owned(),
                    value: b"42".to_vec(),
                },
                Mutation::CheckpointPut {
                    name: "posted".to_owned(),
                    value: b"yes".to_vec(),
                },
            ],
        )
        .expect("transaction should commit");
    assert_eq!(next, 1);
    assert_eq!(store.get("issue").unwrap(), Some(b"42".to_vec()));
    assert_eq!(store.checkpoint("posted").unwrap(), Some(b"yes".to_vec()));
    let error = store
        .commit(
            revision,
            &[Mutation::Put {
                key: "issue".to_owned(),
                value: b"stale".to_vec(),
            }],
        )
        .expect_err("stale revision should conflict");
    assert_eq!(error.kind(), StateErrorKind::Conflict);
    drop(store);

    let reopened =
        DurableStore::open(&path, Durability::Full, limits()).expect("store should reopen");
    assert_eq!(reopened.revision().unwrap(), 1);
    assert_eq!(reopened.get("issue").unwrap(), Some(b"42".to_vec()));
    reopened
        .commit(
            1,
            &[Mutation::Delete {
                key: "issue".to_owned(),
            }],
        )
        .expect("delete should commit");
    assert_eq!(reopened.get("issue").unwrap(), None);
}

#[test]
fn independent_connections_serialize_with_revision_conflicts() {
    let directory = TestDirectory::new("concurrent");
    let path = directory.database();
    let first = DurableStore::open(&path, Durability::Normal, limits()).unwrap();
    let second = DurableStore::open(&path, Durability::Normal, limits()).unwrap();
    let first_revision = first.revision().unwrap();
    let second_revision = second.revision().unwrap();
    assert_eq!(first_revision, second_revision);
    first
        .commit(
            first_revision,
            &[Mutation::Put {
                key: "winner".to_owned(),
                value: b"first".to_vec(),
            }],
        )
        .unwrap();
    assert_eq!(
        second
            .commit(
                second_revision,
                &[Mutation::Put {
                    key: "winner".to_owned(),
                    value: b"second".to_vec(),
                }],
            )
            .unwrap_err()
            .kind(),
        StateErrorKind::Conflict
    );
    assert_eq!(second.get("winner").unwrap(), Some(b"first".to_vec()));
}

#[test]
fn revision_bound_reads_reject_mixed_snapshots() {
    let directory = TestDirectory::new("revision-reads");
    let path = directory.database();
    let first = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    let second = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    first
        .commit(
            0,
            &[Mutation::Put {
                key: "key".to_owned(),
                value: b"one".to_vec(),
            }],
        )
        .unwrap();
    assert_eq!(
        first.get_at_revision("key", 1).unwrap(),
        Some(b"one".to_vec())
    );
    second
        .commit(
            1,
            &[Mutation::Put {
                key: "key".to_owned(),
                value: b"two".to_vec(),
            }],
        )
        .unwrap();
    assert_eq!(
        first
            .get_at_revision("key", 1)
            .expect_err("stale read revision should conflict")
            .kind(),
        StateErrorKind::Conflict
    );
}

#[test]
fn busy_timeout_surfaces_a_bounded_transaction_conflict() {
    let directory = TestDirectory::new("busy");
    let path = directory.database();
    let store = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

    let error = store
        .commit(
            0,
            &[Mutation::Put {
                key: "blocked".to_owned(),
                value: b"value".to_vec(),
            }],
        )
        .expect_err("busy writer should fail within the configured timeout");
    assert_eq!(error.kind(), StateErrorKind::Conflict);
    blocker.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn mutation_limits_fail_without_advancing_revision() {
    let directory = TestDirectory::new("mutation-limits");
    let store = DurableStore::open(&directory.database(), Durability::Full, limits()).unwrap();
    let error = store
        .commit(
            0,
            &[Mutation::Put {
                key: "key".to_owned(),
                value: vec![0; limits().max_value_bytes + 1],
            }],
        )
        .expect_err("oversized value should fail");
    assert_eq!(error.kind(), StateErrorKind::Limit);
    assert_eq!(store.revision().unwrap(), 0);
    assert_eq!(store.get("key").unwrap(), None);
}

#[test]
fn replay_records_survive_restart_conflict_and_expiry() {
    let directory = TestDirectory::new("replay");
    let path = directory.database();
    let policy = retention(2, 64);
    let store = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    let lease = match store
        .replay_decision(
            ReplayRequest {
                artifact: &digest(1),
                kind: ReplayKind::Http,
                operation: "fetch",
                input_digest: &digest(2),
                owner: &owner(3),
                now_millis: 1_000,
            },
            policy,
        )
        .unwrap()
    {
        ReplayDecision::Execute(lease) => lease,
        other => panic!("unexpected replay decision: {other:?}"),
    };
    assert_eq!(
        store
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(1),
                    kind: ReplayKind::Http,
                    operation: "fetch",
                    input_digest: &digest(2),
                    owner: &owner(4),
                    now_millis: 1_001,
                },
                policy,
            )
            .unwrap(),
        ReplayDecision::InProgress
    );
    store
        .complete_replay(&lease, b"response", 1_002, policy)
        .unwrap();
    drop(store);

    let reopened = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    assert_eq!(
        reopened
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(1),
                    kind: ReplayKind::Http,
                    operation: "fetch",
                    input_digest: &digest(2),
                    owner: &owner(5),
                    now_millis: 1_003,
                },
                policy,
            )
            .unwrap(),
        ReplayDecision::Replay(b"response".to_vec())
    );
    assert_eq!(
        reopened
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(1),
                    kind: ReplayKind::Http,
                    operation: "fetch",
                    input_digest: &digest(9),
                    owner: &owner(5),
                    now_millis: 1_004,
                },
                policy,
            )
            .unwrap(),
        ReplayDecision::Conflict
    );
    assert!(matches!(
        reopened
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(8),
                    kind: ReplayKind::Http,
                    operation: "fetch",
                    input_digest: &digest(2),
                    owner: &owner(7),
                    now_millis: 1_005,
                },
                policy,
            )
            .unwrap(),
        ReplayDecision::Execute(_)
    ));
    assert!(matches!(
        reopened
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(1),
                    kind: ReplayKind::Http,
                    operation: "fetch",
                    input_digest: &digest(2),
                    owner: &owner(6),
                    now_millis: 62_000,
                },
                policy,
            )
            .unwrap(),
        ReplayDecision::Execute(_)
    ));
}

#[test]
fn replay_and_idempotency_cleanup_enforce_entry_and_byte_bounds() {
    let directory = TestDirectory::new("retention");
    let store = DurableStore::open(&directory.database(), Durability::Full, limits()).unwrap();
    let policy = retention(1, 8);
    for index in 0..2 {
        let lease = match store
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(1),
                    kind: ReplayKind::Ai,
                    operation: &format!("operation-{index}"),
                    input_digest: &digest(index + 2),
                    owner: &owner(index + 4),
                    now_millis: 1_000 + i64::from(index),
                },
                policy,
            )
            .unwrap()
        {
            ReplayDecision::Execute(lease) => lease,
            other => panic!("unexpected replay decision: {other:?}"),
        };
        store
            .complete_replay(&lease, &[index; 8], 1_010 + i64::from(index), policy)
            .unwrap();
    }
    assert_eq!(store.replay_counts().unwrap(), (1, 8));

    for index in 0..2 {
        let key = format!("request-{index}");
        let lease = match store
            .idempotency_decision(
                &digest(7),
                &key,
                &digest(index + 8),
                &owner(index + 10),
                2_000 + i64::from(index),
                policy,
            )
            .unwrap()
        {
            IdempotencyDecision::Execute(lease) => lease,
            other => panic!("unexpected idempotency decision: {other:?}"),
        };
        store
            .complete_idempotency(&lease, &[index; 8], 2_010 + i64::from(index), policy)
            .unwrap();
    }
    assert_eq!(store.idempotency_counts().unwrap(), (1, 8));
}

#[test]
fn durable_idempotency_survives_restart_and_detects_conflicts() {
    let directory = TestDirectory::new("idempotency");
    let path = directory.database();
    let policy = retention(8, 1024);
    let store = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    let lease = match store
        .idempotency_decision(
            &digest(1),
            "incoming-key",
            &digest(2),
            &owner(3),
            1_000,
            policy,
        )
        .unwrap()
    {
        IdempotencyDecision::Execute(lease) => lease,
        other => panic!("unexpected idempotency decision: {other:?}"),
    };
    assert_eq!(
        store
            .idempotency_decision(
                &digest(1),
                "incoming-key",
                &digest(2),
                &owner(9),
                1_000,
                policy,
            )
            .unwrap(),
        IdempotencyDecision::InProgress
    );
    store
        .complete_idempotency(&lease, b"response", 1_001, policy)
        .unwrap();
    drop(store);

    let reopened = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    assert_eq!(
        reopened
            .idempotency_decision(
                &digest(1),
                "incoming-key",
                &digest(2),
                &owner(4),
                1_002,
                policy,
            )
            .unwrap(),
        IdempotencyDecision::Replay(b"response".to_vec())
    );
    assert_eq!(
        reopened
            .idempotency_decision(
                &digest(1),
                "incoming-key",
                &digest(9),
                &owner(4),
                1_003,
                policy,
            )
            .unwrap(),
        IdempotencyDecision::Conflict
    );
    assert!(matches!(
        reopened
            .idempotency_decision(
                &digest(1),
                "incoming-key",
                &digest(2),
                &owner(5),
                62_000,
                policy,
            )
            .unwrap(),
        IdempotencyDecision::Execute(_)
    ));
}

#[test]
fn rejects_limits_corruption_wrong_identity_and_newer_schema() {
    let directory = TestDirectory::new("invalid");
    let invalid_limits = StoreLimits {
        max_operations: 0,
        ..limits()
    };
    assert_eq!(
        DurableStore::open(
            &directory.path.join("invalid-limits.db"),
            Durability::Full,
            invalid_limits,
        )
        .expect_err("invalid limits should fail")
        .kind(),
        StateErrorKind::Limit
    );

    let corrupt = directory.path.join("corrupt.db");
    fs::write(&corrupt, b"not sqlite").unwrap();
    assert_eq!(
        DurableStore::open(&corrupt, Durability::Full, limits())
            .expect_err("corrupt database should fail")
            .kind(),
        StateErrorKind::Database
    );

    let newer = directory.path.join("newer.db");
    configure_foreign_database(&newer, 0x4b52_4954, 2);
    assert_eq!(
        DurableStore::open(&newer, Durability::Full, limits())
            .expect_err("newer schema should fail")
            .kind(),
        StateErrorKind::Database
    );

    let foreign = directory.path.join("foreign.db");
    configure_foreign_database(&foreign, 1234, 1);
    assert_eq!(
        DurableStore::open(&foreign, Durability::Full, limits())
            .expect_err("foreign database should fail")
            .kind(),
        StateErrorKind::Database
    );
    let connection = rusqlite::Connection::open(&foreign).unwrap();
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");

    let malformed = directory.path.join("malformed.db");
    DurableStore::open(&malformed, Durability::Full, limits()).unwrap();
    let connection = rusqlite::Connection::open(&malformed).unwrap();
    connection.execute("DROP INDEX replay_cleanup", []).unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&malformed, Durability::Full, limits())
            .expect_err("missing schema index should fail")
            .kind(),
        StateErrorKind::Database
    );

    let malformed_table = directory.path.join("malformed-table.db");
    DurableStore::open(&malformed_table, Durability::Full, limits()).unwrap();
    let connection = rusqlite::Connection::open(&malformed_table).unwrap();
    connection.execute("DROP TABLE checkpoints", []).unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&malformed_table, Durability::Full, limits())
            .expect_err("missing schema table should fail")
            .kind(),
        StateErrorKind::Database
    );

    let triggered = directory.path.join("triggered.db");
    DurableStore::open(&triggered, Durability::Full, limits()).unwrap();
    let connection = rusqlite::Connection::open(&triggered).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER corrupt_revision AFTER INSERT ON kv
             BEGIN
               UPDATE meta SET revision = 0 WHERE id = 1;
             END;",
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&triggered, Durability::Full, limits())
            .expect_err("unexpected trigger should fail schema validation")
            .kind(),
        StateErrorKind::Database
    );

    let unversioned_view = directory.path.join("unversioned-view.db");
    let connection = rusqlite::Connection::open(&unversioned_view).unwrap();
    connection
        .execute("CREATE VIEW unexpected AS SELECT 1", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&unversioned_view, Durability::Full, limits())
            .expect_err("unversioned schema objects must not be mutated")
            .kind(),
        StateErrorKind::Database
    );
    let connection = rusqlite::Connection::open(&unversioned_view).unwrap();
    let table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_count, 0);
}

#[test]
fn concurrent_first_open_initializes_one_valid_store() {
    let directory = TestDirectory::new("initialize-race");
    let path = Arc::new(directory.database());
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                DurableStore::open(&path, Durability::Full, limits())
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for worker in workers {
        worker
            .join()
            .expect("initializer should not panic")
            .expect("concurrent initializer should succeed");
    }
    let store = DurableStore::open(&path, Durability::Full, limits()).unwrap();
    assert_eq!(store.revision().unwrap(), 0);

    let mut bounded_limits = limits();
    bounded_limits.max_replay_entries = 1;
    bounded_limits.max_replay_bytes = 8;
    let bounded = DurableStore::open(
        &directory.path.join("bounded.db"),
        Durability::Full,
        bounded_limits,
    )
    .unwrap();
    assert_eq!(
        bounded
            .replay_decision(
                ReplayRequest {
                    artifact: &digest(1),
                    kind: ReplayKind::Http,
                    operation: "operation",
                    input_digest: &digest(2),
                    owner: &owner(3),
                    now_millis: 1_000,
                },
                retention(2, 8),
            )
            .expect_err("retention policy must not exceed the store bounds")
            .kind(),
        StateErrorKind::Limit
    );
}

#[cfg(unix)]
#[test]
fn direct_store_open_refuses_symbolic_links() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new("nofollow");
    let real_parent = directory.path.join("real");
    fs::create_dir(&real_parent).unwrap();
    let linked_parent = directory.path.join("linked");
    symlink(&real_parent, &linked_parent).unwrap();
    let target = linked_parent.join("target.db");
    DurableStore::open(&target, Durability::Full, limits()).unwrap();
    let link = directory.path.join("link.db");
    symlink(real_parent.join("target.db"), &link).unwrap();
    assert_eq!(
        DurableStore::open(&link, Durability::Full, limits())
            .expect_err("SQLite open must not follow a database symlink")
            .kind(),
        StateErrorKind::Database
    );
}

#[test]
fn killed_writer_preserves_committed_state_and_rolls_back_open_transaction() {
    let directory = TestDirectory::new("killed-writer");
    let database = directory.database();
    DurableStore::open(&database, Durability::Full, limits()).unwrap();
    let ready = directory.path.join("ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_writer_helper"])
        .env("KRIT_STATE_CRASH_DB", &database)
        .env("KRIT_STATE_CRASH_READY", &ready)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("crash writer should start");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("crash writer exited before staging its transaction: {status}");
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("crash writer did not stage its transaction in time");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("crash writer should be terminated");
    let status = child.wait().expect("crash writer should be reaped");
    assert!(!status.success());

    let store = DurableStore::open(&database, Durability::Full, limits()).unwrap();
    assert_eq!(store.get("committed").unwrap(), Some(b"yes".to_vec()));
    assert_eq!(store.get("uncommitted").unwrap(), None);
    assert_eq!(store.revision().unwrap(), 1);
}

#[test]
fn crash_writer_helper() {
    let (Some(database), Some(ready)) = (
        std::env::var_os("KRIT_STATE_CRASH_DB"),
        std::env::var_os("KRIT_STATE_CRASH_READY"),
    ) else {
        return;
    };
    let connection = rusqlite::Connection::open(PathBuf::from(database)).unwrap();
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO kv(key, value) VALUES('committed', x'796573');
             UPDATE meta SET revision = revision + 1 WHERE id = 1;
             COMMIT;
             BEGIN IMMEDIATE;
             INSERT INTO kv(key, value) VALUES('uncommitted', x'6e6f');
             UPDATE meta SET revision = revision + 1 WHERE id = 1;",
        )
        .unwrap();
    fs::write(PathBuf::from(ready), b"ready").unwrap();
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn configure_foreign_database(path: &Path, application_id: i64, version: i64) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .pragma_update(None, "application_id", application_id)
        .unwrap();
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
}
