use std::{fs, path::Path, time::Duration};

use krit::{Source, analyze, lower, parse_source, run_source};
use krit_package::Manifest;
use krit_runtime::{GrantSet, Runtime, RuntimeErrorKind, RuntimeLimits};
use krit_wasm::{
    ArtifactMetadata, BuildOptions, BuiltComponent, PROGRAM_WORLD, PURE_PROGRAM_WORLD,
    STDOUT_INTERFACE, build_component, digest_bytes,
};
use wasmparser::{Operator, Parser, Payload};

fn compile(source_text: &str, stdout: bool) -> BuiltComponent {
    let source = Source::new("src/main.krit", source_text);
    let program = parse_source(&source).expect("test source should parse");
    let analysis = analyze(&program).expect("test source should analyze");
    let module = lower(&program, &analysis).expect("test source should lower");
    let mut options = BuildOptions::new("2026", "test/program", "1.2.3", "src/main.krit");
    if stdout {
        options.grant_effect("io.stdout");
    }
    build_component(&module, &options).expect("test source should compile")
}

fn manifest(stdout: bool) -> Manifest {
    Manifest::parse(&format!(
        r#"
schema = 1

[package]
name = "test/program"
version = "1.2.3"
edition = "2026"
entry = "src/main.krit"
license = "Apache-2.0"
target = "wasm-component"

[capabilities]
stdout = {stdout}
"#
    ))
    .expect("test manifest should parse")
}

fn execute(
    artifact: &BuiltComponent,
    stdout: bool,
) -> Result<krit_runtime::ExecutionResult, krit_runtime::RuntimeError> {
    Runtime::default().execute(
        &artifact.bytes,
        &artifact.metadata,
        &GrantSet::from_manifest(&manifest(stdout)),
    )
}

#[test]
fn runs_factorial_with_exact_buffered_output_and_stats() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = fs::read_to_string(repository.join("examples/factorial.krit"))
        .expect("factorial source should be readable");
    let artifact = compile(&source, true);

    let result = execute(&artifact, true).expect("factorial should run");

    assert_eq!(result.output, b"720\n");
    assert_eq!(result.stats.host_calls, 1);
    assert_eq!(result.stats.output_bytes, 4);
    assert!(result.stats.fuel_consumed > 0);
    assert_eq!(
        result.stats.fuel_consumed + result.stats.fuel_remaining,
        result.stats.fuel_budget
    );
}

#[test]
fn pure_world_links_zero_imports_and_ignores_unused_manifest_authority() {
    let artifact = compile("let answer = 6 * 7;\n", false);
    assert_eq!(artifact.metadata.world, PURE_PROGRAM_WORLD);
    assert!(artifact.metadata.imports.is_empty());

    let result = execute(&artifact, true).expect("unused stdout authority should stay unlinked");
    assert!(result.output.is_empty());
    assert_eq!(result.stats.host_calls, 0);
}

#[test]
fn stdout_world_requires_the_manifest_grant() {
    let artifact = compile("println(42);\n", true);
    assert_eq!(artifact.metadata.world, PROGRAM_WORLD);
    assert_eq!(artifact.metadata.imports, [STDOUT_INTERFACE]);

    let error = execute(&artifact, false).expect_err("stdout must be denied");
    assert_eq!(error.code(), "K5001");
    assert_eq!(error.kind(), RuntimeErrorKind::Authorization);
}

#[test]
fn repeated_runs_use_fresh_state_and_are_deterministic() {
    let artifact = compile("print(1);\nprintln(true);\nprintln({});\n", true);
    let runtime = Runtime::default();
    let grants = GrantSet::from_manifest(&manifest(true));

    let first = runtime
        .execute(&artifact.bytes, &artifact.metadata, &grants)
        .expect("first run should succeed");
    let second = runtime
        .execute(&artifact.bytes, &artifact.metadata, &grants)
        .expect("second run should succeed");

    assert_eq!(first.output, b"1true\n()\n");
    assert_eq!(first.output, second.output);
    assert_eq!(first.stats.host_calls, 3);
    assert_eq!(second.stats.host_calls, 3);
    assert_eq!(first.stats.policy_version, second.stats.policy_version);
    assert_eq!(first.stats.fuel_budget, second.stats.fuel_budget);
    assert_eq!(first.stats.fuel_consumed, second.stats.fuel_consumed);
    assert_eq!(first.stats.fuel_remaining, second.stats.fuel_remaining);
    assert_eq!(first.stats.output_bytes, second.stats.output_bytes);
    assert_eq!(runtime.active_deadline_workers(), 0);
}

