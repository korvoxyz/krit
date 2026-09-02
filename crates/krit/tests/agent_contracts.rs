use krit::{Effect, EntrypointKind, Source, SymbolKind, Type, analyze, lower, parse_source};

fn analyze_source(text: &str) -> Result<krit::Analysis, krit::Diagnostic> {
    let source = Source::new("agent.krit", text);
    let program = parse_source(&source)?;
    analyze(&program)
}

fn diagnostic_code(text: &str) -> &'static str {
    analyze_source(text)
        .expect_err("source should fail checking")
        .code()
}

#[test]
fn durable_state_checkpoint_and_replay_facts_are_exact() {
    let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let current = state_get("agent-work", "issue");
    let saved = state_put("agent-work", "issue", request.body);
    let checkpoint = checkpoint_get("agent-work", "posted-message");
    let marked = checkpoint_put("agent-work", "posted-message", "done");
    let fetched = replay_http(
        "agent-work",
        "fetch-issue",
        "https://api.example.com",
        request,
    );
    let summary = replay_ai("agent-work", "summarize", "reviewer", request.body);
    record { status: 200, headers: [], body: request.path }
}
"#;
    let analysis = analyze_source(source).expect("durable source should analyze");
    let webhook = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == SymbolKind::Webhook)
        .expect("webhook should exist");
    let Type::Function(function) = webhook.ty() else {
        panic!("webhook should have function type")
    };
    assert_eq!(
        function
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["state.transaction"]
    );
    assert_eq!(
        function
            .requirements()
            .iter()
            .map(|requirement| (requirement.capability().as_str(), requirement.resource()))
            .collect::<Vec<_>>(),
        [
            ("ai.invoke", "reviewer"),
            ("http.request", "https://api.example.com"),
            ("state.transaction", "agent-work"),
        ]
    );
}

#[test]
fn durable_resource_identities_are_direct_canonical_literals() {
    for source in [
        r#"let store = "agent-work"; state_get(store, "key");"#,
        r#"checkpoint_get("agent-work", "Invalid Name");"#,
        r#"replay_ai("agent-work", "Invalid Name", "reviewer", "input");"#,
        r#"replay_http("agent-work", "fetch", "https://example.com/path", record { method: "GET", path: "/", query: "", headers: [], body: "" });"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3008");
    }

    assert_eq!(
        diagnostic_code(
            r#"
match secret("github-token") {
    Ok(token) => state_put("agent-work", "key", token),
    Err(error) => Err(error),
};
"#
        ),
        "K3001"
    );
}

#[test]
fn webhook_contract_types_and_transitive_capabilities_are_stable() {
    let source = Source::new(
        "agent.krit",
        r#"
fn read_model() -> Result<String, String> {
    config_string("agent.model")
}

fn read_token() -> Result<Secret, String> {
    secret("github-token")
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    let model = read_model();
    let token = read_token();
    record {
        status: 200,
        headers: [record { name: "x-request-id", value: "one" }],
        body: request.path,
    }
}
"#,
    );
    let program = parse_source(&source).expect("webhook source should parse");
    let analysis = analyze(&program).expect("webhook source should analyze");
    let webhook = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == SymbolKind::Webhook)
        .expect("webhook symbol should exist");

    assert_eq!(
        webhook.ty().to_string(),
        "fn(HttpRequest) -> HttpResponse effects {config.read, secret.read} requirements {config.read(\"agent.model\"), secret.read(\"github-token\")}"
    );
    assert!(analysis.effects().is_empty());
    assert!(analysis.requirements().is_empty());

    let module = lower(&program, &analysis).expect("webhook source should lower");
    assert_eq!(module.entrypoints().len(), 2);
    let entrypoint = &module.entrypoints()[1];
    assert_eq!(entrypoint.kind, EntrypointKind::Webhook);
    let function = &module.functions()[entrypoint.function.as_u32() as usize];
    assert_eq!(function.debug_name.as_deref(), Some("handle"));
    assert_eq!(
        function.signature.parameters[0].as_ref(),
        &Type::HttpRequest
    );
    assert_eq!(function.signature.result.as_ref(), &Type::HttpResponse);
    assert_eq!(
        function
            .signature
            .effects
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["config.read", "secret.read"]
    );
    assert_eq!(
        function
            .signature
            .requirements
            .iter()
            .map(|requirement| (
                requirement.capability().as_str(),
                requirement.resource().to_owned(),
            ))
            .collect::<Vec<_>>(),
        [
            ("config.read", "agent.model".to_owned()),
            ("secret.read", "github-token".to_owned()),
        ]
    );
    assert!(module.render_text().contains(
        "entry f3 webhook handle effects {config.read, secret.read} requirements {config.read(\"agent.model\"), secret.read(\"github-token\")}"
    ));
}

