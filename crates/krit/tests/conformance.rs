use std::{
    fs,
    path::{Path, PathBuf},
};

use krit::{Source, run_source};

#[test]
fn normative_conformance_cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("conformance/cases");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();

    assert!(!cases.is_empty(), "no conformance cases found");
    for case in cases {
        run_case(&case);
    }
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

fn run_case(case: &Path) {
    let program_path = case.join("program.krit");
    let text = fs::read_to_string(&program_path).expect("program should be readable");
    let source = Source::new("program.krit", text);
    let mut output = Vec::new();
    let result = run_source(&source, &mut output);

    let expected_status = fs::read_to_string(case.join("expect.status"))
        .expect("expect.status should be readable")
        .trim()
        .parse::<u8>()
        .expect("expect.status should contain a number");
    let expected_output = read_optional(case.join("expect.stdout"));
    let expected_codes = read_optional(case.join("expect.diagnostics"))
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let (actual_status, actual_codes) = match result {
        Ok(_) => (0, Vec::new()),
        Err(diagnostic) => (1, vec![diagnostic.code().to_owned()]),
    };
    let actual_output = String::from_utf8(output).expect("Krit output should be UTF-8");
    let case_name = case
        .strip_prefix(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("conformance/cases"),
        )
        .unwrap_or(case)
        .display()
        .to_string();

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

fn read_optional(path: PathBuf) -> String {
    if path.is_file() {
        fs::read_to_string(path).expect("expectation should be readable")
    } else {
        String::new()
    }
}
