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
/// scheme, no space, no backslash, no query or fragment delimiter, no control
/// byte, and no traversal form (see [`has_traversal_form`]).
pub(crate) fn is_origin_form_path(path: &str) -> bool {
    has_origin_form_shape(path) && !has_traversal_form(path)
}

fn has_origin_form_shape(path: &str) -> bool {
    // A raw space can never be sent: HTTP/1.1 forbids it in a request target
    // and libcurl refuses it, but only after approval and DNS resolution.
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("://")
        && !path
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b' ' | b'\\' | b'?' | b'#'))
}

/// Whether a path could address a different resource once a server or proxy
/// normalizes or decodes it.
///
/// An exact origin is the only HTTP authority a program holds, but servers
/// commonly resolve dot segments, strip `;` path parameters, and percent-decode
/// (sometimes twice, behind a proxy) before routing, so a path built from
/// untrusted text could otherwise climb out of the prefix the program meant to
/// address. Each segment is decoded once, and the path is refused when:
///
/// - a segment is a dot segment: once decoded, the part before its first `;`,
///   `?`, or `#`, with ASCII spaces trimmed from both ends, is exactly `.` or
///   `..` (`..`, `..;`, `.;x=1`, `..%3B`, `..%3F`, `..%23`, `..%20`);
/// - an escape decodes to `.`, `/`, `\`, or an ASCII control byte (`%2E`,
///   `%2F`, `%5C`, `%00`-`%1F`, `%7F`, either case), so `%2e%2e` or `.%2e`
///   cannot spell a dot segment, `%2f` cannot split one segment into two, and
///   `%00` or `%0A` cannot truncate a segment or be trimmed from one;
/// - a decoded segment still holds a `%` followed by a hex digit, `u`, or `U`
///   (`%252e`, `%2541`, `%25%32%65`), so a second decoding pass cannot produce
///   another escaped byte;
/// - escapes spell overlong UTF-8 (`%C0%AE`, `%C1%DC`, `%E0%80%AE`), which a
///   lenient decoder reads as `.`, `/`, `\`, or another shorter character (see
///   [`holds_overlong_utf8`]);
/// - a `%` does not begin a well-formed `%HH` escape, because how a server
///   decodes it is implementation-defined and so unknown.
///
/// Every other form stays allowed, such as dots inside a segment, `%20` beside
/// other text, `%3F` or `%23` after a segment's name, or a `%25` that no hex
/// digit, `u`, or `U` follows. Transforms specific to one server, such as
/// Unicode compatibility normalization, are not modelled
/// (WEBHOOK-CONTRACTS.md, "Request paths and queries"). The outbound transfer
/// never resolves or decodes the path: libcurl's own dot-segment normalization
/// is disabled.
fn has_traversal_form(path: &str) -> bool {
    // No accepted escape decodes to `/`, so the raw segments are exactly the
    // decoded ones and each can be checked on its own.
    let mut decoded = Vec::new();
    path.split('/').any(|segment| {
        decoded.clear();
        !decode_segment(segment, &mut decoded)
            || is_dot_segment(&decoded)
            || holds_escape(&decoded)
            || holds_overlong_utf8(&decoded)
    })
}

/// Percent-decodes `segment` once into `decoded`, or returns `false` for a `%`
/// that does not begin a well-formed `%HH` escape or for an escape of `.`, `/`,
/// `\`, or an ASCII control byte.
fn decode_segment(segment: &str, decoded: &mut Vec<u8>) -> bool {
    let mut bytes = segment.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            decoded.push(byte);
            continue;
        }
        let high = bytes.next().and_then(hex_digit);
        let low = bytes.next().and_then(hex_digit);
        let (Some(high), Some(low)) = (high, low) else {
            return false;
        };
        let byte = (high << 4) | low;
        // A raw control byte breaks the shape rule. Its escape is refused as
        // well: servers truncate a decoded segment at NUL, trim whitespace such
        // as `%09` or `%0A` from it, and can split a request line or header
        // they rebuild from a decoded CR or LF.
        if matches!(byte, b'.' | b'/' | b'\\') || byte.is_ascii_control() {
            return false;
        }
        decoded.push(byte);
    }
    true
}

/// Whether a decoded segment names `.` or `..` once a server cuts it short
/// and trims it.
///
/// Tomcat, Jetty, and Spring strip `;` path parameters before resolving a
/// segment. A decoded `?` or `#` ends the segment in the same way for a server
/// or proxy that decodes the target once and then parses it again, so
/// `..%3F` reads as `..` followed by a query. Servers and frameworks that trim
/// a segment (Java `String.trim`, for one) read `..%20` as `..`. Control bytes
/// never reach this check: [`decode_segment`] refuses their escapes.
fn is_dot_segment(decoded: &[u8]) -> bool {
    let name = decoded
        .split(|byte| matches!(byte, b';' | b'?' | b'#'))
        .next()
        .unwrap_or_default();
    matches!(name.trim_ascii(), b"." | b"..")
}

