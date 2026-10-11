//! `krit check --target component` must report exactly the backend
//! diagnostics (`K7001`/`K7002`, codes and spans) that `krit build` reports,
//! for every example, every conformance program, and the target probes.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use krit::{Source, analyze, lower, parse_source};

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

/// One reported backend diagnostic: code, start byte, end byte.
type TargetDiagnostic = (String, u64, u64);

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("target-{name}-{}-{id}", std::process::id()));
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

fn json_lines(output: &Output) -> Vec<serde_json::Value> {
    String::from_utf8(output.stderr.clone())
        .expect("diagnostics should be UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("diagnostic should be one JSON object"))
        .collect()
}

fn codes(output: &Output) -> Vec<String> {
    severity_codes(output, "error")
}

/// The codes of the reported diagnostics with `severity`, in output order.
fn severity_codes(output: &Output, severity: &str) -> Vec<String> {
    json_lines(output)
        .iter()
        .filter(|diagnostic| diagnostic["severity"] == severity)
        .map(|diagnostic| {
            diagnostic["code"]
                .as_str()
                .expect("diagnostic should have a code")
                .to_owned()
        })
        .collect()
}

fn target_diagnostics(output: &Output) -> Vec<TargetDiagnostic> {
    json_lines(output)
        .iter()
        .filter(|diagnostic| matches!(diagnostic["code"].as_str(), Some("K7001" | "K7002")))
        .map(|diagnostic| {
            (
                diagnostic["code"].as_str().unwrap_or_default().to_owned(),
                diagnostic["span"]["start"]["byte"]
                    .as_u64()
                    .expect("diagnostic should have a start byte"),
                diagnostic["span"]["end"]["byte"]
                    .as_u64()
                    .expect("diagnostic should have an end byte"),
            )
        })
        .collect()
}

fn corpus() -> Vec<PathBuf> {
    let root = repository_root();
    let mut files = Vec::new();
    for directory in ["examples", "crates/krit-cli/tests/target-probes"] {
        for entry in fs::read_dir(root.join(directory)).expect("corpus directory should exist") {
            let path = entry.expect("directory entry should be readable").path();
            if path
                .extension()
                .is_some_and(|extension| extension == "krit")
            {
                files.push(path);
            }
        }
    }
    collect_programs(&root.join("conformance/cases"), &mut files);
    collect_programs(&root.join("conformance/check"), &mut files);
    files.sort();
    files
}

fn collect_programs(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("conformance directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            let program = path.join("program.krit");
            if program.is_file() {
                files.push(program);
            } else {
                collect_programs(&path, files);
            }
        }
    }
}

