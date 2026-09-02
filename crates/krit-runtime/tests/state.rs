use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, AiAdapterConfig, ApprovalOperation, CancellationHandle,
    DenyAllApprovalPolicy, Durability, DurableState, DurableStoreDefinition, DurableStoreLimits,
    ExplicitApprovalPolicy, GrantSet, HostInputs, HttpHeader, HttpJsonAdapterConfig, HttpRequest,
    NetworkPolicy, RetentionPolicy, Runtime, SecretStore,
};
use krit_state::DurableStore;
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "krit-runtime-state-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn database(&self) -> PathBuf {
        self.path.join("state.db")
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
        max_operations: 1024,
        max_key_bytes: 256,
        max_value_bytes: 64 * 1024,
        max_transaction_bytes: 1024 * 1024,
        max_database_bytes: 64 * 1024 * 1024,
        max_replay_entries: 1024,
        max_replay_bytes: 16 * 1024 * 1024,
    }
}

fn retention() -> RetentionPolicy {
    RetentionPolicy {
        max_entries: 1024,
        max_bytes: 16 * 1024 * 1024,
        ttl: Duration::from_secs(7 * 24 * 60 * 60),
        lease: Duration::from_secs(30),
    }
}

fn durable(path: PathBuf, idempotency: bool) -> DurableState {
    DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path,
                durability: Durability::Full,
                limits: store_limits(),
                replay: retention(),
            },
        )]),
        idempotency.then(|| "agent-work".to_owned()),
    )
    .expect("durable state should open")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/state"
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

fn compile(source_text: &str, capabilities: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/state", "1.0.0", "src/main.krit");
    for capability in capabilities {
        options.grant_effect(*capability);
    }
    build_component(&module, &options).expect("source should compile")
}

