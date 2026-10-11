use http::{HeaderName, HeaderValue, Method};
use serde::{Deserialize, Serialize};

use crate::{RuntimeError, RuntimeLimits};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<HttpHeader>,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpResponse {
    pub status: i64,
    pub headers: Vec<HttpHeader>,
    pub body: String,
}

impl HttpRequest {
    pub(crate) fn validate(&self, limits: RuntimeLimits) -> Result<(), RuntimeError> {
        let method = Method::from_bytes(self.method.as_bytes()).map_err(|_| {
            RuntimeError::guest("K4001", "HTTP request method is not a valid token")
        })?;
        if method == Method::CONNECT {
            return Err(RuntimeError::guest(
                "K4001",
                "HTTP CONNECT is not permitted",
            ));
        }
        validate_path_query(&self.path, &self.query)?;
        validate_headers(&self.headers, limits)?;
        if self.body.len() > limits.request_body_bytes() {
            return Err(RuntimeError::resource(format!(
                "HTTP request body exceeds the {}-byte limit",
                limits.request_body_bytes()
            )));
        }
        Ok(())
    }
}

impl HttpResponse {
    pub(crate) fn validate(&self, limits: RuntimeLimits) -> Result<(), RuntimeError> {
        if !(100..=599).contains(&self.status) {
            return Err(RuntimeError::guest(
                "K4001",
                "HTTP response status must be in 100..=599",
            ));
        }
        validate_headers(&self.headers, limits)?;
        if self.body.len() > limits.response_body_bytes() {
            return Err(RuntimeError::resource(format!(
                "HTTP response body exceeds the {}-byte limit",
                limits.response_body_bytes()
            )));
        }
        Ok(())
    }
}

pub(crate) fn validate_headers(
    headers: &[HttpHeader],
    limits: RuntimeLimits,
) -> Result<(), RuntimeError> {
    if headers.len() > limits.header_count() {
        return Err(RuntimeError::resource(format!(
            "HTTP header count exceeds the {}-header limit",
            limits.header_count()
        )));
    }
    let mut bytes = 0usize;
    for header in headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| RuntimeError::guest("K4001", "HTTP header name is invalid"))?;
        HeaderValue::from_str(&header.value)
            .map_err(|_| RuntimeError::guest("K4001", "HTTP header value is invalid"))?;
        if forbidden_header(&name) {
            return Err(RuntimeError::guest(
                "K4001",
                format!("HTTP header `{name}` is not permitted"),
            ));
        }
        bytes = bytes
            .checked_add(header.name.len())
            .and_then(|value| value.checked_add(header.value.len()))
            .ok_or_else(|| RuntimeError::resource("HTTP header byte count overflowed"))?;
        if bytes > limits.header_bytes() {
            return Err(RuntimeError::resource(format!(
                "HTTP headers exceed the {}-byte limit",
                limits.header_bytes()
            )));
        }
    }
    Ok(())
}

/// Whether a string is a safe origin-form absolute path.
///
/// Shared so every host-owned path - an inbound webhook request path, a guest
/// outbound request path, an AI adapter path, and a search connector path
/// alike - is held to exactly the same rule: absolute, no authority form, no
/// scheme, no backslash, no query or fragment delimiter, no control byte, and
/// no traversal form (see [`has_traversal_form`]).
pub(crate) fn is_origin_form_path(path: &str) -> bool {
    has_origin_form_shape(path) && !has_traversal_form(path)
}

fn has_origin_form_shape(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("://")
        && !path
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'\\' | b'?' | b'#'))
}

/// Whether a path could address a different resource once a server or proxy
/// normalizes or decodes it.
///
/// An exact origin is the only HTTP authority a program holds, but servers
/// commonly resolve dot segments and decode percent escapes before routing, so
/// a path built from untrusted text could otherwise climb out of the prefix the
/// program meant to address. Every form that can change the segment structure
/// is refused:
///
/// - a segment that is exactly `.` or `..`;
/// - a percent-encoded `.`, `/`, or `\` (`%2E`, `%2F`, `%5C`, either case)
///   anywhere, so `%2e%2e` or `.%2e` cannot spell a dot segment and `%2f`
///   cannot split one segment into two;
/// - a `%` that does not begin a well-formed `%HH` escape, because how a server
///   decodes it is implementation-defined and so unknown.
///
/// Every other well-formed escape, such as `%20` or `%25`, stays allowed. The
/// outbound transfer then sends the path byte-for-byte as validated, with
/// libcurl's own dot-segment normalization disabled.
fn has_traversal_form(path: &str) -> bool {
    if path.split('/').any(|segment| matches!(segment, "." | "..")) {
        return true;
    }
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            continue;
        }
        let high = bytes.next().and_then(hex_digit);
        let low = bytes.next().and_then(hex_digit);
        let (Some(high), Some(low)) = (high, low) else {
            return true;
        };
        if matches!((high << 4) | low, b'.' | b'/' | b'\\') {
            return true;
        }
    }
    false
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn validate_path_query(path: &str, query: &str) -> Result<(), RuntimeError> {
    if !has_origin_form_shape(path) {
        return Err(RuntimeError::guest(
            "K4001",
            "HTTP request path must be a safe origin-form absolute path",
        ));
    }
    // The path is never echoed: it may carry data a diagnostic must not
    // disclose.
    if has_traversal_form(path) {
        return Err(RuntimeError::guest(
            "K4001",
            "HTTP request path must not contain a `.` or `..` segment, a percent-encoded `.`, `/`, or `\\`, or a malformed percent escape",
        ));
    }
    if query.starts_with('?')
        || query
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'#' | b'\\'))
    {
        return Err(RuntimeError::guest(
            "K4001",
            "HTTP request query must not contain a leading `?`, fragment, control byte, or backslash",
        ));
    }
    Ok(())
}

