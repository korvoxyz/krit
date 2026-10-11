use std::{borrow::Cow, fs, path::Path};

use krit::{Source, analyze, lower, parse_source};
use krit_wasm::{
    AI_INTERFACE, ArtifactMetadata, BuildErrorKind, BuildOptions, EMBEDDED_METADATA_SECTION,
    HTTP_ANONYMOUS_INTERFACE, JSON_DECODE_FUNCTION_NAME, LOGGING_INTERFACE,
    MAX_EMBEDDED_METADATA_BYTES, PROGRAM_WORLD, PURE_PROGRAM_WORLD, STATE_ARTIFACT_POLICY_VERSION,
    STATE_INTERFACE, STDOUT_INTERFACE, WEBHOOK_PROGRAM_WORLD, build_component, digest_bytes,
    validate_artifact, validate_component,
};
use wasm_encoder::{
    CodeSection, Component, CustomSection, DataCountSection, DataSection, ElementSection, Elements,
    Function, FunctionSection, HeapType, Instruction, MemorySection, MemoryType, Module,
    ModuleSection, RawSection, RefType, TableSection, TableType, TypeSection,
};
use wasmparser::{KnownCustom, Name, Parser, Payload, Validator, WasmFeatures};

fn compile(
    name: &str,
    source_text: &str,
    stdout: bool,
) -> Result<krit_wasm::BuiltComponent, krit_wasm::BuildError> {
    let source = Source::new(name, source_text);
    let program = parse_source(&source).expect("test source should parse");
    let analysis = analyze(&program).expect("test source should analyze");
    let module = lower(&program, &analysis).expect("test source should lower");
    let mut options = BuildOptions::new("2026", "test/program", "1.0.0", "src/main.krit");
    if stdout {
        options.grant_effect("io.stdout");
    }
    build_component(&module, &options)
}

fn compile_state(source_text: &str) -> Result<krit_wasm::BuiltComponent, krit_wasm::BuildError> {
    let source = Source::new("state.krit", source_text);
    let program = parse_source(&source).expect("state source should parse");
    let analysis = analyze(&program).expect("state source should analyze");
    let module = lower(&program, &analysis).expect("state source should lower");
    let mut options = BuildOptions::new("2026", "test/state", "1.0.0", "src/main.krit");
    options.grant_effect("state.transaction");
    build_component(&module, &options)
}

#[test]
fn state_webhooks_select_the_exact_state_interface_and_policy() {
    let artifact = compile_state(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let saved = state_put("agent-work", "last-path", request.path);
    let value = state_get("agent-work", "last-path");
    record { status: 200, headers: [], body: request.path }
}
"#,
    )
    .expect("state webhook should compile");

    assert_eq!(
        artifact.metadata.world,
        "krit:runtime/webhook-state-program@0.2.0"
    );
    assert_eq!(artifact.metadata.imports, [STATE_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["state.transaction"]);
    assert_eq!(
        artifact.metadata.requirements[0].capability,
        "state.transaction"
    );
    assert_eq!(artifact.metadata.requirements[0].resource, "agent-work");
    assert_eq!(
        artifact.metadata.policy_version,
        STATE_ARTIFACT_POLICY_VERSION
    );
    validate_artifact(&artifact.bytes, &artifact.metadata).expect("state artifact should validate");
}