#[test]
fn webhook_declarations_reject_nesting_duplicates_and_wrong_signatures() {
    let nested = Source::new(
        "nested.krit",
        r#"
fn outer() {
    webhook fn nested(request: HttpRequest) -> HttpResponse {
        record { status: 200, headers: [], body: request.path }
    }
}
"#,
    );
    assert_eq!(
        parse_source(&nested)
            .expect_err("nested webhook should fail")
            .code(),
        "K1004"
    );

    let duplicate = Source::new(
        "duplicate.krit",
        r#"
webhook fn first(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [], body: request.path }
}
webhook fn second(request: HttpRequest) -> HttpResponse {
    record { status: 204, headers: [], body: request.path }
}
"#,
    );
    assert_eq!(
        parse_source(&duplicate)
            .expect_err("duplicate webhook should fail")
            .code(),
        "K2002"
    );

    for source in [
        "webhook fn bad() -> HttpResponse { record { status: 200, headers: [], body: \"\" } }",
        "webhook fn bad(request: HttpRequest) { record { status: 200, headers: [], body: request.path } }",
        "webhook fn bad(request: String) -> HttpResponse { record { status: 200, headers: [], body: request } }",
    ] {
        assert_eq!(diagnostic_code(source), "K3007");
    }
}

#[test]
fn http_aliases_support_fields_and_require_exact_responses() {
    analyze_source(
        r#"
fn request_path(request: HttpRequest) -> String {
    request.path
}
let header: HttpHeader = record { name: "x-id", value: "one" };
webhook fn handle(request: HttpRequest) -> HttpResponse {
    record { status: 200, headers: [header], body: request_path(request) }
}
"#,
    )
    .expect("fixed aliases should participate in structural checking");

    for body in [
        "record { status: 200, body: request.path }",
        "record { status: 200, headers: [], body: request.path, extra: true }",
    ] {
        let source =
            format!("webhook fn handle(request: HttpRequest) -> HttpResponse {{ {body} }}");
        assert_eq!(diagnostic_code(&source), "K3001");
    }
}

#[test]
fn config_and_secret_resources_must_be_direct_valid_literals() {
    for source in [
        "let key = \"agent.model\"; config_string(key);",
        "let read = config_string; read(\"agent.model\");",
        "config_string(\"Agent.Model\");",
        "secret(\"github/token\");",
        "secret(\"-github-token\");",
        "secret(\"github-token-\");",
        "secret(\"github--token\");",
    ] {
        assert_eq!(diagnostic_code(source), "K3008", "{source}");
    }
}

#[test]
fn outbound_http_requires_an_exact_normalized_origin_and_direct_bearer() {
    let analysis = analyze_source(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
        match secret("upstream-token") {
            Ok(token) => match http_request("https://api.example.com", request, Some(token)) {
                Ok(response) => response,
                Err(error) => record { status: 502, headers: [], body: error },
            },
            Err(error) => record { status: 500, headers: [], body: error },
        }
}
"#,
    )
    .expect("exact HTTP origin and bearer use should analyze");
    let webhook = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == SymbolKind::Webhook)
        .expect("webhook should exist");
    let Type::Function(function) = webhook.ty() else {
        panic!("webhook should have a function type");
    };
    assert_eq!(
        function
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["http.request", "secret.read"]
    );
    assert_eq!(
        function
            .requirements()
            .iter()
            .map(|requirement| (
                requirement.capability().as_str(),
                requirement.resource().to_owned(),
            ))
            .collect::<Vec<_>>(),
        [
            ("http.request", "https://api.example.com".to_owned()),
            ("secret.read", "upstream-token".to_owned()),
        ]
    );

    for source in [
        r#"let origin = "https://api.example.com"; http_request(origin, record { method: "GET", path: "/", query: "", headers: [], body: "" }, None);"#,
        r#"http_request("HTTPS://api.example.com", record { method: "GET", path: "/", query: "", headers: [], body: "" }, None);"#,
        r#"http_request("https://api.example.com/", record { method: "GET", path: "/", query: "", headers: [], body: "" }, None);"#,
        r#"http_request("https://api.example.com:443", record { method: "GET", path: "/", query: "", headers: [], body: "" }, None);"#,
        r#"let bearer: Option<Secret> = None; http_request("https://api.example.com", record { method: "GET", path: "/", query: "", headers: [], body: "" }, bearer);"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3008", "{source}");
    }
}

