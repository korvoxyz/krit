use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use krit::{
    Analysis, Block, Builtin, Diagnostic as CompilerDiagnostic, Effect, EntrypointKind, Expression,
    ExpressionKind, MatchKind, Program, RequirementSet, ResolvedName, Severity, Source, Span,
    Statement, StatementKind, SymbolKind as CompilerSymbolKind, Type, TypeAnnotation,
};
use krit_package::Manifest;
use lsp_server::ErrorCode;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CompletionItem, CompletionItemKind,
    CompletionList, CompletionResponse, CompletionTextEdit, Diagnostic, DiagnosticSeverity,
    DocumentChanges, DocumentSymbol, DocumentSymbolResponse, Documentation, Hover, HoverContents,
    MarkupContent, MarkupKind, NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, PublishDiagnosticsParams, Range, SymbolKind, TextDocumentEdit, TextEdit, Uri,
    WorkspaceEdit,
};
use serde::Serialize;
use url::Url;

use crate::position::LineIndex;

pub(crate) const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_DOCUMENT_URI_BYTES: usize = 8 * 1024;
const MAX_OPEN_DOCUMENTS: usize = 128;
const MAX_COMPLETION_ITEMS: usize = 512;
const MAX_DOCUMENT_SYMBOLS: usize = 4096;
const MAX_COMPILER_FACT_ITEMS: usize = 16_384;
const MAX_RENDERED_TYPE_BYTES: usize = 16 * 1024;
const MAX_COMPILER_FACT_DYNAMIC_BYTES: usize = 8 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_FIELD_REPAIR_WORK_BYTES: usize = 1024 * 1024;
const MAX_FIELD_REPAIR_ATTEMPTS: usize = 16;
/// The most analysis warnings one document publishes. A document can hold a
/// warning for every few bytes of source (each read of `_` is one), so without
/// this bound a document well under `MAX_DOCUMENT_BYTES` could need a
/// `publishDiagnostics` notification above the protocol frame limit.
const MAX_PUBLISHED_WARNINGS: usize = 100;
/// The most warning message bytes one document publishes, so that warnings
/// whose messages quote source stay bounded too.
const MAX_PUBLISHED_WARNING_MESSAGE_BYTES: usize = 64 * 1024;
/// The code of the notice that stands in for the warnings a document leaves
/// out of its published diagnostics.
const OMITTED_WARNINGS_CODE: &str = "K8003";
/// The most message bytes the error rejecting a document publishes. Analysis
/// messages render inferred types, and a type can grow far past the source
/// that spells it, so an unbounded message could need a `publishDiagnostics`
/// notification above the protocol frame limit.
const MAX_PUBLISHED_ERROR_MESSAGE_BYTES: usize = 64 * 1024;
/// The most bytes of each note the rejecting error publishes. Notes are fixed
/// text, but a did-you-mean note quotes a source name.
const MAX_PUBLISHED_ERROR_NOTE_BYTES: usize = 4 * 1024;
/// Ends published error text cut to its bound.
const TRUNCATION_MARKER: &str = " [truncated; `krit check` reports it in full]";

pub(crate) struct ServerState {
    workspace_roots: Vec<PathBuf>,
    documents: BTreeMap<Uri, Document>,
}

impl ServerState {
    pub(crate) fn new(mut workspace_roots: Vec<PathBuf>) -> Self {
        workspace_roots.sort();
        workspace_roots.dedup();
        Self {
            workspace_roots,
            documents: BTreeMap::new(),
        }
    }

    pub(crate) fn open(
        &mut self,
        uri: Uri,
        version: i32,
        text: String,
    ) -> PublishDiagnosticsParams {
        if uri.as_str().len() > MAX_DOCUMENT_URI_BYTES {
            return Self::rejected_document_diagnostics(
                uri,
                version,
                format!(
                    "document URI exceeds the {} byte language-server limit",
                    MAX_DOCUMENT_URI_BYTES
                ),
            );
        }
        if !self.documents.contains_key(&uri) && self.documents.len() >= MAX_OPEN_DOCUMENTS {
            return Self::rejected_document_diagnostics(
                uri,
                version,
                format!(
                    "language server already has the maximum {} open documents",
                    MAX_OPEN_DOCUMENTS
                ),
            );
        }
        let document = Document::new(uri.clone(), version, text, &self.workspace_roots);
        let diagnostics = document.publish_diagnostics();
        self.documents.insert(uri, document);
        diagnostics
    }

    pub(crate) fn open_with_manifest(
        &mut self,
        uri: Uri,
        version: i32,
        text: String,
        manifest_path: &Path,
        manifest: &Manifest,
    ) -> Result<PublishDiagnosticsParams, String> {
        if uri.as_str().len() > MAX_DOCUMENT_URI_BYTES {
            return Err("document URI exceeds the language-server limit".to_owned());
        }
        if !self.documents.contains_key(&uri) && self.documents.len() >= MAX_OPEN_DOCUMENTS {
            return Err("language server has too many open documents".to_owned());
        }
        let package = PackageContext::load_exact(&uri, manifest_path, manifest)?;
        let document = Document::with_package(uri.clone(), version, text, Some(package));
        let diagnostics = document.publish_diagnostics();
        self.documents.insert(uri, document);
        Ok(diagnostics)
    }

    fn rejected_document_diagnostics(
        uri: Uri,
        version: i32,
        message: String,
    ) -> PublishDiagnosticsParams {
        PublishDiagnosticsParams::new(
            uri,
            vec![Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("K8002".to_owned())),
                source: Some("krit-lsp".to_owned()),
                message,
                ..Diagnostic::default()
            }],
            Some(version),
        )
    }

    pub(crate) fn change(
        &mut self,
        uri: &Uri,
        version: i32,
        text: String,
    ) -> Result<PublishDiagnosticsParams, String> {
        let current = self
            .documents
            .get(uri)
            .ok_or_else(|| "document is not open".to_owned())?;
        if version <= current.version {
            return Err(format!(
                "document version {version} is not newer than {}",
                current.version
            ));
        }
        let document = Document::new(uri.clone(), version, text, &self.workspace_roots);
        let diagnostics = document.publish_diagnostics();
        self.documents.insert(uri.clone(), document);
        Ok(diagnostics)
    }

    pub(crate) fn close(&mut self, uri: &Uri) -> PublishDiagnosticsParams {
        self.documents.remove(uri);
        PublishDiagnosticsParams::new(uri.clone(), Vec::new(), None)
    }

    pub(crate) fn hover(&self, uri: &Uri, position: Position) -> Result<Option<Hover>, String> {
        self.document(uri)?.hover(position)
    }

    pub(crate) fn completion(
        &self,
        uri: &Uri,
        position: Position,
    ) -> Result<CompletionResponse, String> {
        self.document(uri)?.completion(position)
    }

    pub(crate) fn formatting(&self, uri: &Uri) -> Result<Vec<TextEdit>, String> {
        self.document(uri)?
            .formatting_edit()
            .map(|edit| edit.into_iter().collect())
    }

    pub(crate) fn document_symbols(
        &self,
        uri: &Uri,
    ) -> Result<Option<DocumentSymbolResponse>, String> {
        self.document(uri)?.document_symbols()
    }

    pub(crate) fn code_actions(&self, uri: &Uri) -> Result<Vec<CodeActionOrCommand>, String> {
        self.document(uri)?.code_actions()
    }

    pub(crate) fn compiler_facts(&self, uri: &Uri) -> Result<CompilerFacts, String> {
        self.document(uri)?.compiler_facts()
    }

    fn document(&self, uri: &Uri) -> Result<&Document, String> {
        self.documents
            .get(uri)
            .ok_or_else(|| "document is not open".to_owned())
    }
}

struct Document {
    uri: Uri,
    version: i32,
    source: Source,
    lines: LineIndex,
    analysis: Option<Analysis>,
    /// The analysis warnings the document publishes: a source-order prefix
    /// bounded by `MAX_PUBLISHED_WARNINGS` and
    /// `MAX_PUBLISHED_WARNING_MESSAGE_BYTES`.
    warnings: Vec<CompilerDiagnostic>,
    /// The analysis warnings left out of `warnings`, if any.
    omitted_warnings: Option<OmittedWarnings>,
    /// The parse or analysis error that rejected the document, if any, with
    /// its text bounded by [`bounded_error`]. It is always published, after
    /// the warnings and any omission notice.
    error: Option<CompilerDiagnostic>,
    formatted: Result<String, CompilerDiagnostic>,
    declarations: Vec<DeclarationFact>,
    syntax_kinds: BTreeMap<Span, &'static str>,
    durable_operations: Vec<krit::DurableOperationFact>,
    package: Option<PackageContext>,
    oversized: bool,
}

/// The analysis warnings a document leaves out of its published diagnostics.
/// One informational notice stands in for all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OmittedWarnings {
    count: usize,
    /// The span of the first warning left out, where the notice is reported.
    span: Span,
}

impl OmittedWarnings {
    fn message(self) -> String {
        if self.count == 1 {
            "1 more warning is not shown; `krit check` reports every warning".to_owned()
        } else {
            format!(
                "{} more warnings are not shown; `krit check` reports every warning",
                self.count
            )
        }
    }
}

/// The text a published diagnostic carries: the compiler message, then each
/// note on its own line as `help: <note>`, as the human format prints them.
fn published_message(diagnostic: &CompilerDiagnostic) -> String {
    let mut message = diagnostic.message().to_owned();
    for note in diagnostic.notes() {
        message.push_str("\nhelp: ");
        message.push_str(note);
    }
    message
}

/// Splits source-ordered analysis warnings into the prefix a document
/// publishes, at most `MAX_PUBLISHED_WARNINGS` warnings and
/// `MAX_PUBLISHED_WARNING_MESSAGE_BYTES` of published message text (notes
/// included), and the rest.
fn bound_warnings(
    mut warnings: Vec<CompilerDiagnostic>,
) -> (Vec<CompilerDiagnostic>, Option<OmittedWarnings>) {
    let mut message_bytes = 0_usize;
    let mut kept = 0;
    for warning in warnings.iter().take(MAX_PUBLISHED_WARNINGS) {
        message_bytes = message_bytes.saturating_add(published_message(warning).len());
        if message_bytes > MAX_PUBLISHED_WARNING_MESSAGE_BYTES {
            break;
        }
        kept += 1;
    }
    let omitted = warnings.get(kept).map(|first| OmittedWarnings {
        count: warnings.len() - kept,
        span: first.span(),
    });
    warnings.truncate(kept);
    // An open document holds only the published prefix, not an allocation
    // sized for every warning analysis found.
    warnings.shrink_to_fit();
    (warnings, omitted)
}

/// `text` unchanged when it fits in `limit` bytes, and otherwise cut at a
/// character boundary and ended with [`TRUNCATION_MARKER`], within `limit`.
fn bounded_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit.saturating_sub(TRUNCATION_MARKER.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = String::with_capacity(end + TRUNCATION_MARKER.len());
    bounded.push_str(&text[..end]);
    bounded.push_str(TRUNCATION_MARKER);
    bounded
}

/// The error rejecting a document as the document publishes it: its message
/// cut to `MAX_PUBLISHED_ERROR_MESSAGE_BYTES` and each note to
/// `MAX_PUBLISHED_ERROR_NOTE_BYTES`. Code, span, and every note are kept.
fn bounded_error(error: CompilerDiagnostic) -> CompilerDiagnostic {
    if error.message().len() <= MAX_PUBLISHED_ERROR_MESSAGE_BYTES
        && error
            .notes()
            .iter()
            .all(|note| note.len() <= MAX_PUBLISHED_ERROR_NOTE_BYTES)
    {
        return error;
    }
    let message = bounded_text(error.message(), MAX_PUBLISHED_ERROR_MESSAGE_BYTES);
    let bounded = match error.severity() {
        Severity::Error => CompilerDiagnostic::new(error.code(), message, error.span()),
        Severity::Warning => CompilerDiagnostic::warning(error.code(), message, error.span()),
    };
    error.notes().iter().fold(bounded, |bounded, note| {
        bounded.with_note(bounded_text(note, MAX_PUBLISHED_ERROR_NOTE_BYTES))
    })
}

impl Document {
    fn new(uri: Uri, version: i32, text: String, workspace_roots: &[PathBuf]) -> Self {
        let package = PackageContext::discover(&uri, workspace_roots);
        Self::with_package(uri, version, text, package)
    }

    fn with_package(uri: Uri, version: i32, text: String, package: Option<PackageContext>) -> Self {
        if text.len() > MAX_DOCUMENT_BYTES {
            let source = Source::new(document_name(&uri), "");
            return Self {
                uri,
                version,
                lines: LineIndex::new(source.text()),
                source,
                analysis: None,
                warnings: Vec::new(),
                omitted_warnings: None,
                error: None,
                formatted: Err(CompilerDiagnostic::new(
                    "K8002",
                    format!(
                        "document exceeds the {} byte language-server limit",
                        MAX_DOCUMENT_BYTES
                    ),
                    Span::new(0, 0),
                )),
                declarations: Vec::new(),
                syntax_kinds: BTreeMap::new(),
                durable_operations: Vec::new(),
                package: None,
                oversized: true,
            };
        }

        let source = Source::new(document_name(&uri), text);
        let lines = LineIndex::new(source.text());
        let formatted = krit::format_source(&source);
        let (program, analysis, all_warnings, error) = match krit::parse_source(&source) {
            Ok(program) => {
                let report = krit::analyze_with_warnings(&program);
                match report.result {
                    Ok(analysis) => (Some(program), Some(analysis), report.warnings, None),
                    Err(diagnostic) => (Some(program), None, report.warnings, Some(diagnostic)),
                }
            }
            Err(diagnostic) => (None, None, Vec::new(), Some(diagnostic)),
        };
        let (warnings, omitted_warnings) = bound_warnings(all_warnings);
        let error = error.map(bounded_error);
        let (declarations, syntax_kinds) = match (&program, &analysis) {
            (Some(program), Some(analysis)) => {
                DeclarationCollector::collect(source.text(), program, analysis, source.text().len())
            }
            _ => (Vec::new(), BTreeMap::new()),
        };
        let durable_operations = program
            .as_ref()
            .map(krit::durable_operations)
            .unwrap_or_default();

        Self {
            uri,
            version,
            source,
            lines,
            analysis,
            warnings,
            omitted_warnings,
            error,
            formatted,
            declarations,
            syntax_kinds,
            durable_operations,
            package,
            oversized: false,
        }
    }

