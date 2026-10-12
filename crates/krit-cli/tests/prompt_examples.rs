//! The provider-neutral AI material must teach only Krit that the compiler
//! accepts. Every ```` ```krit ```` block in the system prompt printed by
//! `krit prompt`, the AI pack README, the assist authoring instruction, and
//! the RFCs is extracted and run through the real `krit` binary: it must be
//! canonically formatted, check with no diagnostic at all (warnings included;
//! an entrypoint program is also checked against the component target), and
//! run, or stop only because direct execution has no hosts (`K5003`).
//!
//! Proposed or invalid syntax belongs in ```` ```text ```` blocks, which are
//! never checked.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const SYSTEM_PROMPT: &str = "crates/krit-cli/assets/KRIT-0.3-SYSTEM.md";
const PROMPT_PACK: &str = "ai/prompt-pack.json";
const PACK_README: &str = "ai/README.md";
const AUTHORING_INSTRUCTION: &str = "crates/krit-assist/assets/KRIT-AUTHORING-1.md";

/// The system prompt's size target: 8,000 tokens, estimated as bytes / 4.
const PROMPT_TOKEN_BUDGET: usize = 8_000;
/// The fewest checked examples the system prompt may carry.
const MINIMUM_PROMPT_EXAMPLES: usize = 13;
/// Each habit code must have a row in the "Krit is not ..." table.
const HABIT_CODES: [&str; 13] = [
    "K1101", "K1102", "K1103", "K1104", "K1105", "K1106", "K1107", "K1108", "K1109", "K1110",
    "K1111", "K1112", "K3011",
];

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct ScratchDirectory {
    path: PathBuf,
}

