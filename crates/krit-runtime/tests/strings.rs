//! The guest string runtime must agree with the direct evaluator.
//!
//! String `==`, `!=`, `<`, `<=`, `>`, `>=` (byte-lexicographic over UTF-8)
//! and `+` are compiled into webhook components and compared, on a corpus of
//! edge-case strings, against `krit::execute` running the same handler with
//! the same request.

use std::collections::BTreeMap;

use krit::{Source, analyze, lower, parse_source, run_source};
use krit_package::Manifest;
use krit_runtime::{
    GrantSet, HARD_MAX_LIMITS, HostInputs, HttpRequest, HttpResponse, Runtime, RuntimeErrorKind,
    RuntimeLimits, SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

fn compile(source_text: &str) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("string source should parse");
    let analysis = analyze(&program).expect("string source should analyze");
    let module = lower(&program, &analysis).expect("string source should lower");
    let options = BuildOptions::new("2026", "test/strings", "1.0.0", "src/main.krit");
    build_component(&module, &options).expect("string webhook should compile")
}

fn grants() -> GrantSet {
    GrantSet::from_manifest(
        &Manifest::parse(
            r#"
schema = 1

[package]
name = "test/strings"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
"#,
        )
        .expect("string manifest should parse"),
    )
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

/// Runs `handle(request)` in the direct evaluator and returns the body.
fn evaluate(source_text: &str, request: &HttpRequest) -> String {
    let program = format!(
        "{source_text}\nprint(handle(record {{ method: {}, path: {}, query: {}, headers: [], body: {} }}).body);\n",
        literal(&request.method),
        literal(&request.path),
        literal(&request.query),
        literal(&request.body),
    );
    let mut output = Vec::new();
    run_source(&Source::new("main.krit", program), &mut output)
        .expect("the evaluator should run the handler");
    String::from_utf8(output).expect("evaluator output should be UTF-8")
}

fn invoke(runtime: &Runtime, artifact: &BuiltComponent, request: HttpRequest) -> HttpResponse {
    runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants(),
            &inputs(),
            request,
        )
        .expect("string webhook should run")
        .response
}

/// Edge cases for byte-lexicographic order: empty strings, multibyte UTF-8,
/// bytes on both sides of 0x80 (unsigned comparison), proper prefixes,
/// equal prefixes of different lengths, differences on both sides of the
/// runtime's eight-byte word boundary, and long strings.
fn corpus() -> Vec<String> {
    let mut strings = [
        "",
        "a",
        "b",
        "A",
        "z",
        "ab",
        "abc",
        "abd",
        "abcd",
        "ba",
        "\0",
        "a\0",
        "\u{7f}",
        "\u{80}",
        "é",
        "e\u{301}",
        "ü",
        "€",
        "😀",
        "😁",
        "日本",
        "日本語",
        "\u{ffff}",
        "\u{10ffff}",
        "POST",
        "post",
        "GET",
        "/",
        "/x",
        "/x/",
        "hi ",
        "say \"hi\\\"",
        "line\nbreak",
    ]
    .map(str::to_owned)
    .to_vec();
    for length in [7, 8, 9, 15, 16, 17] {
        strings.push("k".repeat(length));
    }
    strings.push(format!("{}j", "k".repeat(7)));
    strings.push(format!("{}l", "k".repeat(8)));
    strings.push(format!("{}\u{7f}", "k".repeat(7)));
    strings.push(format!("{}\u{80}", "k".repeat(7)));
    let long = "0123456789abcdef".repeat(8);
    strings.push(format!("{}~", &long[..long.len() - 1]));
    strings.push(format!("{long}!"));
    strings.push(long);
    strings.push(format!("{}😀", "é".repeat(50)));
    strings
}

/// A handler whose response body tabulates every operator for its operands.
///
/// `GET /static` compares every pair of literals. Any other request compares
/// the request body against every literal and the body and query against
/// each other, so both host-written and static guest strings are operands.
fn differential_source(corpus: &[String]) -> String {
    let rows = corpus
        .iter()
        .map(|right| format!("row(left, {})", literal(right)))
        .collect::<Vec<_>>()
        .join("\n        + ");
    let tables = corpus
        .iter()
        .map(|left| format!("table({})", literal(left)))
        .collect::<Vec<_>>()
        .join("\n        + ");
    format!(
        r#"fn bit(value: Bool) -> String {{
    if value {{ "1" }} else {{ "0" }}
}}

fn row(left: String, right: String) -> String {{
    bit(left == right) + bit(left != right) + bit(left < right) + bit(left <= right)
        + bit(left > right) + bit(left >= right) + " " + left + "|" + right + "\n"
}}

fn table(left: String) -> String {{
    {rows}
}}

fn every_pair() -> String {{
    {tables}
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    let body = if request.method == "GET" && request.path == "/static" {{
        every_pair()
    }} else {{
        table(request.body) + row(request.body, request.query) + row(request.query, request.body)
    }};
    record {{ status: 200, headers: [], body: body }}
}}
"#
    )
}

