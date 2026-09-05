use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, AiAdapterConfig, ApprovalOperation, CancellationHandle,
    ExplicitApprovalPolicy, GrantSet, HostInputs, HttpHeader, HttpJsonAdapterConfig, HttpRequest,
    IdempotencyPolicy, LogLevel, NetworkPolicy, RateLimitPolicy, RetryPolicy, Runtime, SecretStore,
};
use krit_wasm::{
    AI_INTERFACE, BuildOptions, BuiltComponent, HTTP_INTERFACE, LOGGING_INTERFACE,
    SECRETS_INTERFACE, build_component,
};

fn compile(source_text: &str, capabilities: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/phase4", "1.0.0", "src/main.krit");
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
name = "test/phase4"
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

fn request(body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: "/webhook".to_owned(),
        query: String::new(),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
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

fn listener_origin(listener: &TcpListener) -> String {
    format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("listener address").port()
    )
}

#[test]
fn reference_webhook_runs_github_ai_and_messaging_with_exact_audit_facts() {
    let github = TcpListener::bind("127.0.0.1:0").expect("GitHub mock should bind");
    let ai = TcpListener::bind("127.0.0.1:0").expect("AI mock should bind");
    let messaging = TcpListener::bind("127.0.0.1:0").expect("messaging mock should bind");
    let github_origin = listener_origin(&github);
    let ai_origin = listener_origin(&ai);
    let messaging_origin = listener_origin(&messaging);
    let order = Arc::new(Mutex::new(Vec::new()));

    let github_order = Arc::clone(&order);
    let github_mock = thread::spawn(move || {
        let mut captured = Vec::new();
        for attempt in 1..=2 {
            let (mut stream, _) = github.accept().expect("GitHub mock should accept");
            let request = read_request(&mut stream);
            github_order
                .lock()
                .expect("order lock")
                .push(format!("github-{attempt}"));
            captured.push(request);
            let response = if attempt == 1 {
                b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .as_slice()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nissue details"
                    .as_slice()
            };
            stream.write_all(response).expect("GitHub response");
        }
        captured
    });

    let ai_order = Arc::clone(&order);
    let ai_mock = thread::spawn(move || {
        let (mut stream, _) = ai.accept().expect("AI mock should accept");
        let request = read_request(&mut stream);
        ai_order.lock().expect("order lock").push("ai".to_owned());
        let body = r#"{"output":"\"summary ready\""}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("AI response");
        request
    });

    let messaging_order = Arc::clone(&order);
    let messaging_mock = thread::spawn(move || {
        let (mut stream, _) = messaging.accept().expect("messaging mock should accept");
        let request = read_request(&mut stream);
        messaging_order
            .lock()
            .expect("order lock")
            .push("messaging".to_owned());
        stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("messaging response");
        request
    });

    let source = format!(
        r#"
fn failure(message: String) -> HttpResponse {{
    log_error("reference.failed", [record {{ name: "reason", value: message }}]);
    record {{ status: 502, headers: [], body: "reference flow failed" }}
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    log_info(
        "reference.received",
        [
            record {{ name: "authorization", value: "must-redact" }},
            record {{ name: "delivery", value: request.path }},
        ],
    );
    match secret("github-token") {{
        Ok(github_token) => {{
            let github_request: HttpRequest = record {{
                method: "GET",
                path: "/repos/example/issues/1",
                query: "",
                headers: [],
                body: "",
            }};
            match http_request("{github_origin}", github_request, Some(github_token)) {{
                Ok(github_response) => match ai_invoke("reviewer", github_response.body) {{
                    Ok(model_output) => {{
                        let summary: String = json_decode(model_output);
                        match secret("message-token") {{
                            Ok(message_token) => {{
                                let message_request: HttpRequest = record {{
                                    method: "POST",
                                    path: "/messages",
                                    query: "",
                                    headers: [
                                        record {{
                                            name: "idempotency-key",
                                            value: "reference-delivery",
                                        }},
                                    ],
                                    body: summary,
                                }};
                                match http_request(
                                    "{messaging_origin}",
                                    message_request,
                                    Some(message_token),
                                ) {{
                                    Ok(message_response) => {{
                                        log_info(
                                            "reference.completed",
                                            [record {{ name: "adapter", value: "reviewer" }}],
                                        );
                                        record {{
                                            status: 200,
                                            headers: message_response.headers,
                                            body: summary,
                                        }}
                                    }},
                                    Err(error) => failure(error),
                                }}
                            }},
                            Err(error) => failure(error),
                        }}
                    }},
                    Err(error) => failure(error),
                }},
                Err(error) => failure(error),
            }}
        }},
        Err(error) => failure(error),
    }}
}}
"#
    );
    let artifact = compile(
        &source,
        &["ai.invoke", "http.request", "observe.log", "secret.read"],
    );
    assert_eq!(
        artifact.metadata.imports,
        [
            AI_INTERFACE,
            HTTP_INTERFACE,
            LOGGING_INTERFACE,
            SECRETS_INTERFACE
        ]
    );
    let mut expected_approvals = [
        ("ai.invoke", "reviewer"),
        ("http.bearer", github_origin.as_str()),
        ("http.bearer", messaging_origin.as_str()),
    ];
    expected_approvals.sort_unstable();
    assert_eq!(
        artifact
            .metadata
            .approvals
            .iter()
            .map(|approval| (approval.operation.as_str(), approval.resource.as_str()))
            .collect::<Vec<_>>(),
        expected_approvals
    );

    let manifest = manifest(&format!(
        "http = [\"{github_origin}\", \"{ai_origin}\", \"{messaging_origin}\"]\n\
         secrets = [\"ai-token\", \"github-token\", \"message-token\"]\n\
         ai = [\"reviewer\"]\n\
         logs = true"
    ));
    let inputs = HostInputs::new(
        BTreeMap::new(),
        SecretStore::new(BTreeMap::from([
            ("ai-token".to_owned(), b"ai-private".to_vec()),
            ("github-token".to_owned(), b"github-private".to_vec()),
            ("message-token".to_owned(), b"message-private".to_vec()),
        ]))
        .expect("secrets should be valid"),
    )
    .expect("inputs should be valid")
    .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.ai_adapters.insert(
        "reviewer".to_owned(),
        AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
            origin: ai_origin,
            path: "/v1/invoke".to_owned(),
            model: "deterministic-test-model".to_owned(),
            secret: Some("ai-token".to_owned()),
            max_input_bytes: 1024,
            max_response_bytes: 1024,
            timeout: Duration::from_millis(500),
        }),
    );
    policy.http_retries.insert(
        github_origin.clone(),
        RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        },
    );
    policy.default_http_rate = RateLimitPolicy {
        capacity: 16,
        window: Duration::from_secs(60),
    };
    let approvals = ExplicitApprovalPolicy::new([
        (ApprovalOperation::AiInvoke, "reviewer".to_owned()),
        (ApprovalOperation::HttpBearer, github_origin.clone()),
        (ApprovalOperation::HttpBearer, messaging_origin.clone()),
    ])
    .expect("approvals should be valid");
    let host = AgentHost::new(inputs, policy, Arc::new(approvals)).expect("host should be valid");
    let runtime = Runtime::default();
    let permissions = runtime
        .permissions(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
        )
        .expect("permissions should validate");
    assert_eq!(permissions.approval_required.len(), 3);
    assert_eq!(permissions.approval_status, "not-evaluated");
    assert_eq!(permissions.deployment_grant_status, "not-evaluated");
    let result = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request("{}"),
        )
        .expect("reference flow should run");

    assert_eq!(result.response.status, 200);
    assert_eq!(result.response.body, "summary ready");
    assert_eq!(result.stats.http_calls, 2);
    assert_eq!(result.stats.ai_calls, 1);
    assert_eq!(result.stats.network_attempts, 4);
    assert_eq!(result.stats.retries, 1);
    assert_eq!(result.events.len(), 2);
    assert_eq!(result.events[0].sequence, 0);
    assert_eq!(result.events[0].level, LogLevel::Info);
    assert_eq!(result.events[0].event, "reference.received");
    assert_eq!(result.events[0].fields[0].value, "[REDACTED]");
    assert_eq!(result.events[1].event, "reference.completed");
    assert_eq!(runtime.active_deadline_workers(), 0);
    assert_eq!(runtime.active_dns_workers(), 0);
    assert_eq!(
        *order.lock().expect("order lock"),
        ["github-1", "github-2", "ai", "messaging"]
    );

    let github_requests = github_mock.join().expect("GitHub mock should finish");
    for request in github_requests {
        let request = String::from_utf8(request).expect("GitHub request should be UTF-8");
        assert!(request.starts_with("GET /repos/example/issues/1 HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer github-private\r\n")
        );
        assert!(!request.contains("ai-private"));
        assert!(!request.contains("message-private"));
    }
    let ai_request = String::from_utf8(ai_mock.join().expect("AI mock should finish"))
        .expect("AI request UTF-8");
    let ai_lower = ai_request.to_ascii_lowercase();
    assert!(ai_request.starts_with("POST /v1/invoke HTTP/1.1\r\n"));
    assert!(ai_lower.contains("authorization: bearer ai-private\r\n"));
    assert!(
        ai_request.ends_with(r#"{"model":"deterministic-test-model","input":"issue details"}"#)
    );
    assert!(!ai_request.contains("github-private"));
    let message_request =
        String::from_utf8(messaging_mock.join().expect("messaging mock should finish"))
            .expect("messaging request UTF-8");
    let message_lower = message_request.to_ascii_lowercase();
    assert!(message_request.starts_with("POST /messages HTTP/1.1\r\n"));
    assert!(message_lower.contains("authorization: bearer message-private\r\n"));
    assert!(message_lower.contains("idempotency-key: reference-delivery\r\n"));
    assert!(message_request.ends_with("summary ready"));
}

