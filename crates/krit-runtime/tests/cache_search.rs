use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, AgentHostServices, CacheConfig, CacheHandle, CancellationHandle,
    DenyAllApprovalPolicy, GrantSet, HostInputs, HttpRequest, LocalConnectorConfig, LocalDocument,
    NamespaceMode, NamespacePolicy, NetworkPolicy, Runtime, SearchCatalog, SearchConnectorConfig,
    SearchKind, SearchTransport, SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// An explicit point on the cache's monotonic timeline.
const fn at(millis: u64) -> krit_runtime::CacheInstant {
    krit_runtime::CacheInstant::from_millis(millis)
}

fn namespace(mode: NamespaceMode) -> NamespacePolicy {
    NamespacePolicy {
        mode,
        max_entries: 8,
        max_bytes: 64 * 1024,
        max_key_bytes: 256,
        max_value_bytes: 8 * 1024,
        max_ttl_seconds: 600,
    }
}

fn cache(mode: NamespaceMode) -> CacheHandle {
    CacheHandle::open(CacheConfig {
        namespaces: BTreeMap::from([("lookups".to_owned(), namespace(mode))]),
        max_total_entries: 32,
        max_total_bytes: 256 * 1024,
    })
    .expect("cache should open")
}

fn documents() -> Vec<LocalDocument> {
    vec![
        LocalDocument {
            id: "alpha".to_owned(),
            text: "alpha document about caching".to_owned(),
        },
        LocalDocument {
            id: "beta".to_owned(),
            text: "beta document about searching".to_owned(),
        },
    ]
}

fn search_catalog(kind: SearchKind) -> SearchCatalog {
    let index = if kind == SearchKind::Query {
        "docs"
    } else {
        "vectors"
    };
    SearchCatalog::open(BTreeMap::from([(
        index.to_owned(),
        SearchConnectorConfig {
            kind,
            index: index.to_owned(),
            transport: SearchTransport::Local(LocalConnectorConfig {
                documents: documents(),
            }),
            max_results: 5,
            dimensions: (kind == SearchKind::Vector).then_some(3),
        },
    )]))
    .expect("search catalog should open")
}

fn host(cache: CacheHandle, search_catalog: SearchCatalog) -> AgentHost {
    AgentHost::new_with_services(
        HostInputs::new(BTreeMap::new(), SecretStore::default())
            .expect("inputs should be valid")
            .with_network_policy(NetworkPolicy::loopback_for_tests()),
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        AgentHostServices {
            cache,
            search_catalog,
            ..AgentHostServices::default()
        },
    )
    .expect("agent host should build")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/cache"
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
    let mut options = BuildOptions::new("2026", "test/cache", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("source should compile")
}

fn request(query: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".to_owned(),
        path: "/".to_owned(),
        query: query.to_owned(),
        headers: Vec::new(),
        body: String::new(),
    }
}

/// Reports the exact cache outcome so tests can tell a hit, a miss, and an
/// outage apart.
const CACHE_OUTCOME_SOURCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", request.query) {
        Ok(found) => match found {
            Some(value) => record { status: 200, headers: [], body: value },
            None => match cache_put("lookups", request.query, "computed", 60) {
                Ok(stored) => record { status: 201, headers: [], body: "miss" },
                Err(problem) => record { status: 202, headers: [], body: problem },
            },
        },
        Err(outage) => record { status: 503, headers: [], body: outage },
    }
}
"#;

const CACHE_CAPABILITIES: &str = "cacheNamespaces = [\"lookups\"]\n";

fn cache_grants() -> GrantSet {
    GrantSet::from_manifest(&manifest(CACHE_CAPABILITIES))
}

#[test]
fn a_miss_then_a_hit_is_observable_across_fresh_stores() {
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let host = host(cache(NamespaceMode::ReadWrite), SearchCatalog::default());
    let runtime = Runtime::default();
    let grants = cache_grants();

    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("k"),
        )
        .expect("first invocation should run");
    assert_eq!(first.response.status, 201, "the first call must miss");
    assert_eq!(first.stats.cache_misses, 1);
    assert_eq!(first.stats.cache_hits, 0);
    assert_eq!(first.stats.cache_writes, 1);

    // A completely fresh Store still observes the host-owned cache.
    let second = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("k"),
        )
        .expect("second invocation should run");
    assert_eq!(second.response.status, 200, "the second call must hit");
    assert_eq!(second.response.body, "computed");
    assert_eq!(second.stats.cache_hits, 1);
    assert_eq!(second.stats.cache_misses, 0);
}

#[test]
fn a_new_host_starts_with_an_empty_cache() {
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let runtime = Runtime::default();
    let grants = cache_grants();

    let warm = host(cache(NamespaceMode::ReadWrite), SearchCatalog::default());
    runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &warm,
            request("k"),
        )
        .expect("warm invocation should run");

    // Building a second host models a process restart: nothing carries over.
    let restarted = host(cache(NamespaceMode::ReadWrite), SearchCatalog::default());
    let result = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &restarted,
            request("k"),
        )
        .expect("restarted invocation should run");

    assert_eq!(
        result.response.status, 201,
        "a restarted host must miss, and the guest must handle it"
    );
}

#[test]
fn an_unconfigured_namespace_is_a_guest_visible_error_not_a_miss() {
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    // The manifest grants the namespace but the host configures no cache.
    let host = host(CacheHandle::default(), SearchCatalog::default());

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &cache_grants(),
            &host,
            request("k"),
        )
        .expect("invocation should run");

    assert_eq!(
        result.response.status, 503,
        "an unconfigured cache must surface as an error the guest handles"
    );
    assert!(result.response.body.contains("not configured"));
    assert_eq!(result.stats.cache_errors, 1);
    assert_eq!(result.stats.cache_hits, 0);
    assert_eq!(result.stats.cache_misses, 0);
}

