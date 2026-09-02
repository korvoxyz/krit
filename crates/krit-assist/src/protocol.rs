use serde::{Deserialize, Serialize};

pub const AUTHORING_PROTOCOL_VERSION: u32 = 1;
pub const REQUEST_SCHEMA_VERSION: u32 = 1;
pub const RESPONSE_SCHEMA_VERSION: u32 = 1;
pub const PROPOSAL_SCHEMA_VERSION: u32 = 1;
pub const PROMPT_PACK_VERSION: &str = "0.2.12";
pub const LANGUAGE_VERSION: &str = "0.2.0";
pub const LANGUAGE_EDITION: &str = "2026";
pub const AUTHORING_INSTRUCTION: &str = include_str!("../assets/KRIT-AUTHORING-1.md");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SuggestionKind {
    Completion,
    DiagnosticRepair,
    SemanticCleanup,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TextRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start: TextPosition,
    pub end: TextPosition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DocumentPrecondition {
    pub path: String,
    pub digest: String,
    pub byte_length: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RequestTarget {
    pub document: DocumentPrecondition,
    pub selection: TextRange,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextRole {
    Target,
    Additional,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContextRedaction {
    pub range: TextRange,
    pub category: String,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContextSlice {
    pub role: ContextRole,
    pub document: DocumentPrecondition,
    pub range: TextRange,
    pub text: String,
    pub redactions: Vec<ContextRedaction>,
    pub untrusted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AssistRequest {
    pub schema: u32,
    pub authoring_protocol: u32,
    pub prompt_pack_version: String,
    pub language_version: String,
    pub edition: String,
    pub request_id: String,
    pub kind: SuggestionKind,
    pub instruction: String,
    pub intent: String,
    pub target: RequestTarget,
    pub contexts: Vec<ContextSlice>,
    pub compiler_facts: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResponseDocument {
    pub path: String,
    pub base_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProposedTextEdit {
    pub range: TextRange,
    pub new_text: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AssistResponse {
    pub schema: u32,
    pub authoring_protocol: u32,
    pub request_id: String,
    pub document: ResponseDocument,
    pub summary: String,
    pub edits: Vec<ProposedTextEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderDescriptor {
    pub kind: String,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_source: Option<String>,
}
