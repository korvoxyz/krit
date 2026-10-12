//! Webhooks that combine the 0.3 control-flow features (postfix `?`,
//! let-else, nested patterns) with the 0.3 data features (named
//! construction, entry lists, the unit value, doc comments, interpolation,
//! text blocks, and the standard library) build and run.
//!
//! Each feature lowers on its own; these tests pin their interactions: a `?`
//! inside a named construction's field, an entry list, or a
//! `url_path_segment` call in an outbound request path; let-else arms that
//! build responses with holes and text blocks; nested patterns over
//! `state_get`'s `Result<Option<String>, String>` whose arms interpolate.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use krit::{Source, analyze_with_warnings, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, DenyAllApprovalPolicy, Durability, DurableState,
    DurableStoreDefinition, DurableStoreLimits, GrantSet, HostInputs, HttpHeader, HttpRequest,
    NetworkPolicy, RetentionPolicy, Runtime, SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "krit-runtime-cross-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Checks `source_text` without any warning, in particular no `K9013` for
/// the paths it builds, and compiles it with `effects` granted.
fn compile(source_text: &str, effects: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let report = analyze_with_warnings(&program);
    assert!(
        report.warnings.is_empty(),
        "unexpected warnings: {:?}",
        report.warnings
    );
    let analysis = report.result.expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/cross", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("source should compile")
}

fn grants(capabilities: &str) -> GrantSet {
    GrantSet::from_manifest(
        &Manifest::parse(&format!(
            r#"
schema = 1

[package]
name = "test/cross"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
        ))
        .expect("manifest should parse"),
    )
}

fn inputs() -> HostInputs {
    HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid")
}