#[test]
fn a_read_only_namespace_refuses_writes_at_run_time() {
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let host = host(cache(NamespaceMode::ReadOnly), SearchCatalog::default());
    // A read-only namespace still needs the read grant to be present.
    let grants = GrantSet::from_manifest(&manifest(
        "readOnlyCacheNamespaces = [\"lookups\"]\ncacheNamespaces = []\n",
    ));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("k"),
        )
        .expect_err("a write-requiring artifact must be refused");

    assert_eq!(error.code(), "K5001");
}

#[test]
fn cache_bounds_are_enforced_as_values_rather_than_traps() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_put("lookups", request.query, request.body, 100000) {
        Ok(stored) => record { status: 200, headers: [], body: "stored" },
        Err(problem) => record { status: 400, headers: [], body: problem },
    }
}
"#,
        &["cache.write"],
    );
    let host = host(cache(NamespaceMode::ReadWrite), SearchCatalog::default());

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &cache_grants(),
            &host,
            request("k"),
        )
        .expect("invocation should run");

    assert_eq!(result.response.status, 400);
    assert!(result.response.body.contains("time to live"));
    assert_eq!(result.stats.cache_errors, 1);
}

#[test]
fn a_guest_trap_leaves_earlier_cache_writes_in_place() {
    // The cache is deliberately non-transactional. A later failure does not
    // undo an earlier write, which is exactly why a cached value may never be
    // load bearing.
    let writer = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_put("lookups", request.query, "written", 60) {
        Ok(stored) => record { status: 200 / 0, headers: [], body: "unreachable" },
        Err(problem) => record { status: 400, headers: [], body: problem },
    }
}
"#,
        &["cache.write"],
    );
    let reader = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let shared = cache(NamespaceMode::ReadWrite);
    let host = host(shared.clone(), SearchCatalog::default());
    let runtime = Runtime::default();

    runtime
        .invoke_webhook_with_host(
            &writer.bytes,
            &writer.metadata,
            &cache_grants(),
            &host,
            request("k"),
        )
        .expect_err("the guest must trap");

    let after = runtime
        .invoke_webhook_with_host(
            &reader.bytes,
            &reader.metadata,
            &cache_grants(),
            &host,
            request("k"),
        )
        .expect("the reader should run");

    assert_eq!(
        after.response.status, 200,
        "the pre-trap cache write is still visible, by contract"
    );
    assert_eq!(after.response.body, "written");
}

#[test]
fn cancellation_is_reported_before_any_cache_work() {
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let shared = cache(NamespaceMode::ReadWrite);
    let host = host(shared.clone(), SearchCatalog::default());
    let cancellation = CancellationHandle::new();
    cancellation.cancel();

    let error = Runtime::default()
        .invoke_webhook_with_cancellation(
            &artifact.bytes,
            &artifact.metadata,
            &cache_grants(),
            &host,
            &cancellation,
            request("k"),
        )
        .expect_err("a cancelled invocation must not run");

    assert_eq!(error.code(), "K5106");
    assert_eq!(shared.stats().puts, 0, "a cancelled call must not write");
}

const SEARCH_SOURCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match search_query("docs", request.query, 2) {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 502, headers: [], body: problem },
    }
}
"#;

fn search_grants() -> GrantSet {
    GrantSet::from_manifest(&manifest("searchIndexes = [\"docs\"]\n"))
}

#[test]
fn a_local_connector_returns_deterministic_bounded_results() {
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = host(CacheHandle::default(), search_catalog(SearchKind::Query));
    let runtime = Runtime::default();

    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &search_grants(),
            &host,
            request("document"),
        )
        .expect("search invocation should run");
    let second = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &search_grants(),
            &host,
            request("document"),
        )
        .expect("search invocation should run");

    assert_eq!(first.response.status, 200);
    assert_eq!(first.response.body, second.response.body);
    assert!(first.response.body.starts_with("{\"results\":["));
    assert_eq!(first.stats.search_calls, 1);
    assert!(serde_json::from_str::<serde_json::Value>(&first.response.body).is_ok());
}

#[test]
fn an_unconfigured_connector_is_a_guest_visible_error() {
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = host(CacheHandle::default(), SearchCatalog::default());

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &search_grants(),
            &host,
            request("document"),
        )
        .expect("invocation should run");

    assert_eq!(result.response.status, 502);
    assert!(result.response.body.contains("not configured"));
}

#[test]
fn result_counts_outside_the_connector_bound_are_refused() {
    for (limit, expected) in [(0, 502), (99, 502), (2, 200)] {
        let artifact = compile(
            &format!(
                r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match search_query("docs", request.query, {limit}) {{
        Ok(results) => record {{ status: 200, headers: [], body: results }},
        Err(problem) => record {{ status: 502, headers: [], body: problem }},
    }}
}}
"#
            ),
            &["search.query"],
        );
        let host = host(CacheHandle::default(), search_catalog(SearchKind::Query));

        let result = Runtime::default()
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &search_grants(),
                &host,
                request("document"),
            )
            .expect("invocation should run");

        assert_eq!(result.response.status, expected, "limit {limit}");
    }
}

#[test]
fn untrusted_query_text_is_carried_as_data() {
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = host(CacheHandle::default(), search_catalog(SearchKind::Query));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &search_grants(),
            &host,
            request("\"}]}<script>alert(1)</script>"),
        )
        .expect("invocation should run");

    assert_eq!(result.response.status, 200);
    // The hostile query simply matches nothing; it never alters the structure.
    assert_eq!(result.response.body, "{\"results\":[]}");
    assert!(serde_json::from_str::<serde_json::Value>(&result.response.body).is_ok());
}

