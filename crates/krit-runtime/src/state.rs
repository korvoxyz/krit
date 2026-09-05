use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use krit_state::{CommitPlan, Completion, DurableStore, Mutation, StateErrorKind};

use crate::RuntimeError;

pub use krit_state::{
    BucketPolicy, Durability, QueuePolicy, RetentionPolicy, SchedulePolicy, StoreLimits,
};

#[derive(Clone, Debug)]
pub struct DurableStoreDefinition {
    pub path: PathBuf,
    pub durability: Durability,
    pub limits: StoreLimits,
    pub replay: RetentionPolicy,
}

impl DurableStoreDefinition {
    /// Validates all store and replay bounds without touching the filesystem.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        self.replay.validate(self.limits).map_err(map_state_error)
    }
}

#[derive(Clone, Debug)]
pub struct QueueDefinition {
    pub store: String,
    pub policy: QueuePolicy,
}

#[derive(Clone, Debug)]
pub struct ScheduleDefinition {
    pub store: String,
    pub policy: SchedulePolicy,
}

#[derive(Clone, Debug)]
pub struct BucketDefinition {
    pub store: String,
    pub policy: BucketPolicy,
}

#[derive(Clone, Debug)]
pub(crate) struct ResourceBinding<P> {
    pub(crate) store: String,
    pub(crate) policy: P,
}

#[derive(Clone, Default)]
pub struct DurableState {
    stores: Arc<BTreeMap<String, Arc<StoreBinding>>>,
    durable_idempotency_store: Option<String>,
    queues: Arc<BTreeMap<String, ResourceBinding<QueuePolicy>>>,
    schedules: Arc<BTreeMap<String, ResourceBinding<SchedulePolicy>>>,
    buckets: Arc<BTreeMap<String, ResourceBinding<BucketPolicy>>>,
}

pub(crate) struct StoreBinding {
    store: DurableStore,
    replay: RetentionPolicy,
}

impl DurableState {
    pub const MAX_STORES: usize = 16;
    pub const MAX_QUEUES: usize = krit_state::MAX_QUEUES;
    pub const MAX_SCHEDULES: usize = krit_state::MAX_SCHEDULES;
    pub const MAX_BUCKETS: usize = krit_state::MAX_BUCKETS;

    /// Validates the entire durable configuration before opening any store.
    pub fn validate_configuration(
        definitions: &BTreeMap<String, DurableStoreDefinition>,
        durable_idempotency_store: Option<&str>,
        queues: &BTreeMap<String, QueueDefinition>,
        schedules: &BTreeMap<String, ScheduleDefinition>,
        buckets: &BTreeMap<String, BucketDefinition>,
    ) -> Result<(), RuntimeError> {
        if definitions.len() > Self::MAX_STORES {
            return Err(RuntimeError::state_conflict(
                "configured durable stores exceed the protocol bound",
            ));
        }
        for (name, definition) in definitions {
            validate_resource_name(name)?;
            definition.validate()?;
        }
        let stores = definitions.keys().map(String::as_str).collect();
        if durable_idempotency_store.is_some_and(|name| !definitions.contains_key(name)) {
            return Err(RuntimeError::durable_state(
                "durable idempotency store is not configured",
            ));
        }
        validate_job_definitions(&stores, queues, schedules, buckets)
    }

    /// Adds runtime-dependent lease checks to pure configuration validation.
    ///
    /// Scheduler ownership must precede reservation, so leases need to cover
    /// one execution deadline plus the backing store's bounded SQLite wait.
    pub fn validate_configuration_for_runtime(
        definitions: &BTreeMap<String, DurableStoreDefinition>,
        durable_idempotency_store: Option<&str>,
        queues: &BTreeMap<String, QueueDefinition>,
        schedules: &BTreeMap<String, ScheduleDefinition>,
        buckets: &BTreeMap<String, BucketDefinition>,
        deadline: Duration,
    ) -> Result<(), RuntimeError> {
        Self::validate_configuration(
            definitions,
            durable_idempotency_store,
            queues,
            schedules,
            buckets,
        )?;
        for definition in definitions.values() {
            validate_lease(
                definition.replay.lease,
                definition.limits.busy_timeout,
                deadline,
                "replay",
            )?;
        }
        for definition in queues.values() {
            validate_lease(
                definition.policy.lease,
                definitions[&definition.store].limits.busy_timeout,
                deadline,
                "queue",
            )?;
        }
        for definition in schedules.values() {
            validate_lease(
                definition.policy.lease,
                definitions[&definition.store].limits.busy_timeout,
                deadline,
                "schedule",
            )?;
        }
        Ok(())
    }