fn request(method: &str, query: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        path: "/notes".to_owned(),
        query: query.to_owned(),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

fn header(name: &str, value: &str) -> HttpHeader {
    HttpHeader {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

const NOTES: &str = r#"
/// The state key of the note a request names, encoded so the name cannot
/// leave its segment.
///
/// Fails when the name cannot be a path segment.
fn note_key(name: String) -> Result<String, String> {
    let segment = url_path_segment(name)?;
    Ok("notes/\(segment)")
}

/// Answers a note read, deciding the stored value with nested patterns.
fn read(key: String, name: String) -> HttpResponse {
    match state_get("agent-work", key) {
        Ok(Some(text)) => HttpResponse {
            status: 200,
            headers: ["x-key": key, "x-bytes": int_to_string(string_byte_len(text))],
            body: "\(name): \(text)",
        },
        Ok(None) => HttpResponse { status: 404, body: """
            missing note
              \(name)
            """ },
        Err(error) => HttpResponse { status: 500, body: "state failed: \(error)" },
    }
}

/// Stores a note, passing every failure on with `?`.
fn write(key: String, text: String) -> Result<HttpResponse, String> {
    let previous = state_get("agent-work", key)?;
    let stored: Unit = state_put("agent-work", key, text)?;
    let replaced = match previous {
        Some(_) => "true",
        None => "false",
    };
    Ok(
        HttpResponse {
            status: 201,
            headers: ["x-key": key, "x-replaced": replaced],
            body: "stored " + url_path_segment(text)?,
        },
    )
}

/// Stores a note in a helper that answers every outcome with let-else.
fn put(key: String, text: String) -> HttpResponse {
    let Ok(response) = write(key, text) else
        Err(error) => HttpResponse { status: 500, body: "not stored: \(error)" };
    response
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let name = request.query;
    let Ok(key) = note_key(name) else
        Err(error) => HttpResponse { status: 400, body: "bad name \(name): \(error)" };
    if request.method == "PUT" {
        put(key, request.body)
    } else {
        read(key, name)
    }
}
"#;

fn durable(directory: &TestDirectory) -> DurableState {
    DurableState::open(
        BTreeMap::from([(
            "agent-work".to_owned(),
            DurableStoreDefinition {
                path: directory.path.join("state.db"),
                durability: Durability::Full,
                limits: DurableStoreLimits {
                    busy_timeout: Duration::from_millis(250),
                    max_operations: 1024,
                    max_key_bytes: 256,
                    max_value_bytes: 64 * 1024,
                    max_transaction_bytes: 1024 * 1024,
                    max_database_bytes: 64 * 1024 * 1024,
                    max_replay_entries: 1024,
                    max_replay_bytes: 16 * 1024 * 1024,
                },
                replay: RetentionPolicy {
                    max_entries: 1024,
                    max_bytes: 16 * 1024 * 1024,
                    ttl: Duration::from_secs(7 * 24 * 60 * 60),
                    lease: Duration::from_secs(30),
                },
            },
        )]),
        None,
    )
    .expect("durable state should open")
}

#[test]
fn a_notes_webhook_mixing_control_and_data_features_builds_and_runs() {
    let directory = TestDirectory::new("notes");
    let artifact = compile(NOTES, &["state.transaction"]);
    let grants = grants("state = [\"agent-work\"]");
    let host = AgentHost::new_with_state(
        inputs(),
        AgentHostPolicy::default(),
        Arc::new(DenyAllApprovalPolicy),
        durable(&directory),
    )
    .expect("agent host should build");
    let send = |method: &str, name: &str, body: &str| {
        Runtime::default()
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                request(method, name, body),
            )
            .expect("the webhook should run")
            .response
    };

    // `Ok(None)`: a text block with a hole, from a nested-pattern arm.
    let missing = send("GET", "first+note", "");
    assert_eq!(missing.status, 404);
    assert_eq!(missing.body, "missing note\n  first+note");
    assert!(missing.headers.is_empty());

    // The write passes `state_get`, `state_put`, and `url_path_segment`
    // results on with `?`, the last inside a named construction's field.
    let stored = send("PUT", "first+note", "hello world");
    assert_eq!(stored.status, 201);
    assert_eq!(stored.body, "stored hello%20world");
    assert_eq!(
        stored.headers,
        [
            header("x-key", "notes/first%2Bnote"),
            header("x-replaced", "false")
        ]
    );

    // `Ok(Some(text))`: interpolation and an entry list in a construction.
    let found = send("GET", "first+note", "");
    assert_eq!(found.status, 200);
    assert_eq!(found.body, "first+note: hello world");
    assert_eq!(
        found.headers,
        [
            header("x-key", "notes/first%2Bnote"),
            header("x-bytes", "11")
        ]
    );

    // A `?` that fails inside the construction's field returns `Err` from
    // `write`, whose let-else arm answers; the state write before it is
    // committed with the invocation, as the effects before a `?` stay
    // performed.
    let refused = send("PUT", "first+note", "..");
    assert_eq!(refused.status, 500);
    assert_eq!(
        refused.body,
        "not stored: path segment reads as `.` or `..`"
    );
    let overwritten = send("GET", "first+note", "");
    assert_eq!(overwritten.body, "first+note: ..");

    // The webhook body's own let-else answers before any state access.
    let bad = send("GET", "..", "");
    assert_eq!(bad.status, 400);
    assert_eq!(bad.body, "bad name ..: path segment reads as `.` or `..`");
}

fn read_request(stream: &mut TcpStream) -> String {
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
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).expect("request should be UTF-8")
}

fn fetching_webhook(origin: &str) -> String {
    format!(
        r#"
/// Fetches one note from the notes service, encoding its name as a single
/// path segment and passing every failure on with `?`.
fn fetch(name: String, page: Int) -> Result<String, String> {{
    let response = http_request(
        "{origin}",
        HttpRequest {{
            method: "GET",
            path: "/notes/" + url_path_segment(name)?,
            query: url_query(["page": int_to_string(page), "q": name]),
            headers: ["accept": "text/plain", "x-name": name],
        }},
        None,
    )?;
    let status = int_to_string(response.status);
    if response.status == 200 {{
        Ok(response.body)
    }} else {{
        Err("upstream answered \(status)")
    }}
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let name = request.body;
    let Ok(note) = fetch(name, 2) else
        Err(error) => HttpResponse {{ status: 502, body: "could not fetch \(name): \(error)" }};
    let Ok(_) = log_info("note.fetched", ["name": name, "bytes": int_to_string(string_byte_len(note))]) else
        Err(error) => HttpResponse {{ status: 500, body: error }};
    HttpResponse {{ status: 200, headers: ["x-name": name], body: """
        note \(name)
        \(note)
        """ }}
}}
"#
    )
}

#[test]
fn an_outbound_path_built_with_a_propagated_segment_is_sent_encoded() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("mock address").port()
    );
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request should arrive");
        let captured = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .expect("mock response should write");
        captured
    });

    let artifact = compile(&fetching_webhook(&origin), &["http.request", "observe.log"]);
    let grants = grants(&format!("http = [\"{origin}\"]\nlogs = true"));
    let inputs = inputs().with_network_policy(NetworkPolicy::loopback_for_tests());
    let runtime = Runtime::default();

    let fetched = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &inputs,
            request("POST", "", "octo cat"),
        )
        .expect("the webhook should run");
    assert_eq!(fetched.response.status, 200);
    assert_eq!(fetched.response.body, "note octo cat\nhello");
    assert_eq!(fetched.response.headers, [header("x-name", "octo cat")]);
    assert_eq!(fetched.stats.http_calls, 1);
    assert_eq!(fetched.events.len(), 1);
    assert_eq!(fetched.events[0].event, "note.fetched");
    assert_eq!(
        fetched.events[0]
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.value.as_str()))
            .collect::<Vec<_>>(),
        [("name", "octo cat"), ("bytes", "5")]
    );
    let captured = mock.join().expect("mock should finish");
    assert!(
        captured.starts_with("GET /notes/octo%20cat?page=2&q=octo%20cat HTTP/1.1\r\n"),
        "unexpected request: {captured}"
    );
    let lower = captured.to_ascii_lowercase();
    assert!(lower.contains("\r\naccept: text/plain\r\n"), "{captured}");
    assert!(lower.contains("\r\nx-name: octo cat\r\n"), "{captured}");

    // A name that is not a segment fails at `url_path_segment(name)?`, inside
    // the construction, before any request is made; the webhook's let-else
    // arm answers.
    let refused = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &inputs,
            request("POST", "", ".."),
        )
        .expect("the webhook should run");
    assert_eq!(refused.response.status, 502);
    assert_eq!(
        refused.response.body,
        "could not fetch ..: path segment reads as `.` or `..`"
    );
    assert_eq!(refused.stats.http_calls, 0);
    assert!(refused.events.is_empty());
}
