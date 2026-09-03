use krit::{Source, analyze, parse_source};

fn diagnostic(source_text: &str) -> String {
    let source = Source::new("cache.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    let error = analyze(&program).expect_err("source should be rejected");
    error.code().to_owned()
}

fn accepted(source_text: &str) {
    let source = Source::new("cache.krit", source_text);
    let program = parse_source(&source).expect("source should parse");
    analyze(&program).expect("source should analyze");
}

#[test]
fn namespaces_and_indexes_must_be_direct_canonical_literals() {
    // A computed resource name would defeat static auditing.
    for rejected in [
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get(request.path, "k") {
        Ok(found) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_put(request.path, "k", "v", 60) {
        Ok(done) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_delete(request.path, "k") {
        Ok(done) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match search_query(request.path, "q", 5) {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match vector_search(request.path, "[1]", 5) {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        // A non-canonical namespace name is rejected too.
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("Not Canonical", "k") {
        Ok(found) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
    ] {
        assert_eq!(diagnostic(rejected), "K3008");
    }
}

#[test]
fn indirect_references_to_cache_builtins_are_rejected() {
    assert_eq!(
        diagnostic(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let indirect = cache_get;
    record { status: 200, headers: [], body: "ok" }
}
"#
        ),
        "K3008"
    );
    assert_eq!(
        diagnostic(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let indirect = search_query;
    record { status: 200, headers: [], body: "ok" }
}
"#
        ),
        "K3008"
    );
}

#[test]
fn a_secret_can_never_be_cached_or_searched() {
    // Opaque handles have no printable or storable form, so every position
    // that would persist or transmit one is refused.
    for rejected in [
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match secret("token") {
        Ok(handle) => match cache_put("lookups", "k", handle, 60) {
            Ok(done) => record { status: 200, headers: [], body: "ok" },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match secret("token") {
        Ok(handle) => match search_query("docs", handle, 5) {
            Ok(results) => record { status: 200, headers: [], body: results },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match secret("token") {
        Ok(handle) => match cache_get("lookups", handle) {
            Ok(found) => record { status: 200, headers: [], body: "ok" },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
    ] {
        let code = diagnostic(rejected);
        assert!(
            code == "K3001" || code == "K3002",
            "an opaque handle must never reach the cache or a connector, got {code}"
        );
    }
}

#[test]
fn cache_and_search_signatures_are_fixed() {
    // Wrong arity or wrong argument types are static errors.
    for rejected in [
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups") {
        Ok(found) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_put("lookups", "k", "v") {
        Ok(done) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        // The time to live is an integer, not a string.
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_put("lookups", "k", "v", "60") {
        Ok(done) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        // The result count is an integer, not a string.
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match search_query("docs", "q", "5") {
        Ok(results) => record { status: 200, headers: [], body: results },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
    ] {
        assert!(!diagnostic(rejected).is_empty());
    }
}

#[test]
fn a_cache_result_must_be_handled_explicitly() {
    // `cache_get` yields `Result<Option<String>, String>`: source cannot use a
    // cached value without deciding what a miss and an outage mean.
    accepted(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", "k") {
        Ok(found) => match found {
            Some(value) => record { status: 200, headers: [], body: value },
            None => record { status: 404, headers: [], body: "miss" },
        },
        Err(problem) => record { status: 503, headers: [], body: problem },
    }
}
"#,
    );
    // Treating the outer Result as if it were the value is a type error.
    assert!(
        !diagnostic(
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: cache_get("lookups", "k") }
}
"#
        )
        .is_empty()
    );
}

#[test]
fn cache_and_search_effects_are_reported_exactly() {
    let source = Source::new(
        "cache.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", "k") {
        Ok(found) => match search_query("docs", "q", 3) {
            Ok(results) => match cache_delete("lookups", "k") {
                Ok(gone) => record { status: 200, headers: [], body: results },
                Err(problem) => record { status: 500, headers: [], body: problem },
            },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
    );
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = krit::lower(&program, &analysis).expect("source should lower");
    let entrypoint = module
        .entrypoints()
        .iter()
        .find(|entrypoint| entrypoint.kind == krit::EntrypointKind::Webhook)
        .expect("a webhook entrypoint should exist");
    let function = &module.functions()[entrypoint.function.as_u32() as usize];

    let effects = function
        .signature
        .effects
        .iter()
        .map(|effect| effect.as_str())
        .collect::<Vec<_>>();
    assert_eq!(effects, ["cache.read", "cache.write", "search.query"]);

    let requirements = function
        .signature
        .requirements
        .iter()
        .map(|requirement| {
            (
                requirement.capability().as_str(),
                requirement.resource().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        requirements,
        [
            ("cache.read", "lookups".to_owned()),
            ("cache.write", "lookups".to_owned()),
            ("search.query", "docs".to_owned()),
        ]
    );
}

#[test]
fn durable_facts_report_namespaces_and_keys_for_auditing() {
    let source = Source::new(
        "cache.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", "static-key") {
        Ok(found) => match search_query("docs", "q", 3) {
            Ok(results) => record { status: 200, headers: [], body: results },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
    );
    let program = parse_source(&source).expect("source should parse");

    let facts = krit::durable_operations(&program)
        .into_iter()
        .map(|operation| {
            (
                operation.kind().as_str(),
                operation.store().map(str::to_owned),
                operation.identity().map(str::to_owned),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        facts,
        [
            (
                "cache-get",
                Some("lookups".to_owned()),
                Some("static-key".to_owned())
            ),
            ("search-query", Some("docs".to_owned()), None),
        ]
    );
}

#[test]
fn cache_operations_are_unavailable_in_direct_execution() {
    let source = Source::new(
        "cache.krit",
        "let outcome = cache_get(\"lookups\", \"k\");\nprintln(1);\n",
    );
    let program = parse_source(&source).expect("source should parse");
    analyze(&program).expect("source should analyze");

    let mut output = Vec::new();
    let error = krit::execute(&program, &mut output).expect_err("host effects must be refused");

    assert_eq!(error.code(), "K5003");
}