fn forbidden_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization"
            | "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_path(path: &str) -> HttpRequest {
        HttpRequest {
            method: "GET".to_owned(),
            path: path.to_owned(),
            query: String::new(),
            headers: Vec::new(),
            body: String::new(),
        }
    }

    /// Paths whose segment structure a server could change by normalizing or
    /// decoding them.
    const TRAVERSAL_PATHS: [&str; 26] = [
        // Dot segments, wherever they sit.
        "/.",
        "/..",
        "/./",
        "/../",
        "/a/.",
        "/a/..",
        "/a/./b",
        "/a/../b",
        "/a/b/../../c",
        // Percent-encoded `.` in either case, alone or completing a dot segment.
        "/%2e",
        "/%2E",
        "/a/%2e%2e/b",
        "/a/.%2e/b",
        "/a/%2E./b",
        "/a/file%2ejson",
        // Percent-encoded `/` in either case.
        "/a%2fb",
        "/a%2Fb",
        "/v1/repos/owner%2F..%2Fadmin",
        // Percent-encoded `\` in either case.
        "/a%5cb",
        "/a%5C..%5Cb",
        // Malformed escapes, whose decoding is implementation-defined.
        "/%",
        "/a%2",
        "/a%zz",
        "/a%%2e",
        "/a%2g",
        "/100%",
    ];

    #[test]
    fn every_traversal_form_is_refused_with_k4001() {
        for path in TRAVERSAL_PATHS {
            assert!(!is_origin_form_path(path), "`{path}` must be refused");
            let error = request_with_path(path)
                .validate(RuntimeLimits::default())
                .expect_err("a traversal form must fail validation");
            assert_eq!(error.code(), "K4001", "`{path}`");
            assert_eq!(
                error.message(),
                "HTTP request path must not contain a `.` or `..` segment, a percent-encoded `.`, `/`, or `\\`, or a malformed percent escape",
                "`{path}`"
            );
        }
    }

    #[test]
    fn a_refused_path_is_never_echoed_in_the_diagnostic() {
        let error = request_with_path("/private-token-123/../admin")
            .validate(RuntimeLimits::default())
            .expect_err("a dot segment must fail validation");
        assert!(!error.message().contains("private-token-123"));
        assert!(!error.message().contains("admin"));
    }

    #[test]
    fn ordinary_paths_and_other_escapes_stay_allowed() {
        for path in [
            "/",
            "/incoming",
            "/repos/owner/name/issues/1",
            "/trailing/",
            "/a//b",
            // Dots that are not a whole segment.
            "/.well-known/webhook",
            "/v1.2/items",
            "/file.json",
            "/a..b",
            "/...",
            "/a/.../b",
            "/..a",
            "/a/b.",
            // Well-formed escapes of anything but `.`, `/`, or `\`.
            "/a%20b",
            "/search/caf%C3%A9",
            "/100%25",
            "/%2525",
            "/a%2Db",
            "/a%3a%3Ab",
            "/%7Euser",
        ] {
            assert!(is_origin_form_path(path), "`{path}` must be allowed");
            request_with_path(path)
                .validate(RuntimeLimits::default())
                .unwrap_or_else(|error| panic!("`{path}` must validate: {error}"));
        }
    }

    #[test]
    fn the_query_keeps_its_own_rule() {
        // Encoded separators and dots are ordinary data in a query string.
        let request = HttpRequest {
            query: "next=%2Fadmin%2F..&path=../a&q=a%2eb".to_owned(),
            ..request_with_path("/search")
        };
        request
            .validate(RuntimeLimits::default())
            .expect("a query may carry encoded separators");
    }

    #[test]
    fn the_shape_rule_is_reported_before_the_traversal_rule() {
        for path in ["relative/../path", "//evil.example/..", "/a\\..\\b"] {
            let error = request_with_path(path)
                .validate(RuntimeLimits::default())
                .expect_err("an unsafe shape must fail validation");
            assert_eq!(error.code(), "K4001");
            assert_eq!(
                error.message(),
                "HTTP request path must be a safe origin-form absolute path",
                "`{path}`"
            );
        }
    }
}
