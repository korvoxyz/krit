use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, BucketDefinition, BucketPolicy, CancellationHandle,
    DeliveryOutcome, DeliveryRequest, DenyAllApprovalPolicy, Durability, DurableState,
    DurableStoreDefinition, DurableStoreLimits, GrantSet, HostInputs, NetworkPolicy,
    QueueDefinition, QueuePolicy, RetentionPolicy, Runtime, ScheduleDefinition, SchedulePolicy,
    SecretStore,
};
use krit_state::{
    BucketPolicy as StoreBucketPolicy, CommitPlan, DurableStore, Mutation,
    QueuePolicy as StoreQueuePolicy,
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
            "krit-runtime-jobs-{name}-{}-{id}",
            std::process::id()
        ));
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

fn store_limits() -> DurableStoreLimits {
    DurableStoreLimits {
        busy_timeout: Duration::from_millis(250),
        max_operations: 128,
        max_key_bytes: 256,
        max_value_bytes: 64 * 1024,
        max_transaction_bytes: 1024 * 1024,
        max_database_bytes: 64 * 1024 * 1024,
        max_replay_entries: 64,
        max_replay_bytes: 1024 * 1024,
    }
}

fn retention() -> RetentionPolicy {
    RetentionPolicy {
        max_entries: 64,
        max_bytes: 1024 * 1024,
        ttl: Duration::from_secs(3600),
        lease: Duration::from_secs(30),
    }
}

fn queue_policy(max_attempts: u32) -> QueuePolicy {
    QueuePolicy {
        max_depth: 16,
        max_job_bytes: 4096,
        max_queue_bytes: 65_536,
        max_attempts,
        lease: Duration::from_secs(30),
        backoff: Duration::from_secs(1),
        max_backoff: Duration::from_secs(8),
        dead_letter_max_entries: 8,
        dead_letter_ttl: Duration::from_secs(3600),
    }
}

fn bucket_policy() -> BucketPolicy {
    BucketPolicy {
        max_objects: 16,
        max_key_bytes: 128,
        max_object_bytes: 4096,
        max_bucket_bytes: 65_536,
    }
}

fn schedule_policy() -> SchedulePolicy {
    SchedulePolicy {
        interval: Duration::from_secs(60),
        start_millis: 0,
        max_catch_up: 2,
        max_attempts: 2,
        lease: Duration::from_secs(30),
        backoff: Duration::from_secs(1),
        max_backoff: Duration::from_secs(8),
        retention: Duration::from_secs(3600),
        max_retained_fires: 16,
    }
}

struct Resources {
    queues: bool,
    schedules: bool,
    buckets: bool,
    max_attempts: u32,
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            queues: true,
            schedules: false,
            buckets: true,
            max_attempts: 2,
        }
    }
}

fn durable(path: PathBuf, resources: Resources) -> DurableState {
    let state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path,
                durability: Durability::Full,
                limits: store_limits(),
                replay: retention(),
            },
        )]),
        None,
    )
    .expect("durable state should open");
    let queues = if resources.queues {
        BTreeMap::from([(
            "render-jobs".to_owned(),
            QueueDefinition {
                store: "agent-work".to_owned(),
                policy: queue_policy(resources.max_attempts),
            },
        )])
    } else {
        BTreeMap::new()
    };
    let schedules = if resources.schedules {
        BTreeMap::from([(
            "hourly-sweep".to_owned(),
            ScheduleDefinition {
                store: "agent-work".to_owned(),
                policy: schedule_policy(),
            },
        )])
    } else {
        BTreeMap::new()
    };
    let buckets = if resources.buckets {
        BTreeMap::from([(
            "render-output".to_owned(),
            BucketDefinition {
                store: "agent-work".to_owned(),
                policy: bucket_policy(),
            },
        )])
    } else {
        BTreeMap::new()
    };
    state
        .with_jobs(queues, schedules, buckets)
        .expect("job resources should bind")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/jobs"
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
    let mut options = BuildOptions::new("2026", "test/jobs", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("source should compile")
}