#[test]
fn approval_defaults_to_deny_and_cancellation_before_guest_is_distinct() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", request.body) {
        Ok(output) => record { status: 200, headers: [], body: output },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["ai.invoke"],
    );
    let manifest = manifest(
        r#"
http = ["https://ai.example"]
ai = ["reviewer"]
"#,
    );
    let mut policy = AgentHostPolicy::default();
    policy.ai_adapters.insert(
        "reviewer".to_owned(),
        AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
            origin: "https://ai.example".to_owned(),
            path: "/invoke".to_owned(),
            model: "test".to_owned(),
            secret: None,
            max_input_bytes: 1024,
            max_response_bytes: 1024,
            timeout: Duration::from_millis(100),
        }),
    );
    let host = AgentHost::new(
        HostInputs::new(BTreeMap::new(), SecretStore::default()).expect("inputs"),
        policy,
        Arc::new(krit_runtime::DenyAllApprovalPolicy),
    )
    .expect("host");
    let runtime = Runtime::default();
    let denied = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request("private prompt"),
        )
        .expect("approval denial should be guest-visible");
    assert_eq!(denied.response.status, 503);
    assert!(denied.response.body.contains("approval denied"));
    assert_eq!(denied.stats.network_attempts, 0);

    let cancellation = CancellationHandle::new();
    cancellation.cancel();
    let cancelled = runtime
        .invoke_webhook_with_cancellation(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            &cancellation,
            request("private prompt"),
        )
        .expect_err("pre-execution cancellation should be a runtime error");
    assert_eq!(cancelled.code(), "K5106");
}

