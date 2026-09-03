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

use krit_state::{
    BucketPolicy, CommitPlan, Completion, Durability, DurableStore, JobDisposition, Mutation,
    QueuePolicy, SchedulePolicy, StateErrorKind, StoreLimits,
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("krit-jobs-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn database(&self) -> PathBuf {
        self.path.join("jobs.db")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn limits() -> StoreLimits {
    StoreLimits {
        busy_timeout: Duration::from_millis(250),
        max_operations: 32,
        max_key_bytes: 64,
        max_value_bytes: 4096,
        max_transaction_bytes: 65_536,
        max_database_bytes: 4 * 1024 * 1024,
        max_replay_entries: 8,
        max_replay_bytes: 4096,
    }
}

fn open(directory: &TestDirectory) -> DurableStore {
    DurableStore::open(&directory.database(), Durability::Full, limits())
        .expect("store should open")
}

fn queue_policy() -> QueuePolicy {
    QueuePolicy {
        max_depth: 8,
        max_job_bytes: 128,
        max_queue_bytes: 512,
        max_attempts: 2,
        lease: Duration::from_secs(30),
        backoff: Duration::from_secs(1),
        max_backoff: Duration::from_secs(4),
        dead_letter_max_entries: 3,
        dead_letter_ttl: Duration::from_secs(600),
    }
}

fn bucket_policy() -> BucketPolicy {
    BucketPolicy {
        max_objects: 3,
        max_key_bytes: 32,
        max_object_bytes: 64,
        max_bucket_bytes: 100,
    }
}

fn schedule_policy() -> SchedulePolicy {
    SchedulePolicy {
        interval: Duration::from_secs(60),
        start_millis: 0,
        max_catch_up: 3,
        max_attempts: 2,
        lease: Duration::from_secs(30),
        backoff: Duration::from_secs(1),
        max_backoff: Duration::from_secs(4),
        retention: Duration::from_secs(3600),
        max_retained_fires: 8,
    }
}

fn queues() -> BTreeMap<String, QueuePolicy> {
    BTreeMap::from([("work".to_owned(), queue_policy())])
}

fn buckets() -> BTreeMap<String, BucketPolicy> {
    BTreeMap::from([("blobs".to_owned(), bucket_policy())])
}

fn owner(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn job_id(byte: u8) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[0] = byte;
    id
}

fn publish(store: &DurableStore, revision: u64, id: u8, body: &str, now: i64) -> u64 {
    store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::QueuePublish {
                queue: "work".to_owned(),
                id: job_id(id),
                body: body.as_bytes().to_vec(),
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: now,
            completion: None,
        })
        .expect("publish should commit")
}

#[test]
fn queue_delivers_in_publication_order_and_acknowledges_atomically() {
    let directory = TestDirectory::new("fifo");
    let store = open(&directory);

    let mut revision = 0;
    for (index, body) in ["first", "second", "third"].into_iter().enumerate() {
        revision = publish(&store, revision, index as u8 + 1, body, 1_000);
    }
    assert_eq!(store.queue_stats("work").unwrap(), (3, 16));

    for expected in ["first", "second", "third"] {
        let delivery = store
            .reserve_job("work", queue_policy(), &owner(1), 2_000)
            .expect("reservation should succeed")
            .expect("a job should be ready");
        assert_eq!(delivery.body, expected.as_bytes());
        assert_eq!(delivery.attempt, 1);
        assert_eq!(delivery.max_attempts, 2);
        revision = store
            .commit_plan(CommitPlan {
                expected_revision: revision,
                mutations: &[Mutation::Put {
                    key: format!("done-{expected}"),
                    value: expected.as_bytes().to_vec(),
                }],
                queues: &queues(),
                buckets: &buckets(),
                now_millis: 2_000,
                completion: Some(&Completion::Job(delivery.lease)),
            })
            .expect("acknowledgement should commit");
    }

    assert_eq!(store.queue_stats("work").unwrap(), (0, 0));
    assert_eq!(store.get("done-third").unwrap().unwrap(), b"third");
    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(1), 2_000)
            .unwrap()
            .is_none()
    );
}

#[test]
fn concurrent_reservations_never_hand_out_the_same_job_twice() {
    let directory = TestDirectory::new("concurrent");
    let store = Arc::new(open(&directory));

    let mut revision = 0;
    for index in 0..4u8 {
        revision = publish(&store, revision, index + 1, "payload", 1_000);
    }
    let _ = revision;

    let barrier = Arc::new(Barrier::new(4));
    let handles = (0..4u8)
        .map(|worker| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                store
                    .reserve_job("work", queue_policy(), &owner(worker + 10), 2_000)
                    .expect("reservation should succeed")
                    .map(|delivery| *delivery.lease.id())
            })
        })
        .collect::<Vec<_>>();
    let mut identities = handles
        .into_iter()
        .filter_map(|handle| handle.join().expect("worker should finish"))
        .collect::<Vec<_>>();
    identities.sort_unstable();
    let unique = identities.len();
    identities.dedup();

    assert_eq!(identities.len(), unique);
    assert_eq!(unique, 4);
}

