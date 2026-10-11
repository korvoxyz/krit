use std::{
    fs,
    path::{Path, PathBuf},
};

use krit::{Diagnostic, Severity, Source, analyze, analyze_with_warnings, parse_source};

#[test]
fn static_check_conformance_cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("conformance/check");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();

    assert!(!cases.is_empty(), "no static check cases found");
    on_compiler_stack(|| {
        for case in cases {
            run_case(&case, &root);
        }
    });
}

/// Runs `work` with the stack `krit check` gives the compiler pipeline.
fn on_compiler_stack(work: impl FnOnce() + Send) {
    krit::run_on_thread("check-conformance", krit::COMPILER_STACK_BYTES, work)
        .expect("check thread should start");
}

/// `krit run` refuses any program that fails checking, so every runtime case,
/// including those that expect a runtime error, must pass static checking.
#[test]
fn all_runtime_cases_and_examples_are_statically_valid() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_root = repository.join("conformance/cases");
    let mut cases = Vec::new();
    collect_cases(&runtime_root, &mut cases);
    cases.extend([
        repository.join("examples/factorial.krit"),
        repository.join("examples/lists.krit"),
    ]);
    cases.sort();

    on_compiler_stack(|| check_all_valid(cases));
}

fn check_all_valid(cases: Vec<PathBuf>) {
    for case in cases {
        let program_path = if case.is_dir() {
            case.join("program.krit")
        } else {
            case
        };
        let text = fs::read_to_string(&program_path).expect("program should be readable");
        let source = Source::new(program_path.to_string_lossy().into_owned(), text);
        let program = parse_source(&source).expect("valid program should parse");
        analyze(&program).unwrap_or_else(|diagnostic| {
            panic!(
                "{} should pass static checking: {}",
                program_path.display(),
                diagnostic.render_human(&source)
            )
        });
    }
}

fn collect_cases(directory: &Path, cases: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("check conformance directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            if path.join("program.krit").is_file() {
                cases.push(path);
            } else {
                collect_cases(&path, cases);
            }
        }
    }
}

fn run_case(case: &Path, root: &Path) {
    let text = fs::read_to_string(case.join("program.krit")).expect("program should be readable");
    let source = Source::new("program.krit", text);
    let (result, warnings) = match parse_source(&source) {
        Ok(program) => {
            let report = analyze_with_warnings(&program);
            (report.result.map(|_| ()), report.warnings)
        }
        Err(diagnostic) => (Err(diagnostic), Vec::new()),
    };

    let mut expected_status = fs::read_to_string(case.join("expect.status"))
        .expect("expect.status should be readable")
        .trim()
        .parse::<u8>()
        .expect("expect.status should contain a number");
    let mut expected_codes = read_optional(case.join("expect.diagnostics"))
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // Build-target diagnostics (`K7xxx`) come from the WebAssembly backend,
    // which `krit check` runs for entrypoint files after front-end analysis.
    // This crate has no backend, so such cases must pass the front end here;
    // the CLI conformance harness checks the target diagnostics themselves.
    if !expected_codes.is_empty() && expected_codes.iter().all(|code| code.starts_with("K7")) {
        expected_status = 0;
        expected_codes.clear();
    }
    let expected_warnings = read_optional(case.join("expect.warnings"))
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let actual_warnings = warnings
        .iter()
        .map(|warning| warning.code().to_owned())
        .collect::<Vec<_>>();
    let case_name = case
        .strip_prefix(root)
        .unwrap_or(case)
        .display()
        .to_string();
    if let Err(diagnostic) = &result {
        assert_notes(diagnostic, case, &case_name);
    }
    let (actual_status, actual_codes) = match result {
        Ok(_) => (0, Vec::new()),
        Err(diagnostic) => (1, vec![diagnostic.code().to_owned()]),
    };

    assert_eq!(
        actual_status, expected_status,
        "status mismatch in {case_name}"
    );
    assert_eq!(
        actual_codes, expected_codes,
        "diagnostic mismatch in {case_name}"
    );
    assert_eq!(
        actual_warnings, expected_warnings,
        "warning mismatch in {case_name}"
    );
    assert!(
        warnings
            .iter()
            .all(|warning| warning.severity() == Severity::Warning),
        "analysis returned a non-warning beside its result in {case_name}"
    );
}

/// The error carries the number of notes in `expect.notes`, which defaults to
/// exactly one fix for habit codes (`K1101`-`K1112`) and method calls
/// (`K3011`) and to none otherwise (`spec/DIAGNOSTICS.md`). Every note is one
/// line; its wording may vary.
fn assert_notes(diagnostic: &Diagnostic, case: &Path, case_name: &str) {
    let code = diagnostic.code();
    let requires_hint = code == "K3011"
        || code
            .strip_prefix("K11")
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| (1..=12).contains(&number));
    let expected = read_optional(case.join("expect.notes"));
    let expected = if expected.is_empty() {
        usize::from(requires_hint)
    } else {
        expected
            .trim()
            .parse::<usize>()
            .expect("expect.notes should contain a number")
    };
    assert_eq!(
        diagnostic.notes().len(),
        expected,
        "note count of {code} in {case_name}"
    );
    assert!(
        diagnostic
            .notes()
            .iter()
            .all(|note| !note.is_empty() && !note.contains(['\n', '\r'])),
        "every note in {case_name} is one line"
    );
}

fn read_optional(path: PathBuf) -> String {
    if path.is_file() {
        fs::read_to_string(path).expect("expectation should be readable")
    } else {
        String::new()
    }
}