fn request(body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: "/incoming".to_owned(),
        query: String::new(),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

fn host(inputs: HostInputs, state: DurableState) -> AgentHost {
    AgentHost::new_with_state(
        inputs,
        AgentHostPolicy::default(),
        std::sync::Arc::new(DenyAllApprovalPolicy),
        state,
    )
    .expect("agent host should build")
}

#[test]
fn state_commits_on_success_rolls_back_on_trap_and_survives_restart() {
    let directory = TestDirectory::new("transaction");
    let database = directory.database();
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match state_get("agent-work", "last") {
        Ok(previous) => match state_put("agent-work", "last", request.body) {
            Ok(done) => match previous {
                Some(value) => record { status: 200, headers: [], body: value },
                None => record { status: 200, headers: [], body: "none" },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let grants = GrantSet::from_manifest(&manifest("state = [\"agent-work\"]"));
    let runtime = Runtime::default();
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default()).unwrap();
    let first_host = host(inputs.clone(), durable(database.clone(), false));
    let state_value = "state-sensitive-value";
    assert!(
        !artifact
            .bytes
            .windows(state_value.len())
            .any(|window| window == state_value.as_bytes())
    );
    assert!(
        !serde_json::to_string(&artifact.metadata)
            .unwrap()
            .contains(state_value)
    );
    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &first_host,
            request(state_value),
        )
        .expect("first invocation should commit");
    assert_eq!(first.response.body, "none");
    assert_eq!(first.stats.policy_version, 2);
    assert_eq!(first.stats.state_reads, 1);
    assert_eq!(first.stats.state_writes, 1);
    assert!(
        !serde_json::to_string(&first.stats)
            .unwrap()
            .contains(state_value)
    );
    assert!(first.events.is_empty());
    drop(first_host);

    let second_host = host(inputs, durable(database.clone(), false));
    let second = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &second_host,
            request("two"),
        )
        .expect("restarted invocation should read durable state");
    assert_eq!(second.response.body, state_value);

    let trapping = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match state_put("agent-work", "rolled-back", request.body) {
        Ok(done) => {
            let boom = 1 / 0;
            record { status: 200, headers: [], body: request.body }
        },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let error = runtime
        .invoke_webhook_with_host(
            &trapping.bytes,
            &trapping.metadata,
            &grants,
            &second_host,
            request("never"),
        )
        .expect_err("trapping invocation should roll back");
    assert_eq!(error.code(), "K4004");
    let direct = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    assert_eq!(direct.get("rolled-back").unwrap(), None);
    assert_eq!(direct.get("last").unwrap(), Some(b"two".to_vec()));
}

#[test]
fn checkpoint_resume_replays_completed_http_without_repeating_the_mock_call() {
    let directory = TestDirectory::new("checkpoint-replay");
    let database = directory.database();
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let cancellation = CancellationHandle::new();
    let cancel_after_replay = cancellation.clone();
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("one request should arrive");
        let mut request = [0; 2048];
        let _ = stream
            .read(&mut request)
            .expect("request should be readable");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nremote")
            .expect("response should write");
        drop(stream);
        thread::sleep(Duration::from_millis(3));
        cancel_after_replay.cancel();
    });
    let probes = (0..900)
        .map(|index| {
            format!(
                "            let probe{index} = state_get(\"agent-work\", \"probe-{index}\");\n"
            )
        })
        .collect::<String>();
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "GET",
        path: "/work",
        query: "",
        headers: [],
        body: "",
    }};
    match replay_http("agent-work", "fetch-work", "{origin}", outbound) {{
        Ok(response) => {{
{probes}            match checkpoint_put("agent-work", "fetched", response.body) {{
            Ok(done) => record {{ status: 200, headers: [], body: response.body }},
            Err(error) => record {{ status: 500, headers: [], body: error }},
            }}
        }},
        Err(error) => record {{ status: 502, headers: [], body: error }},
    }}
}}
"#
        ),
        &["state.transaction"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nstate = [\"agent-work\"]"));
    let grants = GrantSet::from_manifest(&manifest);
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .unwrap()
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let runtime = Runtime::default();
    let first_host = host(inputs.clone(), durable(database.clone(), false));
    let first = runtime.invoke_webhook_with_cancellation(
        &artifact.bytes,
        &artifact.metadata,
        &grants,
        &first_host,
        &cancellation,
        request("fail"),
    );
    assert_eq!(
        first.expect_err("first invocation should cancel").code(),
        "K5106"
    );
    drop(first_host);
    mock.join().expect("mock should complete");

    let store = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    assert_eq!(store.checkpoint("fetched").unwrap(), None);
    assert_eq!(store.replay_counts().unwrap().0, 1);
    drop(store);

    let second_host = host(inputs, durable(database.clone(), false));
    let second = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &second_host,
            request("resume"),
        )
        .expect("second invocation should replay and commit checkpoint");
    assert_eq!(second.response.body, "remote");
    assert_eq!(second.stats.http_calls, 0);
    assert_eq!(second.stats.replay_hits, 1);
    let store = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    assert_eq!(
        store.checkpoint("fetched").unwrap(),
        Some(b"remote".to_vec())
    );
    drop(store);

    let reader = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match checkpoint_get("agent-work", "fetched") {
        Ok(value) => match value {
            Some(saved) => record { status: 200, headers: [], body: saved },
            None => record { status: 404, headers: [], body: "missing" },
        },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let checkpoint = runtime
        .invoke_webhook_with_host(
            &reader.bytes,
            &reader.metadata,
            &grants,
            &second_host,
            request("read"),
        )
        .expect("checkpoint reader should succeed");
    assert_eq!(checkpoint.response.body, "remote");
    assert_eq!(checkpoint.stats.checkpoint_reads, 1);
}

#[test]
fn durable_inbound_idempotency_replays_across_hosts_and_is_credential_sensitive() {
    let directory = TestDirectory::new("idempotency");
    let database = directory.database();
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("one request should arrive");
        let mut request = [0; 2048];
        let _ = stream
            .read(&mut request)
            .expect("request should be readable");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .expect("response should write");
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "GET",
        path: "/once",
        query: "",
        headers: [],
        body: "",
    }};
    match http_request("{origin}", outbound, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 502, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nstate = [\"agent-work\"]"));
    let grants = GrantSet::from_manifest(&manifest);
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .unwrap()
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let runtime = Runtime::default();
    let mut incoming = request("same");
    incoming.headers = vec![
        HttpHeader {
            name: "idempotency-key".to_owned(),
            value: "request-one".to_owned(),
        },
        HttpHeader {
            name: "x-api-credential".to_owned(),
            value: "credential-a".to_owned(),
        },
    ];
    let first_host = host(inputs.clone(), durable(database.clone(), true));
    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &first_host,
            incoming.clone(),
        )
        .expect("first request should execute");
    assert_eq!(first.response.body, "ok");
    drop(first_host);
    mock.join().expect("mock should complete");

    let second_host = host(inputs, durable(database.clone(), true));
    let replayed = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &second_host,
            incoming.clone(),
        )
        .expect("second request should replay");
    assert!(replayed.stats.idempotency_replayed);
    assert_eq!(replayed.stats.policy_version, 2);
    assert_eq!(replayed.response.body, "ok");

    incoming.headers[1].value = "credential-b".to_owned();
    let conflict = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &second_host,
            incoming,
        )
        .expect("digest conflict should be an HTTP response");
    assert_eq!(conflict.response.status, 409);
    let store = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    assert_eq!(store.idempotency_counts().unwrap().0, 1);
}