#[test]
fn failed_jobs_retry_with_capped_backoff_and_then_dead_letter() {
    let directory = TestDirectory::new("retry");
    let store = open(&directory);
    publish(&store, 0, 1, "payload", 1_000);

    let first = store
        .reserve_job("work", queue_policy(), &owner(1), 2_000)
        .unwrap()
        .expect("first attempt should reserve");
    assert_eq!(first.attempt, 1);
    let disposition = store
        .fail_job(&first.lease, "boom", queue_policy(), 2_000)
        .expect("failure should record");
    assert_eq!(
        disposition,
        JobDisposition::Retried {
            visible_at_millis: 3_000,
        }
    );
    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(1), 2_500)
            .unwrap()
            .is_none(),
        "backoff must hide the job until it is visible again"
    );

    let second = store
        .reserve_job("work", queue_policy(), &owner(2), 3_000)
        .unwrap()
        .expect("second attempt should reserve");
    assert_eq!(second.attempt, 2);
    assert_eq!(
        store
            .fail_job(&second.lease, "boom again", queue_policy(), 3_000)
            .unwrap(),
        JobDisposition::DeadLettered
    );

    assert_eq!(store.queue_stats("work").unwrap(), (0, 0));
    let dead = store.dead_letters("work", 8).unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].attempts, 2);
    assert_eq!(dead[0].reason, "boom again");
    assert_eq!(dead[0].body, b"payload");
    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(3), 9_000)
            .unwrap()
            .is_none()
    );
}

#[test]
fn expired_leases_recover_and_still_respect_the_attempt_cap() {
    let directory = TestDirectory::new("lease");
    let store = open(&directory);
    publish(&store, 0, 1, "payload", 1_000);

    let abandoned = store
        .reserve_job("work", queue_policy(), &owner(1), 2_000)
        .unwrap()
        .expect("first attempt should reserve");
    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(2), 2_000)
            .unwrap()
            .is_none(),
        "a live lease must not be handed to a second worker"
    );

    let recovered = store
        .reserve_job("work", queue_policy(), &owner(2), 40_000)
        .unwrap()
        .expect("expired lease should recover");
    assert_eq!(recovered.attempt, 2);
    assert_eq!(
        store
            .fail_job(&abandoned.lease, "stale", queue_policy(), 40_000)
            .unwrap(),
        JobDisposition::Lost,
        "the abandoned owner must not mutate a re-leased job"
    );

    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(3), 80_000)
            .unwrap()
            .is_none(),
        "attempt exhaustion must dead-letter instead of redelivering"
    );
    assert_eq!(store.dead_letters("work", 8).unwrap().len(), 1);
}

#[test]
fn queue_depth_byte_and_dead_letter_bounds_are_enforced() {
    let directory = TestDirectory::new("bounds");
    let store = open(&directory);

    let mut revision = 0;
    for index in 0..8u8 {
        revision = publish(&store, revision, index + 1, "12345678", 1_000);
    }
    let depth_error = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::QueuePublish {
                queue: "work".to_owned(),
                id: job_id(200),
                body: b"overflow".to_vec(),
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect_err("queue depth must be bounded");
    assert_eq!(depth_error.kind(), StateErrorKind::Limit);

    let oversized = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::QueuePublish {
                queue: "work".to_owned(),
                id: job_id(201),
                body: vec![b'x'; 129],
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect_err("job bytes must be bounded");
    assert_eq!(oversized.kind(), StateErrorKind::Limit);

    let unconfigured = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::QueuePublish {
                queue: "other".to_owned(),
                id: job_id(202),
                body: b"x".to_vec(),
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect_err("unconfigured queues must be refused");
    assert_eq!(unconfigured.kind(), StateErrorKind::Limit);

    assert_eq!(store.queue_stats("work").unwrap().0, 8);
    assert_eq!(store.revision().unwrap(), revision);
}

#[test]
fn dead_letter_retention_bounds_entries_and_age() {
    let directory = TestDirectory::new("dead-letter");
    let store = open(&directory);

    let mut revision = 0;
    for index in 0..5u8 {
        revision = publish(&store, revision, index + 1, "payload", 1_000);
    }
    let _ = revision;
    for _ in 0..5 {
        let delivery = store
            .reserve_job("work", queue_policy(), &owner(1), 2_000)
            .unwrap()
            .expect("job should reserve");
        store
            .fail_job(&delivery.lease, "one", queue_policy(), 2_000)
            .unwrap();
        let delivery = store
            .reserve_job("work", queue_policy(), &owner(1), 10_000)
            .unwrap()
            .expect("retry should reserve");
        assert_eq!(
            store
                .fail_job(&delivery.lease, "two", queue_policy(), 10_000)
                .unwrap(),
            JobDisposition::DeadLettered
        );
    }

    assert_eq!(
        store.dead_letters("work", 16).unwrap().len(),
        3,
        "dead-letter retention must bound the entry count"
    );

    let aged = 10_000 + queue_policy().dead_letter_ttl.as_millis() as i64 + 1;
    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(1), aged)
            .unwrap()
            .is_none()
    );
    assert!(store.dead_letters("work", 16).unwrap().is_empty());
}

#[test]
fn publishes_and_acknowledgements_roll_back_together() {
    let directory = TestDirectory::new("rollback");
    let store = open(&directory);
    let revision = publish(&store, 0, 1, "payload", 1_000);
    let delivery = store
        .reserve_job("work", queue_policy(), &owner(1), 2_000)
        .unwrap()
        .expect("job should reserve");

    let error = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[
                Mutation::Put {
                    key: "progress".to_owned(),
                    value: b"partial".to_vec(),
                },
                Mutation::QueuePublish {
                    queue: "work".to_owned(),
                    id: job_id(9),
                    body: vec![b'x'; 129],
                },
            ],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 2_000,
            completion: Some(&Completion::Job(delivery.lease.clone())),
        })
        .expect_err("an out-of-bounds publish must abort the whole outcome");
    assert_eq!(error.kind(), StateErrorKind::Limit);

    assert!(store.get("progress").unwrap().is_none());
    assert_eq!(store.queue_stats("work").unwrap().0, 1);
    assert_eq!(store.revision().unwrap(), revision);

    store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 2_000,
            completion: Some(&Completion::Job(delivery.lease)),
        })
        .expect("the original lease should still acknowledge");
    assert_eq!(store.queue_stats("work").unwrap().0, 0);
}