    pub fn open(
        definitions: BTreeMap<String, DurableStoreDefinition>,
        durable_idempotency_store: Option<String>,
    ) -> Result<Self, RuntimeError> {
        Self::open_with_jobs(
            definitions,
            durable_idempotency_store,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
    }

    /// Opens stores only after every store, job binding, and policy validates.
    ///
    /// Prefer this to `open(...).with_jobs(...)` when the complete configuration
    /// is available: an invalid job policy then cannot create or migrate stores.
    pub fn open_with_jobs(
        definitions: BTreeMap<String, DurableStoreDefinition>,
        durable_idempotency_store: Option<String>,
        queues: BTreeMap<String, QueueDefinition>,
        schedules: BTreeMap<String, ScheduleDefinition>,
        buckets: BTreeMap<String, BucketDefinition>,
    ) -> Result<Self, RuntimeError> {
        Self::validate_configuration(
            &definitions,
            durable_idempotency_store.as_deref(),
            &queues,
            &schedules,
            &buckets,
        )?;
        let mut stores = BTreeMap::new();
        for (name, definition) in definitions {
            let store =
                DurableStore::open(&definition.path, definition.durability, definition.limits)
                    .map_err(map_state_error)?;
            stores.insert(
                name,
                Arc::new(StoreBinding {
                    store,
                    replay: definition.replay,
                }),
            );
        }
        Self {
            stores: Arc::new(stores),
            durable_idempotency_store,
            queues: Arc::new(BTreeMap::new()),
            schedules: Arc::new(BTreeMap::new()),
            buckets: Arc::new(BTreeMap::new()),
        }
        .with_jobs(queues, schedules, buckets)
    }

    /// Binds manifest-granted queues, schedules, and buckets to already-opened
    /// stores. Every binding must name a configured store.
    pub fn with_jobs(
        mut self,
        queues: BTreeMap<String, QueueDefinition>,
        schedules: BTreeMap<String, ScheduleDefinition>,
        buckets: BTreeMap<String, BucketDefinition>,
    ) -> Result<Self, RuntimeError> {
        validate_job_definitions(&self.store_names().collect(), &queues, &schedules, &buckets)?;
        let mut queue_bindings = BTreeMap::new();
        for (name, definition) in queues {
            queue_bindings.insert(
                name,
                ResourceBinding {
                    store: definition.store,
                    policy: definition.policy,
                },
            );
        }
        let mut schedule_bindings = BTreeMap::new();
        for (name, definition) in schedules {
            schedule_bindings.insert(
                name,
                ResourceBinding {
                    store: definition.store,
                    policy: definition.policy,
                },
            );
        }
        let mut bucket_bindings = BTreeMap::new();
        for (name, definition) in buckets {
            bucket_bindings.insert(
                name,
                ResourceBinding {
                    store: definition.store,
                    policy: definition.policy,
                },
            );
        }
        self.queues = Arc::new(queue_bindings);
        self.schedules = Arc::new(schedule_bindings);
        self.buckets = Arc::new(bucket_bindings);
        Ok(self)
    }

    /// Stores that back at least one configured queue, schedule, or bucket.
    ///
    /// These stores are host-owned: a package never names them, so they do not
    /// require a `state.transaction` grant on their own.
    pub fn job_store_names(&self) -> BTreeSet<&str> {
        self.queues
            .values()
            .map(|binding| binding.store.as_str())
            .chain(
                self.schedules
                    .values()
                    .map(|binding| binding.store.as_str()),
            )
            .chain(self.buckets.values().map(|binding| binding.store.as_str()))
            .collect()
    }

    pub fn queue_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.queues.keys().map(String::as_str)
    }