fn host(state: DurableState) -> AgentHost {
    AgentHost::new_with_state(
        HostInputs::new(BTreeMap::new(), SecretStore::default())
            .expect("inputs should be valid")
            .with_network_policy(NetworkPolicy::loopback_for_tests()),
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        state,
    )
    .expect("agent host should build")
}

fn store_queues() -> BTreeMap<String, StoreQueuePolicy> {
    BTreeMap::from([("render-jobs".to_owned(), queue_policy(2))])
}

fn store_buckets() -> BTreeMap<String, StoreBucketPolicy> {
    BTreeMap::from([("render-output".to_owned(), bucket_policy())])
}

/// Enqueues one job directly through the store so that worker tests do not
/// depend on the publisher artifact.
fn seed_job(database: &Path, id: u8, body: &str) {
    let store =
        DurableStore::open(database, Durability::Full, store_limits()).expect("store should open");
    let revision = store.revision().expect("revision should read");
    let mut identity = [0u8; 16];
    identity[0] = id;
    store
        .commit_plan(CommitPlan {
            expected_revision: revision,
            mutations: &[Mutation::QueuePublish {
                queue: "render-jobs".to_owned(),
                id: identity,
                body: body.as_bytes().to_vec(),
            }],
            queues: &store_queues(),
            buckets: &store_buckets(),
            now_millis: 1_000,
            completion: None,
        })
        .expect("seed publish should commit");
}

fn wall_clock_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("host clock should be after the UNIX epoch")
            .as_millis(),
    )
    .expect("host clock should fit in i64")
}

fn open_store(database: &Path) -> DurableStore {
    DurableStore::open(database, Durability::Full, store_limits()).expect("store should open")
}

const WORKER_SOURCE: &str = r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_get("render-output", job.id) {
        Ok(existing) => match existing {
            Some(previous) => Ok(previous),
            None => match object_put("render-output", job.id, job.body) {
                Ok(stored) => match checkpoint_put("agent-work", "last-render", job.id) {
                    Ok(marked) => Ok(job.body),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            },
        },
        Err(error) => Err(error),
    }
}
"#;

const WORKER_EFFECTS: &[&str] = &[
    "object.read",
    "object.write",
    "queue.consume",
    "state.transaction",
];

const WORKER_CAPABILITIES: &str = r#"
buckets = ["render-output"]
consumes = ["render-jobs"]
state = ["agent-work"]
"#;

#[test]
fn webhook_publish_worker_and_object_outcome_commit_across_processes() {
    let directory = TestDirectory::new("reference");
    let database = directory.database();
    let publisher = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["queue.publish"],
    );
    let publisher_manifest = manifest("queues = [\"render-jobs\"]\n");
    let runtime = Runtime::default();

    let response = runtime
        .invoke_webhook_with_host(
            &publisher.bytes,
            &publisher.metadata,
            &GrantSet::from_manifest(&publisher_manifest),
            &host(durable(
                database.clone(),
                Resources {
                    buckets: false,
                    ..Resources::default()
                },
            )),
            krit_runtime::HttpRequest {
                method: "POST".to_owned(),
                path: "/render".to_owned(),
                query: String::new(),
                headers: Vec::new(),
                body: "payload".to_owned(),
            },
        )
        .expect("publish should succeed");
    assert_eq!(response.response.status, 202);
    assert_eq!(response.stats.queue_publishes, 1);
    let identity = response.response.body.clone();
    assert_eq!(identity.len(), 32);
    assert!(identity.chars().all(|value| value.is_ascii_hexdigit()));

    assert_eq!(
        open_store(&database).queue_stats("render-jobs").unwrap().0,
        1
    );

    let worker = compile(WORKER_SOURCE, WORKER_EFFECTS);
    let worker_manifest = manifest(WORKER_CAPABILITIES);
    // The publisher stamps visibility with the host wall clock, so the worker
    // dispatches with the same clock rather than a synthetic instant.
    let now = wall_clock_millis();
    let result = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&worker_manifest),
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: now,
            cancellation: &CancellationHandle::new(),
        })
        .expect("worker should dispatch");

    assert_eq!(
        result.outcome,
        DeliveryOutcome::Completed {
            id: identity.clone(),
            attempt: 1,
        }
    );
    assert_eq!(result.detail, "payload");
    assert_eq!(result.stats.object_writes, 1);

    let store = open_store(&database);
    assert_eq!(store.queue_stats("render-jobs").unwrap(), (0, 0));
    assert_eq!(
        store.object("render-output", &identity).unwrap().unwrap(),
        b"payload"
    );
    assert_eq!(
        store.checkpoint("last-render").unwrap().unwrap(),
        identity.as_bytes()
    );

    let idle = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&worker_manifest),
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: now,
            cancellation: &CancellationHandle::new(),
        })
        .expect("idle dispatch should succeed");
    assert!(idle.outcome.is_idle());
    assert_eq!(store.object_stats("render-output").unwrap().0, 1);
}

