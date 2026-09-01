use std::{fs, path::Path};

use krit::{Source, analyze, lower, parse_source};
use krit_wasm::{
    ArtifactMetadata, BuildErrorKind, BuildOptions, EMBEDDED_METADATA_SECTION, PROGRAM_WORLD,
    PURE_PROGRAM_WORLD, STDOUT_INTERFACE, build_component, digest_bytes, validate_artifact,
    validate_component,
};
use wasm_encoder::{
    Component, CustomSection, MemorySection, MemoryType, Module, ModuleSection, RawSection,
};
use wasmparser::{Parser, Payload};

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
            "fn identity(value) { value }\n",
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