#[test]
fn replay_artifacts_keep_external_authority_as_exact_requirements() {
    let artifact = compile_state(
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let http = replay_http(
        "agent-work",
        "fetch",
        "https://api.example.com",
        request,
    );
    let ai = replay_ai("agent-work", "summarize", "reviewer", request.body);
    record { status: 200, headers: [], body: request.path }
}
"#,
    )
    .expect("replay webhook should compile");

    assert_eq!(artifact.metadata.imports, [STATE_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["state.transaction"]);
    assert_eq!(
        artifact
            .metadata
            .requirements
            .iter()
            .map(|requirement| (
                requirement.capability.as_str(),
                requirement.resource.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("ai.invoke", "reviewer"),
            ("http.request", "https://api.example.com"),
            ("state.transaction", "agent-work"),
        ]
    );
    assert_eq!(artifact.metadata.approvals[0].operation, "ai.invoke");
    assert_eq!(artifact.metadata.approvals[0].resource, "reviewer");
}

#[test]
fn factorial_builds_as_a_valid_deterministic_component() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = fs::read_to_string(repository.join("examples/factorial.krit"))
        .expect("factorial example should be readable");
    let first = compile("/checkout-a/examples/factorial.krit", &text, true)
        .expect("factorial should compile");
    let second = compile("/different/root/examples/factorial.krit", &text, true)
        .expect("factorial should compile again");

    assert_eq!(first, second);
    assert!(first.bytes.starts_with(b"\0asm\x0d\0\x01\0"));
    assert_eq!(first.metadata.world, PROGRAM_WORLD);
    assert_eq!(first.metadata.imports, [STDOUT_INTERFACE]);
    assert_eq!(first.metadata.effects, ["io.stdout"]);
    assert_eq!(first.metadata.byte_size, first.bytes.len() as u64);
    assert_eq!(first.bytes.len(), 1393);
    assert_eq!(
        first.metadata.digest,
        "blake3:6b3c8591c1e82b520f5551a9440405e2f3433864c9f43b04ebf9169e21e8001b"
    );

    let inspection =
        validate_artifact(&first.bytes, &first.metadata).expect("artifact should validate");
    assert_eq!(inspection.world, PROGRAM_WORLD);
    assert_eq!(inspection.imports, [STDOUT_INTERFACE]);
    assert_eq!(inspection.effects, ["io.stdout"]);
    assert_eq!(inspection.exports, ["run"]);
    assert_eq!(inspection.core_module_count, 1);
    assert_eq!(inspection.table_count, 1);
    assert!(inspection.table_elements > 0);
    assert_eq!(inspection.memory_count, 0);
}

#[test]
fn pure_builds_select_the_zero_import_world_and_ignore_unused_grants() {
    let without_grant = compile("/checkout-a/pure.krit", "let answer = 6 * 7;\n", false)
        .expect("pure source should compile without grants");
    let with_unused_grant = compile("/different/root/pure.krit", "let answer = 6 * 7;\n", true)
        .expect("unused stdout authority must not change the artifact");

    assert_eq!(without_grant, with_unused_grant);
    assert_eq!(without_grant.metadata.world, PURE_PROGRAM_WORLD);
    assert!(without_grant.metadata.imports.is_empty());
    assert!(without_grant.metadata.effects.is_empty());

    let inspection = validate_artifact(&without_grant.bytes, &without_grant.metadata)
        .expect("pure artifact should validate");
    assert_eq!(inspection.world, PURE_PROGRAM_WORLD);
    assert!(inspection.imports.is_empty());
    assert!(inspection.effects.is_empty());
    assert_eq!(count_core_imports(&without_grant.bytes), 0);
}

#[test]
fn rejects_missing_stdout_authority() {
    let error = compile("missing.krit", "println(1);", false)
        .expect_err("missing stdout authority should fail");
    assert_eq!(error.code(), "K5001");
    assert_eq!(error.kind(), BuildErrorKind::Capability);
    assert!(error.span().is_some());
}

#[test]
fn compiles_the_complete_primitive_non_capturing_subset() {
    let artifact = compile(
        "supported.krit",
        r#"
fn apply(function, value) {
    function(value)
}
fn increment(value) {
    value + 1
}
fn classify(value) {
    if value < 0 || value == 0 {
        !false
    } else {
        value / 2 > 0 && value % 2 >= 0
    }
}
let answer = apply(increment, 40) * 2 - 40;
print(answer);
println(classify(-answer));
println({});
"#,
        true,
    )
    .expect("supported primitive Core should compile");

    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("supported artifact should validate");
}

#[test]
fn supports_the_minimum_i64_literal_but_rejects_out_of_range_ints() {
    compile("minimum.krit", "println(-9223372036854775808);\n", true)
        .expect("minimum i64 literal should compile");

    let error = compile("too-large.krit", "println(9223372036854775808);\n", true)
        .expect_err("out-of-range positive literal should fail closed");
    assert_eq!(error.code(), "K7002");
    assert_eq!(error.kind(), BuildErrorKind::UnsupportedSemantics);
}

