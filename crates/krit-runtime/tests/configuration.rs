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

use krit_runtime::{
    AgentHost, AgentHostPolicy, AgentHostServices, BucketDefinition, DenyAllApprovalPolicy,
    Durability, DurableState, DurableStoreDefinition, HostInputs, QueueDefinition, RuntimeError,
    ScheduleDefinition, SecretStore,
};
use krit_state::{
    BucketPolicy, CommitPlan, DurableStore, MAX_BUCKET_BYTES, MAX_BUCKET_OBJECTS,
    MAX_DEAD_LETTER_ENTRIES, MAX_DEAD_LETTER_RETENTION, MAX_DELIVERY_ATTEMPTS,
    MAX_DELIVERY_BACKOFF, MAX_DELIVERY_LEASE, MAX_OBJECT_BYTES, MAX_OBJECT_KEY_BYTES,
    MAX_QUEUE_BYTES, MAX_QUEUE_DEPTH, MAX_QUEUE_JOB_BYTES, MAX_REPLAY_BYTES, MAX_REPLAY_ENTRIES,
    MAX_REPLAY_LEASE, MAX_REPLAY_RESULT_BYTES, MAX_REPLAY_TTL, MAX_RETAINED_FIRES,
    MAX_SCHEDULE_CATCH_UP, MAX_SCHEDULE_INTERVAL, MAX_SCHEDULE_RETENTION, MAX_STATE_BUSY_TIMEOUT,
    MAX_STATE_DATABASE_BYTES, MAX_STATE_KEY_BYTES, MAX_STATE_OPERATIONS,
    MAX_STATE_TRANSACTION_BYTES, MAX_STATE_VALUE_BYTES, MIN_SCHEDULE_INTERVAL,
    MINIMUM_DATABASE_BYTES, Mutation, QueuePolicy, ReplayDecision, ReplayKind, ReplayRequest,
    RetentionPolicy, SchedulePolicy, StateErrorKind, StoreLimits,
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!(
            ".krit-configuration-test-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn maximum_store_limits() -> StoreLimits {
    StoreLimits {
        busy_timeout: MAX_STATE_BUSY_TIMEOUT,
        max_operations: MAX_STATE_OPERATIONS,
        max_key_bytes: MAX_STATE_KEY_BYTES,
        max_value_bytes: MAX_STATE_VALUE_BYTES,
        max_transaction_bytes: MAX_STATE_TRANSACTION_BYTES,
        max_database_bytes: MAX_STATE_DATABASE_BYTES,
        max_replay_entries: MAX_REPLAY_ENTRIES,
        max_replay_bytes: MAX_REPLAY_BYTES,
    }
}

fn maximum_retention() -> RetentionPolicy {
    RetentionPolicy {
        max_entries: MAX_REPLAY_ENTRIES,
        max_bytes: MAX_REPLAY_BYTES,
        ttl: MAX_REPLAY_TTL,
        lease: MAX_REPLAY_LEASE,
    }
}

fn maximum_queue() -> QueuePolicy {
    QueuePolicy {
        max_depth: MAX_QUEUE_DEPTH,
        max_job_bytes: MAX_QUEUE_JOB_BYTES,
        max_queue_bytes: MAX_QUEUE_BYTES,
        max_attempts: MAX_DELIVERY_ATTEMPTS,
        lease: MAX_DELIVERY_LEASE,
        backoff: MAX_DELIVERY_BACKOFF,
        max_backoff: MAX_DELIVERY_BACKOFF,
        dead_letter_max_entries: MAX_DEAD_LETTER_ENTRIES,
        dead_letter_ttl: MAX_DEAD_LETTER_RETENTION,
    }
}

fn maximum_schedule() -> SchedulePolicy {
    SchedulePolicy {
        interval: MAX_SCHEDULE_INTERVAL,
        start_millis: i64::MAX,
        max_catch_up: MAX_SCHEDULE_CATCH_UP,
        max_attempts: MAX_DELIVERY_ATTEMPTS,
        lease: MAX_DELIVERY_LEASE,
        backoff: MAX_DELIVERY_BACKOFF,
        max_backoff: MAX_DELIVERY_BACKOFF,
        retention: MAX_SCHEDULE_RETENTION,
        max_retained_fires: MAX_RETAINED_FIRES,
    }
}

fn maximum_bucket() -> BucketPolicy {
    BucketPolicy {
        max_objects: MAX_BUCKET_OBJECTS,
        max_key_bytes: MAX_OBJECT_KEY_BYTES,
        max_object_bytes: MAX_OBJECT_BYTES,
        max_bucket_bytes: MAX_BUCKET_BYTES,
    }
}

macro_rules! invalid_policies {
    ($base:expr; $($field:ident: $value:expr),+ $(,)?) => {
        vec![$((stringify!($field), { let mut policy = $base; policy.$field = $value; policy })),+]
    };
}

fn invalid_store_limits() -> Vec<(&'static str, StoreLimits)> {
    invalid_policies!(maximum_store_limits();
        busy_timeout: MAX_STATE_BUSY_TIMEOUT + Duration::from_nanos(1),
        max_operations: MAX_STATE_OPERATIONS + 1,
        max_key_bytes: MAX_STATE_KEY_BYTES + 1,
        max_value_bytes: MAX_STATE_VALUE_BYTES + 1,
        max_transaction_bytes: MAX_STATE_TRANSACTION_BYTES + 1,
        max_database_bytes: MAX_STATE_DATABASE_BYTES + 1,
        max_replay_entries: MAX_REPLAY_ENTRIES + 1,
        max_replay_bytes: MAX_REPLAY_BYTES + 1,
        busy_timeout: Duration::ZERO,
        busy_timeout: Duration::from_nanos(1),
        busy_timeout: Duration::from_micros(1500),
        max_operations: 0,
        max_key_bytes: 0,
        max_value_bytes: 0,
        max_transaction_bytes: 0,
        max_database_bytes: MINIMUM_DATABASE_BYTES - 1,
        max_replay_entries: 0,
        max_replay_bytes: 0,
    )
}

fn invalid_retention() -> Vec<(&'static str, RetentionPolicy)> {
    invalid_policies!(maximum_retention();
        max_entries: MAX_REPLAY_ENTRIES + 1,
        max_bytes: MAX_REPLAY_BYTES + 1,
        ttl: MAX_REPLAY_TTL + Duration::from_nanos(1),
        lease: MAX_REPLAY_LEASE + Duration::from_nanos(1),
        max_entries: 0,
        max_bytes: 0,
        ttl: Duration::ZERO,
        lease: Duration::ZERO,
        lease: Duration::from_nanos(1),
        lease: Duration::from_micros(1500),
        ttl: Duration::from_nanos(1),
    )
}

fn invalid_queues() -> Vec<(&'static str, QueuePolicy)> {
    invalid_policies!(maximum_queue();
        max_depth: MAX_QUEUE_DEPTH + 1,
        max_job_bytes: MAX_QUEUE_JOB_BYTES + 1,
        max_queue_bytes: MAX_QUEUE_BYTES + 1,
        max_attempts: MAX_DELIVERY_ATTEMPTS + 1,
        lease: MAX_DELIVERY_LEASE + Duration::from_nanos(1),
        backoff: MAX_DELIVERY_BACKOFF + Duration::from_nanos(1),
        max_backoff: MAX_DELIVERY_BACKOFF + Duration::from_nanos(1),
        dead_letter_max_entries: MAX_DEAD_LETTER_ENTRIES + 1,
        dead_letter_ttl: MAX_DEAD_LETTER_RETENTION + Duration::from_nanos(1),
        max_depth: 0,
        max_job_bytes: 0,
        max_queue_bytes: MAX_QUEUE_JOB_BYTES - 1,
        max_attempts: 0,
        lease: Duration::ZERO,
        lease: Duration::from_nanos(1),
        lease: Duration::from_micros(1500),
        backoff: Duration::ZERO,
        backoff: Duration::from_nanos(1),
        max_backoff: MAX_DELIVERY_BACKOFF - Duration::from_millis(1),
        dead_letter_max_entries: 0,
        dead_letter_ttl: Duration::ZERO,
    )
}

fn invalid_schedules() -> Vec<(&'static str, SchedulePolicy)> {
    invalid_policies!(maximum_schedule();
        interval: MAX_SCHEDULE_INTERVAL + Duration::from_nanos(1),
        max_catch_up: MAX_SCHEDULE_CATCH_UP + 1,
        max_attempts: MAX_DELIVERY_ATTEMPTS + 1,
        lease: MAX_DELIVERY_LEASE + Duration::from_nanos(1),
        backoff: MAX_DELIVERY_BACKOFF + Duration::from_nanos(1),
        max_backoff: MAX_DELIVERY_BACKOFF + Duration::from_nanos(1),
        retention: MAX_SCHEDULE_RETENTION + Duration::from_nanos(1),
        max_retained_fires: MAX_RETAINED_FIRES + 1,
        interval: MIN_SCHEDULE_INTERVAL - Duration::from_nanos(1),
        start_millis: -1,
        max_catch_up: 0,
        max_attempts: 0,
        lease: Duration::ZERO,
        lease: Duration::from_nanos(1),
        lease: Duration::from_micros(1500),
        backoff: Duration::ZERO,
        max_backoff: MAX_DELIVERY_BACKOFF - Duration::from_millis(1),
        retention: Duration::ZERO,
        max_retained_fires: 0,
    )
}

fn invalid_buckets() -> Vec<(&'static str, BucketPolicy)> {
    invalid_policies!(maximum_bucket();
        max_objects: MAX_BUCKET_OBJECTS + 1,
        max_key_bytes: MAX_OBJECT_KEY_BYTES + 1,
        max_object_bytes: MAX_OBJECT_BYTES + 1,
        max_bucket_bytes: MAX_BUCKET_BYTES + 1,
        max_objects: 0,
        max_key_bytes: 0,
        max_object_bytes: 0,
        max_bucket_bytes: MAX_OBJECT_BYTES - 1,
    )
}

#[derive(Clone)]
struct Configuration {
    stores: BTreeMap<String, DurableStoreDefinition>,
    idempotency: Option<String>,
    queues: BTreeMap<String, QueueDefinition>,
    schedules: BTreeMap<String, ScheduleDefinition>,
    buckets: BTreeMap<String, BucketDefinition>,
}

impl Configuration {
    fn new(directory: &TestDirectory) -> Self {
        let definition = |name| DurableStoreDefinition {
            path: directory.path(name),
            durability: Durability::Full,
            limits: maximum_store_limits(),
            replay: maximum_retention(),
        };
        Self {
            stores: BTreeMap::from([
                ("a-existing".to_owned(), definition("existing.db")),
                ("b-new".to_owned(), definition("new.db")),
                ("z-invalid".to_owned(), definition("invalid.db")),
            ]),
            idempotency: Some("a-existing".to_owned()),
            queues: BTreeMap::from([(
                "queue".to_owned(),
                QueueDefinition {
                    store: "a-existing".to_owned(),
                    policy: maximum_queue(),
                },
            )]),
            schedules: BTreeMap::from([(
                "schedule".to_owned(),
                ScheduleDefinition {
                    store: "a-existing".to_owned(),
                    policy: maximum_schedule(),
                },
            )]),
            buckets: BTreeMap::from([(
                "bucket".to_owned(),
                BucketDefinition {
                    store: "a-existing".to_owned(),
                    policy: maximum_bucket(),
                },
            )]),
        }
    }

    fn open(self) -> Result<DurableState, RuntimeError> {
        DurableState::open_with_jobs(
            self.stores,
            self.idempotency,
            self.queues,
            self.schedules,
            self.buckets,
        )
    }

    fn host(self) -> Result<AgentHost, RuntimeError> {
        let services = AgentHostServices {
            durable_state: self.open()?,
            ..AgentHostServices::default()
        };
        AgentHost::new_with_services(
            HostInputs::new(BTreeMap::new(), SecretStore::default()).unwrap(),
            AgentHostPolicy::default(),
            Arc::new(DenyAllApprovalPolicy),
            services,
        )
    }

    fn validate_for_runtime(&self, deadline: Duration) -> Result<(), RuntimeError> {
        DurableState::validate_configuration_for_runtime(
            &self.stores,
            self.idempotency.as_deref(),
            &self.queues,
            &self.schedules,
            &self.buckets,
            deadline,
        )
    }

    fn store_mut(&mut self) -> &mut DurableStoreDefinition {
        self.stores.get_mut("z-invalid").unwrap()
    }
}

fn schema_one_fixture(path: &Path) -> Vec<u8> {
    drop(DurableStore::open(path, Durability::Full, maximum_store_limits()).unwrap());
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode = DELETE;
             DROP TABLE queue_jobs;
             DROP TABLE queue_dead;
             DROP TABLE schedule_fires;
             DROP TABLE schedule_cursors;
             DROP TABLE objects;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);
    fs::read(path).unwrap()
}