/// Whether a decoded segment would hold another escape (`%HH`, or IIS-style
/// `%uHHHH`) for a server that decodes it a second time.
fn holds_escape(decoded: &[u8]) -> bool {
    decoded.windows(2).any(|pair| {
        pair[0] == b'%' && (hex_digit(pair[1]).is_some() || matches!(pair[1], b'u' | b'U'))
    })
}

/// Whether decoded bytes spell an overlong UTF-8 sequence, which a lenient
/// decoder reads as a shorter character such as `.` (`C0 AE`) or `/` (`C0 AF`).
///
/// A lenient decoder often takes the low six bits of each byte after a lead
/// byte without checking that it is a continuation byte (the IIS 4/5 class),
/// so `C1 DC` reads as `\` and `E0 40 6E` as `.`. Each check therefore reads
/// the byte after a lead the same way. `C0` and `C1` begin only overlong forms
/// and never occur in UTF-8, so either is refused whatever follows, including
/// the `/` after a segment's last byte. After any other lead, that `/` (`2F`)
/// makes no sequence overlong, so each segment can be checked on its own.
///
/// Valid UTF-8 never trips a check, so raw text (the path is a Rust string)
/// can only trip one beside an escaped byte.
fn holds_overlong_utf8(decoded: &[u8]) -> bool {
    decoded.iter().any(|byte| matches!(byte, 0xC0 | 0xC1))
        || decoded.windows(2).any(|pair| {
            let bits = pair[1] & 0x3F;
            match pair[0] {
                0xE0 => bits < 0x20,
                0xF0 => bits < 0x10,
                // Five- and six-byte forms, which only pre-RFC 3629 decoders
                // accept.
                0xF8 => bits < 0x08,
                0xFC => bits < 0x04,
                _ => false,
            }
        })
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

const TRAVERSAL_MESSAGE: &str = "HTTP request path must not contain a dot segment, a percent-encoded `.`, `/`, `\\`, or control byte, a double-encoded or overlong UTF-8 escape, or a malformed percent escape";

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
        return Err(RuntimeError::guest("K4001", TRAVERSAL_MESSAGE));
    }
    // A query keeps its own rule: encoded separators in it are ordinary data,
    // but a raw space can no more be sent there than in the path.
    if query.starts_with('?')
        || query
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b' ' | b'#' | b'\\'))
    {
        return Err(RuntimeError::guest(
            "K4001",
            "HTTP request query must not contain a leading `?`, fragment, control byte, space, or backslash",
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
    const TRAVERSAL_PATHS: &[&str] = &[
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
        // Dot segments behind `;` path parameters, which Tomcat, Jetty, and
        // Spring strip before resolving the segment.
        "/a/..;/b",
        "/a/.;/b",
        "/a/..;x=1/b",
        "/a/..;a;b",
        "/..;",
        "/a/..%3b/b",
        "/a/.%3B/b",
        // Dot segments behind a decoded `?` or `#`, which a server that decodes
        // and then parses the target again reads as the end of the path.
        "/a/..%3F/b",
        "/a/..%3f",
        "/a/..%3F",
        "/a/..%3Fx=1/b",
        "/a/.%3f/b",
        "/a/..%23/b",
        "/a/.%23frag",
        // Dot segments padded with escaped spaces, which a trimming server
        // strips.
        "/a/..%20/b",
        "/a/%20../b",
        "/a/%20.%20",
        "/a/..%20%20;x/b",
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
        // Percent-encoded control bytes: NUL can truncate `..%00` to `..`, and
        // a trimming server strips `..%09` or `..%0a` to `..`.
        "/a%00",
        "/a/..%00/b",
        "/a/..%09/b",
        "/a/..%0a/b",
        "/a/..%0D%0AX-Injected:1/b",
        "/a%01",
        "/a%1F",
        "/a%7f",
        "/a%7F",
        // Escapes that survive one decoding pass, at any depth.
        "/a/%252e%252e/b",
        "/a/%252E",
        "/a%252fb",
        "/a%255Cb",
        "/a/%2500",
        "/a/%25252e",
        "/a/%25%32%65",
        "/a/%2541",
        "/a/100%25A",
        "/%2525",
        "/a/%25u002e",
        "/a/%25U002E",
        "/a/%25c0%25ae",
        // Overlong UTF-8, which a lenient decoder reads as ASCII.
        "/a/%c0%ae%c0%ae/b",
        "/a%c0%afb",
        "/a/%C1%9C",
        "/a/%e0%80%ae",
        "/a/%f0%80%80%ae",
        "/a/%f8%80%80%80%ae",
        "/a/%fc%80%80%80%80%ae",
        // The same forms for a decoder that reads the low six bits of each
        // byte after a lead without checking it, as IIS 4 and 5 did. `C0` and
        // `C1` never occur in UTF-8, so either is refused whatever follows.
        "/a/..%c1%1c..",
        "/a/..%c1%dc..",
        "/a/%c1%1c",
        "/a/%c0%6e%c0%6e/b",
        "/a/%c0n%c0n/b",
        "/a/%c0%ee",
        "/a/..%C0/b",
        "/%C0",
        "/%c1",
        "/a/%e0%40%6e",
        "/a/%e0@n%e0@n/b",
        "/a/%E0A%9C",
        "/a/%f0%40%80%ae",
        "/a/%f8%40",
        "/a/%fc%40",
        // Malformed escapes, whose decoding is implementation-defined.
        "/%",
        "/a%2",
        "/a%zz",
        "/a%%2e",
        "/a%2g",
        "/100%",
        "/a%u002e",
    ];

    #[test]
    fn every_traversal_form_is_refused_with_k4001() {
        for path in TRAVERSAL_PATHS {
            assert!(!is_origin_form_path(path), "`{path}` must be refused");
            let error = request_with_path(path)
                .validate(RuntimeLimits::default())
                .expect_err("a traversal form must fail validation");
            assert_eq!(error.code(), "K4001", "`{path}`");
            assert_eq!(error.message(), TRAVERSAL_MESSAGE, "`{path}`");
        }
        assert_eq!(
            TRAVERSAL_MESSAGE,
            "HTTP request path must not contain a dot segment, a percent-encoded `.`, `/`, `\\`, or control byte, a double-encoded or overlong UTF-8 escape, or a malformed percent escape"
        );
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
            // Path parameters on a segment that is not a dot segment.
            "/files;v=2/report",
            "/a/.x;y/b",
            "/a/..b;c",
            "/a/...;x",
            "/a/;../b",
            // An escaped `?` or `#` that does not end a dot segment.
            "/C%23",
            "/a%3Fb",
            "/a/x%3F../b",
            "/a/%3F..",
            "/a/%23",
            // Escaped spaces that do not pad a dot segment.
            "/a/%20",
            "/a/.%20./b",
            "/a/...%20",
            // Well-formed escapes of anything but `.`, `/`, `\`, or a control
            // byte, and a `%25` that no hex digit, `u`, or `U` follows.
            "/a%20b",
            "/search/caf%C3%A9",
            "/search/caf%c3%a9",
            "/100%25",
            "/50%25off",
            "/%25/next",
            "/a%2Db",
            "/a%3a%3Ab",
            "/%7Euser",
            "/a%3Bb",
            // A lone Latin-1 byte other than `C0` or `C1` is invalid UTF-8 but
            // not overlong, even read leniently.
            "/caf%E9",
            "/d%E9j%E0-vu",
            "/voil%E0",
            "/%E0la",
            "/%F0x",
            "/%C2",
            "/%E0%A4%85",
            "/%F0%9F%98%80",
            // Raw non-ASCII text.
            "/caf\u{e9}",
            // Unicode compatibility forms are server-specific and out of scope
            // (WEBHOOK-CONTRACTS.md, "Request paths and queries").
            "/\u{ff0e}\u{ff0e}/admin",
            "/%EF%BC%8E%EF%BC%8E/admin",
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
            query: "next=%2Fadmin%2F..&path=../a&q=a%2eb&d=%252e&s=a+b%20c".to_owned(),
            ..request_with_path("/search")
        };
        request
            .validate(RuntimeLimits::default())
            .expect("a query may carry encoded separators");
    }

    #[test]
    fn a_raw_space_is_refused_in_the_path_and_the_query() {
        // libcurl refuses a raw space anywhere in the target, so the host
        // refuses it first, before approval or DNS resolution.
        for path in ["/a b", "/a/ ", " /a"] {
            assert!(!is_origin_form_path(path), "`{path}` must be refused");
            let error = request_with_path(path)
                .validate(RuntimeLimits::default())
                .expect_err("a raw space must fail validation");
            assert_eq!(error.code(), "K4001");
            assert_eq!(
                error.message(),
                "HTTP request path must be a safe origin-form absolute path",
                "`{path:?}`"
            );
        }
        let error = HttpRequest {
            query: "q=a b".to_owned(),
            ..request_with_path("/search")
        }
        .validate(RuntimeLimits::default())
        .expect_err("a raw space in a query must fail validation");
        assert_eq!(error.code(), "K4001");
        assert_eq!(
            error.message(),
            "HTTP request query must not contain a leading `?`, fragment, control byte, space, or backslash"
        );
    }

    #[test]
    fn the_shape_rule_is_reported_before_the_traversal_rule() {
        for path in [
            "relative/../path",
            "//evil.example/..",
            "/a\\..\\b",
            "/a b/..",
        ] {
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

    #[test]
    fn escaped_and_raw_control_bytes_are_both_refused() {
        // A raw control byte breaks the shape rule; its escape breaks the
        // traversal rule. Neither form of any control byte is accepted.
        for byte in (0u8..0x20).chain([0x7F]) {
            let raw = format!("/a{}b", char::from(byte));
            assert!(!is_origin_form_path(&raw), "raw {byte:#04x}");
            for escaped in [format!("/a%{byte:02x}b"), format!("/a%{byte:02X}b")] {
                assert!(!is_origin_form_path(&escaped), "`{escaped}`");
            }
        }
    }

    #[test]
    fn a_deeply_nested_encoding_is_refused_in_one_pass() {
        // One decoding pass decides every depth, so a long nested encoding is
        // refused without decoding it level by level.
        let path = format!("/{}2e", "%25".repeat(200_000));
        assert!(!is_origin_form_path(&path));
    }
}