#[test]
fn durable_ai_replay_survives_agent_host_restart() {
    let directory = TestDirectory::new("ai-replay");
    let database = directory.database();
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("one AI request should arrive");
        let mut request = [0; 4096];
        let size = stream
            .read(&mut request)
            .expect("AI request should be readable");
        let request = String::from_utf8_lossy(&request[..size]).to_ascii_lowercase();
        assert!(request.contains("idempotency-key: krit-replay-"));
        let body = r#"{"output":"summary"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("AI response should write");
    });
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match replay_ai("agent-work", "summarize", "reviewer", request.body) {
        Ok(summary) => record { status: 200, headers: [], body: summary },
        Err(error) => record { status: 502, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let manifest = manifest(&format!(
        "http = [\"{origin}\"]\nai = [\"reviewer\"]\nstate = [\"agent-work\"]"
    ));
    let grants = GrantSet::from_manifest(&manifest);
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .unwrap()
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let make_policy = || {
        let mut policy = AgentHostPolicy::default();
        policy.ai_adapters.insert(
            "reviewer".to_owned(),
            AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
                origin: origin.clone(),
                path: "/ai".to_owned(),
                model: "test".to_owned(),
                secret: None,
                max_input_bytes: 1024,
                max_response_bytes: 1024,
                timeout: Duration::from_millis(500),
            }),
        );
        policy
    };
    let make_host = || {
        AgentHost::new_with_state(
            inputs.clone(),
            make_policy(),
            std::sync::Arc::new(
                ExplicitApprovalPolicy::new([(ApprovalOperation::AiInvoke, "reviewer".to_owned())])
                    .unwrap(),
            ),
            durable(database.clone(), false),
        )
        .unwrap()
    };
    let runtime = Runtime::default();
    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &make_host(),
            request("input"),
        )
        .expect("first AI replay operation should execute");
    assert_eq!(first.response.body, "summary");
    assert_eq!(first.stats.ai_calls, 1);
    assert_eq!(first.stats.replay_misses, 1);
    mock.join().expect("AI mock should complete");

    let denied_host = AgentHost::new_with_state(
        inputs.clone(),
        make_policy(),
        std::sync::Arc::new(DenyAllApprovalPolicy),
        durable(database.clone(), false),
    )
    .unwrap();
    let denied = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &denied_host,
            request("input"),
        )
        .expect("replay approval denial should be guest-visible");
    assert_eq!(denied.response.status, 502);
    assert!(denied.response.body.contains("approval denied"));

    let second = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &make_host(),
            request("input"),
        )
        .expect("second AI operation should replay");
    assert_eq!(second.response.body, "summary");
    assert_eq!(second.stats.ai_calls, 0);
    assert_eq!(second.stats.replay_hits, 1);
}

#[test]
fn unsafe_non_idempotent_http_replay_is_rejected_before_network_access() {
    let directory = TestDirectory::new("unsafe-replay");
    let database = directory.database();
    let origin = "http://127.0.0.1:9";
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "POST",
        path: "/unsafe",
        query: "",
        headers: [],
        body: "side-effect",
    }};
    match replay_http("agent-work", "unsafe-post", "{origin}", outbound) {{
        Ok(response) => response,
        Err(error) => record {{ status: 502, headers: [], body: error }},
    }}
}}
"#
        ),
        &["state.transaction"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nstate = [\"agent-work\"]"));
    let grants = GrantSet::from_manifest(&manifest);
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .unwrap()
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let runtime = Runtime::default();
    let error = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(inputs, durable(database, false)),
            request("input"),
        )
        .expect_err("unsafe replay should trap before network access");
    assert_eq!(error.code(), "K5203");
    assert!(error.message().contains("Idempotency-Key"));
}

