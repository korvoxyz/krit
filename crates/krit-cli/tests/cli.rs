use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use krit_wasm::{ArtifactMetadata, validate_artifact};

static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        Self { path }
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, contents).expect("test source should be written");
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CLI crate should be inside the workspace")
}

fn krit() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_krit"));
    command.current_dir(repository_root());
    command
}

fn krit_in(directory: &TestDirectory) -> Command {
    let mut command = krit();
    command.current_dir(&directory.path);
    command
}

#[test]
fn runs_an_example() {
    let output = krit()
        .args(["run", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"720\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn checks_without_executing_effects() {
    let output = krit()
        .args(["check", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"checked examples/factorial.krit\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn check_accepts_pure_builtins_with_conservative_latent_effects() {
    let directory = TestDirectory::new("conservative-builtin-effects");
    for (name, source) in [
        (
            "branch-constructor.krit",
            r#"
let wrap = if true {
    Some
} else {
    fn(value) {
        println(value);
        Some(value)
    }
};
let result = wrap(1);
"#,
        ),
        (
            "higher-order-some.krit",
            r#"
fn apply(constructor, value) {
    constructor(value)
}
fn loud_some(value) {
    println(value);
    Some(value)
}
let first = apply(Some, 1);
let second = apply(loud_some, 2);
"#,
        ),
        (
            "json-encode.krit",
            r#"
let encode = if true {
    json_encode
} else {
    fn(value) {
        println(value);
        json_encode(value)
    }
};
let encoded = encode(1);
"#,
        ),
        (
            "json-decode.krit",
            r#"
let decode = if true {
    json_decode
} else {
    fn(value) {
        println(value);
        json_decode(value)
    }
};
let decoded: Int = decode("1");
"#,
        ),
    ] {
        let path = directory.file(name, source);
        let output = krit()
            .arg("check")
            .arg(&path)
            .output()
            .expect("Krit should start");

        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).expect("output should be UTF-8"),
            format!("checked {}\n", path.display())
        );
        assert!(output.stderr.is_empty(), "{name}");
    }
}

#[test]
fn explains_typed_core_facts_in_human_form() {
    let output = krit()
        .args(["explain", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let explanation = String::from_utf8(output.stdout).expect("explanation should be UTF-8");
    assert!(explanation.starts_with("Krit explanation (schema 1)\n"));
    assert!(explanation.contains("entrypoint: module-init f0\n"));
    assert!(explanation.contains("effects: {io.stdout}\n"));
    assert!(explanation.contains("b0 factorial: fn(Int) -> Int effects {}\n"));
    assert!(explanation.contains("core:\ncore module\n"));
    assert!(!explanation.contains(repository_root().to_string_lossy().as_ref()));
}

#[test]
fn explains_typed_core_facts_as_deterministic_json() {
    let first = krit()
        .args(["explain", "--json", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");
    let second = krit()
        .args(["explain", "--json", "examples/factorial.krit"])
        .output()
        .expect("Krit should start");

    assert!(first.status.success());
    assert!(first.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    let explanation: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("explanation should be valid JSON");
    assert_eq!(explanation["schema"], 1);
    assert_eq!(explanation["entrypoint"]["id"], 0);
    assert_eq!(explanation["entrypoint"]["kind"], "module-init");
    assert_eq!(explanation["entrypoint"]["resultType"], "Unit");
    assert_eq!(explanation["entrypoint"]["effects"][0], "io.stdout");
    assert_eq!(explanation["bindings"][0]["name"], "factorial");
    assert_eq!(
        explanation["bindings"][0]["type"],
        "fn(Int) -> Int effects {}"
    );
    assert!(
        explanation["core"]
            .as_str()
            .expect("Core rendering should be a string")
            .starts_with("core module\n")
    );
}

#[test]
fn explanation_json_uses_serde_escaping_and_json_diagnostics() {
    let directory = TestDirectory::new("explain-json");
    let valid = directory.file("valid.krit", "let text = \"line\\n\\\"quoted\\\"\";\n");
    let output = krit()
        .arg("explain")
        .arg("--json")
        .arg(&valid)
        .output()
        .expect("Krit should start");
    assert!(output.status.success());
    let explanation: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("explanation should be valid JSON");
    assert!(
        explanation["core"]
            .as_str()
            .expect("Core rendering should be a string")
            .contains(r#"string "line\n\"quoted\"""#)
    );

    let invalid = directory.file("invalid.krit", "let value = 1 + true;\n");
    let diagnostic = krit()
        .arg("explain")
        .arg("--json")
        .arg(&invalid)
        .output()
        .expect("Krit should start");
    assert_eq!(diagnostic.status.code(), Some(1));
    assert!(diagnostic.stdout.is_empty());
    assert!(
        String::from_utf8(diagnostic.stderr)
            .expect("diagnostic should be UTF-8")
            .starts_with("{\"schema\":1,")
    );
}

#[test]
fn explain_rejects_invalid_usage() {
    for arguments in [
        vec!["explain"],
        vec!["explain", "--unknown", "examples/factorial.krit"],
        vec!["explain", "examples/factorial.krit", "examples/lists.krit"],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }
}

#[test]
fn formatter_check_is_read_only_and_reports_noncanonical_files() {
    let directory = TestDirectory::new("fmt-check");
    let ugly = directory.file("ugly.krit", "let value=1;\r\n");
    let canonical = directory.file("canonical.krit", "let value = 1;\n");

    let output = krit()
        .arg("fmt")
        .arg("--check")
        .arg(&ugly)
        .arg(&canonical)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&ugly).expect("source should be readable"),
        b"let value=1;\r\n"
    );
    assert_eq!(
        fs::read(&canonical).expect("source should be readable"),
        b"let value = 1;\n"
    );
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert_eq!(diagnostic.matches("error[K8001]").count(), 1);
    assert!(diagnostic.contains(ugly.to_string_lossy().as_ref()));
}

#[test]
fn formatter_writes_multiple_files_in_argument_order() {
    let directory = TestDirectory::new("fmt-write");
    let first = directory.file("first.krit", "\tlet first=record{value:1,};\r\n");
    let second = directory.file("second.krit", "println(  \"ready\"  );");

    let output = krit()
        .arg("fmt")
        .arg(&first)
        .arg(&second)
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(&first).expect("source should be readable"),
        "let first = record { value: 1 };\n"
    );
    assert_eq!(
        fs::read_to_string(&second).expect("source should be readable"),
        "println(\"ready\");\n"
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("output should be UTF-8"),
        format!(
            "formatted {}\nformatted {}\n",
            first.display(),
            second.display()
        )
    );
    assert!(output.stderr.is_empty());

    let checked = krit()
        .arg("fmt")
        .arg("--check")
        .arg(&first)
        .arg(&second)
        .output()
        .expect("Krit should start");
    assert!(checked.status.success());
    assert!(checked.stdout.is_empty());
    assert!(checked.stderr.is_empty());
    assert_eq!(
        fs::read_dir(&directory.path)
            .expect("test directory should be readable")
            .count(),
        2,
        "formatter should not leave staged files"
    );
}

#[test]
fn formatter_width_repros_are_exact_and_stable_after_write() {
    let directory = TestDirectory::new("fmt-width-stability");
    let growth = directory.file(
        "growth.krit",
        "let vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv: List<Int> = [1, 2, 3];\nprintln(match vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv { [] => 0, [head, ..tail] => head });\n",
    );
    let shrink = directory.file(
        "shrink.krit",
        "let wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww = record { one: [1, 2, 3,], two: [4, 5, 6,] };\nprintln(wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww);\n",
    );

    let output = krit()
        .arg("fmt")
        .arg(&growth)
        .arg(&shrink)
        .output()
        .expect("Krit should start");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(&growth).expect("growth source should be readable"),
        "let vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv: List<Int> = [1, 2, 3];\n\nprintln(match vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv {\n    [] => 0,\n    [head, ..tail] => head,\n});\n"
    );
    assert_eq!(
        fs::read_to_string(&shrink).expect("shrink source should be readable"),
        "let wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww = record { one: [1, 2, 3], two: [4, 5, 6] };\n\nprintln(wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww);\n"
    );

    let checked = krit()
        .arg("fmt")
        .arg("--check")
        .arg(&growth)
        .arg(&shrink)
        .output()
        .expect("Krit should start");
    assert!(checked.status.success());
    assert!(checked.stdout.is_empty());
    assert!(checked.stderr.is_empty());
}

#[test]
fn formatter_does_not_write_any_file_when_batch_validation_fails() {
    let directory = TestDirectory::new("fmt-batch-failure");
    let valid = directory.file("valid.krit", "let value=1;\n");
    let invalid = directory.file("invalid.krit", "let broken = ;\n");

    let output = krit()
        .arg("fmt")
        .arg(&valid)
        .arg(&invalid)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(&valid).expect("source should be readable"),
        "let value=1;\n"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K1001]")
    );
    let entries = fs::read_dir(&directory.path)
        .expect("test directory should be readable")
        .count();
    assert_eq!(entries, 2, "formatter should not leave staged files");
}

#[test]
fn formatter_does_not_write_any_file_when_batch_read_fails() {
    let directory = TestDirectory::new("fmt-batch-read-failure");
    let valid = directory.file("valid.krit", "let value=1;\n");
    let missing = directory.path.join("missing.krit");

    let output = krit()
        .arg("fmt")
        .arg(&valid)
        .arg(&missing)
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(&valid).expect("source should be readable"),
        "let value=1;\n"
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("krit: could not read")
    );
    assert_eq!(
        fs::read_dir(&directory.path)
            .expect("test directory should be readable")
            .count(),
        1,
        "formatter should not leave staged files"
    );
}

#[test]
fn formatter_rejects_invalid_usage() {
    for arguments in [vec!["fmt"], vec!["fmt", "--unknown", "file.krit"]] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }
}

