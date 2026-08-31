use std::{path::Path, process::Command};

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
