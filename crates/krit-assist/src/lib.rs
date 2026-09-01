mod context;
mod error;
mod proposal;
mod protocol;
mod provider;

pub use context::{
    ContextSelection, Inspection, PreparedAssistance, RequestOptions, RequestedRange,
    prepare_request,
};
pub use error::{AssistError, AssistErrorKind};
pub use proposal::{
    AcceptedProposal, AssistProposal, CompilerSnapshot, DiagnosticSnapshot, PermissionFact,
    PermissionKey, PermissionReview, RequestedPermissionReview, ReviewReport, ReviewedProposal,
    SetDelta, TypeSnapshot, accept_reviewed, build_proposal, load_proposal, review_loaded_proposal,
    review_proposal, suggest, write_proposal,
};
pub use protocol::{
    AUTHORING_PROTOCOL_VERSION, AssistRequest, AssistResponse, ContextRedaction, ContextRole,
    ContextSlice, DocumentPrecondition, ProposedTextEdit, ProviderDescriptor, RequestTarget,
    ResponseDocument, SuggestionKind, TextPosition, TextRange,
};
pub use provider::{
    MAX_PROVIDER_RESPONSE_BYTES, ProviderConfig, SuggestionProvider, decode_response,
};

pub fn render_inspection_human(inspection: &Inspection) -> Result<String, AssistError> {
    let request = serde_json::to_string_pretty(&inspection.request)
        .map_err(|_| AssistError::io("could not render assist inspection"))?;
    Ok(format!(
        "Krit assist inspection (schema {})\nprovider: {} {}\ncredential source: {}\ncontext bytes: {}\nexact provider request:\n{}\n",
        inspection.schema,
        escape_human_text(&inspection.provider.kind),
        escape_human_text(&inspection.provider.endpoint),
        escape_human_text(
            inspection
                .provider
                .credential_source
                .as_deref()
                .unwrap_or("(none)")
        ),
        inspection.total_context_bytes,
        escape_human_text(&request)
    ))
}

pub fn render_proposal_human(proposal: &AssistProposal) -> Result<String, AssistError> {
    let review = serde_json::to_string_pretty(&proposal.review)
        .map_err(|_| AssistError::io("could not render assist review"))?;
    let summary = serde_json::to_string(&proposal.review.provider_summary_untrusted)
        .map_err(|_| AssistError::io("could not render provider summary"))?;
    Ok(format!(
        "Krit assist proposal (schema {})\nproposal: {}\nprovider summary (untrusted): {}\nreview facts:\n{}\ndiff:\n{}",
        proposal.schema,
        proposal.proposal_id,
        escape_human_text(&summary),
        escape_human_text(&review),
        escape_human_text(&proposal.diff)
    ))
}

pub fn render_review_human(reviewed: &ReviewedProposal) -> Result<String, AssistError> {
    render_proposal_human(reviewed.proposal())
}

pub fn escape_human_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if is_terminal_control(character) {
            escaped.push_str(&format!("\\u{{{:04x}}}", character as u32));
        } else {
            escaped.push(character);
        }
    }
    escaped
}

pub(crate) fn is_terminal_control(character: char) -> bool {
    (character.is_control() && !matches!(character, '\n' | '\t'))
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn authoring_dependencies_do_not_leak_runtime_or_compiler_backends() {
        let manifest = include_str!("../Cargo.toml");
        for dependency in [
            "krit-runtime",
            "krit-wasm",
            "krit-cli",
            "wasmtime",
            "tiny_http",
        ] {
            let dependency_line = format!("{dependency}.workspace = true");
            assert!(
                !manifest
                    .lines()
                    .take_while(|line| *line != "[dev-dependencies]")
                    .any(|line| line == dependency_line),
                "{dependency} must not be a normal assist dependency"
            );
        }
    }
}
