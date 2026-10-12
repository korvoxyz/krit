//! The host's rule for HTTP request paths and queries, and the
//! percent-encoding builders that keep text inside that rule.
//!
//! One rule covers every request path the host accepts: an inbound webhook
//! request, a guest `http_request`, durable HTTP replay, and the request an AI
//! adapter or search connector builds (`spec/WEBHOOK-CONTRACTS.md`, "Request
//! paths and queries"). The rule lives here, in a leaf crate, so the host
//! enforces it and the language's standard library (`url_path_segment` and
//! `url_query`, `spec/LANGUAGE.md`, "Standard library") builds text that it
//! accepts from one definition.

use std::fmt;

/// Whether a string is a safe origin-form absolute path.
///
/// Every host-owned path - an inbound webhook request path, a guest outbound
/// request path, an AI adapter path, and a search connector path alike - is
/// held to exactly this rule: absolute, no authority form, no scheme, no
/// space, no backslash, no query or fragment delimiter, no control byte (see
/// [`has_origin_form_shape`]), and no traversal form (see
/// [`has_traversal_form`]).
pub fn is_origin_form_path(path: &str) -> bool {
    has_origin_form_shape(path) && !has_traversal_form(path)
}

/// Whether a path has the shape of an origin-form absolute path: it starts
/// with `/` but not `//`, and holds no `://`, ASCII control byte, space,
/// backslash, `?`, or `#`.
pub fn has_origin_form_shape(path: &str) -> bool {
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
pub fn has_traversal_form(path: &str) -> bool {
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

/// Whether a request query is one the host sends: it has no leading `?` and
/// holds no ASCII control byte, space, `#`, or backslash.
///
/// A query keeps its own rule: encoded separators in it are ordinary data,
/// but a raw space can no more be sent there than in the path.
pub fn is_valid_query(query: &str) -> bool {
    !query.starts_with('?')
        && !query
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b' ' | b'#' | b'\\'))
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
    decoded
        .windows(2)
        .any(|pair| pair[0] == b'%' && begins_escape(pair[1]))
}

/// Whether a byte after `%` would make a decoder read an escape: a hex digit
/// (`%HH`) or `u`/`U` (IIS-style `%uHHHH`).
fn begins_escape(byte: u8) -> bool {
    hex_digit(byte).is_some() || matches!(byte, b'u' | b'U')
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

/// Whether `byte` is an RFC 3986 unreserved character (`A`-`Z`, `a`-`z`,
/// `0`-`9`, `-`, `.`, `_`, `~`), the only bytes the builders leave unencoded.
pub const fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// Appends `text` to `output` with every byte that is not RFC 3986 unreserved
/// written as `%HH` with uppercase hex digits, so a space is `%20` and each
/// byte of a multibyte UTF-8 character is escaped on its own.
pub fn percent_encode(text: &str, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        if is_unreserved(byte) {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
}

/// Why `url_path_segment` refuses a text: the host path rule would refuse the
/// segment its percent-encoding produces, or the text is empty.
///
/// The variants are checked in declaration order and the first that applies
/// is reported. Their messages never repeat the text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PathSegmentError {
    /// The text is empty: the segment would vanish, leaving `//` in a path.
    Empty,
    /// The text holds `/`, `\`, or an ASCII control byte, whose escapes
    /// (`%2F`, `%5C`, `%00`-`%1F`, `%7F`) the host refuses.
    ForbiddenByte,
    /// The text holds `%` followed by a hex digit, `u`, or `U`: once encoded
    /// it would decode to an escape again (a double encoding).
    EscapeLike,
    /// The text reads as `.` or `..` after the part from its first `;`, `?`,
    /// or `#` is cut off and ASCII spaces are trimmed from both ends.
    DotSegment,
}

impl PathSegmentError {
    /// Returns why the host path rule would refuse the segment `text` encodes
    /// to, or `None` when it accepts it.
    ///
    /// Every escape the encoding writes decodes back to the byte it encodes,
    /// so the host's decoded segment is exactly `text`, and its checks reduce
    /// to these, in this order. Valid UTF-8, which every Rust string is, never
    /// spells an overlong sequence, and the encoding never writes a `/`, a
    /// space, a backslash, `?`, `#`, `:`, or a malformed escape, so the shape
    /// rule and the remaining checks always hold.
    pub fn check(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            Some(Self::Empty)
        } else if bytes
            .iter()
            .any(|byte| matches!(byte, b'/' | b'\\') || byte.is_ascii_control())
        {
            Some(Self::ForbiddenByte)
        } else if holds_escape(bytes) {
            Some(Self::EscapeLike)
        } else if is_dot_segment(bytes) {
            Some(Self::DotSegment)
        } else {
            None
        }
    }

    /// The `Err` text `url_path_segment` returns, which never contains the
    /// refused text.
    pub const fn message(self) -> &'static str {
        match self {
            Self::Empty => "path segment is empty",
            Self::ForbiddenByte => "path segment contains `/`, `\\`, or an ASCII control character",
            Self::EscapeLike => "path segment contains `%` followed by a hex digit, `u`, or `U`",
            Self::DotSegment => "path segment reads as `.` or `..`",
        }
    }
}