#[test]
fn string_operators_agree_with_the_evaluator_on_the_corpus() {
    let corpus = corpus();
    let source = differential_source(&corpus);
    let artifact = compile(&source);
    let runtime = Runtime::new(HARD_MAX_LIMITS).expect("runtime should initialize");

    let every_pair = request("GET", "/static", "", "");
    let expected = evaluate(&source, &every_pair);
    let actual = invoke(&runtime, &artifact, every_pair);
    assert_eq!(actual.status, 200);
    assert_eq!(
        expected.matches('|').count(),
        corpus.len() * corpus.len(),
        "every pair should be tabulated"
    );
    assert_eq!(actual.body, expected, "static operands disagree");

    // Host-written operands. A request query cannot carry control bytes.
    let queries = ["", "a", "\u{80}", "kkkkkkkl", "日本語"];
    for (index, body) in [
        "",
        "b",
        "\u{7f}",
        "e\u{301}",
        "kkkkkkk\u{80}",
        &"0123456789abcdef".repeat(8),
    ]
    .into_iter()
    .enumerate()
    {
        let dynamic = request("POST", "/dynamic", queries[index % queries.len()], body);
        let expected = evaluate(&source, &dynamic);
        let actual = invoke(&runtime, &artifact, dynamic);
        assert_eq!(
            actual.body, expected,
            "dynamic operands disagree for {body:?}"
        );
    }

    // Spot checks keep the evaluator itself honest about byte order.
    let lines = evaluate(&source, &request("GET", "/static", "", ""));
    for (left, right, bits) in [
        ("", "", "100101"),
        ("", "a", "011100"),
        ("abc", "abcd", "011100"),
        ("abd", "abcd", "010011"),
        ("\u{7f}", "\u{80}", "011100"),
        ("e\u{301}", "é", "011100"),
        ("😀", "😁", "011100"),
        ("kkkkkkkj", "kkkkkkkk", "011100"),
    ] {
        let line = format!("{bits} {left}|{right}");
        assert!(lines.lines().any(|candidate| candidate == line), "{line}");
    }
}

#[test]
fn post_routing_webhook_composes_strings_through_invoke() {
    let artifact = compile(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    if request.method == "POST" && request.path == "/x" {
        record { status: 200, headers: [], body: "hi " + request.path }
    } else {
        if request.method != "POST" {
            record { status: 405, headers: [], body: "method " + request.method + " not allowed" }
        } else {
            record { status: 404, headers: [], body: "no route for " + request.path }
        }
    }
}
"#,
    );
    let runtime = Runtime::default();
    for (method, path, status, body) in [
        ("POST", "/x", 200, "hi /x"),
        ("GET", "/x", 405, "method GET not allowed"),
        ("POST", "/x/", 404, "no route for /x/"),
        ("POST", "/X", 404, "no route for /X"),
        ("POST", "/", 404, "no route for /"),
        ("POST", "/é", 404, "no route for /é"),
    ] {
        let response = invoke(&runtime, &artifact, request(method, path, "", ""));
        assert_eq!(
            response,
            HttpResponse {
                status,
                headers: Vec::new(),
                body: body.to_owned(),
            },
            "{method} {path}"
        );
    }
}

#[test]
fn long_strings_concatenate_and_compare_without_truncation() {
    let artifact = compile(
        r#"
fn bit(value: Bool) -> String {
    if value { "1" } else { "0" }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let joined = request.body + request.query + request.body;
    let again = request.body + request.query + request.body;
    let flags = bit(joined == again) + bit(joined < joined + "")
        + bit(request.body < joined) + bit(joined + "!" > joined);
    record { status: 200, headers: [], body: flags + joined }
}
"#,
    );
    let body = "abcdefghijklmnopqrstuvwxyz0123456789-é😀".repeat(5_000);
    let query = "q=1";
    let response = invoke(
        &Runtime::default(),
        &artifact,
        request("POST", "/long", query, &body),
    );
    assert_eq!(response.body, format!("1011{body}{query}{body}"));
}

#[test]
fn concatenation_that_exhausts_guest_memory_traps_without_a_response() {
    let artifact = compile(
        r#"
fn grow(text: String, rounds: Int) -> String {
    if rounds == 0 {
        text
    } else {
        grow(text + text, rounds - 1)
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: grow(request.body, 64) }
}
"#,
    );
    let mut limits = RuntimeLimits::default();
    limits
        .narrow_memory_bytes(2 * 1024 * 1024)
        .expect("memory should narrow");
    let runtime = Runtime::new(limits).expect("runtime should initialize");
    let error = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants(),
            &inputs(),
            request("POST", "/grow", "", &"x".repeat(4096)),
        )
        .expect_err("unbounded concatenation should exhaust guest memory");
    assert_eq!(error.kind(), RuntimeErrorKind::ResourceLimit, "{error:?}");
    assert_eq!(error.code(), "K5103");

    // The failed instance leaves nothing behind: a small input still works.
    let small = runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants(),
            &inputs(),
            request("POST", "/grow", "", ""),
        )
        .expect("empty strings never allocate");
    assert_eq!(small.response.body, "");
}