#[test]
fn guest_failure_retries_then_succeeds_without_partial_state() {
    let directory = TestDirectory::new("retry");
    let database = directory.database();
    seed_job(&database, 1, "retry-me");
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_put("render-output", job.id, job.body) {
        Ok(stored) => if job.attempt < 2 { Err("not yet") } else { Ok(job.body) },
        Err(error) => Err(error),
    }
}
"#,
        WORKER_EFFECTS,
    );
    let worker_manifest = manifest(WORKER_CAPABILITIES);
    let grants = GrantSet::from_manifest(&worker_manifest);

    let first = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("first attempt should dispatch");
    assert!(matches!(
        first.outcome,
        DeliveryOutcome::Retried {
            attempt: 1,
            visible_at_millis: 3_000,
            ..
        }
    ));
    assert_eq!(first.detail, "not yet");
    let store = open_store(&database);
    assert_eq!(
        store.object_stats("render-output").unwrap(),
        (0, 0),
        "a failed delivery must not commit its staged object"
    );

    let second = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: 4_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("second attempt should dispatch");
    assert!(matches!(
        second.outcome,
        DeliveryOutcome::Completed { attempt: 2, .. }
    ));
    assert_eq!(store.object_stats("render-output").unwrap().0, 1);
    assert_eq!(store.queue_stats("render-jobs").unwrap(), (0, 0));
}

#[test]
fn exhausted_attempts_dead_letter_and_stop_redelivering() {
    let directory = TestDirectory::new("dead-letter");
    let database = directory.database();
    seed_job(&database, 1, "always-fails");
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    Err(job.body)
}
"#,
        &["queue.consume"],
    );
    let worker_manifest = manifest("consumes = [\"render-jobs\"]\n");
    let grants = GrantSet::from_manifest(&worker_manifest);
    let runtime = Runtime::default();

    let mut outcomes = Vec::new();
    for now in [2_000, 4_000, 8_000] {
        outcomes.push(
            runtime
                .dispatch_job(DeliveryRequest {
                    bytes: &worker.bytes,
                    metadata: &worker.metadata,
                    grants: &grants,
                    agent_host: &host(durable(
                        database.clone(),
                        Resources {
                            buckets: false,
                            ..Resources::default()
                        },
                    )),
                    resource: "render-jobs",
                    now_millis: now,
                    cancellation: &CancellationHandle::new(),
                })
                .expect("dispatch should succeed")
                .outcome,
        );
    }

    assert!(matches!(outcomes[0], DeliveryOutcome::Retried { .. }));
    assert!(matches!(
        outcomes[1],
        DeliveryOutcome::DeadLettered { attempt: 2, .. }
    ));
    assert!(outcomes[2].is_idle());

    let store = open_store(&database);
    assert_eq!(store.queue_stats("render-jobs").unwrap(), (0, 0));
    let dead = store.dead_letters("render-jobs", 8).unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].attempts, 2);
    assert_eq!(dead[0].reason, "always-fails");
}

