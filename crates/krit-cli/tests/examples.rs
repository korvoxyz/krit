//! The release gate for the shipped examples (PRD 0.3 §5, gate 2): every
//! example is canonically formatted, checks without a diagnostic, and builds
//! from its manifest; `scripts/metrics.py` keeps every example at depth 3 or
//! less; and `examples/webhook-agent.krit` stays within 50 lines.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

const MAX_DEPTH: &str = "3";
const WEBHOOK_AGENT_MAX_LINES: usize = 50;

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("examples-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
    }
}

impl Drop for ScratchDirectory {
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

/// Every `examples/*.krit` source, relative to the repository root.
fn examples() -> Vec<String> {
    let mut files = fs::read_dir(repository_root().join("examples"))
        .expect("examples should be readable")
        .map(|entry| entry.expect("directory entry should be readable").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "krit")
        })
        .map(|path| {
            format!(
                "examples/{}",
                path.file_name()
                    .expect("example should have a name")
                    .to_string_lossy()
            )
        })
        .collect::<Vec<_>>();
    files.sort();
    assert!(files.len() >= 9, "examples should be found: {files:?}");
    files
}

#[test]
fn every_example_is_canonical_and_checks_without_diagnostics() {
    for example in examples() {
        let format = krit()
            .args(["fmt", "--check", &example])
            .output()
            .expect("fmt should start");
        assert!(
            format.status.success(),
            "{example} should be canonically formatted: {}",
            String::from_utf8_lossy(&format.stderr)
        );

        let check = krit()
            .args(["check", &example])
            .output()
            .expect("check should start");
        assert!(
            check.status.success(),
            "{example} should check: {}",
            String::from_utf8_lossy(&check.stderr)
        );
        assert!(
            check.stderr.is_empty(),
            "{example} should check without warnings: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }
}

#[test]
fn every_example_package_builds_from_its_manifest() {
    let directory = ScratchDirectory::new("build");
    let mut built = 0;
    for example in examples() {
        let manifest = format!("{example}.pkg");
        if !repository_root().join(&manifest).is_file() {
            continue;
        }
        let artifact = directory.path.join(format!("{built}.wasm"));
        let build = krit()
            .args(["build", "--manifest", &manifest, "--output"])
            .arg(&artifact)
            .output()
            .expect("build should start");
        assert!(
            build.status.success(),
            "{manifest} should build: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        assert!(artifact.is_file(), "{manifest} should write its artifact");
        built += 1;
    }
    assert_eq!(built, 7, "every example package should be built");
}

#[test]
fn webhook_agent_stays_within_fifty_lines() {
    let text = fs::read_to_string(repository_root().join("examples/webhook-agent.krit"))
        .expect("webhook agent should be readable");
    let lines = text.lines().count();
    assert!(
        lines <= WEBHOOK_AGENT_MAX_LINES,
        "examples/webhook-agent.krit has {lines} lines; the 0.3 gate allows \
         {WEBHOOK_AGENT_MAX_LINES}"
    );
}

/// Whether a working `python3` is on the path. A missing interpreter, or a
/// launcher stub that cannot run code, skips the metrics check instead of
/// failing it; CI runs the same check in its quality job.
fn python_available() -> bool {
    match Command::new("python3")
        .args(["-I", "-c", "import sys; sys.exit(0)"])
        .output()
    {
        Ok(output) => output.status.success(),
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => panic!("python3 probe should start or be missing: {error}"),
    }
}

#[test]
fn metrics_keep_every_example_at_depth_three_or_less() {
    if !python_available() {
        eprintln!("skipping the example depth check: python3 is not available");
        return;
    }
    let output = Command::new("python3")
        .current_dir(repository_root())
        .args([
            "-I",
            "scripts/metrics.py",
            "--check-depth",
            MAX_DEPTH,
            "examples",
        ])
        .output()
        .expect("python3 should start");
    assert!(
        output.status.success(),
        "scripts/metrics.py --check-depth {MAX_DEPTH} examples failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let table = String::from_utf8(output.stdout).expect("metrics should be UTF-8");
    for example in examples() {
        assert!(
            table.contains(&example),
            "{example} should be measured:\n{table}"
        );
    }
}

/// The checked-in host file configures no secret, so the default invocation
/// fails safely as an application response before any network call, exactly
/// as the README states.
#[cfg(unix)]
#[test]
fn webhook_agent_fixture_fails_safely_without_secrets() {
    let directory = ScratchDirectory::new("webhook-agent");
    let artifact = directory.path.join("webhook-agent.wasm");
    let build = krit()
        .args([
            "build",
            "--manifest",
            "examples/webhook-agent.krit.pkg",
            "--output",
        ])
        .arg(&artifact)
        .output()
        .expect("build should start");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let invoke = krit()
        .args([
            "invoke",
            "--manifest",
            "examples/webhook-agent.krit.pkg",
            "--host-config",
            "examples/webhook-agent.host.json",
            "--request",
            "examples/webhook-agent.request.json",
            "--artifact",
        ])
        .arg(&artifact)
        .output()
        .expect("invoke should start");
    assert!(
        invoke.status.success(),
        "{}",
        String::from_utf8_lossy(&invoke.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&invoke.stdout),
        "{\"status\":502,\"headers\":[],\"body\":\"reference flow failed\"}\n"
    );
    let events = String::from_utf8(invoke.stderr)
        .expect("log events should be UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("event should be JSON"))
        .map(|event| {
            (
                event["level"].as_str().unwrap_or_default().to_owned(),
                event["event"].as_str().unwrap_or_default().to_owned(),
                event["fields"].to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            (
                "info".to_owned(),
                "reference.received".to_owned(),
                r#"[{"name":"delivery","value":"/webhook"}]"#.to_owned(),
            ),
            (
                "error".to_owned(),
                "reference.failed".to_owned(),
                r#"[{"name":"reason","value":"secret `github-token` is not configured"}]"#
                    .to_owned(),
            ),
        ]
    );
}