/// Writes a package for `program` that grants every capability its analysis
/// requires, so that `build` reaches the backend instead of stopping at
/// `K5001`. Programs that fail analysis get an empty grant set.
fn package_for(program: &Path, text: &str, directory: &ScratchDirectory) -> PathBuf {
    let mut grants = BTreeMap::<&'static str, BTreeSet<String>>::new();
    // The corpus includes the deepest programs the parser accepts, so the
    // compiler stages run on the stack `krit` gives them.
    let requirements = krit::run_on_thread("package-for", krit::COMPILER_STACK_BYTES, || {
        required_capabilities(text)
    })
    .expect("analysis thread should start");
    for (capability, resource) in requirements {
        let key = match capability.as_str() {
            "config.read" => "config",
            "http.request" => "http",
            "secret.read" => "secrets",
            "ai.invoke" => "ai",
            "state.transaction" => "state",
            "queue.publish" => "queues",
            "queue.consume" => "consumes",
            "schedule.trigger" => "schedules",
            "object.read" | "object.write" => "buckets",
            "cache.read" | "cache.write" => "cacheNamespaces",
            "search.query" => "searchIndexes",
            "search.vector" => "vectorIndexes",
            "database.read" | "database.write" => "databases",
            other => panic!("{}: unmapped capability `{other}`", program.display()),
        };
        grants.entry(key).or_default().insert(resource);
    }
    let mut manifest = String::from(
        "schema = 1\n\n[package]\nname = \"test/target-agreement\"\nversion = \"1.0.0\"\n\
         edition = \"2026\"\nentry = \"main.krit\"\nlicense = \"Apache-2.0\"\n\n\
         [capabilities]\nstdout = true\nlogs = true\n",
    );
    for (key, resources) in grants {
        let resources = resources
            .iter()
            .map(|resource| format!("{resource:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        manifest.push_str(&format!("{key} = [{resources}]\n"));
    }
    fs::write(directory.path.join("main.krit"), text).expect("program should be copied");
    let manifest_path = directory.path.join("krit.pkg");
    fs::write(&manifest_path, manifest).expect("manifest should be written");
    manifest_path
}

/// The capability requirements of `text`'s analysis and entrypoints, or none
/// when it fails analysis.
fn required_capabilities(text: &str) -> BTreeSet<(String, String)> {
    let source = Source::new("main.krit", text);
    let mut requirements = BTreeSet::new();
    if let Ok(parsed) = parse_source(&source)
        && let Ok(analysis) = analyze(&parsed)
    {
        for expression in analysis.expressions() {
            for requirement in expression.requirements().iter() {
                requirements.insert((
                    requirement.capability().as_str().to_owned(),
                    requirement.resource().to_owned(),
                ));
            }
        }
        // Entrypoint declarations add requirements (such as the consumed
        // queue) that no expression carries.
        if let Ok(module) = lower(&parsed, &analysis) {
            for function in module.functions() {
                for requirement in function.signature.requirements.iter() {
                    requirements.insert((
                        requirement.capability().as_str().to_owned(),
                        requirement.resource().to_owned(),
                    ));
                }
            }
        }
    }
    requirements
}

fn build(program: &Path, directory: &ScratchDirectory) -> Output {
    let checked_in = program.with_extension("krit.pkg");
    let manifest = if checked_in.is_file() {
        checked_in
    } else {
        let text = fs::read_to_string(program).expect("program should be readable");
        package_for(program, &text, directory)
    };
    krit()
        .arg("build")
        .arg("--diagnostic-format")
        .arg("json")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(directory.path.join("artifact.wasm"))
        .output()
        .expect("build should start")
}

fn check(program: &Path, target: &str) -> Output {
    krit()
        .args(["check", "--diagnostic-format", "json", "--target", target])
        .arg(program)
        .output()
        .expect("check should start")
}

#[test]
fn check_target_component_reports_exactly_the_build_diagnostics() {
    let files = corpus();
    assert!(files.len() >= 60, "corpus is unexpectedly small");
    let mut failing = 0;
    for (index, program) in files.iter().enumerate() {
        let name = program
            .strip_prefix(repository_root())
            .unwrap_or(program)
            .display()
            .to_string();
        let directory = ScratchDirectory::new(&index.to_string());
        let checked = check(program, "component");
        let built = build(program, &directory);

        let build_codes = codes(&built);
        assert!(
            !build_codes
                .iter()
                .any(|code| matches!(code.as_str(), "K5001" | "K6001")),
            "{name}: build stopped before the backend: {build_codes:?}"
        );
        let expected = target_diagnostics(&built);
        let actual = target_diagnostics(&checked);
        assert_eq!(
            actual, expected,
            "{name}: check --target disagrees with build"
        );
        assert_eq!(
            checked.status.success(),
            built.status.success(),
            "{name}: check and build disagree on success: {:?}",
            codes(&checked)
        );
        for (code, start, end) in &actual {
            assert!(
                (*start, *end) != (0, 0),
                "{name}: {code} must carry the originating source span"
            );
        }
        if !actual.is_empty() {
            failing += 1;
        }
    }
    assert!(failing >= 10, "the corpus should exercise backend failures");
}

#[test]
fn entrypoint_files_are_checked_as_components_by_default() {
    let probe =
        repository_root().join("crates/krit-cli/tests/target-probes/webhook-unsupported-mix.krit");
    let default = krit()
        .args(["check", "--diagnostic-format", "json"])
        .arg(&probe)
        .output()
        .expect("check should start");
    let explicit = check(&probe, "component");
    assert_eq!(default.status.code(), Some(1));
    assert!(default.stdout.is_empty());
    assert_eq!(default.stderr, explicit.stderr);
    let reported = target_diagnostics(&default);
    assert!(
        reported.len() >= 5,
        "every unsupported construct should be reported: {reported:?}"
    );
    let starts = reported
        .iter()
        .map(|(_, start, _)| *start)
        .collect::<Vec<_>>();
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(starts, sorted, "diagnostics should be sorted by span");

    let none = check(&probe, "none");
    assert!(none.status.success());
    assert!(none.stderr.is_empty());

    let module = check(&probe, "module");
    assert_eq!(module.status.code(), Some(1));
    assert_eq!(codes(&module), ["K7002"]);
}

#[test]
fn module_programs_skip_target_checks_unless_requested() {
    let lists = repository_root().join("examples/lists.krit");
    let default = krit()
        .arg("check")
        .arg(&lists)
        .output()
        .expect("check should start");
    assert!(default.status.success());
    assert!(default.stderr.is_empty());

    for target in ["component", "module"] {
        let targeted = check(&lists, target);
        assert_eq!(targeted.status.code(), Some(1), "{target}");
        assert!(targeted.stdout.is_empty(), "{target}");
        assert!(
            codes(&targeted).iter().all(|code| code == "K7002"),
            "{target}"
        );
    }

    let human = krit()
        .args(["check", "--target", "component"])
        .arg(&lists)
        .output()
        .expect("check should start");
    let stderr = String::from_utf8(human.stderr).expect("diagnostics should be UTF-8");
    assert!(stderr.lines().count() >= 3, "{stderr}");
    assert!(
        stderr
            .lines()
            .all(|line| line.contains(": error[K7002]: ") && line.contains("policy 1")),
        "{stderr}"
    );
}

#[test]
fn dead_helpers_no_longer_fail_entrypoint_builds() {
    let probe = repository_root()
        .join("crates/krit-cli/tests/target-probes/webhook-dead-helpers-builds.krit");
    let directory = ScratchDirectory::new("dead-helpers");
    let built = build(&probe, &directory);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(directory.path.join("artifact.wasm").is_file());
    let checked = krit()
        .arg("check")
        .arg(&probe)
        .output()
        .expect("check should start");
    assert!(checked.status.success());
}

#[test]
fn string_routing_builds_and_checks_as_a_component() {
    let probe = repository_root()
        .join("crates/krit-cli/tests/target-probes/webhook-routing-strings-builds.krit");
    let directory = ScratchDirectory::new("string-routing");
    let built = build(&probe, &directory);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(directory.path.join("artifact.wasm").is_file());
    let checked = krit()
        .args(["check", "--diagnostic-format", "json"])
        .arg(&probe)
        .output()
        .expect("check should start");
    assert!(checked.status.success());
    assert!(checked.stderr.is_empty());
}

#[test]
fn build_reports_every_diagnostic_in_json() {
    let probe =
        repository_root().join("crates/krit-cli/tests/target-probes/webhook-unsupported-mix.krit");
    let directory = ScratchDirectory::new("build-json");
    let text = fs::read_to_string(&probe).expect("probe should be readable");
    let manifest = package_for(&probe, &text, &directory);

    let json = krit()
        .args(["build", "--diagnostic-format=json", "--manifest"])
        .arg(&manifest)
        .output()
        .expect("build should start");
    let human = krit()
        .args(["build", "--manifest"])
        .arg(&manifest)
        .output()
        .expect("build should start");
    assert_eq!(json.status.code(), Some(1));
    assert_eq!(json.status.code(), human.status.code());
    let diagnostics = json_lines(&json);
    assert!(
        diagnostics.len() >= 5,
        "every unsupported construct should be reported: {diagnostics:?}"
    );
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic["schema"], 1);
        assert_eq!(diagnostic["file"], "main.krit");
        assert!(diagnostic["span"]["start"]["line"].as_u64() > Some(1));
    }
    assert_eq!(
        String::from_utf8(human.stderr)
            .expect("diagnostics should be UTF-8")
            .lines()
            .count(),
        diagnostics.len()
    );

    let missing = krit()
        .args(["build", "--diagnostic-format", "json", "--manifest"])
        .arg(directory.path.join("missing.pkg"))
        .output()
        .expect("build should start");
    assert_eq!(missing.status.code(), Some(3));
    assert_eq!(codes(&missing), ["K6001"]);
}

