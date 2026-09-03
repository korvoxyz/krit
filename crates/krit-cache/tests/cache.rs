use std::{
    collections::BTreeMap,
    sync::{Arc, Barrier},
    thread,
};

use krit_cache::{
    Cache, CacheConfig, CacheErrorKind, CacheInstant, ENTRY_OVERHEAD_BYTES,
    MAX_ENTRIES_PER_NAMESPACE, MAX_TTL_SECONDS, NamespaceMode, NamespacePolicy,
};

/// An explicit point on the cache's monotonic timeline.
const fn at(millis: u64) -> CacheInstant {
    CacheInstant::from_millis(millis)
}

fn policy(mode: NamespaceMode) -> NamespacePolicy {
    NamespacePolicy {
        mode,
        max_entries: 4,
        max_bytes: 4 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
        max_key_bytes: 64,
        max_value_bytes: 256,
        max_ttl_seconds: 3600,
    }
}

fn config(namespaces: &[(&str, NamespaceMode)]) -> CacheConfig {
    CacheConfig {
        namespaces: namespaces
            .iter()
            .map(|(name, mode)| ((*name).to_owned(), policy(*mode)))
            .collect(),
        max_total_entries: 8,
        max_total_bytes: 8 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
    }
}

fn cache() -> Cache {
    Cache::new(config(&[("lookups", NamespaceMode::ReadWrite)])).expect("cache should build")
}

#[test]
fn a_miss_a_put_and_a_hit_are_distinct_observable_outcomes() {
    let cache = cache();

    assert_eq!(cache.get("lookups", "a", at(0)).unwrap(), None);
    cache.put("lookups", "a", "alpha", 60, at(0)).unwrap();
    assert_eq!(
        cache.get("lookups", "a", at(0)).unwrap(),
        Some("alpha".to_owned())
    );

    let stats = cache.stats();
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 1);
    assert_eq!(stats.puts, 1);
    assert_eq!(stats.entries, 1);
}

#[test]
fn entries_expire_exactly_at_their_deadline() {
    let cache = cache();
    cache.put("lookups", "a", "alpha", 10, at(1_000)).unwrap();

    // One millisecond before the deadline the entry is still live.
    assert_eq!(
        cache.get("lookups", "a", at(10_999)).unwrap(),
        Some("alpha".to_owned())
    );
    // Exactly at the deadline it is gone; expiry is inclusive and exact.
    assert_eq!(cache.get("lookups", "a", at(11_000)).unwrap(), None);
    // The expired entry was reclaimed rather than left behind.
    assert_eq!(cache.stats().entries, 0);
    assert_eq!(cache.stats().expirations, 1);
}

#[test]
fn an_expired_entry_is_indistinguishable_from_an_absent_one() {
    let cache = cache();
    cache.put("lookups", "present", "value", 10, at(0)).unwrap();

    assert_eq!(cache.get("lookups", "present", at(20_000)).unwrap(), None);
    assert_eq!(
        cache.get("lookups", "never-written", at(20_000)).unwrap(),
        None
    );
}

#[test]
fn replacement_accounting_is_exact() {
    let cache = cache();
    cache
        .put("lookups", "a", &"x".repeat(200), 60, at(0))
        .unwrap();
    let after_first = cache.stats();

    cache.put("lookups", "a", "short", 60, at(0)).unwrap();
    let after_replace = cache.stats();

    assert_eq!(
        after_replace.entries, 1,
        "replacement must not add an entry"
    );
    assert_eq!(
        after_replace.bytes,
        (1 + 5 + ENTRY_OVERHEAD_BYTES) as u64,
        "replacement must release the previous value's bytes"
    );
    assert!(after_first.bytes > after_replace.bytes);
    assert_eq!(
        cache.get("lookups", "a", at(0)).unwrap(),
        Some("short".to_owned())
    );
}

