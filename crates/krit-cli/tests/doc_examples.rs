//! Every `krit` code block in `README.md` and in `spec/*.md` is a complete
//! program that the real `krit check` accepts without any diagnostic, so the
//! documentation cannot drift from the compiler. A block followed directly by
//! a `text` block is also run, and its standard output must equal that block.
//!
//! Signatures and fragments are written as `text` blocks instead. The layout
//! examples of the formatter section of `spec/LANGUAGE.md` ("16. Canonical
//! source format") show fragments and are exempt.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// A `krit` code block of a Markdown document.
struct Example {
    /// `path:line` of the block's first source line, for failure messages.
    location: String,
    source: String,
    /// The contents of a `text` block that directly follows the block: the
    /// standard output `krit run` must print.
    expected_output: Option<String>,
}

/// The heading whose section holds formatter layout fragments.
const FORMATTER_SECTION: &str = "## 16. ";

#[test]
fn readme_examples_check_with_the_compiler() {
    let examples = examples(&repository_root().join("README.md"));
    assert!(
        examples.len() >= 10,
        "README.md should hold its Krit examples, found {}",
        examples.len()
    );
    assert!(
        examples
            .iter()
            .any(|example| example.expected_output.is_some()),
        "README.md should show the output of at least one example"
    );
    check_examples(&examples, "readme");
}

#[test]
fn specification_examples_check_with_the_compiler() {
    let mut documents = fs::read_dir(repository_root().join("spec"))
        .expect("spec directory should be readable")
        .map(|entry| entry.expect("spec entry should be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect::<Vec<_>>();
    documents.sort();
    let examples = documents
        .iter()
        .flat_map(|document| examples(document))
        .collect::<Vec<_>>();
    assert!(
        examples.len() >= 15,
        "the specifications should hold their Krit examples, found {}",
        examples.len()
    );
    check_examples(&examples, "spec");
}

/// Writes each example to a scratch file, requires `krit check` to accept it
/// with no diagnostic at all (not even a warning), and compares the output of
/// `krit run` where the document shows it.
fn check_examples(examples: &[Example], name: &str) {
    let scratch = ScratchDirectory::new(name);
    for (index, example) in examples.iter().enumerate() {
        let file = scratch.path.join(format!("example-{index}.krit"));
        fs::write(&file, &example.source).expect("example should be written");

        let checked = krit()
            .args(["check", "--diagnostic-format", "json"])
            .arg(&file)
            .output()
            .expect("krit check should run");
        assert!(
            checked.status.success() && checked.stderr.is_empty(),
            "{}: `krit check` should accept the example without diagnostics; status {}, \
             diagnostics:\n{}",
            example.location,
            checked.status,
            stderr(&checked)
        );

        if let Some(expected) = &example.expected_output {
            let ran = krit()
                .arg("run")
                .arg(&file)
                .output()
                .expect("krit run should run");
            assert!(
                ran.status.success(),
                "{}: `krit run` should succeed; status {}, diagnostics:\n{}",
                example.location,
                ran.status,
                stderr(&ran)
            );
            assert_eq!(
                String::from_utf8(ran.stdout).expect("output should be UTF-8"),
                *expected,
                "{}: `krit run` should print the output the document shows",
                example.location
            );
        }
    }
}

/// The `krit` code blocks of the Markdown document at `path`, outside the
/// formatter section of the language specification.
fn examples(path: &Path) -> Vec<Example> {
    let text = fs::read_to_string(path).expect("document should be readable");
    let lines = text.lines().collect::<Vec<_>>();
    let relative = path
        .strip_prefix(repository_root())
        .unwrap_or(path)
        .display()
        .to_string();
    let mut examples = Vec::new();
    let mut in_formatter_section = false;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("## ") {
            in_formatter_section = line.starts_with(FORMATTER_SECTION);
        }
        let Some((indent, info)) = fence(line) else {
            index += 1;
            continue;
        };
        let (body, end) = fenced_body(&lines, index, indent, &relative);
        if info == "krit" && !in_formatter_section {
            examples.push(Example {
                location: format!("{relative}:{}", index + 2),
                source: body,
                expected_output: output_after(&lines, end, &relative),
            });
        }
        index = end + 1;
    }
    examples
}

/// The `text` block that follows the fence closing at `end`, separated from it
/// by at most blank lines.
fn output_after(lines: &[&str], end: usize, relative: &str) -> Option<String> {
    let next = (end + 1..lines.len()).find(|&index| !lines[index].trim().is_empty())?;
    let (indent, info) = fence(lines[next])?;
    (info == "text").then(|| fenced_body(lines, next, indent, relative).0)
}

/// The body of the fenced block that opens at `open` with `indent` leading
/// spaces, each line without that indentation and ending in a line feed, and
/// the index of its closing fence.
fn fenced_body(lines: &[&str], open: usize, indent: usize, relative: &str) -> (String, usize) {
    let close = (open + 1..lines.len())
        .find(|&index| fence(lines[index]).is_some_and(|(_, info)| info.is_empty()))
        .unwrap_or_else(|| panic!("{relative}:{}: code block should close", open + 1));
    let body = lines[open + 1..close]
        .iter()
        .map(|line| {
            let strip = line
                .bytes()
                .take(indent)
                .take_while(|byte| *byte == b' ')
                .count();
            format!("{}\n", &line[strip..])
        })
        .collect();
    (body, close)
}

/// The indentation and info string of a fence line such as ```` ```krit ````.
fn fence(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let info = trimmed.strip_prefix("```")?;
    Some((line.len() - trimmed.len(), info.trim()))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new(name: &str) -> Self {
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("doc-examples-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
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