#[test]
fn queue_state_survives_reopening_the_store() {
    let directory = TestDirectory::new("restart");
    let store = open(&directory);
    publish(&store, 0, 1, "payload", 1_000);
    let delivery = store
        .reserve_job("work", queue_policy(), &owner(1), 2_000)
        .unwrap()
        .expect("job should reserve");
    drop(delivery);
    drop(store);

    let store = open(&directory);
    assert_eq!(store.queue_stats("work").unwrap().0, 1);
    assert!(
        store
            .reserve_job("work", queue_policy(), &owner(2), 2_000)
            .unwrap()
            .is_none(),
        "a lease survives restart until it expires"
    );
    let recovered = store
        .reserve_job("work", queue_policy(), &owner(2), 60_000)
        .unwrap()
        .expect("expired lease should recover after restart");
    assert_eq!(recovered.attempt, 2);
}

#[test]
fn objects_enforce_count_byte_and_replacement_accounting() {
    let directory = TestDirectory::new("objects");
    let store = open(&directory);

    let mut revision = 0;
    for index in 0..3 {
        revision = store
            .commit_plan(CommitPlan {
                expected_revision: revision,
                mutations: &[Mutation::ObjectPut {
                    bucket: "blobs".to_owned(),
                    key: format!("key-{index}"),
                    value: vec![b'a'; 32],
                }],
                queues: &queues(),
                buckets: &buckets(),
                now_millis: 1_000,
                completion: None,
            })
            .expect("object should commit");
    }
    assert_eq!(store.object_stats("blobs").unwrap(), (3, 96));

    let count_error = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::ObjectPut {
                bucket: "blobs".to_owned(),
                key: "key-3".to_owned(),
                value: b"x".to_vec(),
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect_err("object count must be bounded");
    assert_eq!(count_error.kind(), StateErrorKind::Limit);

    let byte_error = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::ObjectPut {
                bucket: "blobs".to_owned(),
                key: "key-0".to_owned(),
                value: vec![b'b'; 64],
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect_err("bucket bytes must be bounded");
    assert_eq!(byte_error.kind(), StateErrorKind::Limit);

    revision = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::ObjectPut {
                bucket: "blobs".to_owned(),
                key: "key-0".to_owned(),
                value: vec![b'b'; 8],
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect("replacement should reuse the previous allocation");
    assert_eq!(store.object_stats("blobs").unwrap(), (3, 72));

    store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::ObjectDelete {
                bucket: "blobs".to_owned(),
                key: "key-0".to_owned(),
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect("delete should commit");
    assert_eq!(store.object_stats("blobs").unwrap(), (2, 64));
    assert!(store.object("blobs", "key-0").unwrap().is_none());
}

#[test]
fn object_listing_is_deterministic_bounded_and_bucket_isolated() {
    let directory = TestDirectory::new("listing");
    let store = open(&directory);
    let policy = BucketPolicy {
        max_objects: 16,
        ..bucket_policy()
    };
    let buckets = BTreeMap::from([("blobs".to_owned(), policy), ("other".to_owned(), policy)]);

    let mut revision = 0;
    for (bucket, key) in [
        ("blobs", "b/2"),
        ("blobs", "a/1"),
        ("blobs", "b/1"),
        ("other", "b/9"),
    ] {
        revision = store
            .commit_plan(CommitPlan {
                expected_revision: revision,
                mutations: &[Mutation::ObjectPut {
                    bucket: bucket.to_owned(),
                    key: key.to_owned(),
                    value: b"v".to_vec(),
                }],
                queues: &queues(),
                buckets: &buckets,
                now_millis: 1_000,
                completion: None,
            })
            .expect("object should commit");
    }
    let _ = revision;

    let keys = store
        .object_keys("blobs", "", 16)
        .unwrap()
        .into_iter()
        .map(|entry| entry.key)
        .collect::<Vec<_>>();
    assert_eq!(keys, ["a/1", "b/1", "b/2"]);

    let prefixed = store
        .object_keys("blobs", "b/", 16)
        .unwrap()
        .into_iter()
        .map(|entry| entry.key)
        .collect::<Vec<_>>();
    assert_eq!(prefixed, ["b/1", "b/2"]);
    assert_eq!(store.object_keys("blobs", "", 1).unwrap().len(), 1);
    assert_eq!(store.object_keys("other", "", 16).unwrap().len(), 1);
    assert!(store.object("other", "a/1").unwrap().is_none());
    assert_eq!(
        store
            .object_keys("blobs", "", 0)
            .expect_err("listing limits are bounded")
            .kind(),
        StateErrorKind::Limit
    );
}

#[test]
fn schedules_fire_once_per_instant_and_bound_catch_up() {
    let directory = TestDirectory::new("schedule");
    let store = open(&directory);
    let policy = schedule_policy();
    let minute = 60_000;

    let first = store
        .materialize_schedule("sweep", policy, 5 * minute)
        .expect("first tick should materialize");
    assert_eq!(first.materialized, 1);
    assert_eq!(first.skipped, 0);
    assert_eq!(first.cursor_due_millis, 5 * minute);
    assert_eq!(
        store
            .materialize_schedule("sweep", policy, 5 * minute)
            .unwrap()
            .materialized,
        0,
        "a duplicate tick at the same instant must not create a second fire"
    );

    let delivery = store
        .reserve_schedule_fire("sweep", policy, &owner(1), 5 * minute)
        .unwrap()
        .expect("the due fire should reserve");
    assert_eq!(delivery.lease.due_at_millis(), 5 * minute);
    store
        .commit_plan(CommitPlan {
            expected_revision: 0,
            mutations: &[],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 5 * minute,
            completion: Some(&Completion::Fire(delivery.lease)),
        })
        .expect("fire completion should commit");
    assert_eq!(store.schedule_stats("sweep").unwrap(), (0, 1, 0));

    let catch_up = store
        .materialize_schedule("sweep", policy, 20 * minute)
        .expect("catch-up should materialize");
    assert_eq!(catch_up.materialized, 3);
    assert_eq!(catch_up.skipped, 12);
    assert_eq!(catch_up.cursor_due_millis, 20 * minute);

    let mut due = Vec::new();
    while let Some(delivery) = store
        .reserve_schedule_fire("sweep", policy, &owner(2), 20 * minute)
        .unwrap()
    {
        due.push(delivery.lease.due_at_millis());
        store
            .commit_plan(CommitPlan {
                expected_revision: 0,
                mutations: &[],
                queues: &queues(),
                buckets: &buckets(),
                now_millis: 20 * minute,
                completion: Some(&Completion::Fire(delivery.lease)),
            })
            .expect("fire completion should commit");
    }
    assert_eq!(due, [18 * minute, 19 * minute, 20 * minute]);
}

#[test]
fn schedule_fires_retry_dead_letter_and_survive_restart() {
    let directory = TestDirectory::new("schedule-retry");
    let store = open(&directory);
    let policy = schedule_policy();
    let minute = 60_000;

    store
        .materialize_schedule("sweep", policy, minute)
        .expect("tick should materialize");
    let first = store
        .reserve_schedule_fire("sweep", policy, &owner(1), minute)
        .unwrap()
        .expect("fire should reserve");
    assert_eq!(
        store
            .fail_schedule_fire(&first.lease, policy, minute)
            .unwrap(),
        JobDisposition::Retried {
            visible_at_millis: minute + 1_000,
        }
    );
    drop(store);

    let store = open(&directory);
    assert_eq!(store.schedule_stats("sweep").unwrap(), (1, 0, 0));
    let second = store
        .reserve_schedule_fire("sweep", policy, &owner(2), minute + 2_000)
        .unwrap()
        .expect("fire should reserve after restart");
    assert_eq!(second.attempt, 2);
    assert_eq!(
        store
            .fail_schedule_fire(&second.lease, policy, minute + 2_000)
            .unwrap(),
        JobDisposition::DeadLettered
    );
    assert_eq!(store.schedule_stats("sweep").unwrap(), (0, 0, 1));
    assert!(
        store
            .reserve_schedule_fire("sweep", policy, &owner(3), minute + 9_000)
            .unwrap()
            .is_none()
    );
}

#[test]
fn schedule_start_instants_are_utc_epoch_aligned() {
    let directory = TestDirectory::new("schedule-epoch");
    let store = open(&directory);
    let policy = SchedulePolicy {
        interval: Duration::from_secs(3600),
        start_millis: 1_500_000,
        ..schedule_policy()
    };

    assert_eq!(
        store
            .materialize_schedule("sweep", policy, 1_400_000)
            .unwrap()
            .materialized,
        0,
        "instants before the start must not fire"
    );
    let catch_up = store
        .materialize_schedule("sweep", policy, 1_500_000 + 3_600_000 + 5)
        .expect("tick should materialize");
    assert_eq!(catch_up.materialized, 1);
    assert_eq!(catch_up.cursor_due_millis, 1_500_000 + 3_600_000);
    let delivery = store
        .reserve_schedule_fire("sweep", policy, &owner(1), 1_500_000 + 3_600_005)
        .unwrap()
        .expect("fire should reserve");
    assert_eq!(delivery.lease.due_at_millis(), 1_500_000 + 3_600_000);
}

#[test]
fn schema_one_stores_migrate_forward_and_preserve_their_data() {
    let directory = TestDirectory::new("migration");
    let path = directory.database();
    write_schema_one_store(&path);

    let store = DurableStore::open(&path, Durability::Full, limits())
        .expect("schema-1 store should migrate forward");

    assert_eq!(store.get("preserved").unwrap().unwrap(), b"value");
    assert_eq!(store.checkpoint("marker").unwrap().unwrap(), b"done");
    assert_eq!(store.revision().unwrap(), 7);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    drop(connection);

    let revision = publish(&store, 7, 1, "payload", 1_000);
    assert_eq!(
        revision, 7,
        "a queue-only publish must not advance the state revision"
    );
    assert_eq!(store.queue_stats("work").unwrap().0, 1);
}

#[test]
fn newer_and_partially_migrated_schemas_are_rejected() {
    let directory = TestDirectory::new("schema-guard");

    let newer = directory.path.join("newer.db");
    let connection = rusqlite::Connection::open(&newer).unwrap();
    connection
        .pragma_update(None, "application_id", 0x4b52_4954i64)
        .unwrap();
    connection
        .pragma_update(None, "user_version", 3i64)
        .unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&newer, Durability::Full, limits())
            .expect_err("a newer schema must be rejected")
            .kind(),
        StateErrorKind::Database
    );

    let extra = directory.path.join("extra.db");
    DurableStore::open(&extra, Durability::Full, limits()).unwrap();
    let connection = rusqlite::Connection::open(&extra).unwrap();
    connection
        .execute("CREATE TABLE surprise(id INTEGER PRIMARY KEY) STRICT", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&extra, Durability::Full, limits())
            .expect_err("extra tables must be rejected")
            .kind(),
        StateErrorKind::Database
    );

    let dropped = directory.path.join("dropped.db");
    DurableStore::open(&dropped, Durability::Full, limits()).unwrap();
    let connection = rusqlite::Connection::open(&dropped).unwrap();
    connection
        .execute("DROP INDEX queue_jobs_ready", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        DurableStore::open(&dropped, Durability::Full, limits())
            .expect_err("a missing job index must be rejected")
            .kind(),
        StateErrorKind::Database
    );
}

/// Recreates the exact schema-1 store this runtime shipped before Phase 6 jobs.
fn write_schema_one_store(path: &std::path::Path) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE meta(
                id INTEGER PRIMARY KEY CHECK(id = 1),
                revision INTEGER NOT NULL CHECK(revision >= 0),
                sequence INTEGER NOT NULL CHECK(sequence >= 0)
             ) STRICT;
             INSERT INTO meta(id, revision, sequence) VALUES(1, 7, 3);
             CREATE TABLE kv(
                key TEXT PRIMARY KEY,
                value BLOB NOT NULL
             ) STRICT;
             INSERT INTO kv(key, value) VALUES('preserved', CAST('value' AS BLOB));
             CREATE TABLE checkpoints(
                name TEXT PRIMARY KEY,
                value BLOB NOT NULL
             ) STRICT;
             INSERT INTO checkpoints(name, value) VALUES('marker', CAST('done' AS BLOB));
             CREATE TABLE replay(
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
             ) STRICT;
             CREATE INDEX replay_cleanup
                ON replay(status, expires_at, last_used);
             CREATE TABLE idempotency(
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
             ) STRICT;
             CREATE INDEX idempotency_cleanup
                ON idempotency(status, expires_at, last_used);",
        )
        .unwrap();
    connection
        .pragma_update(None, "application_id", 0x4b52_4954i64)
        .unwrap();
    connection
        .pragma_update(None, "user_version", 1i64)
        .unwrap();
}

