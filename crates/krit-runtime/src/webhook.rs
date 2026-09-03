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
/// Shared so every host-owned path - a webhook request path and a search
/// connector path alike - is held to exactly the same rule: absolute, no
/// authority form, no scheme, no backslash, no query or fragment delimiter, and
/// no control byte.
pub(crate) fn is_origin_form_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains("://")
        && !path
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'\\' | b'?' | b'#'))
}

fn validate_path_query(path: &str, query: &str) -> Result<(), RuntimeError> {
    if !is_origin_form_path(path) {
        return Err(RuntimeError::guest(
            "K4001",
            "HTTP request path must be a safe origin-form absolute path",
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
