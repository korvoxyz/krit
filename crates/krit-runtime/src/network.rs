use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use curl::easy::{Easy, HttpVersion, List, SslOpt};
use http::{HeaderName, HeaderValue};
use krit_capability::{HttpOrigin, HttpScheme};
use zeroize::Zeroize;

use crate::{
    CancellationHandle, HttpHeader, HttpRequest, HttpResponse, NetworkPolicy, RuntimeLimits,
    host::SecretBytes,
};

const MIN_CURL_TIMEOUT: Duration = Duration::from_millis(1);
const MAX_DNS_WORKERS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkFailureKind {
    Cancelled,
    Deadline,
    Retryable,
    Fatal,
}

#[derive(Debug)]
pub(crate) struct NetworkFailure {
    pub kind: NetworkFailureKind,
    pub message: String,
}

pub(crate) struct SendContext<'a> {
    pub policy: NetworkPolicy,
    pub limits: RuntimeLimits,
    pub remaining: Duration,
    pub cancellation: &'a CancellationHandle,
    pub active_dns_workers: &'a Arc<AtomicUsize>,
}

impl NetworkFailure {
    fn new(kind: NetworkFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        self.kind == NetworkFailureKind::Retryable
    }
}

impl From<String> for NetworkFailure {
    fn from(message: String) -> Self {
        Self::new(NetworkFailureKind::Fatal, message)
    }
}