fn anonymous_host(origin: &str, retry: RetryPolicy, rate: RateLimitPolicy) -> AgentHost {
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.http_retries.insert(origin.to_owned(), retry);
    policy.http_rates.insert(origin.to_owned(), rate);
    AgentHost::new(
        inputs,
        policy,
        Arc::new(krit_runtime::DenyAllApprovalPolicy),
    )
    .expect("host")
}

#[test]
fn retries_only_safe_or_explicitly_idempotent_requests_and_honors_attempt_cap() {
    for (name, method, headers, expected_attempts) in [
        ("unsafe", "POST", "headers: [],", 1usize),
        (
            "idempotent-post",
            "POST",
            r#"headers: [record { name: "idempotency-key", value: "outbound-one" }],"#,
            2,
        ),
        ("get", "GET", "headers: [],", 2),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
        listener
            .set_nonblocking(false)
            .expect("listener should configure");
        let origin = listener_origin(&listener);
        let mock = thread::spawn(move || {
            let mut captured = Vec::new();
            for attempt in 0..expected_attempts {
                let (mut stream, _) = listener.accept().expect("mock should accept");
                captured.push(read_request(&mut stream));
                let response = if attempt + 1 == expected_attempts && expected_attempts > 1 {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                        .as_slice()
                } else {
                    b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: invalid\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .as_slice()
                };
                stream.write_all(response).expect("mock response");
            }
            captured
        });
        let source = format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "{method}",
        path: "/retry",
        query: "",
        {headers}
        body: request.body,
    }};
    match http_request("{origin}", outbound, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 598, headers: [], body: error }},
    }}
}}
"#
        );
        let artifact = compile(&source, &["http.request"]);
        let manifest = manifest(&format!("http = [\"{origin}\"]"));
        let host = anonymous_host(
            &origin,
            RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
            RateLimitPolicy {
                capacity: 8,
                window: Duration::from_secs(60),
            },
        );
        let result = Runtime::default()
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &GrantSet::from_manifest(&manifest),
                &host,
                request("payload"),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            result.stats.network_attempts as usize, expected_attempts,
            "{name}"
        );
        assert_eq!(
            result.stats.retries as usize,
            expected_attempts.saturating_sub(1),
            "{name}"
        );
        assert_eq!(
            result.response.status,
            if expected_attempts == 1 { 503 } else { 200 },
            "{name}"
        );
        assert_eq!(
            mock.join().expect("mock should finish").len(),
            expected_attempts
        );
    }
}