    pub fn schedule_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.schedules.keys().map(String::as_str)
    }

    pub fn bucket_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.buckets.keys().map(String::as_str)
    }

    pub(crate) fn queue(
        &self,
        name: &str,
    ) -> Result<(Arc<StoreBinding>, QueuePolicy), RuntimeError> {
        let binding = self
            .queues
            .get(name)
            .ok_or_else(|| RuntimeError::authorization("durable queue is not configured"))?;
        Ok((self.binding(&binding.store)?, binding.policy))
    }

    pub(crate) fn schedule(
        &self,
        name: &str,
    ) -> Result<(Arc<StoreBinding>, SchedulePolicy), RuntimeError> {
        let binding = self
            .schedules
            .get(name)
            .ok_or_else(|| RuntimeError::authorization("durable schedule is not configured"))?;
        Ok((self.binding(&binding.store)?, binding.policy))
    }

    pub(crate) fn bucket(&self, name: &str) -> Result<(String, BucketPolicy), RuntimeError> {
        let binding = self.buckets.get(name).ok_or_else(|| {
            RuntimeError::authorization("durable object bucket is not configured")
        })?;
        Ok((binding.store.clone(), binding.policy))
    }

    pub(crate) fn schedule_store(&self, name: &str) -> Result<String, RuntimeError> {
        self.schedules
            .get(name)
            .map(|binding| binding.store.clone())
            .ok_or_else(|| RuntimeError::authorization("durable schedule is not configured"))
    }

    pub(crate) fn queue_store(&self, name: &str) -> Result<String, RuntimeError> {
        self.queues
            .get(name)
            .map(|binding| binding.store.clone())
            .ok_or_else(|| RuntimeError::authorization("durable queue is not configured"))
    }

    pub(crate) fn queue_policies(&self) -> BTreeMap<String, QueuePolicy> {
        self.queues
            .iter()
            .map(|(name, binding)| (name.clone(), binding.policy))
            .collect()
    }

    pub(crate) fn bucket_policies(&self) -> BTreeMap<String, BucketPolicy> {
        self.buckets
            .iter()
            .map(|(name, binding)| (name.clone(), binding.policy))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.stores.is_empty()
    }

    pub fn store_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.stores.keys().map(String::as_str)
    }

    pub(crate) fn binding(&self, name: &str) -> Result<Arc<StoreBinding>, RuntimeError> {
        self.stores
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::authorization("durable state store is not configured"))
    }

    pub(crate) fn idempotency_binding(&self) -> Option<Arc<StoreBinding>> {
        self.durable_idempotency_store
            .as_ref()
            .and_then(|name| self.stores.get(name))
            .cloned()
    }

    pub fn validate_for_runtime(&self, deadline: Duration) -> Result<(), RuntimeError> {
        for binding in self.stores.values() {
            validate_lease(
                binding.replay.lease,
                binding.store.limits().busy_timeout,
                deadline,
                "replay",
            )?;
        }
        // A delivery lease must outlive one complete guest execution, otherwise
        // a second worker could reserve the same job while the first is still
        // running and both could reach their outcome boundary.
        for binding in self.queues.values() {
            let store = self.binding(&binding.store)?;
            validate_lease(
                binding.policy.lease,
                store.store.limits().busy_timeout,
                deadline,
                "queue",
            )?;
        }
        for binding in self.schedules.values() {
            let store = self.binding(&binding.store)?;
            validate_lease(
                binding.policy.lease,
                store.store.limits().busy_timeout,
                deadline,
                "schedule",
            )?;
        }
        Ok(())
    }
}

fn validate_resource_name(name: &str) -> Result<(), RuntimeError> {
    if krit_capability::is_valid_resource_name(name) {
        Ok(())
    } else {
        Err(RuntimeError::durable_state(
            "durable resource name must use the canonical resource grammar",
        ))
    }
}

