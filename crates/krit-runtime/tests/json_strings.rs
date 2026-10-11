//! `json_decode` into `String` must agree with the direct evaluator.
//!
//! Webhook components decode JSON string text with a generated guest decoder.
//! On a corpus of escaped, surrogate-pair, whitespace-padded, multibyte, and
//! long inputs the component's result is compared with `krit::run_source`
//! decoding the same text, and on a corpus of malformed inputs both must
//! fail with `K4009`. The end-to-end tests decode escaped HTTP response
//! bodies and model output through the real host.

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
    GrantSet, HARD_MAX_LIMITS, HostInputs, HttpJsonAdapterConfig, HttpRequest, NetworkPolicy,
    Runtime, RuntimeError, RuntimeErrorKind, SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

const DECODE_BODY: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let decoded: String = json_decode(request.body);
    record { status: 200, headers: [], body: decoded }
}
"#;

fn compile(source_text: &str, effects: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("JSON source should parse");
    let analysis = analyze(&program).expect("JSON source should analyze");
    let module = lower(&program, &analysis).expect("JSON source should lower");
    let mut options = BuildOptions::new("2026", "test/json", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("JSON webhook should compile")
}

fn manifest(capabilities: &str) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/json"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
    ))
    .expect("JSON manifest should parse")
}

fn inputs() -> HostInputs {
    HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid")
}

fn request(body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: "/decode".to_owned(),
        query: String::new(),
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

/// Decodes `json` as a String in the direct evaluator.
fn evaluate(json: &str) -> Result<String, String> {
    let program = format!(
        "let decoded: String = json_decode({});\nprint(decoded);\n",
        literal(json)
    );
    let mut output = Vec::new();
    run_source(&Source::new("main.krit", program), &mut output)
        .map(|_| String::from_utf8(output).expect("evaluator output should be UTF-8"))
        .map_err(|diagnostic| diagnostic.code().to_owned())
}

fn invoke(
    runtime: &Runtime,
    artifact: &BuiltComponent,
    body: &str,
) -> Result<String, RuntimeError> {
    runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest("")),
            &inputs(),
            request(body),
        )
        .map(|result| {
            assert_eq!(result.response.status, 200);
            result.response.body
        })
}

/// JSON texts that decode to a string: every short escape, `\u` escapes on
/// both sides of each UTF-8 width boundary and of the surrogate block, in
/// both hex cases, surrogate pairs at the ends of the supplementary range,
/// raw multibyte and DEL bytes, whitespace padding (a model's trailing
/// newline), escaped backslashes before quotes, and escapes on both sides of
/// eight-byte boundaries in long text.
fn valid_corpus() -> Vec<String> {
    let mut texts = [
        r#""""#,
        r#""plain""#,
        r#""\"""#,
        r#""\\""#,
        r#""\/""#,
        r#""/""#,
        r#""\b""#,
        r#""\f""#,
        r#""\n""#,
        r#""\r""#,
        r#""\t""#,
        r#""line one\nline two\r\n\tindented""#,
        r#""say \"hi\" to C:\\path\/x""#,
        r#""\u0000""#,
        r#""\u001f""#,
        r#""\u0041\u0062""#,
        r#""\u007f""#,
        r#""\u0080""#,
        r#""caf\u00e9 caf\u00E9""#,
        r#""\u07ff""#,
        r#""\u0800""#,
        r#""\u20ac and \u20AC""#,
        r#""\ud7ff""#,
        r#""\ue000""#,
        r#""\ufffd""#,
        r#""\uffff""#,
        r#""\ud83d\ude00""#,
        r#""\uD83D\uDE00""#,
        r#""\ud800\udc00""#,
        r#""\udbff\udfff""#,
        r#""\u2705 done \ud83d\ude80""#,
        "\"raw 日本語 é 😀 \u{7f} \u{2028}\"",
        "\"mixed \\u65e5\u{672c} \\ud83d\\ude00😀\"",
        r#"  "padded"  "#,
        "\n\t\"padded\"\r\n",
        "\"trailing newline from a model\"\n",
        r#""\\""#,
        r#""a\\\\""#,
        r#""\\u0041""#,
        r#""\\\"""#,
        r#""{\"nested\":\"json\"}""#,
    ]
    .map(str::to_owned)
    .to_vec();
    for prefix in [6, 7, 8, 9, 15, 16] {
        texts.push(format!(
            "\"{}\\n{}\"",
            "k".repeat(prefix),
            "v".repeat(prefix)
        ));
        texts.push(format!("\"{}\\ud83d\\ude00\"", "k".repeat(prefix)));
    }
    let long = "0123456789abcdef".repeat(64);
    texts.push(format!("\"{long}\""));
    texts.push(format!("\"{long}\\t{long}\\u00e9\\\"\""));
    texts.push(format!("\"{}\"", "\\n".repeat(2048)));
    texts
}

