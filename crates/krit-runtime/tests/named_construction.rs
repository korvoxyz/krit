//! Named construction of the built-in contract records builds and runs.
//!
//! Webhook components build their outbound requests with
//! `HttpRequest { method, path }`, whose omitted query, headers, and body take
//! their spec-fixed defaults, their headers and log fields with
//! `HttpHeader { ... }` and `LogField { ... }`, and their responses with
//! `HttpResponse { status, body }`. The component must send exactly the
//! request the construction describes and return what the direct evaluator
//! computes for the same handler.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source, run_source};
use krit_package::Manifest;
use krit_runtime::{
    GrantSet, HostInputs, HttpHeader, HttpRequest, HttpResponse, NetworkPolicy, Runtime,
    SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

fn compile(source_text: &str, effects: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("construction source should parse");
    let analysis = analyze(&program).expect("construction source should analyze");
    let module = lower(&program, &analysis).expect("construction source should lower");
    let mut options = BuildOptions::new("2026", "test/construction", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("construction webhook should compile")
}

fn grants(capabilities: &str) -> GrantSet {
    GrantSet::from_manifest(
        &Manifest::parse(&format!(
            r#"
schema = 1

[package]
name = "test/construction"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
        ))
        .expect("construction manifest should parse"),
    )
}

fn inputs() -> HostInputs {
    HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid")
}

fn request(method: &str, path: &str) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        query: String::new(),
        headers: Vec::new(),
        body: String::new(),
    }
}

/// Runs `handle(request)` in the direct evaluator and returns the printed
/// response record.
fn evaluate(source_text: &str, method: &str, path: &str) -> String {
    let program = format!(
        "{source_text}\nprint(handle(HttpRequest {{ method: {method:?}, path: {path:?} }}));\n"
    );
    let mut output = Vec::new();
    run_source(&Source::new("main.krit", program), &mut output)
        .expect("the evaluator should run the handler");
    String::from_utf8(output).expect("evaluator output should be UTF-8")
}

/// Renders a component response as the evaluator prints the same record.
fn render(response: &HttpResponse) -> String {
    let headers = response
        .headers
        .iter()
        .map(|header| {
            format!(
                "record {{ name: {:?}, value: {:?} }}",
                header.name, header.value
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "record {{ status: {}, headers: [{headers}], body: {:?} }}",
        response.status, response.body
    )
}

const ROUTING: &str = r#"
fn route(request: HttpRequest) -> HttpResponse {
    if request.method == "POST" {
        HttpResponse { status: 201, body: "created " + request.path }
    } else {
        let accept = HttpHeader { name: "x-method", value: request.method };
        HttpResponse { status: 200, headers: [accept], body: request.path }
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    route(request)
}
"#;

#[test]
fn named_responses_build_and_agree_with_the_evaluator() {
    let artifact = compile(ROUTING, &[]);
    let runtime = Runtime::default();
    for (method, path) in [("GET", "/hello"), ("POST", "/items"), ("DELETE", "/")] {
        let response = runtime
            .invoke_webhook(
                &artifact.bytes,
                &artifact.metadata,
                &grants(""),
                &inputs(),
                request(method, path),
            )
            .expect("named-construction webhook should run")
            .response;
        assert_eq!(
            render(&response),
            evaluate(ROUTING, method, path),
            "{method} {path}"
        );
    }

    let response = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants(""),
            &inputs(),
            request("GET", "/hello"),
        )
        .expect("named-construction webhook should run")
        .response;
    assert_eq!(
        response,
        HttpResponse {
            status: 200,
            headers: vec![HttpHeader {
                name: "x-method".to_owned(),
                value: "GET".to_owned(),
            }],
            body: "/hello".to_owned(),
        }
    );
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
    String::from_utf8(bytes).expect("request should be UTF-8")
}

#[test]
fn a_named_request_sends_its_defaults_and_a_named_response_returns() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("mock address").port()
    );
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request should arrive");
        let captured = read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfetched")
            .expect("mock response should write");
        captured
    });

    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    match http_request("{origin}", HttpRequest {{ method: "GET", path: "/x" }}, None) {{
        Ok(response) => {{
            let logged = log_info(
                "fetch.done",
                [LogField {{ name: "body", value: response.body }}],
            );
            HttpResponse {{ status: 200, body: request.path }}
        }},
        Err(error) => HttpResponse {{ status: 502, body: error }},
    }}
}}
"#
        ),
        &["http.request", "observe.log"],
    );
    let result = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants(&format!("http = [\"{origin}\"]\nlogs = true")),
            &inputs().with_network_policy(NetworkPolicy::loopback_for_tests()),
            request("POST", "/inbound"),
        )
        .expect("named-construction request should run");
    assert_eq!(
        result.response,
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: "/inbound".to_owned(),
        }
    );
    assert_eq!(result.stats.http_calls, 1);
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].event, "fetch.done");
    assert_eq!(result.events[0].fields[0].name, "body");
    assert_eq!(result.events[0].fields[0].value, "fetched");

    // The omitted query, headers, and body went out as their defaults: no
    // query string and an empty body.
    let captured = mock.join().expect("mock should finish");
    assert!(
        captured.starts_with("GET /x HTTP/1.1\r\n"),
        "unexpected request: {captured}"
    );
    let (_, body) = captured
        .split_once("\r\n\r\n")
        .expect("request should have a header terminator");
    assert!(body.is_empty(), "unexpected body: {body:?}");
}