fn validate_job_definitions(
    stores: &BTreeSet<&str>,
    queues: &BTreeMap<String, QueueDefinition>,
    schedules: &BTreeMap<String, ScheduleDefinition>,
    buckets: &BTreeMap<String, BucketDefinition>,
) -> Result<(), RuntimeError> {
    if queues.len() > DurableState::MAX_QUEUES
        || schedules.len() > DurableState::MAX_SCHEDULES
        || buckets.len() > DurableState::MAX_BUCKETS
    {
        return Err(RuntimeError::state_conflict(
            "configured queues, schedules, or buckets exceed the protocol bounds",
        ));
    }
    let require_store = |name: &str, store: &str| {
        validate_resource_name(name)?;
        if !stores.contains(store) {
            return Err(RuntimeError::durable_state(
                "durable job resource names an unconfigured store",
            ));
        }
        Ok(())
    };
    for (name, definition) in queues {
        require_store(name, &definition.store)?;
        definition.policy.validate().map_err(map_state_error)?;
    }
    for (name, definition) in schedules {
        require_store(name, &definition.store)?;
        definition.policy.validate().map_err(map_state_error)?;
    }
    for (name, definition) in buckets {
        require_store(name, &definition.store)?;
        definition.policy.validate().map_err(map_state_error)?;
    }
    Ok(())
}

fn validate_lease(
    lease: Duration,
    busy_timeout: Duration,
    deadline: Duration,
    resource: &str,
) -> Result<(), RuntimeError> {
    let minimum = deadline
        .checked_add(busy_timeout)
        .ok_or_else(|| RuntimeError::durable_state("durable lease minimum overflowed"))?;
    if lease < minimum {
        return Err(RuntimeError::durable_state(format!(
            "durable {resource} lease must cover the runtime deadline and database busy timeout",
        )));
    }
    Ok(())
}

impl std::fmt::Debug for DurableState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableState")
            .field("stores", &self.stores.keys().collect::<Vec<_>>())
            .field("durable_idempotency_store", &self.durable_idempotency_store)
            .field("queues", &self.queues.keys().collect::<Vec<_>>())
            .field("schedules", &self.schedules.keys().collect::<Vec<_>>())
            .field("buckets", &self.buckets.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Default)]
pub(crate) struct InvocationState {
    active: Option<ActiveStore>,
    operations: u64,
    reads: u64,
    writes: u64,
    checkpoint_reads: u64,
    checkpoint_writes: u64,
    replay_hits: u64,
    replay_misses: u64,
    object_reads: u64,
    object_writes: u64,
    queue_publishes: u64,
}

struct ActiveStore {
    name: String,
    binding: Arc<StoreBinding>,
    revision: u64,
    values: BTreeMap<String, Option<Vec<u8>>>,
    checkpoints: BTreeMap<String, Vec<u8>>,
    objects: BTreeMap<(String, String), Option<Vec<u8>>>,
    publishes: Vec<(String, [u8; 16], Vec<u8>)>,
}

impl InvocationState {
    pub(crate) fn get(
        &mut self,
        durable: &DurableState,
        store: &str,
        key: &str,
    ) -> Result<Option<String>, RuntimeError> {
        self.record_operation(durable, store)?;
        self.reads = self.reads.saturating_add(1);
        let active = self.active.as_mut().expect("active store was initialized");
        let value = match active.values.get(key) {
            Some(value) => value.clone(),
            None => active
                .binding
                .store
                .get_at_revision(key, active.revision)
                .map_err(map_state_error)?,
        };
        value
            .map(|value| {
                String::from_utf8(value)
                    .map_err(|_| RuntimeError::durable_state("durable state value is not UTF-8"))
            })
            .transpose()
    }

    pub(crate) fn put(
        &mut self,
        durable: &DurableState,
        store: &str,
        key: String,
        value: String,
    ) -> Result<(), RuntimeError> {
        self.record_operation(durable, store)?;
        self.writes = self.writes.saturating_add(1);
        let active = self.active.as_mut().expect("active store was initialized");
        validate_key_value(&active.binding, &key, Some(value.as_bytes()))?;
        active.values.insert(key, Some(value.into_bytes()));
        self.validate_staged()
    }

