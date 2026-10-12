//! The guest standard library must agree with the direct evaluator.
//!
//! `int_to_string`, `url_path_segment`, `url_query`, `string_byte_len`,
//! `string_is_empty`, `string_contains`, `string_starts_with`, and
//! `string_ends_with` are compiled into webhook components and compared, on
//! corpora of edge cases, against `krit::run_source` running the same handler
//! with the same request. A GitHub-style webhook then builds an outbound path
//! from untrusted text and sends it through the host's path hardening.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use krit::{Source, analyze_with_warnings, lower, parse_source, run_source};
use krit_package::Manifest;
use krit_runtime::{
    GrantSet, HARD_MAX_LIMITS, HostInputs, HttpHeader, HttpRequest, HttpResponse, NetworkPolicy,
    Runtime, SecretStore,
};
use krit_wasm::{BuildOptions, BuiltComponent, build_component};

/// Compiles `source_text`, which must check without warnings.
fn compile(source_text: &str, effects: &[&str]) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("stdlib source should parse");
    let report = analyze_with_warnings(&program);
    assert!(
        report.warnings.is_empty(),
        "stdlib source should check without warnings: {:?}",
        report.warnings
    );
    let analysis = report.result.expect("stdlib source should analyze");
    let module = lower(&program, &analysis).expect("stdlib source should lower");
    let mut options = BuildOptions::new("2026", "test/stdlib", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    build_component(&module, &options).expect("stdlib webhook should compile")
}

fn grants(capabilities: &str) -> GrantSet {
    GrantSet::from_manifest(
        &Manifest::parse(&format!(
            r#"
schema = 1

[package]
name = "test/stdlib"
version = "1.0.0"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"

[capabilities]
{capabilities}
"#
        ))
        .expect("stdlib manifest should parse"),
    )
}

fn inputs() -> HostInputs {
    HostInputs::new(BTreeMap::new(), SecretStore::default())
        .expect("empty host inputs should be valid")
}

