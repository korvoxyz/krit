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
    HttpHeader, HttpRequest, HttpResponse, NetworkPolicy, Runtime, SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

fn compile(source_text: &str, capabilities: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("webhook source should parse");
    let analysis = analyze(&program).expect("webhook source should analyze");
    let module = lower(&program, &analysis).expect("webhook source should lower");
    let mut options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
    for capability in capabilities {
        options.grant_effect(*capability);
    }
    build_component(&module, &options).expect("webhook source should compile")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/webhook"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
    ))
    .expect("webhook manifest should parse")
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

#[test]
fn invokes_a_typed_pure_webhook() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record {
        status: 201,
        headers: request.headers,
        body: request.body,
    }
}
"#,
        &[],
    );
    let request = HttpRequest {
        headers: vec![HttpHeader {
            name: "x-test".to_owned(),
            value: "one".to_owned(),
        }],
        query: "a=1".to_owned(),
        path: "/echo".to_owned(),
        ..request("hello")
    };
    let manifest = manifest("");
    let runtime = Runtime::default();
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid");
    let grants = GrantSet::from_manifest(&manifest);
    let result = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &inputs,
            request.clone(),
        )
        .expect("webhook should run");
    let repeated = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &inputs,
            request,
        )
        .expect("repeated webhook should use fresh state");

    assert_eq!(
        result.response,
        HttpResponse {
            status: 201,
            headers: vec![HttpHeader {
                name: "x-test".to_owned(),
                value: "one".to_owned(),
            }],
            body: "hello".to_owned(),
        }
    );
    assert!(result.output.is_empty());
    assert_eq!(result.stats.http_calls, 0);
    assert_eq!(repeated.response, result.response);
    assert_eq!(repeated.stats.host_calls, result.stats.host_calls);
    assert_eq!(runtime.active_deadline_workers(), 0);
}

#[test]
fn config_results_are_visible_to_the_webhook() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match config_string("agent.model") {
        Ok(model) => record { status: 200, headers: [], body: model },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
        &["config.read"],
    );
    let manifest = manifest(r#"config = ["agent.model"]"#);
    let grants = GrantSet::from_manifest(&manifest);

    let missing = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("empty host inputs should be valid"),
            request(""),
        )
        .expect("missing config should be a guest-visible result");
    assert_eq!(missing.response.status, 500);
    assert!(missing.response.body.contains("not configured"));

    let configured = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &HostInputs::new(
                BTreeMap::from([("agent.model".to_owned(), "krit-test".to_owned())]),
                SecretStore::default(),
            )
            .expect("host config should be valid"),
            request(""),
        )
        .expect("configured webhook should run");
    assert_eq!(configured.response.status, 200);
    assert_eq!(configured.response.body, "krit-test");
}

