use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use krit_state::{DurableStore, Mutation, StateErrorKind};

use crate::RuntimeError;

pub use krit_state::{Durability, RetentionPolicy, StoreLimits};

#[derive(Clone, Debug)]
pub struct DurableStoreDefinition {
    pub path: PathBuf,
    pub durability: Durability,
    pub limits: StoreLimits,
    pub replay: RetentionPolicy,
}

#[derive(Clone, Default)]
pub struct DurableState {
    stores: Arc<BTreeMap<String, Arc<StoreBinding>>>,
    durable_idempotency_store: Option<String>,
}

pub(crate) struct StoreBinding {
    store: DurableStore,
    replay: RetentionPolicy,
}

impl DurableState {
    pub fn open(
        definitions: BTreeMap<String, DurableStoreDefinition>,
        durable_idempotency_store: Option<String>,
    ) -> Result<Self, RuntimeError> {
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
        if durable_idempotency_store
            .as_ref()
            .is_some_and(|name| !stores.contains_key(name))
        {
            return Err(RuntimeError::durable_state(
                "durable idempotency store is not configured",
            ));
        }
        Ok(Self {
            stores: Arc::new(stores),
            durable_idempotency_store,
        })
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

    pub(crate) fn validate_for_runtime(&self, deadline: Duration) -> Result<(), RuntimeError> {
        for binding in self.stores.values() {
            let minimum = deadline
                .checked_add(binding.store.limits().busy_timeout)
                .ok_or_else(|| RuntimeError::durable_state("durable lease minimum overflowed"))?;
            if binding.replay.lease < minimum {
                return Err(RuntimeError::durable_state(
                    "durable replay lease must cover the runtime deadline and database busy timeout",
                ));
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for DurableState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableState")
            .field("stores", &self.stores.keys().collect::<Vec<_>>())
            .field("durable_idempotency_store", &self.durable_idempotency_store)
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
}

struct ActiveStore {
    name: String,
    binding: Arc<StoreBinding>,
    revision: u64,
    values: BTreeMap<String, Option<Vec<u8>>>,
    checkpoints: BTreeMap<String, Vec<u8>>,
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

    pub(crate) fn commit(&mut self) -> Result<(), RuntimeError> {
        let Some(active) = self.active.take() else {
            return Ok(());
        };
        let mut mutations = Vec::with_capacity(active.values.len() + active.checkpoints.len());
        for (key, value) in active.values {
            mutations.push(match value {
                Some(value) => Mutation::Put { key, value },
                None => Mutation::Delete { key },
            });
        }
        for (name, value) in active.checkpoints {
            mutations.push(Mutation::CheckpointPut { name, value });
        }
        if mutations.is_empty() {
            return Ok(());
        }
        active
            .binding
            .store
            .commit(active.revision, &mutations)
            .map_err(map_state_error)?;
        Ok(())
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