#[test]
fn configured_ai_retries_reuse_one_host_generated_idempotency_key() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        let mut captured = Vec::new();
        for attempt in 0..2 {
            let (mut stream, _) = listener.accept().expect("mock should accept");
            captured.push(read_request(&mut stream));
            let response = if attempt == 0 {
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .as_slice()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"output\":\"ok\"}"
                    .as_slice()
            };
            stream.write_all(response).expect("mock response");
        }
        captured
    });
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", request.body) {
        Ok(output) => record { status: 200, headers: [], body: output },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["ai.invoke"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nai = [\"reviewer\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.ai_adapters.insert(
        "reviewer".to_owned(),
        AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
            origin,
            path: "/invoke".to_owned(),
            model: "test".to_owned(),
            secret: None,
            max_input_bytes: 1024,
            max_response_bytes: 1024,
            timeout: Duration::from_millis(500),
        }),
    );
    policy.ai_retries.insert(
        "reviewer".to_owned(),
        RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        },
    );
    let host = AgentHost::new(
        inputs,
        policy,
        Arc::new(
            ExplicitApprovalPolicy::new([(ApprovalOperation::AiInvoke, "reviewer".to_owned())])
                .expect("approval"),
        ),
    )
    .expect("host");

    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request("summarize"),
        )
        .expect("AI retry should succeed");

    assert_eq!(result.response.status, 200);
    assert_eq!(result.response.body, "ok");
    assert_eq!(result.stats.network_attempts, 2);
    assert_eq!(result.stats.retries, 1);
    let captured = mock.join().expect("mock should finish");
    let keys = captured
        .iter()
        .map(|request| {
            String::from_utf8_lossy(request)
                .lines()
                .find_map(|line| {
                    line.strip_prefix("idempotency-key: ")
                        .or_else(|| line.strip_prefix("Idempotency-Key: "))
                })
                .expect("AI request should contain an idempotency key")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1]);
    assert!(keys[0].starts_with("krit-ai-"));
}

#[test]
fn rate_limits_are_per_resource_and_tracking_is_bounded() {
    let first = TcpListener::bind("127.0.0.1:0").expect("first mock");
    let second = TcpListener::bind("127.0.0.1:0").expect("second mock");
    let first_origin = listener_origin(&first);
    let second_origin = listener_origin(&second);
    let first_mock = thread::spawn(move || {
        let (mut stream, _) = first.accept().expect("first accept");
        let _ = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("first response");
    });
    let second_mock = thread::spawn(move || {
        let (mut stream, _) = second.accept().expect("second accept");
        let _ = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("second response");
    });
    let source = |origin: &str| {
        format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match http_request("{origin}", request, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 429, headers: [], body: error }},
    }}
}}
"#
        )
    };
    let first_artifact = compile(&source(&first_origin), &["http.request"]);
    let second_artifact = compile(&source(&second_origin), &["http.request"]);
    let first_manifest = manifest(&format!("http = [\"{first_origin}\"]"));
    let second_manifest = manifest(&format!("http = [\"{second_origin}\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let policy = AgentHostPolicy {
        default_http_rate: RateLimitPolicy {
            capacity: 1,
            window: Duration::from_secs(60),
        },
        max_tracked_resources: 1,
        ..AgentHostPolicy::default()
    };
    let host = AgentHost::new(
        inputs,
        policy,
        Arc::new(krit_runtime::DenyAllApprovalPolicy),
    )
    .expect("host");
    let runtime = Runtime::default();
    let first_result = runtime
        .invoke_webhook_with_host(
            &first_artifact.bytes,
            &first_artifact.metadata,
            &GrantSet::from_manifest(&first_manifest),
            &host,
            request(""),
        )
        .expect("first resource should run");
    let second_result = runtime
        .invoke_webhook_with_host(
            &second_artifact.bytes,
            &second_artifact.metadata,
            &GrantSet::from_manifest(&second_manifest),
            &host,
            request(""),
        )
        .expect("second resource should replace the first tracked entry");
    assert_eq!(first_result.response.status, 200);
    assert_eq!(second_result.response.status, 200);
    assert_eq!(first_result.stats.network_attempts, 1);
    assert_eq!(second_result.stats.network_attempts, 1);
    assert_eq!(host.tracked_rate_resource_count(), 1);
    first_mock.join().expect("first mock");
    second_mock.join().expect("second mock");
}