#[test]
fn check_rejects_static_errors_without_partial_output() {
    let output = krit()
        .args([
            "check",
            "--diagnostic-format",
            "json",
            "conformance/check/type/mixed-addition/program.krit",
        ])
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.contains("\"code\":\"K3001\""));
}

#[test]
fn check_preserves_fields_through_returned_records() {
    for path in [
        "conformance/check/type/returned-record-missing/program.krit",
        "conformance/check/type/returned-record-field-type/program.krit",
    ] {
        let output = krit()
            .args(["check", path])
            .output()
            .expect("Krit should start");

        assert_eq!(output.status.code(), Some(1), "{path}");
        assert!(output.stdout.is_empty(), "{path}");
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .contains("error[K3001]"),
            "{path}"
        );
    }

    for path in [
        "conformance/check/valid/returned-record-presence/program.krit",
        "conformance/check/valid/returned-record-field-type/program.krit",
    ] {
        let output = krit()
            .args(["check", path])
            .output()
            .expect("Krit should start");

        assert!(output.status.success(), "{path}");
        assert!(output.stderr.is_empty(), "{path}");
    }
}

#[test]
fn emits_machine_readable_diagnostics() {
    let output = krit()
        .args([
            "run",
            "--diagnostic-format",
            "json",
            "conformance/cases/errors/undefined-name/program.krit",
        ])
        .output()
        .expect("Krit should start");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.starts_with("{\"schema\":1,"));
    assert!(diagnostic.contains("\"code\":\"K2001\""));
    assert!(diagnostic.ends_with('\n'));
}