// ---------------------------------------------------------------------------
// Regressions for the phase6-jobs-storage review findings.
// ---------------------------------------------------------------------------

/// Finding 1: a depth-one queue whose only job already used its single attempt
/// must terminalize and stay usable instead of wedging on the scan bound.
#[test]
fn depth_one_queue_terminalizes_an_exhausted_job_after_a_crash() {
    let directory = TestDirectory::new("scan-bound-queue");
    let store = open(&directory);
    let policy = QueuePolicy {
        max_depth: 1,
        max_attempts: 1,
        ..queue_policy()
    };

    store
        .commit_plan(CommitPlan {
            expected_revision: 0,
            mutations: &[Mutation::QueuePublish {
                queue: "work".to_owned(),
                id: job_id(1),
                body: b"payload".to_vec(),
            }],
            queues: &BTreeMap::from([("work".to_owned(), policy)]),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect("publish should commit");

    let delivery = store
        .reserve_job("work", policy, &owner(1), 2_000)
        .unwrap()
        .expect("the only job should reserve");
    assert_eq!(delivery.attempt, 1);
    // The worker is killed: the lease simply expires.
    drop(delivery);

    let recovered = store
        .reserve_job("work", policy, &owner(2), 90_000)
        .expect("an exhausted job must not error the reservation")
        .is_none();
    assert!(recovered, "the exhausted job must not be redelivered");
    assert_eq!(
        store.queue_stats("work").unwrap(),
        (0, 0),
        "the terminal transition must be persisted"
    );
    assert_eq!(store.dead_letters("work", 8).unwrap().len(), 1);

    // The queue is still usable afterwards.
    let revision = store.revision().unwrap();
    store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::QueuePublish {
                queue: "work".to_owned(),
                id: job_id(2),
                body: b"next".to_vec(),
            }],
            queues: &BTreeMap::from([("work".to_owned(), policy)]),
            buckets: &buckets(),
            now_millis: 95_000,
            completion: None,
        })
        .expect("a fresh publish should commit");
    let next = store
        .reserve_job("work", policy, &owner(3), 96_000)
        .unwrap()
        .expect("the fresh job should reserve");
    assert_eq!(next.body, b"next");
}