/// JSON texts the evaluator rejects with `K4009`: empty and blank text,
/// missing or extra quotes and values, unknown and truncated escapes,
/// non-hex digits, lone and misordered surrogates, raw control characters
/// inside the string, and non-JSON padding (form feed, byte-order mark).
fn invalid_corpus() -> Vec<String> {
    [
        "",
        "   ",
        "\n",
        "\"",
        "\" ",
        "\"unterminated",
        "unquoted",
        "'single'",
        "\"a\" \"b\"",
        "\"a\"x",
        "x\"a\"",
        "\"a\"\"",
        "\"a\"b\"",
        r#""\a""#,
        r#""\x41""#,
        r#""\'""#,
        r#""\U0041""#,
        r#""\0""#,
        r#""\""#,
        r#""abc\""#,
        r#""\u""#,
        r#""\u12""#,
        r#""\u12G4""#,
        r#""\u 123""#,
        r#""\u-123""#,
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\udfff""#,
        r#""\ud800x""#,
        r#""\ud800\n""#,
        r#""\ud800\u0041""#,
        r#""\ud800\ud800""#,
        r#""\udfff\ud800""#,
        r#""\ud83d""#,
        r#""x\ud83d""#,
        r#""\ud83d\u""#,
        r#""\ud83d\ude0""#,
        r#""\ud83d\\ude00""#,
        r#""\ud83d""\ude00""#,
        "\"raw\nnewline\"",
        "\"raw\ttab\"",
        "\"raw\rreturn\"",
        "\"raw\u{0}nul\"",
        "\"raw\u{1f}unit\"",
        "\u{c}\"form feed\"",
        "\"form feed\"\u{c}",
        "\u{feff}\"bom\"",
        "\u{a0}\"nbsp\"",
    ]
    .map(str::to_owned)
    .to_vec()
}