#[test]
fn rejects_every_layout_and_semantic_family_outside_policy_one() {
    for (name, source, expected_kind) in [
        (
            "generic",
            "fn ignore(value) { 0 }\nprintln(ignore(None));\n",
            BuildErrorKind::ResidualType,
        ),
        (
            "string",
            "let value: String = \"text\";\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "list",
            "let value: List<Int> = [1];\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "record",
            "let value = record { field: 1 };\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "option",
            "let value: Option<Int> = Some(1);\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "result",
            "let value: Result<Int, Bool> = Ok(1);\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "json",
            "let value = json_encode(1);\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "capture",
            "let offset = 1;\nlet add = fn(value) { value + offset };\nprintln(add(1));\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
        (
            "print-string",
            "println(\"text\");\n",
            BuildErrorKind::UnsupportedSemantics,
        ),
    ] {
        let error =
            compile(&format!("{name}.krit"), source, true).expect_err("source should fail closed");
        assert_eq!(error.kind(), expected_kind, "{name}: {error}");
        assert!(matches!(error.code(), "K7001" | "K7002"), "{name}: {error}");
        assert!(error.span().is_some(), "{name}: {error}");
    }
}

#[test]
fn metadata_round_trips_and_rejects_byte_or_metadata_tampering() {
    let artifact = compile("metadata.krit", "println(42);\n", true).expect("source should compile");
    let json = serde_json::to_vec(&artifact.metadata).expect("metadata should serialize");
    let decoded: ArtifactMetadata =
        serde_json::from_slice(&json).expect("metadata should deserialize");
    assert_eq!(decoded, artifact.metadata);

    let mut tampered_bytes = artifact.bytes.clone();
    let last = tampered_bytes
        .last_mut()
        .expect("component should contain bytes");
    *last ^= 1;
    let error = validate_artifact(&tampered_bytes, &artifact.metadata)
        .expect_err("tampered bytes should fail");
    assert_eq!(error.kind(), BuildErrorKind::DigestMismatch);

    let mut tampered_metadata = artifact.metadata.clone();
    tampered_metadata.digest = digest_bytes(b"not the component");
    let error = validate_artifact(&artifact.bytes, &tampered_metadata)
        .expect_err("tampered digest should fail");
    assert_eq!(error.kind(), BuildErrorKind::DigestMismatch);

    let mut tampered_metadata = artifact.metadata.clone();
    tampered_metadata.effects.clear();
    let error = validate_artifact(&artifact.bytes, &tampered_metadata)
        .expect_err("tampered effects should fail");
    assert_eq!(error.kind(), BuildErrorKind::Metadata);
}

#[test]
fn rejects_rebuilt_stdout_artifact_with_underdeclared_effects() {
    let artifact =
        compile("underdeclared.krit", "println(42);\n", true).expect("source should compile");
    let tampered_bytes = replace_embedded_metadata(&artifact.bytes, PROGRAM_WORLD, &[]);
    let mut tampered_metadata = artifact.metadata.clone();
    tampered_metadata.digest = digest_bytes(&tampered_bytes);
    tampered_metadata.byte_size = tampered_bytes.len() as u64;
    tampered_metadata.effects.clear();

    let error = validate_artifact(&tampered_bytes, &tampered_metadata)
        .expect_err("stdout imports must derive the stdout effect");
    assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact);
    assert!(error.to_string().contains("metadata section"));
}

#[test]
fn rejects_a_sidecar_world_that_does_not_match_the_component() {
    let artifact =
        compile("world.krit", "let value = 42;\n", false).expect("pure source should compile");
    let mut tampered_metadata = artifact.metadata.clone();
    tampered_metadata.world = PROGRAM_WORLD.to_owned();

    let error = validate_artifact(&artifact.bytes, &tampered_metadata)
        .expect_err("sidecar world mismatch should fail");
    assert_eq!(error.kind(), BuildErrorKind::Metadata);
}