#[test]
fn inbound_idempotency_replays_conflicts_and_uses_bounded_lru() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        let mut count = 0usize;
        while count < 3 {
            let (mut stream, _) = listener.accept().expect("mock should accept");
            let _ = read_request(&mut stream);
            count += 1;
            let body = count.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("mock response");
        }
        count
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "GET",
        path: "/effect",
        query: "",
        headers: [],
        body: "",
    }};
    match http_request("{origin}", outbound, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 598, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let policy = AgentHostPolicy {
        idempotency: IdempotencyPolicy {
            max_entries: 1,
            max_bytes: 1024,
            ttl: Duration::from_secs(60),
            max_key_bytes: 32,
        },
        ..AgentHostPolicy::default()
    };
    let host = AgentHost::new(
        inputs,
        policy,
        Arc::new(krit_runtime::DenyAllApprovalPolicy),
    )
    .expect("host");
    let runtime = Runtime::default();
    let grants = GrantSet::from_manifest(&manifest);
    let keyed = |key: &str, body: &str| HttpRequest {
        headers: vec![HttpHeader {
            name: "Idempotency-Key".to_owned(),
            value: key.to_owned(),
        }],
        ..request(body)
    };

    let first = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            keyed("one", "same"),
        )
        .expect("first request");
    let replay = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            keyed("one", "same"),
        )
        .expect("replay");
    assert_eq!(first.response, replay.response);
    assert!(replay.stats.idempotency_replayed);
    assert_eq!(replay.stats.network_attempts, 0);

    let credential_conflict = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            HttpRequest {
                headers: vec![
                    HttpHeader {
                        name: "Idempotency-Key".to_owned(),
                        value: "one".to_owned(),
                    },
                    HttpHeader {
                        name: "Cookie".to_owned(),
                        value: "session=other-caller".to_owned(),
                    },
                ],
                ..request("same")
            },
        )
        .expect("credential change is an idempotency conflict");
    assert_eq!(credential_conflict.response.status, 409);
    assert!(!credential_conflict.stats.idempotency_replayed);
    assert_eq!(credential_conflict.stats.network_attempts, 0);

    let conflict = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            keyed("one", "different"),
        )
        .expect("conflict is an HTTP response");
    assert_eq!(conflict.response.status, 409);
    assert!(!conflict.stats.idempotency_replayed);

    for invalid in [
        HttpRequest {
            headers: vec![HttpHeader {
                name: "idempotency-key".to_owned(),
                value: "contains space".to_owned(),
            }],
            ..request("same")
        },
        HttpRequest {
            headers: vec![
                HttpHeader {
                    name: "idempotency-key".to_owned(),
                    value: "one".to_owned(),
                },
                HttpHeader {
                    name: "Idempotency-Key".to_owned(),
                    value: "one".to_owned(),
                },
            ],
            ..request("same")
        },
    ] {
        let rejected = runtime
            .invoke_webhook_with_host(&artifact.bytes, &artifact.metadata, &grants, &host, invalid)
            .expect("invalid idempotency keys should return HTTP 400");
        assert_eq!(rejected.response.status, 400);
        assert!(!rejected.stats.idempotency_replayed);
        assert_eq!(rejected.stats.network_attempts, 0);
    }

    let second_key = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            keyed("two", "second"),
        )
        .expect("second key");
    assert_eq!(second_key.response.body, "2");
    let evicted = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            keyed("one", "same"),
        )
        .expect("evicted key should execute again");
    assert_eq!(evicted.response.body, "3");
    assert_eq!(host.idempotency_entry_count(), 1);
    assert!(host.idempotency_cached_bytes() <= 1024);
    assert_eq!(mock.join().expect("mock should finish"), 3);
}

#[test]
fn inbound_idempotency_cache_evicts_to_its_byte_budget() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.body }
}
"#,
        &[],
    );
    let manifest = manifest("");
    let policy = AgentHostPolicy {
        idempotency: IdempotencyPolicy {
            max_entries: 10,
            max_bytes: 256,
            ttl: Duration::from_secs(60),
            max_key_bytes: 32,
        },
        ..AgentHostPolicy::default()
    };
    let host = AgentHost::new(
        HostInputs::default(),
        policy,
        Arc::new(krit_runtime::DenyAllApprovalPolicy),
    )
    .expect("host");
    let runtime = Runtime::default();
    let grants = GrantSet::from_manifest(&manifest);
    let keyed = |key: &str, body: String| HttpRequest {
        headers: vec![HttpHeader {
            name: "Idempotency-Key".to_owned(),
            value: key.to_owned(),
        }],
        ..request(&body)
    };

    for (key, value) in [("one", "a".repeat(100)), ("two", "b".repeat(100))] {
        runtime
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                keyed(key, value),
            )
            .expect("request should execute");
    }

    assert_eq!(host.idempotency_entry_count(), 1);
    assert!(host.idempotency_cached_bytes() <= 256);
    let evicted = runtime
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            keyed("one", "a".repeat(100)),
        )
        .expect("evicted request should execute");
    assert!(!evicted.stats.idempotency_replayed);
}