#[test]
fn escaped_json_strings_decode_exactly_as_in_the_evaluator() {
    let artifact = compile(DECODE_BODY, &[]);
    let runtime = Runtime::default();
    for json in valid_corpus() {
        let expected = evaluate(&json)
            .unwrap_or_else(|code| panic!("evaluator should decode {json:?}, got {code}"));
        let actual = invoke(&runtime, &artifact, &json)
            .unwrap_or_else(|error| panic!("component should decode {json:?}: {error:?}"));
        assert_eq!(actual, expected, "decoding {json:?}");
    }

    // Spot checks keep the evaluator itself honest.
    for (json, expected) in [
        (r#""\ud83d\ude00""#, "😀"),
        (r#""\udbff\udfff""#, "\u{10ffff}"),
        (r#""\u0000""#, "\0"),
        (r#""\"\\\/\b\f\n\r\t""#, "\"\\/\u{8}\u{c}\n\r\t"),
        (
            "\"trailing newline from a model\"\n",
            "trailing newline from a model",
        ),
    ] {
        assert_eq!(evaluate(json).as_deref(), Ok(expected), "{json:?}");
        assert_eq!(
            invoke(&runtime, &artifact, json).expect("component should decode"),
            expected
        );
    }
}

#[test]
fn malformed_json_strings_fail_with_k4009_like_the_evaluator() {
    let artifact = compile(DECODE_BODY, &[]);
    let runtime = Runtime::default();
    for json in invalid_corpus() {
        assert_eq!(
            evaluate(&json),
            Err("K4009".to_owned()),
            "evaluator should reject {json:?}"
        );
        let error = invoke(&runtime, &artifact, &json)
            .expect_err(&format!("component should reject {json:?}"));
        assert_eq!(error.code(), "K4009", "{json:?}: {error:?}");
        assert_eq!(error.kind(), RuntimeErrorKind::InvalidJson, "{json:?}");
        assert!(
            json.trim().is_empty() || !error.message().contains(json.trim()),
            "the diagnostic must not repeat the input"
        );
    }
}

#[test]
fn non_string_json_values_fail_closed_as_invalid_json() {
    // The dynamic evaluator would return these values unchecked (decoding is
    // not yet schema-directed); a component decoding into `String` cannot
    // represent them and reports invalid JSON instead.
    let artifact = compile(DECODE_BODY, &[]);
    let runtime = Runtime::default();
    for json in [
        "1",
        "-0",
        "true",
        "null",
        "{}",
        "[]",
        "[\"x\"]",
        "{\"Some\":\"x\"}",
    ] {
        let error = invoke(&runtime, &artifact, json)
            .expect_err(&format!("component should reject {json:?}"));
        assert_eq!(error.code(), "K4009", "{json:?}: {error:?}");
    }
}

#[test]
fn decoded_strings_compose_with_literals_operators_and_further_decoding() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let inner: String = json_decode(request.body);
    let twice: String = json_decode(inner);
    let fixed: String = json_decode("\"caf\\u00e9 \\ud83d\\ude00\\n\"");
    let same = if twice == "quoted \"text\"\n" { "1" } else { "0" };
    record { status: 200, headers: [], body: same + "|" + inner + "|" + twice + "|" + fixed }
}
"#,
        &[],
    );
    // JSON string text whose content is itself JSON string text.
    let inner = serde_json::to_string("quoted \"text\"\n").expect("inner text should encode");
    let body = serde_json::to_string(&inner).expect("outer text should encode");
    let decoded = invoke(&Runtime::default(), &artifact, &body).expect("decoding should succeed");
    assert_eq!(
        decoded,
        "1|\"quoted \\\"text\\\"\\n\"|quoted \"text\"\n|café 😀\n"
    );
}

/// A runtime with the full fuel budget and `memory` bytes of guest memory.
fn memory_bound_runtime(memory: usize) -> Runtime {
    let mut limits = HARD_MAX_LIMITS;
    limits
        .narrow_memory_bytes(memory)
        .expect("memory should narrow");
    Runtime::new(limits).expect("runtime should initialize")
}

#[test]
fn invalid_json_never_allocates_and_valid_json_is_bounded_by_guest_memory() {
    let artifact = compile(DECODE_BODY, &[]);
    let runtime = memory_bound_runtime(1024 * 1024);
    let plain = "a".repeat(600_000);

    // Validation precedes allocation: a large text with a lone surrogate at
    // the end is K4009, although its decoded copy could not fit.
    let invalid = format!("\"{plain}\\n\\ud800\"");
    let error = invoke(&runtime, &artifact, &invalid).expect_err("lone surrogate");
    assert_eq!(error.code(), "K4009", "{error:?}");

    // Plain text shares the request bytes, so it decodes without a copy.
    let decoded =
        invoke(&runtime, &artifact, &format!("\"{plain}\"")).expect("plain text needs no copy");
    assert_eq!(decoded.len(), plain.len());

    // An escape forces a copy, which does not fit beside the request.
    let error = invoke(&runtime, &artifact, &format!("\"{plain}\\n\""))
        .expect_err("decoded copy cannot fit");
    assert_eq!(error.kind(), RuntimeErrorKind::ResourceLimit, "{error:?}");
    assert_eq!(error.code(), "K5103");

    // The same artifact still decodes escapes within bounds afterwards.
    assert_eq!(
        invoke(&runtime, &artifact, r#""ok\n""#).expect("small input decodes"),
        "ok\n"
    );
}

#[test]
fn large_bodies_decode_within_the_default_fuel_budget() {
    // Plain runs are scanned eight bytes at a time and escaped text is
    // scanned twice (validate and measure, then write): a full-size plain
    // request body and 128 KiB of densely escaped ASCII-only text both fit.
    let artifact = compile(DECODE_BODY, &[]);
    let runtime = Runtime::default();
    let plain = "plain model answer, no escapes; ".repeat(32 * 1024 - 1);
    let json = format!("\"{plain}\"");
    assert!(json.len() > 1_000_000 && json.len() <= 1024 * 1024);
    let decoded = invoke(&runtime, &artifact, &json).expect("a plain body should decode");
    assert_eq!(decoded, plain);

    let line = "Line with \"quotes\", a tab\t, caf\u{e9} and \u{1f600}.";
    let escaped = format!("{line}\n").repeat(2200);
    // As an ASCII-only serializer writes it: non-ASCII as `\u` escapes.
    let json = serde_json::to_string(&escaped)
        .expect("text should encode")
        .replace('\u{e9}', "\\u00e9")
        .replace('\u{1f600}', "\\ud83d\\ude00");
    assert!(json.is_ascii() && json.len() > 128 * 1024);
    let decoded = invoke(&runtime, &artifact, &json).expect("an escaped body should decode");
    assert_eq!(decoded, escaped);
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

#[test]
fn webhook_decodes_escaped_http_bodies_and_model_output_end_to_end() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("mock address").port()
    );
    let issue = "Crash on \"save\"\nSteps:\n\t1. open C:\\tmp\\a.txt\n\t2. caf\u{e9} \u{1f600}";
    let summary =
        "Summary:\n- crash when saving \"a.txt\"\n- path C:\\tmp \u{2705}\r\n\u{1f680}/done";
    // The issue arrives as a JSON string body; the model answers with JSON
    // string text (escapes, \u escapes, a surrogate pair) inside the
    // adapter's `output` field, followed by the newline models often add.
    let issue_body = serde_json::to_string(issue).expect("issue should encode");
    let model_text = format!(
        "{}\n",
        serde_json::to_string(summary)
            .expect("summary should encode")
            .replace('\u{1f680}', "\\ud83d\\ude80")
            .replace('\u{2705}', "\\u2705")
    );
    assert!(model_text.contains("\\n") && model_text.contains("\\ud83d\\ude80"));
    let adapter_body = serde_json::json!({ "output": model_text }).to_string();
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("issue request should arrive");
        let issue_request = read_request(&mut stream);
        respond(&mut stream, &issue_body);
        drop(stream);
        let (mut stream, _) = listener.accept().expect("AI request should arrive");
        let ai_request = read_request(&mut stream);
        respond(&mut stream, &adapter_body);
        (issue_request, ai_request)
    });

    let artifact = compile(
        &format!(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let fetch: HttpRequest = record {{
        method: "GET",
        path: "/issues/1",
        query: "",
        headers: [],
        body: "",
    }};
    match http_request("{origin}", fetch, None) {{
        Ok(response) => {{
            let issue: String = json_decode(response.body);
            match ai_invoke("reviewer", issue) {{
                Ok(model_output) => {{
                    let summary: String = json_decode(model_output);
                    record {{ status: 200, headers: [], body: summary }}
                }},
                Err(error) => record {{ status: 503, headers: [], body: error }},
            }}
        }},
        Err(error) => record {{ status: 502, headers: [], body: error }},
    }}
}}
"#
        ),
        &["ai.invoke", "http.request"],
    );
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
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "http = [\"{origin}\"]\nai = [\"reviewer\"]"
    )));
    let result = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request(""),
        )
        .expect("escaped bodies should decode end to end");
    assert_eq!(result.response.status, 200);
    assert_eq!(result.response.body, summary);
    assert_eq!(result.stats.http_calls, 1);
    assert_eq!(result.stats.ai_calls, 1);

    let (issue_request, ai_request) = mock.join().expect("mock should finish");
    assert!(issue_request.starts_with("GET /issues/1 HTTP/1.1\r\n"));
    // The model received the decoded issue, re-encoded by the adapter.
    let ai_body = ai_request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("AI request should have a body");
    let ai_json: serde_json::Value = serde_json::from_str(ai_body).expect("AI body is JSON");
    assert_eq!(ai_json["input"], issue);
}