#[test]
fn matches_the_direct_evaluator_for_the_supported_subset() {
    for source_text in [
        r#"
fn factorial(value) {
    if value == 0 {
        1
    } else {
        value * factorial(value - 1)
    }
}
println(factorial(6));
"#,
        r#"
fn apply(function, value) {
    function(value)
}
fn increment(value) {
    value + 1
}
println(apply(increment, 41));
"#,
        r#"
let left = false && {
    println(100);
    true
};
let right = true || {
    println(200);
    false
};
println(left);
println(right);
println({});
"#,
        r#"
fn classify(value) {
    if value < 0 {
        false
    } else {
        value % 2 == 0
    }
}
print(classify(-2));
print(7 / 2);
println({});
"#,
    ] {
        let artifact = compile(source_text, true);
        let sandbox = execute(&artifact, true)
            .expect("sandbox should execute")
            .output;
        let source = Source::new("src/main.krit", source_text);
        let mut direct = Vec::new();
        run_source(&source, &mut direct).expect("direct evaluator should execute");
        assert_eq!(sandbox, direct, "{source_text}");
    }
}

#[test]
fn maps_arithmetic_traps_to_stable_runtime_codes() {
    for (source, code, kind) in [
        (
            "println(1 / 0);\n",
            "K4004",
            RuntimeErrorKind::DivisionByZero,
        ),
        (
            "println(1 % 0);\n",
            "K4004",
            RuntimeErrorKind::DivisionByZero,
        ),
        (
            "println(-9223372036854775808 / -1);\n",
            "K4005",
            RuntimeErrorKind::IntegerOverflow,
        ),
        (
            "println(-9223372036854775808 % -1);\n",
            "K4005",
            RuntimeErrorKind::IntegerOverflow,
        ),
        (
            "let minimum = -9223372036854775808;\nprintln(-minimum);\n",
            "K4005",
            RuntimeErrorKind::IntegerOverflow,
        ),
        (
            "println(9223372036854775807 + 1);\n",
            "K4005",
            RuntimeErrorKind::IntegerOverflow,
        ),
        (
            "println(-9223372036854775808 - 1);\n",
            "K4005",
            RuntimeErrorKind::IntegerOverflow,
        ),
        (
            "println(9223372036854775807 * 2);\n",
            "K4005",
            RuntimeErrorKind::IntegerOverflow,
        ),
    ] {
        let direct_source = Source::new("src/main.krit", source);
        let direct =
            run_source(&direct_source, &mut Vec::new()).expect_err("direct arithmetic should fail");
        assert_eq!(direct.code(), code, "direct evaluator: {source}");

        let artifact = compile(source, true);
        let error = execute(&artifact, true).expect_err("arithmetic should trap");
        assert_eq!(error.code(), code, "{source}");
        assert_eq!(error.kind(), kind, "{source}");
    }
}

#[test]
fn adversarial_unreachable_is_not_misreported_as_integer_overflow() {
    let mut artifact = compile("println(1 + 2);\n", true);
    let offset = first_i64_add_offset(&artifact.bytes);
    artifact.bytes[offset] = 0x00;
    artifact.metadata.digest = digest_bytes(&artifact.bytes);

    let error = execute(&artifact, true).expect_err("unreachable should trap");

    assert_eq!(error.code(), "K4001");
    assert_eq!(error.kind(), RuntimeErrorKind::GuestTrap);
    assert!(error.message().contains("unreachable"));
}