fn request(path: &str, query: &str, headers: &[(&str, &str)], body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers: headers
            .iter()
            .map(|(name, value)| HttpHeader {
                name: (*name).to_owned(),
                value: (*value).to_owned(),
            })
            .collect(),
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

/// Runs `handle(request)` in the direct evaluator and returns the response.
fn evaluate(source_text: &str, request: &HttpRequest) -> HttpResponse {
    let headers = request
        .headers
        .iter()
        .map(|header| format!("{}: {}", literal(&header.name), literal(&header.value)))
        .collect::<Vec<_>>();
    let headers = if headers.is_empty() {
        "[]".to_owned()
    } else {
        format!("[{}]", headers.join(", "))
    };
    let program = format!(
        "{source_text}\nlet response = handle(HttpRequest {{ method: {}, path: {}, query: {}, \
         headers: {headers}, body: {} }});\nprintln(int_to_string(response.status));\n\
         print(response.body);\n",
        literal(&request.method),
        literal(&request.path),
        literal(&request.query),
        literal(&request.body),
    );
    let mut output = Vec::new();
    run_source(&Source::new("main.krit", program), &mut output)
        .expect("the evaluator should run the handler");
    let output = String::from_utf8(output).expect("evaluator output should be UTF-8");
    let (status, body) = output.split_once('\n').expect("status line");
    HttpResponse {
        status: status.parse().expect("status should be an integer"),
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

fn invoke(runtime: &Runtime, artifact: &BuiltComponent, request: HttpRequest) -> HttpResponse {
    runtime
        .invoke_webhook(
            &artifact.bytes,
            &artifact.metadata,
            &grants(""),
            &inputs(),
            request,
        )
        .expect("stdlib webhook should run")
        .response
}

/// Texts for the string functions and `url_path_segment`: empty and
/// multibyte text, every refusal of the path rule and the forms beside each,
/// reserved and unreserved bytes, and lengths on both sides of the guest's
/// eight-byte word boundary.
fn text_corpus() -> Vec<String> {
    let mut texts = [
        "",
        "a",
        "ab",
        "abc",
        "b",
        "é",
        "e\u{301}",
        "😀",
        "日本語",
        "\u{10ffff}",
        "\u{ff0e}\u{ff0e}",
        "AZaz09-._~",
        "a b",
        "!*'();:@&=+$,?#[]",
        ".",
        "..",
        "...",
        " .. ",
        "  . ",
        "..;x=1",
        ".?",
        "..#frag",
        ";..",
        "..a",
        "a/b",
        "/",
        "a\\b",
        "\t",
        "a\u{0}",
        "\u{7f}",
        "\u{80}",
        "%",
        "%2",
        "%41",
        "%2e%2e",
        "100%",
        "100%25",
        "50%off",
        "%zz",
        "%u002e",
        "%U",
        "x%",
        "owner.name",
        "octo-cat",
    ]
    .map(str::to_owned)
    .to_vec();
    for length in [7, 8, 9, 15, 16, 17] {
        texts.push("k".repeat(length));
        texts.push(format!("{}.", "k".repeat(length)));
    }
    texts.push(format!("{}j", "k".repeat(7)));
    texts.push(format!("{}j", "k".repeat(15)));
    texts.push("0123456789abcdef".repeat(4));
    texts.push(format!("{} {}", "é".repeat(20), "😀"));
    texts
}

/// Needles for the searches: empty, single bytes, multibyte, partial
/// multibyte overlaps, and parts longer than eight bytes.
fn part_corpus() -> Vec<String> {
    let mut parts = [
        "", "a", "b", "c", "ab", "bc", "abc", "abcd", "é", "\u{301}", "😀", ".", "..", "/", "%",
        "k", "j",
    ]
    .map(str::to_owned)
    .to_vec();
    parts.push("k".repeat(8));
    parts.push(format!("{}j", "k".repeat(7)));
    parts.push("0123456789abcdef".repeat(2));
    parts
}

/// Integers for `int_to_string`, written as literals.
const INTEGERS: &[&str] = &[
    "0",
    "1",
    "-1",
    "9",
    "10",
    "-10",
    "99",
    "100",
    "123456789",
    "-987654321",
    "1_000_000",
    "9223372036854775807",
    "-9223372036854775807",
    "-9223372036854775808",
    "4294967296",
    "-4294967296",
];

/// A handler whose response body tabulates every standard-library function.
///
/// `/static` applies each function to literal operands. Any other path
/// applies them to the request's body, query, path, and headers, so host
/// strings and lists are operands too.
fn differential_source(texts: &[String], parts: &[String]) -> String {
    let text_rows = texts
        .iter()
        .map(|text| format!("texts({})", literal(text)))
        .collect::<Vec<_>>()
        .join("\n        + ");
    let searches = parts
        .iter()
        .map(|part| format!("search(text, {})", literal(part)))
        .collect::<Vec<_>>()
        .join("\n        + ");
    let search_rows = texts
        .iter()
        .chain(parts)
        .map(|text| format!("searches({})", literal(text)))
        .collect::<Vec<_>>()
        .join("\n        + ");
    let integers = INTEGERS
        .iter()
        .map(|integer| format!("int_to_string({integer}) + \" \""))
        .collect::<Vec<_>>()
        .join("\n        + ");
    let queries = texts
        .chunks(3)
        .map(|chunk| {
            let entries = chunk
                .iter()
                .zip(chunk.iter().rev())
                .map(|(name, value)| format!("{}: {}", literal(name), literal(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("url_query([{entries}]) + \"\\n\"")
        })
        .collect::<Vec<_>>()
        .join("\n        + ");
    format!(
        r#"fn bit(value: Bool) -> String {{
    if value {{ "1" }} else {{ "0" }}
}}

fn segment(text: String) -> String {{
    match url_path_segment(text) {{
        Ok(encoded) => "ok " + encoded,
        Err(reason) => "err " + reason,
    }}
}}

fn texts(text: String) -> String {{
    segment(text) + " | " + url_query(["k": text, text: "v"]) + " | "
        + int_to_string(string_byte_len(text)) + " " + bit(string_is_empty(text)) + "\n"
}}

fn search(text: String, part: String) -> String {{
    bit(string_contains(text, part)) + bit(string_starts_with(text, part))
        + bit(string_ends_with(text, part))
}}

fn searches(text: String) -> String {{
    {searches}
        + "\n"
}}

fn every_text() -> String {{
    {text_rows}
}}

fn every_search() -> String {{
    {search_rows}
}}

fn integers() -> String {{
    {integers}
}}

fn queries() -> String {{
    url_query([]) + "|" + url_query([]) + "\n"
        + {queries}
}}

fn dynamic(request: HttpRequest) -> String {{
    let length = string_byte_len(request.body);
    texts(request.body) + texts(request.query) + texts(request.path)
        + searches(request.body) + search(request.body, request.query)
        + search(request.query, request.body) + url_query(request.headers) + "\n"
        + int_to_string(length) + " " + int_to_string(0 - length) + " "
        + int_to_string(length * 1000003 - 9223372036854775807) + "\n"
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    if request.path == "/static" {{
        HttpResponse {{
            status: 200,
            body: every_text() + every_search() + integers() + "\n" + queries(),
        }}
    }} else {{
        HttpResponse {{ status: 200, body: dynamic(request) }}
    }}
}}
"#
    )
}

#[test]
fn standard_library_agrees_with_the_evaluator_on_the_corpus() {
    let texts = text_corpus();
    let parts = part_corpus();
    let source = differential_source(&texts, &parts);
    let artifact = compile(&source, &[]);
    let runtime = Runtime::new(HARD_MAX_LIMITS).expect("runtime should initialize");

    let every_case = request("/static", "", &[], "");
    let expected = evaluate(&source, &every_case);
    let actual = invoke(&runtime, &artifact, every_case);
    assert_eq!(actual.status, 200);
    assert_eq!(actual.body, expected.body, "static operands disagree");

    // Spot checks keep the evaluator itself honest.
    for line in [
        "err path segment is empty | k=&=v | 0 1",
        "ok a%20b | k=a%20b&a%20b=v | 3 0",
        "ok %C3%A9 | k=%C3%A9&%C3%A9=v | 2 0",
        "ok AZaz09-._~ | k=AZaz09-._~&AZaz09-._~=v | 10 0",
        "err path segment reads as `.` or `..` | k=..&..=v | 2 0",
        "err path segment reads as `.` or `..` | k=%20..%20&%20..%20=v | 4 0",
        "err path segment contains `/`, `\\`, or an ASCII control character | k=a%2Fb&a%2Fb=v | 3 0",
        "err path segment contains `%` followed by a hex digit, `u`, or `U` | k=%2541&%2541=v | 3 0",
        "ok 100%25 | k=100%25&100%25=v | 4 0",
        "ok %3B.. | k=%3B..&%3B..=v | 3 0",
        "ok ... | k=...&...=v | 3 0",
        "0 1 -1 9 10 -10 99 100 123456789 -987654321 1000000 9223372036854775807 \
         -9223372036854775807 -9223372036854775808 4294967296 -4294967296 ",
        "|",
    ] {
        assert!(
            expected.body.lines().any(|candidate| candidate == line),
            "missing `{line}` in\n{}",
            expected.body
        );
    }

    // Host-written operands: bodies, queries, paths, and header lists. A
    // request query or path cannot carry control bytes or spaces.
    for (path, query, headers, body) in [
        ("/dynamic", "", &[][..], ""),
        ("/a", "q=1", &[("x-a", "1")][..], "a b/c"),
        ("/é", "x", &[("accept", "*/*"), ("x-b", "é ü")][..], ".."),
        ("/k", "kkkkkkkj", &[][..], "kkkkkkkkkkkkkkkj"),
        ("/z", "%41", &[("x", "")][..], "\u{7f}\t%41"),
        ("/1", "abc", &[][..], &"0123456789abcdef".repeat(64)),
    ] {
        let dynamic = request(path, query, headers, body);
        let expected = evaluate(&source, &dynamic);
        let actual = invoke(&runtime, &artifact, dynamic);
        assert_eq!(actual, expected, "dynamic operands disagree for {body:?}");
    }
}

/// Characters that exercise every branch of the path rule, the encoder, and
/// the searches, plus ordinary and multibyte text.
const ALPHABET: &[char] = &[
    'a', 'Z', '0', 'f', 'G', 'u', 'U', '-', '_', '~', '.', '/', '\\', ';', '?', '#', ' ', '%', ':',
    '&', '=', '+', '\t', '\n', '\0', '\u{7f}', '\u{80}', 'é', '\u{ff0e}', '😀',
];

#[test]
fn url_path_segment_and_searches_agree_with_the_evaluator_on_generated_texts() {
    let mut state = 0x853C_49E6_748F_EA9B_u64;
    let mut next = |bound: usize| {
        // xorshift64: deterministic, so failures reproduce.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    let mut cases = Vec::new();
    for _ in 0..600 {
        let text = (0..next(10))
            .map(|_| ALPHABET[next(ALPHABET.len())])
            .collect::<Vec<_>>();
        // Parts are mostly taken from the text itself (prefixes, suffixes,
        // and inner runs), so every search also finds matches.
        let start = next(text.len() + 1);
        let end = start + next(text.len() - start + 1);
        let part = if next(4) == 0 {
            (0..next(3))
                .map(|_| ALPHABET[next(ALPHABET.len())])
                .collect::<String>()
        } else {
            text[start..end].iter().collect::<String>()
        };
        cases.push((text.into_iter().collect::<String>(), part));
    }
    let texts = cases.iter().map(|(text, _)| text).collect::<Vec<_>>();
    let functions = cases
        .chunks(100)
        .enumerate()
        .map(|(index, chunk)| {
            let rows = chunk
                .iter()
                .map(|(text, part)| format!("row({}, {})", literal(text), literal(part)))
                .collect::<Vec<_>>()
                .join("\n        + ");
            format!("fn rows{index}() -> String {{\n    {rows}\n}}\n")
        })
        .collect::<String>();
    let calls = (0..texts.len().div_ceil(100))
        .map(|index| format!("rows{index}()"))
        .collect::<Vec<_>>()
        .join(" + ");
    let source = format!(
        r#"fn bit(value: Bool) -> String {{
    if value {{ "1" }} else {{ "0" }}
}}

fn row(text: String, part: String) -> String {{
    let segment = match url_path_segment(text) {{
        Ok(encoded) => "ok " + encoded,
        Err(reason) => "err " + reason,
    }};
    segment + " " + bit(string_contains(text, part)) + bit(string_starts_with(text, part))
        + bit(string_ends_with(text, part)) + "\n"
}}

{functions}
webhook fn handle(request: HttpRequest) -> HttpResponse {{
    HttpResponse {{ status: 200, body: {calls} }}
}}
"#
    );
    let artifact = compile(&source, &[]);
    let runtime = Runtime::new(HARD_MAX_LIMITS).expect("runtime should initialize");
    let probe = request("/generated", "", &[], "");
    let expected = evaluate(&source, &probe);
    let actual = invoke(&runtime, &artifact, probe);
    assert_eq!(actual, expected);
    assert_eq!(expected.body.lines().count(), texts.len());
    let accepted = expected
        .body
        .lines()
        .filter(|line| line.starts_with("ok "))
        .count();
    assert!(accepted > 100 && accepted < 500, "{accepted} accepted");
}

const LONG_SOURCE: &str = r#"
fn repeat(text: String, rounds: Int) -> String {
    if rounds == 0 {
        text
    } else {
        repeat(text + text, rounds - 1)
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let big = repeat(request.body, 6);
    let query = url_query(["q": big, big: "v", "": ""]);
    let found = string_contains(big, request.query);
    let body = match url_path_segment(big) {
        Ok(segment) => segment,
        Err(reason) => reason,
    };
    HttpResponse {
        status: if found { 200 } else { 404 },
        body: int_to_string(string_byte_len(query)) + " " + body,
    }
}
"#;

#[test]
fn builders_handle_long_and_repetitive_input() {
    let artifact = compile(LONG_SOURCE, &[]);
    let runtime = Runtime::new(HARD_MAX_LIMITS).expect("runtime should initialize");
    let body = "a é/".repeat(300);
    let long = request("/long", "é/a", &[], &body);
    let expected = evaluate(LONG_SOURCE, &long);
    let response = invoke(&runtime, &artifact, long);
    assert_eq!(response, expected);
    assert_eq!(response.status, 200);
    // `q=E&E=v&=` for the 64-fold body encoded as E.
    let encoded = "a%20%C3%A9%2F".len() * 300 * 64;
    assert_eq!(
        response.body,
        format!(
            "{} path segment contains `/`, `\\`, or an ASCII control character",
            2 * encoded + 7
        )
    );
}

/// Accepts one request on `listener`, answers 201, and returns the raw head.
fn accept_one(listener: TcpListener) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("origin should accept one request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout should configure");
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).expect("request should read");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        stream
            .write_all(
                b"HTTP/1.1 201 Created\r\nContent-Length: 7\r\nConnection: close\r\n\r\nlabeled",
            )
            .expect("response should write");
        String::from_utf8(bytes).expect("request should be UTF-8")
    })
}

