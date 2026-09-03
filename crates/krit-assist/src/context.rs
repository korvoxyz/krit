use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use krit::{
    Block, Expression, ExpressionKind, MatchKind, Program, Source, Statement, StatementKind,
    ValueLiteral,
};
use krit_package::Manifest;
use serde::Serialize;

use crate::{
    error::AssistError,
    protocol::{
        AUTHORING_INSTRUCTION, AUTHORING_PROTOCOL_VERSION, AssistRequest, ContextRedaction,
        ContextRole, ContextSlice, DocumentPrecondition, LANGUAGE_EDITION, LANGUAGE_VERSION,
        PROMPT_PACK_VERSION, ProviderDescriptor, REQUEST_SCHEMA_VERSION, RequestTarget,
        SuggestionKind, TextPosition, TextRange,
    },
};

pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_CONTEXTS: usize = 16;
pub const MAX_CONTEXT_SLICE_BYTES: usize = 64 * 1024;
pub const MAX_CONTEXT_BYTES: usize = 256 * 1024;
pub const MAX_INTENT_BYTES: usize = 4 * 1024;
pub const MAX_REQUEST_BYTES: usize = 512 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_IGNORE_BYTES: usize = 64 * 1024;
const MAX_IGNORE_LINES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestedRange {
    WholeDocument,
    Utf16 {
        start: TextPosition,
        end: TextPosition,
    },
}

impl FromStr for RequestedRange {
    type Err = AssistError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "all" {
            return Ok(Self::WholeDocument);
        }
        let (start, end) = value.split_once('-').ok_or_else(|| {
            AssistError::context(
                "range must be `all` or `START_LINE:START_UTF16-END_LINE:END_UTF16`",
            )
        })?;
        Ok(Self::Utf16 {
            start: parse_position(start)?,
            end: parse_position(end)?,
        })
    }
}

fn parse_position(value: &str) -> Result<TextPosition, AssistError> {
    let (line, character) = value
        .split_once(':')
        .ok_or_else(|| AssistError::context("range position must be `LINE:UTF16_CHARACTER`"))?;
    let line = line
        .parse()
        .map_err(|_| AssistError::context("range line must be an unsigned integer"))?;
    let character = character
        .parse()
        .map_err(|_| AssistError::context("range character must be an unsigned integer"))?;
    Ok(TextPosition { line, character })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextSelection {
    pub path: PathBuf,
    pub range: RequestedRange,
}

#[derive(Clone, Debug)]
pub struct RequestOptions {
    pub manifest_path: PathBuf,
    pub target_path: PathBuf,
    pub selection: RequestedRange,
    pub contexts: Vec<ContextSelection>,
    pub intent: String,
    pub kind: SuggestionKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub schema: u32,
    pub provider: ProviderDescriptor,
    pub request: AssistRequest,
    pub total_context_bytes: usize,
    pub excluded_context: Vec<&'static str>,
}

#[derive(Debug)]
pub struct PreparedAssistance {
    pub(crate) state: PreparedState,
    inspection: Inspection,
}

impl PreparedAssistance {
    pub fn inspection(&self) -> &Inspection {
        &self.inspection
    }

    pub fn request(&self) -> &AssistRequest {
        &self.inspection.request
    }
}

#[derive(Debug)]
pub(crate) struct PreparedState {
    pub(crate) manifest_path: PathBuf,
    pub(crate) target_path: PathBuf,
    pub(crate) manifest_digest: String,
    pub(crate) target_precondition: DocumentPrecondition,
    pub(crate) context_preconditions: Vec<DocumentPrecondition>,
    pub(crate) base_source: String,
}

pub fn prepare_request(
    provider: ProviderDescriptor,
    options: RequestOptions,
) -> Result<PreparedAssistance, AssistError> {
    validate_intent(&options.intent)?;
    if options.contexts.len().saturating_add(1) > MAX_CONTEXTS {
        return Err(AssistError::context(format!(
            "too many selected context ranges; maximum is {MAX_CONTEXTS}"
        )));
    }

    let package = load_package(&options.manifest_path)?;
    let target = resolve_source_path(&package, &options.target_path, true)?;
    if target.canonical != package.entry {
        return Err(AssistError::context(
            "the assist target must be the package entry source",
        ));
    }
    let base_source = read_resolved_source(&package, &target)?;
    let target_range = resolve_range(&base_source, &options.selection)?;
    let target_precondition = document_precondition(&target.relative, &base_source);
    let mut context_preconditions = vec![target_precondition.clone()];
    let mut contexts = vec![build_context_slice(
        ContextRole::Target,
        &target,
        &base_source,
        target_range.clone(),
    )?];

    for selected in options.contexts {
        let source_path = resolve_source_path(&package, &selected.path, false)?;
        let source = read_resolved_source(&package, &source_path)?;
        let range = resolve_range(&source, &selected.range)?;
        context_preconditions.push(document_precondition(&source_path.relative, &source));
        contexts.push(build_context_slice(
            ContextRole::Additional,
            &source_path,
            &source,
            range,
        )?);
    }
    contexts.sort_by(|left, right| {
        left.role
            .cmp(&right.role)
            .then_with(|| left.document.path.cmp(&right.document.path))
            .then_with(|| left.range.start_byte.cmp(&right.range.start_byte))
            .then_with(|| left.range.end_byte.cmp(&right.range.end_byte))
    });
    contexts.dedup_by(|left, right| {
        left.role == right.role
            && left.document.path == right.document.path
            && left.range == right.range
    });
    context_preconditions.sort_by(|left, right| left.path.cmp(&right.path));
    context_preconditions.dedup_by(|left, right| left.path == right.path);
    let total_context_bytes = contexts.iter().map(|context| context.text.len()).sum();
    if total_context_bytes > MAX_CONTEXT_BYTES {
        return Err(AssistError::context(format!(
            "selected redacted context exceeds the {MAX_CONTEXT_BYTES}-byte limit"
        )));
    }

    let compiler_facts = krit_lsp::compiler_facts_for_document_with_manifest(
        &target.canonical,
        &package.manifest_path,
        &package.manifest,
        &base_source,
    )
    .map_err(|_| AssistError::context("could not derive bounded language-server facts"))?;
    let compiler_facts = provider_compiler_facts(compiler_facts, &target_range)?;
    let mut request = AssistRequest {
        schema: REQUEST_SCHEMA_VERSION,
        authoring_protocol: AUTHORING_PROTOCOL_VERSION,
        prompt_pack_version: PROMPT_PACK_VERSION.to_owned(),
        language_version: LANGUAGE_VERSION.to_owned(),
        edition: LANGUAGE_EDITION.to_owned(),
        request_id: String::new(),
        kind: options.kind,
        instruction: AUTHORING_INSTRUCTION.to_owned(),
        intent: options.intent,
        target: RequestTarget {
            document: contexts[0].document.clone(),
            selection: target_range,
        },
        contexts,
        compiler_facts,
    };
    request.request_id = request_id(&request)?;
    let request_bytes = serde_json::to_vec(&request)
        .map_err(|_| AssistError::context("could not serialize authoring request"))?;
    if request_bytes.len() > MAX_REQUEST_BYTES {
        return Err(AssistError::context(format!(
            "authoring request exceeds the {MAX_REQUEST_BYTES}-byte limit"
        )));
    }

    Ok(PreparedAssistance {
        state: PreparedState {
            manifest_path: package.manifest_path,
            target_path: target.canonical,
            manifest_digest: package.manifest_digest,
            target_precondition,
            context_preconditions,
            base_source,
        },
        inspection: Inspection {
            schema: 1,
            provider,
            request,
            total_context_bytes,
            excluded_context: vec![
                "paths outside the package root",
                "paths matched by .kritignore",
                "generated and non-Krit files",
                "capability resource literals",
                "secret-like string literals",
                "host configuration, runtime data, and credentials",
            ],
        },
    })
}

pub(crate) struct LoadedPackage {
    pub(crate) root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest_digest: String,
    pub(crate) manifest: Manifest,
    pub(crate) entry: PathBuf,
    ignore: Gitignore,
}

pub(crate) fn load_package(path: &Path) -> Result<LoadedPackage, AssistError> {
    let manifest_path = path
        .canonicalize()
        .map_err(|_| AssistError::context("package manifest is not accessible"))?;
    if !manifest_path.is_file() {
        return Err(AssistError::context("package manifest must be a file"));
    }
    let contents = read_bounded_utf8(&manifest_path, MAX_MANIFEST_BYTES, "package manifest")?;
    let manifest = Manifest::parse(&contents)
        .map_err(|_| AssistError::context("package manifest is invalid"))?;
    let entry = manifest
        .resolve_entry(&manifest_path)
        .map_err(|_| AssistError::context("package entry is invalid"))?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| AssistError::context("package manifest has no parent directory"))?
        .to_owned();
    let ignore = load_ignore(&root)?;
    Ok(LoadedPackage {
        root,
        manifest_path,
        manifest_digest: digest_bytes(contents.as_bytes()),
        manifest,
        entry,
        ignore,
    })
}

pub(crate) struct ResolvedSource {
    pub(crate) canonical: PathBuf,
    pub(crate) relative: String,
}

pub(crate) fn resolve_source_path(
    package: &LoadedPackage,
    path: &Path,
    writable: bool,
) -> Result<ResolvedSource, AssistError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        package.root.join(path)
    };
    if writable
        && fs::symlink_metadata(&absolute)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        return Err(AssistError::context(
            "assist target cannot be a symbolic link",
        ));
    }
    let canonical = absolute
        .canonicalize()
        .map_err(|_| AssistError::context("selected source is not accessible"))?;
    if !canonical.starts_with(&package.root) || !canonical.is_file() {
        return Err(AssistError::context(
            "selected source must be a file inside the package root",
        ));
    }
    if canonical
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("krit")
    {
        return Err(AssistError::context(
            "only `.krit` source files may be selected as model context",
        ));
    }
    let relative_path = canonical
        .strip_prefix(&package.root)
        .map_err(|_| AssistError::context("selected source escapes the package root"))?;
    if is_generated_path(relative_path) {
        return Err(AssistError::context(
            "generated or repository-control paths cannot be model context",
        ));
    }
    if package
        .ignore
        .matched_path_or_any_parents(&canonical, false)
        .is_ignore()
    {
        return Err(AssistError::context(
            "selected source is excluded by `.kritignore`",
        ));
    }
    let relative = path_text(relative_path)?;
    Ok(ResolvedSource {
        canonical,
        relative,
    })
}

fn is_generated_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name) if name == "target" || name == ".git"
        )
    })
}

fn load_ignore(root: &Path) -> Result<Gitignore, AssistError> {
    let path = root.join(".kritignore");
    let mut builder = GitignoreBuilder::new(root);
    if path.exists() {
        if fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(AssistError::context(
                "`.kritignore` cannot be a symbolic link",
            ));
        }
        let contents = read_bounded_utf8(&path, MAX_IGNORE_BYTES, "`.kritignore`")?;
        if contents.lines().count() > MAX_IGNORE_LINES {
            return Err(AssistError::context(format!(
                "`.kritignore` exceeds the {MAX_IGNORE_LINES}-line limit"
            )));
        }
        for (index, line) in contents.lines().enumerate() {
            builder.add_line(None, line).map_err(|_| {
                AssistError::context(format!("`.kritignore` line {} is invalid", index + 1))
            })?;
        }
    }
    builder
        .build()
        .map_err(|_| AssistError::context("could not compile `.kritignore`"))
}

pub(crate) fn read_source(path: &Path) -> Result<String, AssistError> {
    read_bounded_utf8(path, MAX_SOURCE_BYTES, "source file")
}

pub(crate) fn read_resolved_source(
    package: &LoadedPackage,
    source: &ResolvedSource,
) -> Result<String, AssistError> {
    #[cfg(unix)]
    let file = open_package_file_no_follow(&package.root, Path::new(&source.relative))?;
    #[cfg(not(unix))]
    let file = fs::File::open(&source.canonical)
        .map_err(|_| AssistError::io("could not read source file"))?;
    read_bounded_utf8_file(file, MAX_SOURCE_BYTES, "source file")
}