    /// Publishes the bounded warnings in source order, then the notice for
    /// any warnings left out, then the rejecting error, if any.
    fn publish_diagnostics(&self) -> PublishDiagnosticsParams {
        let diagnostics = if self.oversized {
            vec![Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("K8002".to_owned())),
                source: Some("krit-lsp".to_owned()),
                message: format!(
                    "document exceeds the {} byte language-server limit",
                    MAX_DOCUMENT_BYTES
                ),
                ..Diagnostic::default()
            }]
        } else {
            let mut diagnostics = self
                .warnings
                .iter()
                .map(|diagnostic| self.lsp_diagnostic(diagnostic))
                .collect::<Vec<_>>();
            if let Some(omitted) = self.omitted_warnings {
                diagnostics.push(Diagnostic {
                    range: self.lines.range(self.source.text(), omitted.span),
                    severity: Some(DiagnosticSeverity::INFORMATION),
                    code: Some(NumberOrString::String(OMITTED_WARNINGS_CODE.to_owned())),
                    source: Some("krit-lsp".to_owned()),
                    message: omitted.message(),
                    ..Diagnostic::default()
                });
            }
            diagnostics.extend(
                self.error
                    .iter()
                    .map(|diagnostic| self.lsp_diagnostic(diagnostic)),
            );
            diagnostics
        };
        PublishDiagnosticsParams::new(self.uri.clone(), diagnostics, Some(self.version))
    }

    fn lsp_diagnostic(&self, diagnostic: &CompilerDiagnostic) -> Diagnostic {
        Diagnostic {
            range: self.lines.range(self.source.text(), diagnostic.span()),
            severity: Some(match diagnostic.severity() {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
            }),
            code: Some(NumberOrString::String(diagnostic.code().to_owned())),
            source: Some("krit".to_owned()),
            message: published_message(diagnostic),
            ..Diagnostic::default()
        }
    }

    fn hover(&self, position: Position) -> Result<Option<Hover>, String> {
        let offset = self.lines.offset(self.source.text(), position)?;
        let Some(analysis) = &self.analysis else {
            return Ok(None);
        };

        if let Some(declaration) = self
            .declarations
            .iter()
            .filter(|declaration| {
                declaration
                    .name_span
                    .is_some_and(|span| span_contains(span, offset))
            })
            .min_by_key(|declaration| {
                declaration
                    .name_span
                    .map_or(usize::MAX, |span| span.end - span.start)
            })
        {
            let symbol = &analysis.symbols()[declaration.symbol_index];
            let range = declaration
                .name_span
                .map(|span| self.lines.range(self.source.text(), span));
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: self.render_symbol_hover(symbol, declaration)?,
                }),
                range,
            }));
        }

        let Some(expression) = analysis
            .expressions()
            .iter()
            .filter(|expression| span_contains(expression.span(), offset))
            .min_by_key(|expression| expression.span().end - expression.span().start)
        else {
            return Ok(None);
        };
        let effect_context = analysis
            .expressions()
            .iter()
            .filter(|candidate| {
                span_contains(candidate.span(), offset)
                    && (!candidate.effects().is_empty() || !candidate.requirements().is_empty())
            })
            .min_by_key(|candidate| candidate.span().end - candidate.span().start);

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: self.render_expression_hover(expression, effect_context)?,
            }),
            range: Some(self.lines.range(self.source.text(), expression.span())),
        }))
    }

    fn completion(&self, position: Position) -> Result<CompletionResponse, String> {
        let offset = self.lines.offset(self.source.text(), position)?;
        if let Some(context) = resource_completion_context(self.source.text(), offset) {
            return Ok(self.limit_completions(self.resource_completions(context)));
        }
        if let Some(context) = field_completion_context(self.source.text(), offset) {
            return Ok(self.limit_completions(self.field_completions(context)?));
        }
        if type_completion_context(self.source.text(), offset) {
            return Ok(self.limit_completions(type_completions()));
        }

        let mut items = self.symbol_completions(offset)?;
        items.extend(builtin_completions());
        items.extend(keyword_completions());
        sort_completions(&mut items);
        items.dedup_by(|left, right| left.label == right.label);
        Ok(self.limit_completions(items))
    }

    fn resource_completions(&self, context: ResourceCompletionContext) -> Vec<CompletionItem> {
        let Some(package) = &self.package else {
            return Vec::new();
        };
        let resources = match context.builtin {
            Builtin::AiInvoke => &package.manifest.capabilities.ai,
            Builtin::ConfigString => &package.manifest.capabilities.config,
            // A secret is granted by the legacy list or by its own
            // `[secrets.<name>]` binding table.
            Builtin::Secret => {
                return self.build_resource_completions(
                    &context,
                    package,
                    &package.manifest.secret_names(),
                );
            }
            Builtin::HttpRequest => &package.manifest.capabilities.http,
            Builtin::StateGet
            | Builtin::StatePut
            | Builtin::StateDelete
            | Builtin::CheckpointGet
            | Builtin::CheckpointPut
            | Builtin::ReplayHttp
            | Builtin::ReplayAi => &package.manifest.capabilities.state,
            Builtin::QueuePublish => &package.manifest.capabilities.queues,
            Builtin::ObjectPut | Builtin::ObjectDelete => &package.manifest.capabilities.buckets,
            // A read grant is satisfied by the writable list *or* the read-only
            // list, so completion offers the union rather than half of it.
            Builtin::ObjectGet => {
                return self.build_resource_completions(
                    &context,
                    package,
                    &union_resources(
                        &package.manifest.capabilities.buckets,
                        &package.manifest.capabilities.read_only_buckets,
                    ),
                );
            }
            Builtin::DatabaseBeginWrite => &package.manifest.capabilities.databases,
            Builtin::DatabaseBeginRead => {
                return self.build_resource_completions(
                    &context,
                    package,
                    &union_resources(
                        &package.manifest.capabilities.databases,
                        &package.manifest.capabilities.read_only_databases,
                    ),
                );
            }
            Builtin::CachePut | Builtin::CacheDelete => {
                &package.manifest.capabilities.cache_namespaces
            }
            Builtin::CacheGet => {
                return self.build_resource_completions(
                    &context,
                    package,
                    &union_resources(
                        &package.manifest.capabilities.cache_namespaces,
                        &package.manifest.capabilities.read_only_cache_namespaces,
                    ),
                );
            }
            Builtin::SearchQuery => &package.manifest.capabilities.search_indexes,
            Builtin::VectorSearch => &package.manifest.capabilities.vector_indexes,
            _ => return Vec::new(),
        };
        self.build_resource_completions(&context, package, resources)
    }

    /// Builds completion items for one already-selected resource list.
    fn build_resource_completions(
        &self,
        context: &ResourceCompletionContext,
        package: &PackageContext,
        resources: &[String],
    ) -> Vec<CompletionItem> {
        let replacement = self.lines.range(
            self.source.text(),
            Span::new(context.content_start, context.cursor),
        );
        let capability = builtin_capability(context.builtin).unwrap_or("capability");
        let mut items = resources
            .iter()
            .map(|resource| CompletionItem {
                label: resource.clone(),
                kind: Some(CompletionItemKind::VALUE),
                detail: Some(format!(
                    "{capability} resource granted by package {}",
                    package.manifest.package.name
                )),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    replacement,
                    resource.clone(),
                ))),
                ..CompletionItem::default()
            })
            .collect::<Vec<_>>();
        sort_completions(&mut items);
        items
    }

    fn field_completions(
        &self,
        context: FieldCompletionContext,
    ) -> Result<Vec<CompletionItem>, String> {
        let Some(ty) = self.receiver_type(&context) else {
            return Ok(Vec::new());
        };
        let Some(fields) = ty.record_fields() else {
            return Ok(Vec::new());
        };
        let replacement = self.lines.range(
            self.source.text(),
            Span::new(context.replacement_start, context.cursor),
        );
        let mut items = Vec::with_capacity(fields.len().min(MAX_COMPLETION_ITEMS + 1));
        for field in fields.into_iter().take(MAX_COMPLETION_ITEMS + 1) {
            items.push(CompletionItem {
                label: field.name().to_owned(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(render_type(field.ty())?),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    replacement,
                    field.name().to_owned(),
                ))),
                ..CompletionItem::default()
            });
        }
        sort_completions(&mut items);
        Ok(items)
    }

    fn receiver_type(&self, context: &FieldCompletionContext) -> Option<Type> {
        if let Some(analysis) = &self.analysis
            && let Some(declaration) =
                self.visible_declarations(context.cursor)
                    .into_iter()
                    .find(|declaration| {
                        analysis.symbols()[declaration.symbol_index].name() == context.receiver
                    })
        {
            return Some(analysis.symbols()[declaration.symbol_index].ty().clone());
        }

        let mut without_field = self.source.text().to_owned();
        without_field.replace_range(context.dot..context.cursor, "");
        if let Some(ty) = receiver_type_from_source(&without_field, context) {
            return Some(ty);
        }

        let parsed =
            krit::parse_source(&Source::new("<lsp-field-candidates>", without_field)).ok()?;
        let mut candidates = field_candidates(&parsed);
        candidates.extend(
            [
                "body", "headers", "method", "name", "path", "query", "status", "value",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let attempts = field_repair_attempts(self.source.text().len());
        for candidate in candidates.into_iter().take(attempts) {
            let mut repaired = self.source.text().to_owned();
            repaired.replace_range(context.dot..context.cursor, &format!(".{candidate}"));
            if let Some(ty) = receiver_type_from_source(&repaired, context) {
                return Some(ty);
            }
        }
        None
    }

    fn symbol_completions(&self, offset: usize) -> Result<Vec<CompletionItem>, String> {
        let Some(analysis) = &self.analysis else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for declaration in self
            .visible_declarations(offset)
            .into_iter()
            .take(MAX_COMPLETION_ITEMS + 1)
        {
            let symbol = &analysis.symbols()[declaration.symbol_index];
            let ty = render_type(symbol.ty())?;
            items.push(CompletionItem {
                label: symbol.name().to_owned(),
                kind: Some(symbol_completion_kind(symbol.kind())),
                detail: Some(ty.clone()),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: symbol_documentation(symbol.kind(), &ty),
                })),
                ..CompletionItem::default()
            });
        }
        sort_completions(&mut items);
        Ok(items)
    }

    fn visible_declarations(&self, offset: usize) -> Vec<&DeclarationFact> {
        let Some(analysis) = &self.analysis else {
            return Vec::new();
        };
        let mut by_name = BTreeMap::<&str, &DeclarationFact>::new();
        for declaration in &self.declarations {
            if offset < declaration.visibility.start || offset > declaration.visibility.end {
                continue;
            }
            let symbol = &analysis.symbols()[declaration.symbol_index];
            match by_name.get(symbol.name()) {
                Some(existing)
                    if visibility_priority(existing.visibility)
                        >= visibility_priority(declaration.visibility) => {}
                _ => {
                    by_name.insert(symbol.name(), declaration);
                }
            }
        }
        by_name.into_values().collect()
    }

    fn limit_completions(&self, mut items: Vec<CompletionItem>) -> CompletionResponse {
        let is_incomplete = items.len() > MAX_COMPLETION_ITEMS;
        items.truncate(MAX_COMPLETION_ITEMS);
        CompletionResponse::List(CompletionList {
            is_incomplete,
            items,
        })
    }

    fn formatting_edit(&self) -> Result<Option<TextEdit>, String> {
        match &self.formatted {
            Ok(formatted) if formatted == self.source.text() => Ok(None),
            Ok(formatted) => Ok(Some(TextEdit::new(
                self.lines.full_range(self.source.text()),
                formatted.clone(),
            ))),
            Err(diagnostic) => Err(format!(
                "{}: cannot format document: {}",
                diagnostic.code(),
                diagnostic.message()
            )),
        }
    }

    #[allow(deprecated)]
    fn document_symbols(&self) -> Result<Option<DocumentSymbolResponse>, String> {
        let Some(analysis) = &self.analysis else {
            return Ok(None);
        };
        let mut declarations = self
            .declarations
            .iter()
            .filter(|declaration| analysis.symbols()[declaration.symbol_index].is_top_level())
            .collect::<Vec<_>>();
        declarations
            .sort_by_key(|declaration| analysis.symbols()[declaration.symbol_index].span().start);
        if declarations.len() > MAX_DOCUMENT_SYMBOLS {
            return Err(format!(
                "document contains more than {MAX_DOCUMENT_SYMBOLS} top-level symbols"
            ));
        }
        let mut render_budget = MAX_COMPILER_FACT_DYNAMIC_BYTES;
        let mut symbols = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let symbol = &analysis.symbols()[declaration.symbol_index];
            let range = self.lines.range(self.source.text(), symbol.span());
            let selection_range = declaration
                .name_span
                .map(|span| self.lines.range(self.source.text(), span))
                .unwrap_or(range);
            symbols.push(DocumentSymbol {
                name: symbol.name().to_owned(),
                detail: Some(symbol_detail(
                    symbol.kind(),
                    &render_fact_type(symbol.ty(), &mut render_budget)?,
                )),
                kind: document_symbol_kind(symbol.kind()),
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: None,
            });
        }
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    fn code_actions(&self) -> Result<Vec<CodeActionOrCommand>, String> {
        let Some(edit) = self.formatting_edit()? else {
            return Ok(Vec::new());
        };
        let workspace_edit = WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: self.uri.clone(),
                    version: Some(self.version),
                },
                edits: vec![OneOf::Left(edit)],
            }])),
            change_annotations: None,
        };
        Ok(vec![CodeActionOrCommand::CodeAction(CodeAction {
            title: "Format document with krit fmt".to_owned(),
            kind: Some(CodeActionKind::SOURCE_FIX_ALL),
            diagnostics: None,
            edit: Some(workspace_edit),
            is_preferred: Some(true),
            ..CodeAction::default()
        })])
    }

    fn compiler_facts(&self) -> Result<CompilerFacts, String> {
        let fact_items = self.analysis.as_ref().map_or(0, |analysis| {
            analysis.expressions().len() + analysis.symbols().len()
        });
        if fact_items > MAX_COMPILER_FACT_ITEMS {
            return Err(format!(
                "document requires {fact_items} compiler facts; maximum is {MAX_COMPILER_FACT_ITEMS}"
            ));
        }

        let diagnostics = if self.oversized {
            vec![CompilerDiagnosticFact {
                severity: "error",
                code: "K8002".to_owned(),
                message: format!(
                    "document exceeds the {} byte language-server limit",
                    MAX_DOCUMENT_BYTES
                ),
                notes: Vec::new(),
                span: ByteSpan { start: 0, end: 0 },
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            }]
        } else {
            // The same bounded list, in the same order, that the document
            // publishes.
            let compiler_fact = |diagnostic: &CompilerDiagnostic| CompilerDiagnosticFact {
                severity: diagnostic.severity().as_str(),
                code: diagnostic.code().to_owned(),
                message: diagnostic.message().to_owned(),
                notes: diagnostic.notes().to_vec(),
                span: diagnostic.span().into(),
                range: self.lines.range(self.source.text(), diagnostic.span()),
            };
            let mut diagnostics = self.warnings.iter().map(compiler_fact).collect::<Vec<_>>();
            if let Some(omitted) = self.omitted_warnings {
                diagnostics.push(CompilerDiagnosticFact {
                    severity: "info",
                    code: OMITTED_WARNINGS_CODE.to_owned(),
                    message: omitted.message(),
                    notes: Vec::new(),
                    span: omitted.span.into(),
                    range: self.lines.range(self.source.text(), omitted.span),
                });
            }
            diagnostics.extend(self.error.iter().map(compiler_fact));
            diagnostics
        };

        let analysis = self.analysis.as_ref();
        let referenced = analysis.map(referenced_symbols).unwrap_or_default();
        let mut fact_budget = MAX_COMPILER_FACT_DYNAMIC_BYTES;
        let mut symbols = Vec::new();
        let mut expressions = Vec::new();
        if let Some(analysis) = analysis {
            symbols.reserve(analysis.symbols().len());
            for (index, symbol) in analysis.symbols().iter().enumerate() {
                let declaration = self
                    .declarations
                    .iter()
                    .find(|declaration| declaration.symbol_index == index);
                let (effects, requirements) = type_effects(symbol.ty());
                let inferred_type = render_fact_type(symbol.ty(), &mut fact_budget)?;
                let declared_type = declaration
                    .and_then(|declaration| declaration.declared_type.clone())
                    .map(|declared| consume_fact_type(declared, &mut fact_budget))
                    .transpose()?;
                symbols.push(SymbolFact {
                    id: symbol.id().as_u32(),
                    name: consume_fact_string(symbol.name().to_owned(), &mut fact_budget)?,
                    kind: compiler_symbol_kind(symbol.kind()),
                    inferred_type,
                    declared_type,
                    top_level: symbol.is_top_level(),
                    referenced: referenced.contains(&symbol.id().as_u32()),
                    span: symbol.span().into(),
                    range: self.lines.range(self.source.text(), symbol.span()),
                    selection_range: declaration
                        .and_then(|declaration| declaration.name_span)
                        .map(|span| self.lines.range(self.source.text(), span)),
                    visibility_range: declaration.map(|declaration| {
                        self.lines.range(self.source.text(), declaration.visibility)
                    }),
                    effects: consume_fact_strings(effects, &mut fact_budget)?,
                    capability_requirements: consume_requirement_facts(
                        requirements,
                        self.package.as_ref(),
                        &mut fact_budget,
                    )?,
                });
            }

            expressions.reserve(analysis.expressions().len());
            for expression in analysis.expressions() {
                expressions.push(ExpressionFact {
                    syntax_kind: self
                        .syntax_kinds
                        .get(&expression.span())
                        .copied()
                        .unwrap_or("expression"),
                    inferred_type: render_fact_type(expression.ty(), &mut fact_budget)?,
                    span: expression.span().into(),
                    range: self.lines.range(self.source.text(), expression.span()),
                    effects: consume_fact_strings(
                        expression
                            .effects()
                            .iter()
                            .map(|effect| effect.as_str().to_owned())
                            .collect(),
                        &mut fact_budget,
                    )?,
                    capability_requirements: consume_requirement_facts(
                        expression.requirements(),
                        self.package.as_ref(),
                        &mut fact_budget,
                    )?,
                    resolved: consume_resolved_fact(
                        resolved_fact(expression.resolved_name(), analysis),
                        &mut fact_budget,
                    )?,
                });
            }
        }
        let module = analysis.map(|analysis| ModuleFact {
            effects: all_entrypoint_effects(analysis),
            capability_requirements: all_entrypoint_permissions(analysis, self.package.as_ref()),
            entrypoints: entrypoint_facts(analysis, self.package.as_ref()),
        });
        let formatting = match self.formatting_edit() {
            Ok(edit) => FormattingFact {
                available: true,
                canonical: edit.is_none(),
                edits: edit.into_iter().collect(),
                error: None,
            },
            Err(error) => FormattingFact {
                available: false,
                canonical: false,
                edits: Vec::new(),
                error: Some(error),
            },
        };

        Ok(CompilerFacts {
            schema: 1,
            authoring_protocol: 1,
            language_version: "0.2.0",
            edition: "2026",
            document_version: self.version,
            valid: self.analysis.is_some() && !self.oversized,
            diagnostics,
            module,
            durable_state: DurableStateFact {
                schema: 1,
                operations: self
                    .durable_operations
                    .iter()
                    .map(|operation| DurableOperationFact {
                        kind: operation.kind().as_str(),
                        store: operation.store().map(str::to_owned),
                        identity: operation.identity().map(str::to_owned),
                        external_capability: operation.external_capability(),
                        external_resource: operation.external_resource().map(str::to_owned),
                        range: self.lines.range(self.source.text(), operation.span()),
                        span: operation.span().into(),
                    })
                    .collect(),
            },
            package: analysis
                .and_then(|analysis| self.package.as_ref().map(|package| package.facts(analysis))),
            symbols,
            expressions,
            formatting,
        })
    }

    fn render_symbol_hover(
        &self,
        symbol: &krit::SymbolAnalysis,
        declaration: &DeclarationFact,
    ) -> Result<String, String> {
        let inferred = render_type(symbol.ty())?;
        let mut output = format!(
            "```krit\n{}: {}\n```\n\n**Kind:** `{}`",
            symbol.name(),
            inferred,
            compiler_symbol_kind(symbol.kind())
        );
        if let Some(declared) = &declaration.declared_type {
            if declared.len() > MAX_RENDERED_TYPE_BYTES {
                return Err("declared type exceeds the language-server display limit".to_owned());
            }
            output.push_str(&format!("\n\n**Declared:** `{declared}`"));
        }
        let (effects, requirements) = type_effects(symbol.ty());
        append_effects_and_requirements(&mut output, effects, requirements, self.package.as_ref());
        if let Some((kind, _)) = deployment_entrypoint_kind(symbol.kind()) {
            output.push_str(&format!("\n\n**Entrypoint:** `{}`", kind.as_str()));
        }
        if symbol.is_top_level()
            && let Some(package) = &self.package
        {
            output.push_str(&format!(
                "\n\n**Package:** `{}` {} (edition {})",
                package.manifest.package.name,
                package.manifest.package.version,
                package.manifest.package.edition
            ));
        }
        Ok(output)
    }

    fn render_expression_hover(
        &self,
        expression: &krit::ExpressionAnalysis,
        effect_context: Option<&krit::ExpressionAnalysis>,
    ) -> Result<String, String> {
        let inferred = render_type(expression.ty())?;
        let mut output = match expression.resolved_name() {
            Some(ResolvedName::Builtin(builtin)) => format!(
                "```krit\n{}: {}\n```\n\n**Built-in category:** `{}`\n\n{}",
                builtin.as_str(),
                builtin.signature(),
                builtin.category().as_str(),
                builtin.documentation()
            ),
            Some(ResolvedName::Symbol(id)) => {
                if let Some(analysis) = &self.analysis
                    && let Some(symbol) = analysis.symbols().iter().find(|symbol| symbol.id() == id)
                {
                    format!(
                        "```krit\n{}: {}\n```\n\n**Kind:** `{}`",
                        symbol.name(),
                        inferred,
                        compiler_symbol_kind(symbol.kind())
                    )
                } else {
                    format!("```krit\n{inferred}\n```")
                }
            }
            None => format!("```krit\n{inferred}\n```"),
        };
        let context = effect_context.unwrap_or(expression);
        append_effects_and_requirements(
            &mut output,
            context
                .effects()
                .iter()
                .map(|effect| effect.as_str().to_owned())
                .collect(),
            context.requirements(),
            self.package.as_ref(),
        );
        Ok(output)
    }
}