#[test]
fn empty_secret_containers_lower_without_storing_a_handle() {
    for text in [
        "let values: List<Secret> = [];",
        "fn empty() -> List<Secret> { [] }",
        "let value: Option<Secret> = None;",
    ] {
        let source = Source::new("agent.krit", text);
        let program = parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("empty container should analyze");
        lower(&program, &analysis).expect("empty container should lower and verify");
    }
}

#[test]
fn agent_effects_and_resources_are_lexicographically_sorted_and_deduplicated() {
    let analysis = analyze_source(
        r#"
secret("z-token");
config_string("z.key");
println("visible");
secret("a-token");
config_string("a.key");
secret("z-token");
"#,
    )
    .expect("agent effects should analyze");

    assert_eq!(
        analysis
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["config.read", "io.stdout", "secret.read"]
    );
    assert_eq!(
        analysis
            .requirements()
            .iter()
            .map(|requirement| (
                requirement.capability().as_str(),
                requirement.resource().to_owned(),
            ))
            .collect::<Vec<_>>(),
        [
            ("config.read", "a.key".to_owned()),
            ("config.read", "z.key".to_owned()),
            ("secret.read", "a-token".to_owned()),
            ("secret.read", "z-token".to_owned()),
        ]
    );
}

#[test]
fn secret_handles_cannot_be_revealed_or_stored() {
    for source in [
        r#"match secret("github-token") { Ok(value) => println(value), Err(message) => println(message) };"#,
        r#"match secret("github-token") { Ok(value) => json_encode(value), Err(message) => message };"#,
        r#"match secret("github-token") { Ok(value) => value == value, Err(message) => false };"#,
        r#"match secret("github-token") { Ok(value) => [value], Err(message) => [] };"#,
        r#"let stored = record { value: secret("github-token") };"#,
        r#"match secret("github-token") { Ok(value) => Some(value), Err(message) => None };"#,
        r#"let decoded: Secret = json_decode("\"not-a-handle\"");"#,
        r#"fn consume(value: Secret) -> Unit {} match secret("github-token") { Ok(value) => consume(value), Err(message) => {} };"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3009", "{source}");
    }
}

#[test]
fn ai_and_logging_contracts_emit_exact_sorted_facts() {
    let analysis = analyze_source(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    log_info(
        "review.started",
        [record { name: "delivery", value: request.path }],
    );
    match ai_invoke("reviewer", request.body) {
        Ok(output) => record { status: 200, headers: [], body: output },
        Err(error) => record { status: 502, headers: [], body: error },
    }
}
"#,
    )
    .expect("AI and logging source should analyze");
    let webhook = analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.kind() == SymbolKind::Webhook)
        .expect("webhook should exist");
    let Type::Function(function) = webhook.ty() else {
        panic!("webhook should be a function");
    };
    assert_eq!(
        function
            .effects()
            .iter()
            .map(Effect::as_str)
            .collect::<Vec<_>>(),
        ["ai.invoke", "observe.log"]
    );
    assert_eq!(
        function
            .requirements()
            .iter()
            .map(|requirement| (
                requirement.capability().as_str(),
                requirement.resource().to_owned(),
            ))
            .collect::<Vec<_>>(),
        [("ai.invoke", "reviewer".to_owned())]
    );
}

#[test]
fn ai_adapter_and_log_event_names_must_be_direct_canonical_literals() {
    for source in [
        r#"let adapter = "reviewer"; ai_invoke(adapter, "input");"#,
        r#"let invoke = ai_invoke; invoke("reviewer", "input");"#,
        r#"ai_invoke("Reviewer", "input");"#,
        r#"let event = "review.started"; log_info(event, []);"#,
        r#"let log = log_error; log("review.failed", []);"#,
        r#"log_info("review..started", []);"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3008", "{source}");
    }
}

#[test]
fn logging_cannot_accept_or_structurally_hide_secret_handles() {
    for source in [
        r#"match secret("token") { Ok(value) => log_info("review.started", [record { name: "token", value: value }]), Err(error) => Err(error) };"#,
        r#"match secret("token") { Ok(value) => log_error("review.failed", [record { name: "value", value: json_encode(value) }]), Err(error) => Err(error) };"#,
    ] {
        assert_eq!(diagnostic_code(source), "K3009", "{source}");
    }
}