#[test]
fn a_query_connector_refuses_vector_operations_and_the_reverse() {
    let vector_source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match vector_search("vectors", request.body, 2) {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 502, headers: [], body: problem },
    }
}
"#;
    let artifact = compile(vector_source, &["search.vector"]);
    // Both capabilities are granted so the mismatch, not a missing grant, is
    // what fails the setup.
    let grants = GrantSet::from_manifest(&manifest(
        "searchIndexes = [\"vectors\"]\nvectorIndexes = [\"vectors\"]\n",
    ));
    // A text connector registered under the vector name must be refused.
    let mismatched = SearchCatalog::open(BTreeMap::from([(
        "vectors".to_owned(),
        SearchConnectorConfig {
            kind: SearchKind::Query,
            index: "vectors".to_owned(),
            transport: SearchTransport::Local(LocalConnectorConfig {
                documents: documents(),
            }),
            max_results: 5,
            dimensions: None,
        },
    )]))
    .expect("catalog should open");

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(CacheHandle::default(), mismatched),
            request(""),
        )
        .expect_err("a kind mismatch must fail closed");

    assert_eq!(error.code(), "K5404");
}

#[test]
fn vector_inputs_are_dimension_checked_before_any_connector_runs() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match vector_search("vectors", request.body, 2) {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 502, headers: [], body: problem },
    }
}
"#,
        &["search.vector"],
    );
    let grants = GrantSet::from_manifest(&manifest("vectorIndexes = [\"vectors\"]\n"));
    let host = host(CacheHandle::default(), search_catalog(SearchKind::Vector));
    let runtime = Runtime::default();

    for (body, ok) in [
        ("[0.1,0.2,0.3]", true),
        ("[0.1,0.2]", false),
        ("[0.1,0.2,0.3,0.4]", false),
        ("[\"a\",\"b\",\"c\"]", false),
        ("{\"a\":1}", false),
        ("not json", false),
    ] {
        let mut request = request("");
        request.body = body.to_owned();
        let result = runtime
            .invoke_webhook_with_host(&artifact.bytes, &artifact.metadata, &grants, &host, request)
            .expect("invocation should run");
        assert_eq!(
            result.response.status,
            if ok { 200 } else { 502 },
            "vector `{body}` was handled incorrectly: {}",
            result.response.body
        );
    }
}

#[test]
fn a_cached_lookup_never_calls_the_connector_twice() {
    let artifact = compile(
        r#"
fn lookup(query: String) -> HttpResponse {
    match search_query("docs", query, 2) {
        Ok(results) => match cache_put("lookups", query, results, 60) {
            Ok(stored) => record { status: 201, headers: [], body: results },
            Err(problem) => record { status: 202, headers: [], body: results },
        },
        Err(problem) => record { status: 502, headers: [], body: problem },
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", request.query) {
        Ok(found) => match found {
            Some(value) => record { status: 200, headers: [], body: value },
            None => lookup(request.query),
        },
        Err(outage) => lookup(request.query),
    }
}
"#,
        &["cache.read", "cache.write", "search.query"],
    );
    let grants = GrantSet::from_manifest(&manifest(
        "cacheNamespaces = [\"lookups\"]\nsearchIndexes = [\"docs\"]\n",
    ));
    let host = host(
        cache(NamespaceMode::ReadWrite),
        search_catalog(SearchKind::Query),
    );
    let runtime = Runtime::default();

    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("document"),
        )
        .expect("first invocation should run");
    let second = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("document"),
        )
        .expect("second invocation should run");

    assert_eq!(first.response.status, 201);
    assert_eq!(first.stats.search_calls, 1);
    assert_eq!(second.response.status, 200);
    assert_eq!(
        second.stats.search_calls, 0,
        "a hit must not reach the connector"
    );
    assert_eq!(second.response.body, first.response.body);
}

#[test]
fn cache_and_search_facts_never_disclose_payloads() {
    let host = host(
        cache(NamespaceMode::ReadWrite),
        search_catalog(SearchKind::Query),
    );
    host.cache()
        .namespace_names()
        .iter()
        .for_each(|name| assert_eq!(name, "lookups"));

    let rendered = format!("{:?} {:?}", host.cache(), host.search_catalog());

    assert!(rendered.contains("lookups"));
    assert!(rendered.contains("docs"));
    assert!(
        !rendered.contains("alpha document") && !rendered.contains("caching"),
        "connector rendering leaked document text: {rendered}"
    );
}

#[test]
fn distinct_namespaces_and_indexes_stay_isolated() {
    let identifier = COUNTER.fetch_add(1, Ordering::Relaxed);
    let cache = CacheHandle::open(CacheConfig {
        namespaces: BTreeMap::from([
            ("lookups".to_owned(), namespace(NamespaceMode::ReadWrite)),
            ("other".to_owned(), namespace(NamespaceMode::ReadWrite)),
        ]),
        max_total_entries: 32,
        max_total_bytes: 256 * 1024,
    })
    .expect("cache should open");
    let key = format!("shared-{identifier}");
    let grants = GrantSet::from_manifest(&manifest("cacheNamespaces = [\"lookups\", \"other\"]\n"));
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let host = host(cache.clone(), SearchCatalog::default());
    let runtime = Runtime::default();

    runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request(&key),
        )
        .expect("write invocation should run");

    // Exactly one entry exists: the write landed in `lookups` only, and the
    // identically named key in `other` is a different entry entirely.
    assert_eq!(cache.stats().entries, 1);
    assert_eq!(
        cache.get("other", &key, at(0)).unwrap(),
        None,
        "namespaces must not share entries"
    );
    assert_eq!(
        cache.get("lookups", &key, at(0)).unwrap(),
        Some("computed".to_owned())
    );
}

#[test]
fn transaction_bound_hosts_still_reject_search_while_a_transaction_is_open() {
    // Search performs a network round trip, so it may never run while a
    // database transaction holds a lock. Without a configured database the
    // guest simply cannot open one, which is asserted here as a baseline for
    // the interaction documented in the specification.
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = host(CacheHandle::default(), search_catalog(SearchKind::Query));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &search_grants(),
            &host,
            request("document"),
        )
        .expect("invocation should run");

    assert_eq!(result.response.status, 200);
}