#[test]
fn config_secret_and_http_form_a_real_bounded_webhook_path() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener should bind");
    let address = listener.local_addr().expect("mock address should exist");
    let (sender, receiver) = mpsc::sync_channel(1);
    let mock = thread::spawn(move || {
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
            .write_all(
                b"HTTP/1.1 202 Accepted\r\nX-Upstream: yes\r\nContent-Length: 5\r\nConnection: close\r\n\r\nworld",
            )
            .expect("mock response should write");
    });
    let origin = format!("http://127.0.0.1:{}", address.port());
    let artifact = compile(
        &format!(
            r#"
fn make_failure(message: String) -> HttpResponse {{
    record {{ status: 502, headers: [], body: message }}
}}

fn failure(message: String) -> HttpResponse {{
    make_failure(message)
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match config_string("agent.model") {{
        Ok(model) => match secret("upstream-token") {{
            Ok(token) => {{
                let outbound: HttpRequest = record {{
                    method: "POST",
                    path: "/upstream",
                    query: "",
                    headers: [
                        record {{ name: "x-first", value: "one" }},
                        record {{ name: "x-model", value: model }},
                        record {{ name: "x-first", value: "three" }},
                    ],
                    body: request.body,
                }};
                match http_request("{origin}", outbound, Some(token)) {{
                    Ok(response) => response,
                    Err(error) => failure(error),
                }}
            }},
            Err(error) => failure(error),
        }},
        Err(error) => failure(error),
    }}
}}
"#
        ),
        &["config.read", "http.request", "secret.read"],
    );
    let manifest = manifest(&format!(
        "config = [\"agent.model\"]\nhttp = [\"{origin}\"]\nsecrets = [\"upstream-token\"]"
    ));
    let inputs = HostInputs::new(
        BTreeMap::from([("agent.model".to_owned(), "krit-test".to_owned())]),
        SecretStore::new(BTreeMap::from([(
            "upstream-token".to_owned(),
            b"unit-value".to_vec(),
        )]))
        .expect("secret store should be valid"),
    )
    .expect("host inputs should be valid")
    .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let host = AgentHost::new(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(
            ExplicitApprovalPolicy::new([(ApprovalOperation::HttpBearer, origin.clone())])
                .expect("approval should be valid"),
        ),
    )
    .expect("agent host should be valid");
    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request("hello"),
        )
        .expect("bounded outbound webhook should run");
    assert_eq!(result.response.status, 202);
    assert_eq!(result.response.body, "world");
    assert_eq!(
        result.response.headers,
        [HttpHeader {
            name: "x-upstream".to_owned(),
            value: "yes".to_owned(),
        }]
    );
    assert_eq!(result.stats.http_calls, 1);
    let captured = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("mock should capture request");
    let captured = String::from_utf8(captured).expect("mock request should be UTF-8");
    let lowercase = captured.to_ascii_lowercase();
    assert!(captured.starts_with("POST /upstream HTTP/1.1\r\n"));
    assert!(lowercase.contains("authorization: bearer unit-value\r\n"));
    assert!(lowercase.contains("x-model: krit-test\r\n"));
    let first = lowercase.find("x-first: one\r\n").expect("first duplicate");
    let model = lowercase
        .find("x-model: krit-test\r\n")
        .expect("middle header");
    let third = lowercase
        .find("x-first: three\r\n")
        .expect("second duplicate");
    assert!(
        first < model && model < third,
        "header order must be preserved"
    );
    assert!(captured.ends_with("hello"));
    mock.join().expect("mock should finish");
}

fn anonymous_http_artifact(origin: &str, error_body: &str) -> BuiltComponent {
    compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match http_request("{origin}", request, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 598, headers: [], body: {error_body} }},
    }}
}}
"#
        ),
        &["http.request"],
    )
}

fn spawn_mock(response: &'static [u8], delay: Duration) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener should bind");
    let address = listener.local_addr().expect("mock address should exist");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("mock timeout should configure");
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        thread::sleep(delay);
        let _ = stream.write_all(response);
    });
    (format!("http://127.0.0.1:{}", address.port()), handle)
}