fn append_effects_and_requirements(
    output: &mut String,
    effects: Vec<String>,
    requirements: &RequirementSet,
    package: Option<&PackageContext>,
) {
    output.push_str("\n\n**Effects:** `");
    output.push('{');
    output.push_str(&effects.join(", "));
    output.push('}');
    output.push('`');
    if requirements.is_empty() {
        output.push_str("\n\n**Capability requirements:** `(none)`");
        return;
    }
    output.push_str("\n\n**Capability requirements:**");
    for requirement in requirements.iter() {
        let status = package.map(|package| {
            if package.manifest.grants_permission(
                requirement.capability().as_str(),
                Some(requirement.resource()),
            ) {
                "granted"
            } else {
                "missing"
            }
        });
        output.push_str(&format!(
            "\n\n- `{}` resource `{}`{}",
            requirement.capability().as_str(),
            requirement.resource(),
            status.map_or(String::new(), |status| format!(": **{status}**"))
        ));
    }
}

fn render_type(ty: &Type) -> Result<String, String> {
    ty.render_bounded(MAX_RENDERED_TYPE_BYTES).ok_or_else(|| {
        format!(
            "inferred type exceeds the {} byte language-server display limit",
            MAX_RENDERED_TYPE_BYTES
        )
    })
}

fn render_fact_type(ty: &Type, budget: &mut usize) -> Result<String, String> {
    consume_fact_type(render_type(ty)?, budget)
}

fn consume_fact_type(rendered: String, budget: &mut usize) -> Result<String, String> {
    if rendered.len() > MAX_RENDERED_TYPE_BYTES || rendered.len() > *budget {
        return Err("compiler type facts exceed the bounded output limit".to_owned());
    }
    *budget -= rendered.len();
    Ok(rendered)
}

fn consume_fact_string(rendered: String, budget: &mut usize) -> Result<String, String> {
    if rendered.len() > *budget {
        return Err("compiler facts exceed the bounded output limit".to_owned());
    }
    *budget -= rendered.len();
    Ok(rendered)
}

fn consume_fact_strings(rendered: Vec<String>, budget: &mut usize) -> Result<Vec<String>, String> {
    for value in &rendered {
        if value.len() > *budget {
            return Err("compiler facts exceed the bounded output limit".to_owned());
        }
        *budget -= value.len();
    }
    Ok(rendered)
}

fn consume_requirement_facts(
    requirements: &RequirementSet,
    package: Option<&PackageContext>,
    budget: &mut usize,
) -> Result<Vec<RequiredPermissionFact>, String> {
    let facts = requirement_facts(requirements, package);
    for fact in &facts {
        if fact.capability.len() > *budget {
            return Err("compiler facts exceed the bounded output limit".to_owned());
        }
        *budget -= fact.capability.len();
        if let Some(resource) = &fact.resource {
            if resource.len() > *budget {
                return Err("compiler facts exceed the bounded output limit".to_owned());
            }
            *budget -= resource.len();
        }
    }
    Ok(facts)
}

fn consume_resolved_fact(
    fact: Option<ResolvedFact>,
    budget: &mut usize,
) -> Result<Option<ResolvedFact>, String> {
    if let Some(fact) = &fact {
        if fact.name.len() > *budget {
            return Err("compiler facts exceed the bounded output limit".to_owned());
        }
        *budget -= fact.name.len();
    }
    Ok(fact)
}

fn type_effects(ty: &Type) -> (Vec<String>, &RequirementSet) {
    match ty {
        Type::Function(function) => (
            function
                .effects()
                .iter()
                .map(|effect| effect.as_str().to_owned())
                .collect(),
            function.requirements(),
        ),
        _ => (Vec::new(), empty_requirements()),
    }
}

fn empty_requirements() -> &'static RequirementSet {
    static EMPTY: std::sync::OnceLock<RequirementSet> = std::sync::OnceLock::new();
    EMPTY.get_or_init(RequirementSet::default)
}

#[derive(Clone, Copy)]
struct ResourceCompletionContext {
    builtin: Builtin,
    content_start: usize,
    cursor: usize,
}

fn resource_completion_context(text: &str, cursor: usize) -> Option<ResourceCompletionContext> {
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let mut in_string = false;
    let mut escaped = false;
    let mut opening = None;
    for (relative, character) in text[line_start..cursor].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && in_string {
            escaped = true;
        } else if character == '"' {
            in_string = !in_string;
            if in_string {
                opening = Some(line_start + relative);
            } else {
                opening = None;
            }
        }
    }
    if !in_string {
        return None;
    }
    let opening = opening?;
    let mut before = opening;
    while before > 0 && text.as_bytes()[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    if before == 0 || text.as_bytes()[before - 1] != b'(' {
        return None;
    }
    before -= 1;
    while before > 0 && text.as_bytes()[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    let name_end = before;
    while before > 0 && is_identifier_continue(text.as_bytes()[before - 1]) {
        before -= 1;
    }
    let builtin = Builtin::from_name(&text[before..name_end])?;
    builtin_capability(builtin)?;
    Some(ResourceCompletionContext {
        builtin,
        content_start: opening + 1,
        cursor,
    })
}

fn builtin_capability(builtin: Builtin) -> Option<&'static str> {
    match builtin {
        Builtin::AiInvoke => Some("ai.invoke"),
        Builtin::ConfigString => Some("config.read"),
        Builtin::Secret => Some("secret.read"),
        Builtin::HttpRequest => Some("http.request"),
        Builtin::StateGet
        | Builtin::StatePut
        | Builtin::StateDelete
        | Builtin::CheckpointGet
        | Builtin::CheckpointPut
        | Builtin::ReplayHttp
        | Builtin::ReplayAi => Some("state.transaction"),
        Builtin::QueuePublish => Some("queue.publish"),
        Builtin::ObjectGet => Some("object.read"),
        Builtin::ObjectPut | Builtin::ObjectDelete => Some("object.write"),
        Builtin::DatabaseBeginRead => Some("database.read"),
        Builtin::DatabaseBeginWrite => Some("database.write"),
        Builtin::CacheGet => Some("cache.read"),
        Builtin::CachePut | Builtin::CacheDelete => Some("cache.write"),
        Builtin::SearchQuery => Some("search.query"),
        Builtin::VectorSearch => Some("search.vector"),
        _ => None,
    }
}