#[test]
fn enforces_fuel_deadline_host_call_output_table_instance_and_stack_limits() {
    let factorial = compile(
        r#"
fn factorial(value) {
    if value == 0 {
        1
    } else {
        value * factorial(value - 1)
    }
}
println(factorial(6));
"#,
        true,
    );
    let grants = GrantSet::from_manifest(&manifest(true));

    let mut fuel_limits = RuntimeLimits::default();
    fuel_limits.narrow_fuel(1).expect("fuel should narrow");
    let fuel = Runtime::new(fuel_limits)
        .expect("runtime should initialize")
        .execute(&factorial.bytes, &factorial.metadata, &grants)
        .expect_err("fuel should exhaust");
    assert_eq!(fuel.code(), "K5101");

    let mut table_limits = RuntimeLimits::default();
    table_limits
        .narrow_table_elements(0)
        .expect("table elements should narrow");
    let table = Runtime::new(table_limits)
        .expect("runtime should initialize")
        .execute(&factorial.bytes, &factorial.metadata, &grants)
        .expect_err("table should be denied before compilation");
    assert_eq!(table.code(), "K5103");

    let mut instance_limits = RuntimeLimits::default();
    instance_limits
        .narrow_instances(0)
        .expect("instances should narrow");
    let instance = Runtime::new(instance_limits)
        .expect("runtime should initialize")
        .execute(&factorial.bytes, &factorial.metadata, &grants)
        .expect_err("instance should be denied before compilation");
    assert_eq!(instance.code(), "K5103");

    let calls_artifact = compile("print(1);\nprint(2);\n", true);
    let mut call_limits = RuntimeLimits::default();
    call_limits
        .narrow_host_calls(1)
        .expect("host calls should narrow");
    let calls = Runtime::new(call_limits)
        .expect("runtime should initialize")
        .execute(&calls_artifact.bytes, &calls_artifact.metadata, &grants)
        .expect_err("host calls should exhaust");
    assert_eq!(calls.code(), "K5104");

    let output_artifact = compile("print(12345);\n", true);
    let mut output_limits = RuntimeLimits::default();
    output_limits
        .narrow_output_bytes(4)
        .expect("output should narrow");
    let output = Runtime::new(output_limits)
        .expect("runtime should initialize")
        .execute(&output_artifact.bytes, &output_artifact.metadata, &grants)
        .expect_err("output should exhaust");
    assert_eq!(output.code(), "K5105");

    let recursion = compile("fn forever() -> Unit { forever() }\nforever();\n", false);
    let pure_grants = GrantSet::from_manifest(&manifest(false));
    let stack = Runtime::default()
        .execute(&recursion.bytes, &recursion.metadata, &pure_grants)
        .expect_err("unbounded recursion should hit the Wasm stack limit");
    assert_eq!(stack.code(), "K5103");

    let mut deadline_limits = RuntimeLimits::default();
    deadline_limits
        .narrow_deadline(Duration::from_nanos(1))
        .expect("deadline should narrow");
    let deadline_runtime = Runtime::new(deadline_limits).expect("runtime should initialize");
    let deadline = deadline_runtime
        .execute(&recursion.bytes, &recursion.metadata, &pure_grants)
        .expect_err("deadline should interrupt recursion");
    assert_eq!(deadline.code(), "K5102");
    assert_eq!(deadline_runtime.active_deadline_workers(), 0);
}

#[test]
fn guest_cannot_raise_host_limits() {
    let mut limits = RuntimeLimits::default();
    assert!(limits.narrow_fuel(limits.fuel().saturating_add(1)).is_err());
    assert!(
        limits
            .narrow_output_bytes(limits.output_bytes().saturating_add(1))
            .is_err()
    );
    assert!(
        limits
            .narrow_deadline(limits.deadline() + Duration::from_millis(1))
            .is_err()
    );
}