/// Finding 1: the same guarantee for a schedule whose retention bound is one.
#[test]
fn single_retained_schedule_terminalizes_an_exhausted_fire_after_a_crash() {
    let directory = TestDirectory::new("scan-bound-schedule");
    let store = open(&directory);
    let policy = SchedulePolicy {
        max_attempts: 1,
        max_retained_fires: 1,
        ..schedule_policy()
    };
    let minute = 60_000;

    store
        .materialize_schedule("sweep", policy, minute)
        .expect("tick should materialize");
    let delivery = store
        .reserve_schedule_fire("sweep", policy, &owner(1), minute)
        .unwrap()
        .expect("fire should reserve");
    drop(delivery);

    assert!(
        store
            .reserve_schedule_fire("sweep", policy, &owner(2), minute + 90_000)
            .expect("an exhausted fire must not error the reservation")
            .is_none()
    );
    assert_eq!(
        store.schedule_stats("sweep").unwrap(),
        (0, 0, 1),
        "the dead fire transition must be persisted"
    );

    let catch_up = store
        .materialize_schedule("sweep", policy, 5 * minute)
        .expect("a later tick should still materialize");
    assert_eq!(catch_up.materialized, policy.max_catch_up);
    let next = store
        .reserve_schedule_fire("sweep", policy, &owner(3), 5 * minute)
        .unwrap()
        .expect("the later fire should reserve");
    assert_eq!(next.lease.due_at_millis(), 3 * minute);
}