#[derive(Clone)]
struct FieldCompletionContext {
    receiver: String,
    receiver_start: usize,
    receiver_end: usize,
    dot: usize,
    replacement_start: usize,
    cursor: usize,
}

fn field_completion_context(text: &str, cursor: usize) -> Option<FieldCompletionContext> {
    let mut replacement_start = cursor;
    while replacement_start > 0 && is_identifier_continue(text.as_bytes()[replacement_start - 1]) {
        replacement_start -= 1;
    }
    let mut dot = replacement_start;
    while dot > 0 && text.as_bytes()[dot - 1].is_ascii_whitespace() {
        dot -= 1;
    }
    if dot == 0 || text.as_bytes()[dot - 1] != b'.' {
        return None;
    }
    dot -= 1;
    let mut receiver_end = dot;
    while receiver_end > 0 && text.as_bytes()[receiver_end - 1].is_ascii_whitespace() {
        receiver_end -= 1;
    }
    let mut receiver_start = receiver_end;
    while receiver_start > 0 && is_identifier_continue(text.as_bytes()[receiver_start - 1]) {
        receiver_start -= 1;
    }
    if receiver_start == receiver_end || !is_identifier_start(text.as_bytes()[receiver_start]) {
        return None;
    }
    Some(FieldCompletionContext {
        receiver: text[receiver_start..receiver_end].to_owned(),
        receiver_start,
        receiver_end,
        dot,
        replacement_start,
        cursor,
    })
}

fn receiver_type_from_source(text: &str, context: &FieldCompletionContext) -> Option<Type> {
    let source = Source::new("<lsp-field-completion>", text.to_owned());
    let program = krit::parse_source(&source).ok()?;
    let analysis = krit::analyze(&program).ok()?;
    let receiver = analysis
        .expressions()
        .iter()
        .filter(|expression| {
            expression.span().start == context.receiver_start
                && expression.span().end == context.receiver_end
        })
        .find_map(|expression| expression.resolved_name())?;
    let ResolvedName::Symbol(id) = receiver else {
        return None;
    };
    analysis
        .symbols()
        .iter()
        .find(|symbol| symbol.id() == id)
        .map(|symbol| symbol.ty().clone())
}

fn field_repair_attempts(document_bytes: usize) -> usize {
    (MAX_FIELD_REPAIR_WORK_BYTES / document_bytes.max(1)).clamp(1, MAX_FIELD_REPAIR_ATTEMPTS)
}

fn field_candidates(program: &Program) -> BTreeSet<String> {
    fn annotation_fields(annotation: &TypeAnnotation, fields: &mut BTreeSet<String>) {
        match &annotation.kind {
            krit::TypeKind::List(element) | krit::TypeKind::Option(element) => {
                annotation_fields(element, fields);
            }
            krit::TypeKind::Result(value, error) => {
                annotation_fields(value, fields);
                annotation_fields(error, fields);
            }
            krit::TypeKind::Record(record_fields) => {
                for field in record_fields {
                    fields.insert(field.name.clone());
                    annotation_fields(&field.annotation, fields);
                }
            }
            krit::TypeKind::Int
            | krit::TypeKind::Bool
            | krit::TypeKind::String
            | krit::TypeKind::Unit
            | krit::TypeKind::HttpHeader
            | krit::TypeKind::HttpRequest
            | krit::TypeKind::HttpResponse
            | krit::TypeKind::LogField
            | krit::TypeKind::QueueJob
            | krit::TypeKind::ScheduleEvent
            | krit::TypeKind::DatabaseTransaction
            | krit::TypeKind::Secret => {}
        }
    }

    fn block_fields(block: &Block, fields: &mut BTreeSet<String>) {
        for statement in &block.statements {
            statement_fields(statement, fields);
        }
        if let Some(tail) = block.tail.as_deref() {
            expression_fields(tail, fields);
        }
    }

    fn statement_fields(statement: &Statement, fields: &mut BTreeSet<String>) {
        match &statement.kind {
            StatementKind::Let {
                annotation, value, ..
            } => {
                if let Some(annotation) = annotation {
                    annotation_fields(annotation, fields);
                }
                expression_fields(value, fields);
            }
            StatementKind::Function {
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::Webhook {
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::QueueConsumer {
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::ScheduleHandler {
                parameters,
                return_type,
                body,
                ..
            } => {
                for parameter in parameters {
                    if let Some(annotation) = &parameter.annotation {
                        annotation_fields(annotation, fields);
                    }
                }
                if let Some(return_type) = return_type {
                    annotation_fields(return_type, fields);
                }
                block_fields(body, fields);
            }
            StatementKind::Expression(expression) => expression_fields(expression, fields),
        }
    }

    fn expression_fields(expression: &Expression, fields: &mut BTreeSet<String>) {
        // `else if` continuations and the left spine of an operator chain are
        // followed by this loop rather than by recursion, so a long chain
        // never deepens the walk.
        let mut pending_operands = Vec::new();
        let mut current = expression;
        loop {
            match &current.kind {
                ExpressionKind::If {
                    condition,
                    consequent,
                    alternative,
                } => {
                    expression_fields(condition, fields);
                    block_fields(consequent, fields);
                    current = alternative;
                    continue;
                }
                ExpressionKind::Binary { left, right, .. } => {
                    pending_operands.push(right.as_ref());
                    current = left;
                    continue;
                }
                ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
                ExpressionKind::List(elements) => {
                    for element in elements {
                        expression_fields(element, fields);
                    }
                }
                ExpressionKind::Record(record_fields) => {
                    for field in record_fields {
                        fields.insert(field.name.clone());
                        expression_fields(&field.value, fields);
                    }
                }
                ExpressionKind::Construct {
                    fields: written, ..
                } => {
                    for field in written {
                        fields.insert(field.name.clone());
                        expression_fields(&field.value, fields);
                    }
                }
                ExpressionKind::FieldAccess { value, field } => {
                    fields.insert(field.clone());
                    expression_fields(value, fields);
                }
                ExpressionKind::Block(block) => block_fields(block, fields),
                ExpressionKind::Function {
                    parameters,
                    return_type,
                    body,
                } => {
                    for parameter in parameters {
                        if let Some(annotation) = &parameter.annotation {
                            annotation_fields(annotation, fields);
                        }
                    }
                    if let Some(return_type) = return_type {
                        annotation_fields(return_type, fields);
                    }
                    block_fields(body, fields);
                }
                ExpressionKind::Call { callee, arguments } => {
                    expression_fields(callee, fields);
                    for argument in arguments {
                        expression_fields(argument, fields);
                    }
                }
                ExpressionKind::Match { subject, kind } => {
                    expression_fields(subject, fields);
                    match kind {
                        MatchKind::List {
                            empty_case,
                            cons_case,
                            ..
                        } => {
                            expression_fields(empty_case, fields);
                            expression_fields(cons_case, fields);
                        }
                        MatchKind::Variants { arms, .. } => {
                            for arm in arms {
                                expression_fields(&arm.value, fields);
                            }
                        }
                    }
                }
                ExpressionKind::Unary { operand, .. } => expression_fields(operand, fields),
            }
            let Some(operand) = pending_operands.pop() else {
                break;
            };
            current = operand;
        }
    }

    let mut fields = BTreeSet::new();
    for statement in &program.statements {
        statement_fields(statement, &mut fields);
    }
    fields
}

fn type_completion_context(text: &str, cursor: usize) -> bool {
    let mut start = cursor;
    while start > 0 && is_identifier_continue(text.as_bytes()[start - 1]) {
        start -= 1;
    }
    while start > 0 && text.as_bytes()[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    start > 0
        && (text.as_bytes()[start - 1] == b':'
            || (start >= 2 && &text.as_bytes()[start - 2..start] == b"->"))
}

fn type_completions() -> Vec<CompletionItem> {
    [
        "Bool",
        "HttpHeader",
        "HttpRequest",
        "HttpResponse",
        "Int",
        "List",
        "LogField",
        "Option",
        "QueueJob",
        "Record",
        "Result",
        "ScheduleEvent",
        "Secret",
        "String",
        "Unit",
    ]
    .into_iter()
    .map(|name| CompletionItem {
        label: name.to_owned(),
        kind: Some(CompletionItemKind::TYPE_PARAMETER),
        detail: Some("Krit built-in type".to_owned()),
        ..CompletionItem::default()
    })
    .collect()
}

fn builtin_completions() -> Vec<CompletionItem> {
    Builtin::ALL
        .into_iter()
        .map(|builtin| CompletionItem {
            label: builtin.as_str().to_owned(),
            kind: Some(match builtin.category() {
                krit::BuiltinCategory::Constructor => CompletionItemKind::CONSTRUCTOR,
                krit::BuiltinCategory::Conversion | krit::BuiltinCategory::HostEffect => {
                    CompletionItemKind::FUNCTION
                }
            }),
            detail: Some(builtin.signature().to_owned()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: builtin.documentation().to_owned(),
            })),
            ..CompletionItem::default()
        })
        .collect()
}

fn keyword_completions() -> Vec<CompletionItem> {
    [
        "else", "false", "fn", "if", "let", "match", "queue", "record", "schedule", "true",
        "webhook",
    ]
    .into_iter()
    .map(|keyword| CompletionItem {
        label: keyword.to_owned(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some("Krit keyword".to_owned()),
        ..CompletionItem::default()
    })
    .collect()
}

/// Sorted, deduplicated union of a writable and a read-only grant list.
fn union_resources(writable: &[String], read_only: &[String]) -> Vec<String> {
    let mut resources = writable
        .iter()
        .chain(read_only)
        .cloned()
        .collect::<Vec<_>>();
    resources.sort();
    resources.dedup();
    resources
}

fn sort_completions(items: &mut [CompletionItem]) {
    items.sort_by(|left, right| left.label.cmp(&right.label));
    for (index, item) in items.iter_mut().enumerate() {
        item.sort_text = Some(format!("{index:04}:{}", item.label));
    }
}

fn symbol_completion_kind(kind: CompilerSymbolKind) -> CompletionItemKind {
    match kind {
        CompilerSymbolKind::Let | CompilerSymbolKind::Parameter | CompilerSymbolKind::Match => {
            CompletionItemKind::VARIABLE
        }
        CompilerSymbolKind::Function
        | CompilerSymbolKind::Webhook
        | CompilerSymbolKind::QueueConsumer
        | CompilerSymbolKind::ScheduleHandler => CompletionItemKind::FUNCTION,
    }
}

fn document_symbol_kind(kind: CompilerSymbolKind) -> SymbolKind {
    match kind {
        CompilerSymbolKind::Let | CompilerSymbolKind::Parameter | CompilerSymbolKind::Match => {
            SymbolKind::VARIABLE
        }
        CompilerSymbolKind::Function
        | CompilerSymbolKind::Webhook
        | CompilerSymbolKind::QueueConsumer
        | CompilerSymbolKind::ScheduleHandler => SymbolKind::FUNCTION,
    }
}

fn symbol_documentation(kind: CompilerSymbolKind, ty: &str) -> String {
    format!("{} with inferred type `{ty}`.", compiler_symbol_kind(kind))
}

fn symbol_detail(kind: CompilerSymbolKind, ty: &str) -> String {
    format!("{}: {ty}", compiler_symbol_kind(kind))
}

fn compiler_symbol_kind(kind: CompilerSymbolKind) -> &'static str {
    match kind {
        CompilerSymbolKind::Let => "let",
        CompilerSymbolKind::Function => "function",
        CompilerSymbolKind::Webhook => "webhook",
        CompilerSymbolKind::QueueConsumer => "queue consumer",
        CompilerSymbolKind::ScheduleHandler => "schedule handler",
        CompilerSymbolKind::Parameter => "parameter",
        CompilerSymbolKind::Match => "match binding",
    }
}

fn visibility_priority(span: Span) -> (usize, std::cmp::Reverse<usize>) {
    (span.start, std::cmp::Reverse(span.end - span.start))
}

fn span_contains(span: Span, offset: usize) -> bool {
    if span.start == span.end {
        offset == span.start
    } else {
        span.start <= offset && offset < span.end
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn document_name(uri: &Uri) -> String {
    uri_to_file_path(uri)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "<lsp-document>".to_owned())
}

pub(crate) fn uri_to_file_path(uri: &Uri) -> Option<PathBuf> {
    Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

struct PackageContext {
    manifest: Manifest,
}

impl PackageContext {
    fn load_exact(uri: &Uri, manifest_path: &Path, manifest: &Manifest) -> Result<Self, String> {
        let file = uri_to_file_path(uri)
            .ok_or_else(|| "document is not a file URI".to_owned())?
            .canonicalize()
            .map_err(|_| "document path is not accessible".to_owned())?;
        let entry = manifest
            .resolve_entry(manifest_path)
            .map_err(|_| "explicit package entry is invalid".to_owned())?;
        if file != entry {
            return Err("document is not the explicit package entry".to_owned());
        }
        Ok(Self {
            manifest: manifest.clone(),
        })
    }

    fn discover(uri: &Uri, workspace_roots: &[PathBuf]) -> Option<Self> {
        let file = uri_to_file_path(uri)?;
        let boundary = workspace_roots
            .iter()
            .filter(|root| file.starts_with(root))
            .max_by_key(|root| root.components().count());
        let mut directory = file.parent()?;
        for _ in 0..32 {
            let candidate = directory.join("krit.pkg");
            if candidate.is_file() {
                let contents = read_manifest(&candidate)?;
                let manifest = match Manifest::parse(&contents) {
                    Ok(manifest) => manifest,
                    Err(_) => {
                        eprintln!(
                            "krit-lsp: nearest package manifest is invalid; run `krit package check`"
                        );
                        return None;
                    }
                };
                let entry = match manifest.resolve_entry(&candidate) {
                    Ok(entry) => entry,
                    Err(_) => {
                        eprintln!(
                            "krit-lsp: nearest package entry is invalid; run `krit package check`"
                        );
                        return None;
                    }
                };
                let file = match file.canonicalize() {
                    Ok(file) => file,
                    Err(_) => return None,
                };
                return (file == entry).then_some(Self { manifest });
            }
            if boundary.is_some_and(|boundary| directory == boundary) {
                break;
            }
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }
        None
    }

    fn facts(&self, analysis: &Analysis) -> PackageFact {
        let required = all_entrypoint_permissions(analysis, Some(self));
        let required_set = required
            .iter()
            .map(|permission| {
                (
                    permission.capability.as_str(),
                    permission.resource.as_deref(),
                )
            })
            .collect::<BTreeSet<_>>();
        let requested_permissions = self
            .manifest
            .permission_plan()
            .requested
            .into_iter()
            .map(|permission| {
                let used =
                    required_set.contains(&(permission.capability, permission.resource.as_deref()));
                RequestedPermissionFact {
                    capability: permission.capability,
                    resource: permission.resource,
                    used,
                }
            })
            .collect();
        PackageFact {
            schema: 1,
            name: self.manifest.package.name.clone(),
            version: self.manifest.package.version.clone(),
            edition: self.manifest.package.edition.clone(),
            entry: self
                .manifest
                .package
                .entry
                .to_string_lossy()
                .replace('\\', "/"),
            target: self.manifest.package.target.clone(),
            // Validation rejects any declared dependency with `K6003`, so this
            // schema-1 field stays empty until dependency resolution exists.
            dependencies: self
                .manifest
                .dependencies
                .iter()
                .map(|(name, requirement)| PackageDependencyFact {
                    name: name.clone(),
                    requirement: requirement.to_string(),
                })
                .collect(),
            requested_permissions,
            all_required_granted: required
                .iter()
                .all(|permission| permission.granted == Some(true)),
            required_permissions: required,
        }
    }
}

fn read_manifest(path: &Path) -> Option<String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => {
            eprintln!(
                "krit-lsp: nearest package manifest could not be read; run `krit package check`"
            );
            return None;
        }
    };
    let mut contents = String::new();
    if file
        .take((MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_string(&mut contents)
        .is_err()
    {
        eprintln!(
            "krit-lsp: nearest package manifest is not bounded UTF-8; run `krit package check`"
        );
        return None;
    }
    if contents.len() > MAX_MANIFEST_BYTES {
        eprintln!("krit-lsp: nearest package manifest exceeds the bounded input limit");
        return None;
    }
    Some(contents)
}

#[derive(Clone)]
struct DeclarationFact {
    symbol_index: usize,
    name_span: Option<Span>,
    visibility: Span,
    declared_type: Option<String>,
}

struct DeclarationCollector<'a> {
    text: &'a str,
    symbols: BTreeMap<(Span, String, CompilerSymbolKind), usize>,
    declarations: Vec<DeclarationFact>,
    syntax_kinds: BTreeMap<Span, &'static str>,
}

impl<'a> DeclarationCollector<'a> {
    fn collect(
        text: &'a str,
        program: &Program,
        analysis: &Analysis,
        scope_end: usize,
    ) -> (Vec<DeclarationFact>, BTreeMap<Span, &'static str>) {
        let symbols = analysis
            .symbols()
            .iter()
            .enumerate()
            .map(|(index, symbol)| {
                (
                    (symbol.span(), symbol.name().to_owned(), symbol.kind()),
                    index,
                )
            })
            .collect();
        let mut collector = Self {
            text,
            symbols,
            declarations: Vec::new(),
            syntax_kinds: BTreeMap::new(),
        };
        for statement in &program.statements {
            collector.statement(statement, scope_end);
        }
        (collector.declarations, collector.syntax_kinds)
    }

    fn statement(&mut self, statement: &Statement, scope_end: usize) {
        match &statement.kind {
            StatementKind::Let {
                name,
                annotation,
                value,
            } => {
                self.add(
                    name,
                    CompilerSymbolKind::Let,
                    statement.span,
                    find_identifier_span(self.text, statement.span, name),
                    Span::new(statement.span.end, scope_end),
                    annotation.as_ref().map(ToString::to_string),
                );
                self.expression(value);
            }
            StatementKind::Function {
                name,
                parameters,
                return_type,
                body,
            }
            | StatementKind::Webhook {
                name,
                parameters,
                return_type,
                body,
            }
            | StatementKind::QueueConsumer {
                name,
                parameters,
                return_type,
                body,
                ..
            }
            | StatementKind::ScheduleHandler {
                name,
                parameters,
                return_type,
                body,
                ..
            } => {
                let kind = match statement.kind {
                    StatementKind::Webhook { .. } => CompilerSymbolKind::Webhook,
                    StatementKind::QueueConsumer { .. } => CompilerSymbolKind::QueueConsumer,
                    StatementKind::ScheduleHandler { .. } => CompilerSymbolKind::ScheduleHandler,
                    _ => CompilerSymbolKind::Function,
                };
                let name_search_span = match &statement.kind {
                    StatementKind::QueueConsumer { queue_span, .. } => {
                        Span::new(queue_span.end, statement.span.end)
                    }
                    StatementKind::ScheduleHandler { schedule_span, .. } => {
                        Span::new(schedule_span.end, statement.span.end)
                    }
                    _ => statement.span,
                };
                self.add(
                    name,
                    kind,
                    statement.span,
                    find_identifier_span(self.text, name_search_span, name),
                    Span::new(statement.span.start, scope_end),
                    Some(declared_function_type(
                        kind,
                        parameters
                            .iter()
                            .map(|parameter| parameter.annotation.as_ref()),
                        return_type.as_ref(),
                    )),
                );
                for parameter in parameters {
                    self.add(
                        &parameter.name,
                        CompilerSymbolKind::Parameter,
                        parameter.span,
                        Some(parameter.span),
                        body.span,
                        parameter.annotation.as_ref().map(ToString::to_string),
                    );
                }
                self.block(body);
            }
            StatementKind::Expression(expression) => self.expression(expression),
        }
    }

    fn block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.statement(statement, block.span.end);
        }
        if let Some(tail) = block.tail.as_deref() {
            self.expression(tail);
        }
    }

    fn expression(&mut self, expression: &Expression) {
        // `else if` continuations and the left spine of an operator chain are
        // followed by this loop rather than by recursion, so a long chain
        // never deepens the walk; expressions are still visited in source
        // order.
        let mut pending_operands = Vec::new();
        let mut current = expression;
        loop {
            self.syntax_kinds
                .insert(current.span, expression_kind(&current.kind));
            match &current.kind {
                ExpressionKind::If {
                    condition,
                    consequent,
                    alternative,
                } => {
                    self.expression(condition);
                    self.block(consequent);
                    current = alternative;
                    continue;
                }
                ExpressionKind::Binary { left, right, .. } => {
                    pending_operands.push(right.as_ref());
                    current = left;
                    continue;
                }
                ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => {}
                ExpressionKind::List(elements) => {
                    for element in elements {
                        self.expression(element);
                    }
                }
                ExpressionKind::Record(fields) => {
                    for field in fields {
                        self.expression(&field.value);
                    }
                }
                ExpressionKind::Construct { fields, .. } => {
                    for field in fields {
                        self.expression(&field.value);
                    }
                }
                ExpressionKind::FieldAccess { value, .. } => self.expression(value),
                ExpressionKind::Block(block) => self.block(block),
                ExpressionKind::Function {
                    parameters, body, ..
                } => {
                    for parameter in parameters {
                        self.add(
                            &parameter.name,
                            CompilerSymbolKind::Parameter,
                            parameter.span,
                            Some(parameter.span),
                            body.span,
                            parameter.annotation.as_ref().map(ToString::to_string),
                        );
                    }
                    self.block(body);
                }
                ExpressionKind::Call { callee, arguments } => {
                    self.expression(callee);
                    for argument in arguments {
                        self.expression(argument);
                    }
                }
                ExpressionKind::Match { subject, kind } => {
                    self.expression(subject);
                    match kind {
                        MatchKind::List {
                            empty_case,
                            head_name,
                            head_span,
                            tail_name,
                            tail_span,
                            cons_case,
                        } => {
                            self.expression(empty_case);
                            self.add(
                                head_name,
                                CompilerSymbolKind::Match,
                                *head_span,
                                Some(*head_span),
                                cons_case.span,
                                None,
                            );
                            self.add(
                                tail_name,
                                CompilerSymbolKind::Match,
                                *tail_span,
                                Some(*tail_span),
                                cons_case.span,
                                None,
                            );
                            self.expression(cons_case);
                        }
                        MatchKind::Variants { arms, .. } => {
                            for arm in arms {
                                if let Some(binding) = &arm.binding {
                                    self.add(
                                        binding,
                                        CompilerSymbolKind::Match,
                                        arm.span,
                                        find_identifier_span(self.text, arm.span, binding),
                                        arm.value.span,
                                        None,
                                    );
                                }
                                self.expression(&arm.value);
                            }
                        }
                    }
                }
                ExpressionKind::Unary { operand, .. } => self.expression(operand),
            }
            let Some(operand) = pending_operands.pop() else {
                break;
            };
            current = operand;
        }
    }

    fn add(
        &mut self,
        name: &str,
        kind: CompilerSymbolKind,
        symbol_span: Span,
        name_span: Option<Span>,
        visibility: Span,
        declared_type: Option<String>,
    ) {
        let Some(symbol_index) = self
            .symbols
            .get(&(symbol_span, name.to_owned(), kind))
            .copied()
        else {
            return;
        };
        self.declarations.push(DeclarationFact {
            symbol_index,
            name_span,
            visibility,
            declared_type,
        });
    }
}

fn declared_function_type<'a>(
    kind: CompilerSymbolKind,
    parameters: impl IntoIterator<Item = Option<&'a TypeAnnotation>>,
    return_type: Option<&TypeAnnotation>,
) -> String {
    let parameters = parameters
        .into_iter()
        .map(|annotation| annotation.map_or_else(|| "_".to_owned(), ToString::to_string))
        .collect::<Vec<_>>()
        .join(", ");
    let prefix = deployment_entrypoint_kind(kind).map_or_else(
        || "fn".to_owned(),
        |(kind, _)| format!("{} fn", kind.as_str()),
    );
    format!(
        "{prefix}({parameters}) -> {}",
        return_type.map_or_else(|| "_".to_owned(), ToString::to_string)
    )
}

