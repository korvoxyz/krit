use krit::{CoreModule, Source, Span, analyze, lower, parse_source};
use krit_wasm::{
    BuildError, BuildErrorKind, BuildOptions, CheckTarget, build_component,
    build_component_diagnostics, check_target, validate_artifact,
};

fn lower_source(text: &str) -> CoreModule {
    let source = Source::new("src/main.krit", text);
    let program = parse_source(&source).expect("test source should parse");
    let analysis = analyze(&program).expect("test source should analyze");
    lower(&program, &analysis).expect("test source should lower")
}

fn options(effects: &[&str]) -> BuildOptions {
    let mut options = BuildOptions::new("2026", "test/target", "1.0.0", "src/main.krit");
    for effect in effects {
        options.grant_effect(*effect);
    }
    options
}

fn span_of(text: &str, needle: &str) -> Span {
    let start = text
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` should occur in the test source"));
    Span::new(start, start + needle.len())
}

fn starts(errors: &[BuildError]) -> Vec<usize> {
    errors
        .iter()
        .map(|error| {
            error
                .span()
                .expect("target diagnostic should be located")
                .start
        })
        .collect()
}

fn assert_sorted_unique_and_located(errors: &[BuildError]) {
    assert!(!errors.is_empty());
    for error in errors {
        assert!(
            error.span().is_some(),
            "{} should carry a source span: {error}",
            error.code()
        );
    }
    let keys = errors
        .iter()
        .map(|error| (error.span(), error.code()))
        .collect::<Vec<_>>();
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(keys, sorted, "diagnostics should be sorted and unique");
}

#[test]
fn unused_generic_and_unsupported_helpers_are_eliminated_before_layout_checks() {
    let module_program = lower_source(
        r#"
fn identity(value) { value }
fn via_identity(value) { identity(value) }
println(1);
"#,
    );
    let artifact = build_component(&module_program, &options(&["io.stdout"]))
        .expect("unused generic helpers must not fail a module program");
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("module program with eliminated helpers should validate");
    assert!(check_target(&module_program, &options(&[]), CheckTarget::Component).is_empty());

    let webhook = lower_source(
        r#"
fn identity(value) { value }

fn first(items: List<Int>) -> Int {
    match items {
        [] => 0,
        [head, ..tail] => head,
    }
}

fn status() -> Int {
    200
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: status(), headers: [], body: request.path }
}
"#,
    );
    let artifact = build_component(&webhook, &options(&[]))
        .expect("unused helpers must not fail a webhook component");
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("webhook with eliminated helpers should validate");
    assert!(check_target(&webhook, &options(&[]), CheckTarget::Component).is_empty());
}

#[test]
fn live_parametric_values_still_report_k7001_at_their_spans() {
    let text = "fn ignore(value) { 0 }\nprintln(ignore(None));\n";
    let module = lower_source(text);
    let errors = build_component_diagnostics(&module, &options(&["io.stdout"]))
        .expect_err("a live residual type must fail closed");

    assert_sorted_unique_and_located(&errors);
    assert!(errors.iter().all(|error| error.code() == "K7001"));
    assert!(
        errors
            .iter()
            .all(|error| error.kind() == BuildErrorKind::ResidualType)
    );
    assert!(starts(&errors).contains(&span_of(text, "None").start));
    assert_eq!(
        check_target(&module, &options(&[]), CheckTarget::Component),
        errors
    );
}

#[test]
fn every_unsupported_construct_is_reported_at_its_operation() {
    let text = r#"
fn first(items: List<Int>) -> Int {
    match items {
        [] => 0,
        [head, ..tail] => head,
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let pair = record { left: 1, right: 2 };
    let count = first([1, 2]);
    let encoded = json_encode(1);
    let same = config_string("service.a") == config_string("service.b");
    record { status: 200, headers: [], body: request.path }
}
"#;
    let module = lower_source(text);
    let errors = build_component_diagnostics(&module, &options(&["config.read"]))
        .expect_err("unsupported constructs must fail closed");

    assert_sorted_unique_and_located(&errors);
    assert!(errors.iter().all(|error| error.code() == "K7002"));
    let starts = starts(&errors);
    for construct in [
        "match items",
        "record { left: 1, right: 2 }",
        "[1, 2]",
        "json_encode",
        "config_string(\"service.a\") == config_string(\"service.b\")",
    ] {
        assert!(
            starts.contains(&span_of(text, construct).start),
            "`{construct}` should be reported: {errors:#?}"
        );
    }
    assert!(
        errors
            .iter()
            .any(|error| error.message().contains("composite equality"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message().contains("list matching"))
    );

    assert_eq!(
        build_component(&module, &options(&["config.read"])).expect_err("build should fail"),
        errors[0]
    );
    assert_eq!(
        check_target(&module, &options(&[]), CheckTarget::Component),
        errors
    );
}

#[test]
fn diagnostics_name_the_policy_of_the_program_kind() {
    let module_errors = check_target(
        &lower_source("let value: String = \"text\";\n"),
        &options(&[]),
        CheckTarget::Component,
    );
    assert!(!module_errors.is_empty());
    assert!(
        module_errors
            .iter()
            .all(|error| error.message().contains("WebAssembly policy 1")),
        "{module_errors:#?}"
    );

    let webhook_errors = check_target(
        &lower_source(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let pair = record { left: request.path };
    record { status: 200, headers: [], body: request.path }
}
"#,
        ),
        &options(&[]),
        CheckTarget::Component,
    );
    assert!(!webhook_errors.is_empty());
    for error in &webhook_errors {
        assert!(error.message().contains("WebAssembly policy 2"), "{error}");
        assert!(!error.message().contains("policy 1"), "{error}");
    }
}

#[test]
fn named_constructions_build_in_webhooks_and_fail_at_their_span_in_modules() {
    let webhook = lower_source(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let fetch = HttpRequest { method: "GET", path: request.path };
    let logged = log_info("seen", [LogField { name: "path", value: fetch.path }]);
    HttpResponse { status: 200, headers: [HttpHeader { name: "x", value: "y" }], body: fetch.path }
}
"#,
    );
    let effects = options(&["observe.log"]);
    assert!(check_target(&webhook, &effects, CheckTarget::Component).is_empty());
    let artifact =
        build_component(&webhook, &effects).expect("a webhook of named constructions builds");
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("a webhook of named constructions validates");

    // Policy 1 has no record layout; the construction, including its
    // defaults, is reported at the construction, never at `1:1`.
    let text = "let status = HttpResponse { status: 1 }.status;\nprintln(status);\n";
    let errors = check_target(&lower_source(text), &options(&[]), CheckTarget::Module);
    assert_sorted_unique_and_located(&errors);
    assert!(
        errors.iter().all(|error| error.code() == "K7002"),
        "{errors:#?}"
    );
    assert_eq!(starts(&errors), [span_of(text, "HttpResponse").start]);
}

#[test]
fn entry_lists_and_unit_build_in_webhooks_and_fail_at_their_span_in_modules() {
    let webhook = lower_source(
        r#"
/// Accepts every path but `/`.
fn validate(path: String) -> Result<Unit, String> {
    if path == "/" { Err("root") } else { Ok(()) }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let fetch = HttpRequest { method: "GET", path: "/x", headers: ["accept": "text/plain"] };
    let logged = log_info("seen", ["path": fetch.path, "method": request.method]);
    match validate(request.path) {
        Ok(done) => HttpResponse { status: 200, headers: ["x-" + "path": request.path], body: fetch.path },
        Err(reason) => {
            let failed = log_error("rejected", ["reason": reason]);
            HttpResponse { status: 400, body: reason }
        },
    }
}
"#,
    );
    let effects = options(&["observe.log"]);
    assert!(check_target(&webhook, &effects, CheckTarget::Component).is_empty());
    let artifact = build_component(&webhook, &effects).expect("a webhook of entry lists builds");
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("a webhook of entry lists validates");

    // Policy 1 has no string or list layout: the entry list and its key and
    // value are reported at their own spans, never at `1:1`.
    let text = "let fields = [\"k\": \"v\"];\nprintln(1);\n";
    let errors = check_target(&lower_source(text), &options(&[]), CheckTarget::Module);
    assert_sorted_unique_and_located(&errors);
    assert!(
        errors.iter().all(|error| error.code() == "K7002"),
        "{errors:#?}"
    );
    assert_eq!(
        starts(&errors),
        [
            span_of(text, "[").start,
            span_of(text, "\"k\"").start,
            span_of(text, "\"v\"").start,
        ]
    );
}

#[test]
fn interpolation_and_text_blocks_build_in_webhooks_and_fail_at_their_span_in_modules() {
    let webhook = lower_source(
        r#"
/// Composes the reviewer's prompt.
fn prompt(request: HttpRequest) -> String {
    """
    Review "\(request.path)" (\(request.method)):

    \(request.body)
    """
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", prompt(request)) {
        Ok(answer) => HttpResponse { status: 200, body: "verdict: \(answer)" },
        Err(reason) => HttpResponse { status: 502, body: "\(reason)" },
    }
}
"#,
    );
    let effects = options(&["ai.invoke"]);
    assert!(check_target(&webhook, &effects, CheckTarget::Component).is_empty());
    let artifact = build_component(&webhook, &effects).expect("a webhook composing strings builds");
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("a webhook composing strings validates");

    // Policy 1 has no string layout: the text parts and the concatenation
    // are reported inside the interpolated string, never at `1:1`.
    let text = "let name = \"Ada\";\nlet greeting = \"hey \\(name)!\";\nprintln(1);\n";
    let errors = check_target(&lower_source(text), &options(&[]), CheckTarget::Module);
    assert_sorted_unique_and_located(&errors);
    assert!(
        errors.iter().all(|error| error.code() == "K7002"),
        "{errors:#?}"
    );
    let string = span_of(text, "\"hey \\(name)!\"");
    let inside = errors
        .iter()
        .filter_map(BuildError::span)
        .filter(|span| span.start >= string.start && span.end <= string.end)
        .map(|span| span.start)
        .collect::<Vec<_>>();
    assert!(inside.contains(&span_of(text, "hey ").start), "{errors:#?}");
    assert!(inside.contains(&span_of(text, "!\"").start), "{errors:#?}");
}

#[test]
fn the_standard_library_builds_in_webhooks_and_fails_at_its_span_in_modules() {
    let webhook = lower_source(
        r#"
fn bit(value: Bool) -> String {
    if value { "1" } else { "0" }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let number = int_to_string(string_byte_len(request.body));
    let query = url_query(["q": request.query, "page": number]);
    let echoed = url_query(request.headers);
    let flags = bit(string_is_empty(request.body)) + bit(string_contains(request.body, "x"))
        + bit(string_starts_with(request.path, "/")) + bit(string_ends_with(request.path, "/"));
    match url_path_segment(request.body) {
        Ok(segment) => HttpResponse { status: 200, body: "/a/\(segment)?\(query)&\(echoed)\(flags)" },
        Err(reason) => HttpResponse { status: 400, body: reason },
    }
}
"#,
    );
    assert!(check_target(&webhook, &options(&[]), CheckTarget::Component).is_empty());
    let artifact = build_component(&webhook, &options(&[])).expect("a stdlib webhook builds");
    validate_artifact(&artifact.bytes, &artifact.metadata).expect("a stdlib webhook validates");

    // Policy 1 has no string layout, so no standard-library function lowers
    // there; each call's built-in is reported at its name.
    let text = "let size = string_byte_len(\"abc\");\nprintln(size);\n";
    let errors = check_target(&lower_source(text), &options(&[]), CheckTarget::Module);
    assert_sorted_unique_and_located(&errors);
    assert!(
        errors.iter().all(|error| error.code() == "K7002"),
        "{errors:#?}"
    );
    assert!(
        errors.iter().any(|error| {
            error.span().map(|span| span.start) == Some(span_of(text, "string_byte_len").start)
                && error.message().contains("`string_byte_len`")
                && error.message().contains("policy 1")
        }),
        "{errors:#?}"
    );

    // Like every built-in but the stdout functions, a standard-library
    // function must be called directly in a component.
    let text = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let search = string_contains;
    HttpResponse { status: if search(request.path, "x") { 200 } else { 404 } }
}
"#;
    let errors = check_target(&lower_source(text), &options(&[]), CheckTarget::Component);
    assert_sorted_unique_and_located(&errors);
    assert!(
        errors.iter().any(|error| {
            error.span().map(|span| span.start) == Some(span_of(text, "string_contains").start)
                && error.message().contains("must remain a direct call")
        }),
        "{errors:#?}"
    );
}

#[test]
fn target_checks_ignore_package_grants_that_builds_enforce() {
    let module = lower_source("println(1);\n");
    assert!(check_target(&module, &options(&[]), CheckTarget::Component).is_empty());
    assert!(check_target(&module, &options(&[]), CheckTarget::Module).is_empty());

    let error = build_component(&module, &options(&[]))
        .expect_err("build must still enforce package grants");
    assert_eq!(error.code(), "K5001");
}

#[test]
fn module_target_rejects_entrypoint_programs_at_the_declaration() {
    let text = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
}
"#;
    let module = lower_source(text);
    assert!(check_target(&module, &options(&[]), CheckTarget::Component).is_empty());

    let errors = check_target(&module, &options(&[]), CheckTarget::Module);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), "K7002");
    assert_eq!(
        errors[0].span().map(|span| span.start),
        Some(span_of(text, "webhook fn handle").start)
    );
}

#[test]
fn effectful_module_initialization_is_reported_at_the_effect() {
    let text = r#"
let token = config_string("service.token");

webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
}
"#;
    let errors = check_target(&lower_source(text), &options(&[]), CheckTarget::Component);
    assert_sorted_unique_and_located(&errors);
    assert!(
        starts(&errors).contains(&span_of(text, "config_string(\"service.token\")").start),
        "{errors:#?}"
    );
}
