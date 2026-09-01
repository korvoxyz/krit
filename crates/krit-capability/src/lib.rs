use std::{error::Error, fmt, net::IpAddr};

use url::{Host, Url};

pub fn is_valid_resource_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['.', '-'])
        && !name.ends_with(['.', '-'])
        && !name.contains("..")
        && !name.contains("--")
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HttpOrigin {
    scheme: HttpScheme,
    host: String,
    port: u16,
    normalized: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HttpScheme {
    Http,
    Https,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpOriginError {
    message: &'static str,
}

impl HttpOrigin {
    pub fn parse_exact(value: &str) -> Result<Self, HttpOriginError> {
        if value.is_empty()
            || value.len() > 2048
            || !value.is_ascii()
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(HttpOriginError::invalid());
        }
        let parsed = Url::parse(value).map_err(|_| HttpOriginError::invalid())?;
        let scheme = match parsed.scheme() {
            "http" => HttpScheme::Http,
            "https" => HttpScheme::Https,
            _ => return Err(HttpOriginError::invalid()),
        };
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(HttpOriginError::invalid());
        }
        let host = match parsed.host().ok_or_else(HttpOriginError::invalid)? {
            Host::Domain(host) => {
                if host.is_empty()
                    || host.len() > 253
                    || host.split('.').any(|label| {
                        label.is_empty()
                            || label.len() > 63
                            || label.starts_with('-')
                            || label.ends_with('-')
                            || !label.bytes().all(|byte| {
                                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                            })
                    })
                {
                    return Err(HttpOriginError::invalid());
                }
                host.to_owned()
            }
            Host::Ipv4(address) => address.to_string(),
            Host::Ipv6(address) => format!("[{address}]"),
        };
        let port = parsed
            .port_or_known_default()
            .ok_or_else(HttpOriginError::invalid)?;
        if port == 0 {
            return Err(HttpOriginError::invalid());
        }
        let default_port = scheme.default_port();
        let normalized = if port == default_port {
            format!("{}://{host}", scheme.as_str())
        } else {
            format!("{}://{host}:{port}", scheme.as_str())
        };
        if value != normalized {
            return Err(HttpOriginError::not_normalized());
        }
        Ok(Self {
            scheme,
            host,
            port,
            normalized,
        })
    }

    pub const fn scheme(&self) -> HttpScheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    pub fn ip_address(&self) -> Option<IpAddr> {
        self.host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(&self.host)
            .parse()
            .ok()
    }
}

impl HttpScheme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    pub const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

impl fmt::Display for HttpOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl HttpOriginError {
    const fn invalid() -> Self {
        Self {
            message: "origin must be lowercase `http[s]://host[:port]` without userinfo, path, query, or fragment",
        }
    }

    const fn not_normalized() -> Self {
        Self {
            message: "origin must use its normalized spelling (lowercase host and no default port or trailing slash)",
        }
    }
}

impl fmt::Display for HttpOriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for HttpOriginError {}

#[cfg(test)]
mod tests {
    use super::{HttpOrigin, HttpScheme, is_valid_resource_name};

    #[test]
    fn accepts_only_canonical_resource_names() {
        for valid in ["a", "agent.model", "github-token", "model2"] {
            assert!(is_valid_resource_name(valid), "{valid}");
        }
        for invalid in [
            "",
            ".agent",
            "-token",
            "agent.",
            "token-",
            "agent..model",
            "github--token",
            "Agent",
            "github/token",
        ] {
            assert!(!is_valid_resource_name(invalid), "{invalid}");
        }
        assert!(!is_valid_resource_name(&"a".repeat(65)));
    }

    #[test]
    fn accepts_only_exact_normalized_http_origins() {
        let https = HttpOrigin::parse_exact("https://api.example.com")
            .expect("normalized HTTPS origin should parse");
        assert_eq!(https.scheme(), HttpScheme::Https);
        assert_eq!(https.host(), "api.example.com");
        assert_eq!(https.port(), 443);
        assert_eq!(https.as_str(), "https://api.example.com");

        let ipv6 = HttpOrigin::parse_exact("http://[::1]:8080")
            .expect("normalized IPv6 origin should parse");
        assert_eq!(ipv6.host(), "[::1]");
        assert_eq!(ipv6.port(), 8080);

        for invalid in [
            "",
            "HTTPS://api.example.com",
            "https://API.example.com",
            "https://api.example.com/",
            "https://api.example.com:443",
            "http://api.example.com:80",
            "https://user@api.example.com",
            "https://api.example.com/path",
            "https://api.example.com?query",
            "https://api.example.com#fragment",
            "ftp://api.example.com",
            "https://bad_host.example",
        ] {
            assert!(HttpOrigin::parse_exact(invalid).is_err(), "{invalid}");
        }
    }
}
