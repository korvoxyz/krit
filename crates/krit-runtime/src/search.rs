//! Provider-neutral search and vector connectors.
//!
//! A connector is a *named host-owned binding*, never a branded SDK. Guest code
//! names an index; it never sees an endpoint, a path, a credential, a model, a
//! provider identity, or a raw handle. Results are untrusted external input:
//! they are bounded, validated against a fixed schema, and returned as
//! deterministic JSON text that source code must inspect explicitly. Nothing
//! here executes a result.

use serde::{Deserialize, Serialize};

use crate::{HttpHeader, HttpRequest, HttpResponse, RuntimeError};

/// Hard bound on configured connectors.
pub const MAX_SEARCH_CONNECTORS: usize = 8;
/// Hard bound on one query string.
pub const MAX_QUERY_BYTES: usize = 4 * 1024;
/// Hard bound on the encoded vector a caller may submit.
pub const MAX_VECTOR_BYTES: usize = 64 * 1024;
/// Hard bound on vector dimensions.
pub const MAX_VECTOR_DIMENSIONS: usize = 4096;
/// Hard bound on results one call may request.
pub const MAX_SEARCH_RESULTS: usize = 100;
/// Hard bound on one encoded result document.
pub const MAX_SEARCH_RESPONSE_BYTES: usize = 256 * 1024;
/// Hard bound on one result identifier.
const MAX_RESULT_ID_BYTES: usize = 512;
/// Hard bound on one result snippet.
const MAX_RESULT_SNIPPET_BYTES: usize = 8 * 1024;
/// Hard bound on a connector path.
const MAX_CONNECTOR_PATH_BYTES: usize = 256;

/// Operation a connector supports.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SearchKind {
    /// Text query, reached through `search_query`.
    Query,
    /// Vector similarity, reached through `vector_search`.
    Vector,
}

impl SearchKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Vector => "vector",
        }
    }

    pub const fn capability(self) -> &'static str {
        match self {
            Self::Query => "search.query",
            Self::Vector => "search.vector",
        }
    }
}

/// Transport a connector uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchTransport {
    /// Strict generic JSON over HTTPS. No branded protocol, no SDK.
    HttpJson(HttpJsonConnectorConfig),
    /// Deterministic in-process connector with a fixed document set.
    ///
    /// Exists so that reference examples and tests can prove cache and fallback
    /// behaviour without a network. It performs no I/O.
    Local(LocalConnectorConfig),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpJsonConnectorConfig {
    pub origin: String,
    pub path: String,
    pub secret: Option<String>,
    pub max_response_bytes: usize,
    pub timeout: std::time::Duration,
}

/// One deterministic document in a local connector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalDocument {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalConnectorConfig {
    pub documents: Vec<LocalDocument>,
}

/// One configured connector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchConnectorConfig {
    pub kind: SearchKind,
    pub index: String,
    pub transport: SearchTransport,
    pub max_results: usize,
    /// Required only for a vector connector.
    pub dimensions: Option<usize>,
}