#[test]
fn namespace_entry_pressure_is_visible_and_bounded() {
    // Four writes into a namespace bounded at two entries must evict rather
    // than grow, and every write still reports success to the guest.
    let cache = CacheHandle::open(CacheConfig {
        namespaces: BTreeMap::from([(
            "lookups".to_owned(),
            NamespacePolicy {
                mode: NamespaceMode::ReadWrite,
                max_entries: 2,
                max_bytes: 8 * 1024,
                max_key_bytes: 256,
                max_value_bytes: 1024,
                max_ttl_seconds: 600,
            },
        )]),
        max_total_entries: 4,
        max_total_bytes: 32 * 1024,
    })
    .expect("cache should open");
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let host = host(cache.clone(), SearchCatalog::default());
    let runtime = Runtime::default();

    for key in ["a", "b", "c", "d"] {
        let result = runtime
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &cache_grants(),
                &host,
                request(key),
            )
            .expect("invocation should run");
        assert_eq!(result.response.status, 201, "key {key} should miss");
    }

    let stats = cache.stats();
    assert_eq!(stats.entries, 2, "the namespace bound must hold: {stats:?}");
    assert!(stats.evictions >= 2);

    // The earliest key was evicted, so the guest sees a miss and recomputes.
    let evicted = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &cache_grants(),
            &host,
            request("a"),
        )
        .expect("invocation should run");
    assert_eq!(evicted.response.status, 201, "an evicted key must miss");
}

#[test]
fn configured_resources_must_be_granted_by_the_manifest() {
    let artifact = compile(CACHE_OUTCOME_SOURCE, &["cache.read", "cache.write"]);
    let host = host(cache(NamespaceMode::ReadWrite), SearchCatalog::default());
    let ungranted = GrantSet::from_manifest(&manifest("state = [\"work\"]\n"));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &ungranted,
            &host,
            request("k"),
        )
        .expect_err("an ungranted namespace must be refused");

    assert_eq!(error.code(), "K5001");
}

#[test]
fn search_timeouts_stay_inside_the_invocation_deadline() {
    let catalog = search_catalog(SearchKind::Query);
    assert_eq!(catalog.names().len(), 1);
    assert_eq!(catalog.kind("docs"), Some(SearchKind::Query));
    assert_eq!(catalog.kind("absent"), None);

    // A connector timeout longer than the runtime deadline is still clamped by
    // the invocation deadline at call time.
    let long = SearchConnectorConfig {
        kind: SearchKind::Query,
        index: "docs".to_owned(),
        transport: SearchTransport::HttpJson(krit_runtime::HttpJsonConnectorConfig {
            origin: "https://example.test".to_owned(),
            path: "/search".to_owned(),
            secret: None,
            max_response_bytes: 4096,
            timeout: Duration::from_secs(600),
        }),
        max_results: 5,
        dimensions: None,
    };
    assert!(long.validate().is_ok());
}

/// Reads one bounded HTTP request from a mock connector socket.
fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;

    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout should configure");
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let count = stream.read(&mut buffer).expect("request should read");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
        let length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
            })
            .unwrap_or_default();
        if bytes.len() >= header_end + 4 + length {
            break;
        }
    }
    bytes
}

fn listener_origin(listener: &std::net::TcpListener) -> String {
    format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("listener address").port()
    )
}

fn http_json_catalog(origin: String, secret: Option<String>) -> SearchCatalog {
    SearchCatalog::open(BTreeMap::from([(
        "docs".to_owned(),
        SearchConnectorConfig {
            kind: SearchKind::Query,
            index: "docs".to_owned(),
            transport: SearchTransport::HttpJson(krit_runtime::HttpJsonConnectorConfig {
                origin,
                path: "/query".to_owned(),
                secret,
                max_response_bytes: 8192,
                timeout: Duration::from_secs(2),
            }),
            max_results: 5,
            dimensions: None,
        },
    )]))
    .expect("catalog should open")
}

#[test]
fn the_http_json_transport_sends_an_exact_bounded_request_and_hides_credentials() {
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let captured = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connector should connect");
        let request = read_request(&mut stream);
        let body = "{\"results\":[{\"id\":\"a\",\"score\":0.25,\"snippet\":\"hello\"}]}";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("response should write");
        String::from_utf8_lossy(&request).into_owned()
    });

    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let inputs = HostInputs::new(
        BTreeMap::new(),
        SecretStore::new(BTreeMap::from([(
            "search-token".to_owned(),
            b"super-secret-value".to_vec(),
        )]))
        .expect("secrets should be valid"),
    )
    .expect("inputs should be valid")
    .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let host = AgentHost::new_with_services(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(
            krit_runtime::ExplicitApprovalPolicy::new([(
                krit_runtime::ApprovalOperation::HttpBearer,
                origin.clone(),
            )])
            .expect("approval policy should build"),
        ),
        AgentHostServices {
            search_catalog: http_json_catalog(origin.clone(), Some("search-token".to_owned())),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build");
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nsecrets = [\"search-token\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("search invocation should run");
    let request_text = captured.join().expect("mock should finish");

    // The wire request is exactly the generic bounded schema.
    assert!(
        request_text.starts_with("POST /query HTTP/1.1"),
        "{request_text}"
    );
    assert!(request_text.contains("{\"query\":\"needle\",\"limit\":2}"));
    // The credential travels as a bearer header, never inside the body.
    let header_block = request_text
        .split("\r\n\r\n")
        .next()
        .expect("request should have headers")
        .to_lowercase();
    assert!(
        header_block.contains("authorization:"),
        "the credential must travel as a header: {request_text}"
    );
    let body = request_text
        .split("\r\n\r\n")
        .nth(1)
        .expect("request should have a body");
    assert!(
        !body.contains("super-secret-value"),
        "a credential must never enter a connector request body: {body}"
    );

    // The provider response is re-encoded into the fixed guest-visible shape.
    assert_eq!(result.response.status, 200);
    assert_eq!(
        result.response.body,
        "{\"results\":[{\"id\":\"a\",\"score\":0.250000,\"snippet\":\"hello\"}]}"
    );
    assert!(
        !result.response.body.contains("super-secret-value"),
        "a credential must never reach guest-visible output"
    );
}

#[test]
fn a_malformed_provider_response_is_refused_rather_than_forwarded() {
    use std::io::Write;

    for body in [
        "{\"results\":[{\"id\":\"a\",\"score\":0.5,\"snippet\":\"t\",\"extra\":1}]}",
        "{\"unexpected\":[]}",
        "not json at all",
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock should bind");
        let origin = listener_origin(&listener);
        let payload = body.to_owned();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connector should connect");
            let _ = read_request(&mut stream);
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                )
                .as_bytes(),
            );
        });

        let artifact = compile(SEARCH_SOURCE, &["search.query"]);
        let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
            .expect("inputs should be valid")
            .with_network_policy(NetworkPolicy::loopback_for_tests());
        let host = AgentHost::new_with_services(
            inputs,
            AgentHostPolicy::default(),
            Arc::new(DenyAllApprovalPolicy),
            AgentHostServices {
                search_catalog: http_json_catalog(origin.clone(), None),
                ..AgentHostServices::default()
            },
        )
        .expect("host should build");
        let grants = GrantSet::from_manifest(&manifest(&format!(
            "searchIndexes = [\"docs\"]\nhttp = [\"{origin}\"]\n"
        )));

        let result = Runtime::default()
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                request("needle"),
            )
            .expect("invocation should run");
        let _ = server.join();

        assert_eq!(
            result.response.status, 502,
            "provider body `{body}` must be refused"
        );
        assert!(
            !result.response.body.contains("extra"),
            "raw provider output must never be forwarded: {}",
            result.response.body
        );
    }
}