#[test]
fn an_interrupted_worker_lease_recovers_without_losing_the_job() {
    let directory = TestDirectory::new("lease-recovery");
    let database = directory.database();
    seed_job(&database, 1, "interrupted");

    let store = open_store(&database);
    let abandoned = store
        .reserve_job("render-jobs", queue_policy(2), &[9u8; 16], 2_000)
        .unwrap()
        .expect("a killed worker holds one lease");
    drop(abandoned);
    drop(store);

    let worker = compile(WORKER_SOURCE, WORKER_EFFECTS);
    let worker_manifest = manifest(WORKER_CAPABILITIES);
    let grants = GrantSet::from_manifest(&worker_manifest);
    let runtime = Runtime::default();

    let blocked = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: 3_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("dispatch should succeed");
    assert!(
        blocked.outcome.is_idle(),
        "a live lease must not be redelivered"
    );

    let recovered = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: 60_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("dispatch should succeed");
    assert!(matches!(
        recovered.outcome,
        DeliveryOutcome::Completed { attempt: 2, .. }
    ));
    assert_eq!(
        open_store(&database)
            .object_stats("render-output")
            .unwrap()
            .0,
        1
    );
}

#[test]
fn traps_and_cancellation_roll_back_and_record_one_bounded_attempt() {
    let directory = TestDirectory::new("trap");
    let database = directory.database();
    seed_job(&database, 1, "trap-me");
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_put("render-output", job.id, job.body) {
        Ok(stored) => Ok(job.body),
        Err(error) => Err(error),
    }
}
"#,
        WORKER_EFFECTS,
    );
    let worker_manifest = manifest(WORKER_CAPABILITIES);
    let grants = GrantSet::from_manifest(&worker_manifest);
    let cancellation = CancellationHandle::new();
    cancellation.cancel();

    let error = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &cancellation,
        })
        .expect_err("cancelled dispatch must fail closed");
    assert_eq!(error.code(), "K5106");

    let store = open_store(&database);
    assert_eq!(store.object_stats("render-output").unwrap(), (0, 0));
    assert_eq!(
        store.queue_stats("render-jobs").unwrap().0,
        1,
        "a cancelled delivery keeps the job durable"
    );
}

#[test]
fn schedule_triggers_fire_once_recover_and_persist_objects() {
    let directory = TestDirectory::new("schedule");
    let database = directory.database();
    let handler = compile(
        r#"
schedule "hourly-sweep" fn handle(event: ScheduleEvent) -> Result<String, String> {
    match object_put("render-output", event.id, event.schedule) {
        Ok(stored) => Ok(event.id),
        Err(error) => Err(error),
    }
}
"#,
        &["object.write", "schedule.trigger"],
    );
    let handler_manifest = manifest(
        r#"
buckets = ["render-output"]
schedules = ["hourly-sweep"]
"#,
    );
    let grants = GrantSet::from_manifest(&handler_manifest);
    let resources = || Resources {
        queues: false,
        schedules: true,
        buckets: true,
        max_attempts: 2,
    };
    let runtime = Runtime::default();

    let (catch_up, first) = runtime
        .dispatch_schedule(DeliveryRequest {
            bytes: &handler.bytes,
            metadata: &handler.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), resources())),
            resource: "hourly-sweep",
            now_millis: 120_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("first tick should dispatch");
    assert_eq!(catch_up.materialized, 1);
    assert_eq!(
        first.outcome,
        DeliveryOutcome::Completed {
            id: "hourly-sweep@120000".to_owned(),
            attempt: 1,
        }
    );

    let (_, repeated) = runtime
        .dispatch_schedule(DeliveryRequest {
            bytes: &handler.bytes,
            metadata: &handler.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), resources())),
            resource: "hourly-sweep",
            now_millis: 120_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("duplicate tick should dispatch");
    assert!(
        repeated.outcome.is_idle(),
        "a committed fire must not repeat on the same instant"
    );

    let (catch_up, second) = runtime
        .dispatch_schedule(DeliveryRequest {
            bytes: &handler.bytes,
            metadata: &handler.metadata,
            grants: &grants,
            agent_host: &host(durable(database.clone(), resources())),
            resource: "hourly-sweep",
            now_millis: 600_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("catch-up tick should dispatch");
    assert_eq!(catch_up.materialized, 2);
    assert_eq!(catch_up.skipped, 6);
    assert!(matches!(second.outcome, DeliveryOutcome::Completed { .. }));

    let store = open_store(&database);
    assert_eq!(store.schedule_stats("hourly-sweep").unwrap().1, 2);
    assert_eq!(store.object_stats("render-output").unwrap().0, 2);
}

#[test]
fn ungranted_or_unconfigured_delivery_resources_fail_closed() {
    let directory = TestDirectory::new("permissions");
    let database = directory.database();
    let worker = compile(WORKER_SOURCE, WORKER_EFFECTS);
    let runtime = Runtime::default();

    let ungranted = manifest(
        r#"
buckets = ["render-output"]
state = ["agent-work"]
"#,
    );
    let error = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&ungranted),
            agent_host: &host(durable(database.clone(), Resources::default())),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("an ungranted consume must fail closed");
    assert_eq!(error.code(), "K5001");

    let granted = manifest(WORKER_CAPABILITIES);
    let error = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&granted),
            agent_host: &host(durable(
                database.clone(),
                Resources {
                    queues: false,
                    ..Resources::default()
                },
            )),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("an unconfigured queue must fail closed");
    assert_eq!(error.code(), "K5001");

    let publisher = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match queue_publish("render-jobs", request.body) {
        Ok(id) => record { status: 202, headers: [], body: id },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["queue.publish"],
    );
    let error = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &publisher.bytes,
            metadata: &publisher.metadata,
            grants: &GrantSet::from_manifest(&manifest("queues = [\"render-jobs\"]\n")),
            agent_host: &host(durable(
                database,
                Resources {
                    buckets: false,
                    ..Resources::default()
                },
            )),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("a webhook artifact must not dispatch as a worker");
    assert_eq!(error.code(), "K5002");
}

