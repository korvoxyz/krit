use std::{
    fs,
    path::{Path, PathBuf},
};

use krit::{Source, analyze, lower, parse_source, run_source};

#[test]
fn normative_conformance_cases() {
    let root = conformance_root("cases");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();

    assert!(!cases.is_empty(), "no conformance cases found");
    // `krit run` evaluates on a thread with this stack, so the depth limits
    // are reached before the native stack is exhausted.
    krit::run_on_thread("conformance", krit::EVALUATOR_STACK_BYTES, || {
        for case in cases {
            run_case(&case, &root);
        }
    })
    .expect("conformance thread should start");
}

/// `run` performs the same analysis as `check`, so every program that check
/// rejects is refused by run with the same diagnostic and no output.
#[test]
fn run_refuses_every_check_failure() {
    let root = conformance_root("check");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.retain(|case| read_status(case) != 0);
    cases.sort();

    assert!(!cases.is_empty(), "no failing check cases found");
    krit::run_on_thread("check-refusal", krit::EVALUATOR_STACK_BYTES, || {
        for case in cases {
            let case_name = case_name(&case, &root);
            let source = read_program(&case);
            let mut output = Vec::new();
            let Err(diagnostic) = run_source(&source, &mut output) else {
                panic!("run should refuse {case_name}");
            };
            assert_eq!(
                vec![diagnostic.code().to_owned()],
                read_codes(&case),
                "diagnostic mismatch in {case_name}"
            );
            assert!(output.is_empty(), "run printed output for {case_name}");
        }
    })
    .expect("check refusal thread should start");
}

fn conformance_root(suite: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("conformance")
        .join(suite)
}

fn collect_cases(directory: &Path, cases: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("conformance directory should be readable") {
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
    let case_name = case_name(case, root);
    let source = read_program(case);
    assert_passes_check(&source, &case_name);

    let mut output = Vec::new();
    let result = run_source(&source, &mut output);

    let expected_status = read_status(case);
    let expected_output = read_optional(case.join("expect.stdout"));
    let expected_codes = read_codes(case);

    let (actual_status, actual_codes) = match result {
        Ok(_) => (0, Vec::new()),
        Err(diagnostic) => (1, vec![diagnostic.code().to_owned()]),
    };
    let actual_output = String::from_utf8(output).expect("Krit output should be UTF-8");

    assert_eq!(
        actual_status, expected_status,
        "status mismatch in {case_name}"
    );
    assert_eq!(
        actual_output, expected_output,
        "stdout mismatch in {case_name}"
    );
    assert_eq!(
        actual_codes, expected_codes,
        "diagnostic mismatch in {case_name}"
    );
}

/// Runtime cases define evaluation behavior, so each must pass `krit check`
/// (parsing, analysis, and Core lowering). A program that check rejects never
/// reaches the evaluator and belongs in `conformance/check` instead.
fn assert_passes_check(source: &Source, case_name: &str) {
    let program = parse_source(source).and_then(|program| {
        let analysis = analyze(&program)?;
        Ok((program, analysis))
    });
    let (program, analysis) = program.unwrap_or_else(|diagnostic| {
        panic!(
            "runtime case {case_name} must pass check; move it to conformance/check: {}",
            diagnostic.render_human(source)
        )
    });
    if let Err(error) = lower(&program, &analysis) {
        panic!("runtime case {case_name} should lower to Core IR: {error}");
    }
}

fn case_name(case: &Path, root: &Path) -> String {
    case.strip_prefix(root)
        .unwrap_or(case)
        .display()
        .to_string()
}

fn read_program(case: &Path) -> Source {
    let text = fs::read_to_string(case.join("program.krit")).expect("program should be readable");
    Source::new("program.krit", text)
}

fn read_status(case: &Path) -> u8 {
    fs::read_to_string(case.join("expect.status"))
        .expect("expect.status should be readable")
        .trim()
        .parse::<u8>()
        .expect("expect.status should contain a number")
}

fn read_codes(case: &Path) -> Vec<String> {
    read_optional(case.join("expect.diagnostics"))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn read_optional(path: PathBuf) -> String {
    if path.is_file() {
        fs::read_to_string(path).expect("expectation should be readable")
    } else {
        String::new()
    }
}