    pub(crate) fn delete(
        &mut self,
        durable: &DurableState,
        store: &str,
        key: String,
    ) -> Result<(), RuntimeError> {
        self.record_operation(durable, store)?;
        self.writes = self.writes.saturating_add(1);
        let active = self.active.as_mut().expect("active store was initialized");
        validate_key_value(&active.binding, &key, None)?;
        active.values.insert(key, None);
        self.validate_staged()
    }

    pub(crate) fn checkpoint_get(
        &mut self,
        durable: &DurableState,
        store: &str,
        name: &str,
    ) -> Result<Option<String>, RuntimeError> {
        self.record_operation(durable, store)?;
        self.checkpoint_reads = self.checkpoint_reads.saturating_add(1);
        let active = self.active.as_mut().expect("active store was initialized");
        let value = match active.checkpoints.get(name) {
            Some(value) => Some(value.clone()),
            None => active
                .binding
                .store
                .checkpoint_at_revision(name, active.revision)
                .map_err(map_state_error)?,
        };
        value
            .map(|value| {
                String::from_utf8(value).map_err(|_| {
                    RuntimeError::durable_state("durable checkpoint value is not UTF-8")
                })
            })
            .transpose()
    }

    pub(crate) fn checkpoint_put(
        &mut self,
        durable: &DurableState,
        store: &str,
        name: String,
        value: String,
    ) -> Result<(), RuntimeError> {
        self.record_operation(durable, store)?;
        self.checkpoint_writes = self.checkpoint_writes.saturating_add(1);
        let active = self.active.as_mut().expect("active store was initialized");
        validate_key_value(&active.binding, &name, Some(value.as_bytes()))?;
        active.checkpoints.insert(name, value.into_bytes());
        self.validate_staged()
    }

    /// Commits every staged mutation and the optional delivery acknowledgement
    /// in one durable transaction, or commits nothing at all.
    pub(crate) fn commit_outcome(
        &mut self,
        durable: &DurableState,
        completion: Option<&Completion>,
        now_millis: i64,
    ) -> Result<(), RuntimeError> {
        let Some(active) = self.active.take() else {
            if completion.is_some() {
                return Err(RuntimeError::delivery(
                    "delivery acknowledgement has no active durable store",
                ));
            }
            return Ok(());
        };
        let mut mutations = Vec::with_capacity(
            active.values.len()
                + active.checkpoints.len()
                + active.objects.len()
                + active.publishes.len(),
        );
        for (key, value) in active.values {
            mutations.push(match value {
                Some(value) => Mutation::Put { key, value },
                None => Mutation::Delete { key },
            });
        }
        for (name, value) in active.checkpoints {
            mutations.push(Mutation::CheckpointPut { name, value });
        }
        for ((bucket, key), value) in active.objects {
            mutations.push(match value {
                Some(value) => Mutation::ObjectPut { bucket, key, value },
                None => Mutation::ObjectDelete { bucket, key },
            });
        }
        for (queue, id, body) in active.publishes {
            mutations.push(Mutation::QueuePublish { queue, id, body });
        }
        if mutations.is_empty() && completion.is_none() {
            return Ok(());
        }
        active
            .binding
            .store
            .commit_plan(CommitPlan {
                expected_revision: active.revision,
                mutations: &mutations,
                queues: &durable.queue_policies(),
                buckets: &durable.bucket_policies(),
                now_millis,
                completion,
            })
            .map_err(map_state_error)?;
        Ok(())
    }

    /// Binds the invocation transaction to one store before guest execution so
    /// that a delivery acknowledgement always commits with staged mutations.
    pub(crate) fn bind(&mut self, durable: &DurableState, store: &str) -> Result<(), RuntimeError> {
        self.activate(durable, store)
    }

    pub(crate) fn object_get(
        &mut self,
        durable: &DurableState,
        bucket: &str,
        key: &str,
    ) -> Result<Option<String>, RuntimeError> {
        let (store, _) = durable.bucket(bucket)?;
        self.record_operation(durable, &store)?;
        self.object_reads = self.object_reads.saturating_add(1);
        let active = self.active.as_mut().expect("active store was initialized");
        let staged = active.objects.get(&(bucket.to_owned(), key.to_owned()));
        let value = match staged {
            Some(value) => value.clone(),
            None => active
                .binding
                .store
                .object_at_revision(bucket, key, active.revision)
                .map_err(map_state_error)?,
        };
        value
            .map(|value| {
                String::from_utf8(value)
                    .map_err(|_| RuntimeError::durable_state("durable object is not UTF-8"))
            })
            .transpose()
    }