#[test]
fn rejects_malformed_components_and_forbidden_ambient_imports() {
    let error =
        validate_component(b"\0asm\x0d\0\x01").expect_err("truncated component should be rejected");
    assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact);

    let artifact = compile("imports.krit", "println(1);\n", true).expect("source should compile");
    let mut tampered = artifact.bytes.clone();
    replace_all_equal_length(
        &mut tampered,
        b"krit:runtime/stdout@0.2.0",
        b"wasi:runtime/stdout@0.2.0",
    );
    let error =
        validate_component(&tampered).expect_err("ambient component import should be rejected");
    assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact);
}

#[test]
fn restrictive_validation_rejects_forbidden_core_features() {
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: Some(1),
        memory64: false,
        shared: true,
        page_size_log2: None,
    });
    let mut module = Module::new();
    module.section(&memories);
    let mut component = Component::new();
    component.section(&ModuleSection(&module));

    let error = validate_component(&component.finish())
        .expect_err("shared memory must be rejected by the feature policy");
    assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact);
    assert!(error.to_string().contains("policy 1 validation"));
}

#[test]
fn restrictive_validation_rejects_operand_sized_bulk_operators() {
    // Wasmtime charges these operators fuel per byte or element on top of
    // their flat cost and checks the epoch deadline only once per operation.
    // The feature policy admits none of them, so the guest code of every
    // accepted artifact keeps the flat per-operator fuel model and loop-header
    // epoch checks. (Instantiation charges are separate; krit-runtime's
    // engine_contract_tests pin them.)
    // The memory, table, and function scaffolding alone is policy-valid, so a
    // rejection below is caused by the operator or the passive segment it
    // needs.
    Validator::new_with_features(WasmFeatures::COMPONENT_MODEL)
        .validate_all(&bulk_operator_component(&[], false))
        .expect("scaffolding must satisfy the feature policy");

    let zero = Instruction::I32Const(0);
    let null = Instruction::RefNull(HeapType::FUNC);
    let cases = [
        (
            "memory.copy",
            Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ),
        ("memory.fill", Instruction::MemoryFill(0)),
        (
            "memory.init",
            Instruction::MemoryInit {
                mem: 0,
                data_index: 0,
            },
        ),
        (
            "table.copy",
            Instruction::TableCopy {
                src_table: 0,
                dst_table: 0,
            },
        ),
        (
            "table.init",
            Instruction::TableInit {
                elem_index: 0,
                table: 0,
            },
        ),
        ("table.fill", Instruction::TableFill(0)),
        ("table.grow", Instruction::TableGrow(0)),
    ];

    for (operator, instruction) in cases {
        let body = match instruction {
            Instruction::TableFill(_) => {
                vec![zero.clone(), null.clone(), zero.clone(), instruction]
            }
            Instruction::TableGrow(_) => {
                vec![null.clone(), zero.clone(), instruction, Instruction::Drop]
            }
            _ => vec![zero.clone(), zero.clone(), zero.clone(), instruction],
        };
        let segments = matches!(operator, "memory.init" | "table.init");
        let component = bulk_operator_component(&body, segments);
        // The same bytes validate once the proposals are enabled, so the
        // rejection below comes from the feature policy alone.
        Validator::new_with_features(
            WasmFeatures::COMPONENT_MODEL
                | WasmFeatures::BULK_MEMORY
                | WasmFeatures::REFERENCE_TYPES,
        )
        .validate_all(&component)
        .unwrap_or_else(|error| panic!("{operator} fixture must be otherwise valid: {error}"));

        let error = validate_component(&component)
            .expect_err("operand-sized bulk operators must be rejected by the feature policy");
        assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact, "{operator}");
        assert!(
            error.to_string().contains("policy 1 validation"),
            "{operator}: {error}"
        );
    }
}