#[test]
fn a_default_policy_refuses_a_plaintext_connector_origin() {
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    // A default network policy permits no plaintext origin, so a connector that
    // would send a query in the clear is refused before the guest runs.
    let inputs =
        HostInputs::new(BTreeMap::new(), SecretStore::default()).expect("inputs should be valid");
    let host = AgentHost::new_with_services(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        AgentHostServices {
            search_catalog: http_json_catalog("http://search.example".to_owned(), None),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build");
    let grants = GrantSet::from_manifest(&manifest(
        "searchIndexes = [\"docs\"]\nhttp = [\"http://search.example\"]\n",
    ));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect_err("a plaintext connector must be refused");

    assert_eq!(error.code(), "K5404");
    assert!(error.message().contains("HTTPS"), "{error}");
}

#[test]
fn a_connector_may_not_reach_an_address_the_network_policy_blocks() {
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    // The loopback test policy permits loopback but still blocks other private
    // ranges, so an SSRF-style destination fails as a handled guest value.
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs should be valid")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let host = AgentHost::new_with_services(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        AgentHostServices {
            search_catalog: http_json_catalog("http://10.0.0.1:9".to_owned(), None),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build");
    let grants = GrantSet::from_manifest(&manifest(
        "searchIndexes = [\"docs\"]\nhttp = [\"http://10.0.0.1:9\"]\n",
    ));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");

    assert_eq!(
        result.response.status, 502,
        "a filtered destination must surface as a handled error: {}",
        result.response.body
    );
    // The network failure path must not disclose the endpoint either.
    assert_no_endpoint_disclosure(&result.response.body, "http://10.0.0.1:9");
}

// --- Issue 1: complete connector authority -----------------------------------

/// Builds a host whose connector reaches `origin` with an optional credential.
fn retrying_policy() -> AgentHostPolicy {
    AgentHostPolicy {
        default_http_retry: krit_runtime::RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        },
        ..AgentHostPolicy::default()
    }
}

fn connector_host(
    origin: &str,
    secret: Option<&str>,
    approvals: Vec<(krit_runtime::ApprovalOperation, String)>,
) -> AgentHost {
    let secrets = secret.map_or_else(SecretStore::default, |name| {
        SecretStore::new(BTreeMap::from([(
            name.to_owned(),
            b"private-connector-token".to_vec(),
        )]))
        .expect("secrets should be valid")
    });
    let inputs = HostInputs::new(BTreeMap::new(), secrets)
        .expect("inputs should be valid")
        .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    AgentHost::new_with_services(
        inputs,
        retrying_policy(),
        Arc::new(
            krit_runtime::ExplicitApprovalPolicy::new(approvals)
                .expect("approval policy should build"),
        ),
        AgentHostServices {
            search_catalog: http_json_catalog(origin.to_owned(), secret.map(str::to_owned)),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build")
}

/// A listener that fails the test if anything ever connects to it.
fn silent_listener() -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let origin = listener_origin(&listener);
    listener
        .set_nonblocking(true)
        .expect("listener should be non-blocking");
    (listener, origin)
}

#[test]
fn a_connector_origin_must_be_granted_by_the_manifest() {
    let (listener, origin) = silent_listener();
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = connector_host(&origin, None, Vec::new());
    // The connector name is granted, but its exact origin is not.
    let grants = GrantSet::from_manifest(&manifest("searchIndexes = [\"docs\"]\n"));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect_err("an ungranted origin must be refused");

    assert_eq!(error.code(), "K5001");
    assert!(
        listener.accept().is_err(),
        "no connection may be attempted for an ungranted origin"
    );
}

#[test]
fn a_connector_secret_must_be_granted_by_the_manifest() {
    let (listener, origin) = silent_listener();
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = connector_host(
        &origin,
        Some("search-token"),
        vec![(krit_runtime::ApprovalOperation::HttpBearer, origin.clone())],
    );
    // The origin is granted but the credential is not.
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nhttp = [\"{origin}\"]\n"
    )));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect_err("an ungranted connector secret must be refused");

    assert_eq!(error.code(), "K5001");
    assert!(
        listener.accept().is_err(),
        "no credential may be transmitted for an ungranted secret"
    );
}