#[test]
fn failed_invocations_are_not_cached_and_keep_only_redacted_validated_logs() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("mock should accept");
            let _ = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("mock response");
        }
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    log_info(
        "before.failure",
        [record {{ name: "authorization", value: request.body }}],
    );
    let side_effect = http_request("{origin}", request, None);
    record {{ status: 99, headers: [], body: "not published" }}
}}
"#
        ),
        &["http.request", "observe.log"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nlogs = true"));
    let host = anonymous_host(
        &origin,
        RetryPolicy::default(),
        RateLimitPolicy {
            capacity: 8,
            window: Duration::from_secs(60),
        },
    );
    let runtime = Runtime::default();
    let grants = GrantSet::from_manifest(&manifest);
    for _ in 0..2 {
        let error = runtime
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                HttpRequest {
                    headers: vec![HttpHeader {
                        name: "idempotency-key".to_owned(),
                        value: "failed-key".to_owned(),
                    }],
                    ..request("private prompt")
                },
            )
            .expect_err("invalid response should fail");
        assert_eq!(error.events().len(), 1);
        assert_eq!(error.events()[0].fields[0].value, "[REDACTED]");
        assert!(!format!("{error:?}").contains("private prompt"));
    }
    assert_eq!(host.idempotency_entry_count(), 0);
    mock.join().expect("mock should finish");
}

#[test]
fn logging_validation_is_atomic_and_field_order_is_preserved() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match log_info(
        "validation.event",
        [
            record { name: "first", value: "one" },
            record { name: request.body, value: "two" },
        ],
    ) {
        Ok(unit) => record { status: 200, headers: [], body: "unexpected" },
        Err(error) => record { status: 400, headers: [], body: error },
    }
}
"#,
        &["observe.log"],
    );
    let manifest = manifest("logs = true");
    let result = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default()).expect("inputs"),
            request("INVALID"),
        )
        .expect("log validation should be guest-visible");
    assert_eq!(result.response.status, 400);
    assert!(result.events.is_empty());

    let valid = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &HostInputs::new(BTreeMap::new(), SecretStore::default()).expect("inputs"),
            request("second"),
        )
        .expect("valid log should run");
    assert_eq!(
        valid.events[0]
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

struct FirstAttemptOnlyApproval {
    calls: AtomicUsize,
}

impl krit_runtime::ApprovalPolicy for FirstAttemptOnlyApproval {
    fn approve(&self, _request: &krit_runtime::ApprovalRequest) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst) == 0
    }
}

#[test]
fn approval_is_rechecked_before_retry_and_cancellation_interrupts_backoff() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let (seen_sender, seen_receiver) = mpsc::sync_channel(1);
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept");
        let _ = read_request(&mut stream);
        seen_sender.send(()).expect("signal");
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("response");
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match secret("token") {{
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
    let manifest = manifest(&format!("http = [\"{origin}\"]\nsecrets = [\"token\"]"));
    let inputs = HostInputs::new(
        BTreeMap::new(),
        SecretStore::new(BTreeMap::from([("token".to_owned(), b"value".to_vec())]))
            .expect("secret"),
    )
    .expect("inputs")
    .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.http_retries.insert(
        origin.clone(),
        RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(2),
        },
    );
    let approval = Arc::new(FirstAttemptOnlyApproval {
        calls: AtomicUsize::new(0),
    });
    let host = AgentHost::new(inputs, policy, approval.clone()).expect("host");
    let cancellation = CancellationHandle::new();
    let runtime = Runtime::new(krit_runtime::HARD_MAX_LIMITS).expect("runtime");
    let grants = GrantSet::from_manifest(&manifest);
    let mut retryable = request("");
    retryable.method = "GET".to_owned();
    let result = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            runtime.invoke_webhook_with_cancellation(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                &cancellation,
                retryable,
            )
        });
        seen_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("first attempt");
        cancellation.cancel();
        worker.join().expect("invocation thread")
    })
    .expect("backoff cancellation should be guest-visible");
    assert_eq!(result.response.status, 598);
    assert!(result.response.body.contains("cancelled"));
    assert_eq!(result.stats.network_attempts, 1);
    assert_eq!(runtime.active_deadline_workers(), 0);
    assert_eq!(runtime.active_dns_workers(), 0);
    assert_eq!(approval.calls.load(Ordering::SeqCst), 1);
    mock.join().expect("mock");
}

#[test]
fn rate_exhaustion_is_visible_without_a_second_network_attempt() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept");
        let _ = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("mock response");
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let first = http_request("{origin}", request, None);
    match http_request("{origin}", request, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 429, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]"));
    let host = anonymous_host(
        &origin,
        RetryPolicy::default(),
        RateLimitPolicy {
            capacity: 1,
            window: Duration::from_secs(60),
        },
    );
    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request(""),
        )
        .expect("rate denial should be guest-visible");
    assert_eq!(result.response.status, 429);
    assert!(result.response.body.contains("rate limit exceeded"));
    assert_eq!(result.stats.network_attempts, 1);
    assert_eq!(result.stats.rate_limit_denials, 1);
    mock.join().expect("mock");
}

