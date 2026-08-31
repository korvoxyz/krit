use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

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