impl fmt::Display for PathSegmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for PathSegmentError {}

/// Percent-encodes `text` as one path segment, or refuses it.
///
/// Every byte but the RFC 3986 unreserved ones is written `%HH` with
/// uppercase hex digits. The text is refused, with the first reason
/// [`PathSegmentError::check`] finds, when it is empty or when the host path
/// rule would refuse the segment, so `"/" + segment` and
/// `prefix + "/" + segment + "/" + suffix` are accepted for every `Ok`
/// segment and safe prefix and suffix, and decoding the segment once gives
/// back exactly `text`.
pub fn encode_path_segment(text: &str) -> Result<String, PathSegmentError> {
    if let Some(error) = PathSegmentError::check(text) {
        return Err(error);
    }
    let mut encoded = String::with_capacity(text.len());
    percent_encode(text, &mut encoded);
    debug_assert!(
        is_origin_form_path(&format!("/{encoded}")),
        "an accepted segment must satisfy the host path rule"
    );
    Ok(encoded)
}

/// Builds a query string `name=value&name=value` from ordered pairs, with
/// every name and value percent-encoded as [`percent_encode`] does, no
/// leading `?`, and `""` for no pairs.
///
/// The result holds only unreserved bytes, `%`, hex digits, `=`, and `&`, so
/// [`is_valid_query`] accepts every result.
pub fn encode_query<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut query = String::new();
    for (index, (name, value)) in pairs.into_iter().enumerate() {
        if index > 0 {
            query.push('&');
        }
        percent_encode(name, &mut query);
        query.push('=');
        percent_encode(value, &mut query);
    }
    debug_assert!(is_valid_query(&query), "a built query must be sendable");
    query
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes every `%HH` escape of a builder's output, which holds only
    /// unreserved bytes and well-formed escapes.
    fn decode(encoded: &str) -> Vec<u8> {
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let high = hex_digit(bytes[index + 1]).expect("escape has a high digit");
                let low = hex_digit(bytes[index + 2]).expect("escape has a low digit");
                decoded.push((high << 4) | low);
                index += 3;
            } else {
                assert!(is_unreserved(bytes[index]), "`{encoded}` holds a raw byte");
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        decoded
    }

    fn encoded(text: &str) -> String {
        let mut output = String::new();
        percent_encode(text, &mut output);
        output
    }

    /// Characters that exercise every branch of the host rule, plus ordinary
    /// and multibyte text.
    const ALPHABET: &[char] = &[
        'a', 'Z', '0', '9', 'f', 'G', 'u', 'U', '-', '_', '~', '.', '/', '\\', ';', '?', '#', ' ',
        '%', ':', '@', '&', '=', '+', '\t', '\n', '\r', '\0', '\u{7f}', '\u{80}', '\u{a0}', 'é',
        '\u{ff0e}', '😀',
    ];

    /// Asserts the `url_path_segment` contract for one text: `Ok` exactly
    /// when the text is not empty and the host accepts the encoded segment,
    /// alone and between other segments, and `Ok` output decodes to the text.
    fn assert_segment_contract(text: &str) {
        let candidate = encoded(text);
        let accepted = is_origin_form_path(&format!("/{candidate}"));
        assert_eq!(
            accepted,
            is_origin_form_path(&format!("/repos/{candidate}/issues")),
            "{text:?}: a segment's acceptance must not depend on its neighbours"
        );
        match encode_path_segment(text) {
            Ok(segment) => {
                assert!(!text.is_empty());
                assert!(
                    accepted,
                    "{text:?} encodes to a refused segment `{segment}`"
                );
                assert_eq!(segment, candidate);
                assert_eq!(decode(&segment), text.as_bytes(), "{text:?}");
            }
            Err(error) => {
                assert!(
                    text.is_empty() || !accepted,
                    "{text:?} is refused ({error}) but the host accepts `/{candidate}`"
                );
                assert_eq!(error == PathSegmentError::Empty, text.is_empty());
            }
        }
    }

    #[test]
    fn encodes_every_byte_but_the_unreserved_ones_in_uppercase_hex() {
        assert_eq!(encoded("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(encoded("a b"), "a%20b");
        assert_eq!(encoded("café"), "caf%C3%A9");
        assert_eq!(encoded("😀"), "%F0%9F%98%80");
        assert_eq!(
            encoded("!*'();:@&=+$,/?#[]%"),
            "%21%2A%27%28%29%3B%3A%40%26%3D%2B%24%2C%2F%3F%23%5B%5D%25"
        );
        assert_eq!(
            encode_path_segment("a/b"),
            Err(PathSegmentError::ForbiddenByte)
        );
        assert_eq!(
            encode_path_segment("owner.name").as_deref(),
            Ok("owner.name")
        );
        assert_eq!(encode_path_segment("a b;c").as_deref(), Ok("a%20b%3Bc"));
        assert_eq!(encode_path_segment("100%").as_deref(), Ok("100%25"));
        assert_eq!(encode_path_segment("50%off").as_deref(), Ok("50%25off"));
        assert_eq!(encode_path_segment("...").as_deref(), Ok("..."));
        assert_eq!(encode_path_segment("..a").as_deref(), Ok("..a"));
        assert_eq!(encode_path_segment("a..").as_deref(), Ok("a.."));
        assert_eq!(encode_path_segment(";..").as_deref(), Ok("%3B.."));
    }

    #[test]
    fn refuses_with_the_first_reason_that_applies() {
        for (text, error) in [
            ("", PathSegmentError::Empty),
            ("/", PathSegmentError::ForbiddenByte),
            ("a\\b", PathSegmentError::ForbiddenByte),
            ("a\u{0}", PathSegmentError::ForbiddenByte),
            ("\t..", PathSegmentError::ForbiddenByte),
            ("..\u{7f}", PathSegmentError::ForbiddenByte),
            ("../%41", PathSegmentError::ForbiddenByte),
            ("%41", PathSegmentError::EscapeLike),
            ("%2e%2e", PathSegmentError::EscapeLike),
            ("100%25", PathSegmentError::EscapeLike),
            ("%u002e", PathSegmentError::EscapeLike),
            ("%U002E", PathSegmentError::EscapeLike),
            ("..%3B", PathSegmentError::EscapeLike),
            (".", PathSegmentError::DotSegment),
            ("..", PathSegmentError::DotSegment),
            (" .. ", PathSegmentError::DotSegment),
            ("..;x=1", PathSegmentError::DotSegment),
            (".?", PathSegmentError::DotSegment),
            ("..#frag", PathSegmentError::DotSegment),
            ("  .;..", PathSegmentError::DotSegment),
        ] {
            assert_eq!(encode_path_segment(text), Err(error), "{text:?}");
            assert_eq!(PathSegmentError::check(text), Some(error), "{text:?}");
            assert_segment_contract(text);
        }
        for error in [
            PathSegmentError::Empty,
            PathSegmentError::ForbiddenByte,
            PathSegmentError::EscapeLike,
            PathSegmentError::DotSegment,
        ] {
            let message = error.message();
            assert!(message.starts_with("path segment "), "{message}");
            assert!(!message.contains(['\n', '\r']), "{message}");
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn segments_agree_with_the_host_rule_on_every_short_text() {
        // Every text of up to three characters from the alphabet: 34^3 texts.
        let mut text = String::new();
        assert_segment_contract(&text);
        for first in ALPHABET {
            for second in ALPHABET.iter().copied().map(Some).chain([None]) {
                for third in ALPHABET.iter().copied().map(Some).chain([None]) {
                    text.clear();
                    text.push(*first);
                    text.extend(second);
                    text.extend(third);
                    assert_segment_contract(&text);
                }
            }
        }
    }

    #[test]
    fn segments_agree_with_the_host_rule_on_generated_texts() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = |bound: usize| {
            // xorshift64: deterministic, so failures reproduce.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for _ in 0..20_000 {
            let length = next(12);
            let text = (0..length)
                .map(|_| ALPHABET[next(ALPHABET.len())])
                .collect::<String>();
            assert_segment_contract(&text);
        }
    }

    #[test]
    fn query_builder_encodes_names_and_values_and_always_passes_the_query_rule() {
        assert_eq!(encode_query([]), "");
        assert_eq!(encode_query([("q", "a b")]), "q=a%20b");
        assert_eq!(
            encode_query([("state", "open"), ("labels", "bug,good first issue")]),
            "state=open&labels=bug%2Cgood%20first%20issue"
        );
        assert_eq!(encode_query([("", "")]), "=");
        assert_eq!(encode_query([("a&b", "c=d#e?")]), "a%26b=c%3Dd%23e%3F");
        assert_eq!(encode_query([("?x", "é")]), "%3Fx=%C3%A9");
        for first in ALPHABET {
            for second in ALPHABET {
                let name = format!("{first}{second}");
                let value = format!("{second}{first}");
                let query = encode_query([(name.as_str(), value.as_str()), ("k", "")]);
                assert!(is_valid_query(&query), "`{query}`");
                let (pair, rest) = query.split_once('&').expect("two pairs");
                assert_eq!(rest, "k=");
                let (encoded_name, encoded_value) = pair.split_once('=').expect("a pair");
                assert_eq!(decode(encoded_name), name.as_bytes());
                assert_eq!(decode(encoded_value), value.as_bytes());
            }
        }
    }

    #[test]
    fn the_query_rule_refuses_only_unsendable_queries() {
        for valid in [
            "",
            "a=1",
            "next=%2Fadmin%2F..&path=../a",
            "s=a+b%20c",
            "q=?",
        ] {
            assert!(is_valid_query(valid), "{valid:?}");
        }
        for invalid in [
            "?a=1", "q=a b", "q=#", "q=\\", "q=\u{0}", "q=\u{7f}", "q=\n",
        ] {
            assert!(!is_valid_query(invalid), "{invalid:?}");
        }
    }
}