fn ai_test_host(origin: String, max_response_bytes: usize, timeout: Duration) -> AgentHost {
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.ai_adapters.insert(
        "reviewer".to_owned(),
        AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
            origin,
            path: "/invoke".to_owned(),
            model: "test".to_owned(),
            secret: None,
            max_input_bytes: 1024,
            max_response_bytes,
            timeout,
        }),
    );
    AgentHost::new(
        inputs,
        policy,
        Arc::new(
            ExplicitApprovalPolicy::new([(ApprovalOperation::AiInvoke, "reviewer".to_owned())])
                .expect("approval"),
        ),
    )
    .expect("host")
}

#[test]
fn ai_errors_are_bounded_redacted_and_model_text_is_never_executed() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", request.body) {
        Ok(output) => record { status: 200, headers: [], body: output },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["ai.invoke"],
    );
    let prompt = "private prompt that must not leak";

    for (case, response, max_response, delay) in [
        (
            "malformed",
            b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nnot-json"
                .as_slice(),
            1024,
            Duration::ZERO,
        ),
        (
            "oversized",
            b"HTTP/1.1 200 OK\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{\"output\":\"12345\"}"
                .as_slice(),
            8,
            Duration::ZERO,
        ),
        (
            "provider",
            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 13\r\nConnection: close\r\n\r\nprivate-error"
                .as_slice(),
            1024,
            Duration::ZERO,
        ),
        (
            "timeout",
            b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"output\":\"x\"}"
                .as_slice(),
            1024,
            Duration::from_millis(100),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock");
        let origin = listener_origin(&listener);
        let bytes = response.to_vec();
        let mock = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = read_request(&mut stream);
            thread::sleep(delay);
            let _ = stream.write_all(&bytes);
        });
        let manifest = manifest(&format!(
            "http = [\"{origin}\"]\nai = [\"reviewer\"]"
        ));
        let timeout = if case == "timeout" {
            Duration::from_millis(20)
        } else {
            Duration::from_millis(500)
        };
        let host = ai_test_host(origin, max_response, timeout);
        let result = Runtime::default()
            .invoke_webhook_with_host(
                &artifact.bytes,
                &artifact.metadata,
                &GrantSet::from_manifest(&manifest),
                &host,
                request(prompt),
            )
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(result.response.status, 503, "{case}");
        assert!(!result.response.body.contains(prompt), "{case}");
        assert!(
            !serde_json::to_string(&result.stats)
                .expect("stats")
                .contains(prompt),
            "{case}"
        );
        assert!(!format!("{host:?}").contains(prompt), "{case}");
        mock.join().expect("mock");
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("mock");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_request(&mut stream);
        let body = r#"{"output":"println(999);"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("response");
    });
    let manifest = manifest(&format!("http = [\"{origin}\"]\nai = [\"reviewer\"]"));
    let host = ai_test_host(origin, 1024, Duration::from_millis(500));
    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request(prompt),
        )
        .expect("model text should be returned as data");
    assert_eq!(result.response.body, "println(999);");
    assert!(result.output.is_empty());
    mock.join().expect("mock");
}

#[test]
fn cancellation_aborts_an_active_transfer() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let (started_sender, started_receiver) = mpsc::sync_channel(1);
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept");
        let _ = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nx")
            .expect("partial response");
        started_sender.send(()).expect("signal");
        for _ in 0..99 {
            thread::sleep(Duration::from_millis(20));
            if stream.write_all(b"x").is_err() {
                break;
            }
        }
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
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
    let host = anonymous_host(
        &origin,
        RetryPolicy::default(),
        RateLimitPolicy {
            capacity: 8,
            window: Duration::from_secs(60),
        },
    );
    let cancellation = CancellationHandle::new();
    let runtime = Runtime::new(krit_runtime::HARD_MAX_LIMITS).expect("runtime");
    let grants = GrantSet::from_manifest(&manifest);
    let started = std::time::Instant::now();
    let result = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            runtime.invoke_webhook_with_cancellation(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &host,
                &cancellation,
                request(""),
            )
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("transfer should start");
        cancellation.cancel();
        worker.join().expect("worker")
    })
    .expect("transfer cancellation should be guest-visible");
    assert_eq!(result.response.status, 598);
    assert!(result.response.body.contains("cancelled"));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(runtime.active_deadline_workers(), 0);
    mock.join().expect("mock");
}