fn assert_no_side_effects(directory: &TestDirectory, before: &[u8]) {
    assert_eq!(fs::read(directory.path("existing.db")).unwrap(), before);
    let entries: Vec<_> = fs::read_dir(&directory.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["existing.db"]);
}

#[test]
fn direct_services_accept_every_protocol_maximum_inclusively() {
    let directory = TestDirectory::new();
    let mut configuration = Configuration::new(&directory);
    for index in configuration.stores.len()..DurableState::MAX_STORES {
        let mut definition = configuration.store_mut().clone();
        definition.path = directory.path(&format!("store-{index}.db"));
        configuration
            .stores
            .insert(format!("store-{index}"), definition);
    }
    for index in 1..DurableState::MAX_QUEUES {
        configuration.queues.insert(
            format!("queue-{index}"),
            configuration.queues["queue"].clone(),
        );
    }
    for index in 1..DurableState::MAX_SCHEDULES {
        configuration.schedules.insert(
            format!("schedule-{index}"),
            configuration.schedules["schedule"].clone(),
        );
    }
    for index in 1..DurableState::MAX_BUCKETS {
        configuration.buckets.insert(
            format!("bucket-{index}"),
            configuration.buckets["bucket"].clone(),
        );
    }
    configuration
        .validate_for_runtime(MAX_DELIVERY_LEASE - MAX_STATE_BUSY_TIMEOUT)
        .unwrap();
    let durable = configuration.clone().open().unwrap();
    assert_eq!(durable.store_names().len(), DurableState::MAX_STORES);
    assert_eq!(durable.queue_names().len(), DurableState::MAX_QUEUES);
    assert_eq!(durable.schedule_names().len(), DurableState::MAX_SCHEDULES);
    assert_eq!(durable.bucket_names().len(), DurableState::MAX_BUCKETS);
    drop(durable);
    drop(configuration.host().unwrap());
}