/// Proves that nothing connected to `listener`.
fn assert_never_contacted(listener: &TcpListener) {
    listener
        .set_nonblocking(true)
        .expect("listener should become nonblocking");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(error) => panic!("unexpected accept failure: {error}"),
        Ok(_) => panic!("a refused segment reached the network"),
    }
    listener
        .set_nonblocking(false)
        .expect("listener should become blocking");
}

/// A GitHub-style labeling webhook: the repository owner arrives as
/// untrusted text in the request body and the issue number is computed, so
/// the outbound path is built from `url_path_segment` and `int_to_string`,
/// and its query from `url_query`. An owner the path rule would refuse is
/// answered with 400 and never sent.
fn labeling_source(origin: &str) -> String {
    format!(
        r#"
/// Labels issue `number` of `owner`'s `app` repository.
fn label(owner: String, number: Int, label: String) -> HttpResponse {{
    match url_path_segment(owner) {{
        Ok(segment) => {{
            let issue = int_to_string(number);
            let query = url_query(["label": label, "per_page": int_to_string(100)]);
            let outbound = HttpRequest {{
                method: "POST",
                path: "/repos/\(segment)/app/issues/\(issue)/labels",
                query,
                headers: ["accept": "application/vnd.github+json"],
                body: "[\"triage\"]",
            }};
            match http_request("{origin}", outbound, None) {{
                Ok(response) => response,
                Err(error) => HttpResponse {{ status: 502, body: error }},
            }}
        }},
        Err(reason) => HttpResponse {{ status: 400, body: "invalid owner: " + reason }},
    }}
}}

webhook fn handle(request: HttpRequest) -> HttpResponse {{
    label(request.body, string_byte_len(request.query) * 7, request.query)
}}
"#
    )
}

