use std::{collections::BTreeMap, sync::Arc};

use krit_cache::{Cache, CacheError, CacheErrorKind};

use crate::{
    RuntimeError,
    search::{SearchConnectorConfig, SearchKind},
};

pub use krit_cache::{
    CacheConfig, CacheInstant, CacheStats, MAX_ENTRIES_PER_NAMESPACE, MAX_KEY_BYTES,
    MAX_NAMESPACE_BYTES, MAX_NAMESPACES, MAX_TOTAL_BYTES, MAX_TOTAL_ENTRIES, MAX_TTL_SECONDS,
    MAX_VALUE_BYTES, MIN_TTL_SECONDS, NamespaceMode, NamespacePolicy,
};

/// Host-owned cache shared by every invocation on one embedding.
///
/// The handle is cloneable and cheap: a fresh Wasm `Store` per invocation still
/// observes the same cache, which is exactly why the cache must never carry
/// meaning that an invocation depends on. A restart of the host process loses
/// every entry, and that loss is a normal, documented outcome.
#[derive(Clone)]
pub struct CacheHandle {
    cache: Arc<Cache>,
}

impl Default for CacheHandle {
    fn default() -> Self {
        Self {
            cache: Arc::new(Cache::empty()),
        }
    }
}

impl CacheHandle {
    /// Builds a shared cache from a fully validated configuration.
    pub fn open(config: CacheConfig) -> Result<Self, RuntimeError> {
        Cache::new(config)
            .map(|cache| Self {
                cache: Arc::new(cache),
            })
            .map_err(map_cache_error)
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn namespace_names(&self) -> Vec<String> {
        self.cache.namespace_names()
    }

    pub fn mode(&self, namespace: &str) -> Option<NamespaceMode> {
        self.cache.mode(namespace)
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// The current point on the cache's monotonic timeline. Infallible.
    pub fn now(&self) -> CacheInstant {
        self.cache.now()
    }

    /// Reads one entry. Public because the embedding owns the cache and may
    /// legitimately inspect or seed it; the host clock is always explicit.
    pub fn get(
        &self,
        namespace: &str,
        key: &str,
        now: CacheInstant,
    ) -> Result<Option<String>, RuntimeError> {
        self.cache.get(namespace, key, now).map_err(map_cache_error)
    }

    pub fn put(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        ttl_seconds: i64,
        now: CacheInstant,
    ) -> Result<(), RuntimeError> {
        self.cache
            .put(namespace, key, value, ttl_seconds, now)
            .map_err(map_cache_error)
    }

    /// Seeds one entry with host authority, so a read-only namespace accepts
    /// it. Guest code can still only read such a namespace.
    pub fn seed(
        &self,
        namespace: &str,
        key: &str,
        value: &str,
        ttl_seconds: i64,
        now: CacheInstant,
    ) -> Result<(), RuntimeError> {
        self.cache
            .seed(namespace, key, value, ttl_seconds, now)
            .map_err(map_cache_error)
    }

    pub fn delete(&self, namespace: &str, key: &str) -> Result<(), RuntimeError> {
        self.cache.delete(namespace, key).map_err(map_cache_error)
    }
}

impl std::fmt::Debug for CacheHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CacheHandle")
            .field("namespaces", &self.namespace_names())
            .finish_non_exhaustive()
    }
}

pub(crate) fn map_cache_error(error: CacheError) -> RuntimeError {
    match error.kind() {
        CacheErrorKind::Namespace => RuntimeError::cache(error.message()),
        CacheErrorKind::Limit => RuntimeError::cache_limit(error.message()),
        CacheErrorKind::Unavailable => RuntimeError::cache_unavailable(error.message()),
    }
}

/// Host-owned set of configured search and vector connectors.
#[derive(Clone, Default)]
pub struct SearchCatalog {
    connectors: Arc<BTreeMap<String, SearchConnectorConfig>>,
}

impl SearchCatalog {
    /// Builds a catalog from fully validated connector definitions.
    pub fn open(connectors: BTreeMap<String, SearchConnectorConfig>) -> Result<Self, RuntimeError> {
        if connectors.len() > crate::search::MAX_SEARCH_CONNECTORS {
            return Err(RuntimeError::search(
                "configured search connectors exceed the Phase 7 bound",
            ));
        }
        for (name, connector) in &connectors {
            if !krit_capability::is_valid_resource_name(name) {
                return Err(RuntimeError::search(
                    "search connector name must use the canonical resource grammar",
                ));
            }
            // Everything except the transport scheme is checked here. The
            // scheme is policy-dependent, so it is enforced where the policy is
            // known: the CLI requires HTTPS in phase-one validation, and
            // `validate_agent_host` re-validates every connector against this
            // host's network policy before any invocation runs. A default
            // policy refuses plaintext, so an embedding cannot bypass it.
            connector.validate_with_plaintext_allowance(true)?;
        }
        Ok(Self {
            connectors: Arc::new(connectors),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }

    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.connectors.keys().map(String::as_str)
    }

    pub fn kind(&self, name: &str) -> Option<SearchKind> {
        self.connectors.get(name).map(|connector| connector.kind)
    }

    pub(crate) fn connector(&self, name: &str) -> Option<&SearchConnectorConfig> {
        self.connectors.get(name)
    }

    pub(crate) fn connectors(&self) -> impl Iterator<Item = (&str, &SearchConnectorConfig)> {
        self.connectors
            .iter()
            .map(|(name, connector)| (name.as_str(), connector))
    }
}

impl std::fmt::Debug for SearchCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Endpoints, paths, and secret names are absent from every rendering.
        formatter
            .debug_struct("SearchCatalog")
            .field("connectors", &self.connectors.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// Invocation-local cache and search counters.
#[derive(Debug, Default)]
pub(crate) struct InvocationCache {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) writes: u64,
    pub(crate) deletes: u64,
    pub(crate) errors: u64,
    pub(crate) search_calls: u64,
    pub(crate) vector_calls: u64,
}
