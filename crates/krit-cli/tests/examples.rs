//! The release gate for the shipped examples (PRD 0.3 §5, gate 2) and the
//! conformance floor (PRD 0.3 §6, R5.3).
//!
//! Every example is canonically formatted and checks without a diagnostic.
//! Every example except the documented exceptions in [`UNBUILDABLE_EXAMPLES`]
//! passes the component target check and builds from the manifest whose
//! entry it is: its own `examples/<name>.krit.pkg`, or the repository's root
//! `krit.pkg` for `examples/factorial.krit`. `scripts/metrics.py` keeps every
//! example at depth 3 or less, `examples/webhook-agent.krit` stays within 50
//! lines, and the conformance suites keep at least 40 run, 60 check, and 8
//! format cases.

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

/// Examples that `krit build` cannot compile, each an exception to gate 2
/// that the release owner accepts or resolves.
///
/// `examples/lists.krit` sums a `List<Int>` by list matching and prints the
/// list. WebAssembly policy 1 has no guest layout for `List<Int>` and no list
/// matching (both 0.4 scope), so the example runs with `krit run` only. The
/// list is exact: a backend that starts building an exception, or a new
/// example that cannot build, fails this gate until the list is updated.
const UNBUILDABLE_EXAMPLES: &[&str] = &["examples/lists.krit"];

/// The PRD 0.3 floor for each conformance suite: its directory under
/// `conformance/`, the file that marks a case directory, and the minimum
/// number of cases.
const CONFORMANCE_FLOOR: [(&str, &str, usize); 3] = [
    ("cases", "program.krit", 40),
    ("check", "program.krit", 60),
    ("format", "input.krit", 8),
];

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

fn is_unbuildable(example: &str) -> bool {
    UNBUILDABLE_EXAMPLES.contains(&example)
}

/// The manifest that builds `example`: `examples/<name>.krit.pkg` or the
/// root `krit.pkg`, whichever names the example as its entry.
fn manifest_for(example: &str) -> Option<String> {
    [format!("{example}.pkg"), "krit.pkg".to_owned()]
        .into_iter()
        .find(|manifest| {
            let Ok(text) = fs::read_to_string(repository_root().join(manifest)) else {
                return false;
            };
            let parsed = krit_package::Manifest::parse(&text)
                .unwrap_or_else(|error| panic!("{manifest} should parse: {error}"));
            let directory = Path::new(manifest).parent().unwrap_or(Path::new(""));
            directory.join(&parsed.package.entry) == Path::new(example)
        })
}

/// The component target check runs the whole WebAssembly pipeline that
/// `krit build` runs, so an example fails it exactly when it cannot build.
/// Only the documented exceptions may fail, and only on WebAssembly policy
/// limits (`K7002`), never on a source error.
#[test]
fn only_the_documented_exceptions_fail_the_component_target_check() {
    let mut unbuildable = Vec::new();
    for example in examples() {
        let check = krit()
            .args([
                "check",
                "--target",
                "component",
                "--diagnostic-format",
                "json",
                &example,
            ])
            .output()
            .expect("check should start");
        let diagnostics = String::from_utf8(check.stderr).expect("diagnostics should be UTF-8");
        if check.status.success() {
            assert!(
                diagnostics.is_empty(),
                "{example} should pass the component target check without diagnostics: \
                 {diagnostics}"
            );
            continue;
        }
        let codes = diagnostics
            .lines()
            .map(|line| {
                let diagnostic = serde_json::from_str::<serde_json::Value>(line)
                    .expect("diagnostic should be JSON");
                diagnostic["code"].as_str().unwrap_or_default().to_owned()
            })
            .collect::<Vec<_>>();
        assert!(
            !codes.is_empty() && codes.iter().all(|code| code == "K7002"),
            "{example} should fail the component target check only on WebAssembly policy \
             limits (K7002): {diagnostics}"
        );
        unbuildable.push(example);
    }
    assert_eq!(
        unbuildable, UNBUILDABLE_EXAMPLES,
        "the examples that cannot build should be exactly the documented exceptions"
    );
}

/// An example that cannot build is still a working `krit run` program.
#[test]
fn every_unbuildable_example_runs() {
    for example in UNBUILDABLE_EXAMPLES {
        let run = krit()
            .args(["run", example])
            .output()
            .expect("run should start");
        assert!(
            run.status.success(),
            "{example} should run: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(
            run.stderr.is_empty(),
            "{example} should run without diagnostics: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

#[test]
fn every_buildable_example_builds_from_its_manifest() {
    let directory = ScratchDirectory::new("build");
    let examples = examples();
    let mut built = 0;
    for example in &examples {
        if is_unbuildable(example) {
            continue;
        }
        let manifest = manifest_for(example)
            .unwrap_or_else(|| panic!("{example} should be the entry of a manifest"));
        let artifact = directory.path.join(format!("{built}.wasm"));
        let build = krit()
            .args(["build", "--manifest", &manifest, "--output"])
            .arg(&artifact)
            .output()
            .expect("build should start");
        assert!(
            build.status.success(),
            "{manifest} should build {example}: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        assert!(artifact.is_file(), "{manifest} should write its artifact");
        built += 1;
    }
    assert_eq!(
        built + UNBUILDABLE_EXAMPLES.len(),
        examples.len(),
        "every example but the documented exceptions should be built"
    );
}

/// The number of case directories under `directory`: those holding `marker`.
fn count_cases(directory: &Path, marker: &str) -> usize {
    if directory.join(marker).is_file() {
        return 1;
    }
    fs::read_dir(directory)
        .expect("conformance directory should be readable")
        .map(|entry| entry.expect("directory entry should be readable").path())
        .filter(|path| path.is_dir())
        .map(|path| count_cases(&path, marker))
        .sum()
}

#[test]
fn conformance_suites_meet_the_release_floor() {
    for (suite, marker, floor) in CONFORMANCE_FLOOR {
        let count = count_cases(&repository_root().join("conformance").join(suite), marker);
        assert!(
            count >= floor,
            "conformance/{suite} has {count} cases; PRD 0.3 requires at least {floor}"
        );
    }
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