pub(crate) fn send(
    origin: &HttpOrigin,
    request: &HttpRequest,
    bearer: Option<&SecretBytes>,
    context: SendContext<'_>,
) -> Result<HttpResponse, NetworkFailure> {
    let SendContext {
        policy,
        limits,
        remaining,
        cancellation,
        active_dns_workers,
    } = context;
    if cancellation.is_cancelled() {
        return Err(NetworkFailure::new(
            NetworkFailureKind::Cancelled,
            "outbound HTTP operation was cancelled",
        ));
    }
    if bearer.is_some() && origin.scheme() == HttpScheme::Http && !policy.permits_plaintext_bearer()
    {
        return Err(
            "bearer authentication over plain HTTP is denied by host policy"
                .to_owned()
                .into(),
        );
    }
    let started = Instant::now();
    let deadline = started + remaining;
    let initial_timeout = limits
        .http_timeout()
        .min(limits.read_timeout())
        .min(remaining);
    if initial_timeout < MIN_CURL_TIMEOUT {
        return Err(NetworkFailure::new(
            NetworkFailureKind::Deadline,
            "outbound HTTP invocation deadline expired",
        ));
    }
    let resolve_timeout = limits.connect_timeout().min(initial_timeout);
    let addresses = resolve_addresses(
        origin,
        policy,
        resolve_timeout,
        cancellation,
        deadline,
        active_dns_workers,
    )?;
    let overall_timeout = limits
        .http_timeout()
        .min(limits.read_timeout())
        .min(remaining.saturating_sub(started.elapsed()));
    if overall_timeout < MIN_CURL_TIMEOUT {
        return Err(NetworkFailure::new(
            NetworkFailureKind::Deadline,
            "outbound HTTP invocation deadline expired after DNS resolution",
        ));
    }
    let target = if request.query.is_empty() {
        format!("{}{}", origin.as_str(), request.path)
    } else {
        format!("{}{}?{}", origin.as_str(), request.path, request.query)
    };

    let mut easy = Easy::new();
    easy.url(&target)
        .map_err(|error| format!("invalid outbound HTTP URL: {error}"))?;
    easy.proxy("")
        .map_err(|error| format!("could not disable outbound proxy inheritance: {error}"))?;
    easy.follow_location(false)
        .map_err(|error| format!("could not disable outbound redirects: {error}"))?;
    easy.max_redirections(0)
        .map_err(|error| format!("could not bound outbound redirects: {error}"))?;
    easy.fail_on_error(false)
        .map_err(|error| format!("could not configure HTTP status handling: {error}"))?;
    easy.http_version(HttpVersion::V11)
        .map_err(|error| format!("could not select bounded HTTP version: {error}"))?;
    easy.fresh_connect(true)
        .and_then(|()| easy.forbid_reuse(true))
        .map_err(|error| format!("could not isolate outbound HTTP connection: {error}"))?;
    easy.connect_timeout(limits.connect_timeout().min(overall_timeout))
        .map_err(|error| format!("could not configure connect timeout: {error}"))?;
    easy.timeout(overall_timeout)
        .map_err(|error| format!("could not configure overall HTTP timeout: {error}"))?;
    easy.progress(true)
        .map_err(|error| format!("could not enable HTTP cancellation checks: {error}"))?;
    easy.ssl_verify_peer(true)
        .and_then(|()| easy.ssl_verify_host(true))
        .map_err(|error| format!("could not enforce TLS verification: {error}"))?;
    let mut ssl_options = SslOpt::new();
    ssl_options.native_ca(true);
    easy.ssl_options(&ssl_options)
        .map_err(|error| format!("could not select native TLS trust roots: {error}"))?;

    if origin.ip_address().is_none() {
        let mut resolve = List::new();
        let addresses = addresses
            .iter()
            .map(|address| match address.ip() {
                IpAddr::V4(address) => address.to_string(),
                IpAddr::V6(address) => format!("[{address}]"),
            })
            .collect::<Vec<_>>()
            .join(",");
        resolve
            .append(&format!(
                "{}:{}:{addresses}",
                origin.host().trim_matches(['[', ']']),
                origin.port()
            ))
            .map_err(|error| format!("could not construct pinned DNS result: {error}"))?;
        easy.resolve(resolve)
            .map_err(|error| format!("could not pin outbound DNS result: {error}"))?;
    }

    let mut headers = List::new();
    let mut has_content_type = false;
    let mut has_expect = false;
    for header in &request.headers {
        has_content_type |= header.name.eq_ignore_ascii_case("content-type");
        has_expect |= header.name.eq_ignore_ascii_case("expect");
        headers
            .append(&format!("{}: {}", header.name, header.value))
            .map_err(|error| format!("could not encode outbound HTTP header: {error}"))?;
    }
    if let Some(secret) = bearer {
        let bytes = secret.expose_for_bearer();
        let header = HeaderValue::from_bytes(bytes)
            .map_err(|_| "secret is not a valid bearer credential".to_owned())?;
        header
            .to_str()
            .map_err(|_| "secret is not a valid bearer credential".to_owned())?;
        let mut value = Vec::with_capacity(22usize.saturating_add(bytes.len()));
        value.extend_from_slice(b"Authorization: Bearer ");
        value.extend_from_slice(bytes);
        let value_string = std::str::from_utf8(&value)
            .map_err(|_| "secret is not a valid bearer credential".to_owned())?;
        headers
            .append(value_string)
            .map_err(|error| format!("could not encode bearer header: {error}"))?;
        value.zeroize();
    }
    if !has_expect {
        headers
            .append("Expect:")
            .map_err(|error| format!("could not disable automatic Expect header: {error}"))?;
    }
    if !has_content_type {
        headers
            .append("Content-Type:")
            .map_err(|error| format!("could not disable automatic Content-Type: {error}"))?;
    }
    easy.http_headers(headers)
        .map_err(|error| format!("could not configure outbound HTTP headers: {error}"))?;
    if request.method.eq_ignore_ascii_case("HEAD") {
        easy.nobody(true)
            .map_err(|error| format!("could not configure bodyless HEAD response: {error}"))?;
    } else if !request.body.is_empty()
        || !matches!(request.method.as_str(), "GET" | "OPTIONS" | "TRACE")
    {
        easy.post_fields_copy(request.body.as_bytes())
            .map_err(|error| format!("could not configure outbound HTTP body: {error}"))?;
    }
    easy.custom_request(&request.method)
        .map_err(|error| format!("could not configure outbound HTTP method: {error}"))?;

    let mut response_body = Vec::new();
    let mut response_headers = Vec::new();
    let mut raw_header_count = 0usize;
    let mut raw_header_bytes = 0usize;
    let mut header_failure = None;
    let mut body_limit_exceeded = false;
    let mut cancelled = false;
    let mut deadline_expired = false;
    let perform = {
        let mut transfer = easy.transfer();
        transfer
            .progress_function(|_, _, _, _| {
                if cancellation.is_cancelled() {
                    cancelled = true;
                    return false;
                }
                if Instant::now() >= deadline {
                    deadline_expired = true;
                    return false;
                }
                true
            })
            .map_err(|error| format!("could not install HTTP progress callback: {error}"))?;
        transfer
            .header_function(|line| {
                raw_header_bytes = match raw_header_bytes.checked_add(line.len()) {
                    Some(bytes) => bytes,
                    None => {
                        header_failure = Some("HTTP response header bytes overflowed".to_owned());
                        return false;
                    }
                };
                if raw_header_bytes > limits.header_bytes() {
                    header_failure =
                        Some("outbound HTTP response headers exceed limits".to_owned());
                    return false;
                }
                if line.starts_with(b"HTTP/") {
                    response_headers.clear();
                    return true;
                }
                if line == b"\r\n" || line == b"\n" {
                    return true;
                }
                raw_header_count = match raw_header_count.checked_add(1) {
                    Some(count) => count,
                    None => {
                        header_failure = Some("HTTP response header count overflowed".to_owned());
                        return false;
                    }
                };
                if raw_header_count > limits.header_count() {
                    header_failure =
                        Some("outbound HTTP response headers exceed limits".to_owned());
                    return false;
                }
                let line = line
                    .strip_suffix(b"\r\n")
                    .or_else(|| line.strip_suffix(b"\n"))
                    .unwrap_or(line);
                let Ok(line) = std::str::from_utf8(line) else {
                    header_failure =
                        Some("outbound HTTP response contains a non-UTF-8 header".to_owned());
                    return false;
                };
                let Some((name, value)) = line.split_once(':') else {
                    header_failure =
                        Some("outbound HTTP response contains a malformed header".to_owned());
                    return false;
                };
                let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                    header_failure =
                        Some("outbound HTTP response contains an invalid header name".to_owned());
                    return false;
                };
                let value = value.trim_matches([' ', '\t']);
                if HeaderValue::from_str(value).is_err() {
                    header_failure =
                        Some("outbound HTTP response contains an invalid header value".to_owned());
                    return false;
                }
                if !protocol_managed_header(name.as_str()) {
                    response_headers.push(HttpHeader {
                        name: name.as_str().to_owned(),
                        value: value.to_owned(),
                    });
                }
                true
            })
            .map_err(|error| format!("could not install HTTP header callback: {error}"))?;
        transfer
            .write_function(|data| {
                let Some(next) = response_body.len().checked_add(data.len()) else {
                    body_limit_exceeded = true;
                    return Ok(0);
                };
                if next > limits.response_body_bytes() {
                    body_limit_exceeded = true;
                    return Ok(0);
                }
                response_body.extend_from_slice(data);
                Ok(data.len())
            })
            .map_err(|error| format!("could not install HTTP body callback: {error}"))?;
        transfer.perform()
    };
    if let Some(error) = header_failure {
        return Err(error.into());
    }
    if body_limit_exceeded {
        return Err(format!(
            "outbound HTTP response body exceeds the {}-byte limit",
            limits.response_body_bytes()
        )
        .into());
    }
    if cancelled {
        return Err(NetworkFailure::new(
            NetworkFailureKind::Cancelled,
            "outbound HTTP operation was cancelled",
        ));
    }
    if deadline_expired {
        return Err(NetworkFailure::new(
            NetworkFailureKind::Deadline,
            "outbound HTTP invocation deadline expired",
        ));
    }
    if let Err(error) = perform {
        let retryable = error.is_couldnt_resolve_host()
            || error.is_couldnt_connect()
            || error.is_operation_timedout()
            || error.is_got_nothing()
            || error.is_send_error()
            || error.is_recv_error();
        return Err(NetworkFailure::new(
            if retryable {
                NetworkFailureKind::Retryable
            } else {
                NetworkFailureKind::Fatal
            },
            if error.is_operation_timedout() {
                "outbound HTTP request timed out".to_owned()
            } else if retryable {
                "outbound HTTP connection failed".to_owned()
            } else {
                format!("outbound HTTP request failed: {error}")
            },
        ));
    }
    let status = i64::from(
        easy.response_code()
            .map_err(|error| format!("could not read outbound HTTP status: {error}"))?,
    );
    if (300..=399).contains(&status) {
        return Err("outbound HTTP redirects are denied".to_owned().into());
    }
    let body = String::from_utf8(response_body)
        .map_err(|_| "outbound HTTP response body is not valid UTF-8".to_owned())?;
    let response = HttpResponse {
        status,
        headers: response_headers,
        body,
    };
    response
        .validate(limits)
        .map_err(|error| error.message().to_owned())?;
    Ok(response)
}