#[test]
fn replay_http_distinguishes_outbound_idempotency_keys() {
    let directory = TestDirectory::new("replay-idempotency-key");
    let database = directory.database();
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("one request should arrive");
        let mut request = [0; 2048];
        let size = stream
            .read(&mut request)
            .expect("request should be readable");
        let request = String::from_utf8_lossy(&request[..size]).to_ascii_lowercase();
        assert!(request.contains("idempotency-key: operation-one\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .expect("response should write");
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "POST",
        path: "/work",
        query: "",
        headers: [record {{ name: "idempotency-key", value: request.body }}],
        body: "same-operation-body",
    }};
    match replay_http("agent-work", "submit-work", "{origin}", outbound) {{
        Ok(response) => response,
        Err(error) => record {{ status: 502, headers: [], body: error }},
    }}
}}
"#
        ),
        &["state.transaction"],
    );
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "http = [\"{origin}\"]\nstate = [\"agent-work\"]"
    )));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .unwrap()
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let runtime = Runtime::default();
    let agent_host = host(inputs, durable(database, false));

    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &agent_host,
            request("operation-one"),
        )
        .expect("first keyed operation should execute");
    assert_eq!(first.response.body, "ok");
    mock.join().expect("mock should complete");

    let error = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &agent_host,
            request("operation-two"),
        )
        .expect_err("a different keyed operation must conflict with the replay record");
    assert_eq!(error.code(), "K5203");
}

#[test]
fn protocol_one_rejects_cross_store_transactions_without_partial_commit() {
    let directory = TestDirectory::new("cross-store");
    let first_path = directory.path.join("first.db");
    let second_path = directory.path.join("second.db");
    let state = DurableState::open(
        BTreeMap::from([
            (
                "store-a".to_owned(),
                DurableStoreDefinition {
                    path: first_path.clone(),
                    durability: Durability::Full,
                    limits: store_limits(),
                    replay: retention(),
                },
            ),
            (
                "store-b".to_owned(),
                DurableStoreDefinition {
                    path: second_path,
                    durability: Durability::Full,
                    limits: store_limits(),
                    replay: retention(),
                },
            ),
        ]),
        None,
    )
    .unwrap();
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let first = state_put("store-a", "key", "one");
    let second = state_put("store-b", "key", "two");
    record { status: 200, headers: [], body: request.body }
}
"#,
        &["state.transaction"],
    );
    let grants = GrantSet::from_manifest(&manifest("state = [\"store-a\", \"store-b\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default()).unwrap();
    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(inputs, state),
            request("input"),
        )
        .expect_err("cross-store transaction should fail");
    assert_eq!(error.code(), "K5202");
    let first = DurableStore::open(&first_path, Durability::Full, store_limits()).unwrap();
    assert_eq!(first.get("key").unwrap(), None);
}