    pub(crate) fn object_put(
        &mut self,
        durable: &DurableState,
        bucket: &str,
        key: String,
        value: String,
    ) -> Result<(), RuntimeError> {
        let (store, policy) = durable.bucket(bucket)?;
        self.record_operation(durable, &store)?;
        self.object_writes = self.object_writes.saturating_add(1);
        validate_object_key_value(policy, &key, Some(value.as_bytes()))?;
        let active = self.active.as_mut().expect("active store was initialized");
        active
            .objects
            .insert((bucket.to_owned(), key), Some(value.into_bytes()));
        self.validate_staged()
    }

    pub(crate) fn object_delete(
        &mut self,
        durable: &DurableState,
        bucket: &str,
        key: String,
    ) -> Result<(), RuntimeError> {
        let (store, policy) = durable.bucket(bucket)?;
        self.record_operation(durable, &store)?;
        self.object_writes = self.object_writes.saturating_add(1);
        validate_object_key_value(policy, &key, None)?;
        let active = self.active.as_mut().expect("active store was initialized");
        active.objects.insert((bucket.to_owned(), key), None);
        self.validate_staged()
    }

    pub(crate) fn queue_publish(
        &mut self,
        durable: &DurableState,
        queue: &str,
        body: String,
        id: [u8; 16],
    ) -> Result<(), RuntimeError> {
        let (_, policy) = durable.queue(queue)?;
        let store = durable.queue_store(queue)?;
        self.record_operation(durable, &store)?;
        self.queue_publishes = self.queue_publishes.saturating_add(1);
        if body.len() > policy.max_job_bytes {
            return Err(RuntimeError::state_conflict(
                "durable queue job exceeds its configured byte limit",
            ));
        }
        let active = self.active.as_mut().expect("active store was initialized");
        // Depth is a per-queue bound: an atomic fan-out to several queues must
        // not be charged against one queue's budget.
        let staged = active
            .publishes
            .iter()
            .filter(|(staged_queue, _, _)| staged_queue == queue)
            .count();
        if staged >= policy.max_depth {
            return Err(RuntimeError::state_conflict(
                "durable queue publishes exceed the configured depth",
            ));
        }
        active
            .publishes
            .push((queue.to_owned(), id, body.into_bytes()));
        self.validate_staged()
    }

    pub(crate) const fn object_reads(&self) -> u64 {
        self.object_reads
    }

    pub(crate) const fn object_writes(&self) -> u64 {
        self.object_writes
    }

    pub(crate) const fn queue_publishes(&self) -> u64 {
        self.queue_publishes
    }

    pub(crate) fn replay_binding(
        &mut self,
        durable: &DurableState,
        store: &str,
    ) -> Result<Arc<StoreBinding>, RuntimeError> {
        self.record_operation(durable, store)?;
        Ok(self
            .active
            .as_ref()
            .expect("active store was initialized")
            .binding
            .clone())
    }

    pub(crate) fn record_replay(&mut self, replayed: bool) {
        if replayed {
            self.replay_hits = self.replay_hits.saturating_add(1);
        } else {
            self.replay_misses = self.replay_misses.saturating_add(1);
        }
    }

    pub(crate) const fn operations(&self) -> u64 {
        self.operations
    }

    pub(crate) const fn reads(&self) -> u64 {
        self.reads
    }

    pub(crate) const fn writes(&self) -> u64 {
        self.writes
    }

    pub(crate) const fn checkpoint_reads(&self) -> u64 {
        self.checkpoint_reads
    }

    pub(crate) const fn checkpoint_writes(&self) -> u64 {
        self.checkpoint_writes
    }

    pub(crate) const fn replay_hits(&self) -> u64 {
        self.replay_hits
    }

    pub(crate) const fn replay_misses(&self) -> u64 {
        self.replay_misses
    }