#[test]
fn validates_the_workspace_manifest() {
    let output = krit()
        .args(["package", "check"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"checked akshay/krit (krit.pkg)\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn displays_requested_permissions() {
    let output = krit()
        .args(["permissions"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"Requested capabilities for akshay/krit:\n  io.stdout\nDeployment grants: not evaluated\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn emits_permissions_as_json() {
    let output = krit()
        .args(["permissions", "--json"])
        .output()
        .expect("Krit should start");

    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        b"{\"schema\":1,\"package\":\"akshay/krit\",\"requested\":[{\"capability\":\"io.stdout\"}],\"grantStatus\":\"not-evaluated\"}\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn prints_the_versioned_generation_prompt() {
    let output = krit().arg("prompt").output().expect("Krit should start");

    assert!(output.status.success());
    assert!(
        output
            .stdout
            .starts_with(b"# Krit 0.2 code-generation instruction\n")
    );
    assert!(
        output
            .stdout
            .windows(b"Never invent syntax".len())
            .any(|window| window == b"Never invent syntax")
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn builds_default_and_custom_component_outputs() {
    let directory = TestDirectory::new("build-outputs");
    directory.file("main.krit", "println(720);\n");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.2.3"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );

    let default = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    assert!(
        default.status.success(),
        "{}",
        String::from_utf8_lossy(&default.stderr)
    );
    let default_component = directory.path.join("target/krit/program.wasm");
    assert!(default_component.is_file());
    assert_valid_artifact(&default_component);
    assert_eq!(
        String::from_utf8(default.stdout).expect("build output should be UTF-8"),
        format!(
            "built {}\nmetadata {}.json\n",
            default_component.display(),
            default_component.display()
        )
    );

    let custom_component = directory.path.join("dist/custom.component.wasm");
    let custom = krit()
        .arg("build")
        .arg(format!("--manifest={}", manifest.display()))
        .arg("--output")
        .arg(&custom_component)
        .output()
        .expect("Krit should start");
    assert!(
        custom.status.success(),
        "{}",
        String::from_utf8_lossy(&custom.stderr)
    );
    assert_valid_artifact(&custom_component);

    let mut relative_command = krit();
    relative_command.current_dir(&directory.path);
    let relative = relative_command
        .args([
            "build",
            "--manifest",
            "krit.pkg",
            "--output",
            "relative.wasm",
        ])
        .output()
        .expect("Krit should start");
    assert!(
        relative.status.success(),
        "{}",
        String::from_utf8_lossy(&relative.stderr)
    );
    assert_valid_artifact(&directory.path.join("relative.wasm"));
}

#[test]
fn builds_and_sandboxes_a_package_without_implicit_fallbacks() {
    let directory = TestDirectory::new("sandbox-factorial");
    directory.file(
        "main.krit",
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
    );
    directory.file("krit.pkg", &package_manifest(true));

    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let sandbox = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should sandbox");
    assert!(sandbox.status.success());
    assert_eq!(sandbox.stdout, b"720\n");
    assert!(sandbox.stderr.is_empty());
}

#[test]
fn sandbox_reports_missing_component_and_sidecar_actionably() {
    let directory = TestDirectory::new("sandbox-missing");
    directory.file("main.krit", "println(1);\n");
    directory.file("krit.pkg", &package_manifest(true));

    let missing_component = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(missing_component.status.code(), Some(1));
    assert!(missing_component.stdout.is_empty());
    let diagnostic =
        String::from_utf8(missing_component.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.contains("error[K7003]"));
    assert!(diagnostic.contains("run `krit build` first"));

    let missing_permissions = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(directory.path.join("missing.wasm"))
        .output()
        .expect("Krit should start");
    assert_eq!(missing_permissions.status.code(), Some(1));
    assert!(missing_permissions.stdout.is_empty());
    assert!(
        String::from_utf8(missing_permissions.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7003]")
    );

    let artifact = directory.path.join("program.wasm");
    fs::write(&artifact, b"not wasm").expect("placeholder artifact should be written");
    let missing_sidecar = krit_in(&directory)
        .args(["sandbox", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert_eq!(missing_sidecar.status.code(), Some(1));
    assert!(missing_sidecar.stdout.is_empty());
    let diagnostic = String::from_utf8(missing_sidecar.stderr).expect("diagnostic should be UTF-8");
    assert!(diagnostic.contains("error[K7003]"));
    assert!(diagnostic.contains("adjacent artifact metadata"));
}

#[test]
fn sandbox_rejects_tampering_and_manifest_grant_denial() {
    let directory = TestDirectory::new("sandbox-denial");
    directory.file("main.krit", "println(42);\n");
    let manifest = directory.file("krit.pkg", &package_manifest(true));
    let build = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should build");
    assert!(build.status.success());
    let artifact = directory.path.join("target/krit/program.wasm");

    let mut tampered = fs::read(&artifact).expect("artifact should be readable");
    *tampered.last_mut().expect("artifact should not be empty") ^= 1;
    fs::write(&artifact, tampered).expect("artifact should be tampered");
    let rejected = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    assert!(
        String::from_utf8(rejected.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7004]")
    );

    let rebuild = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should rebuild");
    assert!(rebuild.status.success());
    let sidecar = metadata_path(&artifact);
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&sidecar).expect("metadata should be readable"))
            .expect("metadata should be JSON");
    metadata["unexpected"] = serde_json::json!(true);
    fs::write(
        &sidecar,
        serde_json::to_vec(&metadata).expect("metadata should serialize"),
    )
    .expect("metadata should be tampered");
    let rejected_metadata = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(rejected_metadata.status.code(), Some(1));
    assert!(rejected_metadata.stdout.is_empty());
    assert!(
        String::from_utf8(rejected_metadata.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7003]")
    );

    let rebuild = krit_in(&directory)
        .arg("build")
        .output()
        .expect("Krit should rebuild");
    assert!(rebuild.status.success());
    fs::write(&manifest, package_manifest(false)).expect("manifest should be narrowed");
    let denied = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8(denied.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5001]")
    );
}

#[test]
fn sandbox_supports_pure_and_repeated_buffered_output() {
    let pure = TestDirectory::new("sandbox-pure");
    pure.file("main.krit", "let answer = 6 * 7;\n");
    pure.file("krit.pkg", &package_manifest(false));
    assert!(
        krit_in(&pure)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let pure_run = krit_in(&pure)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert!(pure_run.status.success());
    assert!(pure_run.stdout.is_empty());
    assert!(pure_run.stderr.is_empty());

    let repeated = TestDirectory::new("sandbox-repeated");
    repeated.file("main.krit", "print(1);\nprintln(true);\nprintln({});\n");
    repeated.file("krit.pkg", &package_manifest(true));
    assert!(
        krit_in(&repeated)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    for _ in 0..2 {
        let run = krit_in(&repeated)
            .arg("sandbox")
            .output()
            .expect("Krit should start");
        assert!(run.status.success());
        assert_eq!(run.stdout, b"1true\n()\n");
        assert!(run.stderr.is_empty());
    }
}

#[test]
fn sandbox_rolls_back_buffered_output_when_the_guest_fails() {
    let directory = TestDirectory::new("sandbox-output-rollback");
    directory.file("main.krit", "println(1);\nprintln(1 / 0);\n");
    directory.file("krit.pkg", &package_manifest(true));
    assert!(
        krit_in(&directory)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );

    let failed = krit_in(&directory)
        .arg("sandbox")
        .output()
        .expect("Krit should start");
    assert_eq!(failed.status.code(), Some(1));
    assert!(failed.stdout.is_empty());
    assert!(
        String::from_utf8(failed.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K4004]")
    );
}

#[test]
fn artifact_permissions_report_allowed_and_denied_human_and_json() {
    let directory = TestDirectory::new("artifact-permissions");
    directory.file("main.krit", "println(42);\n");
    let manifest = directory.file("krit.pkg", &package_manifest(true));
    assert!(
        krit_in(&directory)
            .arg("build")
            .output()
            .expect("Krit should build")
            .status
            .success()
    );
    let artifact = directory.path.join("target/krit/program.wasm");

    let allowed_human = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert!(allowed_human.status.success());
    assert!(allowed_human.stderr.is_empty());
    let allowed_human = String::from_utf8(allowed_human.stdout).expect("report should be UTF-8");
    assert!(allowed_human.contains("Required:\n  io.stdout\n"));
    assert!(allowed_human.contains("Effective:\n  io.stdout\n"));
    assert!(allowed_human.contains("Local manifest grants: allowed\n"));
    assert!(allowed_human.contains("Deployment grants: not evaluated\n"));

    let allowed_json = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert!(allowed_json.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&allowed_json.stdout).expect("report should be JSON");
    assert_eq!(report["localGrantStatus"], "allowed");
    assert_eq!(report["deploymentGrantStatus"], "not-evaluated");
    assert_eq!(report["effective"][0]["capability"], "io.stdout");

    fs::write(&manifest, package_manifest(false)).expect("manifest should be narrowed");
    let denied_human = krit_in(&directory)
        .args(["permissions", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert_eq!(denied_human.status.code(), Some(4));
    assert!(denied_human.stderr.is_empty());
    let denied_human = String::from_utf8(denied_human.stdout).expect("report should be UTF-8");
    assert!(denied_human.contains("Denied:\n  io.stdout\n"));
    assert!(denied_human.contains("Local manifest grants: denied\n"));
    assert!(denied_human.contains("Deployment grants: not evaluated\n"));

    let denied_json = krit_in(&directory)
        .args(["permissions", "--json", "--artifact"])
        .arg(&artifact)
        .output()
        .expect("Krit should start");
    assert_eq!(denied_json.status.code(), Some(4));
    assert!(denied_json.stderr.is_empty());
    let report: serde_json::Value =
        serde_json::from_slice(&denied_json.stdout).expect("report should be JSON");
    assert_eq!(report["localGrantStatus"], "denied");
    assert_eq!(report["denied"][0]["capability"], "io.stdout");
    assert_eq!(report["deploymentGrantStatus"], "not-evaluated");
}

#[test]
fn sandbox_and_artifact_permissions_use_documented_usage_and_manifest_exits() {
    for arguments in [
        vec!["sandbox", "--artifact"],
        vec!["sandbox", "--unknown"],
        vec!["sandbox", "unexpected"],
        vec!["permissions", "--artifact"],
        vec!["permissions", "--artifact=a", "--artifact=b"],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }

    let directory = TestDirectory::new("sandbox-manifest-exit");
    let missing = directory.path.join("missing.pkg");
    for arguments in [
        vec![
            "sandbox".to_owned(),
            "--manifest".to_owned(),
            missing.to_string_lossy().into_owned(),
        ],
        vec![
            "permissions".to_owned(),
            "--artifact".to_owned(),
            directory
                .path
                .join("missing.wasm")
                .to_string_lossy()
                .into_owned(),
            missing.to_string_lossy().into_owned(),
        ],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(3));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .contains("error[K6001]")
        );
    }
}

#[test]
fn build_fails_closed_for_capabilities_and_unsupported_layouts() {
    let directory = TestDirectory::new("build-fail-closed");
    directory.file("main.krit", "println(1);\n");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
"#,
    );
    let output = directory.path.join("program.wasm");
    let denied = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    assert!(
        String::from_utf8(denied.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K5001]")
    );
    assert!(!output.exists());
    assert!(!metadata_path(&output).exists());

    fs::write(directory.path.join("main.krit"), "println(\"text\");\n")
        .expect("source should be replaced");
    let manifest_text = fs::read_to_string(&manifest).expect("manifest should be readable");
    fs::write(
        &manifest,
        format!("{manifest_text}\n[capabilities]\nstdout = true\n"),
    )
    .expect("manifest should be replaced");
    let unsupported = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(unsupported.stdout.is_empty());
    assert!(
        String::from_utf8(unsupported.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7002]")
    );
    assert!(!output.exists());
    assert!(!metadata_path(&output).exists());
}

#[test]
fn build_output_failure_preserves_the_previous_pair() {
    let directory = TestDirectory::new("build-atomic-failure");
    directory.file("main.krit", "println(1);\n");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );
    let output = directory.file("program.wasm", "previous component");
    let metadata = metadata_path(&output);
    fs::create_dir(&metadata).expect("metadata destination should be a directory");

    let failed = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("Krit should start");
    assert_eq!(failed.status.code(), Some(1));
    assert!(failed.stdout.is_empty());
    assert!(
        String::from_utf8(failed.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K7003]")
    );
    assert_eq!(
        fs::read_to_string(&output).expect("old component should remain"),
        "previous component"
    );
    assert!(metadata.is_dir());
    assert_eq!(
        fs::read_dir(&directory.path)
            .expect("test directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".krit-"))
            .count(),
        0,
        "failed build should not leave staged files"
    );
}

#[test]
fn build_rejects_invalid_options() {
    for arguments in [
        vec!["build", "--unknown"],
        vec!["build", "--manifest"],
        vec!["build", "--output"],
        vec!["build", "krit.pkg"],
    ] {
        let output = krit().args(arguments).output().expect("Krit should start");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .expect("diagnostic should be UTF-8")
                .starts_with("krit: ")
        );
    }
}

#[cfg(unix)]
#[test]
fn build_rejects_an_entry_symlink_that_escapes_the_package() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new("build-entry-symlink");
    let outside = directory
        .path
        .parent()
        .expect("test directory should have a parent")
        .join(format!(
            "outside-{}-{}.krit",
            std::process::id(),
            TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed)
        ));
    fs::write(&outside, "println(1);\n").expect("outside source should be written");
    symlink(&outside, directory.path.join("main.krit")).expect("entry symlink should be created");
    let manifest = directory.file(
        "krit.pkg",
        r#"
schema = 1

[package]
name = "test/program"
version = "1.0.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = true
"#,
    );

    let output = krit()
        .arg("build")
        .arg("--manifest")
        .arg(&manifest)
        .output()
        .expect("Krit should start");
    let _ = fs::remove_file(&outside);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .expect("diagnostic should be UTF-8")
            .contains("error[K6001]")
    );
    assert!(!directory.path.join("target").exists());
}

fn assert_valid_artifact(path: &Path) {
    let bytes = fs::read(path).expect("component should be readable");
    let metadata: ArtifactMetadata = serde_json::from_slice(
        &fs::read(metadata_path(path)).expect("artifact metadata should be readable"),
    )
    .expect("artifact metadata should be valid JSON");
    validate_artifact(&bytes, &metadata).expect("CLI artifact should validate");
}

fn metadata_path(path: &Path) -> PathBuf {
    let mut metadata = path.as_os_str().to_os_string();
    metadata.push(".json");
    PathBuf::from(metadata)
}

fn package_manifest(stdout: bool) -> String {
    format!(
        r#"
schema = 1

[package]
name = "test/program"
version = "1.2.3"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
stdout = {stdout}
"#
    )
}
