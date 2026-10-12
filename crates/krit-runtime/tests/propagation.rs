//! Postfix `?` in deployed components.
//!
//! A helper that returns `Result` chains fallible host calls with `?`; each
//! `Err` returns from the helper at once through a WebAssembly `return`, from
//! any nesting depth, and the webhook entrypoint matches the helper's result.
//! Each test invokes the built component with host inputs that succeed or
//! fail at a chosen step.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, ApprovalOperation, ExplicitApprovalPolicy, GrantSet, HostInputs,
    HttpRequest, HttpResponse, NetworkPolicy, Runtime, SecretStore, WebhookExecutionResult,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

fn compile(source_text: &str, capabilities: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/propagation", "1.0.0", "src/main.krit");
    for capability in capabilities {
        options.grant_effect(*capability);
    }
    build_component(&module, &options).expect("source should compile")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/propagation"
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

fn request(method: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        path: "/incoming".to_owned(),
        query: String::new(),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

/// A webhook whose helper chains `config_string(...)?`, `secret(...)?`, and
/// `http_request(...)?` against `origin`.
fn forwarding_webhook(origin: &str) -> BuiltComponent {
    compile(
        &format!(
            r#"
fn forward(request: HttpRequest) -> Result<HttpResponse, String> {{
    let model = config_string("agent.model")?;
    let token = secret("upstream-token")?;
    let outbound: HttpRequest = record {{
        method: "POST",
        path: "/upstream",
        query: "",
        headers: [record {{ name: "x-model", value: model }}],
        body: request.body,
    }};
    let response = http_request("{origin}", outbound, Some(token))?;
    Ok(record {{ status: response.status, headers: [], body: "upstream: " + response.body }})
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match forward(request) {{
        Ok(response) => response,
        Err(error) => record {{ status: 502, headers: [], body: "failed: " + error }},
    }}
}}
"#
        ),
        &["config.read", "http.request", "secret.read"],
    )
}

/// Host inputs holding the given configuration and secret entries.
fn host(origin: &str, config: &[(&str, &str)], secrets: &[(&str, &str)]) -> AgentHost {
    let inputs = HostInputs::new(
        config
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        SecretStore::new(
            secrets
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.as_bytes().to_vec()))
                .collect(),
        )
        .expect("secret store should be valid"),
    )
    .expect("host inputs should be valid")
    .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    AgentHost::new(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(
            ExplicitApprovalPolicy::new([(ApprovalOperation::HttpBearer, origin.to_owned())])
                .expect("approval should be valid"),
        ),
    )
    .expect("agent host should be valid")
}

fn invoke(artifact: &BuiltComponent, origin: &str, host: &AgentHost) -> WebhookExecutionResult {
    let manifest = manifest(&format!(
        "config = [\"agent.model\"]\nhttp = [\"{origin}\"]\nsecrets = [\"upstream-token\"]"
    ));
    Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            host,
            request("POST", "hello"),
        )
        .expect("every outcome of the helper should be guest-visible")
}

/// Serves one request on `listener`, sends its bytes to the returned channel,
/// and answers `200 world`.
fn serve_once(listener: TcpListener) -> (mpsc::Receiver<Vec<u8>>, thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept one request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("mock timeout should configure");
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let count = stream.read(&mut chunk).expect("mock request should read");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") && bytes.ends_with(b"hello") {
                break;
            }
        }
        sender.send(bytes).expect("mock capture should send");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nworld")
            .expect("mock response should write");
    });
    (receiver, server)
}

#[test]
fn a_helper_chaining_question_marks_reaches_the_upstream_when_every_step_succeeds() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener
            .local_addr()
            .expect("mock address should exist")
            .port()
    );
    let (captured, server) = serve_once(listener);
    let artifact = forwarding_webhook(&origin);
    let result = invoke(
        &artifact,
        &origin,
        &host(
            &origin,
            &[("agent.model", "krit-test")],
            &[("upstream-token", "unit-value")],
        ),
    );
    assert_eq!(result.response.status, 200);
    assert_eq!(result.response.body, "upstream: world");
    assert_eq!(result.stats.http_calls, 1);
    let captured = String::from_utf8(
        captured
            .recv_timeout(Duration::from_secs(2))
            .expect("mock should capture the request"),
    )
    .expect("captured request should be UTF-8")
    .to_ascii_lowercase();
    assert!(captured.starts_with("post /upstream http/1.1\r\n"));
    assert!(captured.contains("authorization: bearer unit-value\r\n"));
    assert!(captured.contains("x-model: krit-test\r\n"));
    server.join().expect("mock should finish");
}

