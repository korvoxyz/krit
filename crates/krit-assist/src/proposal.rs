use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

use krit::{Source, analyze, format_source, lower, parse_source};
use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::{
    context::{
        LoadedPackage, MAX_CONTEXT_BYTES, MAX_CONTEXT_SLICE_BYTES, MAX_CONTEXTS,
        MAX_MANIFEST_BYTES, MAX_REQUEST_BYTES, PreparedAssistance, digest_bytes,
        document_precondition, load_package, provider_compiler_facts, read_bounded_utf8,
        read_resolved_source, read_source, request_id, resolve_source_path, validate_intent,
        validate_text_range, verify_context_slice,
    },
    error::AssistError,
    protocol::{
        AUTHORING_INSTRUCTION, AUTHORING_PROTOCOL_VERSION, AssistRequest, AssistResponse,
        LANGUAGE_EDITION, LANGUAGE_VERSION, PROMPT_PACK_VERSION, PROPOSAL_SCHEMA_VERSION,
        ProposedTextEdit, ProviderDescriptor, REQUEST_SCHEMA_VERSION, TextRange,
    },
    provider::SuggestionProvider,
};

pub const MAX_EDITS: usize = 64;
pub const MAX_EDIT_BYTES: usize = 256 * 1024;
pub const MAX_EDIT_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_PROVIDER_SUMMARY_BYTES: usize = 4 * 1024;
pub const MAX_PROPOSAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
const MAX_REVIEW_TYPE_BYTES: usize = 16 * 1024;
static STAGED_FILE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionKey {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

impl FromStr for PermissionKey {
    type Err = AssistError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (capability, resource) = value
            .split_once('=')
            .map_or((value, None), |(capability, resource)| {
                (capability, Some(resource.to_owned()))
            });
        let resource_required = matches!(
            capability,
            "ai.invoke"
                | "config.read"
                | "http.request"
                | "object.read"
                | "object.write"
                | "queue.consume"
                | "queue.publish"
                | "schedule.trigger"
                | "secret.read"
                | "state.transaction"
        );
        let resource_forbidden = matches!(capability, "io.stdout" | "observe.log");
        if !resource_required && !resource_forbidden {
            return Err(AssistError::permission(
                "permission approval names an unknown capability",
            ));
        }
        if resource_required && resource.as_deref().is_none_or(str::is_empty) {
            return Err(AssistError::permission(
                "resource capability approval requires `CAPABILITY=RESOURCE`",
            ));
        }
        if resource_forbidden && resource.is_some() {
            return Err(AssistError::permission(
                "resource-less capability approval must not include `=RESOURCE`",
            ));
        }
        Ok(Self {
            capability: capability.to_owned(),
            resource,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DiagnosticSnapshot {
    pub code: String,
    pub message: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TypeSnapshot {
    pub name: String,
    pub kind: String,
    pub inferred_type: String,
    pub effects: Vec<String>,
    pub capability_requirements: Vec<PermissionFact>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionFact {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub granted: bool,
}

impl PermissionFact {
    fn key(&self) -> PermissionKey {
        PermissionKey {
            capability: self.capability.clone(),
            resource: self.resource.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompilerSnapshot {
    pub valid: bool,
    pub diagnostics: Vec<DiagnosticSnapshot>,
    pub top_level_types: Vec<TypeSnapshot>,
    pub effects: Vec<String>,
    pub required_permissions: Vec<PermissionFact>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SetDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedPermissionReview {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub used_before: bool,
    pub used_after: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PermissionReview {
    pub requested: Vec<RequestedPermissionReview>,
    pub added_required: Vec<PermissionFact>,
    pub removed_required: Vec<PermissionFact>,
    pub missing_after: Vec<PermissionFact>,
    pub approval_required: Vec<PermissionKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReviewReport {
    pub schema: u32,
    pub provider_summary_untrusted: String,
    pub formatting_changed_provider_text: bool,
    pub before: CompilerSnapshot,
    pub after: CompilerSnapshot,
    pub effect_delta: SetDelta,
    pub permissions: PermissionReview,
    pub explanation: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AssistProposal {
    pub schema: u32,
    pub authoring_protocol: u32,
    pub proposal_id: String,
    pub provider: ProviderDescriptor,
    pub manifest_digest: String,
    pub target_precondition: crate::protocol::DocumentPrecondition,
    pub context_preconditions: Vec<crate::protocol::DocumentPrecondition>,
    pub request: AssistRequest,
    pub response: AssistResponse,
    pub candidate_digest: String,
    pub diff: String,
    pub review: ReviewReport,
}

pub struct ReviewedProposal {
    proposal: AssistProposal,
    package: LoadedPackage,
    target_path: PathBuf,
    base_source: String,
    candidate: String,
}

impl ReviewedProposal {
    pub fn proposal(&self) -> &AssistProposal {
        &self.proposal
    }

    pub fn review(&self) -> &ReviewReport {
        &self.proposal.review
    }

    pub fn diff(&self) -> &str {
        &self.proposal.diff
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedProposal {
    pub schema: u32,
    pub proposal_id: String,
    pub target: String,
    pub digest: String,
    pub review: ReviewReport,
}

pub fn suggest(
    prepared: PreparedAssistance,
    provider: &dyn SuggestionProvider,
) -> Result<AssistProposal, AssistError> {
    let response = provider.suggest(prepared.request())?;
    build_proposal(prepared, response)
}

pub fn build_proposal(
    prepared: PreparedAssistance,
    response: AssistResponse,
) -> Result<AssistProposal, AssistError> {
    validate_request(&prepared.inspection().request)?;
    validate_response(
        &prepared.inspection().request,
        &prepared.state.base_source,
        &response,
    )?;
    let package = load_package(&prepared.state.manifest_path)?;
    if package.manifest_digest != prepared.state.manifest_digest {
        return Err(AssistError::proposal(
            "package manifest changed while creating the proposal",
        ));
    }
    let target = resolve_source_path(
        &package,
        Path::new(&prepared.inspection().request.target.document.path),
        true,
    )?;
    let current_source = read_resolved_source(&package, &target)?;
    if document_precondition(&target.relative, &current_source)
        != prepared.state.target_precondition
    {
        return Err(AssistError::proposal(
            "target document changed during provider invocation",
        ));
    }
    for precondition in &prepared.state.context_preconditions {
        let source_path = resolve_source_path(&package, Path::new(&precondition.path), false)?;
        let source = read_resolved_source(&package, &source_path)?;
        if document_precondition(&source_path.relative, &source) != *precondition {
            return Err(AssistError::proposal(
                "selected context changed during provider invocation",
            ));
        }
    }
    for context in &prepared.inspection().request.contexts {
        if verify_context_slice(&package, context)? != *context {
            return Err(AssistError::proposal(
                "selected context changed during provider invocation",
            ));
        }
    }
    let candidate = candidate_source(
        &prepared.state.target_path,
        &prepared.state.base_source,
        &response.edits,
    )?;
    let provider = prepared.inspection().provider.clone();
    let request = prepared.inspection().request.clone();
    let proposal = proposal_from_candidate(
        provider,
        prepared.state.manifest_digest,
        prepared.state.target_precondition,
        prepared.state.context_preconditions,
        request,
        response,
        &package,
        &prepared.state.target_path,
        &prepared.state.base_source,
        &candidate.raw,
        &candidate.canonical,
    )?;
    Ok(proposal)
}

pub fn write_proposal(path: &Path, proposal: &AssistProposal) -> Result<(), AssistError> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return Err(AssistError::io(
            "proposal output must use the `.json` extension",
        ));
    }
    let mut bytes = serde_json::to_vec_pretty(proposal)
        .map_err(|_| AssistError::io("could not serialize proposal"))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_PROPOSAL_BYTES {
        return Err(AssistError::proposal(format!(
            "proposal exceeds the {MAX_PROPOSAL_BYTES}-byte limit"
        )));
    }
    atomic_replace(path, &bytes, None, None)
}

pub fn load_proposal(path: &Path) -> Result<AssistProposal, AssistError> {
    let contents = read_bounded_utf8(path, MAX_PROPOSAL_BYTES, "proposal")?;
    serde_json::from_str(&contents)
        .map_err(|_| AssistError::proposal("proposal is not strict schema-1 JSON"))
}

pub fn review_proposal(
    manifest_path: &Path,
    proposal_path: &Path,
) -> Result<ReviewedProposal, AssistError> {
    let proposal = load_proposal(proposal_path)?;
    review_loaded_proposal(manifest_path, proposal)
}

pub fn review_loaded_proposal(
    manifest_path: &Path,
    proposal: AssistProposal,
) -> Result<ReviewedProposal, AssistError> {
    validate_proposal_versions(&proposal)?;
    validate_request(&proposal.request)?;
    if proposal.proposal_id != proposal_id(&proposal)? {
        return Err(AssistError::proposal("proposal identity is invalid"));
    }
    let package = load_package(manifest_path)?;
    if package.manifest_digest != proposal.manifest_digest {
        return Err(AssistError::proposal(
            "package manifest changed after suggestion generation",
        ));
    }
    let target_path = package.root.join(&proposal.request.target.document.path);
    let target = resolve_source_path(&package, &target_path, true)?;
    if target.canonical != package.entry {
        return Err(AssistError::proposal(
            "proposal target is not the package entry source",
        ));
    }
    let base_source = read_resolved_source(&package, &target)?;
    if document_precondition(&target.relative, &base_source) != proposal.target_precondition {
        return Err(AssistError::proposal("proposal target document is stale"));
    }
    for precondition in &proposal.context_preconditions {
        let source_path = resolve_source_path(&package, Path::new(&precondition.path), false)?;
        let source = read_resolved_source(&package, &source_path)?;
        if document_precondition(&source_path.relative, &source) != *precondition {
            return Err(AssistError::proposal("proposal context document is stale"));
        }
    }
    for context in &proposal.request.contexts {
        if verify_context_slice(&package, context)? != *context {
            return Err(AssistError::proposal(
                "proposal context does not match the selected redacted source",
            ));
        }
    }
    let facts = krit_lsp::compiler_facts_for_document_with_manifest(
        &target.canonical,
        &package.manifest_path,
        &package.manifest,
        &base_source,
    )
    .map_err(|_| AssistError::proposal("could not recompute language-server facts"))?;
    let facts = provider_compiler_facts(facts, &proposal.request.target.selection)?;
    if facts != proposal.request.compiler_facts {
        return Err(AssistError::proposal(
            "proposal compiler context is stale or modified",
        ));
    }
    validate_response(&proposal.request, &base_source, &proposal.response)?;
    let candidate = candidate_source(&target.canonical, &base_source, &proposal.response.edits)?;
    let recomputed = proposal_from_candidate(
        proposal.provider.clone(),
        proposal.manifest_digest.clone(),
        proposal.target_precondition.clone(),
        proposal.context_preconditions.clone(),
        proposal.request.clone(),
        proposal.response.clone(),
        &package,
        &target.canonical,
        &base_source,
        &candidate.raw,
        &candidate.canonical,
    )?;
    if recomputed != proposal {
        return Err(AssistError::proposal(
            "proposal review facts or diff were modified",
        ));
    }
    Ok(ReviewedProposal {
        proposal,
        package,
        target_path: target.canonical,
        base_source,
        candidate: candidate.canonical,
    })
}

pub fn accept_reviewed(
    reviewed: ReviewedProposal,
    approvals: &BTreeSet<PermissionKey>,
) -> Result<AcceptedProposal, AssistError> {
    let required = reviewed
        .proposal
        .review
        .permissions
        .approval_required
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if approvals != &required {
        return Err(AssistError::permission(
            "permission approvals must exactly match the surfaced authority expansion",
        ));
    }
    if !reviewed
        .proposal
        .review
        .permissions
        .missing_after
        .is_empty()
    {
        return Err(AssistError::permission(
            "candidate requires permissions not granted by the package manifest",
        ));
    }
    if reviewed.package.manifest_digest
        != digest_bytes(
            read_bounded_utf8(
                &reviewed.package.manifest_path,
                MAX_MANIFEST_BYTES,
                "package manifest",
            )?
            .as_bytes(),
        )
    {
        return Err(AssistError::proposal(
            "package manifest changed before acceptance",
        ));
    }
    atomic_replace(
        &reviewed.target_path,
        reviewed.candidate.as_bytes(),
        Some(digest_bytes(reviewed.base_source.as_bytes())),
        Some(
            fs::metadata(&reviewed.target_path)
                .map_err(|_| AssistError::io("could not inspect assist target"))?
                .permissions(),
        ),
    )?;
    Ok(AcceptedProposal {
        schema: 1,
        proposal_id: reviewed.proposal.proposal_id,
        target: reviewed.proposal.request.target.document.path,
        digest: digest_bytes(reviewed.candidate.as_bytes()),
        review: reviewed.proposal.review,
    })
}

fn validate_request(request: &AssistRequest) -> Result<(), AssistError> {
    validate_intent(&request.intent)?;
    if request.schema != REQUEST_SCHEMA_VERSION
        || request.authoring_protocol != AUTHORING_PROTOCOL_VERSION
        || request.prompt_pack_version != PROMPT_PACK_VERSION
        || request.language_version != LANGUAGE_VERSION
        || request.edition != LANGUAGE_EDITION
        || request.instruction != AUTHORING_INSTRUCTION
        || request.request_id != request_id(request)?
    {
        return Err(AssistError::proposal(
            "authoring request version, instruction, or identity is invalid",
        ));
    }
    if request.contexts.is_empty()
        || request.contexts.len() > MAX_CONTEXTS
        || request.contexts[0].role != crate::protocol::ContextRole::Target
        || request.contexts[0].document != request.target.document
        || request.contexts[0].range != request.target.selection
    {
        return Err(AssistError::proposal(
            "authoring request does not contain its selected target context",
        ));
    }
    if request
        .contexts
        .windows(2)
        .any(|pair| context_key(&pair[0]) > context_key(&pair[1]))
    {
        return Err(AssistError::proposal(
            "authoring contexts are not deterministically ordered",
        ));
    }
    let mut context_bytes = 0usize;
    for context in &request.contexts {
        if !context.untrusted || context.text.len() > MAX_CONTEXT_SLICE_BYTES {
            return Err(AssistError::proposal(
                "authoring context is not marked untrusted or exceeds its limit",
            ));
        }
        context_bytes = context_bytes
            .checked_add(context.text.len())
            .ok_or_else(|| AssistError::proposal("authoring context byte count overflowed"))?;
    }
    if context_bytes > MAX_CONTEXT_BYTES {
        return Err(AssistError::proposal(
            "authoring context exceeds the total byte limit",
        ));
    }
    let bytes = serde_json::to_vec(request)
        .map_err(|_| AssistError::proposal("could not serialize authoring request"))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(AssistError::proposal(
            "authoring request exceeds the bounded request limit",
        ));
    }
    Ok(())
}

fn context_key(
    context: &crate::protocol::ContextSlice,
) -> (crate::protocol::ContextRole, &str, usize, usize) {
    (
        context.role,
        &context.document.path,
        context.range.start_byte,
        context.range.end_byte,
    )
}

fn validate_response(
    request: &AssistRequest,
    source: &str,
    response: &AssistResponse,
) -> Result<(), AssistError> {
    if response.schema != crate::protocol::RESPONSE_SCHEMA_VERSION
        || response.authoring_protocol != AUTHORING_PROTOCOL_VERSION
    {
        return Err(AssistError::proposal(
            "provider response uses an unsupported schema",
        ));
    }
    if response.request_id != request.request_id
        || response.document.path != request.target.document.path
        || response.document.base_digest != request.target.document.digest
    {
        return Err(AssistError::proposal(
            "provider response does not match the requested document precondition",
        ));
    }
    if response.summary.len() > MAX_PROVIDER_SUMMARY_BYTES
        || response
            .summary
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t'))
        || response.summary.chars().any(crate::is_terminal_control)
    {
        return Err(AssistError::proposal(format!(
            "provider summary exceeds the {MAX_PROVIDER_SUMMARY_BYTES}-byte limit or contains terminal controls"
        )));
    }
    if response.edits.is_empty() || response.edits.len() > MAX_EDITS {
        return Err(AssistError::proposal(format!(
            "provider must return 1-{MAX_EDITS} text edits"
        )));
    }
    let mut total = 0usize;
    let mut previous: Option<&TextRange> = None;
    for edit in &response.edits {
        validate_text_range(source, &edit.range)?;
        if edit.range.start_byte < request.target.selection.start_byte
            || edit.range.end_byte > request.target.selection.end_byte
        {
            return Err(AssistError::proposal(
                "provider edit escapes the explicitly selected target range",
            ));
        }
        if edit.new_text.len() > MAX_EDIT_TEXT_BYTES
            || edit.new_text.contains('\0')
            || edit.new_text.chars().any(crate::is_terminal_control)
        {
            return Err(AssistError::proposal(format!(
                "one provider edit exceeds the {MAX_EDIT_TEXT_BYTES}-byte limit or contains terminal controls"
            )));
        }
        total = total
            .checked_add(edit.new_text.len())
            .ok_or_else(|| AssistError::proposal("provider edit byte count overflowed"))?;
        if total > MAX_EDIT_BYTES {
            return Err(AssistError::proposal(format!(
                "provider edits exceed the {MAX_EDIT_BYTES}-byte limit"
            )));
        }
        if let Some(previous) = previous {
            if (edit.range.start_byte, edit.range.end_byte)
                < (previous.start_byte, previous.end_byte)
            {
                return Err(AssistError::proposal(
                    "provider edits must be sorted by source range",
                ));
            }
            let duplicate_insertion = previous.start_byte == previous.end_byte
                && edit.range.start_byte == edit.range.end_byte
                && previous.start_byte == edit.range.start_byte;
            if edit.range.start_byte < previous.end_byte || duplicate_insertion {
                return Err(AssistError::proposal(
                    "provider edits overlap or contain ambiguous insertions",
                ));
            }
        }
        previous = Some(&edit.range);
    }
    Ok(())
}

struct CandidateSource {
    raw: String,
    canonical: String,
}

fn candidate_source(
    path: &Path,
    source: &str,
    edits: &[ProposedTextEdit],
) -> Result<CandidateSource, AssistError> {
    let mut raw = source.to_owned();
    for edit in edits.iter().rev() {
        raw.replace_range(edit.range.start_byte..edit.range.end_byte, &edit.new_text);
    }
    if raw.len() > crate::context::MAX_SOURCE_BYTES {
        return Err(AssistError::candidate(
            "candidate source exceeds the bounded source limit",
        ));
    }
    let source_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<assist-candidate>");
    let raw_source = Source::new(source_name, raw.clone());
    let canonical = format_source(&raw_source).map_err(|diagnostic| {
        AssistError::candidate(format!(
            "candidate formatting failed with {}: {}",
            diagnostic.code(),
            diagnostic.message()
        ))
    })?;
    if canonical.len() > crate::context::MAX_SOURCE_BYTES {
        return Err(AssistError::candidate(
            "canonical candidate exceeds the bounded source limit",
        ));
    }
    let canonical_source = Source::new(source_name, canonical.clone());
    let program = parse_source(&canonical_source).map_err(|diagnostic| {
        AssistError::candidate(format!(
            "candidate parsing failed with {}: {}",
            diagnostic.code(),
            diagnostic.message()
        ))
    })?;
    let analysis = analyze(&program).map_err(|diagnostic| {
        AssistError::candidate(format!(
            "candidate analysis failed with {}: {}",
            diagnostic.code(),
            diagnostic.message()
        ))
    })?;
    lower(&program, &analysis)
        .map_err(|_| AssistError::candidate("candidate Core verification failed"))?;
    Ok(CandidateSource { raw, canonical })
}

#[allow(clippy::too_many_arguments)]
fn proposal_from_candidate(
    provider: ProviderDescriptor,
    manifest_digest: String,
    target_precondition: crate::protocol::DocumentPrecondition,
    context_preconditions: Vec<crate::protocol::DocumentPrecondition>,
    request: AssistRequest,
    response: AssistResponse,
    package: &LoadedPackage,
    target_path: &Path,
    base_source: &str,
    raw_candidate: &str,
    canonical_candidate: &str,
) -> Result<AssistProposal, AssistError> {
    let before_facts = krit_lsp::compiler_facts_for_document_with_manifest(
        target_path,
        &package.manifest_path,
        &package.manifest,
        base_source,
    )
    .map_err(|_| AssistError::candidate("could not derive pre-edit compiler facts"))?;
    let after_facts = krit_lsp::compiler_facts_for_document_with_manifest(
        target_path,
        &package.manifest_path,
        &package.manifest,
        canonical_candidate,
    )
    .map_err(|_| AssistError::candidate("could not derive candidate compiler facts"))?;
    let before = compiler_snapshot(&before_facts, &package.manifest)?;
    let after = compiler_snapshot(&after_facts, &package.manifest)?;
    if !after.valid || !after.diagnostics.is_empty() {
        return Err(AssistError::candidate(
            "candidate did not pass deterministic compiler checks",
        ));
    }
    let effect_delta = set_delta(&before.effects, &after.effects);
    let permissions = permission_review(&package.manifest, &before, &after);
    let relative = &request.target.document.path;
    let diff = unified_diff(relative, base_source, canonical_candidate)?;
    let explanation = review_explanation(&before, &after, &effect_delta, &permissions);
    let review = ReviewReport {
        schema: 1,
        provider_summary_untrusted: response.summary.clone(),
        formatting_changed_provider_text: raw_candidate != canonical_candidate,
        before,
        after,
        effect_delta,
        permissions,
        explanation,
    };
    let mut proposal = AssistProposal {
        schema: PROPOSAL_SCHEMA_VERSION,
        authoring_protocol: AUTHORING_PROTOCOL_VERSION,
        proposal_id: String::new(),
        provider,
        manifest_digest,
        target_precondition,
        context_preconditions,
        request,
        response,
        candidate_digest: digest_bytes(canonical_candidate.as_bytes()),
        diff,
        review,
    };
    proposal.proposal_id = proposal_id(&proposal)?;
    let bytes = serde_json::to_vec(&proposal)
        .map_err(|_| AssistError::proposal("could not serialize proposal"))?;
    if bytes.len() > MAX_PROPOSAL_BYTES {
        return Err(AssistError::proposal(format!(
            "proposal exceeds the {MAX_PROPOSAL_BYTES}-byte limit"
        )));
    }
    Ok(proposal)
}

fn validate_proposal_versions(proposal: &AssistProposal) -> Result<(), AssistError> {
    if proposal.schema != PROPOSAL_SCHEMA_VERSION
        || proposal.authoring_protocol != AUTHORING_PROTOCOL_VERSION
    {
        return Err(AssistError::proposal("proposal uses an unsupported schema"));
    }
    if proposal.target_precondition.path != proposal.request.target.document.path
        || proposal.context_preconditions.is_empty()
        || !proposal
            .context_preconditions
            .iter()
            .any(|precondition| precondition == &proposal.target_precondition)
        || proposal
            .context_preconditions
            .windows(2)
            .any(|pair| pair[0].path >= pair[1].path)
    {
        return Err(AssistError::proposal(
            "proposal host-local document preconditions are invalid",
        ));
    }
    let request_paths = proposal
        .request
        .contexts
        .iter()
        .map(|context| context.document.path.as_str())
        .collect::<BTreeSet<_>>();
    let precondition_paths = proposal
        .context_preconditions
        .iter()
        .map(|precondition| precondition.path.as_str())
        .collect::<BTreeSet<_>>();
    if request_paths != precondition_paths {
        return Err(AssistError::proposal(
            "proposal host-local context preconditions do not match the request",
        ));
    }
    Ok(())
}

fn proposal_id(proposal: &AssistProposal) -> Result<String, AssistError> {
    let mut unsigned = proposal.clone();
    unsigned.proposal_id.clear();
    let bytes = serde_json::to_vec(&unsigned)
        .map_err(|_| AssistError::proposal("could not serialize proposal identity"))?;
    Ok(digest_bytes(&bytes))
}

fn unified_diff(path: &str, before: &str, after: &str) -> Result<String, AssistError> {
    let diff = TextDiff::from_lines(before, after);
    let mut unified = diff.unified_diff();
    unified
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"));
    let output = unified.to_string();
    if output.len() > MAX_DIFF_BYTES {
        return Err(AssistError::proposal(format!(
            "review diff exceeds the {MAX_DIFF_BYTES}-byte limit"
        )));
    }
    Ok(output)
}

fn set_delta(before: &[String], after: &[String]) -> SetDelta {
    let before = before.iter().cloned().collect::<BTreeSet<_>>();
    let after = after.iter().cloned().collect::<BTreeSet<_>>();
    SetDelta {
        added: after.difference(&before).cloned().collect(),
        removed: before.difference(&after).cloned().collect(),
    }
}

fn permission_review(
    manifest: &krit_package::Manifest,
    before: &CompilerSnapshot,
    after: &CompilerSnapshot,
) -> PermissionReview {
    let before_required = before
        .required_permissions
        .iter()
        .map(PermissionFact::key)
        .collect::<BTreeSet<_>>();
    let after_required = after
        .required_permissions
        .iter()
        .map(PermissionFact::key)
        .collect::<BTreeSet<_>>();
    let added_keys = after_required
        .difference(&before_required)
        .cloned()
        .collect::<BTreeSet<_>>();
    let removed_keys = before_required
        .difference(&after_required)
        .cloned()
        .collect::<BTreeSet<_>>();
    let added_required = after
        .required_permissions
        .iter()
        .filter(|permission| added_keys.contains(&permission.key()))
        .cloned()
        .collect();
    let removed_required = before
        .required_permissions
        .iter()
        .filter(|permission| removed_keys.contains(&permission.key()))
        .cloned()
        .collect();
    let missing_after = after
        .required_permissions
        .iter()
        .filter(|permission| !permission.granted)
        .cloned()
        .collect();
    let requested = manifest
        .permission_plan()
        .requested
        .into_iter()
        .map(|permission| {
            let key = PermissionKey {
                capability: permission.capability.to_owned(),
                resource: permission.resource.clone(),
            };
            RequestedPermissionReview {
                capability: permission.capability.to_owned(),
                resource: permission.resource,
                used_before: before_required.contains(&key),
                used_after: after_required.contains(&key),
            }
        })
        .collect();
    PermissionReview {
        requested,
        added_required,
        removed_required,
        missing_after,
        approval_required: added_keys.into_iter().collect(),
    }
}

fn review_explanation(
    before: &CompilerSnapshot,
    after: &CompilerSnapshot,
    effects: &SetDelta,
    permissions: &PermissionReview,
) -> String {
    format!(
        "Canonical candidate changes diagnostics {} -> {}, top-level type facts {} -> {}, adds {} effect(s), removes {} effect(s), adds {} required permission(s), and removes {} required permission(s).",
        before.diagnostics.len(),
        after.diagnostics.len(),
        before.top_level_types.len(),
        after.top_level_types.len(),
        effects.added.len(),
        effects.removed.len(),
        permissions.added_required.len(),
        permissions.removed_required.len()
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCompilerFacts {
    valid: bool,
    diagnostics: Vec<RawDiagnostic>,
    module: Option<RawModule>,
    symbols: Vec<RawSymbol>,
}

#[derive(Deserialize)]
struct RawDiagnostic {
    code: String,
    message: String,
    span: RawSpan,
}

#[derive(Deserialize)]
struct RawSpan {
    start: usize,
    end: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawModule {
    effects: Vec<String>,
    capability_requirements: Vec<RawPermission>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSymbol {
    name: String,
    kind: String,
    inferred_type: String,
    top_level: bool,
    effects: Vec<String>,
    capability_requirements: Vec<RawPermission>,
}

#[derive(Deserialize)]
struct RawPermission {
    capability: String,
    resource: Option<String>,
}

fn compiler_snapshot(
    facts: &serde_json::Value,
    manifest: &krit_package::Manifest,
) -> Result<CompilerSnapshot, AssistError> {
    let facts: RawCompilerFacts = serde_json::from_value(facts.clone())
        .map_err(|_| AssistError::candidate("language-server facts use an unknown schema"))?;
    let mut diagnostics = facts
        .diagnostics
        .into_iter()
        .map(|diagnostic| DiagnosticSnapshot {
            code: diagnostic.code,
            message: diagnostic.message,
            start_byte: diagnostic.span.start,
            end_byte: diagnostic.span.end,
        })
        .collect::<Vec<_>>();
    diagnostics.sort_by(|left, right| {
        left.start_byte
            .cmp(&right.start_byte)
            .then_with(|| left.end_byte.cmp(&right.end_byte))
            .then_with(|| left.code.cmp(&right.code))
    });
    let mut top_level_types = facts
        .symbols
        .into_iter()
        .filter(|symbol| symbol.top_level)
        .map(|symbol| {
            if symbol.inferred_type.len() > MAX_REVIEW_TYPE_BYTES {
                return Err(AssistError::candidate(
                    "language-server type fact exceeds the review limit",
                ));
            }
            Ok(TypeSnapshot {
                name: symbol.name,
                kind: symbol.kind,
                inferred_type: symbol.inferred_type,
                effects: symbol.effects,
                capability_requirements: raw_permissions(symbol.capability_requirements, manifest),
            })
        })
        .collect::<Result<Vec<_>, AssistError>>()?;
    top_level_types.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let (effects, required_permissions) = facts.module.map_or_else(
        || (Vec::new(), Vec::new()),
        |module| {
            (
                module.effects,
                raw_permissions(module.capability_requirements, manifest),
            )
        },
    );
    Ok(CompilerSnapshot {
        valid: facts.valid,
        diagnostics,
        top_level_types,
        effects,
        required_permissions,
    })
}

fn raw_permissions(
    permissions: Vec<RawPermission>,
    manifest: &krit_package::Manifest,
) -> Vec<PermissionFact> {
    let mut permissions = permissions
        .into_iter()
        .map(|permission| PermissionFact {
            granted: manifest
                .grants_permission(&permission.capability, permission.resource.as_deref()),
            capability: permission.capability,
            resource: permission.resource,
        })
        .collect::<Vec<_>>();
    permissions.sort();
    permissions.dedup();
    permissions
}

fn atomic_replace(
    path: &Path,
    bytes: &[u8],
    expected_digest: Option<String>,
    permissions: Option<fs::Permissions>,
) -> Result<(), AssistError> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(AssistError::io(
            "atomic output cannot replace a symbolic link",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| AssistError::io("atomic output path has no file name"))?
        .to_string_lossy();
    let mut staged = None;
    for _ in 0..128 {
        let id = STAGED_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.krit-assist-{}-{id}", std::process::id()));
        match create_staged_file(&candidate, permissions.as_ref()) {
            Ok(mut file) => {
                let result = file.write_all(bytes).and_then(|()| file.sync_all());
                if result.is_err() {
                    drop(file);
                    let _ = fs::remove_file(&candidate);
                    return Err(AssistError::io("could not stage atomic output"));
                }
                staged = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(AssistError::io("could not create staged atomic output")),
        }
    }
    let staged =
        staged.ok_or_else(|| AssistError::io("could not allocate staged atomic output"))?;
    if let Some(expected) = expected_digest {
        return install_checked_exchange(path, &staged, &expected);
    }
    if fs::rename(&staged, path).is_err() {
        let _ = fs::remove_file(&staged);
        return Err(AssistError::io("could not install atomic output"));
    }
    Ok(())
}

#[cfg(unix)]
fn create_staged_file(
    path: &Path,
    permissions: Option<&fs::Permissions>,
) -> std::io::Result<fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mode = permissions.map_or(0o600, PermissionsExt::mode);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions.clone())?;
    }
    Ok(file)
}

#[cfg(not(unix))]
fn create_staged_file(
    path: &Path,
    permissions: Option<&fs::Permissions>,
) -> std::io::Result<fs::File> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions.clone())?;
    }
    Ok(file)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn install_checked_exchange(
    path: &Path,
    staged: &Path,
    expected_digest: &str,
) -> Result<(), AssistError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    if renameat_with(CWD, staged, CWD, path, RenameFlags::EXCHANGE).is_err() {
        let _ = fs::remove_file(staged);
        return Err(AssistError::io(
            "could not atomically exchange assist target",
        ));
    }
    let displaced_matches = match read_source(staged) {
        Ok(displaced) => digest_bytes(displaced.as_bytes()) == expected_digest,
        Err(_) => {
            rollback_exchange(path, staged)?;
            return Err(AssistError::proposal(
                "displaced assist source could not be validated",
            ));
        }
    };
    if displaced_matches {
        if fs::remove_file(staged).is_ok() {
            return Ok(());
        }
        rollback_exchange(path, staged)?;
        return Err(AssistError::io(
            "could not finalize atomic assist replacement",
        ));
    }
    rollback_exchange(path, staged)?;
    Err(AssistError::proposal(
        "assist target changed before atomic exchange",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rollback_exchange(path: &Path, staged: &Path) -> Result<(), AssistError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, staged, CWD, path, RenameFlags::EXCHANGE)
        .map_err(|_| AssistError::io("atomic assist rollback failed"))?;
    fs::remove_file(staged)
        .map_err(|_| AssistError::io("could not remove rolled-back assist candidate"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn install_checked_exchange(
    _path: &Path,
    staged: &Path,
    _expected_digest: &str,
) -> Result<(), AssistError> {
    let _ = fs::remove_file(staged);
    Err(AssistError::io(
        "stale-detecting atomic assist acceptance is supported only on macOS and Linux",
    ))
}