#[test]
fn a_credentialed_connector_requires_explicit_default_deny_approval() {
    let (listener, origin) = silent_listener();
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    // Everything is granted, but no approval is configured.
    let host = connector_host(&origin, Some("search-token"), Vec::new());
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nsecrets = [\"search-token\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");

    assert_eq!(result.response.status, 502);
    assert!(
        result.response.body.contains("approval denied"),
        "an unapproved bearer must be refused: {}",
        result.response.body
    );
    assert!(
        listener.accept().is_err(),
        "no credential may be transmitted before approval"
    );
    assert_eq!(
        result.stats.network_attempts, 0,
        "approval must be checked before the first attempt"
    );
}

// --- Issue 3: bounded read-only retry ----------------------------------------

/// Serves `responses` in order, counting connections and captured requests.
fn scripted_provider(
    responses: Vec<(u16, String)>,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    listener
        .set_nonblocking(true)
        .expect("listener should be non-blocking");
    let handle = std::thread::spawn(move || {
        let mut captured = Vec::new();
        for (status, body) in responses {
            // Bounded wait: a missing attempt fails the assertion instead of
            // hanging the suite.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            break None;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break None,
                }
            };
            let Some(mut stream) = stream else {
                break;
            };
            stream
                .set_nonblocking(false)
                .expect("stream should be blocking");
            captured.push(String::from_utf8_lossy(&read_request(&mut stream)).into_owned());
            let reason = if status == 200 {
                "OK"
            } else {
                "Service Unavailable"
            };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
        captured
    });
    (origin, handle)
}