impl ScratchDirectory {
    fn new(name: &str) -> Self {
        let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = repository_root()
            .join("target/krit-cli-tests")
            .join(format!("prompt-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self { path }
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// One fenced code block: its info string, the 1-based line of its opening
/// fence, and its exact text with a final line feed.
struct Block {
    info: String,
    line: usize,
    text: String,
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CLI crate should be inside the workspace")
}

fn read(relative: &str) -> String {
    fs::read_to_string(repository_root().join(relative))
        .unwrap_or_else(|error| panic!("{relative} should be readable: {error}"))
}

fn krit() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_krit"));
    command.current_dir(repository_root());
    command
}

/// The shipped AI material, then every RFC, as repository-relative paths.
fn documents() -> Vec<String> {
    let mut documents = vec![
        SYSTEM_PROMPT.to_owned(),
        PACK_README.to_owned(),
        AUTHORING_INSTRUCTION.to_owned(),
    ];
    let mut rfcs = fs::read_dir(repository_root().join("rfcs"))
        .expect("rfcs/ should exist")
        .map(|entry| entry.expect("RFC entry should be readable").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .map(|path| {
            format!(
                "rfcs/{}",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .expect("RFC file names should be UTF-8")
            )
        })
        .collect::<Vec<_>>();
    rfcs.sort();
    documents.extend(rfcs);
    documents
}

/// Splits Markdown into its fenced blocks. Every fence must close, every
/// opening fence names its language, and a `krit` block starts at column 0
/// so its text is extracted exactly as a reader would copy it.
fn fenced_blocks(document: &str, text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, String, String, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim_start();
        match &mut open {
            None if trimmed.starts_with("```") => {
                let indent = line[..line.len() - trimmed.len()].to_owned();
                let info = trimmed.trim_start_matches('`').trim().to_owned();
                assert!(
                    !info.is_empty(),
                    "{document}:{number}: an opening fence must name its language"
                );
                assert!(
                    info == "krit" || !info.starts_with("krit"),
                    "{document}:{number}: write ```krit for a checked example and ```text for \
                     proposed or invalid syntax, not ```{info}"
                );
                assert!(
                    info != "krit" || indent.is_empty(),
                    "{document}:{number}: a ```krit fence must start at column 0"
                );
                open = Some((number, indent, info, String::new()));
            }
            None => {}
            Some((start, indent, info, body)) => {
                if trimmed == "```" && line.starts_with(indent.as_str()) {
                    blocks.push(Block {
                        info: std::mem::take(info),
                        line: *start,
                        text: std::mem::take(body),
                    });
                    open = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    if let Some((start, ..)) = open {
        panic!("{document}:{start}: the fence opened here never closes");
    }
    blocks
}

fn krit_examples(document: &str) -> Vec<Block> {
    fenced_blocks(document, &read(document))
        .into_iter()
        .filter(|block| block.info == "krit")
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn diagnostic_codes(output: &Output) -> Vec<String> {
    stderr(output)
        .lines()
        .map(|line| {
            let diagnostic: serde_json::Value =
                serde_json::from_str(line).expect("diagnostic should be one JSON object");
            diagnostic["code"]
                .as_str()
                .expect("diagnostic should have a code")
                .to_owned()
        })
        .collect()
}

/// Formats, checks, and runs one extracted example with the real binary.
fn check_example(directory: &ScratchDirectory, document: &str, block: &Block) {
    let location = format!("{document}:{}", block.line);
    let name = document
        .trim_end_matches(".md")
        .replace(|character: char| !character.is_ascii_alphanumeric(), "-");
    let file = directory
        .path
        .join(format!("{name}-line-{}.krit", block.line));
    fs::write(&file, &block.text).expect("example should be written");

    let formatted = krit()
        .args(["fmt", "--check"])
        .arg(&file)
        .output()
        .expect("Krit should start");
    assert!(
        formatted.status.success() && formatted.stderr.is_empty(),
        "{location}: the example is not canonically formatted:\n{}",
        stderr(&formatted)
    );

    let checked = krit()
        .args(["check", "--diagnostic-format", "json"])
        .arg(&file)
        .output()
        .expect("Krit should start");
    assert!(
        checked.status.success() && checked.stderr.is_empty(),
        "{location}: the example must check with no diagnostics:\n{}",
        stderr(&checked)
    );

    let ran = krit()
        .args(["run", "--diagnostic-format", "json"])
        .arg(&file)
        .output()
        .expect("Krit should start");
    let codes = diagnostic_codes(&ran);
    let ran_cleanly = ran.status.success() && codes.is_empty();
    let stopped_for_host =
        !ran.status.success() && !codes.is_empty() && codes.iter().all(|code| code == "K5003");
    assert!(
        ran_cleanly || stopped_for_host,
        "{location}: the example must run, or stop only for an unavailable host (K5003):\n{}",
        stderr(&ran)
    );
}

#[test]
fn every_embedded_krit_example_formats_checks_and_runs() {
    let directory = ScratchDirectory::new("examples");
    let mut checked = 0;
    for document in documents() {
        for block in krit_examples(&document) {
            check_example(&directory, &document, &block);
            checked += 1;
        }
    }
    assert!(
        checked >= MINIMUM_PROMPT_EXAMPLES,
        "only {checked} examples were found"
    );
}

#[test]
fn system_prompt_covers_every_entrypoint_kind_with_enough_examples() {
    let examples = krit_examples(SYSTEM_PROMPT);
    assert!(
        examples.len() >= MINIMUM_PROMPT_EXAMPLES,
        "the system prompt should carry at least {MINIMUM_PROMPT_EXAMPLES} checked examples, \
         found {}",
        examples.len()
    );
    for marker in ["webhook fn handle(", "queue \"", "schedule \"", "println("] {
        assert!(
            examples.iter().any(|block| block.text.contains(marker)),
            "some system prompt example should contain `{marker}`"
        );
    }
}

#[test]
fn system_prompt_stays_within_its_token_budget() {
    let prompt = read(SYSTEM_PROMPT);
    let estimated_tokens = prompt.len().div_ceil(4);
    assert!(
        estimated_tokens <= PROMPT_TOKEN_BUDGET,
        "the system prompt is about {estimated_tokens} tokens ({} bytes / 4); \
         the budget is {PROMPT_TOKEN_BUDGET}",
        prompt.len()
    );
}

#[test]
fn every_cited_diagnostic_code_is_specified() {
    let specification = read("spec/DIAGNOSTICS.md");
    let specified: BTreeSet<&str> = specification
        .lines()
        .filter_map(|line| line.strip_prefix("| `K"))
        .filter_map(|rest| rest.get(..4))
        .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
        .collect();
    assert!(
        specified.len() > 50,
        "DIAGNOSTICS.md code table should parse"
    );
    for document in [SYSTEM_PROMPT, PACK_README, AUTHORING_INSTRUCTION] {
        let text = read(document);
        let bytes = text.as_bytes();
        for (index, _) in text.match_indices('K') {
            let Some(digits) = text.get(index + 1..index + 5) else {
                continue;
            };
            let bounded = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
            let ends = bytes
                .get(index + 5)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric());
            if bounded && ends && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                assert!(
                    specified.contains(digits),
                    "{document} cites K{digits}, which spec/DIAGNOSTICS.md does not define"
                );
            }
        }
    }
}

#[test]
fn system_prompt_maps_every_habit_code_and_documents_the_standard_library() {
    let prompt = read(SYSTEM_PROMPT);
    for code in HABIT_CODES {
        assert!(
            prompt
                .lines()
                .any(|line| line.starts_with(&format!("| {code} |"))),
            "the habit table should have a row for {code}"
        );
    }

    // Every standard-library function and its exact type, as the
    // standard-library section of LANGUAGE.md lists them.
    let language = read("spec/LANGUAGE.md");
    // The section may be a top-level `##` section or a `###` subsection; it
    // ends at the next heading of either level.
    let is_heading = |line: &str| line.starts_with("## ") || line.starts_with("### ");
    let section: Vec<&str> = language
        .lines()
        .skip_while(|line| !(is_heading(line) && line.contains("Standard library")))
        .skip(1)
        .take_while(|line| !is_heading(line))
        .collect();
    assert!(
        !section.is_empty(),
        "LANGUAGE.md should have a standard-library section"
    );
    let mut functions = 0;
    for line in section {
        let cells: Vec<&str> = line.split(" | ").collect();
        if let [name, signature] = cells.as_slice()
            && let Some(name) = name.strip_prefix("| `")
            && let Some(signature) = signature.strip_suffix(" |")
        {
            let name = name.trim_end_matches('`');
            let signature = signature.trim_matches('`').replace(
                "List<Record { name: String, value: String }>",
                "List<HttpHeader>",
            );
            assert!(
                prompt.contains(&format!("`{name}`")),
                "the system prompt should document `{name}`"
            );
            assert!(
                prompt.contains(&format!("`{signature}`")),
                "the system prompt should give `{name}` the type `{signature}`"
            );
            functions += 1;
        }
    }
    assert_eq!(
        functions, 8,
        "LANGUAGE.md should list the stdlib v0 functions"
    );
}

#[test]
fn prompt_pack_names_the_shipped_prompt_and_its_version() {
    let pack: serde_json::Value =
        serde_json::from_str(&read(PROMPT_PACK)).expect("the prompt pack should be JSON");
    let entrypoint = pack["entrypoint"]
        .as_str()
        .expect("the prompt pack should name its entrypoint");
    assert_eq!(
        entrypoint,
        format!("../{SYSTEM_PROMPT}"),
        "ai/prompt-pack.json should point at the shipped system prompt"
    );
    assert!(repository_root().join("ai").join(entrypoint).is_file());

    let version = pack["promptVersion"]
        .as_str()
        .expect("the prompt pack should have a promptVersion");
    let minor = version
        .rsplit_once('.')
        .map(|(minor, _)| minor)
        .expect("promptVersion should be MAJOR.MINOR.PATCH");
    let prompt = read(SYSTEM_PROMPT);
    assert!(
        prompt.starts_with(&format!("# Krit {minor} code-generation instruction\n")),
        "the system prompt title should name language {minor}, as promptVersion {version} does"
    );
    assert!(
        SYSTEM_PROMPT.ends_with(&format!("KRIT-{minor}-SYSTEM.md")),
        "the system prompt file should be named for language {minor}"
    );
    assert!(
        read(PACK_README).contains(&format!("KRIT-{minor}-SYSTEM.md")),
        "ai/README.md should link the current system prompt"
    );
}