fn find_identifier_span(text: &str, span: Span, name: &str) -> Option<Span> {
    let bytes = text.as_bytes();
    let mut cursor = span.start.min(bytes.len());
    let end = span.end.min(bytes.len());
    while cursor < end {
        if is_identifier_start(bytes[cursor]) {
            let start = cursor;
            cursor += 1;
            while cursor < end && is_identifier_continue(bytes[cursor]) {
                cursor += 1;
            }
            if &text[start..cursor] == name {
                return Some(Span::new(start, cursor));
            }
        } else {
            cursor += 1;
        }
    }
    None
}

fn expression_kind(kind: &ExpressionKind) -> &'static str {
    match kind {
        ExpressionKind::Literal(_) => "literal",
        ExpressionKind::Variable(_) => "name",
        ExpressionKind::List(_) => "list",
        ExpressionKind::Record(_) => "record",
        ExpressionKind::Construct { .. } => "construction",
        ExpressionKind::FieldAccess { .. } => "field-access",
        ExpressionKind::Block(_) => "block",
        ExpressionKind::If { .. } => "if",
        ExpressionKind::Function { .. } => "function",
        ExpressionKind::Call { .. } => "call",
        ExpressionKind::Match { .. } => "match",
        ExpressionKind::Unary { .. } => "unary",
        ExpressionKind::Binary { .. } => "binary",
    }
}

fn referenced_symbols(analysis: &Analysis) -> BTreeSet<u32> {
    analysis
        .expressions()
        .iter()
        .filter_map(|expression| match expression.resolved_name() {
            Some(ResolvedName::Symbol(id)) => Some(id.as_u32()),
            Some(ResolvedName::Builtin(_)) | None => None,
        })
        .collect()
}

fn resolved_fact(resolved: Option<ResolvedName>, analysis: &Analysis) -> Option<ResolvedFact> {
    match resolved? {
        ResolvedName::Symbol(id) => analysis
            .symbols()
            .iter()
            .find(|symbol| symbol.id() == id)
            .map(|symbol| ResolvedFact {
                kind: "symbol",
                name: symbol.name().to_owned(),
                id: Some(id.as_u32()),
                category: Some(compiler_symbol_kind(symbol.kind())),
            }),
        ResolvedName::Builtin(builtin) => Some(ResolvedFact {
            kind: "builtin",
            name: builtin.as_str().to_owned(),
            id: None,
            category: Some(builtin.category().as_str()),
        }),
    }
}

fn requirement_facts(
    requirements: &RequirementSet,
    package: Option<&PackageContext>,
) -> Vec<RequiredPermissionFact> {
    requirements
        .iter()
        .map(|requirement| RequiredPermissionFact {
            capability: requirement.capability().as_str().to_owned(),
            resource: Some(requirement.resource().to_owned()),
            granted: package.map(|package| {
                package.manifest.grants_permission(
                    requirement.capability().as_str(),
                    Some(requirement.resource()),
                )
            }),
        })
        .collect()
}

fn permission_facts<'a>(
    effects: impl IntoIterator<Item = &'a Effect>,
    requirements: &RequirementSet,
    package: Option<&PackageContext>,
) -> Vec<RequiredPermissionFact> {
    let mut permissions = requirement_facts(requirements, package);
    for effect in effects {
        if !matches!(effect, Effect::IoStdout | Effect::ObserveLog) {
            continue;
        }
        permissions.push(RequiredPermissionFact {
            capability: effect.as_str().to_owned(),
            resource: None,
            granted: package
                .map(|package| package.manifest.grants_permission(effect.as_str(), None)),
        });
    }
    permissions.sort_by(|left, right| {
        left.capability
            .cmp(&right.capability)
            .then_with(|| left.resource.cmp(&right.resource))
    });
    permissions.dedup_by(|left, right| {
        left.capability == right.capability && left.resource == right.resource
    });
    permissions
}