#[test]
fn malformed_model_output_is_invalid_json_without_leaking_it() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("mock address").port()
    );
    let mock = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("AI request should arrive");
        let _ = read_request(&mut stream);
        respond(&mut stream, r#"{"output":"\"lone \\ud83d surrogate\""}"#);
    });
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", request.body) {
        Ok(model_output) => {
            let summary: String = json_decode(model_output);
            record { status: 200, headers: [], body: summary }
        },
        Err(error) => record { status: 503, headers: [], body: error },
    }
}
"#,
        &["ai.invoke"],
    );
    let inputs = inputs().with_network_policy(NetworkPolicy::loopback_for_tests());
    let mut policy = AgentHostPolicy::default();
    policy.ai_adapters.insert(
        "reviewer".to_owned(),
        AiAdapterConfig::HttpJson(HttpJsonAdapterConfig {
            origin: origin.clone(),
            path: "/v1/invoke".to_owned(),
            model: "test-model".to_owned(),
            secret: None,
            max_input_bytes: 1024,
            max_response_bytes: 1024,
            timeout: Duration::from_millis(500),
        }),
    );
    let approvals =
        ExplicitApprovalPolicy::new([(ApprovalOperation::AiInvoke, "reviewer".to_owned())])
            .expect("approvals should be valid");
    let host = AgentHost::new(inputs, policy, Arc::new(approvals)).expect("host should be valid");
    let grants = GrantSet::from_manifest(&manifest(&format!(
        "http = [\"{origin}\"]\nai = [\"reviewer\"]"
    )));
    let error = Runtime::default()
        .invoke_webhook_with_host(
            &artifact.bytes,
            &artifact.metadata,
            &grants,
            &host,
            request("private prompt"),
        )
        .expect_err("a lone surrogate in model output is invalid JSON");
    assert_eq!(error.code(), "K4009");
    assert_eq!(error.kind(), RuntimeErrorKind::InvalidJson);
    let rendered = format!("{error:?}");
    assert!(!rendered.contains("private prompt"));
    assert!(!rendered.contains("lone"));
    assert!(!rendered.contains("ud83d"));
    mock.join().expect("mock should finish");
}