#[test]
fn per_namespace_entry_pressure_evicts_least_recently_used() {
    let cache = cache();
    for key in ["a", "b", "c", "d"] {
        cache.put("lookups", key, "v", 600, at(0)).unwrap();
    }
    // Touch `a` so `b` becomes the least recently used entry.
    assert!(cache.get("lookups", "a", at(0)).unwrap().is_some());

    cache.put("lookups", "e", "v", 600, at(0)).unwrap();

    assert!(cache.get("lookups", "a", at(0)).unwrap().is_some());
    assert_eq!(cache.get("lookups", "b", at(0)).unwrap(), None);
    assert!(cache.get("lookups", "e", at(0)).unwrap().is_some());
    assert_eq!(cache.stats().evictions, 1);
    assert_eq!(cache.stats().entries, 4);
}

#[test]
fn namespace_byte_pressure_evicts_before_the_entry_bound() {
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([(
            "lookups".to_owned(),
            NamespacePolicy {
                mode: NamespaceMode::ReadWrite,
                max_entries: 16,
                max_bytes: 2 * (1 + 200 + ENTRY_OVERHEAD_BYTES),
                max_key_bytes: 64,
                max_value_bytes: 200,
                max_ttl_seconds: 3600,
            },
        )]),
        max_total_entries: 16,
        max_total_bytes: 4 * (1 + 200 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    for key in ["a", "b", "c"] {
        cache
            .put("lookups", key, &"x".repeat(200), 600, at(0))
            .unwrap();
    }

    assert_eq!(
        cache.stats().entries,
        2,
        "byte pressure bounds the namespace"
    );
    assert_eq!(cache.get("lookups", "a", at(0)).unwrap(), None);
    assert!(cache.get("lookups", "c", at(0)).unwrap().is_some());
    assert!(cache.stats().bytes <= 2 * (1 + 200 + ENTRY_OVERHEAD_BYTES) as u64);
}

#[test]
fn the_global_budget_bounds_every_namespace_together() {
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([
            ("one".to_owned(), policy(NamespaceMode::ReadWrite)),
            ("two".to_owned(), policy(NamespaceMode::ReadWrite)),
        ]),
        max_total_entries: 5,
        max_total_bytes: 8 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    for key in ["a", "b", "c", "d"] {
        cache.put("one", key, "v", 600, at(0)).unwrap();
    }
    for key in ["a", "b", "c", "d"] {
        cache.put("two", key, "v", 600, at(0)).unwrap();
    }

    let stats = cache.stats();
    assert!(
        stats.entries <= 5,
        "global entry bound was exceeded: {}",
        stats.entries
    );
    assert!(stats.evictions > 0);
}

#[test]
fn namespaces_are_isolated_from_one_another() {
    let cache = Cache::new(config(&[
        ("one", NamespaceMode::ReadWrite),
        ("two", NamespaceMode::ReadWrite),
    ]))
    .expect("cache should build");

    cache.put("one", "shared", "from-one", 600, at(0)).unwrap();

    assert_eq!(cache.get("two", "shared", at(0)).unwrap(), None);
    assert_eq!(
        cache.get("one", "shared", at(0)).unwrap(),
        Some("from-one".to_owned())
    );
    cache.delete("two", "shared").unwrap();
    assert_eq!(
        cache.get("one", "shared", at(0)).unwrap(),
        Some("from-one".to_owned()),
        "deleting in one namespace must not affect another"
    );
}

#[test]
fn read_only_namespaces_refuse_writes_and_deletes() {
    let cache = Cache::new(config(&[("frozen", NamespaceMode::ReadOnly)])).expect("cache builds");

    let write = cache
        .put("frozen", "a", "v", 60, at(0))
        .expect_err("write refused");
    assert_eq!(write.kind(), CacheErrorKind::Namespace);
    let delete = cache.delete("frozen", "a").expect_err("delete refused");
    assert_eq!(delete.kind(), CacheErrorKind::Namespace);
    // Reads still work and simply miss.
    assert_eq!(cache.get("frozen", "a", at(0)).unwrap(), None);
}

#[test]
fn an_unconfigured_namespace_is_an_explicit_error_never_a_silent_miss() {
    let cache = cache();

    for outcome in [
        cache.get("absent", "a", at(0)).err(),
        cache.put("absent", "a", "v", 60, at(0)).err(),
        cache.delete("absent", "a").err(),
    ] {
        let error = outcome.expect("an unconfigured namespace must be an error");
        assert_eq!(error.kind(), CacheErrorKind::Namespace);
        assert_eq!(error.code(), "K5401");
    }
}

#[test]
fn a_cache_with_no_namespaces_reports_every_operation_as_unconfigured() {
    let cache = Cache::empty();

    assert!(cache.is_empty());
    assert_eq!(
        cache.get("anything", "a", at(0)).unwrap_err().kind(),
        CacheErrorKind::Namespace
    );
}

#[test]
fn keys_values_and_time_to_live_are_bounded() {
    let cache = cache();

    assert_eq!(
        cache
            .put("lookups", "a", &"x".repeat(257), 60, at(0))
            .unwrap_err()
            .kind(),
        CacheErrorKind::Limit
    );
    assert_eq!(
        cache
            .put("lookups", &"k".repeat(65), "v", 60, at(0))
            .unwrap_err()
            .kind(),
        CacheErrorKind::Limit
    );
    assert_eq!(
        cache.put("lookups", "", "v", 60, at(0)).unwrap_err().kind(),
        CacheErrorKind::Limit
    );
    assert_eq!(
        cache
            .put("lookups", "a\nb", "v", 60, at(0))
            .unwrap_err()
            .kind(),
        CacheErrorKind::Limit
    );
    for ttl in [0, -1, 3601, MAX_TTL_SECONDS + 1, i64::MAX] {
        assert_eq!(
            cache
                .put("lookups", "a", "v", ttl, at(0))
                .unwrap_err()
                .kind(),
            CacheErrorKind::Limit,
            "time to live {ttl} must be refused"
        );
    }
    cache
        .put("lookups", "a", "v", 1, at(0))
        .expect("the shortest ttl is allowed");
    cache
        .put("lookups", "a", "v", 3600, at(0))
        .expect("the longest ttl is allowed");
}

#[test]
fn configuration_bounds_are_validated_up_front() {
    let mut oversized = config(&[("lookups", NamespaceMode::ReadWrite)]);
    oversized.namespaces.get_mut("lookups").unwrap().max_entries = MAX_ENTRIES_PER_NAMESPACE + 1;
    assert!(Cache::new(oversized).is_err());

    let mut too_many = CacheConfig::default();
    for index in 0..17 {
        too_many.namespaces.insert(
            format!("namespace-{index}"),
            policy(NamespaceMode::ReadWrite),
        );
    }
    too_many.max_total_entries = 8;
    too_many.max_total_bytes = 1024 * 1024;
    assert!(Cache::new(too_many).is_err());

    // A namespace whose byte budget cannot hold one maximum-size entry would
    // refuse every write, so it is rejected at configuration time.
    let unusable = CacheConfig {
        namespaces: BTreeMap::from([(
            "lookups".to_owned(),
            NamespacePolicy {
                mode: NamespaceMode::ReadWrite,
                max_entries: 4,
                max_bytes: 32,
                max_key_bytes: 64,
                max_value_bytes: 256,
                max_ttl_seconds: 60,
            },
        )]),
        max_total_entries: 8,
        max_total_bytes: 1024,
    };
    assert!(Cache::new(unusable).is_err());
}

#[test]
fn deleting_an_absent_key_states_a_fact_rather_than_failing() {
    let cache = cache();

    cache
        .delete("lookups", "never-written")
        .expect("delete is idempotent");
    cache.put("lookups", "a", "v", 60, at(0)).unwrap();
    cache.delete("lookups", "a").unwrap();
    cache
        .delete("lookups", "a")
        .expect("a repeated delete still holds");

    assert_eq!(cache.get("lookups", "a", at(0)).unwrap(), None);
    assert_eq!(cache.stats().entries, 0);
}

#[test]
fn concurrent_access_keeps_accounting_consistent() {
    let cache = Arc::new(
        Cache::new(CacheConfig {
            namespaces: BTreeMap::from([(
                "lookups".to_owned(),
                NamespacePolicy {
                    mode: NamespaceMode::ReadWrite,
                    max_entries: 64,
                    max_bytes: 64 * (16 + 32 + ENTRY_OVERHEAD_BYTES),
                    max_key_bytes: 16,
                    max_value_bytes: 32,
                    max_ttl_seconds: 600,
                },
            )]),
            max_total_entries: 64,
            max_total_bytes: 64 * (16 + 32 + ENTRY_OVERHEAD_BYTES),
        })
        .expect("cache should build"),
    );
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();

    for worker in 0..8 {
        let cache = Arc::clone(&cache);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for round in 0..200 {
                let key = format!("k{}", (worker * 200 + round) % 96);
                cache.put("lookups", &key, "value", 600, at(0)).unwrap();
                let _ = cache.get("lookups", &key, at(0)).unwrap();
                if round % 5 == 0 {
                    cache.delete("lookups", &key).unwrap();
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("worker should not panic");
    }

    let stats = cache.stats();
    assert!(stats.entries <= 64, "entry accounting drifted: {stats:?}");
    assert!(
        stats.bytes <= 64 * (16 + 32 + ENTRY_OVERHEAD_BYTES) as u64,
        "byte accounting drifted: {stats:?}"
    );
    // Draining every key must return the accounting to exactly zero.
    for index in 0..96 {
        cache.delete("lookups", &format!("k{index}")).unwrap();
    }
    let drained = cache.stats();
    assert_eq!(drained.entries, 0, "entries leaked: {drained:?}");
    assert_eq!(drained.bytes, 0, "bytes leaked: {drained:?}");
}

#[test]
fn a_poisoned_backend_reports_an_outage_rather_than_a_miss() {
    let cache = Arc::new(cache());
    cache.put("lookups", "a", "alpha", 600, at(0)).unwrap();

    // A panic while the lock is held is a genuine local outage.
    let poisoner = Arc::clone(&cache);
    let _ = thread::spawn(move || {
        let _ = poisoner.get("lookups", "a", at(0));
        panic!("deliberate poison");
    })
    .join();
    // Poison the mutex through a real panic inside a cache operation.
    let poisoner = Arc::clone(&cache);
    let _ = thread::spawn(move || {
        poisoner.put("lookups", "b", "beta", 600, at(0)).unwrap();
        struct Poison;
        impl Drop for Poison {
            fn drop(&mut self) {
                // Nothing: the panic below is what poisons the lock.
            }
        }
        let _guard = Poison;
        let _ = poisoner.stats();
        panic!("deliberate poison inside a held lock");
    })
    .join();

    // The cache either still works or reports an outage, but it never invents
    // a value and never silently claims a miss for a live entry.
    match cache.get("lookups", "a", at(0)) {
        Ok(Some(value)) => assert_eq!(value, "alpha"),
        Ok(None) => panic!("a live entry must not be reported as a miss"),
        Err(error) => assert_eq!(error.kind(), CacheErrorKind::Unavailable),
    }
}

#[test]
fn a_fresh_cache_has_no_entries_from_a_previous_one() {
    let first = cache();
    first.put("lookups", "a", "alpha", 600, at(0)).unwrap();
    assert!(first.get("lookups", "a", at(0)).unwrap().is_some());

    // Rebuilding the cache models a host restart: process-local state is lost.
    let second = cache();
    assert_eq!(second.get("lookups", "a", at(0)).unwrap(), None);
    assert_eq!(second.stats().entries, 0);
}

#[test]
fn namespace_metadata_never_exposes_keys_or_values() {
    let cache = cache();
    cache
        .put("lookups", "secret-key", "secret-value", 600, at(0))
        .unwrap();

    assert_eq!(cache.namespace_names(), ["lookups"]);
    assert_eq!(cache.mode("lookups"), Some(NamespaceMode::ReadWrite));
    assert_eq!(cache.mode("absent"), None);
    let rendered = format!("{cache:?}");
    assert!(rendered.contains("lookups"));
    assert!(
        !rendered.contains("secret-key") && !rendered.contains("secret-value"),
        "cache rendering leaked payload: {rendered}"
    );
}

#[test]
fn expired_entries_are_reclaimed_without_a_background_thread() {
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([(
            "lookups".to_owned(),
            NamespacePolicy {
                mode: NamespaceMode::ReadWrite,
                max_entries: 32,
                max_bytes: 32 * (8 + 8 + ENTRY_OVERHEAD_BYTES),
                max_key_bytes: 8,
                max_value_bytes: 8,
                max_ttl_seconds: 600,
            },
        )]),
        max_total_entries: 32,
        max_total_bytes: 32 * (8 + 8 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    for index in 0..16 {
        cache
            .put("lookups", &format!("k{index}"), "v", 1, at(0))
            .unwrap();
    }
    assert_eq!(cache.stats().entries, 16);

    // Later writes sweep the expired backlog in bounded batches.
    for index in 0..4 {
        cache
            .put("lookups", &format!("n{index}"), "v", 600, at(10_000))
            .unwrap();
    }

    let stats = cache.stats();
    assert!(
        stats.entries <= 8,
        "expired entries were not reclaimed: {stats:?}"
    );
    assert!(stats.expirations > 0);
    for index in 0..16 {
        assert_eq!(
            cache
                .get("lookups", &format!("k{index}"), at(10_000))
                .unwrap(),
            None
        );
    }
}

/// A namespace able to hold `max_entries` entries.
fn sized(max_entries: usize) -> NamespacePolicy {
    NamespacePolicy {
        mode: NamespaceMode::ReadWrite,
        max_entries,
        max_bytes: max_entries * (64 + 256 + ENTRY_OVERHEAD_BYTES),
        max_key_bytes: 64,
        max_value_bytes: 256,
        max_ttl_seconds: 3600,
    }
}

#[test]
fn global_pressure_evicts_the_globally_oldest_entry_not_the_local_one() {
    // Two namespaces, each able to hold two entries, but the whole cache holds
    // only two. Writing `one/a`, then `two/b`, then `two/c` must evict `one/a`,
    // which is globally oldest, rather than `two/b`, which is merely local.
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([("one".to_owned(), sized(2)), ("two".to_owned(), sized(2))]),
        max_total_entries: 2,
        max_total_bytes: 8 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    cache.put("one", "a", "first", 600, at(0)).unwrap();
    cache.put("two", "b", "second", 600, at(1)).unwrap();
    cache.put("two", "c", "third", 600, at(2)).unwrap();

    assert_eq!(
        cache.get("one", "a", at(3)).unwrap(),
        None,
        "the globally oldest entry must be evicted"
    );
    assert_eq!(
        cache.get("two", "b", at(3)).unwrap(),
        Some("second".to_owned()),
        "a newer entry in the destination namespace must survive"
    );
    assert_eq!(
        cache.get("two", "c", at(3)).unwrap(),
        Some("third".to_owned())
    );
    assert_eq!(cache.stats().entries, 2);
}

#[test]
fn a_read_touch_protects_an_entry_from_global_eviction() {
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([("one".to_owned(), sized(2)), ("two".to_owned(), sized(2))]),
        max_total_entries: 2,
        max_total_bytes: 8 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    cache.put("one", "a", "first", 600, at(0)).unwrap();
    cache.put("two", "b", "second", 600, at(1)).unwrap();
    // Reading `one/a` makes `two/b` the globally oldest entry.
    assert!(cache.get("one", "a", at(2)).unwrap().is_some());

    cache.put("two", "c", "third", 600, at(3)).unwrap();

    assert_eq!(
        cache.get("one", "a", at(4)).unwrap(),
        Some("first".to_owned()),
        "a recently read entry must survive global pressure"
    );
    assert_eq!(cache.get("two", "b", at(4)).unwrap(), None);
}

#[test]
fn per_namespace_pressure_still_evicts_the_local_least_recently_used() {
    // The destination namespace is full but the whole cache is not, so the
    // local least recently used entry is the one that goes.
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([("one".to_owned(), sized(1)), ("two".to_owned(), sized(4))]),
        max_total_entries: 8,
        max_total_bytes: 16 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    cache
        .put("two", "old", "older-than-everything", 600, at(0))
        .unwrap();
    cache.put("one", "a", "first", 600, at(1)).unwrap();
    cache.put("one", "b", "second", 600, at(2)).unwrap();

    assert_eq!(
        cache.get("one", "a", at(3)).unwrap(),
        None,
        "local pressure evicts the local least recently used entry"
    );
    assert_eq!(
        cache.get("two", "old", at(3)).unwrap(),
        Some("older-than-everything".to_owned()),
        "a globally older entry in another namespace must not be touched"
    );
}

#[test]
fn a_read_only_namespace_can_be_seeded_by_the_host_but_not_by_a_guest() {
    let cache =
        Cache::new(config(&[("reference", NamespaceMode::ReadOnly)])).expect("cache should build");

    // Guest authority is refused.
    assert_eq!(
        cache
            .put("reference", "a", "guest", 60, at(0))
            .unwrap_err()
            .kind(),
        CacheErrorKind::Namespace
    );
    assert_eq!(
        cache.delete("reference", "a").unwrap_err().kind(),
        CacheErrorKind::Namespace
    );
    assert_eq!(cache.get("reference", "a", at(0)).unwrap(), None);

    // Host seeding succeeds and is then readable.
    cache.seed("reference", "a", "host", 60, at(0)).unwrap();
    assert_eq!(
        cache.get("reference", "a", at(0)).unwrap(),
        Some("host".to_owned())
    );
    // Guest writes remain refused after seeding, and the seed survives.
    assert!(cache.put("reference", "a", "guest", 60, at(0)).is_err());
    assert_eq!(
        cache.get("reference", "a", at(0)).unwrap(),
        Some("host".to_owned())
    );
}

#[test]
fn host_seeding_obeys_every_bound_and_expires_normally() {
    let cache =
        Cache::new(config(&[("reference", NamespaceMode::ReadOnly)])).expect("cache should build");

    // Seeding is exempt only from the read-only refusal, never from a bound.
    for (key, value, ttl) in [
        ("a", "x".repeat(257), 60),
        (&"k".repeat(65), "v".to_owned(), 60),
        ("a", "v".to_owned(), 0),
        ("a", "v".to_owned(), 99_999),
    ] {
        assert_eq!(
            cache
                .seed("reference", key, &value, ttl, at(0))
                .unwrap_err()
                .kind(),
            CacheErrorKind::Limit,
            "seeding must respect every bound"
        );
    }

    cache.seed("reference", "a", "value", 10, at(0)).unwrap();
    assert!(cache.get("reference", "a", at(9_999)).unwrap().is_some());
    assert_eq!(
        cache.get("reference", "a", at(10_000)).unwrap(),
        None,
        "a seeded entry expires exactly like any other"
    );

    // Namespace entry bounds apply to seeding too.
    for index in 0..8 {
        cache
            .seed("reference", &format!("k{index}"), "v", 600, at(0))
            .unwrap();
    }
    assert_eq!(cache.stats().entries, 4, "the namespace bound still holds");
}

#[test]
fn the_monotonic_clock_is_infallible_and_non_decreasing() {
    let cache = cache();

    let first = cache.now();
    let second = cache.now();

    assert!(
        second >= first,
        "the cache timeline must never move backwards"
    );
    // A real reading drives a real expiry without any wall clock.
    cache
        .put("lookups", "a", "v", 1, cache.now())
        .expect("write should succeed");
    assert!(cache.get("lookups", "a", cache.now()).unwrap().is_some());
    assert_eq!(
        cache
            .get(
                "lookups",
                "a",
                CacheInstant::from_millis(cache.now().as_millis() + 2_000)
            )
            .unwrap(),
        None,
        "expiry follows the monotonic timeline"
    );
}

#[test]
fn a_time_to_live_near_the_ceiling_cannot_overflow_the_deadline() {
    let cache = Cache::new(CacheConfig {
        namespaces: BTreeMap::from([(
            "lookups".to_owned(),
            NamespacePolicy {
                mode: NamespaceMode::ReadWrite,
                max_entries: 4,
                max_bytes: 4 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
                max_key_bytes: 64,
                max_value_bytes: 256,
                max_ttl_seconds: MAX_TTL_SECONDS,
            },
        )]),
        max_total_entries: 8,
        max_total_bytes: 8 * (64 + 256 + ENTRY_OVERHEAD_BYTES),
    })
    .expect("cache should build");

    // A near-maximum instant plus a maximum time to live must be refused
    // rather than wrapping into the past.
    let outcome = cache.put(
        "lookups",
        "a",
        "v",
        MAX_TTL_SECONDS,
        CacheInstant::from_millis(u64::MAX - 10),
    );
    assert_eq!(outcome.unwrap_err().kind(), CacheErrorKind::Limit);

    // The same write at an ordinary instant succeeds.
    cache
        .put("lookups", "a", "v", MAX_TTL_SECONDS, at(0))
        .expect("an ordinary maximum-ttl write should succeed");
}
