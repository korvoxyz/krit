//! Entry lists and the unit value build and run.
//!
//! Webhook components write their response headers, outbound request
//! headers, and log fields as entry lists (`["x-path": request.path]`), and
//! their helpers return `Ok(())`. The component must send and return exactly
//! the records the entry lists describe, evaluate keys and values in source
//! order, and agree with the direct evaluator on the same handler.

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
    let program = parse_source(&source).expect("entry-list source should parse");
    let analysis = analyze(&program).expect("entry-list source should analyze");
    let module = lower(&program, &analysis).expect("entry-list source should lower");
    let mut options = BuildOptions::new("2026", "test/entry-lists", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("entry-list webhook should compile")
}

fn grants(capabilities: &str) -> GrantSet {
    GrantSet::from_manifest(
        &Manifest::parse(&format!(
            r#"
schema = 1

[package]
name = "test/entry-lists"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
        ))
        .expect("entry-list manifest should parse"),
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

fn header(name: &str, value: &str) -> HttpHeader {
    HttpHeader {
        name: name.to_owned(),
        value: value.to_owned(),
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
/// Accepts every path but `/`.
fn validate(path: String) -> Result<Unit, String> {
    if path == "/" { Err("root path") } else { Ok(()) }
}

/// Echoes the request in headers.
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match validate(request.path) {
        Ok(done) => HttpResponse {
            status: 200,
            headers: ["x-path": request.path, "x-" + "method": request.method,],
            body: if done == () { "ok" } else { "unreachable" },
        },
        Err(reason) => HttpResponse { status: 400, headers: ["x-reason": reason], body: reason },
    }
}
"#;

#[test]
fn entry_list_responses_build_and_agree_with_the_evaluator() {
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
            .expect("entry-list webhook should run")
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
        .expect("entry-list webhook should run")
        .response;
    assert_eq!(
        response,
        HttpResponse {
            status: 200,
            headers: vec![header("x-path", "/hello"), header("x-method", "GET")],
            body: "ok".to_owned(),
        }
    );
}

#[test]
fn entry_list_keys_and_values_evaluate_in_source_order() {
    let artifact = compile(
        r#"
fn first() -> String {
    let logged = log_info("k1", []);
    "a"
}

fn second() -> String {
    let logged = log_info("v1", []);
    "1"
}

fn third() -> String {
    let logged = log_info("k2", []);
    "b"
}

fn fourth() -> String {
    let logged = log_info("v2", []);
    "2"
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    HttpResponse { status: 200, headers: ["x-" + first(): second(), third(): fourth()] }
}
"#,
        &["observe.log"],
    );
    let result = Runtime::default()
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants("logs = true"),
            &inputs(),
            request("GET", "/"),
        )
        .expect("ordered entry-list webhook should run");
    assert_eq!(
        result
            .events
            .iter()
            .map(|event| event.event.as_str())
            .collect::<Vec<_>>(),
        ["k1", "v1", "k2", "v2"]
    );
    assert_eq!(
        result.response.headers,
        [header("x-a", "1"), header("b", "2")]
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
fn entry_list_request_headers_and_log_fields_reach_the_host() {
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
    let fetch = HttpRequest {{
        method: "GET",
        path: "/x",
        headers: ["accept": "text/plain", "x-inbound-path": request.path],
    }};
    match http_request("{origin}", fetch, None) {{
        Ok(response) => {{
            let logged = log_info("fetch.done", ["body": response.body, "path": request.path]);
            HttpResponse {{ status: 200, body: response.body }}
        }},
        Err(error) => {{
            let logged = log_error("fetch.failed", ["error": error]);
            HttpResponse {{ status: 502, body: error }}
        }},
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
        .expect("entry-list request should run");
    assert_eq!(
        result.response,
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: "fetched".to_owned(),
        }
    );
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].event, "fetch.done");
    let fields = result.events[0]
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.value.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(fields, [("body", "fetched"), ("path", "/inbound")]);

    let captured = mock.join().expect("mock should finish");
    assert!(
        captured.starts_with("GET /x HTTP/1.1\r\n"),
        "unexpected request: {captured}"
    );
    let headers = captured
        .split_once("\r\n\r\n")
        .map(|(headers, _)| headers.to_ascii_lowercase())
        .expect("request should have a header terminator");
    assert!(
        headers.contains("\r\naccept: text/plain"),
        "unexpected request: {captured}"
    );
    assert!(
        headers.contains("\r\nx-inbound-path: /inbound"),
        "unexpected request: {captured}"
    );
}