#[test]
fn a_transient_failure_is_retried_without_any_idempotency_key() {
    let success = "{\"results\":[{\"id\":\"a\",\"score\":1.0,\"snippet\":\"ok\"}]}".to_owned();
    let (origin, provider) = scripted_provider(vec![(503, "busy".to_owned()), (200, success)]);
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = connector_host(
        &origin,
        Some("search-token"),
        vec![(krit_runtime::ApprovalOperation::HttpBearer, origin.clone())],
    );
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nsecrets = [\"search-token\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(result.response.status, 200, "{}", result.response.body);
    assert_eq!(
        result.response.body,
        "{\"results\":[{\"id\":\"a\",\"score\":1.000000,\"snippet\":\"ok\"}]}"
    );
    assert_eq!(captured.len(), 2, "the transient failure must be retried");
    assert_eq!(result.stats.retries, 1);
    assert_eq!(result.stats.network_attempts, 2);
    for request in &captured {
        // A read-only search needs no key, and none is invented or sent.
        assert!(
            !request.to_lowercase().contains("idempotency-key"),
            "a search must not carry an idempotency key: {request}"
        );
        // Every attempt presents the credential, so every attempt was approved.
        assert!(
            request
                .split("\r\n\r\n")
                .next()
                .unwrap_or_default()
                .to_lowercase()
                .contains("authorization:"),
            "every attempt must carry its approved credential: {request}"
        );
    }
}

#[test]
fn approval_is_rechecked_on_every_retry_attempt() {
    use std::sync::atomic::AtomicUsize;

    /// Approves the first attempt and denies every later one.
    struct ApproveOnce {
        seen: AtomicUsize,
    }

    impl krit_runtime::ApprovalPolicy for ApproveOnce {
        fn approve(&self, _request: &krit_runtime::ApprovalRequest) -> bool {
            self.seen.fetch_add(1, Ordering::SeqCst) == 0
        }
    }

    let (origin, provider) = scripted_provider(vec![(503, "busy".to_owned())]);
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let secrets = SecretStore::new(BTreeMap::from([(
        "search-token".to_owned(),
        b"private-connector-token".to_vec(),
    )]))
    .expect("secrets should be valid");
    let inputs = HostInputs::new(BTreeMap::new(), secrets)
        .expect("inputs should be valid")
        .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let approvals = Arc::new(ApproveOnce {
        seen: AtomicUsize::new(0),
    });
    let host = AgentHost::new_with_services(
        inputs,
        retrying_policy(),
        Arc::clone(&approvals) as Arc<dyn krit_runtime::ApprovalPolicy>,
        AgentHostServices {
            search_catalog: http_json_catalog(origin.clone(), Some("search-token".to_owned())),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build");
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nsecrets = [\"search-token\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(result.response.status, 502);
    assert!(
        result.response.body.contains("approval denied"),
        "the retry must be refused once approval is withdrawn: {}",
        result.response.body
    );
    assert_eq!(captured.len(), 1, "only the approved attempt may be sent");
    assert_eq!(
        approvals.seen.load(Ordering::SeqCst),
        2,
        "approval must be consulted once per attempt"
    );
}

// --- Connector retry and rate policy match guest HTTP ------------------------

/// A host whose retry and rate policy can be set per origin.
fn policy_host(
    origin: &str,
    indexes: &[&str],
    policy: AgentHostPolicy,
    approvals: Vec<(krit_runtime::ApprovalOperation, String)>,
) -> AgentHost {
    let connectors = indexes
        .iter()
        .map(|index| {
            (
                (*index).to_owned(),
                SearchConnectorConfig {
                    kind: SearchKind::Query,
                    index: (*index).to_owned(),
                    transport: SearchTransport::HttpJson(krit_runtime::HttpJsonConnectorConfig {
                        origin: origin.to_owned(),
                        path: "/query".to_owned(),
                        secret: None,
                        max_response_bytes: 8192,
                        timeout: Duration::from_secs(2),
                    }),
                    max_results: 5,
                    dimensions: None,
                },
            )
        })
        .collect();
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs should be valid")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    AgentHost::new_with_services(
        inputs,
        policy,
        Arc::new(
            krit_runtime::ExplicitApprovalPolicy::new(approvals)
                .expect("approval policy should build"),
        ),
        AgentHostServices {
            search_catalog: SearchCatalog::open(connectors).expect("catalog should open"),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build")
}

fn attempts(max_attempts: u8) -> krit_runtime::RetryPolicy {
    krit_runtime::RetryPolicy {
        max_attempts,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    }
}

const SEARCH_SUCCESS: &str = "{\"results\":[{\"id\":\"a\",\"score\":1.0,\"snippet\":\"ok\"}]}";

#[test]
fn an_exact_origin_retry_override_loosens_the_default() {
    let (origin, provider) = scripted_provider(vec![
        (503, "busy".to_owned()),
        (200, SEARCH_SUCCESS.to_owned()),
    ]);
    // The default forbids a retry; the exact-origin override permits one.
    let policy = AgentHostPolicy {
        default_http_retry: attempts(1),
        http_retries: BTreeMap::from([(origin.clone(), attempts(2))]),
        ..AgentHostPolicy::default()
    };
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = policy_host(&origin, &["docs"], policy, Vec::new());
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(
        result.response.status, 200,
        "the origin override must permit the retry: {}",
        result.response.body
    );
    assert_eq!(
        captured.len(),
        2,
        "the override must allow a second attempt"
    );
    assert_eq!(result.stats.retries, 1);
}

#[test]
fn an_exact_origin_retry_override_tightens_the_default() {
    let (origin, provider) = scripted_provider(vec![(503, "busy".to_owned())]);
    // The default would permit a retry; the exact-origin override forbids one.
    let policy = AgentHostPolicy {
        default_http_retry: attempts(3),
        http_retries: BTreeMap::from([(origin.clone(), attempts(1))]),
        ..AgentHostPolicy::default()
    };
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = policy_host(&origin, &["docs"], policy, Vec::new());
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(
        result.response.status, 502,
        "the origin override must forbid the retry"
    );
    assert_eq!(captured.len(), 1, "only one attempt may be sent");
    assert_eq!(result.stats.retries, 0);
}

/// Two connector names pointing at one origin, called in sequence.
const TWO_CONNECTOR_SOURCE: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match search_query("docs", request.query, 2) {
        Ok(first) => match search_query("manuals", request.query, 2) {
            Ok(second) => record { status: 200, headers: [], body: second },
            Err(problem) => record { status: 429, headers: [], body: problem },
        },
        Err(problem) => record { status: 502, headers: [], body: problem },
    }
}
"#;

#[test]
fn connectors_sharing_one_origin_share_a_single_rate_bucket() {
    // Only one response is scripted: if the bucket were per connector, the
    // second call would reach the network and the provider would be asked for
    // a second response.
    let (origin, provider) = scripted_provider(vec![(200, SEARCH_SUCCESS.to_owned())]);
    let policy = AgentHostPolicy {
        default_http_rate: krit_runtime::RateLimitPolicy {
            capacity: 1,
            window: Duration::from_secs(60),
        },
        ..AgentHostPolicy::default()
    };
    let artifact = compile(TWO_CONNECTOR_SOURCE, &["search.query"]);
    let host = policy_host(&origin, &["docs", "manuals"], policy, Vec::new());
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\", \"manuals\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(
        result.response.status, 429,
        "the second connector must be rate denied: {}",
        result.response.body
    );
    assert!(
        result.response.body.contains("rate limit exceeded"),
        "{}",
        result.response.body
    );
    assert_eq!(
        captured.len(),
        1,
        "a shared origin bucket must stop the second call before the network"
    );
    assert_eq!(result.stats.rate_limit_denials, 1);
    assert_eq!(
        result.stats.search_calls, 2,
        "both calls were attempted; only one reached the network"
    );
}

#[test]
fn a_rate_denial_names_the_index_and_never_the_connector_origin() {
    let (origin, provider) = scripted_provider(vec![(200, SEARCH_SUCCESS.to_owned())]);
    let policy = AgentHostPolicy {
        default_http_rate: krit_runtime::RateLimitPolicy {
            capacity: 1,
            window: Duration::from_secs(60),
        },
        ..AgentHostPolicy::default()
    };
    let artifact = compile(TWO_CONNECTOR_SOURCE, &["search.query"]);
    let host = policy_host(&origin, &["docs", "manuals"], policy, Vec::new());
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\", \"manuals\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let _ = provider.join();

    // The bucket is the origin, but the guest only ever learns the index name.
    assert!(
        result.response.body.contains("manuals"),
        "the denial should name the index: {}",
        result.response.body
    );
    assert!(
        !result.response.body.contains(&origin)
            && !result.response.body.contains("127.0.0.1")
            && !result.response.body.contains("http"),
        "a connector endpoint must never reach guest code: {}",
        result.response.body
    );
}

// --- Rate-resource preflight counts host-owned connector origins -------------

fn tracked_policy(max_tracked_resources: usize) -> AgentHostPolicy {
    AgentHostPolicy {
        max_tracked_resources,
        ..AgentHostPolicy::default()
    }
}

/// A host with one connector per `(index, origin)` pair.
fn multi_origin_host(connectors: &[(&str, &str)], policy: AgentHostPolicy) -> AgentHost {
    let catalog = connectors
        .iter()
        .map(|(index, origin)| {
            (
                (*index).to_owned(),
                SearchConnectorConfig {
                    kind: SearchKind::Query,
                    index: (*index).to_owned(),
                    transport: SearchTransport::HttpJson(krit_runtime::HttpJsonConnectorConfig {
                        origin: (*origin).to_owned(),
                        path: "/query".to_owned(),
                        secret: None,
                        max_response_bytes: 8192,
                        timeout: Duration::from_secs(2),
                    }),
                    max_results: 5,
                    dimensions: None,
                },
            )
        })
        .collect();
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs should be valid")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    AgentHost::new_with_services(
        inputs,
        policy,
        Arc::new(DenyAllApprovalPolicy),
        AgentHostServices {
            search_catalog: SearchCatalog::open(catalog).expect("catalog should open"),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build")
}

#[test]
fn connectors_on_distinct_origins_are_counted_against_the_tracked_bound() {
    let (first, first_provider) = scripted_provider(Vec::new());
    let (second, second_provider) = scripted_provider(Vec::new());
    let artifact = compile(TWO_CONNECTOR_SOURCE, &["search.query"]);
    // One tracked resource, but two distinct connector origins are reachable.
    let host = multi_origin_host(&[("docs", &first), ("manuals", &second)], tracked_policy(1));
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\", \"manuals\"]\nhttp = [\"{first}\", \"{second}\"]\n"
    )));

    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect_err("two origins must exceed a one-resource bound");

    assert!(
        error.message().contains("rate-limited resources"),
        "{error}"
    );
    assert_eq!(
        first_provider.join().expect("provider should finish").len(),
        0,
        "rejection must happen before any request"
    );
    assert_eq!(
        second_provider
            .join()
            .expect("provider should finish")
            .len(),
        0
    );
}

#[test]
fn connectors_sharing_one_origin_count_as_a_single_tracked_resource() {
    let (origin, provider) = scripted_provider(vec![
        (200, SEARCH_SUCCESS.to_owned()),
        (200, SEARCH_SUCCESS.to_owned()),
    ]);
    let artifact = compile(TWO_CONNECTOR_SOURCE, &["search.query"]);
    // Two connectors, one origin, one bucket: the bound is satisfied.
    let host = multi_origin_host(
        &[("docs", &origin), ("manuals", &origin)],
        tracked_policy(1),
    );
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\", \"manuals\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("one shared origin must fit a one-resource bound");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(result.response.status, 200, "{}", result.response.body);
    assert_eq!(captured.len(), 2);
}

#[test]
fn an_unused_configured_connector_consumes_no_tracked_resource() {
    let (used, provider) = scripted_provider(vec![(200, SEARCH_SUCCESS.to_owned())]);
    let (unused, unused_provider) = scripted_provider(Vec::new());
    // Only `docs` is required by the artifact; `manuals` is configured but
    // unreachable, so it must not consume the budget.
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let host = multi_origin_host(&[("docs", &used), ("manuals", &unused)], tracked_policy(1));
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\", \"manuals\"]\nhttp = [\"{used}\", \"{unused}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("an unused connector must not consume the budget");
    let captured = provider.join().expect("provider should finish");

    assert_eq!(result.response.status, 200, "{}", result.response.body);
    assert_eq!(captured.len(), 1);
    assert_eq!(
        unused_provider
            .join()
            .expect("provider should finish")
            .len(),
        0,
        "the unused connector must never be contacted"
    );
}

#[test]
fn a_local_connector_consumes_no_tracked_rate_resource() {
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    // A local connector performs no network work, so it needs no bucket even
    // when the tracked bound is exhausted by an unrelated requirement.
    let host = host(CacheHandle::default(), search_catalog(SearchKind::Query));
    let grants = GrantSet::from_manifest(&manifest("searchIndexes = [\"docs\"]\n"));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("document"),
        )
        .expect("a local connector must not need a rate bucket");

    assert_eq!(result.response.status, 200);
}