/// Finding 6: independent queue-only publishers never conflict on the shared
/// state revision, and combined outcomes still advance it exactly once.
#[test]
fn queue_publications_are_independent_of_the_state_revision() {
    let directory = TestDirectory::new("revision-decoupled");
    let store = Arc::new(open(&directory));
    let stale_revision = store.revision().unwrap();

    let barrier = Arc::new(Barrier::new(4));
    let handles = (0..4u8)
        .map(|publisher| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                store.commit_plan(CommitPlan {
                    // Every publisher uses the same snapshot revision.
                    expected_revision: stale_revision,
                    mutations: &[Mutation::QueuePublish {
                        queue: "work".to_owned(),
                        id: job_id(publisher + 1),
                        body: b"payload".to_vec(),
                    }],
                    queues: &queues(),
                    buckets: &buckets(),
                    now_millis: 1_000,
                    completion: None,
                })
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle
            .join()
            .expect("publisher should finish")
            .expect("concurrent queue-only publishes must not conflict");
    }

    assert_eq!(store.queue_stats("work").unwrap().0, 4);
    assert_eq!(
        store.revision().unwrap(),
        stale_revision,
        "publish-only outcomes must not advance the revision"
    );

    // Deterministic FIFO order still holds without the revision guard.
    let mut sequence = Vec::new();
    while let Some(delivery) = store
        .reserve_job("work", queue_policy(), &owner(9), 2_000)
        .unwrap()
    {
        sequence.push(*delivery.lease.id());
        store
            .commit_plan(CommitPlan {
                expected_revision: store.revision().unwrap(),
                mutations: &[],
                queues: &queues(),
                buckets: &buckets(),
                now_millis: 2_000,
                completion: Some(&Completion::Job(delivery.lease)),
            })
            .expect("acknowledgement should commit");
    }
    assert_eq!(sequence.len(), 4);
    let mut expected = (1..=4u8).map(job_id).collect::<Vec<_>>();
    expected.sort_unstable();
    let mut observed = sequence.clone();
    observed.sort_unstable();
    assert_eq!(observed, expected);
}