/// Wraps one `() -> ()` function with `body` in a component, beside a
/// one-page memory and a one-element funcref table, plus the passive data and
/// element segments `*.init` operators address when `segments` is set.
fn bulk_operator_component(body: &[Instruction<'_>], segments: bool) -> Vec<u8> {
    let mut types = TypeSection::new();
    types.ty().function([], []);
    let mut functions = FunctionSection::new();
    functions.function(0);
    let mut tables = TableSection::new();
    tables.table(TableType {
        element_type: RefType::FUNCREF,
        table64: false,
        minimum: 1,
        maximum: Some(1),
        shared: false,
    });
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: Some(1),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let mut function = Function::new([]);
    for instruction in body {
        function.instruction(instruction);
    }
    function.instruction(&Instruction::End);
    let mut code = CodeSection::new();
    code.function(&function);

    let mut module = Module::new();
    module
        .section(&types)
        .section(&functions)
        .section(&tables)
        .section(&memories);
    if segments {
        let mut elements = ElementSection::new();
        elements.passive(Elements::Functions(Cow::Borrowed(&[0])));
        module
            .section(&elements)
            .section(&DataCountSection { count: 1 });
    }
    module.section(&code);
    if segments {
        let mut data = DataSection::new();
        data.passive([0]);
        module.section(&data);
    }
    let mut component = Component::new();
    component.section(&ModuleSection(&module));
    component.finish()
}

#[test]
fn builds_a_typed_webhook_with_exact_http_requirements_and_memory() {
    let source = Source::new(
        "src/main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match http_request("https://api.example.com", request, None) {
        Ok(response) => response,
        Err(error) => record { status: 502, headers: [], body: error },
    }
}
"#,
    );
    let program = parse_source(&source).expect("webhook source should parse");
    let analysis = analyze(&program).expect("webhook source should analyze");
    let module = lower(&program, &analysis).expect("webhook source should lower");
    let mut options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
    options.grant_effect("http.request");
    let artifact = build_component(&module, &options).expect("webhook should compile");

    assert!(artifact.metadata.world.contains("webhook-http-program"));
    assert_eq!(artifact.metadata.imports, [HTTP_ANONYMOUS_INTERFACE]);
    assert_eq!(artifact.metadata.effects, ["http.request"]);
    assert_eq!(artifact.metadata.requirements.len(), 1);
    assert_eq!(
        artifact.metadata.requirements[0].resource,
        "https://api.example.com"
    );
    let inspection = validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("webhook artifact should validate");
    assert_eq!(inspection.exports, ["krit:runtime/webhook@0.2.0"]);
    assert_eq!(inspection.memory_count, 1);
    assert!(inspection.memory_minimum_bytes > 0);
    assert_eq!(inspection.memory_maximum_bytes, 16 * 1024 * 1024);

    let tampered =
        replace_embedded_metadata(&artifact.bytes, &artifact.metadata.world, &["http.request"]);
    let mut metadata = artifact.metadata;
    metadata.digest = digest_bytes(&tampered);
    metadata.byte_size = tampered.len() as u64;
    let error = validate_artifact(&tampered, &metadata)
        .expect_err("underdeclared exact resources must fail component validation");
    assert_eq!(error.kind(), BuildErrorKind::InvalidArtifact);

    assert_ne!(metadata.world, WEBHOOK_PROGRAM_WORLD);
}

#[test]
fn embedded_metadata_supports_realistic_capability_sets() {
    let reads = (0..32)
        .map(|index| format!("    config_string(\"service.key-{index}\");\n"))
        .collect::<String>();
    let source = Source::new(
        "src/main.krit",
        format!(
            "webhook fn handle(request: HttpRequest) -> HttpResponse {{\n{reads}    record {{ status: 200, headers: [], body: request.body }}\n}}\n"
        ),
    );
    let program = parse_source(&source).expect("webhook source should parse");
    let analysis = analyze(&program).expect("webhook source should analyze");
    let module = lower(&program, &analysis).expect("webhook source should lower");
    let mut options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
    options.grant_effect("config.read");

    let artifact = build_component(&module, &options)
        .expect("realistic capability metadata should fit the policy");
    assert_eq!(artifact.metadata.requirements.len(), 32);
    let embedded_size = Parser::new(0)
        .parse_all(&artifact.bytes)
        .filter_map(Result::ok)
        .find_map(|payload| match payload {
            Payload::CustomSection(section) if section.name() == EMBEDDED_METADATA_SECTION => {
                Some(section.data().len())
            }
            _ => None,
        })
        .expect("component should contain embedded metadata");
    assert!(embedded_size > 1024);
    assert!(embedded_size <= MAX_EMBEDDED_METADATA_BYTES);
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("artifact with realistic capability metadata should validate");
}