#[test]
fn every_invalid_store_and_retention_bound_precedes_creation_or_migration() {
    let directory = TestDirectory::new();
    let before = schema_one_fixture(&directory.path("existing.db"));
    for (field, limits) in invalid_store_limits() {
        assert_eq!(
            limits.validate().unwrap_err().kind(),
            StateErrorKind::Limit,
            "{field}"
        );
        for path in [directory.path("existing.db"), directory.path("new.db")] {
            let error = DurableStore::open(&path, Durability::Full, limits).unwrap_err();
            assert_eq!(error.kind(), StateErrorKind::Limit, "{field}");
            assert_no_side_effects(&directory, &before);
        }
        let mut configuration = Configuration::new(&directory);
        configuration.store_mut().limits = limits;
        assert_eq!(configuration.host().unwrap_err().code(), "K5202", "{field}");
        assert_no_side_effects(&directory, &before);
    }
    for (field, policy) in invalid_retention() {
        assert!(policy.validate(maximum_store_limits()).is_err(), "{field}");
        let mut configuration = Configuration::new(&directory);
        configuration.store_mut().replay = policy;
        assert_eq!(configuration.host().unwrap_err().code(), "K5202", "{field}");
        assert_no_side_effects(&directory, &before);
    }
    let mut configuration = Configuration::new(&directory);
    configuration.store_mut().limits.max_replay_entries = MAX_REPLAY_ENTRIES - 1;
    assert!(configuration.host().is_err());
    let mut configuration = Configuration::new(&directory);
    configuration.store_mut().limits.max_replay_bytes = MAX_REPLAY_BYTES - 1;
    assert!(configuration.host().is_err());
    assert_no_side_effects(&directory, &before);
}

