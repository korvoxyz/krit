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
    ] {
        assert_eq!(diagnostic_code(source), "K3009", "{source}");
    }
}