#[test]
fn underscored_configuration_keys_build_and_validate_within_the_key_grammar() {
    let source = Source::new(
        "src/main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match config_string("agent.timeout_ms") {
        Ok(value) => record { status: 200, headers: [], body: value },
        Err(error) => record { status: 500, headers: [], body: error },
    }
}
"#,
    );
    let program = parse_source(&source).expect("webhook source should parse");
    let analysis = analyze(&program).expect("webhook source should analyze");
    let module = lower(&program, &analysis).expect("webhook source should lower");
    let mut options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
    options.grant_effect("config.read");
    let artifact = build_component(&module, &options).expect("webhook should compile");
    assert_eq!(artifact.metadata.requirements.len(), 1);
    assert_eq!(artifact.metadata.requirements[0].capability, "config.read");
    assert_eq!(
        artifact.metadata.requirements[0].resource,
        "agent.timeout_ms"
    );
    validate_artifact(&artifact.bytes, &artifact.metadata)
        .expect("an underscored configuration key should validate");

    // Metadata naming a key outside the configuration-key grammar still
    // fails closed before the component is inspected.
    for resource in ["agent__timeout", "_agent.timeout", "agent.timeout_"] {
        let mut metadata = artifact.metadata.clone();
        metadata.requirements[0].resource = resource.to_owned();
        let error = validate_artifact(&artifact.bytes, &metadata)
            .expect_err("a non-canonical configuration key should fail");
        assert_eq!(error.kind(), BuildErrorKind::Metadata, "{resource}");
    }
}

#[test]
fn ai_only_and_log_only_webhooks_select_exact_least_authority_worlds() {
    for (source_text, effect, interface, world_fragment) in [
        (
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", request.body) {
        Ok(output) => record { status: 200, headers: [], body: output },
        Err(error) => record { status: 502, headers: [], body: error },
    }
}
"#,
            "ai.invoke",
            AI_INTERFACE,
            "webhook-ai-program",
        ),
        (
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    log_info("request.received", [record { name: "path", value: request.path }]);
    record { status: 200, headers: [], body: request.body }
}
"#,
            "observe.log",
            LOGGING_INTERFACE,
            "webhook-logs-program",
        ),
    ] {
        let source = Source::new("src/main.krit", source_text);
        let program = parse_source(&source).expect("source should parse");
        let analysis = analyze(&program).expect("source should analyze");
        let module = lower(&program, &analysis).expect("source should lower");
        let mut options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
        options.grant_effect(effect);
        let artifact = build_component(&module, &options).expect("source should compile");
        assert!(artifact.metadata.world.contains(world_fragment));
        assert_eq!(artifact.metadata.imports, [interface]);
        assert_eq!(artifact.metadata.effects, [effect]);
        validate_artifact(&artifact.bytes, &artifact.metadata)
            .expect("least-authority artifact should validate");
    }
}

#[test]
fn ai_approval_metadata_and_bounded_json_string_decoding_are_validated() {
    let source = Source::new(
        "src/main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match ai_invoke("reviewer", request.body) {
        Ok(output) => {
            let parsed: String = json_decode(output);
            record { status: 200, headers: [], body: parsed }
        },
        Err(error) => record { status: 502, headers: [], body: error },
    }
}
"#,
    );
    let program = parse_source(&source).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let mut options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
    options.grant_effect("ai.invoke");
    let artifact = build_component(&module, &options).expect("source should compile");
    assert_eq!(artifact.metadata.approvals.len(), 1);
    assert_eq!(artifact.metadata.approvals[0].operation, "ai.invoke");
    assert_eq!(artifact.metadata.approvals[0].resource, "reviewer");

    let mut forged = artifact.metadata.clone();
    forged.approvals.clear();
    let error = validate_artifact(&artifact.bytes, &forged)
        .expect_err("adjacent approval metadata cannot omit embedded requirements");
    assert_eq!(error.kind(), BuildErrorKind::Metadata);

    let unsupported = Source::new(
        "src/main.krit",
        r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let parsed: Record { value: String } = json_decode(request.body);
    record { status: 200, headers: [], body: parsed.value }
}
"#,
    );
    let program = parse_source(&unsupported).expect("source should parse");
    let analysis = analyze(&program).expect("source should analyze");
    let module = lower(&program, &analysis).expect("source should lower");
    let options = BuildOptions::new("2026", "test/webhook", "1.0.0", "src/main.krit");
    let error = build_component(&module, &options)
        .expect_err("general JSON decoding must remain outside policy 2");
    assert_eq!(error.code(), "K7002");
}