pub(crate) fn read_bounded_utf8(
    path: &Path,
    limit: usize,
    label: &str,
) -> Result<String, AssistError> {
    #[cfg(unix)]
    let file = open_file_no_follow(path, label)?;
    #[cfg(not(unix))]
    let file =
        fs::File::open(path).map_err(|_| AssistError::io(format!("could not read {label}")))?;
    read_bounded_utf8_file(file, limit, label)
}

fn read_bounded_utf8_file(
    file: fs::File,
    limit: usize,
    label: &str,
) -> Result<String, AssistError> {
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| AssistError::io(format!("could not read {label}")))?;
    if bytes.len() > limit {
        return Err(AssistError::context(format!(
            "{label} exceeds the {limit}-byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(|_| AssistError::context(format!("{label} is not UTF-8")))
}

#[cfg(unix)]
fn open_file_no_follow(path: &Path, label: &str) -> Result<fs::File, AssistError> {
    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| AssistError::io(format!("could not read {label}")))?;
    let file = fs::File::from(descriptor);
    if !file
        .metadata()
        .map_err(|_| AssistError::io(format!("could not inspect {label}")))?
        .is_file()
    {
        return Err(AssistError::context(format!("{label} must be a file")));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_package_file_no_follow(root: &Path, relative: &Path) -> Result<fs::File, AssistError> {
    use rustix::fs::{Mode, OFlags};

    let mut descriptor = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| AssistError::context("package root is not safely accessible"))?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AssistError::context(
            "selected source path is not package-relative",
        ));
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            unreachable!("components were validated above")
        };
        let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        if index + 1 < components.len() {
            flags |= OFlags::DIRECTORY;
        }
        descriptor = rustix::fs::openat(&descriptor, *name, flags, Mode::empty())
            .map_err(|_| AssistError::context("selected source changed during safe open"))?;
    }
    let file = fs::File::from(descriptor);
    if !file
        .metadata()
        .map_err(|_| AssistError::context("selected source metadata is unavailable"))?
        .is_file()
    {
        return Err(AssistError::context("selected source must be a file"));
    }
    Ok(file)
}

fn path_text(path: &Path) -> Result<String, AssistError> {
    let mut segments = Vec::new();
    for component in path.components() {
        let Component::Normal(segment) = component else {
            return Err(AssistError::context(
                "selected path must contain only normal package-relative components",
            ));
        };
        let segment = segment
            .to_str()
            .ok_or_else(|| AssistError::context("selected path is not UTF-8"))?;
        if segment.chars().any(crate::is_terminal_control) {
            return Err(AssistError::context(
                "selected path contains terminal control characters",
            ));
        }
        segments.push(segment);
    }
    Ok(segments.join("/"))
}

pub(crate) fn document_precondition(path: &str, source: &str) -> DocumentPrecondition {
    DocumentPrecondition {
        path: path.to_owned(),
        digest: digest_bytes(source.as_bytes()),
        byte_length: source.len(),
    }
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

pub(crate) fn resolve_range(
    source: &str,
    requested: &RequestedRange,
) -> Result<TextRange, AssistError> {
    let index = Utf16Index::new(source);
    match requested {
        RequestedRange::WholeDocument => Ok(index.text_range(source, 0, source.len())),
        RequestedRange::Utf16 { start, end } => {
            let start_byte = index.offset(source, start)?;
            let end_byte = index.offset(source, end)?;
            if start_byte > end_byte {
                return Err(AssistError::context("selected range starts after it ends"));
            }
            Ok(TextRange {
                start_byte,
                end_byte,
                start: start.clone(),
                end: end.clone(),
            })
        }
    }
}

pub(crate) fn validate_text_range(source: &str, range: &TextRange) -> Result<(), AssistError> {
    if range.start_byte > range.end_byte
        || range.end_byte > source.len()
        || !source.is_char_boundary(range.start_byte)
        || !source.is_char_boundary(range.end_byte)
    {
        return Err(AssistError::proposal(
            "edit contains an invalid UTF-8 byte range",
        ));
    }
    let index = Utf16Index::new(source);
    let expected = index.text_range(source, range.start_byte, range.end_byte);
    if &expected != range {
        return Err(AssistError::proposal(
            "edit byte and UTF-16 ranges do not identify the same text",
        ));
    }
    Ok(())
}

fn build_context_slice(
    role: ContextRole,
    source_path: &ResolvedSource,
    source: &str,
    range: TextRange,
) -> Result<ContextSlice, AssistError> {
    if range.end_byte - range.start_byte > MAX_CONTEXT_SLICE_BYTES {
        return Err(AssistError::context(format!(
            "one context range exceeds the {MAX_CONTEXT_SLICE_BYTES}-byte limit"
        )));
    }
    let (text, redactions) = redact_slice(source, &range)?;
    Ok(ContextSlice {
        role,
        document: document_precondition(&source_path.relative, &text),
        range,
        text,
        redactions,
        untrusted: true,
    })
}

pub(crate) fn verify_context_slice(
    package: &LoadedPackage,
    context: &ContextSlice,
) -> Result<ContextSlice, AssistError> {
    let source_path = resolve_source_path(package, Path::new(&context.document.path), false)?;
    let source = read_resolved_source(package, &source_path)?;
    validate_text_range(&source, &context.range)?;
    let (text, redactions) = redact_slice(&source, &context.range)?;
    Ok(ContextSlice {
        role: context.role,
        document: document_precondition(&source_path.relative, &text),
        range: context.range.clone(),
        text,
        redactions,
        untrusted: true,
    })
}

fn redact_slice(
    source: &str,
    selected: &TextRange,
) -> Result<(String, Vec<ContextRedaction>), AssistError> {
    let sensitive = sensitive_literal_spans(source);
    let overlaps = sensitive
        .into_iter()
        .filter_map(|(start, end, category)| {
            let start = start.max(selected.start_byte);
            let end = end.min(selected.end_byte);
            (start < end).then_some((start, end, category))
        })
        .collect::<Vec<_>>();
    // Overlapping regions are merged into their union before anything is
    // emitted. Skipping an interval that starts inside an earlier one would
    // expose the tail of an enclosing region - for example a short secret-like
    // literal sorted ahead of the overflow hull that contains it.
    let overlaps = merge_sensitive_intervals(overlaps);
    let index = Utf16Index::new(source);
    let mut output = String::new();
    let mut redactions = Vec::new();
    let mut cursor = selected.start_byte;
    for (start, end, category) in overlaps {
        debug_assert!(start >= cursor, "merged intervals must not overlap");
        output.push_str(&source[cursor..start]);
        let replacement = format!("<redacted:{category}>");
        output.push_str(&replacement);
        redactions.push(ContextRedaction {
            range: index.text_range(source, start, end),
            category: category.to_owned(),
            replacement,
        });
        cursor = end;
    }
    output.push_str(&source[cursor..selected.end_byte]);
    Ok((output, redactions))
}

/// Merges overlapping or touching sensitive regions into their union.
///
/// The union always uses the maximum end, so an enclosing region is never lost
/// to a shorter one that happens to sort first. A merged region keeps its
/// category when every contributor agrees; otherwise it takes the category of
/// the widest contributor, which is the more conservative description.
fn merge_sensitive_intervals(
    mut intervals: Vec<(usize, usize, &'static str)>,
) -> Vec<(usize, usize, &'static str)> {
    intervals.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
    let mut merged: Vec<(usize, usize, &'static str)> = Vec::with_capacity(intervals.len());
    for (start, end, category) in intervals {
        match merged.last_mut() {
            Some(last) if start <= last.1 => {
                if end > last.1 {
                    last.1 = end;
                }
                if last.2 != category {
                    // Widest contributor wins; equal widths keep the first.
                    let existing_width = last.1 - last.0;
                    let candidate_width = end.saturating_sub(start);
                    if candidate_width > existing_width {
                        last.2 = category;
                    }
                }
            }
            _ => merged.push((start, end, category)),
        }
    }
    merged
}

fn sensitive_literal_spans(source: &str) -> Vec<(usize, usize, &'static str)> {
    if let Ok(program) = krit::parse_source(&Source::new("<assist-redaction>", source)) {
        return ast_sensitive_literal_spans(source, &program);
    }
    tolerant_sensitive_literal_spans(source)
}

/// A lexical scope of immutable `let` bindings, holding *snapshots*.
///
/// A binding stores the regions its value contributed **at its declaration**,
/// resolved against the environment in effect there. Storing the expression and
/// resolving later would let a subsequent same-named binding change what an
/// existing alias means, which would redact the wrong text.
type Bindings<'a> = BTreeMap<&'a str, RedactionTargets>;

fn ast_sensitive_literal_spans(
    source: &str,
    program: &Program,
) -> Vec<(usize, usize, &'static str)> {
    fn statement_spans<'a>(
        source: &str,
        statement: &'a Statement,
        scopes: &mut Vec<Bindings<'a>>,
        spans: &mut BTreeMap<(usize, usize), &'static str>,
    ) {
        match &statement.kind {
            StatementKind::Let { value, .. } => expression_spans(source, value, scopes, spans),
            StatementKind::Function {
                parameters, body, ..
            }
            | StatementKind::Webhook {
                parameters, body, ..
            }
            | StatementKind::QueueConsumer {
                parameters, body, ..
            }
            | StatementKind::ScheduleHandler {
                parameters, body, ..
            } => {
                // A parameter shadows any outer binding of the same name and
                // has no traceable value.
                let mut frame = Bindings::new();
                for parameter in parameters {
                    frame.insert(parameter.name.as_str(), RedactionTargets::opaque());
                }
                scopes.push(frame);
                block_spans(source, body, scopes, spans);
                scopes.pop();
            }
            StatementKind::Expression(expression) => {
                expression_spans(source, expression, scopes, spans);
            }
        }
    }

    /// Records one `let` binding as a snapshot of what its value contributes.
    fn declare<'a>(source: &str, statement: &'a Statement, scopes: &mut Vec<Bindings<'a>>) {
        let StatementKind::Let { name, value, .. } = &statement.kind else {
            return;
        };
        let targets = snapshot(source, value, scopes);
        if let Some(frame) = scopes.last_mut() {
            frame.insert(name.as_str(), targets);
        }
    }

    fn block_spans<'a>(
        source: &str,
        block: &'a Block,
        scopes: &mut Vec<Bindings<'a>>,
        spans: &mut BTreeMap<(usize, usize), &'static str>,
    ) {
        scopes.push(Bindings::new());
        for statement in &block.statements {
            statement_spans(source, statement, scopes, spans);
            // Snapshotted after walking, so a binding sees only what precedes
            // it, and a later `let` shadows an earlier one of the same name.
            declare(source, statement, scopes);
        }
        if let Some(tail) = block.tail.as_deref() {
            expression_spans(source, tail, scopes, spans);
        }
        scopes.pop();
    }

    /// Resolves a name innermost scope first, stopping at a parameter shadow.
    fn resolve<'a>(scopes: &'a [Bindings<'_>], name: &str) -> Option<&'a RedactionTargets> {
        scopes
            .iter()
            .rev()
            .find_map(|frame| frame.get(name))
            .filter(|targets| !targets.opaque)
    }

    /// Everything a value expression contributes, resolved now, not later.
    fn snapshot<'a>(
        source: &str,
        expression: &'a Expression,
        scopes: &[Bindings<'a>],
    ) -> RedactionTargets {
        let mut targets = RedactionTargets::default();
        collect(
            source,
            expression,
            scopes,
            &mut BTreeSet::new(),
            &mut targets,
        );
        if targets.overflowed
            && let Some(range) =
                char_boundary_range(source, expression.span.start, expression.span.end)
        {
            targets.add_range(range);
        }
        targets
    }

    /// Collects every literal that can contribute to `expression`.
    ///
    /// Conservative by design: a literal reached through a nested call, a
    /// record, a list, a conditional, or an immutable `let` alias inherits the
    /// argument's category. The walk stays inside the expression, so unrelated
    /// literals elsewhere in the document are untouched.
    fn collect<'a>(
        source: &str,
        expression: &'a Expression,
        scopes: &[Bindings<'a>],
        visited: &mut BTreeSet<(usize, usize)>,
        targets: &mut RedactionTargets,
    ) {
        if !visited.insert((expression.span.start, expression.span.end)) {
            return;
        }
        match &expression.kind {
            ExpressionKind::Literal(ValueLiteral::String(_)) => {
                if let Some(span) =
                    literal_content_span(source, expression.span.start, expression.span.end)
                {
                    targets.add_span(span);
                }
            }
            ExpressionKind::Literal(_) => {}
            ExpressionKind::Variable(name) => {
                if let Some(bound) = resolve(scopes, name) {
                    targets.merge(bound);
                }
            }
            ExpressionKind::List(elements) => {
                for element in elements {
                    collect(source, element, scopes, visited, targets);
                }
            }
            ExpressionKind::Record(fields) => {
                for field in fields {
                    collect(source, &field.value, scopes, visited, targets);
                }
            }
            ExpressionKind::FieldAccess { value, .. } => {
                collect(source, value, scopes, visited, targets);
            }
            ExpressionKind::Block(block) => collect_block(source, block, scopes, visited, targets),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                collect(source, condition, scopes, visited, targets);
                collect_block(source, consequent, scopes, visited, targets);
                collect(source, alternative, scopes, visited, targets);
            }
            ExpressionKind::Function { body, .. } => {
                collect_block(source, body, scopes, visited, targets);
            }
            ExpressionKind::Call { callee, arguments } => {
                collect(source, callee, scopes, visited, targets);
                for argument in arguments {
                    collect(source, argument, scopes, visited, targets);
                }
            }
            ExpressionKind::Match { subject, kind } => {
                collect(source, subject, scopes, visited, targets);
                match kind {
                    MatchKind::List {
                        empty_case,
                        cons_case,
                        ..
                    } => {
                        collect(source, empty_case, scopes, visited, targets);
                        collect(source, cons_case, scopes, visited, targets);
                    }
                    MatchKind::Variants { arms, .. } => {
                        for arm in arms {
                            collect(source, &arm.value, scopes, visited, targets);
                        }
                    }
                }
            }
            ExpressionKind::Unary { operand, .. } => {
                collect(source, operand, scopes, visited, targets);
            }
            ExpressionKind::Binary { left, right, .. } => {
                collect(source, left, scopes, visited, targets);
                collect(source, right, scopes, visited, targets);
            }
        }
    }

    fn collect_block<'a>(
        source: &str,
        block: &'a Block,
        scopes: &[Bindings<'a>],
        visited: &mut BTreeSet<(usize, usize)>,
        targets: &mut RedactionTargets,
    ) {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Let { value, .. } => {
                    collect(source, value, scopes, visited, targets);
                }
                StatementKind::Expression(expression) => {
                    collect(source, expression, scopes, visited, targets);
                }
                StatementKind::Function { body, .. }
                | StatementKind::Webhook { body, .. }
                | StatementKind::QueueConsumer { body, .. }
                | StatementKind::ScheduleHandler { body, .. } => {
                    collect_block(source, body, scopes, visited, targets);
                }
            }
        }
        if let Some(tail) = block.tail.as_deref() {
            collect(source, tail, scopes, visited, targets);
        }
    }

    fn expression_spans<'a>(
        source: &str,
        expression: &'a Expression,
        scopes: &mut Vec<Bindings<'a>>,
        spans: &mut BTreeMap<(usize, usize), &'static str>,
    ) {
        if let ExpressionKind::Literal(ValueLiteral::String(value)) = &expression.kind
            && looks_secret_like(value)
            && let Some((start, end)) =
                literal_content_span(source, expression.span.start, expression.span.end)
        {
            // A more specific category, if one is found later, replaces this.
            spans.entry((start, end)).or_insert("secret-like");
        }
        match &expression.kind {
            ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
            ExpressionKind::List(elements) => {
                for element in elements {
                    expression_spans(source, element, scopes, spans);
                }
            }
            ExpressionKind::Record(fields) => {
                for field in fields {
                    expression_spans(source, &field.value, scopes, spans);
                }
            }
            ExpressionKind::FieldAccess { value, .. } => {
                expression_spans(source, value, scopes, spans);
            }
            ExpressionKind::Block(block) => block_spans(source, block, scopes, spans),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                expression_spans(source, condition, scopes, spans);
                block_spans(source, consequent, scopes, spans);
                expression_spans(source, alternative, scopes, spans);
            }
            ExpressionKind::Function {
                parameters, body, ..
            } => {
                let mut frame = Bindings::new();
                for parameter in parameters {
                    frame.insert(parameter.name.as_str(), RedactionTargets::opaque());
                }
                scopes.push(frame);
                block_spans(source, body, scopes, spans);
                scopes.pop();
            }
            ExpressionKind::Call { callee, arguments } => {
                // Children are walked first so a nested sensitive call records
                // its own categories; this call's argument categories are then
                // applied on top and take precedence.
                expression_spans(source, callee, scopes, spans);
                for argument in arguments {
                    expression_spans(source, argument, scopes, spans);
                }
                if let ExpressionKind::Variable(name) = &callee.kind {
                    for (index, category) in sensitive_arguments(name.as_str()) {
                        let Some(argument) = arguments.get(*index) else {
                            continue;
                        };
                        for (start, end) in snapshot(source, argument, scopes).targets() {
                            spans.insert((start, end), category);
                        }
                    }
                }
            }
            ExpressionKind::Match { subject, kind } => {
                expression_spans(source, subject, scopes, spans);
                match kind {
                    MatchKind::List {
                        empty_case,
                        cons_case,
                        ..
                    } => {
                        expression_spans(source, empty_case, scopes, spans);
                        expression_spans(source, cons_case, scopes, spans);
                    }
                    MatchKind::Variants { arms, .. } => {
                        for arm in arms {
                            expression_spans(source, &arm.value, scopes, spans);
                        }
                    }
                }
            }
            ExpressionKind::Unary { operand, .. } => {
                expression_spans(source, operand, scopes, spans);
            }
            ExpressionKind::Binary { left, right, .. } => {
                expression_spans(source, left, scopes, spans);
                expression_spans(source, right, scopes, spans);
            }
        }
    }

    let mut spans = BTreeMap::new();
    let mut scopes: Vec<Bindings<'_>> = vec![Bindings::new()];
    for statement in &program.statements {
        statement_spans(source, statement, &mut scopes, &mut spans);
        declare(source, statement, &mut scopes);
    }
    spans
        .into_iter()
        .map(|((start, end), category)| (start, end, category))
        .collect()
}