#[test]
fn object_bounds_and_bucket_isolation_are_enforced_at_the_boundary() {
    let directory = TestDirectory::new("object-bounds");
    let database = directory.database();
    seed_job(&database, 1, "x");
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match object_put("render-output", job.id, job.body) {
        Ok(stored) => match object_get("render-output", "missing-key") {
            Ok(found) => match found {
                Some(value) => Ok(value),
                None => Ok("absent"),
            },
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}
"#,
        WORKER_EFFECTS,
    );
    let state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: database.clone(),
                durability: Durability::Full,
                limits: store_limits(),
                replay: retention(),
            },
        )]),
        None,
    )
    .expect("durable state should open")
    .with_jobs(
        BTreeMap::from([(
            "render-jobs".to_owned(),
            QueueDefinition {
                store: "agent-work".to_owned(),
                policy: queue_policy(2),
            },
        )]),
        BTreeMap::new(),
        BTreeMap::from([(
            "render-output".to_owned(),
            BucketDefinition {
                store: "agent-work".to_owned(),
                policy: BucketPolicy {
                    max_objects: 1,
                    max_key_bytes: 4,
                    max_object_bytes: 8,
                    max_bucket_bytes: 16,
                },
            },
        )]),
    )
    .expect("job resources should bind");

    let error = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&manifest(WORKER_CAPABILITIES)),
            agent_host: &host(state),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("an oversized object key must trap the delivery");
    assert_eq!(error.code(), "K5202");

    assert_eq!(
        open_store(&database).object_stats("render-output").unwrap(),
        (0, 0)
    );
}

#[test]
fn replayed_external_effects_are_not_repeated_after_an_interrupted_delivery() {
    let directory = TestDirectory::new("replay");
    let database = directory.database();
    seed_job(&database, 1, "issue-1");
    let calls = Arc::new(AtomicU64::new(0));
    let (origin, mock) = spawn_counting_mock(Arc::clone(&calls));
    let worker = compile(
        &format!(
            r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {{
    match replay_http(
        "agent-work",
        "fetch-issue",
        "{origin}",
        record {{
            method: "GET",
            path: "/issue",
            query: "",
            headers: [],
            body: "",
        }},
    ) {{
        Ok(response) => match checkpoint_get("agent-work", "posted") {{
            Ok(previous) => match previous {{
                Some(value) => Ok(value),
                None => if job.attempt < 2 {{
                    Err("interrupted before commit")
                }} else {{
                    match checkpoint_put("agent-work", "posted", response.body) {{
                        Ok(marked) => Ok(response.body),
                        Err(error) => Err(error),
                    }}
                }},
            }},
            Err(error) => Err(error),
        }},
        Err(error) => Err(error),
    }}
}}
"#
        ),
        &["queue.consume", "state.transaction"],
    );
    let worker_manifest = manifest(&format!(
        r#"
consumes = ["render-jobs"]
http = ["{origin}"]
state = ["agent-work"]
"#
    ));
    let grants = GrantSet::from_manifest(&worker_manifest);
    let runtime = Runtime::default();

    let first = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(
                database.clone(),
                Resources {
                    buckets: false,
                    ..Resources::default()
                },
            )),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("first attempt should dispatch");
    assert!(matches!(first.outcome, DeliveryOutcome::Retried { .. }));
    assert_eq!(first.stats.replay_misses, 1);
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(
        open_store(&database)
            .checkpoint("posted")
            .unwrap()
            .is_none()
    );

    let second = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(durable(
                database.clone(),
                Resources {
                    buckets: false,
                    ..Resources::default()
                },
            )),
            resource: "render-jobs",
            now_millis: 4_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("second attempt should dispatch");
    assert!(matches!(
        second.outcome,
        DeliveryOutcome::Completed { attempt: 2, .. }
    ));
    assert_eq!(second.stats.replay_hits, 1);
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "a completed external effect must not repeat after recovery"
    );
    assert_eq!(
        open_store(&database).checkpoint("posted").unwrap().unwrap(),
        b"ok"
    );
    mock.join().expect("mock should finish");
}