#[test]
fn policy_one_default_and_hard_maximum_limits_are_exact() {
    let defaults = krit_runtime::DEFAULT_LIMITS;
    assert_eq!(defaults.component_bytes(), 4 * 1024 * 1024);
    assert_eq!(defaults.metadata_bytes(), 64 * 1024);
    assert_eq!(defaults.memory_bytes(), 16 * 1024 * 1024);
    assert_eq!(defaults.table_elements(), 4096);
    assert_eq!(defaults.instances(), 16);
    assert_eq!(defaults.tables(), 8);
    assert_eq!(defaults.memories(), 1);
    assert_eq!(defaults.wasm_stack_bytes(), 512 * 1024);
    assert_eq!(defaults.fuel(), 10_000_000);
    assert_eq!(defaults.deadline(), Duration::from_secs(1));
    assert_eq!(defaults.host_calls(), 1024);
    assert_eq!(defaults.output_bytes(), 1024 * 1024);
    assert_eq!(defaults.request_body_bytes(), 1024 * 1024);
    assert_eq!(defaults.response_body_bytes(), 1024 * 1024);
    assert_eq!(defaults.header_count(), 128);
    assert_eq!(defaults.header_bytes(), 64 * 1024);
    assert_eq!(defaults.http_calls(), 16);
    assert_eq!(defaults.connect_timeout(), Duration::from_millis(250));
    assert_eq!(defaults.read_timeout(), Duration::from_millis(500));
    assert_eq!(defaults.http_timeout(), Duration::from_millis(750));
    assert_eq!(defaults.host_config_bytes(), 64 * 1024);
    assert_eq!(defaults.secret_bytes(), 64 * 1024);

    let maxima = krit_runtime::HARD_MAX_LIMITS;
    assert_eq!(maxima.component_bytes(), 16 * 1024 * 1024);
    assert_eq!(maxima.metadata_bytes(), 1024 * 1024);
    assert_eq!(maxima.memory_bytes(), 64 * 1024 * 1024);
    assert_eq!(maxima.table_elements(), 65_536);
    assert_eq!(maxima.instances(), 64);
    assert_eq!(maxima.tables(), 32);
    assert_eq!(maxima.memories(), 8);
    assert_eq!(maxima.wasm_stack_bytes(), 8 * 1024 * 1024);
    assert_eq!(maxima.fuel(), 1_000_000_000);
    assert_eq!(maxima.deadline(), Duration::from_secs(30));
    assert_eq!(maxima.host_calls(), 1_000_000);
    assert_eq!(maxima.output_bytes(), 16 * 1024 * 1024);
    assert_eq!(maxima.request_body_bytes(), 16 * 1024 * 1024);
    assert_eq!(maxima.response_body_bytes(), 16 * 1024 * 1024);
    assert_eq!(maxima.header_count(), 1024);
    assert_eq!(maxima.header_bytes(), 1024 * 1024);
    assert_eq!(maxima.http_calls(), 1024);
    assert_eq!(maxima.connect_timeout(), Duration::from_secs(5));
    assert_eq!(maxima.read_timeout(), Duration::from_secs(10));
    assert_eq!(maxima.http_timeout(), Duration::from_secs(20));
    assert_eq!(maxima.host_config_bytes(), 1024 * 1024);
    assert_eq!(maxima.secret_bytes(), 1024 * 1024);
    assert_eq!(
        Runtime::new(maxima)
            .expect("hard maxima should be configurable")
            .limits(),
        maxima
    );

    let recursion = compile("fn forever() -> Unit { forever() }\nforever();\n", false);
    let error = Runtime::new(maxima)
        .expect("hard maxima should be configurable")
        .execute(
            &recursion.bytes,
            &recursion.metadata,
            &GrantSet::from_manifest(&manifest(false)),
        )
        .expect_err("hard-maximum recursion should remain inside the Wasm stack bound");
    assert_eq!(error.code(), "K5103");
}

#[test]
fn rejects_tampering_identity_mismatches_and_oversized_inputs_before_compilation() {
    let artifact = compile("println(42);\n", true);
    let grants = GrantSet::from_manifest(&manifest(true));

    let mut tampered = artifact.bytes.clone();
    *tampered.last_mut().expect("component should not be empty") ^= 1;
    let error = Runtime::default()
        .execute(&tampered, &artifact.metadata, &grants)
        .expect_err("tampered bytes should fail");
    assert_eq!(error.code(), "K7004");

    let mut identity = artifact.metadata.clone();
    identity.package.name = "other/program".to_owned();
    let error = Runtime::default()
        .execute(&artifact.bytes, &identity, &grants)
        .expect_err("wrong package identity should fail");
    assert_eq!(error.code(), "K5001");

    let mut version = artifact.metadata.clone();
    version.package.version = "9.9.9".to_owned();
    let error = Runtime::default()
        .execute(&artifact.bytes, &version, &grants)
        .expect_err("wrong package version should fail");
    assert_eq!(error.code(), "K5001");

    let mut entry = artifact.metadata.clone();
    entry.entry = "other.krit".to_owned();
    let error = Runtime::default()
        .execute(&artifact.bytes, &entry, &grants)
        .expect_err("wrong package entry should fail");
    assert_eq!(error.code(), "K5001");

    for mutate in [
        |metadata: &mut ArtifactMetadata| metadata.edition = "2027".to_owned(),
        |metadata: &mut ArtifactMetadata| metadata.target = "native".to_owned(),
        |metadata: &mut ArtifactMetadata| metadata.world = PURE_PROGRAM_WORLD.to_owned(),
        |metadata: &mut ArtifactMetadata| metadata.effects.clear(),
        |metadata: &mut ArtifactMetadata| metadata.imports.clear(),
    ] {
        let mut metadata = artifact.metadata.clone();
        mutate(&mut metadata);
        let error = Runtime::default()
            .execute(&artifact.bytes, &metadata, &grants)
            .expect_err("invalid metadata should fail");
        assert_eq!(error.code(), "K7004");
    }

    let mut limits = RuntimeLimits::default();
    limits
        .narrow_component_bytes(artifact.bytes.len() - 1)
        .expect("component bytes should narrow");
    let oversized = Runtime::new(limits)
        .expect("runtime should initialize")
        .execute(&artifact.bytes, &artifact.metadata, &grants)
        .expect_err("oversized component should fail before compilation");
    assert_eq!(oversized.code(), "K7003");
    assert!(oversized.message().contains("pre-compilation"));
}