#[test]
fn each_failing_step_returns_its_error_from_the_helper_without_running_later_steps() {
    // Nothing listens on this origin, so a request that gets this far fails
    // to connect.
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("address should exist").port()
    );
    drop(listener);
    let artifact = forwarding_webhook(&origin);

    let missing_config = invoke(&artifact, &origin, &host(&origin, &[], &[]));
    assert_eq!(missing_config.response.status, 502);
    assert_eq!(
        missing_config.response.body,
        "failed: configuration key `agent.model` is not configured"
    );
    assert_eq!(missing_config.stats.host_calls, 1);
    assert_eq!(missing_config.stats.http_calls, 0);

    let missing_secret = invoke(
        &artifact,
        &origin,
        &host(&origin, &[("agent.model", "krit-test")], &[]),
    );
    assert_eq!(missing_secret.response.status, 502);
    assert!(
        missing_secret.response.body.starts_with("failed: ")
            && missing_secret.response.body.contains("upstream-token"),
        "{}",
        missing_secret.response.body
    );
    assert_eq!(missing_secret.stats.host_calls, 2);
    assert_eq!(missing_secret.stats.http_calls, 0);

    let unreachable = invoke(
        &artifact,
        &origin,
        &host(
            &origin,
            &[("agent.model", "krit-test")],
            &[("upstream-token", "unit-value")],
        ),
    );
    assert_eq!(unreachable.response.status, 502);
    assert!(
        unreachable.response.body.starts_with("failed: "),
        "{}",
        unreachable.response.body
    );
    assert_eq!(unreachable.stats.http_calls, 1);
}

/// A helper whose `?` exits sit inside an `if` branch, a `match` arm in an
/// `else` block, and the operands of `+`.
const NESTED_SOURCE: &str = r#"
fn describe(request: HttpRequest) -> Result<String, String> {
    let prefix = if request.method == "POST" {
        "post " + config_string("agent.model")?
    } else {
        match config_string("agent.mode") {
            Ok(mode) => "mode " + mode + " " + config_string("agent.model")?,
            Err(error) => "no mode",
        }
    };
    Ok(prefix + " / " + config_string("agent.suffix")?)
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match describe(request) {
        Ok(body) => record { status: 200, headers: [], body: body },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#;

#[test]
fn question_marks_return_from_nested_blocks_and_operands() {
    let artifact = compile(NESTED_SOURCE, &["config.read"]);
    let manifest = manifest(r#"config = ["agent.model", "agent.mode", "agent.suffix"]"#);
    let grants = GrantSet::from_manifest(&manifest);
    let respond = |method: &str, config: &[(&str, &str)]| -> HttpResponse {
        let inputs = HostInputs::new(
            config
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            SecretStore::default(),
        )
        .expect("host inputs should be valid");
        Runtime::default()
            .invoke_webhook(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &inputs,
                request(method, ""),
            )
            .expect("the webhook should run")
            .response
    };
    let unset = |key: &str| format!("configuration key `{key}` is not configured");

    let full = [
        ("agent.model", "m"),
        ("agent.mode", "fast"),
        ("agent.suffix", "s"),
    ];
    let check = |method: &str, config: &[(&str, &str)], status: i64, body: &str| {
        let response = respond(method, config);
        assert_eq!(response.status, status, "{method} {config:?}");
        assert_eq!(response.body, body, "{method} {config:?}");
    };
    check("POST", &full, 200, "post m / s");
    check("GET", &full, 200, "mode fast m / s");
    // The `?` in the `if` branch, and the one in a `match` arm in the `else`
    // block, each return the missing model.
    check("POST", &full[1..], 500, &unset("agent.model"));
    check("GET", &full[1..], 500, &unset("agent.model"));
    check("GET", &[("agent.suffix", "s")], 200, "no mode / s");
    // The `?` in the operand of `+` returns the missing suffix.
    check("GET", &[], 500, &unset("agent.suffix"));
}