fn spawn_counting_mock(calls: Arc<AtomicU64>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener should bind");
    let address = listener.local_addr().expect("mock address should exist");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        calls.fetch_add(1, Ordering::AcqRel);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("mock timeout should configure");
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    });
    (format!("http://127.0.0.1:{}", address.port()), handle)
}

// ---------------------------------------------------------------------------
// Regressions for the phase6-jobs-storage review findings.
// ---------------------------------------------------------------------------

fn state_with_leases(
    database: PathBuf,
    queue_lease: Duration,
    schedule_lease: Duration,
) -> DurableState {
    DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: database,
                durability: Durability::Full,
                limits: store_limits(),
                replay: retention(),
            },
        )]),
        None,
    )
    .expect("durable state should open")
    .with_jobs(
        BTreeMap::from([(
            "render-jobs".to_owned(),
            QueueDefinition {
                store: "agent-work".to_owned(),
                policy: QueuePolicy {
                    lease: queue_lease,
                    ..queue_policy(2)
                },
            },
        )]),
        BTreeMap::from([(
            "hourly-sweep".to_owned(),
            ScheduleDefinition {
                store: "agent-work".to_owned(),
                policy: SchedulePolicy {
                    lease: schedule_lease,
                    ..schedule_policy()
                },
            },
        )]),
        BTreeMap::new(),
    )
    .expect("job resources should bind")
}

/// Finding 2: a delivery lease shorter than one complete guest execution is
/// refused, because two workers could otherwise hold the same job at once.
#[test]
fn delivery_leases_must_cover_the_execution_deadline_and_busy_timeout() {
    let directory = TestDirectory::new("lease-bound");
    let database = directory.database();
    // A consume-only worker keeps this test focused on lease validation.
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    Ok(job.id)
}
"#,
        &["queue.consume"],
    );
    let worker_manifest = manifest(
        r#"
consumes = ["render-jobs"]
schedules = ["hourly-sweep"]
"#,
    );
    let grants = GrantSet::from_manifest(&worker_manifest);
    let runtime = Runtime::default();
    let minimum = runtime.limits().deadline() + store_limits().busy_timeout;

    let below = minimum - Duration::from_millis(1);
    let error = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(state_with_leases(database.clone(), below, minimum)),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("a queue lease below the execution bound must be refused");
    assert_eq!(error.code(), "K5201");
    assert!(error.message().contains("queue lease"));

    let schedule_error = runtime
        .dispatch_schedule(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(state_with_leases(database.clone(), minimum, below)),
            resource: "hourly-sweep",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("a schedule lease below the execution bound must be refused");
    assert_eq!(schedule_error.code(), "K5201");
    assert!(schedule_error.message().contains("schedule lease"));

    // Exactly at the bound the host accepts the configuration and dispatches.
    let accepted = runtime
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &grants,
            agent_host: &host(state_with_leases(database, minimum, minimum)),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("a lease at the execution bound must be accepted");
    assert!(accepted.outcome.is_idle());
}