fn all_entrypoint_permissions(
    analysis: &Analysis,
    package: Option<&PackageContext>,
) -> Vec<RequiredPermissionFact> {
    let mut permissions =
        permission_facts(analysis.effects().iter(), analysis.requirements(), package);
    for (_, _, _, function) in deployment_entrypoints(analysis) {
        permissions.extend(permission_facts(
            function.effects().iter(),
            function.requirements(),
            package,
        ));
    }
    permissions.sort_by(|left, right| {
        left.capability
            .cmp(&right.capability)
            .then_with(|| left.resource.cmp(&right.resource))
    });
    permissions.dedup_by(|left, right| {
        left.capability == right.capability && left.resource == right.resource
    });
    permissions
}

fn all_entrypoint_effects(analysis: &Analysis) -> Vec<String> {
    let mut effects = analysis
        .effects()
        .iter()
        .map(|effect| effect.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    for (_, _, _, function) in deployment_entrypoints(analysis) {
        effects.extend(
            function
                .effects()
                .iter()
                .map(|effect| effect.as_str().to_owned()),
        );
    }
    effects.into_iter().collect()
}

fn deployment_entrypoint_kind(kind: CompilerSymbolKind) -> Option<(EntrypointKind, &'static str)> {
    match kind {
        CompilerSymbolKind::Webhook => Some((EntrypointKind::Webhook, "request")),
        CompilerSymbolKind::QueueConsumer => Some((EntrypointKind::QueueConsumer, "job")),
        CompilerSymbolKind::ScheduleHandler => Some((EntrypointKind::ScheduleHandler, "event")),
        CompilerSymbolKind::Let
        | CompilerSymbolKind::Function
        | CompilerSymbolKind::Parameter
        | CompilerSymbolKind::Match => None,
    }
}

fn deployment_entrypoints(
    analysis: &Analysis,
) -> impl Iterator<
    Item = (
        &krit::SymbolAnalysis,
        EntrypointKind,
        &'static str,
        &krit::FunctionType,
    ),
> {
    analysis
        .symbols()
        .iter()
        .filter(|symbol| symbol.is_top_level())
        .filter_map(|symbol| {
            let (kind, parameter) = deployment_entrypoint_kind(symbol.kind())?;
            let Type::Function(function) = symbol.ty() else {
                return None;
            };
            Some((symbol, kind, parameter, function))
        })
}

fn entrypoint_facts(analysis: &Analysis, package: Option<&PackageContext>) -> Vec<EntrypointFact> {
    let mut entrypoints = vec![EntrypointFact {
        name: "<module-init>".to_owned(),
        kind: "module-init",
        signature: "fn() -> Unit".to_owned(),
        effects: analysis
            .effects()
            .iter()
            .map(|effect| effect.as_str().to_owned())
            .collect(),
        capability_requirements: permission_facts(
            analysis.effects().iter(),
            analysis.requirements(),
            package,
        ),
    }];
    for (symbol, kind, parameter, function) in deployment_entrypoints(analysis) {
        entrypoints.push(EntrypointFact {
            name: symbol.name().to_owned(),
            kind: kind.as_str(),
            signature: format!(
                "{} fn {}({parameter}: {}) -> {}",
                kind.as_str(),
                symbol.name(),
                function.parameters()[0],
                function.return_type()
            ),
            effects: function
                .effects()
                .iter()
                .map(|effect| effect.as_str().to_owned())
                .collect(),
            capability_requirements: permission_facts(
                function.effects().iter(),
                function.requirements(),
                package,
            ),
        });
    }
    entrypoints
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompilerFacts {
    schema: u32,
    authoring_protocol: u32,
    language_version: &'static str,
    edition: &'static str,
    document_version: i32,
    valid: bool,
    diagnostics: Vec<CompilerDiagnosticFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module: Option<ModuleFact>,
    durable_state: DurableStateFact,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<PackageFact>,
    symbols: Vec<SymbolFact>,
    expressions: Vec<ExpressionFact>,
    formatting: FormattingFact,
}

#[derive(Debug, Serialize)]
struct DurableStateFact {
    schema: u32,
    operations: Vec<DurableOperationFact>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DurableOperationFact {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_capability: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_resource: Option<String>,
    span: ByteSpan,
    range: Range,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompilerDiagnosticFact {
    severity: &'static str,
    code: String,
    message: String,
    /// One-line repair hints, as in the JSON Lines `notes` field.
    notes: Vec<String>,
    span: ByteSpan,
    range: Range,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ByteSpan {
    start: usize,
    end: usize,
}

impl From<Span> for ByteSpan {
    fn from(span: Span) -> Self {
        Self {
            start: span.start,
            end: span.end,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModuleFact {
    effects: Vec<String>,
    capability_requirements: Vec<RequiredPermissionFact>,
    entrypoints: Vec<EntrypointFact>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EntrypointFact {
    name: String,
    kind: &'static str,
    signature: String,
    effects: Vec<String>,
    capability_requirements: Vec<RequiredPermissionFact>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SymbolFact {
    id: u32,
    name: String,
    kind: &'static str,
    inferred_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_type: Option<String>,
    top_level: bool,
    referenced: bool,
    span: ByteSpan,
    range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_range: Option<Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility_range: Option<Range>,
    effects: Vec<String>,
    capability_requirements: Vec<RequiredPermissionFact>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExpressionFact {
    syntax_kind: &'static str,
    inferred_type: String,
    span: ByteSpan,
    range: Range,
    effects: Vec<String>,
    capability_requirements: Vec<RequiredPermissionFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved: Option<ResolvedFact>,
}

#[derive(Debug, Serialize)]
struct ResolvedFact {
    kind: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FormattingFact {
    available: bool,
    canonical: bool,
    edits: Vec<TextEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageFact {
    schema: u32,
    name: String,
    version: String,
    edition: String,
    entry: String,
    target: String,
    dependencies: Vec<PackageDependencyFact>,
    requested_permissions: Vec<RequestedPermissionFact>,
    required_permissions: Vec<RequiredPermissionFact>,
    all_required_granted: bool,
}

#[derive(Debug, Serialize)]
struct PackageDependencyFact {
    name: String,
    requirement: String,
}

#[derive(Debug, Serialize)]
struct RequestedPermissionFact {
    capability: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    used: bool,
}

#[derive(Debug, Serialize)]
struct RequiredPermissionFact {
    capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    granted: Option<bool>,
}

pub(crate) fn request_failed(message: impl Into<String>) -> (i32, String) {
    (ErrorCode::RequestFailed as i32, message.into())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use lsp_types::Position;

    use super::*;

    static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let id = DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("krit-lsp-tests-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self { path }
        }

        fn uri(&self, name: &str) -> Uri {
            let url = Url::from_file_path(self.path.join(name)).expect("path should become a URL");
            url.as_str().parse().expect("URL should become an LSP URI")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// A valid program with a 2,000-branch `else if` chain and a 4,000-term
    /// operator chain, the longest flat chains the parser accepts.
    fn long_chain_program() -> String {
        let mut source = String::from("fn pick(n) {\n    if n == 0 {\n        0\n    }");
        for branch in 1..2_000 {
            source.push_str(&format!(
                " else if n == {branch} {{\n        {branch}\n    }}"
            ));
        }
        source.push_str(" else {\n        -1\n    }\n}\n\nlet total = 0");
        source.push_str(&" + pick(1)".repeat(4_000));
        source.push_str(";\n");
        source
    }

    #[test]
    fn walks_long_else_if_and_operator_chains_iteratively() {
        // Runs on a 1 MiB thread: walking either chain recursively, once per
        // link, would need several times more.
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let directory = TestDirectory::new();
                let uri = directory.uri("main.krit");
                let mut state = ServerState::new(vec![directory.path.clone()]);
                let source = long_chain_program();
                let diagnostics = state.open(uri.clone(), 1, source.clone());
                assert!(diagnostics.diagnostics.is_empty());
                let hover = state
                    .hover(&uri, Position::new(0, 4))
                    .expect("hover should succeed");
                assert!(hover.is_some());

                // An unknown receiver makes completion collect candidate field
                // names from the whole program.
                let source = format!("{source}let probe = unknown.;\n");
                let line = source.lines().count() - 1;
                state
                    .change(&uri, 2, source)
                    .expect("the document should change");
                let CompletionResponse::List(_) = state
                    .completion(&uri, Position::new(line as u32, 20))
                    .expect("completion should succeed")
                else {
                    panic!("completion should return a bounded list")
                };
            })
            .expect("test thread should start")
            .join()
            .expect("long chains should be walked on a small stack");
    }

    #[test]
    fn publishes_compiler_diagnostics_with_utf16_ranges() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        let diagnostics = state.open(
            uri,
            1,
            "let robot = \"🤖\"; let value = missing;\n".to_owned(),
        );

        assert_eq!(diagnostics.diagnostics.len(), 1);
        let diagnostic = &diagnostics.diagnostics[0];
        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("K2001".to_owned()))
        );
        assert_eq!(diagnostic.range.start, Position::new(0, 30));
        assert_eq!(diagnostic.range.end, Position::new(0, 37));
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn publishes_fix_hints_as_help_lines_and_compiler_fact_notes() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        let diagnostics = state.open(
            uri.clone(),
            1,
            "let total = 1;\nprintln(totl);\n".to_owned(),
        );

        assert_eq!(diagnostics.diagnostics.len(), 1);
        assert_eq!(
            diagnostics.diagnostics[0].message,
            "undefined name `totl`\nhelp: did you mean `total`?"
        );
        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["diagnostics"][0]["code"], "K2001");
        assert_eq!(facts["diagnostics"][0]["message"], "undefined name `totl`");
        assert_eq!(
            facts["diagnostics"][0]["notes"],
            serde_json::json!(["did you mean `total`?"])
        );

        let changed = state
            .change(&uri, 2, "fn f(x) {\n    return x;\n}\n".to_owned())
            .expect("change should succeed");
        let diagnostic = &changed.diagnostics[0];
        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("K1101".to_owned()))
        );
        assert!(
            diagnostic
                .message
                .starts_with("`return` is not Krit syntax\nhelp: a block's value")
        );
        assert_eq!(diagnostic.range.start, Position::new(1, 4));
    }

    #[test]
    fn publishes_warnings_beside_valid_analysis_and_before_errors() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);

        let valid = state.open(
            uri.clone(),
            1,
            "let _ = 1;\nlet _ = 2;\nprintln(_);\n".to_owned(),
        );
        assert_eq!(valid.diagnostics.len(), 1);
        let warning = &valid.diagnostics[0];
        assert_eq!(warning.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            warning.code,
            Some(NumberOrString::String("K2003".to_owned()))
        );
        assert_eq!(warning.range.start, Position::new(2, 8));
        assert_eq!(warning.range.end, Position::new(2, 9));
        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["valid"], true);
        assert_eq!(facts["diagnostics"][0]["severity"], "warning");
        assert_eq!(facts["diagnostics"][0]["code"], "K2003");
        assert_eq!(
            facts["diagnostics"]
                .as_array()
                .expect("diagnostics should be an array")
                .len(),
            1
        );

        let invalid = state
            .change(&uri, 2, "let _ = 1;\nlet text = _ + \"a\";\n".to_owned())
            .expect("change should succeed");
        let published = invalid
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.severity, diagnostic.code.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            published,
            vec![
                (
                    Some(DiagnosticSeverity::WARNING),
                    Some(NumberOrString::String("K2003".to_owned()))
                ),
                (
                    Some(DiagnosticSeverity::ERROR),
                    Some(NumberOrString::String("K3001".to_owned()))
                ),
            ]
        );
        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["valid"], false);
        assert_eq!(facts["diagnostics"][0]["severity"], "warning");
        assert_eq!(facts["diagnostics"][1]["severity"], "error");
    }

    /// A valid program that binds `_` on its first line and reads it once on
    /// each of the next `reads` lines.
    fn discard_reads_program(reads: usize) -> String {
        let mut source = String::from("let _ = 1;\n");
        source.push_str(&"_;\n".repeat(reads));
        source
    }

    #[test]
    fn bounds_published_warnings_and_reports_the_rest_in_one_notice() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);

        // About 300 KB of source: every one of its warnings, published, would
        // take a notification past the 16 MiB protocol frame limit.
        let reads = 100_001;
        let source = discard_reads_program(reads);
        assert!(source.len() < MAX_DOCUMENT_BYTES);
        let valid = state.open(uri.clone(), 1, source.clone());
        assert_eq!(valid.diagnostics.len(), MAX_PUBLISHED_WARNINGS + 1);
        for (index, warning) in valid.diagnostics[..MAX_PUBLISHED_WARNINGS]
            .iter()
            .enumerate()
        {
            assert_eq!(warning.severity, Some(DiagnosticSeverity::WARNING));
            assert_eq!(
                warning.code,
                Some(NumberOrString::String("K2003".to_owned()))
            );
            assert_eq!(warning.range.start, Position::new(index as u32 + 1, 0));
        }
        let notice = &valid.diagnostics[MAX_PUBLISHED_WARNINGS];
        assert_eq!(notice.severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(
            notice.code,
            Some(NumberOrString::String("K8003".to_owned()))
        );
        assert_eq!(notice.source.as_deref(), Some("krit-lsp"));
        assert_eq!(
            notice.message,
            "99901 more warnings are not shown; `krit check` reports every warning"
        );
        // The notice sits on the first warning left out.
        assert_eq!(
            notice.range.start,
            Position::new(MAX_PUBLISHED_WARNINGS as u32 + 1, 0)
        );
        let published = serde_json::to_vec(&valid).expect("diagnostics should serialize");
        assert!(published.len() < 64 * 1024);
        assert!(
            state
                .hover(&uri, Position::new(0, 4))
                .expect("hover should succeed")
                .is_some()
        );

        // The rejecting error is always published, after the notice, and
        // compiler facts carry the same bounded list.
        let invalid = state
            .change(&uri, 2, format!("{source}missing;\n"))
            .expect("change should succeed");
        assert_eq!(invalid.diagnostics.len(), MAX_PUBLISHED_WARNINGS + 2);
        assert_eq!(invalid.diagnostics[MAX_PUBLISHED_WARNINGS], *notice);
        let error = &invalid.diagnostics[MAX_PUBLISHED_WARNINGS + 1];
        assert_eq!(error.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(error.code, Some(NumberOrString::String("K2001".to_owned())));
        assert_eq!(error.range.start, Position::new(reads as u32 + 1, 0));
        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["valid"], false);
        let diagnostics = facts["diagnostics"]
            .as_array()
            .expect("diagnostics should be an array");
        assert_eq!(diagnostics.len(), MAX_PUBLISHED_WARNINGS + 2);
        assert!(
            diagnostics[..MAX_PUBLISHED_WARNINGS]
                .iter()
                .all(|diagnostic| diagnostic["severity"] == "warning")
        );
        let notice = &diagnostics[MAX_PUBLISHED_WARNINGS];
        assert_eq!(notice["severity"], "info");
        assert_eq!(notice["code"], "K8003");
        assert_eq!(
            notice["message"],
            "99901 more warnings are not shown; `krit check` reports every warning"
        );
        // `let _ = 1;\n` and 100 lines of `_;\n` precede the first omitted read.
        assert_eq!(
            notice["span"],
            serde_json::json!({"start": 311, "end": 312})
        );
        assert_eq!(diagnostics[MAX_PUBLISHED_WARNINGS + 1]["severity"], "error");
        assert_eq!(diagnostics[MAX_PUBLISHED_WARNINGS + 1]["code"], "K2001");

        // At the bound nothing is left out and no notice is published.
        let bounded = state
            .change(&uri, 3, discard_reads_program(MAX_PUBLISHED_WARNINGS))
            .expect("change should succeed");
        assert_eq!(bounded.diagnostics.len(), MAX_PUBLISHED_WARNINGS);
        assert!(
            bounded
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::WARNING))
        );
    }

    #[test]
    fn bounds_the_published_error_message_and_notes() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);

        // Each record doubles the rendered type of the one before it, so the
        // call error's message grows to hundreds of kilobytes from a few
        // hundred bytes of source.
        let mut source = "let v0 = 1;\n".to_owned();
        for level in 1..=14 {
            let previous = level - 1;
            source.push_str(&format!(
                "let v{level} = record {{ a: v{previous}, b: v{previous} }};\n"
            ));
        }
        source.push_str("v14();\n");
        let published = state.open(uri.clone(), 1, source);
        assert_eq!(published.diagnostics.len(), 1);
        let error = &published.diagnostics[0];
        assert_eq!(error.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(error.code, Some(NumberOrString::String("K3002".to_owned())));
        assert!(error.message.len() <= MAX_PUBLISHED_ERROR_MESSAGE_BYTES);
        assert!(error.message.ends_with(TRUNCATION_MARKER));

        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        let message = facts["diagnostics"][0]["message"]
            .as_str()
            .expect("message should be a string");
        assert_eq!(message, error.message);

        // A note keeps its place and is cut to its own bound; text that fits
        // is unchanged.
        let long_name = "é".repeat(MAX_PUBLISHED_ERROR_NOTE_BYTES);
        let bounded = bounded_error(
            CompilerDiagnostic::new(
                "K2001",
                format!("undefined name `{long_name}x`"),
                Span::new(0, 1),
            )
            .with_note(format!("did you mean `{long_name}`?")),
        );
        assert_eq!(bounded.code(), "K2001");
        assert_eq!(bounded.severity(), Severity::Error);
        assert_eq!(bounded.span(), Span::new(0, 1));
        assert_eq!(bounded.message().len(), long_name.len() + 18);
        assert_eq!(bounded.notes().len(), 1);
        assert!(bounded.notes()[0].len() <= MAX_PUBLISHED_ERROR_NOTE_BYTES);
        assert!(bounded.notes()[0].ends_with(TRUNCATION_MARKER));
        let fitting = CompilerDiagnostic::new("K2001", "undefined name `x`", Span::new(0, 1))
            .with_note("did you mean `y`?");
        assert_eq!(bounded_error(fitting.clone()), fitting);
    }

    #[test]
    fn bounds_published_warnings_by_count_and_message_bytes() {
        let warning = |index: usize, message: String| {
            CompilerDiagnostic::warning("K2003", message, Span::new(index, index + 1))
        };
        let short = |count: usize| {
            (0..count)
                .map(|index| warning(index, "short".to_owned()))
                .collect::<Vec<_>>()
        };

        let (kept, omitted) = bound_warnings(Vec::new());
        assert!(kept.is_empty());
        assert_eq!(omitted, None);

        let (kept, omitted) = bound_warnings(short(MAX_PUBLISHED_WARNINGS));
        assert_eq!(kept.len(), MAX_PUBLISHED_WARNINGS);
        assert_eq!(omitted, None);

        let (kept, omitted) = bound_warnings(short(8 * MAX_PUBLISHED_WARNINGS));
        assert_eq!(kept.len(), MAX_PUBLISHED_WARNINGS);
        assert!(
            kept.capacity() < 2 * MAX_PUBLISHED_WARNINGS,
            "the kept prefix must not retain the full allocation"
        );
        assert_eq!(
            omitted.map(|omitted| omitted.count),
            Some(7 * MAX_PUBLISHED_WARNINGS)
        );

        let (kept, omitted) = bound_warnings(short(MAX_PUBLISHED_WARNINGS + 1));
        assert_eq!(kept.len(), MAX_PUBLISHED_WARNINGS);
        let omitted = omitted.expect("one warning should be left out");
        assert_eq!(
            omitted,
            OmittedWarnings {
                count: 1,
                span: Span::new(MAX_PUBLISHED_WARNINGS, MAX_PUBLISHED_WARNINGS + 1),
            }
        );
        assert_eq!(
            omitted.message(),
            "1 more warning is not shown; `krit check` reports every warning"
        );

        // Messages that exactly fill the byte budget are kept. The first
        // warning past it is left out with every later one, however short, so
        // the published warnings stay a source-order prefix.
        let half = MAX_PUBLISHED_WARNING_MESSAGE_BYTES / 2;
        let (kept, omitted) = bound_warnings(vec![
            warning(0, "a".repeat(half)),
            warning(1, "b".repeat(half)),
            warning(2, "c".to_owned()),
            warning(3, "d".to_owned()),
        ]);
        assert_eq!(
            kept.iter()
                .map(|warning| warning.span().start)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            omitted,
            Some(OmittedWarnings {
                count: 2,
                span: Span::new(2, 3),
            })
        );
    }

    #[test]
    fn returns_type_effect_resource_and_package_facts() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path.join("krit.pkg"),
            r#"
