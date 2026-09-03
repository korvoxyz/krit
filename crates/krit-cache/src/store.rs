use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, atomic::AtomicU64},
    time::Instant,
};

use crate::{
    config::{CacheConfig, ENTRY_OVERHEAD_BYTES, NamespaceMode, NamespacePolicy},
    error::CacheError,
};

/// Most expired entries one operation will reclaim.
///
/// Cleanup is opportunistic and strictly bounded, so no single call can be made
/// slow by a large backlog. Correctness never depends on it: expiry is enforced
/// exactly on read, and eviction reclaims capacity on demand.
const MAX_SWEEP_PER_OPERATION: usize = 8;

/// A point on the cache's own monotonic timeline, in milliseconds since the
/// cache was created.
///
/// The cache deliberately never reads a wall clock. A wall clock can jump
/// backwards, which would silently extend an entry's lifetime past its declared
/// time to live, and reading one can fail, which would turn a host clock fault
/// into a guest-visible trap. `Instant` is monotonic and infallible, so neither
/// can happen.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CacheInstant(u64);

impl CacheInstant {
    /// An explicit point on the timeline, for deterministic tests and for an
    /// embedding that drives its own clock.
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    pub const fn as_millis(self) -> u64 {
        self.0
    }
}

/// Who is performing a cache write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Writer {
    /// Guest code, subject to the namespace's read-only mode.
    Guest,
    /// The embedding that owns the cache, seeding reference data.
    Host,
}

/// Numeric cache observability. It never carries a key, value, or namespace
/// payload.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub expirations: u64,
    pub evictions: u64,
    pub puts: u64,
    pub deletes: u64,
    pub entries: u64,
    pub bytes: u64,
}

struct Entry {
    value: String,
    expires_at_millis: u64,
    stamp: u64,
}

/// One namespace with its access and expiry indexes.
///
/// The three structures are maintained together so eviction and expiry are both
/// O(log n) rather than a scan, and so accounting is exact rather than
/// estimated.
struct Namespace {
    policy: NamespacePolicy,
    entries: HashMap<String, Entry>,
    by_access: BTreeMap<u64, String>,
    by_expiry: BTreeMap<(u64, String), ()>,
    bytes: usize,
}

impl Namespace {
    fn new(policy: NamespacePolicy) -> Self {
        Self {
            policy,
            entries: HashMap::new(),
            by_access: BTreeMap::new(),
            by_expiry: BTreeMap::new(),
            bytes: 0,
        }
    }

    fn charge(key: &str, value: &str) -> usize {
        key.len()
            .saturating_add(value.len())
            .saturating_add(ENTRY_OVERHEAD_BYTES)
    }

    /// Removes one entry and returns the bytes it released.
    fn remove(&mut self, key: &str) -> Option<usize> {
        let entry = self.entries.remove(key)?;
        self.by_access.remove(&entry.stamp);
        self.by_expiry
            .remove(&(entry.expires_at_millis, key.to_owned()));
        let released = Self::charge(key, &entry.value);
        self.bytes = self.bytes.saturating_sub(released);
        Some(released)
    }

    /// Removes the least recently used entry.
    fn evict_one(&mut self) -> Option<usize> {
        let (_, key) = self
            .by_access
            .iter()
            .next()
            .map(|(stamp, key)| (*stamp, key.clone()))?;
        self.remove(&key)
    }

    /// Removes up to [`MAX_SWEEP_PER_OPERATION`] entries that have expired.
    fn sweep_expired(&mut self, now_millis: u64) -> (usize, usize) {
        let mut removed = 0;
        let mut released = 0;
        while removed < MAX_SWEEP_PER_OPERATION {
            let Some(((expires_at, key), ())) = self.by_expiry.iter().next() else {
                break;
            };
            if *expires_at > now_millis {
                break;
            }
            let key = key.clone();
            if let Some(bytes) = self.remove(&key) {
                released += bytes;
            }
            removed += 1;
        }
        (removed, released)
    }
}

