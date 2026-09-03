use std::collections::BTreeMap;

use crate::error::CacheError;

/// Hard bound on configured cache namespaces.
pub const MAX_NAMESPACES: usize = 16;
/// Hard bound on live entries in one namespace.
pub const MAX_ENTRIES_PER_NAMESPACE: usize = 4096;
/// Hard bound on live entries across every namespace.
pub const MAX_TOTAL_ENTRIES: usize = 16_384;
/// Hard bound on one cache key.
pub const MAX_KEY_BYTES: usize = 512;
/// Hard bound on one cached value.
pub const MAX_VALUE_BYTES: usize = 64 * 1024;
/// Hard bound on the accounted bytes held by one namespace.
pub const MAX_NAMESPACE_BYTES: usize = 8 * 1024 * 1024;
/// Hard bound on the accounted bytes held by every namespace together.
pub const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Shortest time to live a caller may request.
pub const MIN_TTL_SECONDS: i64 = 1;
/// Longest time to live a caller may request.
pub const MAX_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;
/// Fixed per-entry accounting overhead.
///
/// Charged so that a namespace full of tiny entries still consumes its byte
/// budget honestly rather than appearing free.
pub const ENTRY_OVERHEAD_BYTES: usize = 64;

/// Authority the host grants over one namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NamespaceMode {
    /// Reads are allowed; writes and deletes are refused.
    ReadOnly,
    ReadWrite,
}

impl NamespaceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }
}

/// Bounded policy for one namespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamespacePolicy {
    pub mode: NamespaceMode,
    pub max_entries: usize,
    pub max_bytes: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
    pub max_ttl_seconds: i64,
}

impl NamespacePolicy {
    pub(crate) fn validate(&self) -> Result<(), CacheError> {
        if self.max_entries == 0
            || self.max_entries > MAX_ENTRIES_PER_NAMESPACE
            || self.max_bytes == 0
            || self.max_bytes > MAX_NAMESPACE_BYTES
            || self.max_key_bytes == 0
            || self.max_key_bytes > MAX_KEY_BYTES
            || self.max_value_bytes == 0
            || self.max_value_bytes > MAX_VALUE_BYTES
            || self.max_ttl_seconds < MIN_TTL_SECONDS
            || self.max_ttl_seconds > MAX_TTL_SECONDS
        {
            return Err(CacheError::limit(
                "cache namespace limits are outside the Phase 7 bounds",
            ));
        }
        // One entry at its largest allowed size must fit the namespace budget,
        // otherwise every write would be refused after the fact.
        let largest = self
            .max_key_bytes
            .checked_add(self.max_value_bytes)
            .and_then(|bytes| bytes.checked_add(ENTRY_OVERHEAD_BYTES))
            .ok_or_else(|| CacheError::limit("cache namespace limits overflowed"))?;
        if largest > self.max_bytes {
            return Err(CacheError::limit(
                "cache namespace byte budget cannot hold one entry of its declared maximum size",
            ));
        }
        Ok(())
    }
}

/// Whole-cache configuration, owned by the host.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CacheConfig {
    pub namespaces: BTreeMap<String, NamespacePolicy>,
    pub max_total_entries: usize,
    pub max_total_bytes: usize,
}

impl CacheConfig {
    pub(crate) fn validate(&self) -> Result<(), CacheError> {
        if self.namespaces.len() > MAX_NAMESPACES {
            return Err(CacheError::limit(
                "configured cache namespaces exceed the Phase 7 bound",
            ));
        }
        if self.namespaces.is_empty() {
            // An empty configuration is valid and means "no cache": every
            // operation then reports an unconfigured namespace.
            return Ok(());
        }
        if self.max_total_entries == 0
            || self.max_total_entries > MAX_TOTAL_ENTRIES
            || self.max_total_bytes == 0
            || self.max_total_bytes > MAX_TOTAL_BYTES
        {
            return Err(CacheError::limit(
                "cache totals are outside the Phase 7 bounds",
            ));
        }
        for policy in self.namespaces.values() {
            policy.validate()?;
            if policy.max_entries > self.max_total_entries
                || policy.max_bytes > self.max_total_bytes
            {
                return Err(CacheError::limit(
                    "a cache namespace budget exceeds the whole-cache budget",
                ));
            }
        }
        Ok(())
    }
}