schema = 1

[package]
name = "example/agent"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.model"]
"#,
        )
        .expect("manifest should be written");
        let uri = directory.uri("main.krit");
        fs::write(directory.path.join("main.krit"), "").expect("entry should exist");
        let source = r#"fn model() -> Result<String, String> {
    config_string("agent.model")
}
let configured = model();
"#;
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(uri.clone(), 1, source.to_owned());

        let hover = state
            .hover(&uri, Position::new(1, 8))
            .expect("hover should succeed")
            .expect("call should have hover facts");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("hover should use markdown")
        };
        assert!(markup.value.contains("Result<String, String>"));
        assert!(markup.value.contains("config.read"));
        assert!(markup.value.contains("agent.model"));
        assert!(markup.value.contains("granted"));

        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["schema"], 1);
        assert_eq!(facts["authoringProtocol"], 1);
        assert_eq!(facts["package"]["name"], "example/agent");
        assert_eq!(
            facts["package"]["requiredPermissions"][0]["capability"],
            "config.read"
        );
        assert_eq!(facts["package"]["requiredPermissions"][0]["granted"], true);
    }

    #[test]
    fn completes_fields_visible_symbols_builtins_types_and_resources() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path.join("krit.pkg"),
            r#"
schema = 1

[package]
name = "example/agent"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.model", "agent.timeout-ms"]
state = ["agent-work"]
"#,
        )
        .expect("manifest should be written");
        fs::write(directory.path.join("main.krit"), "").expect("entry should exist");
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(
            uri.clone(),
            1,
            "fn path(request: HttpRequest) -> String {\n    request.\n}\n".to_owned(),
        );
        let CompletionResponse::List(fields) = state
            .completion(&uri, Position::new(1, 12))
            .expect("field completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        assert_eq!(
            fields
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["body", "headers", "method", "path", "query"]
        );

        state
            .change(&uri, 2, "let key = config_string(\"agent.\");\n".to_owned())
            .expect("document change should succeed");
        let CompletionResponse::List(resources) = state
            .completion(&uri, Position::new(0, 31))
            .expect("resource completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        assert_eq!(
            resources
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["agent.model", "agent.timeout-ms"]
        );

        state
            .change(&uri, 3, "let value: Str\n".to_owned())
            .expect("document change should succeed");
        let CompletionResponse::List(types) = state
            .completion(&uri, Position::new(0, 14))
            .expect("type completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        assert!(types.items.iter().any(|item| item.label == "String"));

        state
            .change(&uri, 4, "let value = state_get(\"ag\");\n".to_owned())
            .expect("document change should succeed");
        let CompletionResponse::List(resources) = state
            .completion(&uri, Position::new(0, 25))
            .expect("state resource completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        assert_eq!(
            resources
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["agent-work"]
        );
        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("state compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["durableState"]["operations"][0]["kind"], "state-get");
    }

    #[test]
    fn secret_completion_offers_bound_and_legacy_secrets() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path.join("krit.pkg"),
            r#"
schema = 1

[package]
name = "example/agent"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
http = ["https://api.github.com"]
secrets = ["legacy-token"]

[secrets.github-token]
origins = ["https://api.github.com"]
"#,
        )
        .expect("manifest should be written");
        fs::write(directory.path.join("main.krit"), "").expect("entry should exist");
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(uri.clone(), 1, "let token = secret(\"\");\n".to_owned());
        let CompletionResponse::List(resources) = state
            .completion(&uri, Position::new(0, 20))
            .expect("secret completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        assert_eq!(
            resources
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["github-token", "legacy-token"]
        );
    }

    #[test]
    fn hover_and_completion_accept_a_trailing_empty_line() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(uri.clone(), 1, "let answer = 42;\n".to_owned());

        assert_eq!(
            state
                .hover(&uri, Position::new(1, 0))
                .expect("hover should accept the final empty line"),
            None
        );
        assert!(
            matches!(
                state
                    .completion(&uri, Position::new(1, 0))
                    .expect("completion should accept the final empty line"),
                CompletionResponse::List(_)
            ),
            "completion should return a bounded list"
        );
    }

    #[test]
    fn completion_respects_lexical_visibility_and_custom_record_fields() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(
            uri.clone(),
            1,
            "let top = 1;\nfn choose(parameter: Int) -> Int {\n    let local = parameter;\n    local\n}\nlet result = top;\n"
                .to_owned(),
        );

        let CompletionResponse::List(inside) = state
            .completion(&uri, Position::new(3, 9))
            .expect("completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        let inside = inside
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<BTreeSet<_>>();
        for expected in ["choose", "local", "parameter", "top"] {
            assert!(inside.contains(expected), "{expected} should be visible");
        }

        let CompletionResponse::List(outside) = state
            .completion(&uri, Position::new(5, 16))
            .expect("completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        let outside = outside
            .items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<BTreeSet<_>>();
        assert!(outside.contains("choose"));
        assert!(outside.contains("top"));
        assert!(!outside.contains("local"));
        assert!(!outside.contains("parameter"));

        state
            .change(
                &uri,
                2,
                "fn select(value: Record { alpha: Int, beta: String }) -> String {\n    value.\n}\n"
                    .to_owned(),
            )
            .expect("document change should succeed");
        let CompletionResponse::List(fields) = state
            .completion(&uri, Position::new(1, 10))
            .expect("field completion should succeed")
        else {
            panic!("completion should return a bounded list")
        };
        assert_eq!(
            fields
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
    }

    #[test]
    fn reports_module_init_and_webhook_entrypoints() {
        let directory = TestDirectory::new();
        fs::write(
            directory.path.join("krit.pkg"),
            r#"
schema = 1

[package]
name = "example/webhook"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"

[capabilities]
config = ["agent.model"]
stdout = true
"#,
        )
        .expect("manifest should be written");
        fs::write(directory.path.join("main.krit"), "").expect("entry should exist");
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(
            uri.clone(),
            1,
            r#"webhook fn handle(request: HttpRequest) -> HttpResponse {
    let model = config_string("agent.model");
    println(request.path);
    record { status: 200, headers: [], body: request.path }
}
"#
            .to_owned(),
        );

        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["module"]["entrypoints"][0]["kind"], "module-init");
        assert_eq!(facts["module"]["entrypoints"][1]["kind"], "webhook");
        assert_eq!(facts["module"]["entrypoints"][1]["name"], "handle");
        assert_eq!(
            facts["module"]["entrypoints"][1]["signature"],
            "webhook fn handle(request: HttpRequest) -> HttpResponse"
        );
        assert_eq!(
            facts["package"]["requiredPermissions"][0],
            serde_json::json!({
                "capability": "config.read",
                "resource": "agent.model",
                "granted": true
            })
        );
        assert_eq!(facts["package"]["requestedPermissions"][0]["used"], true);
        assert_eq!(
            facts["package"]["requestedPermissions"][1],
            serde_json::json!({"capability": "io.stdout", "used": true})
        );
    }

    #[test]
    fn reports_queue_and_schedule_contracts_and_transitive_module_permissions() {
        for (kind, parameter, ty, capability, manifest_key) in [
            ("queue", "job", "QueueJob", "queue.consume", "consumes"),
            (
                "schedule",
                "event",
                "ScheduleEvent",
                "schedule.trigger",
                "schedules",
            ),
        ] {
            for granted in [None, Some(false), Some(true)] {
                let directory = TestDirectory::new();
                fs::write(directory.path.join("main.krit"), "").expect("entry should exist");
                if let Some(granted) = granted {
                    let requested = if granted { kind } else { "other" };
                    fs::write(
                        directory.path.join("krit.pkg"),
                        format!(
                            r#"schema = 1
[package]
name = "example/deployment"
version = "0.2.0"
edition = "2026"
entry = "main.krit"
license = "Apache-2.0"
[capabilities]
config = ["agent.model", "unused.model"]
stdout = true
{manifest_key} = ["{requested}"]
"#
                        ),
                    )
                    .expect("manifest should be written");
                }
                let source = format!(
                    r#"println("module init");
fn model() -> Result<String, String> {{
    config_string("agent.model")
}}
fn unused() -> Result<String, String> {{
    config_string("unused.model")
}}
{kind} "{kind}" fn {kind}(input: {ty}) -> Result<String, String> {{
    let configured = model();
    println(input.id);
    Ok(input.id)
}}
"#
                );
                let uri = directory.uri("main.krit");
                let mut state = ServerState::new(vec![directory.path.clone()]);
                let diagnostics = state.open(uri.clone(), 1, source.clone());
                assert!(diagnostics.diagnostics.is_empty());
                let facts = serde_json::to_value(state.compiler_facts(&uri).unwrap()).unwrap();
                assert_eq!(
                    facts,
                    serde_json::to_value(state.compiler_facts(&uri).unwrap()).unwrap(),
                    "compiler facts must be deterministic"
                );
                assert_eq!(facts["schema"], 1);
                assert_eq!(facts["valid"], true);
                let mut required = serde_json::json!([
                    {"capability": "config.read", "resource": "agent.model", "granted": true},
                    {"capability": "io.stdout", "granted": true},
                    {"capability": capability, "resource": kind, "granted": granted}
                ]);
                if granted.is_none() {
                    for permission in required.as_array_mut().unwrap() {
                        permission.as_object_mut().unwrap().remove("granted");
                    }
                }
                assert_eq!(
                    facts["module"]["effects"],
                    serde_json::json!(["config.read", "io.stdout", capability])
                );
                assert_eq!(facts["module"]["capabilityRequirements"], required);
                assert_eq!(
                    facts["module"]["entrypoints"],
                    serde_json::json!([
                        {
                            "name": "<module-init>",
                            "kind": "module-init",
                            "signature": "fn() -> Unit",
                            "effects": ["io.stdout"],
                            "capabilityRequirements": [required[1].clone()]
                        },
                        {
                            "name": kind,
                            "kind": kind,
                            "signature": format!("{kind} fn {kind}({parameter}: {ty}) -> Result<String, String>"),
                            "effects": ["config.read", "io.stdout", capability],
                            "capabilityRequirements": required
                        }
                    ])
                );
                if let Some(granted) = granted {
                    assert_eq!(facts["package"]["requiredPermissions"], required);
                    assert_eq!(facts["package"]["allRequiredGranted"], granted);
                    let requested = facts["package"]["requestedPermissions"].as_array().unwrap();
                    assert!(requested.iter().any(|permission| {
                        permission["resource"] == "agent.model" && permission["used"] == true
                    }));
                    assert!(requested.iter().any(|permission| {
                        permission["resource"] == "unused.model" && permission["used"] == false
                    }));
                    assert!(requested.iter().any(|permission| {
                        permission["capability"] == capability && permission["used"] == granted
                    }));
                } else {
                    assert!(facts.get("package").is_none());
                }

                let name_start = source.find(&format!("fn {kind}(")).unwrap() + 3;
                let name_range = LineIndex::new(&source)
                    .range(&source, Span::new(name_start, name_start + kind.len()));
                let hover = state.hover(&uri, name_range.start).unwrap().unwrap();
                assert_eq!(hover.range, Some(name_range));
                let HoverContents::Markup(markup) = hover.contents else {
                    panic!("hover should use markdown")
                };
                assert!(markup.value.contains(&format!("**Entrypoint:** `{kind}`")));
                assert!(markup.value.contains(capability));
                assert!(markup.value.contains("config.read"));
                assert!(markup.value.contains("agent.model"));
                let symbol = facts["symbols"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|symbol| symbol["name"] == kind)
                    .unwrap();
                assert_eq!(
                    symbol["declaredType"],
                    format!("{kind} fn({ty}) -> Result<String, String>")
                );
                assert_eq!(
                    symbol["selectionRange"],
                    serde_json::to_value(name_range).unwrap()
                );
                let Some(DocumentSymbolResponse::Nested(symbols)) =
                    state.document_symbols(&uri).unwrap()
                else {
                    panic!("document symbols should be available")
                };
                let symbol = symbols.iter().find(|symbol| symbol.name == kind).unwrap();
                assert_eq!(symbol.kind, SymbolKind::FUNCTION);
                assert_eq!(symbol.selection_range, name_range);
            }
        }
    }

    #[test]
    fn completes_every_deployment_entrypoint_keyword_and_contract_type() {
        for keyword in ["webhook", "queue", "schedule"] {
            assert!(
                keyword_completions()
                    .iter()
                    .any(|item| item.label == keyword)
            );
        }
        for ty in ["HttpRequest", "HttpResponse", "QueueJob", "ScheduleEvent"] {
            assert!(type_completions().iter().any(|item| item.label == ty));
        }
    }

    #[test]
    fn reports_named_construction_facts_hover_and_punned_formatting() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let source =
            "let body = \"x\";\nlet response = HttpResponse { status: 200, body: body };\n";
        let mut state = ServerState::new(vec![directory.path.clone()]);
        let published = state.open(uri.clone(), 1, source.to_owned());
        assert!(published.diagnostics.is_empty());

        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        let expressions = facts["expressions"]
            .as_array()
            .expect("expression facts should be a list");
        let construction = expressions
            .iter()
            .find(|fact| fact["syntaxKind"] == "construction")
            .expect("the construction should have an expression fact");
        assert_eq!(
            construction["inferredType"],
            "Record { body: String, headers: List<HttpHeader>, status: Int }"
        );
        assert!(
            expressions
                .iter()
                .any(|fact| fact["syntaxKind"] == "name" && fact["resolved"]["name"] == "body"),
            "the field value should resolve: {expressions:?}"
        );

        let hover = state
            .hover(&uri, Position::new(1, 17))
            .expect("hover should succeed")
            .expect("the construction should have hover facts");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("hover should use markdown")
        };
        assert!(
            markup.value.contains("headers: List<HttpHeader>"),
            "{}",
            markup.value
        );

        let edits = state.formatting(&uri).expect("formatting should succeed");
        assert_eq!(
            edits[0].new_text,
            "let body = \"x\";\nlet response = HttpResponse { status: 200, body };\n"
        );

        let published = state
            .change(
                &uri,
                2,
                "let response = HttpResponse { stauts: 200 };\n".to_owned(),
            )
            .expect("the document should change");
        assert_eq!(published.diagnostics.len(), 1);
        let diagnostic = &published.diagnostics[0];
        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("K3016".to_owned()))
        );
        assert!(
            diagnostic.message.contains("help: did you mean `status`?"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn formatting_is_idempotent_and_available_as_a_code_action() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(uri.clone(), 1, "let answer=6*7;\n".to_owned());

        let edits = state.formatting(&uri).expect("formatting should succeed");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "let answer = 6 * 7;\n");
        assert_eq!(
            state
                .code_actions(&uri)
                .expect("code actions should succeed")
                .len(),
            1
        );

        state
            .change(&uri, 2, edits[0].new_text.clone())
            .expect("formatted document should open");
        assert!(
            state
                .formatting(&uri)
                .expect("formatting should remain available")
                .is_empty()
        );
        assert!(
            state
                .code_actions(&uri)
                .expect("code actions should succeed")
                .is_empty()
        );
    }

    #[test]
    fn rejects_oversized_documents_without_retaining_or_compiling_them() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        let diagnostics = state.open(uri.clone(), 1, "x".repeat(MAX_DOCUMENT_BYTES + 1));

        assert_eq!(diagnostics.diagnostics.len(), 1);
        assert_eq!(
            diagnostics.diagnostics[0].code,
            Some(NumberOrString::String("K8002".to_owned()))
        );
        assert!(state.formatting(&uri).is_err());
        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("bounded failure facts should be available"),
        )
        .expect("facts should serialize");
        assert_eq!(facts["valid"], false);
        assert_eq!(facts["expressions"], serde_json::json!([]));
    }

    #[test]
    fn bounds_the_open_document_set() {
        let directory = TestDirectory::new();
        let mut state = ServerState::new(vec![directory.path.clone()]);
        for index in 0..MAX_OPEN_DOCUMENTS {
            let uri = directory.uri(&format!("document-{index}.krit"));
            let diagnostics = state.open(uri, 1, "let value = 1;\n".to_owned());
            assert!(diagnostics.diagnostics.is_empty());
        }
        let rejected = directory.uri("rejected.krit");
        let diagnostics = state.open(rejected.clone(), 1, "let value = 1;\n".to_owned());
        assert_eq!(
            diagnostics.diagnostics[0].code,
            Some(NumberOrString::String("K8002".to_owned()))
        );
        assert!(state.compiler_facts(&rejected).is_err());
    }

    #[test]
    fn compiler_facts_are_deterministic_and_do_not_execute_host_effects() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(
            uri.clone(),
            1,
            "println(42);\nlet result = http_request(\"https://example.com\", record { method: \"GET\", path: \"/\", query: \"\", headers: [], body: \"\" }, None);\n".to_owned(),
        );

        let first = serde_json::to_vec(
            &state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        let second = serde_json::to_vec(
            &state
                .compiler_facts(&uri)
                .expect("compiler facts should succeed"),
        )
        .expect("facts should serialize");
        assert_eq!(first, second);
        let facts: serde_json::Value =
            serde_json::from_slice(&first).expect("facts should be JSON");
        assert_eq!(
            facts["module"]["effects"],
            serde_json::json!(["http.request", "io.stdout"])
        );
    }

    #[test]
    fn recursive_shared_types_fail_before_unbounded_rendering() {
        let directory = TestDirectory::new();
        let uri = directory.uri("main.krit");
        let mut source = "let value0 = record { leaf: 0 };\n".to_owned();
        for level in 1..=24 {
            source.push_str(&format!(
                "let value{level} = record {{ left: value{}, right: value{} }};\n",
                level - 1,
                level - 1
            ));
        }
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(uri.clone(), 1, source);

        let facts = state
            .compiler_facts(&uri)
            .expect_err("exponentially rendered facts should fail closed");
        assert!(facts.contains("type"));
        let completion = state
            .completion(&uri, Position::new(24, 1))
            .expect_err("completion should use the same bounded renderer");
        assert!(completion.contains("type"));
    }

    #[cfg(unix)]
    #[test]
    fn package_facts_reject_symlinked_entries_outside_the_package() {
        use std::os::unix::fs::symlink;

        let package = TestDirectory::new();
        let outside = TestDirectory::new();
        fs::write(outside.path.join("main.krit"), "let value = 1;\n")
            .expect("outside source should be written");
        symlink(&outside.path, package.path.join("link"))
            .expect("package symlink should be created");
        fs::write(
            package.path.join("krit.pkg"),
            r#"
schema = 1

[package]
name = "example/escaped"
version = "0.2.0"
edition = "2026"
entry = "link/main.krit"
license = "Apache-2.0"
"#,
        )
        .expect("manifest should be written");
        let uri: Uri = Url::from_file_path(package.path.join("link/main.krit"))
            .expect("path should become a URL")
            .as_str()
            .parse()
            .expect("URL should become an LSP URI");
        let mut state = ServerState::new(vec![package.path.clone()]);
        state.open(uri.clone(), 1, "let value = 1;\n".to_owned());

        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("source facts should remain available"),
        )
        .expect("facts should serialize");
        assert!(facts.get("package").is_none());
    }

    #[test]
    fn package_discovery_rejects_oversized_manifests() {
        let directory = TestDirectory::new();
        fs::write(directory.path.join("main.krit"), "let value = 1;\n")
            .expect("entry should be written");
        fs::write(
            directory.path.join("krit.pkg"),
            format!(
                "schema = 1\n\n[package]\nname = \"example/large\"\nversion = \"0.2.0\"\nedition = \"2026\"\nentry = \"main.krit\"\nlicense = \"{}\"\n",
                "x".repeat(MAX_MANIFEST_BYTES)
            ),
        )
        .expect("manifest should be written");
        let uri = directory.uri("main.krit");
        let mut state = ServerState::new(vec![directory.path.clone()]);
        state.open(uri.clone(), 1, "let value = 1;\n".to_owned());

        let facts = serde_json::to_value(
            state
                .compiler_facts(&uri)
                .expect("source facts should remain available"),
        )
        .expect("facts should serialize");
        assert!(facts.get("package").is_none());
    }

    #[test]
    fn package_discovery_rejects_declared_dependencies() {
        let directory = TestDirectory::new();
        fs::write(directory.path.join("main.krit"), "let value = 1;\n")
            .expect("entry should be written");
        let manifest = "schema = 1\n\n[package]\nname = \"example/deps\"\nversion = \"0.2.0\"\nedition = \"2026\"\nentry = \"main.krit\"\nlicense = \"Apache-2.0\"\n\n[dependencies]\n";
        let uri = directory.uri("main.krit");

        for (dependencies, attached) in [("\"krit/json\" = \"1.2.3\"\n", false), ("", true)] {
            fs::write(
                directory.path.join("krit.pkg"),
                format!("{manifest}{dependencies}"),
            )
            .expect("manifest should be written");
            let mut state = ServerState::new(vec![directory.path.clone()]);
            state.open(uri.clone(), 1, "let value = 1;\n".to_owned());

            let facts = serde_json::to_value(
                state
                    .compiler_facts(&uri)
                    .expect("source facts should remain available"),
            )
            .expect("facts should serialize");
            if attached {
                assert_eq!(facts["package"]["dependencies"], serde_json::json!([]));
            } else {
                assert!(facts.get("package").is_none());
            }
        }
    }

    #[test]
    fn field_completion_repair_has_a_cumulative_work_budget() {
        assert_eq!(field_repair_attempts(800_000), 1);
        assert_eq!(field_repair_attempts(264_496), 3);
        assert_eq!(field_repair_attempts(1024), MAX_FIELD_REPAIR_ATTEMPTS);
    }

    #[test]
    fn read_grants_complete_the_union_of_both_lists() {
        let writable = vec!["beta".to_owned(), "alpha".to_owned()];
        let read_only = vec!["gamma".to_owned()];

        // Writable only.
        assert_eq!(
            union_resources(&writable, &[]),
            ["alpha".to_owned(), "beta".to_owned()]
        );
        // Read-only only.
        assert_eq!(union_resources(&[], &read_only), ["gamma".to_owned()]);
        // Both, sorted.
        assert_eq!(
            union_resources(&writable, &read_only),
            ["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()]
        );
        // A name appearing in both lists is offered once. Manifest validation
        // rejects that overlap, so this only guards the completion path.
        assert_eq!(
            union_resources(&["alpha".to_owned()], &["alpha".to_owned()]),
            ["alpha".to_owned()]
        );
        assert!(union_resources(&[], &[]).is_empty());
    }
}