fn read_expectation(path: PathBuf) -> String {
    if path.is_file() {
        fs::read_to_string(path).expect("expectation should be readable")
    } else {
        String::new()
    }
}

/// Runs the static conformance suite through the real CLI, including the
/// default component target for entrypoint files.
#[test]
fn static_conformance_cases_pass_through_the_cli() {
    let root = repository_root().join("conformance/check");
    let mut programs = Vec::new();
    collect_programs(&root, &mut programs);
    programs.sort();
    assert!(
        programs
            .iter()
            .any(|program| program.starts_with(root.join("target"))),
        "target conformance cases should exist"
    );
    for program in programs {
        let case = program.parent().expect("program should be inside a case");
        let name = case
            .strip_prefix(&root)
            .unwrap_or(case)
            .display()
            .to_string();
        let output = krit()
            .args(["check", "--diagnostic-format", "json"])
            .arg(&program)
            .output()
            .expect("check should start");
        let expected_status = read_expectation(case.join("expect.status"))
            .trim()
            .parse::<i32>()
            .expect("expect.status should contain a number");
        let expected_codes = read_expectation(case.join("expect.diagnostics"))
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let expected_warnings = read_expectation(case.join("expect.warnings"))
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            output.status.code(),
            Some(expected_status),
            "{name}: status"
        );
        assert_eq!(codes(&output), expected_codes, "{name}: diagnostics");
        assert_eq!(
            severity_codes(&output, "warning"),
            expected_warnings,
            "{name}: warnings"
        );
        assert_eq!(
            json_lines(&output).len(),
            expected_codes.len() + expected_warnings.len(),
            "{name}: every diagnostic is an error or a warning"
        );
        if expected_status == 0 {
            assert!(
                expected_warnings.is_empty() == output.stderr.is_empty(),
                "{name}: unexpected diagnostics"
            );
        } else {
            assert!(
                output.stdout.is_empty(),
                "{name}: check must not print success"
            );
        }
    }
}