#[test]
fn rejects_wasi_and_unknown_components_before_instantiation() {
    let artifact = compile("println(1);\n", true);
    let grants = GrantSet::from_manifest(&manifest(true));
    let mut wasi = artifact.bytes.clone();
    replace_all_equal_length(
        &mut wasi,
        b"krit:runtime/stdout@0.2.0",
        b"wasi:runtime/stdout@0.2.0",
    );
    let mut wasi_metadata = artifact.metadata.clone();
    wasi_metadata.digest = digest_bytes(&wasi);
    wasi_metadata.byte_size = wasi.len() as u64;
    let error = Runtime::default()
        .execute(&wasi, &wasi_metadata, &grants)
        .expect_err("WASI-like imports must fail validation");
    assert_eq!(error.code(), "K7003");

    let unknown = b"\0asm\x0d\0\x01".to_vec();
    let mut unknown_metadata = artifact.metadata.clone();
    unknown_metadata.digest = digest_bytes(&unknown);
    unknown_metadata.byte_size = unknown.len() as u64;
    let error = Runtime::default()
        .execute(&unknown, &unknown_metadata, &grants)
        .expect_err("unknown component must fail validation");
    assert_eq!(error.code(), "K7003");
}

#[test]
fn failed_invocations_return_no_success_output_and_cleanup_workers() {
    let artifact = compile("println(1);\nprintln(1 / 0);\n", true);
    let runtime = Runtime::default();
    let error = runtime
        .execute(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(true)),
        )
        .expect_err("guest should trap after buffered output");

    assert_eq!(error.code(), "K4004");
    assert_eq!(runtime.active_deadline_workers(), 0);
}

#[test]
fn reports_effective_permissions_without_claiming_deployment_evaluation() {
    let artifact = compile("println(42);\n", true);
    let runtime = Runtime::default();
    let allowed = runtime
        .permissions(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(true)),
        )
        .expect("artifact should validate");
    assert!(allowed.allowed());
    assert_eq!(allowed.required[0].capability, "io.stdout");
    assert_eq!(allowed.effective, allowed.required);
    assert!(allowed.denied.is_empty());
    assert_eq!(allowed.imports, [STDOUT_INTERFACE]);
    assert_eq!(allowed.deployment_grant_status, "not-evaluated");

    let denied = runtime
        .permissions(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(false)),
        )
        .expect("artifact should still validate");
    assert!(!denied.allowed());
    assert_eq!(denied.denied[0].capability, "io.stdout");
    assert!(denied.effective.is_empty());

    let mut wrong_identity = artifact.metadata.clone();
    wrong_identity.package.name = "other/program".to_owned();
    let identity = runtime
        .permissions(
            &artifact.bytes,
            &wrong_identity,
            &GrantSet::from_manifest(&manifest(true)),
        )
        .expect("identity differences belong in the complete permission report");
    assert!(!identity.allowed());
    assert!(
        identity
            .denial_reasons
            .iter()
            .any(|reason| reason.contains("package name"))
    );

    let mut limited = RuntimeLimits::default();
    limited
        .narrow_table_elements(0)
        .expect("table elements should narrow");
    let error = Runtime::new(limited)
        .expect("runtime should initialize")
        .permissions(
            &artifact.bytes,
            &artifact.metadata,
            &GrantSet::from_manifest(&manifest(true)),
        )
        .expect_err("permission inspection should enforce artifact resource shape");
    assert_eq!(error.code(), "K5103");
}

#[test]
fn successful_runs_do_not_accumulate_sleeping_deadline_threads() {
    let artifact = compile("println(1);\n", true);
    let runtime = Runtime::default();
    let grants = GrantSet::from_manifest(&manifest(true));

    for _ in 0..20 {
        runtime
            .execute(&artifact.bytes, &artifact.metadata, &grants)
            .expect("run should succeed");
        assert_eq!(runtime.active_deadline_workers(), 0);
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
    assert!(replaced > 0);
}

fn first_i64_add_offset(bytes: &[u8]) -> usize {
    for payload in Parser::new(0).parse_all(bytes) {
        let Payload::CodeSectionEntry(body) = payload.expect("component should parse") else {
            continue;
        };
        let mut operators = body
            .get_operators_reader()
            .expect("function operators should parse");
        while !operators.eof() {
            let offset = operators.original_position();
            if matches!(
                operators.read().expect("operator should parse"),
                Operator::I64Add
            ) {
                return usize::try_from(offset).expect("operator offset should fit usize");
            }
        }
    }
    panic!("compiled addition should contain i64.add");
}