impl SearchConnectorConfig {
    /// Validates one connector definition with no I/O.
    ///
    /// Strict by default: a connector must use HTTPS. `allow_plaintext` exists
    /// only so a loopback test policy can exercise the real transport, and is
    /// never set by a host configuration.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        self.validate_with_plaintext_allowance(false)
    }

    pub(crate) fn validate_with_plaintext_allowance(
        &self,
        allow_plaintext: bool,
    ) -> Result<(), RuntimeError> {
        if self.index.is_empty() || !krit_capability::is_valid_resource_name(&self.index) {
            return Err(RuntimeError::search(
                "search connector index must use the canonical resource grammar",
            ));
        }
        if self.max_results == 0 || self.max_results > MAX_SEARCH_RESULTS {
            return Err(RuntimeError::search(
                "search connector result bound is outside the Phase 7 bounds",
            ));
        }
        match self.kind {
            SearchKind::Query => {
                if self.dimensions.is_some() {
                    return Err(RuntimeError::search(
                        "a text search connector must not declare vector dimensions",
                    ));
                }
            }
            SearchKind::Vector => {
                let dimensions = self.dimensions.ok_or_else(|| {
                    RuntimeError::search("a vector connector must declare its dimensions")
                })?;
                if dimensions == 0 || dimensions > MAX_VECTOR_DIMENSIONS {
                    return Err(RuntimeError::search(
                        "vector connector dimensions are outside the Phase 7 bounds",
                    ));
                }
            }
        }
        match &self.transport {
            SearchTransport::HttpJson(config) => {
                if config.max_response_bytes == 0
                    || config.max_response_bytes > MAX_SEARCH_RESPONSE_BYTES
                {
                    return Err(RuntimeError::search(
                        "search connector response bound is outside the Phase 7 bounds",
                    ));
                }
                if config.timeout.is_zero() {
                    return Err(RuntimeError::search(
                        "search connector timeout must be positive",
                    ));
                }
                let origin =
                    krit_capability::HttpOrigin::parse_exact(&config.origin).map_err(|_| {
                        RuntimeError::search("search connector origin is not an exact origin")
                    })?;
                // A connector carries user text and, when configured, a
                // credential. Plaintext transport is refused; only an explicit
                // loopback test policy relaxes this, exactly like a bearer.
                if origin.scheme() != krit_capability::HttpScheme::Https && !allow_plaintext {
                    return Err(RuntimeError::search(
                        "search connector origin must use HTTPS",
                    ));
                }
                // The path is held to the same origin-form rule as every other
                // host-owned path, so it can never smuggle an authority, a
                // scheme, a query, or a fragment.
                if config.path.len() > MAX_CONNECTOR_PATH_BYTES
                    || !crate::webhook::is_origin_form_path(&config.path)
                    || !config.path.is_ascii()
                {
                    return Err(RuntimeError::search(
                        "search connector path must be a bounded safe origin-form absolute path",
                    ));
                }
            }
            SearchTransport::Local(config) => {
                if config.documents.len() > MAX_SEARCH_RESULTS {
                    return Err(RuntimeError::search(
                        "local search connector holds too many documents",
                    ));
                }
                for document in &config.documents {
                    if document.id.is_empty()
                        || document.id.len() > MAX_RESULT_ID_BYTES
                        || document.text.len() > MAX_RESULT_SNIPPET_BYTES
                    {
                        return Err(RuntimeError::search(
                            "local search connector document is outside the Phase 7 bounds",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn secret_name(&self) -> Option<&str> {
        match &self.transport {
            SearchTransport::HttpJson(config) => config.secret.as_deref(),
            SearchTransport::Local(_) => None,
        }
    }
}

/// Wire request for a text query. No credential is ever placed in the body.
#[derive(Serialize)]
struct QueryRequestBody<'a> {
    query: &'a str,
    limit: u32,
}

/// Wire request for a vector search.
#[derive(Serialize)]
struct VectorRequestBody<'a> {
    vector: &'a [f64],
    limit: u32,
}

/// Strict provider response schema. Unknown fields are refused so a provider
/// cannot smuggle extra data into guest-visible output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorResponse {
    results: Vec<ConnectorResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorResult {
    id: String,
    score: f64,
    snippet: String,
}

/// One validated result, ready for deterministic encoding.
struct ValidatedResult {
    id: String,
    score: f64,
    snippet: String,
}

/// Builds the bounded HTTP request for a text query.
pub(crate) fn build_query_request(
    config: &HttpJsonConnectorConfig,
    query: &str,
    limit: usize,
) -> Result<HttpRequest, String> {
    let body = serde_json::to_string(&QueryRequestBody {
        query,
        limit: u32::try_from(limit).unwrap_or(u32::MAX),
    })
    .map_err(|_| "search connector could not encode its request".to_owned())?;
    Ok(request_for(config, body))
}

/// Builds the bounded HTTP request for a vector search.
pub(crate) fn build_vector_request(
    config: &HttpJsonConnectorConfig,
    vector: &[f64],
    limit: usize,
) -> Result<HttpRequest, String> {
    let body = serde_json::to_string(&VectorRequestBody {
        vector,
        limit: u32::try_from(limit).unwrap_or(u32::MAX),
    })
    .map_err(|_| "search connector could not encode its request".to_owned())?;
    Ok(request_for(config, body))
}

fn request_for(config: &HttpJsonConnectorConfig, body: String) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: config.path.clone(),
        query: String::new(),
        headers: vec![HttpHeader {
            name: "content-type".to_owned(),
            value: "application/json".to_owned(),
        }],
        body,
    }
}

/// Parses and re-encodes a provider response deterministically.
///
/// The provider's bytes are never handed to the guest directly. Every field is
/// validated and bounded, then re-encoded in a fixed shape, so a hostile or
/// buggy provider cannot control the structure guest code parses.
pub(crate) fn parse_response(
    response: HttpResponse,
    max_response_bytes: usize,
    max_results: usize,
) -> Result<String, String> {
    if !(200..=299).contains(&response.status) {
        return Err(format!(
            "search connector returned HTTP status {}",
            response.status
        ));
    }
    if response.body.len() > max_response_bytes {
        return Err("search connector response exceeded its configured size limit".to_owned());
    }
    let parsed: ConnectorResponse = serde_json::from_str(&response.body).map_err(|_| {
        "search connector response was not valid strict connector output".to_owned()
    })?;
    if parsed.results.len() > max_results {
        return Err("search connector returned more results than its configured bound".to_owned());
    }
    let mut validated = Vec::with_capacity(parsed.results.len());
    for result in parsed.results {
        validated.push(validate_result(result)?);
    }
    encode_results(&validated, max_response_bytes)
}

fn validate_result(result: ConnectorResult) -> Result<ValidatedResult, String> {
    if result.id.is_empty() || result.id.len() > MAX_RESULT_ID_BYTES {
        return Err("search connector returned an out-of-range result identifier".to_owned());
    }
    if result.snippet.len() > MAX_RESULT_SNIPPET_BYTES {
        return Err("search connector returned an oversized result snippet".to_owned());
    }
    if !result.score.is_finite() {
        return Err("search connector returned a non-finite score".to_owned());
    }
    Ok(ValidatedResult {
        id: result.id,
        score: result.score,
        snippet: result.snippet,
    })
}

/// Encodes results in the fixed guest-visible shape.
///
/// ```json
/// {"results":[{"id":"a","score":0.5,"snippet":"text"}]}
/// ```
fn encode_results(results: &[ValidatedResult], max_bytes: usize) -> Result<String, String> {
    let budget = max_bytes.min(MAX_SEARCH_RESPONSE_BYTES);
    let mut encoded = String::from("{\"results\":[");
    for (index, result) in results.iter().enumerate() {
        if index > 0 {
            encoded.push(',');
        }
        encoded.push_str("{\"id\":");
        push_json_string(&mut encoded, &result.id);
        encoded.push_str(",\"score\":");
        if !result.score.is_finite() {
            // Defence in depth: a non-finite score has no JSON spelling, so the
            // whole result set is refused rather than emitting invalid output.
            return Err("search result score is not finite".to_owned());
        }
        encoded.push_str(&format_score(result.score));
        encoded.push_str(",\"snippet\":");
        push_json_string(&mut encoded, &result.snippet);
        encoded.push('}');
        if encoded.len() + 2 > budget {
            return Err("search results exceeded the configured byte bound".to_owned());
        }
    }
    encoded.push_str("]}");
    if encoded.len() > budget {
        return Err("search results exceeded the configured byte bound".to_owned());
    }
    Ok(encoded)
}

/// Renders a score with a fixed precision so identical inputs always produce
/// identical bytes across platforms.
fn format_score(score: f64) -> String {
    debug_assert!(score.is_finite(), "only finite scores are ever encoded");
    let rendered = format!("{score:.6}");
    if rendered == "-0.000000" {
        return "0.000000".to_owned();
    }
    rendered
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

/// Parses and bounds a guest-supplied vector.
///
/// Krit has no float type in protocol 1, so a vector arrives as bounded JSON
/// text. The host validates it strictly: a flat array of finite numbers whose
/// length matches the connector's declared dimensions.
pub(crate) fn parse_vector(encoded: &str, dimensions: usize) -> Result<Vec<f64>, String> {
    if encoded.len() > MAX_VECTOR_BYTES {
        return Err("vector input exceeded its configured byte bound".to_owned());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(encoded).map_err(|_| "vector input was not valid JSON".to_owned())?;
    let serde_json::Value::Array(values) = parsed else {
        return Err("vector input must be a JSON array of numbers".to_owned());
    };
    if values.len() != dimensions {
        return Err(format!(
            "vector input has {} dimensions but the connector declares {dimensions}",
            values.len()
        ));
    }
    let mut vector = Vec::with_capacity(values.len());
    for value in values {
        let serde_json::Value::Number(number) = value else {
            return Err("vector input must contain only numbers".to_owned());
        };
        let number = number
            .as_f64()
            .ok_or_else(|| "vector input contains an unrepresentable number".to_owned())?;
        if !number.is_finite() {
            return Err("vector input must contain only finite numbers".to_owned());
        }
        vector.push(number);
    }
    Ok(vector)
}

/// Runs a deterministic local text query.
///
/// Matching is a simple case-insensitive substring test and the score is a
/// fixed function of the match position, so the same inputs always produce the
/// same bytes. This is a test and reference fixture, not a search engine.
pub(crate) fn local_query(
    config: &LocalConnectorConfig,
    query: &str,
    limit: usize,
) -> Result<String, String> {
    let needle = query.to_lowercase();
    let mut matches = Vec::new();
    for document in &config.documents {
        if needle.is_empty() {
            continue;
        }
        let Some(position) = document.text.to_lowercase().find(&needle) else {
            continue;
        };
        matches.push(ValidatedResult {
            id: document.id.clone(),
            score: 1.0 / (1.0 + position as f64),
            snippet: document.text.clone(),
        });
    }
    // Deterministic ordering: score descending, then identifier ascending.
    matches.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    matches.truncate(limit);
    encode_results(&matches, MAX_SEARCH_RESPONSE_BYTES)
}

/// Runs a deterministic local vector search.
///
/// The score is the cosine similarity between the caller's vector and a vector
/// derived from each document identifier, which is stable and needs no model.
pub(crate) fn local_vector(
    config: &LocalConnectorConfig,
    vector: &[f64],
    limit: usize,
) -> Result<String, String> {
    let mut matches = Vec::new();
    for document in &config.documents {
        let document_vector = derived_vector(&document.id, vector.len());
        let score = cosine_similarity(vector, &document_vector)?;
        matches.push(ValidatedResult {
            id: document.id.clone(),
            score,
            snippet: document.text.clone(),
        });
    }
    matches.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    matches.truncate(limit);
    encode_results(&matches, MAX_SEARCH_RESPONSE_BYTES)
}

/// Derives a stable unit-ish vector from an identifier without randomness.
fn derived_vector(id: &str, dimensions: usize) -> Vec<f64> {
    let mut vector = Vec::with_capacity(dimensions);
    let bytes = id.as_bytes();
    for index in 0..dimensions {
        let byte = bytes.get(index % bytes.len().max(1)).copied().unwrap_or(0);
        vector.push(f64::from(byte) / 255.0);
    }
    vector
}

/// Cosine similarity that cannot overflow for any finite input.
///
/// A naive implementation squares each component, so a legitimate finite vector
/// containing values near `f64::MAX` overflows to infinity and yields `NaN`,
/// which is not representable in JSON. Both vectors are therefore scaled by
/// their largest magnitude first, which changes no ratio and keeps every
/// intermediate inside `[-n, n]`. A non-finite intermediate or result is still
/// reported as an error rather than encoded.
fn cosine_similarity(left: &[f64], right: &[f64]) -> Result<f64, String> {
    let scale = |vector: &[f64]| -> f64 {
        vector
            .iter()
            .map(|value| value.abs())
            .fold(0.0f64, f64::max)
    };
    let left_scale = scale(left);
    let right_scale = scale(right);
    if left_scale == 0.0 || right_scale == 0.0 {
        // A zero vector has no direction; similarity is defined as zero.
        return Ok(0.0);
    }
    let mut dot = 0.0f64;
    let mut left_norm = 0.0f64;
    let mut right_norm = 0.0f64;
    for (a, b) in left.iter().zip(right) {
        let a = a / left_scale;
        let b = b / right_scale;
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    let denominator = left_norm.sqrt() * right_norm.sqrt();
    if !denominator.is_finite() || denominator == 0.0 {
        return Err("vector similarity could not be computed".to_owned());
    }
    let score = dot / denominator;
    if !score.is_finite() {
        return Err("vector similarity produced a non-finite score".to_owned());
    }
    // Rounding can push a unit-length ratio a hair outside the valid range.
    Ok(score.clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_are_strictly_bounded_and_dimension_checked() {
        assert_eq!(parse_vector("[1.0,2.0,3.0]", 3).unwrap().len(), 3);
        assert!(parse_vector("[1.0,2.0]", 3).is_err());
        assert!(parse_vector("[1.0,2.0,3.0,4.0]", 3).is_err());
        assert!(parse_vector("{\"a\":1}", 3).is_err());
        assert!(parse_vector("[\"a\",\"b\",\"c\"]", 3).is_err());
        assert!(parse_vector("[1,2,null]", 3).is_err());
        assert!(parse_vector("not json", 3).is_err());
        assert!(parse_vector(&"1,".repeat(100_000), 3).is_err());
    }

    #[test]
    fn scores_render_deterministically() {
        assert_eq!(format_score(0.5), "0.500000");
        assert_eq!(format_score(-0.0), "0.000000");
        assert_eq!(format_score(1.0 / 3.0), "0.333333");
    }

    #[test]
    fn local_query_results_are_deterministic_and_bounded() {
        let config = LocalConnectorConfig {
            documents: vec![
                LocalDocument {
                    id: "b".to_owned(),
                    text: "the quick brown fox".to_owned(),
                },
                LocalDocument {
                    id: "a".to_owned(),
                    text: "quick start guide".to_owned(),
                },
                LocalDocument {
                    id: "c".to_owned(),
                    text: "unrelated".to_owned(),
                },
            ],
        };

        let first = local_query(&config, "quick", 10).unwrap();
        let second = local_query(&config, "quick", 10).unwrap();

        assert_eq!(first, second);
        // `a` matches at position 0 and outranks `b`, which matches at 4.
        assert!(first.starts_with("{\"results\":[{\"id\":\"a\""));
        assert!(!first.contains("unrelated"));
        assert_eq!(
            local_query(&config, "quick", 1)
                .unwrap()
                .matches("\"id\"")
                .count(),
            1
        );
        assert_eq!(local_query(&config, "", 10).unwrap(), "{\"results\":[]}");
    }

    #[test]
    fn provider_responses_are_strictly_validated() {
        let good = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: "{\"results\":[{\"id\":\"a\",\"score\":0.5,\"snippet\":\"text\"}]}".to_owned(),
        };
        assert_eq!(
            parse_response(good, 4096, 10).unwrap(),
            "{\"results\":[{\"id\":\"a\",\"score\":0.500000,\"snippet\":\"text\"}]}"
        );

        for body in [
            "{\"results\":[{\"id\":\"a\",\"score\":0.5,\"snippet\":\"t\",\"extra\":1}]}",
            "{\"results\":[{\"id\":\"\",\"score\":0.5,\"snippet\":\"t\"}]}",
            "{\"results\":[{\"id\":\"a\",\"score\":null,\"snippet\":\"t\"}]}",
            "{\"unexpected\":[]}",
            "not json",
            "{\"results\":[{\"id\":\"a\",\"score\":0.5}]}",
        ] {
            let response = HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: body.to_owned(),
            };
            assert!(
                parse_response(response, 4096, 10).is_err(),
                "body should be rejected: {body}"
            );
        }
    }

    #[test]
    fn untrusted_result_text_is_escaped_not_executed() {
        let response = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: "{\"results\":[{\"id\":\"a\",\"score\":1.0,\"snippet\":\"</script>\\\"x\\n\"}]}"
                .to_owned(),
        };

        let encoded = parse_response(response, 4096, 10).unwrap();

        assert!(encoded.contains("\\\"x\\n"));
        assert!(serde_json::from_str::<serde_json::Value>(&encoded).is_ok());
    }

    fn connector(origin: &str, path: &str) -> SearchConnectorConfig {
        SearchConnectorConfig {
            kind: SearchKind::Query,
            index: "docs".to_owned(),
            transport: SearchTransport::HttpJson(HttpJsonConnectorConfig {
                origin: origin.to_owned(),
                path: path.to_owned(),
                secret: None,
                max_response_bytes: 4096,
                timeout: std::time::Duration::from_secs(1),
            }),
            max_results: 5,
            dimensions: None,
        }
    }

    #[test]
    fn a_connector_must_use_https() {
        assert!(
            connector("https://search.example", "/query")
                .validate()
                .is_ok()
        );
        for origin in [
            "http://search.example",
            "http://127.0.0.1:8080",
            "http://search.example:443",
        ] {
            let error = connector(origin, "/query")
                .validate()
                .expect_err("plaintext must be refused");
            assert!(error.message().contains("HTTPS"), "{origin}: {error}");
        }
        // Only an explicit loopback test allowance relaxes the rule.
        assert!(
            connector("http://127.0.0.1:8080", "/query")
                .validate_with_plaintext_allowance(true)
                .is_ok()
        );
    }

    #[test]
    fn a_connector_path_must_be_a_safe_origin_form_path() {
        for path in ["/query", "/v1/search", "/"] {
            assert!(
                connector("https://search.example", path).validate().is_ok(),
                "path `{path}` should be accepted"
            );
        }
        for path in [
            "query",                      // not absolute
            "//evil.example/query",       // authority form
            "/query?inject=1",            // query delimiter
            "/query#fragment",            // fragment
            "/que\\ry",                   // backslash
            "https://evil.example/query", // absolute URL
            "/query\u{7f}",               // control byte
            "/query\u{00e9}",             // non-ASCII
            "/\u{0}",                     // NUL
        ] {
            assert!(
                connector("https://search.example", path)
                    .validate()
                    .is_err(),
                "path `{path}` must be refused"
            );
        }
        // The path is bounded.
        assert!(
            connector("https://search.example", &format!("/{}", "a".repeat(300)))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn extreme_finite_vectors_never_overflow_or_encode_nan() {
        let config = LocalConnectorConfig {
            documents: vec![
                LocalDocument {
                    id: "a".to_owned(),
                    text: "alpha".to_owned(),
                },
                LocalDocument {
                    id: "b".to_owned(),
                    text: "beta".to_owned(),
                },
            ],
        };

        for vector in [
            vec![1e308, 1e308, 1e308],
            vec![f64::MAX, f64::MAX, f64::MAX],
            vec![-f64::MAX, 1e308, f64::MIN_POSITIVE],
            vec![f64::MIN_POSITIVE, f64::MIN_POSITIVE, f64::MIN_POSITIVE],
            vec![1.0, 0.0, -1.0],
        ] {
            let encoded = local_vector(&config, &vector, 5)
                .unwrap_or_else(|error| panic!("vector {vector:?} should encode: {error}"));
            assert!(
                !encoded.contains("NaN") && !encoded.contains("inf"),
                "vector {vector:?} encoded a non-finite score: {encoded}"
            );
            let parsed: serde_json::Value = serde_json::from_str(&encoded)
                .unwrap_or_else(|error| panic!("vector {vector:?} produced invalid JSON: {error}"));
            for result in parsed["results"].as_array().expect("results array") {
                let score = result["score"].as_f64().expect("score should be a number");
                assert!(
                    (-1.0..=1.0).contains(&score),
                    "score {score} is outside the valid range"
                );
            }
        }
    }

    #[test]
    fn a_zero_vector_scores_zero_deterministically() {
        let config = LocalConnectorConfig {
            documents: vec![LocalDocument {
                id: "a".to_owned(),
                text: "alpha".to_owned(),
            }],
        };

        let encoded =
            local_vector(&config, &[0.0, 0.0, 0.0], 5).expect("zero vector should encode");

        assert_eq!(
            encoded,
            "{\"results\":[{\"id\":\"a\",\"score\":0.000000,\"snippet\":\"alpha\"}]}"
        );
        assert_eq!(
            local_vector(&config, &[0.0, 0.0, 0.0], 5).unwrap(),
            encoded,
            "the zero vector must be deterministic"
        );
    }

    #[test]
    fn vector_dimension_boundaries_are_enforced() {
        // A zero-dimension connector is refused at configuration time, so
        // `parse_vector` is never reached with zero dimensions.
        let mut zero = connector("https://search.example", "/query");
        zero.kind = SearchKind::Vector;
        zero.dimensions = Some(0);
        assert!(zero.validate().is_err());
        let mut wide_connector = connector("https://search.example", "/query");
        wide_connector.kind = SearchKind::Vector;
        wide_connector.dimensions = Some(MAX_VECTOR_DIMENSIONS + 1);
        assert!(wide_connector.validate().is_err());

        assert_eq!(parse_vector("[1.0]", 1).unwrap(), vec![1.0]);
        let wide = format!("[{}]", vec!["1.0"; MAX_VECTOR_DIMENSIONS].join(","));
        assert_eq!(
            parse_vector(&wide, MAX_VECTOR_DIMENSIONS).unwrap().len(),
            MAX_VECTOR_DIMENSIONS
        );
        assert!(
            parse_vector("[1e400]", 1).is_err(),
            "infinity must be refused"
        );
        assert!(parse_vector("[-1e400]", 1).is_err());
    }

    #[test]
    fn oversized_provider_output_is_refused() {
        let response = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: format!(
                "{{\"results\":[{{\"id\":\"a\",\"score\":1.0,\"snippet\":\"{}\"}}]}}",
                "x".repeat(9000)
            ),
        };
        assert!(parse_response(response, 4096, 10).is_err());
    }
}
