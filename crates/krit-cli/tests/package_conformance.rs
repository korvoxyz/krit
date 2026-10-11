use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn package_conformance_cases() {
    let root = repository_root().join("conformance/package");
    let mut cases = Vec::new();
    collect_cases(&root, &mut cases);
    cases.sort();

    assert!(!cases.is_empty(), "no package conformance cases found");
    for case in cases {
        run_case(&case, &root);
    }
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CLI crate should be inside the workspace")
}

fn collect_cases(directory: &Path, cases: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("package conformance directory should be readable")
    {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            if path.join("krit.pkg").is_file() {
                cases.push(path);
            } else {
                collect_cases(&path, cases);
            }
        }
    }
}

fn run_case(case: &Path, root: &Path) {
    let case_name = case
        .strip_prefix(root)
        .unwrap_or(case)
        .display()
        .to_string();
    // Run from the case directory with a relative manifest so neither the
    // diagnostics nor the outcome depend on the host checkout path.
    let output = Command::new(env!("CARGO_BIN_EXE_krit"))
        .args(["package", "check", "krit.pkg"])
        .current_dir(case)
        .output()
        .expect("Krit should start");

    let expected_status = fs::read_to_string(case.join("expect.status"))
        .expect("expect.status should be readable")
        .trim()
        .parse::<i32>()
        .expect("expect.status should contain a number");
    let expected_codes = read_optional(&case.join("expect.diagnostics"))
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let stderr = String::from_utf8(output.stderr).expect("diagnostics should be UTF-8");
    let actual_codes = stderr
        .lines()
        .map(|line| {
            diagnostic_code(line)
                .unwrap_or_else(|| panic!("{case_name}: stderr line is not a diagnostic: {line}"))
                .to_owned()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        output.status.code(),
        Some(expected_status),
        "status mismatch in {case_name}"
    );
    assert_eq!(
        actual_codes, expected_codes,
        "diagnostic mismatch in {case_name}"
    );
}

/// Extracts `CODE` from the normative human first line
/// `file:line:column: severity[CODE]: message`.
fn diagnostic_code(line: &str) -> Option<&str> {
    let (head, _) = line.split_once("]: ")?;
    let (_, code) = head.rsplit_once('[')?;
    Some(code)
}

fn read_optional(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => panic!("{} should be readable: {error}", path.display()),
    }
}