/// Finding 7: staged depth is charged per queue, so an atomic fan-out to two
/// depth-one queues commits.
#[test]
fn atomic_fan_out_charges_each_queue_its_own_depth() {
    let directory = TestDirectory::new("fan-out");
    let database = directory.database();
    seed_job(&database, 1, "fan-out");
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match queue_publish("thumbnails", job.body) {
        Ok(first) => match queue_publish("indexing", job.body) {
            Ok(second) => Ok(second),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}
"#,
        &["queue.consume", "queue.publish"],
    );
    let worker_manifest = manifest(
        r#"
consumes = ["render-jobs"]
queues = ["indexing", "thumbnails"]
"#,
    );
    let single_depth = |name: &str| {
        (
            name.to_owned(),
            QueueDefinition {
                store: "agent-work".to_owned(),
                policy: QueuePolicy {
                    max_depth: 1,
                    ..queue_policy(2)
                },
            },
        )
    };
    let state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: database.clone(),
                durability: Durability::Full,
                limits: store_limits(),
                replay: retention(),
            },
        )]),
        None,
    )
    .expect("durable state should open")
    .with_jobs(
        BTreeMap::from([
            (
                "render-jobs".to_owned(),
                QueueDefinition {
                    store: "agent-work".to_owned(),
                    policy: queue_policy(2),
                },
            ),
            single_depth("thumbnails"),
            single_depth("indexing"),
        ]),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .expect("job resources should bind");

    let result = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&worker_manifest),
            agent_host: &host(state),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect("fan-out should dispatch");

    assert!(matches!(
        result.outcome,
        DeliveryOutcome::Completed { attempt: 1, .. }
    ));
    assert_eq!(result.stats.queue_publishes, 2);
    let store = open_store(&database);
    assert_eq!(store.queue_stats("thumbnails").unwrap().0, 1);
    assert_eq!(store.queue_stats("indexing").unwrap().0, 1);
    assert_eq!(store.queue_stats("render-jobs").unwrap(), (0, 0));
}

/// Finding 7: the per-queue bound is still enforced for repeated publications
/// to one queue.
#[test]
fn repeated_publications_to_one_queue_still_hit_its_depth_bound() {
    let directory = TestDirectory::new("fan-out-bound");
    let database = directory.database();
    seed_job(&database, 1, "overflow");
    let worker = compile(
        r#"
queue "render-jobs" fn handle(job: QueueJob) -> Result<String, String> {
    match queue_publish("thumbnails", job.body) {
        Ok(first) => match queue_publish("thumbnails", job.body) {
            Ok(second) => Ok(second),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}
"#,
        &["queue.consume", "queue.publish"],
    );
    let worker_manifest = manifest(
        r#"
consumes = ["render-jobs"]
queues = ["thumbnails"]
"#,
    );
    let state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: database.clone(),
                durability: Durability::Full,
                limits: store_limits(),
                replay: retention(),
            },
        )]),
        None,
    )
    .expect("durable state should open")
    .with_jobs(
        BTreeMap::from([
            (
                "render-jobs".to_owned(),
                QueueDefinition {
                    store: "agent-work".to_owned(),
                    policy: queue_policy(2),
                },
            ),
            (
                "thumbnails".to_owned(),
                QueueDefinition {
                    store: "agent-work".to_owned(),
                    policy: QueuePolicy {
                        max_depth: 1,
                        ..queue_policy(2)
                    },
                },
            ),
        ]),
        BTreeMap::new(),
        BTreeMap::new(),
    )
    .expect("job resources should bind");

    let error = Runtime::default()
        .dispatch_job(DeliveryRequest {
            bytes: &worker.bytes,
            metadata: &worker.metadata,
            grants: &GrantSet::from_manifest(&worker_manifest),
            agent_host: &host(state),
            resource: "render-jobs",
            now_millis: 2_000,
            cancellation: &CancellationHandle::new(),
        })
        .expect_err("exceeding one queue's depth must trap the delivery");
    assert_eq!(error.code(), "K5202");
    assert_eq!(
        open_store(&database).queue_stats("thumbnails").unwrap().0,
        0
    );
}