fn resolve_addresses(
    origin: &HttpOrigin,
    policy: NetworkPolicy,
    timeout: Duration,
    cancellation: &CancellationHandle,
    deadline: Instant,
    active_dns_workers: &Arc<AtomicUsize>,
) -> Result<Vec<SocketAddr>, NetworkFailure> {
    let host = origin.host().trim_matches(['[', ']']).to_owned();
    let port = origin.port();
    let addresses = if let Some(address) = origin.ip_address() {
        vec![SocketAddr::new(address, port)]
    } else {
        let previous = active_dns_workers.fetch_add(1, Ordering::AcqRel);
        if previous >= MAX_DNS_WORKERS {
            active_dns_workers.fetch_sub(1, Ordering::AcqRel);
            return Err(NetworkFailure::new(
                NetworkFailureKind::Fatal,
                "bounded DNS worker limit exceeded",
            ));
        }
        let target = format!("{host}:{port}");
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_active = Arc::clone(active_dns_workers);
        let worker = thread::Builder::new()
            .name("krit-http-dns".to_owned())
            .spawn(move || {
                struct ActiveGuard(Arc<AtomicUsize>);
                impl Drop for ActiveGuard {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _guard = ActiveGuard(worker_active);
                let resolved = target
                    .to_socket_addrs()
                    .map(|addresses| addresses.take(16).collect::<Vec<_>>());
                let _ = sender.send(resolved);
            })
            .map_err(|_| {
                active_dns_workers.fetch_sub(1, Ordering::AcqRel);
                NetworkFailure::new(
                    NetworkFailureKind::Fatal,
                    "could not start bounded DNS resolver",
                )
            })?;
        let resolution_deadline = Instant::now() + timeout;
        let resolved = loop {
            if cancellation.is_cancelled() {
                return Err(NetworkFailure::new(
                    NetworkFailureKind::Cancelled,
                    "outbound HTTP DNS resolution was cancelled",
                ));
            }
            let now = Instant::now();
            if now >= resolution_deadline || now >= deadline {
                return Err(NetworkFailure::new(
                    NetworkFailureKind::Retryable,
                    "outbound HTTP DNS resolution timed out",
                ));
            }
            let wait = resolution_deadline
                .saturating_duration_since(now)
                .min(deadline.saturating_duration_since(now))
                .min(Duration::from_millis(10));
            match receiver.recv_timeout(wait) {
                Ok(resolved) => break resolved,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(NetworkFailure::new(
                        NetworkFailureKind::Fatal,
                        "outbound HTTP DNS resolver stopped unexpectedly",
                    ));
                }
            }
        };
        worker.join().map_err(|_| {
            NetworkFailure::new(
                NetworkFailureKind::Fatal,
                "outbound HTTP DNS resolver panicked",
            )
        })?;
        resolved.map_err(|_| {
            NetworkFailure::new(
                NetworkFailureKind::Retryable,
                "outbound HTTP DNS resolution failed",
            )
        })?
    };
    if addresses.is_empty() {
        return Err(NetworkFailure::new(
            NetworkFailureKind::Retryable,
            "outbound HTTP DNS resolution returned no addresses",
        ));
    }
    let mut unique = Vec::new();
    for address in addresses {
        if address.port() != port || !policy.permits_address(address.ip()) {
            return Err(NetworkFailure::new(
                NetworkFailureKind::Fatal,
                format!(
                    "outbound HTTP address `{}` is denied by host network policy",
                    redacted_address_class(address.ip())
                ),
            ));
        }
        if !unique.contains(&address) {
            unique.push(address);
        }
    }
    unique.sort();
    Ok(unique)
}