fn literal_content_span(source: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    (end >= start + 2
        && source.as_bytes().get(start) == Some(&b'"')
        && source.as_bytes().get(end - 1) == Some(&b'"'))
    .then_some((start + 1, end - 1))
}

/// Argument positions whose literal content must be redacted, by built-in name.
///
/// One table serves both the parsed path and the tolerant fallback, so a
/// malformed document cannot redact less than a well-formed one.
fn sensitive_arguments(name: &str) -> &'static [(usize, &'static str)] {
    match name {
        "ai_invoke" | "config_string" | "secret" => &[(0, "capability-resource")],
        "http_request" => &[(0, "capability-resource")],
        "log_error" | "log_info" => &[(0, "event-name")],
        "state_delete" | "state_get" | "state_put" => {
            &[(0, "capability-resource"), (1, "state-key")]
        }
        "checkpoint_get" | "checkpoint_put" => {
            &[(0, "capability-resource"), (1, "checkpoint-name")]
        }
        "replay_ai" | "replay_http" => &[
            (0, "capability-resource"),
            (1, "replay-operation"),
            (2, "capability-resource"),
        ],
        "queue_publish" | "db_begin_read" | "db_begin_write" => &[(0, "capability-resource")],
        "db_execute" | "db_query" => &[(1, "database-statement")],
        "object_delete" | "object_get" | "object_put" => {
            &[(0, "capability-resource"), (1, "object-key")]
        }
        "cache_get" | "cache_delete" => &[(0, "capability-resource"), (1, "cache-key")],
        // A cached value is caller data and is redacted alongside its key.
        "cache_put" => &[
            (0, "capability-resource"),
            (1, "cache-key"),
            (2, "cache-value"),
        ],
        "search_query" => &[(0, "capability-resource"), (1, "search-query")],
        "vector_search" => &[(0, "capability-resource"), (1, "search-vector")],
        _ => &[],
    }
}

/// Hard bound on the exact literal spans one tolerant binding may carry.
///
/// Exceeding it never drops a span: the binding switches to redacting its whole
/// value range instead, which is conservative and costs two integers.
const MAX_TRACKED_BINDING_SPANS: usize = 64;
/// Hard bound on whole-value ranges one tolerant binding may carry before they
/// are collapsed into a single covering hull.
const MAX_TRACKED_BINDING_RANGES: usize = 64;

/// What must be redacted when a binding is used in a sensitive position.
///
/// Shared by the parsed and the tolerant paths so both obey the same bounds and
/// the same overflow behaviour.
#[derive(Clone, Debug, Default)]
struct RedactionTargets {
    /// Exact literal content spans.
    spans: Vec<(usize, usize)>,
    /// Whole-value source ranges, used when exact tracking overflowed.
    ranges: Vec<(usize, usize)>,
    /// A parameter or otherwise untraceable name. Resolution stops here rather
    /// than reaching a same-named outer binding.
    opaque: bool,
    /// Set once exact tracking overflowed, so the whole value range is used.
    overflowed: bool,
}

impl RedactionTargets {
    const fn opaque() -> Self {
        Self {
            spans: Vec::new(),
            ranges: Vec::new(),
            opaque: true,
            overflowed: false,
        }
    }

    /// Records one exact literal span.
    ///
    /// Reaching the exact bound never discards coverage: the spans collected so
    /// far are *migrated* into ranges first. A contributor may live in an
    /// earlier statement, far outside the consuming expression, so clearing it
    /// and relying on the expression's own range would expose it.
    fn add_span(&mut self, span: (usize, usize)) {
        if self.overflowed {
            self.add_range(span);
            return;
        }
        if self.spans.len() >= MAX_TRACKED_BINDING_SPANS {
            self.spill_to_ranges();
            self.add_range(span);
            return;
        }
        if !self.spans.contains(&span) {
            self.spans.push(span);
        }
    }

    /// Converts every exact span into a range and switches to range tracking.
    ///
    /// `add_range` bounds the result, collapsing to a covering hull only if the
    /// range budget is also exhausted.
    fn spill_to_ranges(&mut self) {
        self.overflowed = true;
        for span in std::mem::take(&mut self.spans) {
            self.add_range(span);
        }
    }

    /// Absorbs everything another value contributes.
    ///
    /// Nothing is dropped in either direction: an overflowed receiver still
    /// takes the contributor's exact spans as ranges, and an overflowed
    /// contributor's ranges are always incorporated.
    fn merge(&mut self, other: &Self) {
        for range in &other.ranges {
            self.add_range(*range);
        }
        for span in &other.spans {
            self.add_span(*span);
        }
    }

    /// Every region this value contributes.
    fn targets(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.spans.iter().chain(self.ranges.iter()).copied()
    }

    fn add_range(&mut self, range: (usize, usize)) {
        if self.ranges.contains(&range) {
            return;
        }
        self.ranges.push(range);
        if self.ranges.len() > MAX_TRACKED_BINDING_RANGES {
            // Collapse to a single covering hull. This over-redacts the text
            // between the ranges, which is safe, and keeps the value bounded.
            let start = self.ranges.iter().map(|(start, _)| *start).min();
            let end = self.ranges.iter().map(|(_, end)| *end).max();
            if let (Some(start), Some(end)) = (start, end) {
                self.ranges = vec![(start, end)];
            }
        }
    }
}

/// A `let` binding whose value is still being scanned.
struct PendingBinding<'a> {
    name: &'a str,
    /// Everything the value has contributed so far. Shares the parsed path's
    /// bounds and overflow behaviour exactly.
    targets: RedactionTargets,
    value_start: Option<usize>,
    value_end: usize,
    /// Frame depth when the binding began, so a multiline call, record, or list
    /// does not let a newline terminate it early.
    depth: usize,
    /// Index of the scope the binding was declared in. It is finalised there
    /// even if its value opened and closed nested scopes.
    scope: usize,
}