#[test]
fn approval_denial_on_a_retry_prevents_the_second_network_attempt() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept");
        let _ = read_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("response");
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match secret("token") {{
        Ok(token) => {{
            let outbound: HttpRequest = record {{
                method: "GET",
                path: "/retry",
                query: "",
                headers: [],
                body: "",
            }};
            match http_request("{origin}", outbound, Some(token)) {{
                Ok(response) => response,
                Err(error) => record {{ status: 598, headers: [], body: error }},
            }}
        }},
        Err(error) => record {{ status: 500, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request", "secret.read"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]\nsecrets = [\"token\"]"));
    let inputs = HostInputs::new(
        BTreeMap::new(),
        SecretStore::new(BTreeMap::from([("token".to_owned(), b"value".to_vec())]))
            .expect("secret"),
    )
    .expect("inputs")
    .with_network_policy(NetworkPolicy::loopback_for_tests().with_plaintext_bearer_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.http_retries.insert(
        origin,
        RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        },
    );
    let approval = Arc::new(FirstAttemptOnlyApproval {
        calls: AtomicUsize::new(0),
    });
    let host = AgentHost::new(inputs, policy, approval.clone()).expect("host");
    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request(""),
        )
        .expect("retry approval denial should be guest-visible");
    assert_eq!(result.response.status, 598);
    assert!(result.response.body.contains("approval denied"));
    assert_eq!(result.stats.network_attempts, 1);
    assert_eq!(approval.calls.load(Ordering::SeqCst), 2);
    mock.join().expect("mock");
}

#[test]
fn idempotency_entries_expire_after_the_bounded_ttl() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        for number in 1..=2 {
            let (mut stream, _) = listener.accept().expect("mock should accept");
            let _ = read_request(&mut stream);
            let body = number.to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{body}"
            )
            .expect("mock response");
        }
    });
    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let outbound: HttpRequest = record {{
        method: "GET",
        path: "/ttl",
        query: "",
        headers: [],
        body: "",
    }};
    match http_request("{origin}", outbound, None) {{
        Ok(response) => response,
        Err(error) => record {{ status: 598, headers: [], body: error }},
    }}
}}
"#
        ),
        &["http.request"],
    );
    let manifest = manifest(&format!("http = [\"{origin}\"]"));
    let inputs = HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("inputs")
        .with_network_policy(NetworkPolicy::loopback_for_tests());
    let policy = AgentHostPolicy {
        idempotency: IdempotencyPolicy {
            max_entries: 2,
            max_bytes: 1024,
            ttl: Duration::from_millis(10),
            max_key_bytes: 32,
        },
        ..AgentHostPolicy::default()
    };
    let host = AgentHost::new(
        inputs,
        policy,
        Arc::new(krit_runtime::DenyAllApprovalPolicy),
    )
    .expect("host");
    let runtime = Runtime::default();
    let grants = GrantSet::from_manifest(&manifest);
    let keyed = || HttpRequest {
        headers: vec![HttpHeader {
            name: "idempotency-key".to_owned(),
            value: "ttl-key".to_owned(),
        }],
        ..request("same")
    };
    let first = runtime
        .invoke_webhook_with_host(&artifact.bytes, &artifact.metadata, &grants, &host, keyed())
        .expect("first");
    thread::sleep(Duration::from_millis(30));
    let second = runtime
        .invoke_webhook_with_host(&artifact.bytes, &artifact.metadata, &grants, &host, keyed())
        .expect("expired entry should execute");
    assert_eq!(first.response.body, "1");
    assert_eq!(second.response.body, "2");
    assert!(!second.stats.idempotency_replayed);
    assert_eq!(host.idempotency_entry_count(), 1);
    assert_eq!(runtime.active_dns_workers(), 0);
    mock.join().expect("mock");
}

#[test]
fn malformed_model_output_fails_closed_in_explicit_source_parsing() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = listener_origin(&listener);
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("mock should accept");
        let _ = read_request(&mut stream);
        let body = r#"{"output":"not-json"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("mock response");
    });
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    log_info("model.received", []);
    match ai_invoke("reviewer", request.body) {
        Ok(output) => {
            let parsed: String = json_decode(output);
            record { status: 200, headers: [], body: parsed }
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["ai.invoke", "observe.log"],
    );
    let manifest = manifest(&format!(
        "http = [\"{origin}\"]\nai = [\"reviewer\"]\nlogs = true"
    ));
    let host = ai_test_host(origin, 1024, Duration::from_millis(500));
    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest),
            &host,
            request("private prompt"),
        )
        .expect_err("malformed model JSON string must trap");
    assert_eq!(error.code(), "K4001");
    assert_eq!(error.events().len(), 1);
    assert!(!format!("{error:?}").contains("private prompt"));
    assert!(!format!("{error:?}").contains("not-json"));
    mock.join().expect("mock");
}