fn redacted_address_class(address: IpAddr) -> &'static str {
    match address {
        IpAddr::V4(address) if address.is_loopback() => "loopback",
        IpAddr::V4(address) if address.is_link_local() => "link-local",
        IpAddr::V4(address) if address.is_private() => "private",
        IpAddr::V6(address) if address.is_loopback() => "loopback",
        IpAddr::V6(address) if address.is_unicast_link_local() => "link-local",
        IpAddr::V6(address) if address.is_unique_local() => "private",
        _ => "non-public",
    }
}

fn protocol_managed_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "content-length"
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
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    #[test]
    fn sub_millisecond_budget_expires_before_dns_or_curl() {
        let origin =
            HttpOrigin::parse_exact("https://example.com").expect("origin should be canonical");
        let request = HttpRequest {
            method: "GET".to_owned(),
            path: "/".to_owned(),
            query: String::new(),
            headers: Vec::new(),
            body: String::new(),
        };

        let error = send(
            &origin,
            &request,
            None,
            SendContext {
                policy: NetworkPolicy::default(),
                limits: RuntimeLimits::default(),
                remaining: Duration::from_nanos(1),
                cancellation: &CancellationHandle::new(),
                active_dns_workers: &Arc::new(AtomicUsize::new(0)),
            },
        )
        .expect_err("sub-millisecond budget must not become an unlimited curl timeout");

        assert!(error.message.contains("deadline expired"));
    }

    #[test]
    fn linked_curl_uses_the_pinned_rustls_backend() {
        let version = curl::Version::get();
        let ssl = version
            .ssl_version()
            .expect("static curl should report its TLS backend");
        assert!(ssl.to_ascii_lowercase().contains("rustls"), "{ssl}");
    }

    #[test]
    fn head_finishes_after_headers_without_waiting_for_a_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock should bind");
        let address = listener.local_addr().expect("mock address should exist");
        let mock = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock should accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("mock read timeout should configure");
            let mut request = [0u8; 4096];
            let count = stream.read(&mut request).expect("mock should read request");
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("HEAD / HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\nConnection: keep-alive\r\n\r\n",
                )
                .expect("mock should write response headers");
            thread::sleep(Duration::from_millis(100));
        });
        let origin = HttpOrigin::parse_exact(&format!("http://{address}"))
            .expect("loopback origin should be canonical");
        let request = HttpRequest {
            method: "HEAD".to_owned(),
            path: "/".to_owned(),
            query: String::new(),
            headers: Vec::new(),
            body: String::new(),
        };
        let mut limits = RuntimeLimits::default();
        limits
            .narrow_http_timeout(Duration::from_millis(50))
            .expect("HTTP timeout should narrow");

        let response = send(
            &origin,
            &request,
            None,
            SendContext {
                policy: NetworkPolicy::loopback_for_tests(),
                limits,
                remaining: Duration::from_secs(1),
                cancellation: &CancellationHandle::new(),
                active_dns_workers: &Arc::new(AtomicUsize::new(0)),
            },
        )
        .expect("HEAD should complete from response headers");

        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
        mock.join().expect("mock should finish");
    }

    #[test]
    #[ignore = "requires public DNS and network access"]
    fn trusted_public_https_smoke_test() {
        let origin =
            HttpOrigin::parse_exact("https://example.com").expect("origin should be canonical");
        let request = HttpRequest {
            method: "GET".to_owned(),
            path: "/".to_owned(),
            query: String::new(),
            headers: Vec::new(),
            body: String::new(),
        };

        let response = send(
            &origin,
            &request,
            None,
            SendContext {
                policy: NetworkPolicy::default(),
                limits: crate::HARD_MAX_LIMITS,
                remaining: Duration::from_secs(20),
                cancellation: &CancellationHandle::new(),
                active_dns_workers: &Arc::new(AtomicUsize::new(0)),
            },
        )
        .expect("trusted HTTPS request should succeed");

        assert_eq!(response.status, 200);
    }
}