#[test]
fn a_github_style_path_is_built_from_untrusted_text_and_passes_host_path_hardening() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("origin listener should bind");
    let origin = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("listener address").port()
    );
    let artifact = compile(&labeling_source(&origin), &["http.request"]);
    let grants = grants(&format!("http = [\"{origin}\"]"));
    let inputs = inputs().with_network_policy(NetworkPolicy::loopback_for_tests());
    let runtime = Runtime::default();
    let call = |owner: &str, label: &str| {
        runtime
            .invoke_webhook(
                &artifact.bytes,
                &artifact.metadata,
                &grants,
                &inputs,
                request("/incoming", label, &[], owner),
            )
            .unwrap_or_else(|error| panic!("{owner:?} must not trap: {error}"))
    };

    // Text the path rule would refuse is answered with 400, recoverably and
    // before any connection.
    for (owner, reason) in [
        ("", "path segment is empty"),
        ("..", "path segment reads as `.` or `..`"),
        (" ..;x", "path segment reads as `.` or `..`"),
        (
            "../admin",
            "path segment contains `/`, `\\`, or an ASCII control character",
        ),
        (
            "a\\b",
            "path segment contains `/`, `\\`, or an ASCII control character",
        ),
        (
            "line\nbreak",
            "path segment contains `/`, `\\`, or an ASCII control character",
        ),
        (
            "%2e%2e",
            "path segment contains `%` followed by a hex digit, `u`, or `U`",
        ),
    ] {
        let result = call(owner, "bug");
        assert_eq!(
            result.response,
            HttpResponse {
                status: 400,
                headers: Vec::new(),
                body: format!("invalid owner: {reason}"),
            },
            "{owner:?}"
        );
        assert_eq!(result.stats.http_calls, 0);
    }
    assert_never_contacted(&listener);

    // Accepted text is percent-encoded into one segment, which the host's
    // path hardening accepts and sends exactly as built.
    let server = accept_one(listener);
    let result = call("octo cat;é~..", "good+first/issue");
    assert_eq!(result.response.status, 201);
    assert_eq!(result.response.body, "labeled");
    assert_eq!(result.stats.http_calls, 1);
    let captured = server.join().expect("origin should finish");
    assert!(
        captured.starts_with(
            "POST /repos/octo%20cat%3B%C3%A9~../app/issues/112/labels\
             ?label=good%2Bfirst%2Fissue&per_page=100 HTTP/1.1\r\n"
        ),
        "{captured}"
    );
}
