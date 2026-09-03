//! Bounded namespaced TTL cache backend for Krit.
//!
//! The cache is deliberately *process local* and deliberately *optional*. It is
//! an availability optimisation, never a correctness mechanism: a miss, an
//! expiry, an eviction, and an outage are all reported to the caller as
//! ordinary values so that source code chooses its own fallback. Nothing in the
//! compiler, the artifact, or the runtime outcome model depends on a cache
//! entry being present.
//!
//! Every dimension is bounded: namespaces, entries per namespace, total
//! entries, key bytes, value bytes, bytes per namespace, total bytes, and time
//! to live. There is no background thread, no ambient default cache, no
//! filesystem path, and no unbounded map.

#![forbid(unsafe_code)]

mod config;
mod error;
mod store;

pub use config::{
    CacheConfig, ENTRY_OVERHEAD_BYTES, MAX_ENTRIES_PER_NAMESPACE, MAX_KEY_BYTES,
    MAX_NAMESPACE_BYTES, MAX_NAMESPACES, MAX_TOTAL_BYTES, MAX_TOTAL_ENTRIES, MAX_TTL_SECONDS,
    MAX_VALUE_BYTES, MIN_TTL_SECONDS, NamespaceMode, NamespacePolicy,
};
pub use error::{CacheError, CacheErrorKind};
pub use store::{Cache, CacheInstant, CacheStats};