    pub(crate) fn touched(&self) -> bool {
        self.active.is_some()
    }

    fn record_operation(
        &mut self,
        durable: &DurableState,
        store: &str,
    ) -> Result<(), RuntimeError> {
        let binding = durable.binding(store)?;
        let next = self
            .operations
            .checked_add(1)
            .ok_or_else(|| RuntimeError::state_conflict("state operation count overflowed"))?;
        if next > binding.store.limits().max_operations as u64 {
            return Err(RuntimeError::state_conflict(
                "state operation count exceeds the configured limit",
            ));
        }
        self.operations = next;
        self.activate_checked(binding, store)
    }

    fn activate(&mut self, durable: &DurableState, store: &str) -> Result<(), RuntimeError> {
        let binding = durable.binding(store)?;
        self.activate_checked(binding, store)
    }

    fn activate_checked(
        &mut self,
        binding: Arc<StoreBinding>,
        store: &str,
    ) -> Result<(), RuntimeError> {
        match &self.active {
            Some(active) if active.name != store => Err(RuntimeError::state_conflict(
                "one invocation cannot transact across multiple durable stores",
            )),
            Some(_) => Ok(()),
            None => {
                let revision = binding.store.revision().map_err(map_state_error)?;
                self.active = Some(ActiveStore {
                    name: store.to_owned(),
                    binding,
                    revision,
                    values: BTreeMap::new(),
                    checkpoints: BTreeMap::new(),
                    objects: BTreeMap::new(),
                    publishes: Vec::new(),
                });
                Ok(())
            }
        }
    }

    fn validate_staged(&self) -> Result<(), RuntimeError> {
        let active = self.active.as_ref().expect("active store was initialized");
        let limits = active.binding.store.limits();
        let bytes = active
            .values
            .iter()
            .map(|(key, value)| key.len() + value.as_ref().map_or(0, Vec::len))
            .chain(
                active
                    .checkpoints
                    .iter()
                    .map(|(name, value)| name.len() + value.len()),
            )
            .try_fold(0usize, |total, value| total.checked_add(value))
            .ok_or_else(|| RuntimeError::state_conflict("staged state byte count overflowed"))?;
        if bytes > limits.max_transaction_bytes {
            return Err(RuntimeError::state_conflict(
                "staged state exceeds the configured transaction byte limit",
            ));
        }
        Ok(())
    }
}

fn validate_object_key_value(
    policy: BucketPolicy,
    key: &str,
    value: Option<&[u8]>,
) -> Result<(), RuntimeError> {
    if key.is_empty() || key.len() > policy.max_key_bytes || key.contains('\0') {
        return Err(RuntimeError::state_conflict(
            "durable object key is invalid or exceeds its configured limit",
        ));
    }
    if value.is_some_and(|value| value.len() > policy.max_object_bytes) {
        return Err(RuntimeError::state_conflict(
            "durable object value exceeds its configured limit",
        ));
    }
    Ok(())
}

fn validate_key_value(
    binding: &StoreBinding,
    key: &str,
    value: Option<&[u8]>,
) -> Result<(), RuntimeError> {
    let limits = binding.store.limits();
    if key.is_empty() || key.len() > limits.max_key_bytes || key.contains('\0') {
        return Err(RuntimeError::state_conflict(
            "durable state key is invalid or exceeds its configured limit",
        ));
    }
    if value.is_some_and(|value| value.len() > limits.max_value_bytes) {
        return Err(RuntimeError::state_conflict(
            "durable state value exceeds its configured limit",
        ));
    }
    Ok(())
}

impl StoreBinding {
    pub(crate) fn store(&self) -> &DurableStore {
        &self.store
    }

    pub(crate) const fn replay_policy(&self) -> RetentionPolicy {
        self.replay
    }
}

pub(crate) fn map_state_error(error: krit_state::StateError) -> RuntimeError {
    match error.kind() {
        StateErrorKind::Database => RuntimeError::durable_state(error.message()),
        StateErrorKind::Conflict | StateErrorKind::Limit => {
            RuntimeError::state_conflict(error.message())
        }
        StateErrorKind::Replay => RuntimeError::replay(error.message()),
    }
}
