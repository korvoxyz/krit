use std::{
    fs,
    path::{Path, PathBuf},
};

use krit::{Source, analyze, parse_source};

#[test]
fn static_check_conformance_cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("conformance/check");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();

    assert!(!cases.is_empty(), "no static check cases found");
    for case in cases {
        run_case(&case, &root);
    }
}

#[test]
fn all_runtime_success_cases_and_examples_are_statically_valid() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_root = repository.join("conformance/cases");
    let mut cases = Vec::new();
    collect_cases(&runtime_root, &mut cases);
    cases.retain(|case| {
        fs::read_to_string(case.join("expect.status"))
            .expect("expect.status should be readable")
            .trim()
            == "0"
    });
    cases.extend([
        repository.join("examples/factorial.krit"),
        repository.join("examples/lists.krit"),
    ]);
    cases.sort();

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
    let result = parse_source(&source).and_then(|program| analyze(&program));

    let expected_status = fs::read_to_string(case.join("expect.status"))
        .expect("expect.status should be readable")
        .trim()
        .parse::<u8>()
        .expect("expect.status should contain a number");
    let expected_codes = read_optional(case.join("expect.diagnostics"))
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let (actual_status, actual_codes) = match result {
        Ok(_) => (0, Vec::new()),
        Err(diagnostic) => (1, vec![diagnostic.code().to_owned()]),
    };
    let case_name = case
        .strip_prefix(root)
        .unwrap_or(case)
        .display()
        .to_string();

    assert_eq!(
        actual_status, expected_status,
        "status mismatch in {case_name}"
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