#[test]
fn every_invalid_job_bound_precedes_creation_or_migration() {
    let directory = TestDirectory::new();
    let before = schema_one_fixture(&directory.path("existing.db"));
    for (field, policy) in invalid_queues() {
        assert!(policy.validate().is_err(), "{field}");
        let mut configuration = Configuration::new(&directory);
        configuration.queues.get_mut("queue").unwrap().policy = policy;
        assert_eq!(configuration.host().unwrap_err().code(), "K5202", "{field}");
        assert_no_side_effects(&directory, &before);
    }
    for (field, policy) in invalid_schedules() {
        assert!(policy.validate().is_err(), "{field}");
        let mut configuration = Configuration::new(&directory);
        configuration.schedules.get_mut("schedule").unwrap().policy = policy;
        assert_eq!(configuration.host().unwrap_err().code(), "K5202", "{field}");
        assert_no_side_effects(&directory, &before);
    }
    for (field, policy) in invalid_buckets() {
        assert!(policy.validate().is_err(), "{field}");
        let mut configuration = Configuration::new(&directory);
        configuration.buckets.get_mut("bucket").unwrap().policy = policy;
        assert_eq!(configuration.host().unwrap_err().code(), "K5202", "{field}");
        assert_no_side_effects(&directory, &before);
    }
}