impl<'a> PendingBinding<'a> {
    fn new(name: &'a str, depth: usize, scope: usize) -> Self {
        Self {
            name,
            targets: RedactionTargets::default(),
            value_start: None,
            value_end: 0,
            depth,
            scope,
        }
    }

    fn record_literal(&mut self, span: (usize, usize)) {
        self.targets.add_span(span);
    }

    fn inherit(&mut self, value: &RedactionTargets) {
        self.targets.merge(value);
    }

    /// The value range scanned so far, clamped to character boundaries.
    fn value_range(&self, source: &str) -> Option<(usize, usize)> {
        char_boundary_range(source, self.value_start?, self.value_end)
    }

    fn finish(self, source: &str) -> RedactionTargets {
        let range = self.value_range(source);
        let mut targets = self.targets;
        // The value's own range is added only once exact tracking overflowed;
        // otherwise the exact spans already describe it precisely.
        if targets.overflowed
            && let Some(range) = range
        {
            targets.add_range(range);
        }
        targets
    }

    /// A conservative view of an *unfinished* binding.
    ///
    /// A sensitive reference can occur before the binding's statement is
    /// terminated, for example in truncated source. Everything collected so far
    /// is reported, plus the value range scanned so far, so nothing already
    /// written is exposed.
    fn in_progress(&self, source: &str) -> RedactionTargets {
        let mut targets = self.targets.clone();
        if let Some(range) = self.value_range(source) {
            targets.add_range(range);
        }
        targets
    }
}

/// Clamps a byte range outward to valid UTF-8 boundaries.
fn char_boundary_range(source: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    if start >= end || end > source.len() {
        return None;
    }
    let mut start = start;
    while start > 0 && !source.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = end;
    while end < source.len() && !source.is_char_boundary(end) {
        end += 1;
    }
    (start < end).then_some((start, end))
}

/// The kind of group one delimiter opened.
#[derive(Clone, Copy, Eq, PartialEq)]
enum GroupKind {
    Call,
    Bracket,
    /// A record literal: it groups commas but introduces no lexical scope.
    Record,
    /// A block: it introduces a lexical scope.
    Block,
}

/// One in-flight call while scanning malformed source.
struct CallFrame<'a> {
    name: Option<&'a str>,
    argument: usize,
}

/// The category that applies at the current position, searching outward.
///
/// A literal nested inside another call, a record, or a list is still inside
/// its enclosing call's argument, so the innermost frame that names a
/// sensitive position wins. This is what protects
/// `cache_put("ns", "k", json_encode(record { value: "private" }), 60)`.
fn frame_category(frames: &[CallFrame<'_>]) -> Option<&'static str> {
    frames.iter().rev().find_map(|frame| {
        let name = frame.name?;
        sensitive_arguments(name)
            .iter()
            .find(|(index, _)| *index == frame.argument)
            .map(|(_, category)| *category)
    })
}

fn tolerant_sensitive_literal_spans(source: &str) -> Vec<(usize, usize, &'static str)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut tokens = Vec::new();
    let mut frames: Vec<CallFrame<'_>> = Vec::new();
    let mut groups: Vec<GroupKind> = Vec::new();
    // Lexical binding scopes, innermost last. A nested block's bindings are
    // discarded when it closes, restoring any outer binding it shadowed.
    let mut scopes: Vec<BTreeMap<&str, RedactionTargets>> = vec![BTreeMap::new()];
    // A stack: a binding's value may itself contain a block with further
    // bindings, and every enclosing binding also receives their contributions.
    let mut pending: Vec<PendingBinding<'_>> = Vec::new();
    let mut expect_binding_name = false;
    // Parameter names collected from a `fn (...)` list, installed as
    // untraceable shadows when the body block opens.
    let mut parameters: Vec<&str> = Vec::new();
    let mut collecting_parameters = false;
    let mut expect_parameter_name = false;
    let mut saw_function_keyword = false;
    let mut cursor = 0;

    /// Finalises one binding into the scope it was declared in, not whichever
    /// scope happens to be innermost when its statement ends.
    macro_rules! finish_top {
        () => {
            if let Some(binding) = pending.pop() {
                let name = binding.name;
                let scope = binding.scope;
                let value = binding.finish(source);
                if let Some(frame) = scopes.get_mut(scope) {
                    frame.insert(name, value);
                }
            }
        };
    }

    while cursor < bytes.len() {
        if bytes[cursor] == b'/' && bytes.get(cursor + 1) == Some(&b'/') {
            cursor += 2;
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\n' | b'\r') {
                cursor += 1;
            }
            continue;
        }
        if bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' {
            let start = cursor;
            cursor += 1;
            while cursor < bytes.len() && is_identifier_continue(bytes[cursor]) {
                cursor += 1;
            }
            let identifier = &source[start..cursor];
            for binding in &mut pending {
                binding.value_start.get_or_insert(start);
                binding.value_end = cursor;
            }
            if identifier == "let" {
                expect_binding_name = true;
            } else if identifier == "fn" {
                saw_function_keyword = true;
                parameters.clear();
            } else if expect_binding_name {
                expect_binding_name = false;
                pending.push(PendingBinding::new(
                    identifier,
                    frames.len(),
                    scopes.len().saturating_sub(1),
                ));
            } else if collecting_parameters && expect_parameter_name {
                expect_parameter_name = false;
                parameters.push(identifier);
            } else {
                // Under sequential `let` semantics a declaration is not
                // visible from its own initializer, so the real contributor is
                // always the nearest *finalized* lexical binding. That is
                // resolved independently here: letting a pending declaration
                // short-circuit the lookup would hide the finalized outer
                // value whenever two same-named declarations are open at once.
                //
                // Every same-named pending declaration is then unioned in as a
                // conservative safety net, because in truncated source the
                // reference may sit inside an initializer whose text is
                // already written.
                let barrier =
                    scopes.iter().enumerate().rev().find_map(|(index, frame)| {
                        frame.get(identifier).map(|value| (index, value))
                    });
                // A parameter shadows everything further out. The barrier
                // applies to pending declarations too, so one from outside the
                // parameter's scope can never slip past it.
                let (mut inherited, floor) = match barrier {
                    Some((index, value)) if value.opaque => (None, Some(index)),
                    Some((_, value)) => (Some(value.clone()), None),
                    None => (None, None),
                };
                for binding in pending
                    .iter()
                    .rev()
                    .filter(|binding| binding.name == identifier)
                {
                    if floor.is_some_and(|floor| binding.scope < floor) {
                        continue;
                    }
                    let candidate = binding.in_progress(source);
                    match inherited.as_mut() {
                        Some(existing) => existing.merge(&candidate),
                        None => inherited = Some(candidate),
                    }
                }
                // An alias inside a binding's value inherits the referenced
                // binding's contributions, so a chain of aliases stays
                // protected. Every enclosing binding inherits, including a
                // rebind of the same name: `let token = token;` really does
                // take the outer value. Merging is idempotent and
                // deduplicated, so a self-reference cannot grow or loop.
                if let Some(inherited) = inherited.as_ref() {
                    for binding in &mut pending {
                        binding.inherit(inherited);
                    }
                }
                // A bare name in a sensitive position redacts everything that
                // contributed to its value.
                if let Some(category) = frame_category(&frames)
                    && let Some(inherited) = inherited.as_ref()
                {
                    for (start, end) in inherited.targets() {
                        spans.push((start, end, category));
                    }
                }
            }
            tokens.push(TolerantToken::Identifier(identifier));
            continue;
        }
        if !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'=' {
            for binding in &mut pending {
                binding.value_start.get_or_insert(cursor);
                binding.value_end = cursor + 1;
            }
        }
        match bytes[cursor] {
            b'\n' => {
                // Krit terminates a statement at a newline, so a binding ends
                // here unless a call, record, or list is still open. `\r` is
                // ordinary whitespace, so CRLF is covered by the same rule.
                if pending
                    .last()
                    .is_some_and(|binding| frames.len() <= binding.depth)
                {
                    finish_top!();
                }
                cursor += 1;
                continue;
            }
            b'(' => {
                // A call opens a frame; the callee is whatever identifier
                // immediately precedes it, which may itself be parenthesised.
                if saw_function_keyword && !collecting_parameters {
                    collecting_parameters = true;
                    expect_parameter_name = true;
                }
                frames.push(CallFrame {
                    name: grouped_token_identifier(&tokens),
                    argument: 0,
                });
                groups.push(GroupKind::Call);
                tokens.push(TolerantToken::LeftParen);
                cursor += 1;
                continue;
            }
            b')' => {
                // Only a matching call group is closed; a stray `)` is ignored.
                if groups.last() == Some(&GroupKind::Call) {
                    if collecting_parameters {
                        collecting_parameters = false;
                        expect_parameter_name = false;
                    }
                    frames.pop();
                    groups.pop();
                }
                tokens.push(TolerantToken::RightParen);
                cursor += 1;
                continue;
            }
            b';' => {
                finish_top!();
                tokens.push(TolerantToken::Other);
                cursor += 1;
                continue;
            }
            b',' => {
                // A comma advances the argument index of the innermost call, so
                // a nested call's arguments never shift its parent's indexes.
                if collecting_parameters {
                    expect_parameter_name = true;
                }
                if let Some(frame) = frames.last_mut() {
                    frame.argument = frame.argument.saturating_add(1);
                }
                tokens.push(TolerantToken::Other);
                cursor += 1;
                continue;
            }
            b'[' | b'{' => {
                // A bracketed group inside an argument must not let its commas
                // advance the enclosing call's argument index.
                frames.push(CallFrame {
                    name: None,
                    argument: 0,
                });
                let kind = if bytes[cursor] == b'[' {
                    GroupKind::Bracket
                } else if matches!(tokens.last(), Some(TolerantToken::Identifier("record"))) {
                    // `record { ... }` is a literal, not a lexical scope.
                    GroupKind::Record
                } else {
                    GroupKind::Block
                };
                groups.push(kind);
                if kind == GroupKind::Block {
                    // A block opens a scope. Parameters collected from the
                    // preceding `fn (...)` shadow any outer binding and stop
                    // the trace rather than resolving to a caller's value.
                    let mut frame = BTreeMap::new();
                    if saw_function_keyword {
                        for parameter in parameters.drain(..) {
                            frame.insert(parameter, RedactionTargets::opaque());
                        }
                        saw_function_keyword = false;
                    }
                    scopes.push(frame);
                }
                tokens.push(TolerantToken::Other);
                cursor += 1;
                continue;
            }
            b']' | b'}' => {
                // A closer only closes the group it actually matches. A stray
                // or mismatched closer must never discard a live scope or
                // desynchronise the stacks, so it is ignored instead.
                let matches = matches!(
                    (bytes[cursor], groups.last()),
                    (b']', Some(GroupKind::Bracket))
                        | (b'}', Some(GroupKind::Block | GroupKind::Record))
                );
                if matches {
                    frames.pop();
                    if groups.pop() == Some(GroupKind::Block) && scopes.len() > 1 {
                        // Leaving the block restores every outer binding it
                        // shadowed. A binding declared inside it is finalised
                        // into that scope and then discarded with it; its
                        // contributions already reached every enclosing
                        // binding.
                        while pending
                            .last()
                            .is_some_and(|binding| binding.scope >= scopes.len() - 1)
                        {
                            finish_top!();
                        }
                        scopes.pop();
                    }
                }
                tokens.push(TolerantToken::Other);
                cursor += 1;
                continue;
            }
            b'"' => {}
            byte if byte.is_ascii_whitespace() => {
                cursor += 1;
                continue;
            }
            _ => {
                tokens.push(TolerantToken::Other);
                cursor += 1;
                continue;
            }
        }
        cursor += 1;
        let content_start = cursor;
        let mut escaped = false;
        while cursor < bytes.len() {
            if escaped {
                escaped = false;
            } else if bytes[cursor] == b'\\' {
                escaped = true;
            } else if bytes[cursor] == b'"' {
                break;
            }
            cursor += 1;
        }
        let content_end = cursor.min(bytes.len());
        let content = &source[content_start..content_end];
        // A literal inside a binding's value is remembered, so a later
        // sensitive use of that name can redact it retroactively.
        for binding in &mut pending {
            binding
                .value_start
                .get_or_insert(content_start.saturating_sub(1));
            binding.value_end = (content_end + 1).min(source.len());
            binding.record_literal((content_start, content_end));
        }
        let category =
            frame_category(&frames).or_else(|| looks_secret_like(content).then_some("secret-like"));
        if let Some(category) = category {
            spans.push((content_start, content_end, category));
        }
        if cursor < bytes.len() {
            cursor += 1;
        }
        tokens.push(TolerantToken::Other);
    }
    while !pending.is_empty() {
        finish_top!();
    }
    spans
}

