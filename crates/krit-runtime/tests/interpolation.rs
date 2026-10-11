//! Interpolated strings and text blocks build and run.
//!
//! A webhook composes its model prompt from a text block with holes and
//! sends it through `ai_invoke`; the model must receive exactly the text the
//! direct evaluator produces. String composition with holes, adjacent holes,
//! escapes beside holes, and empty parts must agree with the direct
//! evaluator on every request.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};

use krit::{Source, analyze, lower, parse_source, run_source};
use krit_package::Manifest;
use krit_runtime::{
    AgentHost, AgentHostPolicy, AiAdapterConfig, ApprovalOperation, ExplicitApprovalPolicy,
    GrantSet, HostInputs, HttpJsonAdapterConfig, HttpRequest, HttpResponse, NetworkPolicy, Runtime,
    SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

fn compile(source_text: &str, effects: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("interpolation source should parse");
    let analysis = analyze(&program).expect("interpolation source should analyze");
    let module = lower(&program, &analysis).expect("interpolation source should lower");
    let mut options = BuildOptions::new("2026", "test/interpolation", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("interpolation webhook should compile")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/interpolation"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
    ))
    .expect("interpolation manifest should parse")
}

fn inputs() -> HostInputs {
    HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid")
}

fn request(method: &str, path: &str, query: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

/// Renders `text` as a Krit string literal.
fn literal(text: &str) -> String {
    let mut rendered = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => rendered.push_str("\\\""),
            '\\' => rendered.push_str("\\\\"),
            character if character.is_control() => {
                rendered.push_str(&format!("\\u{{{:x}}}", u32::from(character)));
            }
            character => rendered.push(character),
        }
    }
    rendered.push('"');
    rendered
}

/// Runs `expression` in the direct evaluator with `request` bound, and
/// returns what it prints.
fn evaluate(source_text: &str, expression: &str, request: &HttpRequest) -> String {
    let program = format!(
        "{source_text}\nlet request = HttpRequest {{ method: {}, path: {}, query: {}, body: {} }};\n\
         print({expression});\n",
        literal(&request.method),
        literal(&request.path),
        literal(&request.query),
        literal(&request.body),
    );
    let mut output = Vec::new();
    run_source(&Source::new("main.krit", program), &mut output)
        .expect("the evaluator should run the expression");
    String::from_utf8(output).expect("evaluator output should be UTF-8")
}

const COMPOSE: &str = r#"
/// Every hole shape: a field path, adjacent holes, escapes beside holes, a
/// hole that is the whole string, and empty text around holes.
fn compose(request: HttpRequest) -> String {
    let method = request.method;
    let route = "\(method) \(request.path)";
    let pair = "\(request.query)\(request.body)";
    let quoted = "\"\(request.path)\"\t\\\(method)\\(literal)";
    let whole = "\(request.body)";
    route + "|" + pair + "|" + quoted + "|" + whole + "|" + """
        Route: \(route)
          Body: [\(request.body)]

        \u{2705} \(method)
        """
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    HttpResponse { status: 200, body: compose(request) }
}
"#;

#[test]
fn interpolation_agrees_with_the_evaluator_through_invoke() {
    let artifact = compile(COMPOSE, &[]);
    let runtime = Runtime::default();
    for request in [
        request("POST", "/issues/7", "page=2", "It crashes.\nOn save."),
        request("GET", "/", "", ""),
        request(
            "PUT",
            "/caf\u{e9}/\u{1f600}",
            "q=\"x\"",
            "tab\there \\ done",
        ),
        request("DELETE", "/long", "", &"x".repeat(5_000)),
    ] {
        let response = runtime
            .invoke_webhook(
                &artifact.bytes,
                &artifact.metadata,
                &GrantSet::from_manifest(&manifest("")),
                &inputs(),
                request.clone(),
            )
            .expect("interpolation webhook should run")
            .response;
        assert_eq!(response.status, 200);
        assert_eq!(
            response.body,
            evaluate(COMPOSE, "compose(request)", &request),
            "{} {}",
            request.method,
            request.path
        );
    }

    let response = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest("")),
            &inputs(),
            request("POST", "/a", "k=v", "B"),
        )
        .expect("interpolation webhook should run")
        .response;
    assert_eq!(
        response,
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: "POST /a|k=vB|\"/a\"\t\\POST\\(literal)|B|Route: POST /a\n  Body: [B]\n\n\u{2705} POST"
                .to_owned(),
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

fn respond(stream: &mut TcpStream, body: &str) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("mock response should write");
}

const PROMPT: &str = r#"
/// The reviewer's prompt: instructions, then the untrusted request text.
fn prompt(request: HttpRequest) -> String {
    let route = "\(request.method) \(request.path)";
    """
    Classify the issue below as "bug", "feature", or "question".
    Answer with the label only.

    Route: \(route)
    Issue:
      \(request.body)
    """
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", prompt(request)) {
        Ok(label) => HttpResponse { status: 200, body: "label: \(label)" },
        Err(reason) => HttpResponse { status: 502, body: "review failed: \(reason)" },
    }
}
"#;

#[test]
fn prompt_webhook_sends_a_composed_text_block_to_the_model() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("mock address").port()
    );
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("AI request should arrive");
        let ai_request = read_request(&mut stream);
        respond(&mut stream, r#"{"output":"bug"}"#);
        ai_request
    });

    let artifact = compile(PROMPT, &["ai.invoke"]);
    let inputs = inputs().with_network_policy(NetworkPolicy::loopback_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.ai_adapters.insert(
        "reviewer".to_owned(),
        AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
            origin: origin.clone(),
            path: "/v1/invoke".to_owned(),
            model: "test-model".to_owned(),
            secret: None,
            max_input_bytes: 4096,
            max_response_bytes: 4096,
            timeout: Duration::from_millis(500),
        }),
    );
    let approvals =
        ExplicitApprovalPolicy::new([(ApprovalOperation::AiInvoke, "reviewer".to_owned())])
            .expect("approvals should be valid");
    let host = AgentHost::new(inputs, policy, Arc::new(approvals)).expect("host should be valid");
    // The adapter's origin is granted too, as every AI adapter's must be.
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "http = [\"{origin}\"]\nai = [\"reviewer\"]"
    )));
    let incoming = request("POST", "/issues/42", "", "Saving \"a.txt\" crashes.");
    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            incoming.clone(),
        )
        .expect("the prompt webhook should run");
    assert_eq!(result.response.status, 200);
    assert_eq!(result.response.body, "label: bug");
    assert_eq!(result.stats.ai_calls, 1);

    let expected = "Classify the issue below as \"bug\", \"feature\", or \"question\".\n\
                    Answer with the label only.\n\
                    \n\
                    Route: POST /issues/42\n\
                    Issue:\n  Saving \"a.txt\" crashes.";
    assert_eq!(evaluate(PROMPT, "prompt(request)", &incoming), expected);
    let ai_request = mock.join().expect("mock should finish");
    let ai_body = ai_request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("AI request should have a body");
    let ai_json: serde_json::Value = serde_json::from_str(ai_body).expect("AI body is JSON");
    assert_eq!(ai_json["input"], expected);
}