#[test]
fn counts_names_and_bindings_are_rejected_before_opening_any_store() {
    let directory = TestDirectory::new();
    let before = schema_one_fixture(&directory.path("existing.db"));
    for family in ["stores", "queues", "schedules", "buckets"] {
        let mut configuration = Configuration::new(&directory);
        match family {
            "stores" => {
                let definition = configuration.store_mut().clone();
                configuration.stores = (0..=DurableState::MAX_STORES)
                    .map(|index| (format!("store-{index}"), definition.clone()))
                    .collect();
                configuration.idempotency = None;
                configuration.queues.clear();
                configuration.schedules.clear();
                configuration.buckets.clear();
            }
            "queues" => {
                let definition = configuration.queues["queue"].clone();
                configuration.queues = (0..=DurableState::MAX_QUEUES)
                    .map(|index| (format!("queue-{index}"), definition.clone()))
                    .collect();
            }
            "schedules" => {
                let definition = configuration.schedules["schedule"].clone();
                configuration.schedules = (0..=DurableState::MAX_SCHEDULES)
                    .map(|index| (format!("schedule-{index}"), definition.clone()))
                    .collect();
            }
            "buckets" => {
                let definition = configuration.buckets["bucket"].clone();
                configuration.buckets = (0..=DurableState::MAX_BUCKETS)
                    .map(|index| (format!("bucket-{index}"), definition.clone()))
                    .collect();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            configuration.host().unwrap_err().code(),
            "K5202",
            "{family}"
        );
        assert_no_side_effects(&directory, &before);
    }
    let mut configuration = Configuration::new(&directory);
    configuration.idempotency = Some("missing".to_owned());
    assert!(configuration.clone().host().is_err());
    assert!(DurableState::open(configuration.stores, configuration.idempotency).is_err());
    let mut configuration = Configuration::new(&directory);
    configuration.queues.get_mut("queue").unwrap().store = "missing".to_owned();
    assert!(configuration.host().is_err());
    let mut configuration = Configuration::new(&directory);
    let definition = configuration.store_mut().clone();
    configuration
        .stores
        .insert("INVALID".to_owned(), definition);
    assert!(configuration.host().is_err());
    assert_no_side_effects(&directory, &before);
}

#[test]
fn lower_operations_and_existing_bindings_cannot_bypass_job_validation() {
    let directory = TestDirectory::new();
    let store = DurableStore::open(
        &directory.path("store.db"),
        Durability::Full,
        maximum_store_limits(),
    )
    .unwrap();
    let state = Configuration::new(&directory).open().unwrap();
    let mutation = [Mutation::Put {
        key: "untouched".to_owned(),
        value: b"value".to_vec(),
    }];
    for (field, policy) in invalid_queues() {
        let queues = BTreeMap::from([("queue".to_owned(), policy)]);
        assert!(
            store.reserve_job("queue", policy, &[0; 16], 0).is_err(),
            "{field}"
        );
        assert!(
            store
                .commit_plan(CommitPlan {
                    expected_revision: 0,
                    mutations: &mutation,
                    queues: &queues,
                    buckets: &BTreeMap::new(),
                    now_millis: 0,
                    completion: None,
                })
                .is_err(),
            "{field}"
        );
        assert!(
            state
                .clone()
                .with_jobs(
                    BTreeMap::from([(
                        "queue".to_owned(),
                        QueueDefinition {
                            store: "a-existing".to_owned(),
                            policy,
                        }
                    )]),
                    BTreeMap::new(),
                    BTreeMap::new(),
                )
                .is_err(),
            "{field}"
        );
    }
    for (field, policy) in invalid_schedules() {
        assert!(
            store.materialize_schedule("schedule", policy, 0).is_err(),
            "{field}"
        );
        assert!(
            store
                .reserve_schedule_fire("schedule", policy, &[0; 16], 0)
                .is_err(),
            "{field}"
        );
        assert!(
            state
                .clone()
                .with_jobs(
                    BTreeMap::new(),
                    BTreeMap::from([(
                        "schedule".to_owned(),
                        ScheduleDefinition {
                            store: "a-existing".to_owned(),
                            policy,
                        }
                    )]),
                    BTreeMap::new(),
                )
                .is_err(),
            "{field}"
        );
    }
    for (field, policy) in invalid_buckets() {
        let buckets = BTreeMap::from([("bucket".to_owned(), policy)]);
        assert!(
            store
                .commit_plan(CommitPlan {
                    expected_revision: 0,
                    mutations: &mutation,
                    queues: &BTreeMap::new(),
                    buckets: &buckets,
                    now_millis: 0,
                    completion: None,
                })
                .is_err(),
            "{field}"
        );
        assert!(
            state
                .clone()
                .with_jobs(
                    BTreeMap::new(),
                    BTreeMap::new(),
                    BTreeMap::from([(
                        "bucket".to_owned(),
                        BucketDefinition {
                            store: "a-existing".to_owned(),
                            policy,
                        }
                    )]),
                )
                .is_err(),
            "{field}"
        );
    }
    assert_eq!(store.revision().unwrap(), 0);
    assert_eq!(store.get("untouched").unwrap(), None);
    assert_eq!(store.schedule_stats("schedule").unwrap(), (0, 0, 0));
}

#[test]
fn lower_commit_resource_counts_accept_maxima_and_reject_the_next_entry() {
    let directory = TestDirectory::new();
    let store = DurableStore::open(
        &directory.path("counts.db"),
        Durability::Full,
        maximum_store_limits(),
    )
    .unwrap();
    let mut queues: BTreeMap<_, _> = (0..DurableState::MAX_QUEUES)
        .map(|index| (format!("queue-{index}"), maximum_queue()))
        .collect();
    let mut buckets: BTreeMap<_, _> = (0..DurableState::MAX_BUCKETS)
        .map(|index| (format!("bucket-{index}"), maximum_bucket()))
        .collect();
    let commit = |queues: &BTreeMap<String, QueuePolicy>,
                  buckets: &BTreeMap<String, BucketPolicy>| {
        store.commit_plan(CommitPlan {
            expected_revision: 0,
            mutations: &[],
            queues,
            buckets,
            now_millis: 0,
            completion: None,
        })
    };
    assert_eq!(commit(&queues, &buckets).unwrap(), 0);
    queues.insert("extra".to_owned(), maximum_queue());
    assert_eq!(
        commit(&queues, &buckets).unwrap_err().kind(),
        StateErrorKind::Limit
    );
    queues.remove("extra");
    buckets.insert("extra".to_owned(), maximum_bucket());
    assert_eq!(
        commit(&queues, &buckets).unwrap_err().kind(),
        StateErrorKind::Limit
    );
    assert_eq!(store.revision().unwrap(), 0);
}

#[test]
fn lease_minimum_is_inclusive_and_checked_without_side_effects() {
    let directory = TestDirectory::new();
    let before = schema_one_fixture(&directory.path("existing.db"));
    let configuration = Configuration::new(&directory);
    let deadline = MAX_DELIVERY_LEASE - MAX_STATE_BUSY_TIMEOUT;
    configuration.validate_for_runtime(deadline).unwrap();
    assert!(
        configuration
            .validate_for_runtime(deadline + Duration::from_nanos(1))
            .is_err()
    );
    assert!(configuration.validate_for_runtime(Duration::MAX).is_err());
    for family in ["replay", "queue", "schedule"] {
        let mut configuration = configuration.clone();
        let lease = MAX_DELIVERY_LEASE - Duration::from_millis(1);
        match family {
            "replay" => configuration.store_mut().replay.lease = lease,
            "queue" => configuration.queues.get_mut("queue").unwrap().policy.lease = lease,
            "schedule" => {
                configuration
                    .schedules
                    .get_mut("schedule")
                    .unwrap()
                    .policy
                    .lease = lease
            }
            _ => unreachable!(),
        }
        let error = configuration.validate_for_runtime(deadline).unwrap_err();
        assert!(error.message().contains(family));
    }
    assert_no_side_effects(&directory, &before);
    let durable = configuration.open().unwrap();
    durable.validate_for_runtime(deadline).unwrap();
    assert!(
        durable
            .validate_for_runtime(deadline + Duration::from_nanos(1))
            .is_err()
    );
}

#[test]
fn inclusive_lower_endpoints_remain_usable() {
    let directory = TestDirectory::new();
    let mut configuration = Configuration::new(&directory);
    configuration.store_mut().limits.max_database_bytes = MINIMUM_DATABASE_BYTES;
    let schedule = &mut configuration.schedules.get_mut("schedule").unwrap().policy;
    schedule.interval = MIN_SCHEDULE_INTERVAL;
    schedule.start_millis = 0;
    configuration
        .validate_for_runtime(Duration::from_secs(1))
        .unwrap();
    drop(configuration.host().unwrap());
}

#[test]
fn schedule_instant_preflight_is_inclusive_and_side_effect_free() {
    let mut policy = maximum_schedule();
    policy.start_millis = 0;
    let retention_millis = i64::try_from(MAX_SCHEDULE_RETENTION.as_millis()).unwrap();
    let latest = i64::MAX - retention_millis;
    policy.validate_instant(latest).unwrap();
    assert!(policy.validate_instant(latest + 1).is_err());
    let earliest = i64::MIN + retention_millis;
    policy.validate_instant(earliest).unwrap();
    assert!(policy.validate_instant(earliest - 1).is_err());
}

#[test]
fn lower_replay_retention_and_result_limits_leave_failed_reservations_unmodified() {
    let directory = TestDirectory::new();
    let store = DurableStore::open(
        &directory.path("replay.db"),
        Durability::Full,
        maximum_store_limits(),
    )
    .unwrap();
    let request = ReplayRequest {
        artifact: &[1; 32],
        kind: ReplayKind::Ai,
        operation: "replay",
        input_digest: &[2; 32],
        owner: &[3; 16],
        now_millis: 0,
    };
    for (field, policy) in invalid_retention() {
        assert!(store.replay_decision(request, policy).is_err(), "{field}");
        assert!(
            store
                .idempotency_decision(&[1; 32], "request", &[2; 32], &[3; 16], 0, policy)
                .is_err(),
            "{field}"
        );
    }
    let ReplayDecision::Execute(lease) =
        store.replay_decision(request, maximum_retention()).unwrap()
    else {
        panic!("invalid policies must not reserve work");
    };
    let krit_state::IdempotencyDecision::Execute(idempotency) = store
        .idempotency_decision(
            &[1; 32],
            "request",
            &[2; 32],
            &[3; 16],
            0,
            maximum_retention(),
        )
        .unwrap()
    else {
        panic!("invalid policies must not reserve an inbound request");
    };
    for (field, policy) in invalid_retention() {
        assert!(
            store.complete_replay(&lease, b"result", 1, policy).is_err(),
            "{field}"
        );
        assert!(
            store
                .complete_idempotency(&idempotency, b"response", 1, policy)
                .is_err(),
            "{field}"
        );
    }
    store
        .complete_idempotency(&idempotency, b"response", 1, maximum_retention())
        .unwrap();
    let result = vec![b'x'; MAX_REPLAY_RESULT_BYTES + 1];
    assert_eq!(
        store
            .complete_replay(&lease, &result, 1, maximum_retention())
            .unwrap_err()
            .kind(),
        StateErrorKind::Limit
    );
    store
        .complete_replay(
            &lease,
            &result[..MAX_REPLAY_RESULT_BYTES],
            1,
            maximum_retention(),
        )
        .unwrap();
    assert_eq!(store.replay_counts().unwrap(), (1, MAX_REPLAY_RESULT_BYTES));
}