struct State {
    namespaces: BTreeMap<String, Namespace>,
    total_entries: usize,
    total_bytes: usize,
    max_total_entries: usize,
    max_total_bytes: usize,
    hits: u64,
    misses: u64,
    expirations: u64,
    evictions: u64,
    puts: u64,
    deletes: u64,
}

/// Bounded process-local TTL cache shared by every invocation on one host.
///
/// The cache is explicitly *not* durable and explicitly *not* transactional. It
/// survives across fresh Wasm stores because the embedding owns it, and it is
/// lost when the host process restarts. A queue or schedule delivery that fails
/// after writing to the cache does **not** roll the cache back; that
/// non-transactional behaviour is part of the contract rather than an accident,
/// which is exactly why a cached value may never be load-bearing.
pub struct Cache {
    state: Mutex<State>,
    stamp: AtomicU64,
    /// Origin of this cache's monotonic timeline.
    origin: Instant,
}

impl Cache {
    /// Builds a cache from a fully validated configuration.
    pub fn new(config: CacheConfig) -> Result<Self, CacheError> {
        config.validate()?;
        let namespaces = config
            .namespaces
            .into_iter()
            .map(|(name, policy)| (name, Namespace::new(policy)))
            .collect();
        Ok(Self {
            state: Mutex::new(State {
                namespaces,
                total_entries: 0,
                total_bytes: 0,
                max_total_entries: config.max_total_entries,
                max_total_bytes: config.max_total_bytes,
                hits: 0,
                misses: 0,
                expirations: 0,
                evictions: 0,
                puts: 0,
                deletes: 0,
            }),
            stamp: AtomicU64::new(0),
            origin: Instant::now(),
        })
    }

    /// A cache with no namespaces. Every operation reports an unconfigured
    /// namespace, which source code must handle like any other miss or outage.
    pub fn empty() -> Self {
        Self {
            state: Mutex::new(State {
                namespaces: BTreeMap::new(),
                total_entries: 0,
                total_bytes: 0,
                max_total_entries: 0,
                max_total_bytes: 0,
                hits: 0,
                misses: 0,
                expirations: 0,
                evictions: 0,
                puts: 0,
                deletes: 0,
            }),
            stamp: AtomicU64::new(0),
            origin: Instant::now(),
        }
    }

