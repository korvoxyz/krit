use std::{
    collections::BTreeMap,
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
    let mut overlaps = sensitive
        .into_iter()
        .filter_map(|(start, end, category)| {
            let start = start.max(selected.start_byte);
            let end = end.min(selected.end_byte);
            (start < end).then_some((start, end, category))
        })
        .collect::<Vec<_>>();
    overlaps.sort_by_key(|(start, end, _)| (*start, *end));
    let index = Utf16Index::new(source);
    let mut output = String::new();
    let mut redactions = Vec::new();
    let mut cursor = selected.start_byte;
    for (start, end, category) in overlaps {
        if start < cursor {
            continue;
        }
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

fn sensitive_literal_spans(source: &str) -> Vec<(usize, usize, &'static str)> {
    if let Ok(program) = krit::parse_source(&Source::new("<assist-redaction>", source)) {
        return ast_sensitive_literal_spans(source, &program);
    }
    tolerant_sensitive_literal_spans(source)
}

fn ast_sensitive_literal_spans(
    source: &str,
    program: &Program,
) -> Vec<(usize, usize, &'static str)> {
    fn statement_spans(
        source: &str,
        statement: &Statement,
        spans: &mut BTreeMap<(usize, usize), &'static str>,
    ) {
        match &statement.kind {
            StatementKind::Let { value, .. } => expression_spans(source, value, spans),
            StatementKind::Function { body, .. } | StatementKind::Webhook { body, .. } => {
                block_spans(source, body, spans);
            }
            StatementKind::Expression(expression) => expression_spans(source, expression, spans),
        }
    }

    fn block_spans(
        source: &str,
        block: &Block,
        spans: &mut BTreeMap<(usize, usize), &'static str>,
    ) {
        for statement in &block.statements {
            statement_spans(source, statement, spans);
        }
        if let Some(tail) = block.tail.as_deref() {
            expression_spans(source, tail, spans);
        }
    }

    fn expression_spans(
        source: &str,
        expression: &Expression,
        spans: &mut BTreeMap<(usize, usize), &'static str>,
    ) {
        if let ExpressionKind::Literal(ValueLiteral::String(value)) = &expression.kind
            && looks_secret_like(value)
            && let Some((start, end)) =
                literal_content_span(source, expression.span.start, expression.span.end)
        {
            spans.insert((start, end), "secret-like");
        }
        match &expression.kind {
            ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
            ExpressionKind::List(elements) => {
                for element in elements {
                    expression_spans(source, element, spans);
                }
            }
            ExpressionKind::Record(fields) => {
                for field in fields {
                    expression_spans(source, &field.value, spans);
                }
            }
            ExpressionKind::FieldAccess { value, .. } => expression_spans(source, value, spans),
            ExpressionKind::Block(block) => block_spans(source, block, spans),
            ExpressionKind::If {
                condition,
                consequent,
                alternative,
            } => {
                expression_spans(source, condition, spans);
                block_spans(source, consequent, spans);
                expression_spans(source, alternative, spans);
            }
            ExpressionKind::Function { body, .. } => block_spans(source, body, spans),
            ExpressionKind::Call { callee, arguments } => {
                if let ExpressionKind::Variable(name) = &callee.kind
                    && let Some(argument) = arguments.first()
                    && matches!(
                        argument.kind,
                        ExpressionKind::Literal(ValueLiteral::String(_))
                    )
                    && let Some((start, end)) =
                        literal_content_span(source, argument.span.start, argument.span.end)
                {
                    let category = match name.as_str() {
                        "ai_invoke" | "config_string" | "http_request" | "secret" => {
                            Some("capability-resource")
                        }
                        "log_error" | "log_info" => Some("event-name"),
                        _ => None,
                    };
                    if let Some(category) = category {
                        spans.insert((start, end), category);
                    }
                }
                expression_spans(source, callee, spans);
                for argument in arguments {
                    expression_spans(source, argument, spans);
                }
            }
            ExpressionKind::Match { subject, kind } => {
                expression_spans(source, subject, spans);
                match kind {
                    MatchKind::List {
                        empty_case,
                        cons_case,
                        ..
                    } => {
                        expression_spans(source, empty_case, spans);
                        expression_spans(source, cons_case, spans);
                    }
                    MatchKind::Variants { arms, .. } => {
                        for arm in arms {
                            expression_spans(source, &arm.value, spans);
                        }
                    }
                }
            }
            ExpressionKind::Unary { operand, .. } => expression_spans(source, operand, spans),
            ExpressionKind::Binary { left, right, .. } => {
                expression_spans(source, left, spans);
                expression_spans(source, right, spans);
            }
        }
    }

    let mut spans = BTreeMap::new();
    for statement in &program.statements {
        statement_spans(source, statement, &mut spans);
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

fn tolerant_sensitive_literal_spans(source: &str) -> Vec<(usize, usize, &'static str)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut tokens = Vec::new();
    let mut cursor = 0;
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
            tokens.push(TolerantToken::Identifier(&source[start..cursor]));
            continue;
        }
        match bytes[cursor] {
            b'(' => {
                tokens.push(TolerantToken::LeftParen);
                cursor += 1;
                continue;
            }
            b')' => {
                tokens.push(TolerantToken::RightParen);
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
        let category = preceding_token_call(&tokens)
            .and_then(|name| match name {
                "ai_invoke" | "config_string" | "http_request" | "secret" => {
                    Some("capability-resource")
                }
                "log_error" | "log_info" => Some("event-name"),
                _ => None,
            })
            .or_else(|| looks_secret_like(content).then_some("secret-like"));
        if let Some(category) = category {
            spans.push((content_start, content_end, category));
        }
        if cursor < bytes.len() {
            cursor += 1;
        }
        tokens.push(TolerantToken::Other);
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

fn preceding_token_call<'a>(tokens: &[TolerantToken<'a>]) -> Option<&'a str> {
    if !matches!(tokens.last(), Some(TolerantToken::LeftParen)) {
        return None;
    }
    grouped_token_identifier(&tokens[..tokens.len() - 1])
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
                if name == "resource" && value.is_string() {
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
}