#[test]
fn string_operators_build_with_runtime_helpers_emitted_once_when_used() {
    let webhook = |body: &str| {
        format!(
            "webhook fn handle(request: HttpRequest) -> HttpResponse {{\n    \
             record {{ status: 200, headers: [], body: {body} }}\n}}\n"
        )
    };
    let baseline = compile("plain.krit", &webhook("request.path"), false)
        .expect("a webhook without String operators should build");
    let every_operator = webhook(
        "if request.method == \"POST\" && request.path != \"/\" && request.path < \"/z\" \
         && request.path <= request.body && request.body > \"\" && request.body >= \"a\" { \
         request.path + \"!\" + request.body } else { \"hi \" + request.path }",
    );
    let first = compile("operators.krit", &every_operator, false)
        .expect("String concatenation, equality, and ordering should build");
    let second = compile("operators.krit", &every_operator, false)
        .expect("String operators should build again");
    assert_eq!(
        first.bytes, second.bytes,
        "string lowering must be deterministic"
    );
    validate_artifact(&first.bytes, &first.metadata).expect("string artifact should validate");
    assert_eq!(first.metadata.world, baseline.metadata.world);
    assert!(first.metadata.effects.is_empty());

    let comparisons = compile(
        "comparisons.krit",
        &webhook(
            "if request.path == \"/\" || request.path > request.body { \"a\" } else { \"b\" }",
        ),
        false,
    )
    .expect("String comparisons should build");
    let concatenation = compile(
        "concatenation.krit",
        &webhook("request.path + request.body + request.query"),
        false,
    )
    .expect("String concatenation should build");

    // One comparison helper serves all six operators and one concatenation
    // helper serves every `+`; a module without String operators has neither.
    let functions = count_core_functions(&baseline.bytes);
    assert_eq!(count_core_functions(&first.bytes), functions + 2);
    assert_eq!(count_core_functions(&comparisons.bytes), functions + 1);
    assert_eq!(count_core_functions(&concatenation.bytes), functions + 1);
}

#[test]
fn json_string_decoding_emits_one_named_decoder_only_when_used() {
    let webhook = |prelude: &str, body: &str| {
        format!(
            "webhook fn handle(request: HttpRequest) -> HttpResponse {{\n    {prelude}\n    \
             record {{ status: 200, headers: [], body: {body} }}\n}}\n"
        )
    };
    let baseline = compile("plain.krit", &webhook("", "request.body"), false)
        .expect("a webhook without JSON decoding should build");
    let decoding = webhook(
        "let once: String = json_decode(request.body);\n    \
         let twice: String = json_decode(once);",
        "twice",
    );
    let first = compile("decode.krit", &decoding, false).expect("JSON decoding should build");
    let second = compile("decode.krit", &decoding, false).expect("JSON decoding should rebuild");
    assert_eq!(
        first.bytes, second.bytes,
        "decoder emission must be deterministic"
    );
    validate_artifact(&first.bytes, &first.metadata).expect("decoding artifact should validate");
    assert_eq!(first.metadata.world, baseline.metadata.world);
    assert!(first.metadata.imports.is_empty() && first.metadata.effects.is_empty());

    // Two call sites share one decoder, the only function the core module
    // names; a module without `json_decode` has no decoder and no names.
    let functions = count_core_functions(&baseline.bytes);
    assert_eq!(count_core_functions(&first.bytes), functions + 1);
    assert_eq!(
        core_function_names(&first.bytes),
        [JSON_DECODE_FUNCTION_NAME]
    );
    assert!(core_function_names(&baseline.bytes).is_empty());

    let with_operators = webhook(
        "let decoded: String = json_decode(request.body);",
        "if decoded == \"\" { \"empty\" } else { decoded + \"!\" }",
    );
    let combined = compile("combined.krit", &with_operators, false)
        .expect("JSON decoding with String operators should build");
    assert_eq!(count_core_functions(&combined.bytes), functions + 3);
    assert_eq!(
        core_function_names(&combined.bytes),
        [JSON_DECODE_FUNCTION_NAME]
    );
}