    /// The current point on this cache's monotonic timeline.
    ///
    /// Infallible by construction: no wall clock is consulted, so a host clock
    /// fault can never reach guest code.
    pub fn now(&self) -> CacheInstant {
        CacheInstant(u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    pub fn is_empty(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.namespaces.is_empty())
            .unwrap_or(true)
    }

    /// Configured namespace names, for permission and explain output.
    pub fn namespace_names(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|state| state.namespaces.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn mode(&self, namespace: &str) -> Option<NamespaceMode> {
        self.state
            .lock()
            .ok()?
            .namespaces
            .get(namespace)
            .map(|namespace| namespace.policy.mode)
    }

    /// Reads one live entry.
    ///
    /// An entry whose deadline has passed is never returned and is removed on
    /// the spot, so expiry is exact rather than eventual. The caller cannot
    /// distinguish "absent" from "expired", and must not need to.
    pub fn get(
        &self,
        namespace: &str,
        key: &str,
        now: CacheInstant,
    ) -> Result<Option<String>, CacheError> {
        let now_millis = now.0;
        let mut state = self.lock()?;
        let stamp = self.next_stamp();
        let max_key_bytes = state.namespace(namespace)?.policy.max_key_bytes;
        validate_key(key, max_key_bytes)?;
        let entry = state.namespace_mut(namespace)?;
        let Some(existing) = entry.entries.get(key) else {
            state.misses = state.misses.saturating_add(1);
            return Ok(None);
        };
        if existing.expires_at_millis <= now_millis {
            let released = entry.remove(key).unwrap_or(0);
            state.release(1, released);
            state.expirations = state.expirations.saturating_add(1);
            state.misses = state.misses.saturating_add(1);
            return Ok(None);
        }
        let value = existing.value.clone();
        // Refresh the recency position so eviction really is least-recently-used.
        let previous = existing.stamp;
        let entry = state.namespace_mut(namespace)?;
        entry.by_access.remove(&previous);
        entry.by_access.insert(stamp, key.to_owned());
        if let Some(existing) = entry.entries.get_mut(key) {
            existing.stamp = stamp;
        }
        state.hits = state.hits.saturating_add(1);
        Ok(Some(value))
    }

    /// Writes one entry with an explicit bounded time to live.
    ///
    /// This carries *guest* authority: a read-only namespace refuses it. An
    /// embedding that owns the cache should use [`Self::seed`] instead.
    ///
    /// Replacement is accounted exactly: the previous entry's bytes are
    /// released before the new entry's bytes are charged.
    pub fn put(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        ttl_seconds: i64,
        now: CacheInstant,
    ) -> Result<(), CacheError> {
        self.write(namespace, key, value, ttl_seconds, now, Writer::Guest)
    }

    /// Writes one entry with host authority, so a read-only namespace accepts
    /// it.
    ///
    /// This is how an embedding populates reference data that guest code may
    /// read but must never modify. Every other bound - key bytes, value bytes,
    /// time to live, namespace entries and bytes, and the whole-cache budget -
    /// applies exactly as it does to a guest write; only the read-only refusal
    /// is lifted. It is deliberately a distinct method so host seeding can
    /// never be mistaken for guest write authority.
    pub fn seed(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        ttl_seconds: i64,
        now: CacheInstant,
    ) -> Result<(), CacheError> {
        self.write(namespace, key, value, ttl_seconds, now, Writer::Host)
    }

    fn write(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        ttl_seconds: i64,
        now: CacheInstant,
        writer: Writer,
    ) -> Result<(), CacheError> {
        let now_millis = now.0;
        let mut state = self.lock()?;
        let stamp = self.next_stamp();
        let policy = state.namespace(namespace)?.policy;
        if policy.mode == NamespaceMode::ReadOnly && writer == Writer::Guest {
            return Err(CacheError::namespace(
                "cache namespace is configured read only",
            ));
        }
        validate_key(key, policy.max_key_bytes)?;
        if value.len() > policy.max_value_bytes {
            return Err(CacheError::limit(
                "cache value exceeds its configured byte bound",
            ));
        }
        if ttl_seconds < crate::config::MIN_TTL_SECONDS || ttl_seconds > policy.max_ttl_seconds {
            return Err(CacheError::limit(
                "cache time to live is outside its configured bounds",
            ));
        }
        let expires_at_millis = u64::try_from(ttl_seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
            .and_then(|millis| now_millis.checked_add(millis))
            .ok_or_else(|| CacheError::limit("cache deadline overflowed"))?;

        let charge = Namespace::charge(key, value);
        let entry = state.namespace_mut(namespace)?;
        let (swept, swept_bytes) = entry.sweep_expired(now_millis);
        state.release(swept, swept_bytes);
        state.expirations = state.expirations.saturating_add(swept as u64);

        // Replacement releases the old cost before the new cost is charged.
        let entry = state.namespace_mut(namespace)?;
        if let Some(released) = entry.remove(key) {
            state.release(1, released);
        }

        // Evict until the new entry fits both the namespace and global budgets.
        loop {
            let entry = state.namespace_mut(namespace)?;
            let namespace_fits = entry.entries.len() < entry.policy.max_entries
                && entry.bytes.saturating_add(charge) <= entry.policy.max_bytes;
            if !namespace_fits {
                let Some(released) = entry.evict_one() else {
                    return Err(CacheError::limit(
                        "cache entry cannot fit its namespace budget",
                    ));
                };
                state.release(1, released);
                state.evictions = state.evictions.saturating_add(1);
                continue;
            }
            let global_fits = state.total_entries < state.max_total_entries
                && state.total_bytes.saturating_add(charge) <= state.max_total_bytes;
            if global_fits {
                break;
            }
            // The destination namespace already fits. Whole-cache pressure is
            // therefore resolved by exact global recency: the least recently
            // used entry anywhere is evicted, which may well live in another
            // namespace. Preferring the destination namespace here would
            // discard a fresh local entry while a globally older one survived.
            let Some(released) = state.evict_globally() else {
                return Err(CacheError::limit(
                    "cache entry cannot fit the whole-cache budget",
                ));
            };
            state.release(1, released);
            state.evictions = state.evictions.saturating_add(1);
        }

        let entry = state.namespace_mut(namespace)?;
        entry.entries.insert(
            key.to_owned(),
            Entry {
                value: value.to_owned(),
                expires_at_millis,
                stamp,
            },
        );
        entry.by_access.insert(stamp, key.to_owned());
        entry
            .by_expiry
            .insert((expires_at_millis, key.to_owned()), ());
        entry.bytes = entry.bytes.saturating_add(charge);
        state.total_entries = state.total_entries.saturating_add(1);
        state.total_bytes = state.total_bytes.saturating_add(charge);
        state.puts = state.puts.saturating_add(1);
        Ok(())
    }

    /// Removes one entry. Removing an absent entry is not an error: the
    /// postcondition - the key is not cached - already holds.
    pub fn delete(&self, namespace: &str, key: &str) -> Result<(), CacheError> {
        let mut state = self.lock()?;
        let policy = state.namespace(namespace)?.policy;
        if policy.mode == NamespaceMode::ReadOnly {
            return Err(CacheError::namespace(
                "cache namespace is configured read only",
            ));
        }
        validate_key(key, policy.max_key_bytes)?;
        let entry = state.namespace_mut(namespace)?;
        if let Some(released) = entry.remove(key) {
            state.release(1, released);
        }
        state.deletes = state.deletes.saturating_add(1);
        Ok(())
    }

    pub fn stats(&self) -> CacheStats {
        let Ok(state) = self.state.lock() else {
            return CacheStats::default();
        };
        CacheStats {
            hits: state.hits,
            misses: state.misses,
            expirations: state.expirations,
            evictions: state.evictions,
            puts: state.puts,
            deletes: state.deletes,
            entries: state.total_entries as u64,
            bytes: state.total_bytes as u64,
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, CacheError> {
        self.state
            .lock()
            .map_err(|_| CacheError::unavailable("cache backend is unavailable"))
    }

    fn next_stamp(&self) -> u64 {
        self.stamp
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .wrapping_add(1)
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keys and values are deliberately absent from every rendering.
        formatter
            .debug_struct("Cache")
            .field("namespaces", &self.namespace_names())
            .finish_non_exhaustive()
    }
}

impl State {
    fn namespace(&self, name: &str) -> Result<&Namespace, CacheError> {
        self.namespaces
            .get(name)
            .ok_or_else(|| CacheError::namespace("cache namespace is not configured"))
    }

    fn namespace_mut(&mut self, name: &str) -> Result<&mut Namespace, CacheError> {
        self.namespaces
            .get_mut(name)
            .ok_or_else(|| CacheError::namespace("cache namespace is not configured"))
    }

    fn release(&mut self, entries: usize, bytes: usize) {
        self.total_entries = self.total_entries.saturating_sub(entries);
        self.total_bytes = self.total_bytes.saturating_sub(bytes);
    }

    /// Evicts the least recently used entry across every namespace.
    fn evict_globally(&mut self) -> Option<usize> {
        let target = self
            .namespaces
            .iter()
            .filter_map(|(name, namespace)| {
                namespace
                    .by_access
                    .iter()
                    .next()
                    .map(|(stamp, _)| (*stamp, name.clone()))
            })
            .min_by_key(|(stamp, _)| *stamp)
            .map(|(_, name)| name)?;
        self.namespaces
            .get_mut(&target)
            .and_then(Namespace::evict_one)
    }
}

fn validate_key(key: &str, max_key_bytes: usize) -> Result<(), CacheError> {
    if key.is_empty() {
        return Err(CacheError::limit("cache key must not be empty"));
    }
    if key.len() > max_key_bytes {
        return Err(CacheError::limit(
            "cache key exceeds its configured byte bound",
        ));
    }
    if key.contains('\0') || key.chars().any(|character| character.is_control()) {
        return Err(CacheError::limit(
            "cache key must not contain control characters",
        ));
    }
    Ok(())
}