#[test]
fn runtime_rejects_short_replay_leases_and_oversized_mutations() {
    let lease_directory = TestDirectory::new("short-lease");
    let mut short_retention = retention();
    short_retention.lease = Duration::from_millis(100);
    let short_state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: lease_directory.database(),
                durability: Durability::Full,
                limits: store_limits(),
                replay: short_retention,
            },
        )]),
        None,
    )
    .unwrap();
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match state_get("agent-work", "key") {
        Ok(value) => record { status: 200, headers: [], body: request.body },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let grants = GrantSet::from_manifest(&manifest("state = [\"agent-work\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default()).unwrap();
    let lease_error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(inputs.clone(), short_state),
            request("input"),
        )
        .expect_err("short replay lease should fail runtime preflight");
    assert_eq!(lease_error.code(), "K5201");

    let value_directory = TestDirectory::new("value-limit");
    let value_database = value_directory.database();
    let mut value_limits = store_limits();
    value_limits.max_value_bytes = 4;
    let value_state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: value_database.clone(),
                durability: Durability::Full,
                limits: value_limits,
                replay: retention(),
            },
        )]),
        None,
    )
    .unwrap();
    let writer = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match state_put("agent-work", "key", request.body) {
        Ok(done) => record { status: 200, headers: [], body: "stored" },
        Err(error) => record { status: 422, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let value_error = Runtime::default()
        .invoke_webhook_with_host(
            &writer.bytes,
            &writer.metadata,
            &grants,
            &host(inputs, value_state),
            request("oversized"),
        )
        .expect_err("oversized mutation must not report success to the guest");
    assert_eq!(value_error.code(), "K5202");
    let store = DurableStore::open(&value_database, Durability::Full, value_limits).unwrap();
    assert_eq!(store.get("key").unwrap(), None);
}

#[test]
fn read_only_invocations_do_not_advance_the_store_revision() {
    let directory = TestDirectory::new("read-only");
    let database = directory.database();
    let store = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    store
        .commit(
            0,
            &[krit_state::Mutation::Put {
                key: "key".to_owned(),
                value: b"value".to_vec(),
            }],
        )
        .unwrap();
    drop(store);
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match state_get("agent-work", "key") {
        Ok(value) => match value {
            Some(saved) => record { status: 200, headers: [], body: saved },
            None => record { status: 404, headers: [], body: "missing" },
        },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["state.transaction"],
    );
    let grants = GrantSet::from_manifest(&manifest("state = [\"agent-work\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default()).unwrap();
    let execution = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host(inputs, durable(database.clone(), false)),
            request("input"),
        )
        .expect("read-only invocation should succeed");
    assert_eq!(execution.response.body, "value");
    let store = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    assert_eq!(store.revision().unwrap(), 1);
}

#[test]
fn responses_above_the_store_cache_bound_release_the_idempotency_lease() {
    let directory = TestDirectory::new("idempotency-store-bound");
    let database = directory.database();
    let mut limits = store_limits();
    limits.max_replay_bytes = 64;
    let mut replay = retention();
    replay.max_bytes = 64;
    let state = DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: database.clone(),
                durability: Durability::Full,
                limits,
                replay,
            },
        )]),
        Some("agent-work".to_owned()),
    )
    .unwrap();
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
        &[],
    );
    let grants = GrantSet::from_manifest(&manifest("state = [\"agent-work\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default()).unwrap();
    let host = host(inputs, state);
    let mut incoming = request(&"x".repeat(80));
    incoming.headers = vec![HttpHeader {
        name: "idempotency-key".to_owned(),
        value: "large-response".to_owned(),
    }];
    let runtime = Runtime::default();
    for _ in 0..2 {
        let execution = runtime
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                incoming.clone(),
            )
            .expect("an uncacheable successful response should still be returned");
        assert!(!execution.stats.idempotency_replayed);
        assert_eq!(execution.response.body.len(), 80);
    }
    let store = DurableStore::open(&database, Durability::Full, limits).unwrap();
    assert_eq!(store.idempotency_counts().unwrap(), (0, 0));
}

#[test]
fn failed_invocations_commit_neither_state_nor_durable_idempotency() {
    let directory = TestDirectory::new("failed-idempotency");
    let database = directory.database();
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mock = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("request should arrive");
            let mut request = [0; 2048];
            let _ = stream
                .read(&mut request)
                .expect("request should be readable");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("response should write");
        }
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "GET",
        path: "/twice",
        query: "",
        headers: [],
        body: "",
    }};
    match http_request("{origin}", outbound, None) {{
        Ok(response) => {{
            let staged = state_put("agent-work", "failed", response.body);
            let boom = 1 / 0;
            record {{ status: 200, headers: [], body: response.body }}
        }},
        Err(error) => record {{ status: 502, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request", "state.transaction"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nstate = [\"agent-work\"]"));
    let grants = GrantSet::from_manifest(&manifest);
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .unwrap()
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let mut incoming = request("same");
    incoming.headers = vec![HttpHeader {
        name: "idempotency-key".to_owned(),
        value: "failed-request".to_owned(),
    }];
    let runtime = Runtime::default();
    for _ in 0..2 {
        let error = runtime
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host(inputs.clone(), durable(database.clone(), true)),
                incoming.clone(),
            )
            .expect_err("failed invocation must not complete idempotency");
        assert_eq!(error.code(), "K4004");
    }
    mock.join()
        .expect("both uncached calls should reach the mock");
    let store = DurableStore::open(&database, Durability::Full, store_limits()).unwrap();
    assert_eq!(store.get("failed").unwrap(), None);
    assert_eq!(store.idempotency_counts().unwrap(), (0, 0));
}