#[derive(Clone, Copy)]
enum TolerantToken<'a> {
    Identifier(&'a str),
    LeftParen,
    RightParen,
    Other,
}

fn grouped_token_identifier<'a>(tokens: &[TolerantToken<'a>]) -> Option<&'a str> {
    let mut end = tokens.len();
    let mut closing = 0usize;
    while end > 0 && matches!(tokens[end - 1], TolerantToken::RightParen) {
        closing += 1;
        end -= 1;
    }
    let TolerantToken::Identifier(name) = tokens.get(end.checked_sub(1)?)? else {
        return None;
    };
    if closing == 0 {
        return Some(*name);
    }
    let identifier = end - 1;
    let opening_start = identifier.checked_sub(closing)?;
    if tokens[opening_start..identifier]
        .iter()
        .all(|token| matches!(token, TolerantToken::LeftParen))
    {
        Some(*name)
    } else {
        None
    }
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn looks_secret_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "-----begin ",
        "authorization: bearer ",
        "bearer ",
        "github_pat_",
        "ghp_",
        "sk-",
        "xoxb-",
        "xoxp-",
        "api_key=",
        "apikey=",
        "access_token=",
        "client_secret=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub(crate) fn validate_intent(intent: &str) -> Result<(), AssistError> {
    if intent.trim().is_empty() {
        return Err(AssistError::context("assist intent cannot be empty"));
    }
    if intent.len() > MAX_INTENT_BYTES {
        return Err(AssistError::context(format!(
            "assist intent exceeds the {MAX_INTENT_BYTES}-byte limit"
        )));
    }
    if intent.contains('\0') || looks_secret_like(intent) {
        return Err(AssistError::context(
            "assist intent contains disallowed sensitive material",
        ));
    }
    Ok(())
}

pub(crate) fn request_id(request: &AssistRequest) -> Result<String, AssistError> {
    let mut unsigned = request.clone();
    unsigned.request_id.clear();
    let bytes = serde_json::to_vec(&unsigned)
        .map_err(|_| AssistError::context("could not serialize authoring request"))?;
    Ok(digest_bytes(&bytes))
}

pub(crate) fn provider_compiler_facts(
    facts: serde_json::Value,
    selection: &TextRange,
) -> Result<serde_json::Value, AssistError> {
    let mut filtered = serde_json::json!({
        "schema": facts.get("schema").cloned().unwrap_or(serde_json::Value::Null),
        "authoringProtocol": facts
            .get("authoringProtocol")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "languageVersion": facts
            .get("languageVersion")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "edition": facts.get("edition").cloned().unwrap_or(serde_json::Value::Null),
        "valid": facts.get("valid").cloned().unwrap_or(serde_json::Value::Bool(false)),
        "diagnostics": filter_fact_array(facts.get("diagnostics"), selection),
        "module": facts.get("module").cloned().unwrap_or(serde_json::Value::Null),
        "durableState": facts.get("durableState").cloned().unwrap_or(serde_json::Value::Null),
        "package": facts.get("package").cloned().unwrap_or(serde_json::Value::Null),
        "symbols": filter_fact_array(facts.get("symbols"), selection),
        "expressions": filter_fact_array(facts.get("expressions"), selection),
        "formatting": {
            "available": facts.pointer("/formatting/available").cloned().unwrap_or(serde_json::Value::Bool(false)),
            "canonical": facts.pointer("/formatting/canonical").cloned().unwrap_or(serde_json::Value::Bool(false)),
            "error": facts.pointer("/formatting/error").cloned().unwrap_or(serde_json::Value::Null)
        }
    });
    redact_fact_resources(&mut filtered);
    let bytes = serde_json::to_vec(&filtered)
        .map_err(|_| AssistError::context("could not serialize filtered compiler facts"))?;
    if bytes.len() > MAX_CONTEXT_BYTES {
        return Err(AssistError::context(
            "selected compiler facts exceed the bounded model-context limit",
        ));
    }
    Ok(filtered)
}

fn filter_fact_array(
    value: Option<&serde_json::Value>,
    selection: &TextRange,
) -> serde_json::Value {
    let items = value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            let start = item
                .pointer("/span/start")
                .and_then(serde_json::Value::as_u64);
            let end = item
                .pointer("/span/end")
                .and_then(serde_json::Value::as_u64);
            match (start, end) {
                (Some(start), Some(end)) => {
                    ranges_intersect(start as usize, end as usize, selection)
                }
                _ => false,
            }
        })
        .cloned()
        .collect();
    serde_json::Value::Array(items)
}

fn ranges_intersect(start: usize, end: usize, selection: &TextRange) -> bool {
    if start == end {
        return selection.start_byte <= start && start <= selection.end_byte;
    }
    if selection.start_byte == selection.end_byte {
        start <= selection.start_byte && selection.start_byte <= end
    } else {
        start < selection.end_byte && selection.start_byte < end
    }
}