#[test]
fn policy_one_rejects_string_operators_where_the_string_is_produced() {
    for (operator, source) in [
        ("==", "println(\"a\" == \"b\");\n"),
        ("<", "println(\"a\" < \"b\");\n"),
        ("+", "let joined = \"a\" + \"b\";\nprintln(1);\n"),
    ] {
        let error = compile("module.krit", source, true)
            .expect_err("module programs have no String layout");
        assert_eq!(error.code(), "K7002", "{operator}: {error}");
        assert!(
            error.message().contains("WebAssembly policy 1"),
            "{operator}: {error}"
        );
        let span = error.span().expect("the diagnostic should carry a span");
        let literal = source.find("\"a\"").expect("literal should exist");
        assert_eq!(span.start, literal, "{operator}: {error}");
    }
}

fn replace_all_equal_length(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let mut replaced = 0;
    for index in 0..=bytes.len() - from.len() {
        if &bytes[index..index + from.len()] == from {
            bytes[index..index + to.len()].copy_from_slice(to);
            replaced += 1;
        }
    }
    assert!(replaced > 0, "expected component import name should exist");
}

fn count_core_imports(bytes: &[u8]) -> usize {
    Parser::new(0)
        .parse_all(bytes)
        .filter_map(|payload| match payload.expect("component should parse") {
            Payload::ImportSection(section) => Some(section.into_imports().count()),
            _ => None,
        })
        .sum()
}

/// Counts defined functions across every core module in a component.
fn count_core_functions(bytes: &[u8]) -> u32 {
    Parser::new(0)
        .parse_all(bytes)
        .filter_map(|payload| match payload.expect("component should parse") {
            Payload::CodeSectionStart { count, .. } => Some(count),
            _ => None,
        })
        .sum()
}

/// The function names in every core module's name section, in order.
fn core_function_names(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        let Payload::CustomSection(section) = payload.expect("component should parse") else {
            continue;
        };
        let KnownCustom::Name(reader) = section.as_known() else {
            continue;
        };
        for subsection in reader {
            if let Name::Function(map) = subsection.expect("name section should parse") {
                for naming in map {
                    names.push(naming.expect("function name should parse").name.to_owned());
                }
            }
        }
    }
    names
}

fn replace_embedded_metadata(bytes: &[u8], world: &str, effects: &[&str]) -> Vec<u8> {
    let metadata = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "compilerVersion": env!("CARGO_PKG_VERSION"),
        "edition": "2026",
        "world": world,
        "effects": effects,
        "policyVersion": 1,
    }))
    .expect("replacement metadata should serialize");

    let mut component = Component::new();
    let mut depth = 0u32;
    let mut replaced = false;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.expect("component should parse");
        let top_level = depth == 0;
        let is_metadata = matches!(
            &payload,
            Payload::CustomSection(section) if top_level && section.name() == EMBEDDED_METADATA_SECTION
        );
        if top_level && !is_metadata {
            if let Some((id, range)) = payload.as_section() {
                let start = usize::try_from(range.start).expect("section offset should fit usize");
                let end = usize::try_from(range.end).expect("section offset should fit usize");
                component.section(&RawSection {
                    id,
                    data: &bytes[start..end],
                });
            }
        } else if is_metadata {
            replaced = true;
        }

        match payload {
            Payload::ModuleSection { .. } | Payload::ComponentSection { .. } => {
                depth += 1;
            }
            Payload::End(_) if depth > 0 => {
                depth -= 1;
            }
            _ => {}
        }
    }
    assert!(replaced, "component metadata section should exist");
    component.section(&CustomSection {
        name: EMBEDDED_METADATA_SECTION.into(),
        data: metadata.into(),
    });
    component.finish()
}