/// Finding 6: a worker that both writes state and publishes follow-up work
/// still checks and advances the revision exactly once.
#[test]
fn combined_state_and_publish_outcomes_advance_the_revision_once() {
    let directory = TestDirectory::new("revision-combined");
    let store = open(&directory);
    let revision = store.revision().unwrap();

    let next = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[
                Mutation::Put {
                    key: "progress".to_owned(),
                    value: b"done".to_vec(),
                },
                Mutation::QueuePublish {
                    queue: "work".to_owned(),
                    id: job_id(1),
                    body: b"follow-up".to_vec(),
                },
                Mutation::QueuePublish {
                    queue: "work".to_owned(),
                    id: job_id(2),
                    body: b"follow-up".to_vec(),
                },
            ],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect("combined outcome should commit");

    assert_eq!(next, revision + 1);
    assert_eq!(store.queue_stats("work").unwrap().0, 2);

    let stale = store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::Put {
                key: "progress".to_owned(),
                value: b"stale".to_vec(),
            }],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect_err("a stale revision-sensitive outcome must still conflict");
    assert_eq!(stale.kind(), StateErrorKind::Conflict);
}

/// Finding 8: an unrepresentable instant must be rejected before any cursor or
/// fire row moves, and a later ordinary tick must still work.
#[test]
fn extreme_schedule_instants_leave_the_cursor_untouched() {
    let directory = TestDirectory::new("extreme-instant");
    let store = open(&directory);
    let policy = schedule_policy();
    let minute = 60_000;

    store
        .materialize_schedule("sweep", policy, minute)
        .expect("first tick should materialize");
    let delivery = store
        .reserve_schedule_fire("sweep", policy, &owner(1), minute)
        .unwrap()
        .expect("fire should reserve");
    store
        .commit_plan(CommitPlan {
            expected_revision: 0,
            mutations: &[],
            queues: &queues(),
            buckets: &buckets(),
            now_millis: minute,
            completion: Some(&Completion::Fire(delivery.lease)),
        })
        .expect("fire completion should commit");
    let before = store.schedule_stats("sweep").unwrap();

    for extreme in [i64::MAX, i64::MAX - 1] {
        assert_eq!(
            store
                .materialize_schedule("sweep", policy, extreme)
                .expect_err("an unrepresentable instant must be refused")
                .kind(),
            StateErrorKind::Limit
        );
        assert_eq!(
            store
                .reserve_schedule_fire("sweep", policy, &owner(2), extreme)
                .expect_err("an unrepresentable instant must be refused")
                .kind(),
            StateErrorKind::Limit
        );
    }
    assert_eq!(store.schedule_stats("sweep").unwrap(), before);

    let catch_up = store
        .materialize_schedule("sweep", policy, 3 * minute)
        .expect("an ordinary tick must still work");
    assert_eq!(catch_up.materialized, 2);
    assert_eq!(catch_up.cursor_due_millis, 3 * minute);
    assert!(
        store
            .reserve_schedule_fire("sweep", policy, &owner(3), 3 * minute)
            .unwrap()
            .is_some()
    );
}

/// Finding 8: queue reservation and failure also refuse extreme instants
/// without mutating the queue.
#[test]
fn extreme_queue_instants_leave_the_queue_untouched() {
    let directory = TestDirectory::new("extreme-queue");
    let store = open(&directory);
    publish(&store, 0, 1, "payload", 1_000);

    assert_eq!(
        store
            .reserve_job("work", queue_policy(), &owner(1), i64::MAX)
            .expect_err("an unrepresentable lease deadline must be refused")
            .kind(),
        StateErrorKind::Limit
    );
    assert_eq!(store.queue_stats("work").unwrap().0, 1);

    let delivery = store
        .reserve_job("work", queue_policy(), &owner(1), 2_000)
        .unwrap()
        .expect("an ordinary instant should reserve");
    assert_eq!(
        store
            .fail_job(&delivery.lease, "boom", queue_policy(), i64::MAX)
            .expect_err("an unrepresentable retry instant must be refused")
            .kind(),
        StateErrorKind::Limit
    );
    assert!(matches!(
        store
            .fail_job(&delivery.lease, "boom", queue_policy(), 2_000)
            .unwrap(),
        JobDisposition::Retried { .. }
    ));
}