#[test]
fn outbound_redirects_private_addresses_and_plaintext_bearers_are_denied() {
    let (redirect_origin, redirect_mock) = spawn_mock(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        Duration::ZERO,
    );
    let artifact = anonymous_http_artifact(&redirect_origin, "error");
    let redirect_manifest = manifest(&format!("http = [\"{redirect_origin}\"]"));
    let redirected = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&redirect_manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid")
                .with_network_policy(NetworkPolicy::loopback_for_tests()),
            request(""),
        )
        .expect("redirect denial should be guest-visible");
    assert_eq!(redirected.response.status, 598);
    assert!(redirected.response.body.contains("redirects are denied"));
    redirect_mock.join().expect("redirect mock should finish");

    let listener = TcpListener::bind("127.0.0.1:0").expect("private test listener should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener
            .local_addr()
            .expect("private test address should exist")
            .port()
    );
    let artifact = anonymous_http_artifact(&origin, "error");
    let private_manifest = manifest(&format!("http = [\"{origin}\"]"));
    let denied = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&private_manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            request(""),
        )
        .expect("private address denial should be guest-visible");
    assert_eq!(denied.response.status, 598);
    drop(listener);

    let metadata_origin = "http://169.254.169.254";
    let artifact = anonymous_http_artifact(metadata_origin, "error");
    let metadata_manifest = manifest(&format!("http = [\"{metadata_origin}\"]"));
    let denied = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&metadata_manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            request(""),
        )
        .expect("metadata address denial should be guest-visible");
    assert_eq!(denied.response.status, 598);
    assert!(denied.response.body.contains("link-local"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bearer test listener should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener
            .local_addr()
            .expect("bearer test address should exist")
            .port()
    );
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match secret("unit-secret") {{
        Ok(token) => match http_request("{origin}", request, Some(token)) {{
            Ok(response) => response,
            Err(error) => record {{ status: 598, headers: [], body: error }},
        }},
        Err(error) => record {{ status: 500, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request", "secret.read"],
    );
    let manifest = manifest(&format!(
        "http = [\"{origin}\"]\nsecrets = [\"unit-secret\"]"
    ));
    let inputs = HostInputs::new(
        BTreeMap::new(),
        SecretStore::new(BTreeMap::from([(
            "unit-secret".to_owned(),
            b"unit-value".to_vec(),
        )]))
        .expect("secret store should be valid"),
    )
    .expect("inputs should be valid")
    .with_network_policy(NetworkPolicy::loopback_for_tests());
    let host = AgentHost::new(
        inputs,
        AgentHostPolicy::default(),
        Arc::new(
            ExplicitApprovalPolicy::new([(ApprovalOperation::HttpBearer, origin.clone())])
                .expect("approval should be valid"),
        ),
    )
    .expect("agent host should be valid");
    let denied = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request(""),
        )
        .expect("plaintext bearer denial should be guest-visible");
    assert_eq!(denied.response.status, 598);
    assert!(denied.response.body.contains("plain HTTP"));
    drop(listener);
}

#[test]
fn outbound_timeout_and_response_body_limits_are_bounded() {
    let (timeout_origin, timeout_mock) = spawn_mock(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        Duration::from_millis(900),
    );
    let artifact = anonymous_http_artifact(&timeout_origin, "\"\"");
    let timeout_manifest = manifest(&format!("http = [\"{timeout_origin}\"]"));
    let started = std::time::Instant::now();
    let timed_out = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&timeout_manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid")
                .with_network_policy(NetworkPolicy::loopback_for_tests()),
            request(""),
        )
        .expect("network timeout should be guest-visible");
    assert_eq!(timed_out.response.status, 598);
    assert!(started.elapsed() < Duration::from_secs(2));
    timeout_mock.join().expect("timeout mock should finish");

    let (body_origin, body_mock) = spawn_mock(
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
        Duration::ZERO,
    );
    let artifact = anonymous_http_artifact(&body_origin, "\"\"");
    let body_manifest = manifest(&format!("http = [\"{body_origin}\"]"));
    let mut limits = krit_runtime::RuntimeLimits::default();
    limits
        .narrow_response_body_bytes(4)
        .expect("response limit should narrow");
    let limited = Runtime::new(limits)
        .expect("runtime should initialize")
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&body_manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid")
                .with_network_policy(NetworkPolicy::loopback_for_tests()),
            request(""),
        )
        .expect("body limit should be guest-visible");
    assert_eq!(limited.response.status, 598);
    body_mock.join().expect("body mock should finish");
}

