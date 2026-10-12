use serde::{Deserialize, Serialize};

pub const AUTHORING_PROTOCOL_VERSION: u32 = 1;
pub const REQUEST_SCHEMA_VERSION: u32 = 1;
pub const RESPONSE_SCHEMA_VERSION: u32 = 1;
pub const PROPOSAL_SCHEMA_VERSION: u32 = 1;
/// The `promptVersion` of the shipped `ai/prompt-pack.json`, read at compile
/// time so the advertised version cannot drift from the pack.
pub const PROMPT_PACK_VERSION: &str = prompt_pack_version(PROMPT_PACK_METADATA);
pub const LANGUAGE_VERSION: &str = "0.3.0";
pub const LANGUAGE_EDITION: &str = "2026";
pub const AUTHORING_INSTRUCTION: &str = include_str!("../assets/KRIT-AUTHORING-1.md");

const PROMPT_PACK_METADATA: &str = include_str!("../../../ai/prompt-pack.json");
const PROMPT_VERSION_KEY: &[u8] = b"promptVersion";

/// Returns the `MAJOR.MINOR.PATCH` string stored under `"promptVersion"`.
///
/// This runs in a const context, so a missing, repeated, non-string, or
/// malformed version fails compilation instead of advertising a guess. The
/// scan is lexical; a unit test cross-checks it against a real JSON parse.
const fn prompt_pack_version(json: &str) -> &str {
    let bytes = json.as_bytes();
    let mut found = None;
    let mut index = 0;
    while index < bytes.len() {
        if let Some(range) = version_value_at(bytes, index) {
            assert!(found.is_none(), "prompt-pack promptVersion is repeated");
            found = Some(range);
        }
        index += 1;
    }
    let Some((start, end)) = found else {
        panic!("prompt-pack promptVersion is missing");
    };
    let (_, tail) = bytes.split_at(start);
    let (value, _) = tail.split_at(end - start);
    match std::str::from_utf8(value) {
        Ok(value) => value,
        Err(_) => panic!("prompt-pack promptVersion is not UTF-8"),
    }
}

/// Matches `"promptVersion"`, a colon, and a quoted version starting at
/// `index`, returning the byte range of the version text.
const fn version_value_at(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let key = PROMPT_VERSION_KEY;
    if index + key.len() + 2 > bytes.len() || bytes[index] != b'"' {
        return None;
    }
    let mut offset = 0;
    while offset < key.len() {
        if bytes[index + 1 + offset] != key[offset] {
            return None;
        }
        offset += 1;
    }
    let mut cursor = index + 1 + key.len();
    if bytes[cursor] != b'"' {
        return None;
    }
    cursor = skip_json_whitespace(bytes, cursor + 1);
    if cursor >= bytes.len() || bytes[cursor] != b':' {
        return None;
    }
    cursor = skip_json_whitespace(bytes, cursor + 1);
    assert!(
        cursor < bytes.len() && bytes[cursor] == b'"',
        "prompt-pack promptVersion is not a string"
    );
    let start = cursor + 1;
    let mut end = start;
    let mut dots = 0;
    let mut digits = 0;
    while end < bytes.len() && bytes[end] != b'"' {
        match bytes[end] {
            b'0'..=b'9' => digits += 1,
            b'.' if digits > 0 => {
                dots += 1;
                digits = 0;
            }
            _ => panic!("prompt-pack promptVersion must be MAJOR.MINOR.PATCH"),
        }
        end += 1;
    }
    assert!(
        end < bytes.len(),
        "prompt-pack promptVersion is unterminated"
    );
    assert!(
        dots == 2 && digits > 0,
        "prompt-pack promptVersion must be MAJOR.MINOR.PATCH"
    );
    Some((start, end))
}

const fn skip_json_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b'\n' | b'\r') {
        index += 1;
    }
    index
}

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

#[cfg(test)]
mod tests {
    use super::{
        AUTHORING_PROTOCOL_VERSION, LANGUAGE_EDITION, LANGUAGE_VERSION, PROMPT_PACK_METADATA,
        PROMPT_PACK_VERSION, PROPOSAL_SCHEMA_VERSION, REQUEST_SCHEMA_VERSION,
        RESPONSE_SCHEMA_VERSION, prompt_pack_version,
    };

    fn shipped_pack() -> serde_json::Value {
        serde_json::from_str(PROMPT_PACK_METADATA).expect("the shipped prompt pack should be JSON")
    }

    #[test]
    fn prompt_pack_version_is_read_from_the_shipped_pack() {
        let pack = shipped_pack();
        assert_eq!(pack["schema"], 1);
        assert_eq!(
            pack["promptVersion"].as_str(),
            Some(PROMPT_PACK_VERSION),
            "the advertised prompt-pack version must equal ai/prompt-pack.json"
        );
    }

    #[test]
    fn mirrored_authoring_versions_match_the_shipped_pack() {
        let pack = shipped_pack();
        assert_eq!(pack["languageVersion"].as_str(), Some(LANGUAGE_VERSION));
        assert_eq!(pack["edition"].as_str(), Some(LANGUAGE_EDITION));
        for (field, version) in [
            ("authoringProtocolVersion", AUTHORING_PROTOCOL_VERSION),
            ("authoringRequestSchema", REQUEST_SCHEMA_VERSION),
            ("authoringResponseSchema", RESPONSE_SCHEMA_VERSION),
            ("authoringProposalSchema", PROPOSAL_SCHEMA_VERSION),
        ] {
            assert_eq!(pack[field].as_u64(), Some(u64::from(version)), "{field}");
        }
    }

    #[test]
    fn prompt_pack_version_scan_accepts_only_one_exact_version() {
        assert_eq!(
            prompt_pack_version("{\"schema\":1,\"promptVersion\":\"0.3.0\"}"),
            "0.3.0"
        );
        assert_eq!(
            prompt_pack_version("{\n  \"promptVersion\" \t:\r\n \"10.20.30\"\n}"),
            "10.20.30"
        );
        // A key-like string inside a value, or a longer key, is not a match.
        assert_eq!(
            prompt_pack_version(
                "{\"note\":\"\\\"promptVersion\\\": \\\"9.9.9\\\"\",\"promptVersions\":[1],\"promptVersion\":\"1.2.3\"}"
            ),
            "1.2.3"
        );

        for invalid in [
            "{}",
            "{\"promptVersion\":\"1.2.3\",\"nested\":{\"promptVersion\":\"1.2.3\"}}",
            "{\"promptVersion\":123}",
            "{\"promptVersion\":\"1.2\"}",
            "{\"promptVersion\":\"1.2.3.4\"}",
            "{\"promptVersion\":\"1..3\"}",
            "{\"promptVersion\":\"1.2.\"}",
            "{\"promptVersion\":\"v1.2.3\"}",
            "{\"promptVersion\":\"1.2.3-rc.1\"}",
            "{\"promptVersion\":\"1.2.\\u0033\"}",
            "{\"promptVersion\":\"1.2.3",
        ] {
            assert!(
                std::panic::catch_unwind(|| prompt_pack_version(invalid)).is_err(),
                "{invalid}"
            );
        }
    }
}