fn redact_fact_resources(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                redact_fact_resources(item);
            }
        }
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                if matches!(
                    name.as_str(),
                    "resource" | "store" | "identity" | "externalResource"
                ) && value.is_string()
                {
                    *value = serde_json::Value::String("<redacted:capability-resource>".to_owned());
                } else if name == "inferredType"
                    && let Some(rendered) = value.as_str()
                {
                    *value = serde_json::Value::String(redact_rendered_type(rendered));
                } else if name == "message"
                    && let Some(message) = value.as_str()
                {
                    *value = serde_json::Value::String(redact_rendered_type(message));
                } else {
                    redact_fact_resources(value);
                }
            }

            fn redact_rendered_type(rendered: &str) -> String {
                if !rendered.contains(" requirements {") {
                    return rendered.to_owned();
                }
                let mut output = String::with_capacity(rendered.len());
                let mut characters = rendered.chars();
                while let Some(character) = characters.next() {
                    if character != '"' {
                        output.push(character);
                        continue;
                    }
                    output.push('"');
                    output.push_str("<redacted:capability-resource>");
                    output.push('"');
                    let mut escaped = false;
                    for value in characters.by_ref() {
                        if escaped {
                            escaped = false;
                        } else if value == '\\' {
                            escaped = true;
                        } else if value == '"' {
                            break;
                        }
                    }
                }
                output
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

struct Utf16Index {
    starts: Vec<usize>,
}

impl Utf16Index {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        Self { starts }
    }

    fn offset(&self, source: &str, position: &TextPosition) -> Result<usize, AssistError> {
        let line = position.line as usize;
        let start = *self
            .starts
            .get(line)
            .ok_or_else(|| AssistError::context("selected range line is outside the source"))?;
        let end = self.line_content_end(source, line);
        let target = position.character as usize;
        let mut utf16 = 0;
        for (relative, character) in source[start..end].char_indices() {
            if utf16 == target {
                return Ok(start + relative);
            }
            let next = utf16 + character.len_utf16();
            if target < next {
                return Err(AssistError::context(
                    "selected UTF-16 range splits a surrogate pair",
                ));
            }
            utf16 = next;
        }
        if utf16 == target {
            Ok(end)
        } else {
            Err(AssistError::context(
                "selected UTF-16 character is outside the source line",
            ))
        }
    }

    fn text_range(&self, source: &str, start: usize, end: usize) -> TextRange {
        TextRange {
            start_byte: start,
            end_byte: end,
            start: self.position(source, start),
            end: self.position(source, end),
        }
    }

    fn position(&self, source: &str, byte: usize) -> TextPosition {
        let line = self.starts.partition_point(|start| *start <= byte) - 1;
        let character = source[self.starts[line]..byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        TextPosition {
            line: line as u32,
            character: character as u32,
        }
    }

    fn line_content_end(&self, source: &str, line: usize) -> usize {
        let Some(mut end) = self.starts.get(line + 1).copied() else {
            return source.len();
        };
        if end > 0 && source.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > 0 && source.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_empty_final_line_after_lf_or_crlf() {
        for source in ["one\n", "one\r\n"] {
            let end = TextPosition {
                line: 1,
                character: 0,
            };

            assert_eq!(
                resolve_range(
                    source,
                    &RequestedRange::Utf16 {
                        start: end.clone(),
                        end: end.clone(),
                    },
                )
                .expect("the final empty line should be a valid range"),
                TextRange {
                    start_byte: source.len(),
                    end_byte: source.len(),
                    start: end.clone(),
                    end,
                }
            );
        }
    }

    /// Every redacted literal, as `(content, category)`.
    fn redacted(source: &str) -> Vec<(String, &'static str)> {
        sensitive_literal_spans(source)
            .into_iter()
            .map(|(start, end, category)| (source[start..end].to_owned(), category))
            .collect()
    }

    /// The same, forcing the tolerant fallback used for malformed source.
    fn redacted_tolerant(source: &str) -> Vec<(String, &'static str)> {
        tolerant_sensitive_literal_spans(source)
            .into_iter()
            .map(|(start, end, category)| (source[start..end].to_owned(), category))
            .collect()
    }

    const CACHE_CALLS: &str = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_get("lookups", "user-42-profile") {
        Ok(found) => match cache_put("lookups", "user-42-profile", "private-payload", 60) {
            Ok(stored) => match cache_delete("lookups", "user-42-secret") {
                Ok(gone) => match search_query("docs", "private user question", 3) {
                    Ok(hits) => match vector_search("vectors", "[0.5,0.25,0.125]", 3) {
                        Ok(near) => record { status: 200, headers: [], body: near },
                        Err(problem) => record { status: 500, headers: [], body: problem },
                    },
                    Err(problem) => record { status: 500, headers: [], body: problem },
                },
                Err(problem) => record { status: 500, headers: [], body: problem },
            },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;

    #[test]
    fn parsed_source_redacts_every_cache_and_search_payload() {
        let spans = redacted(CACHE_CALLS);

        for expected in [
            ("lookups", "capability-resource"),
            ("user-42-profile", "cache-key"),
            ("private-payload", "cache-value"),
            ("user-42-secret", "cache-key"),
            ("docs", "capability-resource"),
            ("private user question", "search-query"),
            ("vectors", "capability-resource"),
            ("[0.5,0.25,0.125]", "search-vector"),
        ] {
            assert!(
                spans
                    .iter()
                    .any(|(content, category)| content == expected.0 && *category == expected.1),
                "`{}` was not redacted as `{}`: {spans:?}",
                expected.0,
                expected.1
            );
        }
    }

    #[test]
    fn malformed_source_redacts_exactly_what_parsed_source_does() {
        // The tolerant path must never redact less than the parsed path.
        let parsed = redacted(CACHE_CALLS);
        let tolerant = redacted_tolerant(CACHE_CALLS);

        for (content, category) in &parsed {
            assert!(
                tolerant
                    .iter()
                    .any(|(other, kind)| other == content && kind == category),
                "the tolerant path missed `{content}` (`{category}`): {tolerant:?}"
            );
        }
    }

    #[test]
    fn incomplete_source_still_redacts_later_arguments() {
        // Source that does not parse: the fallback must still track argument
        // positions rather than redacting only the first one.
        for (source, expected) in [
            (
                "cache_put(\"lookups\", \"user-key\", \"private-payload\", 60",
                vec!["lookups", "user-key", "private-payload"],
            ),
            (
                "match cache_get(\"lookups\", \"user-key\") { Ok(",
                vec!["lookups", "user-key"],
            ),
            (
                "search_query(\"docs\", \"private question\"",
                vec!["docs", "private question"],
            ),
            (
                "vector_search(\"vectors\", \"[1.0,2.0]\"",
                vec!["vectors", "[1.0,2.0]"],
            ),
            (
                "cache_delete(\"lookups\", \"user-key\"",
                vec!["lookups", "user-key"],
            ),
        ] {
            let spans = redacted_tolerant(source);
            for content in expected {
                assert!(
                    spans.iter().any(|(found, _)| found == content),
                    "`{content}` leaked from `{source}`: {spans:?}"
                );
            }
        }
    }

    #[test]
    fn a_nested_call_does_not_shift_its_parent_argument_indexes() {
        // The inner call's arguments must not consume the outer call's
        // positions, so the outer value is still redacted.
        let source =
            "cache_put(\"lookups\", json_encode(record { a: 1 }), \"private-payload\", 60)";
        let spans = redacted_tolerant(source);

        assert!(
            spans
                .iter()
                .any(|(content, category)| content == "private-payload"
                    && *category == "cache-value"),
            "a nested call shifted the value argument: {spans:?}"
        );
    }

    #[test]
    fn a_bracketed_argument_does_not_shift_argument_indexes() {
        // Commas inside a list must not advance the enclosing argument index.
        let source = "search_query(\"docs\", \"private question\", 3) and [1, 2, 3]";
        let spans = redacted_tolerant(source);

        assert!(
            spans
                .iter()
                .any(|(content, category)| content == "private question"
                    && *category == "search-query"),
            "a bracketed group shifted the query argument: {spans:?}"
        );
    }

    #[test]
    fn a_variable_alias_in_a_sensitive_argument_is_redacted() {
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let question = "private customer question";
    match search_query("docs", question, 3) {
        Ok(hits) => record { status: 200, headers: [], body: hits },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;

        for spans in [redacted(source), redacted_tolerant(source)] {
            assert!(
                spans
                    .iter()
                    .any(|(content, category)| content == "private customer question"
                        && *category == "search-query"),
                "an aliased query leaked: {spans:?}"
            );
        }
    }

    #[test]
    fn nested_expressions_inside_a_sensitive_argument_are_redacted() {
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match cache_put("lookups", "k", json_encode(record { value: "private-payload" }), 60) {
        Ok(stored) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;

        for spans in [redacted(source), redacted_tolerant(source)] {
            assert!(
                spans
                    .iter()
                    .any(|(content, category)| content == "private-payload"
                        && *category == "cache-value"),
                "a nested payload leaked: {spans:?}"
            );
        }
    }

    #[test]
    fn lists_conditionals_and_alias_chains_all_contribute() {
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let first = "chain-origin";
    let second = first;
    let third = second;
    match cache_put("lookups", third, json_encode([1, "list-payload"]), 60) {
        Ok(stored) => match search_query("docs", if true { "then-query" } else { "else-query" }, 3) {
            Ok(hits) => record { status: 200, headers: [], body: hits },
            Err(problem) => record { status: 500, headers: [], body: problem },
        },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;
        let spans = redacted(source);

        for (content, category) in [
            ("chain-origin", "cache-key"),
            ("list-payload", "cache-value"),
            ("then-query", "search-query"),
            ("else-query", "search-query"),
        ] {
            assert!(
                spans
                    .iter()
                    .any(|(found, kind)| found == content && *kind == category),
                "`{content}` was not redacted as `{category}`: {spans:?}"
            );
        }
    }

    #[test]
    fn shadowing_redacts_the_binding_actually_in_scope() {
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let question = "outer-question";
    let question = "inner-question";
    match search_query("docs", question, 3) {
        Ok(hits) => record { status: 200, headers: [], body: hits },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;
        let spans = redacted(source);

        assert!(
            spans
                .iter()
                .any(|(content, category)| content == "inner-question"
                    && *category == "search-query"),
            "the shadowing binding must be redacted: {spans:?}"
        );
    }

    #[test]
    fn a_function_parameter_shadows_an_outer_binding_without_tracing_it() {
        // `question` inside `lookup` is a parameter, not the outer binding, so
        // the outer literal is not attributed to the call inside `lookup`.
        let source = r#"
fn lookup(question: String) -> HttpResponse {
    match search_query("docs", question, 3) {
        Ok(hits) => record { status: 200, headers: [], body: hits },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}

webhook fn handle(request: HttpRequest) -> HttpResponse {
    lookup("caller-question")
}
"#;
        let spans = redacted(source);

        assert!(
            !spans
                .iter()
                .any(|(content, _)| content == "caller-question"),
            "a parameter must not trace to an unrelated outer literal: {spans:?}"
        );
    }

    #[test]
    fn incomplete_source_still_protects_aliases_and_nesting() {
        for (source, expected) in [
            (
                "let question = \"private customer question\";\n    search_query(\"docs\", question, 3",
                "private customer question",
            ),
            (
                "let payload = \"private-payload\";\n    cache_put(\"lookups\", \"k\", payload, 60",
                "private-payload",
            ),
            (
                "cache_put(\"lookups\", \"k\", json_encode(record { value: \"nested-payload\" }), 60",
                "nested-payload",
            ),
            (
                "vector_search(\"vectors\", json_encode([1.0, \"vector-payload\"]), 3",
                "vector-payload",
            ),
        ] {
            let spans = redacted_tolerant(source);
            assert!(
                spans.iter().any(|(content, _)| content == expected),
                "`{expected}` leaked from malformed source `{source}`: {spans:?}"
            );
        }
    }

    #[test]
    fn tolerant_alias_chains_are_protected_for_every_sensitive_position() {
        // Two- and three-level chains feeding each sensitive position, in
        // malformed source that stops mid-call.
        for (source, expected) in [
            (
                "let first = \"chain-origin\"; let second = first; cache_put(\"lookups\", second, \"v\", 60",
                "chain-origin",
            ),
            (
                "let a = \"deep-value\"; let b = a; let c = b; cache_put(\"lookups\", \"k\", c, 60",
                "deep-value",
            ),
            (
                "let q1 = \"chained-query\"; let q2 = q1; search_query(\"docs\", q2, 3",
                "chained-query",
            ),
            (
                "let v1 = \"[1.0,2.0]\"; let v2 = v1; let v3 = v2; vector_search(\"vectors\", v3, 3",
                "[1.0,2.0]",
            ),
            (
                "let k1 = \"chain-key\"; let k2 = k1; cache_delete(\"lookups\", k2",
                "chain-key",
            ),
            (
                "let n1 = \"chain-namespace\"; let n2 = n1; cache_get(n2, \"k\"",
                "chain-namespace",
            ),
        ] {
            let spans = redacted_tolerant(source);
            assert!(
                spans.iter().any(|(content, _)| content == expected),
                "`{expected}` leaked from `{source}`: {spans:?}"
            );
        }
    }

    #[test]
    fn a_binding_without_a_semicolon_is_closed_at_the_newline() {
        for newline in ["\n", "\r\n"] {
            let source = format!("let q = \"secret-key\"{newline}    search_query(\"docs\", q, 3");
            let spans = redacted_tolerant(&source);

            assert!(
                spans.iter().any(|(content, _)| content == "secret-key"),
                "a newline-terminated binding leaked with {newline:?}: {spans:?}"
            );
        }
    }

    #[test]
    fn a_multiline_binding_value_is_not_terminated_early() {
        // The binding spans several lines inside a call, so the newline rule
        // must not close it before its literals are collected.
        let source = "let payload = json_encode(record {\n    value: \"multiline-payload\",\n})\n                      cache_put(\"lookups\", \"k\", payload, 60";
        let spans = redacted_tolerant(source);

        assert!(
            spans
                .iter()
                .any(|(content, _)| content == "multiline-payload"),
            "a multiline binding leaked: {spans:?}"
        );
    }

    #[test]
    fn tolerant_shadowing_uses_the_most_recent_binding() {
        let source =
            "let q = \"outer-question\";\nlet q = \"inner-question\";\nsearch_query(\"docs\", q, 3";
        let spans = redacted_tolerant(source);

        assert!(
            spans.iter().any(|(content, _)| content == "inner-question"),
            "the shadowing binding must be redacted: {spans:?}"
        );
    }

    #[test]
    fn tolerant_alias_chains_never_redact_less_than_the_parsed_path() {
        // The same fixtures in well-formed source: the fallback must match.
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let first = "chain-origin";
    let second = first;
    let third = second;
    match cache_put("lookups", third, "v", 60) {
        Ok(stored) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;
        let parsed = redacted(source);
        let tolerant = redacted_tolerant(source);

        assert!(
            parsed.iter().any(|(content, _)| content == "chain-origin"),
            "the parsed path must redact the chain: {parsed:?}"
        );
        for (content, _) in &parsed {
            assert!(
                tolerant.iter().any(|(other, _)| other == content),
                "the fallback redacted less than the parsed path: missing `{content}`"
            );
        }
    }

    #[test]
    fn tolerant_alias_tracking_leaves_unrelated_bindings_visible() {
        let source = "let visible = \"ordinary copy\";\nlet alias = visible;\nprintln(alias)\n                      let q = \"private question\";\nsearch_query(\"docs\", q, 3";
        let spans = redacted_tolerant(source);

        assert!(
            spans
                .iter()
                .any(|(content, _)| content == "private question"),
            "the query must be redacted: {spans:?}"
        );
        assert!(
            !spans.iter().any(|(content, _)| content == "ordinary copy"),
            "an unrelated binding must stay visible: {spans:?}"
        );
    }

    /// Applies the tolerant redaction and returns the resulting document.
    fn redact_all_tolerant(source: &str) -> String {
        let mut spans = tolerant_sensitive_literal_spans(source);
        spans.sort_by_key(|(start, _, _)| *start);
        let mut output = String::new();
        let mut cursor = 0;
        for (start, end, category) in spans {
            if start < cursor {
                continue;
            }
            output.push_str(&source[cursor..start]);
            output.push_str(&format!("<redacted:{category}>"));
            cursor = end;
        }
        output.push_str(&source[cursor..]);
        output
    }

    #[test]
    fn an_overflowing_binding_redacts_every_contributing_literal() {
        // At, just past, and far past the exact-tracking bound. No literal in
        // the binding's value may survive into the redacted document.
        for count in [64, 65, 200, 512] {
            let items = (0..count)
                .map(|index| format!("\"ordinary-{index}\""))
                .collect::<Vec<_>>()
                .join(", ");
            for (call, tail) in [
                ("cache_put(\"lookups\", \"k\", payload, 60", "cache value"),
                ("search_query(\"docs\", payload, 3", "search query"),
                ("vector_search(\"vectors\", payload, 3", "search vector"),
                ("cache_put(\"lookups\", payload, \"v\", 60", "cache key"),
            ] {
                let source = format!("let payload = [{items}];\n{call}");
                let redacted = redact_all_tolerant(&source);
                for index in 0..count {
                    assert!(
                        !redacted.contains(&format!("ordinary-{index}")),
                        "literal {index} of {count} leaked through the {tail} position"
                    );
                }
            }
        }
    }

    #[test]
    fn an_overflowing_alias_chain_propagates_conservatively() {
        let items = (0..200)
            .map(|index| format!("\"deep-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!(
            "let a = [{items}];\nlet b = a;\nlet c = b;\ncache_put(\"lookups\", \"k\", c, 60"
        );

        let redacted = redact_all_tolerant(&source);

        for index in 0..200 {
            assert!(
                !redacted.contains(&format!("deep-{index}")),
                "literal {index} leaked through a three-level overflowing chain"
            );
        }
    }

    #[test]
    fn an_overflowing_binding_leaves_unrelated_text_visible() {
        let items = (0..100)
            .map(|index| format!("\"inside-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!(
            "let outside = \"outside-value\";\nprintln(outside)\nlet payload = [{items}];\ncache_put(\"lookups\", \"k\", payload, 60"
        );

        let redacted = redact_all_tolerant(&source);

        assert!(
            !redacted.contains("inside-0") && !redacted.contains("inside-99"),
            "the overflowing binding must be fully redacted"
        );
        assert!(
            redacted.contains("outside-value"),
            "text outside the binding must stay visible: {redacted}"
        );
    }

    #[test]
    fn redacted_spans_always_land_on_character_boundaries() {
        // Multi-byte content around and inside an overflowing binding.
        let items = (0..80)
            .map(|index| format!("\"caf\u{e9}-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let source =
            format!("let payload = [{items}];\ncache_put(\"lookups\", \"k\u{e9}\", payload, 60");

        // Slicing inside the redactor would panic on a non-boundary index.
        let redacted = redact_all_tolerant(&source);

        assert!(redacted.is_char_boundary(0));
        for index in 0..80 {
            assert!(
                !redacted.contains(&format!("caf\u{e9}-{index}")),
                "literal {index} leaked"
            );
        }
    }

    #[test]
    fn a_nested_block_restores_the_outer_binding_after_it_closes() {
        // Inside the block the inner binding wins; after it closes the outer
        // binding is visible again.
        let source = "let q = \"outer-question\";\nif true {\n    let q = \"inner-question\";\n    search_query(\"docs\", q, 3)\n}\nsearch_query(\"docs\", q, 3";
        let spans = redacted_tolerant(source);

        assert!(
            spans.iter().any(|(content, _)| content == "inner-question"),
            "the inner binding must be redacted inside the block: {spans:?}"
        );
        assert!(
            spans.iter().any(|(content, _)| content == "outer-question"),
            "the outer binding must be redacted after the block: {spans:?}"
        );
    }

    #[test]
    fn a_binding_declared_only_inside_a_block_does_not_escape_it() {
        let source =
            "if true {\n    let inner = \"block-only\";\n}\nsearch_query(\"docs\", inner, 3";
        let spans = redacted_tolerant(source);

        // The name is out of scope after the block, so nothing resolves and
        // nothing unrelated is redacted.
        assert!(
            !spans.iter().any(|(content, _)| content == "block-only"),
            "a block-local binding must not resolve outside its scope: {spans:?}"
        );
    }

    #[test]
    fn two_or_more_nested_scopes_resolve_innermost_first() {
        let source = "let q = \"level-one\";\nif true {\n    let q = \"level-two\";\n    if true {\n        let q = \"level-three\";\n        search_query(\"docs\", q, 3)\n    }\n}";
        let spans = redacted_tolerant(source);

        assert!(
            spans.iter().any(|(content, _)| content == "level-three"),
            "the innermost binding must win: {spans:?}"
        );
        for outer in ["level-one", "level-two"] {
            assert!(
                !spans.iter().any(|(content, _)| content == outer),
                "`{outer}` must not be redacted by an inner use: {spans:?}"
            );
        }
    }

    #[test]
    fn a_parameter_shadow_stops_an_outer_binding_trace() {
        let source = "let question = \"caller-question\";\nfn lookup(question: String) -> HttpResponse {\n    search_query(\"docs\", question, 3)\n}";
        let spans = redacted_tolerant(source);

        assert!(
            !spans
                .iter()
                .any(|(content, _)| content == "caller-question"),
            "a parameter must not trace to a same-named outer binding: {spans:?}"
        );
    }

    #[test]
    fn a_webhook_parameter_shadow_also_stops_the_trace() {
        let source = "let request = \"outer-request\";\nwebhook fn handle(request: HttpRequest) -> HttpResponse {\n    search_query(\"docs\", request, 3)\n}";
        let spans = redacted_tolerant(source);

        assert!(
            !spans.iter().any(|(content, _)| content == "outer-request"),
            "a webhook parameter must shadow an outer binding: {spans:?}"
        );
    }

    #[test]
    fn an_unclosed_block_still_resolves_bindings_conservatively() {
        let source = "webhook fn handle(request: HttpRequest) -> HttpResponse {\n    let q = \"unclosed-question\";\n    search_query(\"docs\", q, 3";
        let spans = redacted_tolerant(source);

        assert!(
            spans
                .iter()
                .any(|(content, _)| content == "unclosed-question"),
            "an unclosed block must still protect its bindings: {spans:?}"
        );
    }

    #[test]
    fn a_record_literal_does_not_open_a_binding_scope() {
        // `record { ... }` groups commas but must not discard the binding
        // declared before it.
        let source = "let q = \"record-question\";\nlet shape = record { a: 1 };\nsearch_query(\"docs\", q, 3";
        let spans = redacted_tolerant(source);

        assert!(
            spans
                .iter()
                .any(|(content, _)| content == "record-question"),
            "a record literal must not drop an outer binding: {spans:?}"
        );
    }

    #[test]
    fn the_fallback_never_redacts_less_than_the_parsed_path() {
        // A corpus of well-formed fixtures: every literal the parsed path
        // redacts must also be redacted by the fallback.
        for source in [
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let question = "corpus-query";
    match search_query("docs", question, 3) {
        Ok(hits) => record { status: 200, headers: [], body: hits },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let a = "corpus-chain";
    let b = a;
    match cache_put("lookups", b, json_encode(record { value: "corpus-nested" }), 60) {
        Ok(stored) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    match vector_search("vectors", json_encode([1, "corpus-vector"]), 3) {
        Ok(near) => record { status: 200, headers: [], body: near },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
            r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let key = "corpus-key";
    match cache_delete("lookups", key) {
        Ok(gone) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#,
        ] {
            let parsed = redacted(source);
            let tolerant = redacted_tolerant(source);
            assert!(
                !parsed.is_empty(),
                "the fixture should redact something: {source}"
            );
            for (content, _) in &parsed {
                assert!(
                    tolerant.iter().any(|(other, _)| other == content),
                    "the fallback redacted less than the parsed path: `{content}` in {source}"
                );
            }
        }
    }

    #[test]
    fn an_alias_snapshots_the_binding_in_scope_at_its_declaration() {
        // Valid source: `alias` captures the *first* `q`. A later same-named
        // binding must not retarget it.
        for (call, private) in [
            ("search_query(\"docs\", alias, 3)", "private-query"),
            ("cache_put(\"lookups\", alias, \"v\", 60)", "private-key"),
            ("cache_put(\"lookups\", \"k\", alias, 60)", "private-value"),
            ("cache_delete(\"lookups\", alias)", "private-delete-key"),
            ("cache_get(\"lookups\", alias)", "private-get-key"),
            ("vector_search(\"vectors\", alias, 3)", "private-vector"),
        ] {
            let source = format!(
                "webhook fn handle(request: HttpRequest) -> HttpResponse {{\n                     let q = \"{private}\";\n                     let alias = q;\n                     let q = \"ordinary-shadow\";\n                     let outcome = {call};\n                     record {{ status: 200, headers: [], body: \"ok\" }}\n}}\n"
            );
            let spans = redacted(&source);

            assert!(
                spans.iter().any(|(content, _)| content == private),
                "`{private}` leaked: the alias resolved against a later shadow: {spans:?}"
            );
            assert!(
                !spans
                    .iter()
                    .any(|(content, _)| content == "ordinary-shadow"),
                "the later ordinary binding must stay visible: {spans:?}"
            );
        }
    }

    #[test]
    fn a_block_valued_binding_is_finalised_in_its_declaring_scope() {
        // The value opens and closes a scope; the binding itself belongs to the
        // scope it was declared in and must survive.
        for value in [
            "{ \"private-block\" }",
            "if true { \"private-block\" } else { \"other\" }",
            "record { field: \"private-block\" }",
            "[\"private-block\"]",
            "json_encode(record { field: \"private-block\" })",
        ] {
            let source =
                format!("let payload = {value};\ncache_put(\"lookups\", \"k\", payload, 60");
            let spans = redacted_tolerant(&source);

            assert!(
                spans.iter().any(|(content, _)| content == "private-block"),
                "a {value} valued binding leaked: {spans:?}"
            );
        }
    }

    #[test]
    fn a_binding_declared_inside_a_value_block_still_contributes() {
        let source = "let outer = { let inner = \"nested-private\"; inner };\ncache_put(\"lookups\", \"k\", outer, 60";
        let spans = redacted_tolerant(source);

        assert!(
            spans.iter().any(|(content, _)| content == "nested-private"),
            "a nested binding inside a value must contribute: {spans:?}"
        );
    }

    #[test]
    fn a_mismatched_closer_never_discards_a_live_binding() {
        for source in [
            "{ let q = \"private-mismatch\"; ] search_query(\"docs\", q, 3",
            "{ let q = \"private-mismatch\"; ) search_query(\"docs\", q, 3",
            "let q = \"private-mismatch\";\n) search_query(\"docs\", q, 3",
            "let q = \"private-mismatch\";\n]]] search_query(\"docs\", q, 3",
            "[ let q = \"private-mismatch\"; } search_query(\"docs\", q, 3",
        ] {
            let spans = redacted_tolerant(source);
            assert!(
                spans
                    .iter()
                    .any(|(content, _)| content == "private-mismatch"),
                "a mismatched closer discarded a live binding in `{source}`: {spans:?}"
            );
        }
    }

    #[test]
    fn an_unclosed_binding_is_visible_to_a_later_sensitive_reference() {
        for source in [
            "let q = [\"private-unclosed\"\nsearch_query(\"docs\", q, 3",
            "let q = json_encode(\"private-unclosed\"\nsearch_query(\"docs\", q, 3",
            "let q = record { a: \"private-unclosed\"\nsearch_query(\"docs\", q, 3",
            "let q = [\n    \"private-unclosed\",\n    \"second\"\ncache_put(\"lookups\", \"k\", q, 60",
            "let a = [\"private-unclosed\"\nlet b = a\ncache_put(\"lookups\", \"k\", b, 60",
        ] {
            let spans = redacted_tolerant(source);
            let redacted_text = redact_all_tolerant(source);
            assert!(
                spans
                    .iter()
                    .any(|(content, _)| content == "private-unclosed")
                    || !redacted_text.contains("private-unclosed"),
                "an unclosed binding leaked in `{source}`: {spans:?}"
            );
            assert!(
                !redacted_text.contains("private-unclosed"),
                "an unclosed binding leaked into the document from `{source}`: {redacted_text}"
            );
        }
    }

    #[test]
    fn overlapping_regions_are_merged_so_no_tail_is_exposed() {
        // A secret-like literal sits inside an overflowing binding's value. The
        // shorter span sorts first; without a union merge the enclosing hull
        // would be skipped and its tail exposed.
        let mut items = vec!["\"api_key=AKIAIOSFODNN7EXAMPLE\"".to_owned()];
        items.extend((0..100).map(|index| format!("\"tail-{index}\"")));
        let source = format!(
            "let payload = [{}];\ncache_put(\"lookups\", \"k\", payload, 60",
            items.join(", ")
        );

        let redacted_text = redact_all_tolerant(&source);

        for index in 0..100 {
            assert!(
                !redacted_text.contains(&format!("tail-{index}")),
                "tail literal {index} was exposed by a skipped hull: {redacted_text}"
            );
        }
        assert!(!redacted_text.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn merging_intervals_uses_the_maximum_end() {
        let merged = merge_sensitive_intervals(vec![
            (10, 20, "secret-like"),
            (5, 40, "cache-value"),
            (60, 70, "cache-key"),
        ]);

        assert_eq!(
            merged,
            [(5, 40, "cache-value"), (60, 70, "cache-key")],
            "an enclosing interval must absorb a shorter one"
        );
        // Exactly equal intervals keep the first, most specific category.
        assert_eq!(
            merge_sensitive_intervals(vec![(5, 10, "cache-key"), (5, 10, "secret-like")]),
            [(5, 10, "cache-key")]
        );
        // Touching intervals merge rather than leaving a gap.
        assert_eq!(
            merge_sensitive_intervals(vec![(0, 5, "cache-key"), (5, 9, "cache-value")]),
            [(0, 9, "cache-key")]
        );
    }

    /// `let a = "PLAINTEXT"; let c = <collection>; <sensitive use of c>`
    ///
    /// `filler` controls how many extra literals `c` holds, which is what
    /// pushes the exact-span budget over its bound. `alias_first` moves the
    /// alias to either end so the result cannot depend on ordering.
    fn overflow_alias_fixture(
        filler: usize,
        alias_first: bool,
        record: bool,
        call: &str,
    ) -> String {
        let mut parts = Vec::with_capacity(filler + 1);
        if alias_first {
            parts.push("a".to_owned());
        }
        for index in 0..filler {
            parts.push(format!("\"filler-{index}\""));
        }
        if !alias_first {
            parts.push("a".to_owned());
        }
        let collection = if record {
            let fields = parts
                .iter()
                .enumerate()
                .map(|(index, part)| format!("f{index}: {part}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("record {{ {fields} }}")
        } else {
            format!("[{}]", parts.join(", "))
        };
        format!(
            "let before = \"visible-before\";\n             let a = \"PLAINTEXT\";\n             let c = {collection};\n             {call};\n             let after = \"visible-after\";\n"
        )
    }

    /// Applies the parsed-path redaction and returns the resulting document.
    fn redact_all(source: &str) -> String {
        apply_spans(source, sensitive_literal_spans(source))
    }

    fn apply_spans(source: &str, spans: Vec<(usize, usize, &'static str)>) -> String {
        let merged = merge_sensitive_intervals(spans);
        let mut output = String::new();
        let mut cursor = 0;
        for (start, end, category) in merged {
            if start < cursor {
                continue;
            }
            output.push_str(&source[cursor..start]);
            output.push_str(&format!("<redacted:{category}>"));
            cursor = end;
        }
        output.push_str(&source[cursor..]);
        output
    }

    #[test]
    fn overflow_never_loses_an_alias_contributor_from_an_earlier_statement() {
        // 63 filler literals plus the alias is exactly the exact-span bound;
        // 64 tips it over. The payload lives in an earlier statement, outside
        // the consuming expression, so clearing spans on overflow would expose
        // it. Both siblings must hide it.
        for filler in [63, 64, 200] {
            for alias_first in [true, false] {
                for record in [true, false] {
                    for call in [
                        "cache_put(\"lookups\", \"k\", c, 60)",
                        "cache_put(\"lookups\", c, \"v\", 60)",
                        "search_query(\"docs\", c, 3)",
                        "vector_search(\"vectors\", c, 3)",
                        "cache_delete(\"lookups\", c)",
                    ] {
                        let source = overflow_alias_fixture(filler, alias_first, record, call);
                        for (label, redacted_text) in [
                            ("parsed", redact_all(&source)),
                            ("tolerant", redact_all_tolerant(&source)),
                        ] {
                            assert!(
                                !redacted_text.contains("PLAINTEXT"),
                                "{label} path leaked the alias payload \
                                 (filler={filler}, alias_first={alias_first}, record={record}, \
                                 call={call}):\n{redacted_text}"
                            );
                            for index in 0..filler {
                                assert!(
                                    !redacted_text.contains(&format!("filler-{index}")),
                                    "{label} path leaked filler {index} \
                                     (filler={filler}, alias_first={alias_first})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn overflow_leaves_literals_outside_the_contributing_ranges_visible() {
        let source =
            overflow_alias_fixture(100, true, false, "cache_put(\"lookups\", \"k\", c, 60)");

        for (label, redacted_text) in [
            ("parsed", redact_all(&source)),
            ("tolerant", redact_all_tolerant(&source)),
        ] {
            assert!(
                !redacted_text.contains("PLAINTEXT"),
                "{label} path leaked the payload"
            );
            assert!(
                redacted_text.contains("visible-before"),
                "{label} path over-redacted text before the contributors: {redacted_text}"
            );
            assert!(
                redacted_text.contains("visible-after"),
                "{label} path over-redacted text after the contributors: {redacted_text}"
            );
        }
    }

    #[test]
    fn overflow_survives_a_two_level_alias_chain() {
        let filler = (0..80)
            .map(|index| format!("\"chain-filler-{index}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!(
            "let a = \"PLAINTEXT\";\nlet b = [a];\nlet c = [b, {filler}];\n             cache_put(\"lookups\", \"k\", c, 60);\n"
        );

        for (label, redacted_text) in [
            ("parsed", redact_all(&source)),
            ("tolerant", redact_all_tolerant(&source)),
        ] {
            assert!(
                !redacted_text.contains("PLAINTEXT"),
                "{label} path lost a two-level alias contributor: {redacted_text}"
            );
        }
    }

    #[test]
    fn a_self_shadowing_binding_resolves_the_outer_value() {
        // `let token = token;` is a legal rebind whose right-hand side names
        // the outer binding. Resolving it to the empty declaration under way
        // would expose the outer payload.
        let same_scope = "let token = \"outer-token\";\nlet token = token;\n                          cache_put(\"lookups\", \"k\", token, 60";
        let nested = "let token = \"outer-token\";\n{\n    let token = token;\n                          cache_put(\"lookups\", \"k\", token, 60";
        let twice_nested = "let token = \"outer-token\";\n{\n    let token = token;\n    {\n                                    let token = token;\n        search_query(\"docs\", token, 3";

        for (label, source) in [
            ("same scope", same_scope),
            ("nested", nested),
            ("twice nested", twice_nested),
        ] {
            let redacted_text = redact_all_tolerant(source);
            assert!(
                !redacted_text.contains("outer-token"),
                "the {label} self-shadow leaked the outer value: {redacted_text}"
            );
        }
    }

    #[test]
    fn a_self_shadowing_binding_matches_the_parsed_path() {
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let token = "outer-token";
    let token = token;
    match cache_put("lookups", "k", token, 60) {
        Ok(stored) => record { status: 200, headers: [], body: "ok" },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;
        let parsed = redacted(source);
        let tolerant = redacted_tolerant(source);

        assert!(
            parsed.iter().any(|(content, _)| content == "outer-token"),
            "the parsed path must redact the rebound outer value: {parsed:?}"
        );
        for (content, _) in &parsed {
            assert!(
                tolerant.iter().any(|(other, _)| other == content),
                "the fallback redacted less than the parsed path: `{content}`"
            );
        }
    }

    #[test]
    fn a_self_reference_without_an_outer_binding_terminates() {
        // No outer `token` exists, so the reference resolves to the unfinished
        // declaration itself. It must not loop, and must stay conservative.
        let source = "let token = [token, \"private-self\"];\n                      cache_put(\"lookups\", \"k\", token, 60";

        let redacted_text = redact_all_tolerant(source);

        assert!(
            !redacted_text.contains("private-self"),
            "a self-referencing binding leaked: {redacted_text}"
        );
    }

    #[test]
    fn a_parameter_still_shadows_an_outer_binding_under_self_reference() {
        let source = "let token = \"caller-token\";\n                      fn lookup(token: String) -> HttpResponse {\n                          search_query(\"docs\", token, 3)\n}";
        let spans = redacted_tolerant(source);

        assert!(
            !spans.iter().any(|(content, _)| content == "caller-token"),
            "a parameter must remain opaque: {spans:?}"
        );
    }

    #[test]
    fn simultaneous_same_name_pending_declarations_keep_the_finalized_outer() {
        // Two same-named declarations are open at once. Neither is visible from
        // its own initializer, so the contributor is the finalized outer
        // binding; a pending declaration must not hide it.
        for (label, call) in [
            ("search", "search_query(\"docs\", token, 3"),
            ("cache value", "cache_put(\"lookups\", \"k\", token, 60"),
            ("cache key", "cache_put(\"lookups\", token, \"v\", 60"),
            ("cache delete", "cache_delete(\"lookups\", token"),
            ("cache get", "cache_get(\"lookups\", token"),
            ("vector", "vector_search(\"vectors\", token, 3"),
        ] {
            // Two nested pending declarations.
            let two = format!(
                "let token = \"customer payload\";\n{{\n    let token = {{\n                         let token = token;\n        {call}"
            );
            // Three nested pending declarations.
            let three = format!(
                "let token = \"customer payload\";\n{{\n    let token = {{\n                         let token = {{\n            let token = token;\n            {call}"
            );
            for (depth, source) in [("two", two), ("three", three)] {
                let redacted_text = redact_all_tolerant(&source);
                assert!(
                    !redacted_text.contains("customer payload"),
                    "{depth} nested pending shadows leaked through the {label} position:\n                     {redacted_text}"
                );
            }
        }
    }

    #[test]
    fn a_parameter_barrier_blocks_an_outer_pending_declaration() {
        // The outer `token` declaration is still open, but a parameter of the
        // same name shadows it. The barrier must apply to the pending
        // declaration as well as to finalized bindings.
        let source = "let outer = \"caller-token\";\nlet token = [outer,\n                      fn lookup(token: String) -> HttpResponse {\n                          search_query(\"docs\", token, 3)\n}";
        let spans = redacted_tolerant(source);

        assert!(
            !spans.iter().any(|(content, _)| content == "caller-token"),
            "a pending declaration bypassed a parameter barrier: {spans:?}"
        );
    }

    #[test]
    fn a_sibling_scope_binding_is_not_visible_after_it_closes() {
        // `token` is declared and finalized inside a sibling block, so it is
        // out of scope at the later use and must not be redacted.
        let source = "{\n    let token = \"sibling-only\";\n}\n                      search_query(\"docs\", token, 3";
        let spans = redacted_tolerant(source);

        assert!(
            !spans.iter().any(|(content, _)| content == "sibling-only"),
            "an out-of-scope sibling binding was redacted: {spans:?}"
        );
    }

    #[test]
    fn nested_same_name_pending_shadows_match_the_parsed_path() {
        // The well-formed equivalent of the nested-pending fixture.
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let token = "customer payload";
    let outer = {
        let token = {
            let token = token;
            token
        };
        token
    };
    match search_query("docs", outer, 3) {
        Ok(hits) => record { status: 200, headers: [], body: hits },
        Err(problem) => record { status: 500, headers: [], body: problem },
    }
}
"#;
        let parsed = redacted(source);
        let tolerant = redacted_tolerant(source);

        assert!(
            parsed
                .iter()
                .any(|(content, _)| content == "customer payload"),
            "the parsed path must redact the outer value: {parsed:?}"
        );
        for (content, _) in &parsed {
            assert!(
                tolerant.iter().any(|(other, _)| other == content),
                "the fallback redacted less than the parsed path: `{content}`"
            );
        }
    }

    #[test]
    fn unrelated_scopes_are_not_over_redacted_by_pending_union() {
        let source = "let unrelated = \"visible-copy\";\n                      let token = \"private-token\";\n{\n    let token = token;\n                          search_query(\"docs\", token, 3";
        let redacted_text = redact_all_tolerant(source);

        assert!(
            !redacted_text.contains("private-token"),
            "the private token leaked: {redacted_text}"
        );
        assert!(
            redacted_text.contains("visible-copy"),
            "an unrelated binding was over-redacted: {redacted_text}"
        );
    }

    #[test]
    fn unrelated_literals_stay_visible_alongside_redacted_ones() {
        let source = r#"
webhook fn handle(request: HttpRequest) -> HttpResponse {
    let visible = "ordinary business copy";
    let question = "private customer question";
    match search_query("docs", question, 3) {
        Ok(hits) => record { status: 200, headers: [], body: visible },
        Err(problem) => record { status: 500, headers: [], body: "plain error text" },
    }
}
"#;

        for spans in [redacted(source), redacted_tolerant(source)] {
            assert!(
                spans
                    .iter()
                    .any(|(content, _)| content == "private customer question"),
                "the query must be redacted: {spans:?}"
            );
            for visible in ["ordinary business copy", "plain error text"] {
                assert!(
                    !spans.iter().any(|(content, _)| content == visible),
                    "`{visible}` must stay visible: {spans:?}"
                );
            }
        }
    }

    #[test]
    fn a_non_sensitive_argument_is_not_redacted() {
        // Only the declared positions are redacted; an ordinary literal that is
        // neither a resource, key, nor payload stays visible.
        let spans = redacted_tolerant("println(\"ordinary output\")");

        assert!(
            !spans
                .iter()
                .any(|(content, _)| content == "ordinary output"),
            "an ordinary literal must not be redacted: {spans:?}"
        );
    }
}