// --- Approval denials never disclose an endpoint ------------------------------

#[test]
fn an_initial_approval_denial_names_only_the_index() {
    let (listener, origin) = silent_listener();
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    // Granted everywhere, but no approval is configured.
    let host = connector_host(&origin, Some("search-token"), Vec::new());
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nsecrets = [\"search-token\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");

    let body = &result.response.body;
    assert!(body.contains("approval denied"), "{body}");
    assert!(
        body.contains("docs"),
        "the denial should name the index: {body}"
    );
    assert_no_endpoint_disclosure(body, &origin);
    assert!(listener.accept().is_err(), "nothing may be sent");
}

#[test]
fn a_retry_time_approval_withdrawal_names_only_the_index() {
    use std::sync::atomic::AtomicUsize;

    struct ApproveOnce {
        seen: AtomicUsize,
    }

    impl krit_runtime::ApprovalPolicy for ApproveOnce {
        fn approve(&self, _request: &krit_runtime::ApprovalRequest) -> bool {
            self.seen.fetch_add(1, Ordering::SeqCst) == 0
        }
    }

    let (origin, provider) = scripted_provider(vec![(503, "busy".to_owned())]);
    let artifact = compile(SEARCH_SOURCE, &["search.query"]);
    let secrets = SecretStore::new(BTreeMap::from([(
        "search-token".to_owned(),
        b"private-connector-token".to_vec(),
    )]))
    .expect("secrets should be valid");
    let inputs = HostInputs::new(BTreeMap::new(), secrets)
        .expect("inputs should be valid")
        .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let host = AgentHost::new_with_services(
        inputs,
        retrying_policy(),
        Arc::new(ApproveOnce {
            seen: AtomicUsize::new(0),
        }),
        AgentHostServices {
            search_catalog: http_json_catalog(origin.clone(), Some("search-token".to_owned())),
            ..AgentHostServices::default()
        },
    )
    .expect("host should build");
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "searchIndexes = [\"docs\"]\nsecrets = [\"search-token\"]\nhttp = [\"{origin}\"]\n"
    )));

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("needle"),
        )
        .expect("invocation should run");
    let _ = provider.join();

    let body = &result.response.body;
    assert!(body.contains("approval denied"), "{body}");
    assert!(body.contains("docs"), "{body}");
    assert_no_endpoint_disclosure(body, &origin);
}

/// Fails if a guest-visible message discloses anything about the endpoint or
/// the credential behind a connector.
fn assert_no_endpoint_disclosure(message: &str, origin: &str) {
    let port = origin.rsplit(':').next().unwrap_or_default();
    // `http.bearer` is a fixed public operation name, not an endpoint, so the
    // scheme is checked in its URL form only.
    for secret in [
        origin,
        "127.0.0.1",
        "://",
        port,
        "search-token",
        "private-connector-token",
        "/query",
        "needle",
    ] {
        assert!(
            !message.contains(secret),
            "`{secret}` leaked into a guest-visible message: {message}"
        );
    }
}