#[test]
fn exact_origin_grants_and_request_response_validation_fail_closed() {
    let artifact = anonymous_http_artifact("https://api.example.com", "\"\"");
    let wrong_manifest = manifest(r#"http = ["https://other.example.com"]"#);
    let denied = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&wrong_manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            request(""),
        )
        .expect_err("wrong exact origin grant must fail authorization");
    assert_eq!(denied.code(), "K5001");

    let pure = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 99, headers: [], body: request.body }
}
"#,
        &[],
    );
    let invalid_response = Runtime::default()
        .invoke_webhook(
            &pure.bytes,
            &pure.metadata,
            &GrantSet::from_manifest(&manifest("")),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            request(""),
        )
        .expect_err("invalid guest response must fail");
    assert_eq!(invalid_response.code(), "K4001");

    let invalid_request = Runtime::default()
        .invoke_webhook(
            &pure.bytes,
            &pure.metadata,
            &GrantSet::from_manifest(&manifest("")),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            HttpRequest {
                headers: vec![HttpHeader {
                    name: "host".to_owned(),
                    value: "confused.example".to_owned(),
                }],
                ..request("")
            },
        )
        .expect_err("authority-confusing input header must fail before guest execution");
    assert_eq!(invalid_request.code(), "K4001");

    let mut limits = krit_runtime::RuntimeLimits::default();
    limits
        .narrow_header_count(0)
        .expect("header count should narrow");
    let too_many_headers = Runtime::new(limits)
        .expect("runtime should initialize")
        .invoke_webhook(
            &pure.bytes,
            &pure.metadata,
            &GrantSet::from_manifest(&manifest("")),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            HttpRequest {
                headers: vec![HttpHeader {
                    name: "x-one".to_owned(),
                    value: "one".to_owned(),
                }],
                ..request("")
            },
        )
        .expect_err("header count limit must fail before guest execution");
    assert_eq!(too_many_headers.code(), "K5103");
}

#[test]
fn secret_bytes_never_enter_artifacts_outputs_metadata_or_stats() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match secret("unit-secret") {
        Ok(value) => record { status: 204, headers: [], body: "" },
        Err(error) => record { status: 500, headers: [], body: "" },
    }
}


"#,
        &["secret.read"],
    );
    let manifest = manifest(r#"secrets = ["unit-secret"]"#);
    let secret = b"fixture-sensitive-value".to_vec();
    let missing = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("empty inputs should be valid"),
            request(""),
        )
        .expect("missing secret should be a visible Result");
    assert_eq!(missing.response.status, 500);
    let inputs = HostInputs::new(
        BTreeMap::new(),
        SecretStore::new(BTreeMap::from([("unit-secret".to_owned(), secret.clone())]))
            .expect("secret store should be valid"),
    )
    .expect("inputs should be valid");
    let result = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &inputs,
            request(""),
        )
        .expect("secret acquisition should succeed");
    assert_eq!(result.response.status, 204);
    for bytes in [
        artifact.bytes,
        serde_json::to_vec(&artifact.metadata).expect("metadata should serialize"),
        result.output,
        serde_json::to_vec(&result.stats).expect("stats should serialize"),
        format!("{inputs:?}").into_bytes(),
    ] {
        assert!(
            !bytes
                .windows(secret.len())
                .any(|window| window == secret.as_slice())
        );
    }
}

#[test]
fn failed_webhook_invocations_publish_neither_response_nor_buffered_output() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    println(7);
    record { status: 99, headers: [], body: request.body }
}
"#,
        &["io.stdout"],
    );
    let runtime = Runtime::default();
    let error = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest("stdout = true")),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid"),
            request("not-published"),
        )
        .expect_err("invalid response should roll the invocation back");
    assert_eq!(error.code(), "K4001");
    assert_eq!(runtime.active_deadline_workers(), 0);
}

#[test]
fn outbound_http_call_count_is_strictly_bounded() {
    let (origin, mock) = spawn_mock(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        Duration::ZERO,
    );
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let first = http_request("{origin}", request, None);
    match http_request("{origin}", request, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 598, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]"));
    let mut limits = krit_runtime::RuntimeLimits::default();
    limits
        .narrow_http_calls(1)
        .expect("HTTP call count should narrow");
    let error = Runtime::new(limits)
        .expect("runtime should initialize")
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default())
                .expect("inputs should be valid")
                .with_network_policy(NetworkPolicy::loopback_for_tests()),
            request(""),
        )
        .expect_err("second HTTP call must trap at the host limit");
    assert_eq!(error.code(), "K5104");
    mock.join().expect("first HTTP mock should finish");
}