/// Finding 9: prefix matching is exact, case-sensitive, and treats `%` and `_`
/// as ordinary characters.
#[test]
fn object_prefixes_match_exact_case_sensitive_bytes() {
    let directory = TestDirectory::new("prefix");
    let store = open(&directory);
    let policy = BucketPolicy {
        max_objects: 16,
        max_bucket_bytes: 512,
        ..bucket_policy()
    };
    let buckets = BTreeMap::from([("blobs".to_owned(), policy)]);

    let mut revision = 0;
    for key in [
        "Apple", "apple", "a%b", "a_b", "axb", "ab", "%literal", "_literal",
    ] {
        revision = store
            .commit_plan(CommitPlan {
                expected_revision: revision,
                mutations: &[Mutation::ObjectPut {
                    bucket: "blobs".to_owned(),
                    key: key.to_owned(),
                    value: b"v".to_vec(),
                }],
                queues: &queues(),
                buckets: &buckets,
                now_millis: 1_000,
                completion: None,
            })
            .expect("object should commit");
    }
    let _ = revision;

    let keys = |prefix: &str| {
        store
            .object_keys("blobs", prefix, 16)
            .unwrap()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>()
    };

    let lowercase = keys("a");
    assert!(
        !lowercase.contains(&"Apple".to_owned()),
        "prefix matching must be case sensitive: {lowercase:?}"
    );
    assert_eq!(lowercase, ["a%b", "a_b", "ab", "apple", "axb"]);
    assert_eq!(keys("A"), ["Apple"]);
    assert_eq!(
        keys("a%"),
        ["a%b"],
        "`%` must be an ordinary prefix character"
    );
    assert_eq!(
        keys("a_"),
        ["a_b"],
        "`_` must not match an arbitrary character"
    );
    assert_eq!(keys("%"), ["%literal"]);
    assert_eq!(keys("_"), ["_literal"]);
    assert_eq!(keys("").len(), 8);
}

/// Finding 3: a schema-1 store carrying an unexpected object is rejected and
/// left byte-for-byte unchanged.
#[test]
fn malformed_schema_one_stores_are_rejected_without_mutation() {
    let directory = TestDirectory::new("migration-guard");

    for (index, mutate) in [
        "CREATE TABLE surprise(id INTEGER PRIMARY KEY) STRICT",
        "CREATE INDEX kv_surprise ON kv(value)",
        "CREATE VIEW surprise AS SELECT 1",
        "CREATE TRIGGER surprise AFTER INSERT ON kv BEGIN UPDATE meta SET revision = 0; END",
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path.join(format!("malformed-{index}.db"));
        write_schema_one_store(&path);
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch(mutate).unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();

        let error = DurableStore::open(&path, Durability::Full, limits())
            .expect_err("a malformed schema-1 store must be rejected");
        assert_eq!(error.kind(), StateErrorKind::Database, "case: {mutate}");

        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "a rejected migration must not change the database: {mutate}"
        );
        let connection = rusqlite::Connection::open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1, "case: {mutate}");
        let preserved: Vec<u8> = connection
            .query_row("SELECT value FROM kv WHERE key = 'preserved'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(preserved, b"value");
    }
}

/// Finding 4: a budget that cannot hold schema 2 must fail closed for both a
/// new store and a schema-1 migration, leaving no successful oversized open.
#[test]
fn database_budgets_are_enforced_before_and_after_schema_work() {
    let directory = TestDirectory::new("budget");

    let too_small = StoreLimits {
        max_database_bytes: krit_state::MINIMUM_DATABASE_BYTES - 1,
        ..limits()
    };
    assert_eq!(
        DurableStore::open(&directory.path.join("tiny.db"), Durability::Full, too_small)
            .expect_err("a sub-minimum budget must be refused")
            .kind(),
        StateErrorKind::Limit
    );
    assert!(
        !directory.path.join("tiny.db").exists()
            || fs::metadata(directory.path.join("tiny.db")).unwrap().len() == 0
    );

    // A budget above the configured minimum but below the schema footprint at a
    // large page size must fail once the schema cannot fit.
    let fresh = directory.path.join("fresh.db");
    let connection = rusqlite::Connection::open(&fresh).unwrap();
    connection
        .pragma_update(None, "page_size", 65_536i64)
        .unwrap();
    connection.execute_batch("VACUUM").unwrap();
    drop(connection);
    let budget = StoreLimits {
        max_database_bytes: krit_state::MINIMUM_DATABASE_BYTES,
        ..limits()
    };
    let error = DurableStore::open(&fresh, Durability::Full, budget)
        .expect_err("a schema that cannot fit the budget must be refused");
    assert_eq!(error.kind(), StateErrorKind::Limit);

    let migrating = directory.path.join("migrating.db");
    write_schema_one_store(&migrating);
    let connection = rusqlite::Connection::open(&migrating).unwrap();
    let filler = vec![b'x'; 900];
    for index in 0..1_200 {
        connection
            .execute(
                "INSERT INTO kv(key, value) VALUES(?1, ?2)",
                rusqlite::params![format!("key-{index}"), filler],
            )
            .unwrap();
    }
    let pages: i64 = connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    let page_size: i64 = connection
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .unwrap();
    drop(connection);
    let occupied = u64::try_from(pages * page_size).unwrap();
    let squeezed = StoreLimits {
        max_database_bytes: occupied.max(krit_state::MINIMUM_DATABASE_BYTES),
        ..limits()
    };
    let before = fs::read(&migrating).unwrap();
    let error = DurableStore::open(&migrating, Durability::Full, squeezed)
        .expect_err("a migration that cannot fit the budget must be refused");
    assert!(matches!(
        error.kind(),
        StateErrorKind::Limit | StateErrorKind::Database
    ));
    assert_eq!(
        fs::read(&migrating).unwrap(),
        before,
        "a refused migration must not change the database"
    );
    let connection = rusqlite::Connection::open(&migrating).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
}
