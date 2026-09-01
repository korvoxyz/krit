use std::{env, fs, io::Read, net::IpAddr, path::Path, time::Duration};

use curl::easy::{Easy, List};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::{
    context::MAX_REQUEST_BYTES,
    error::AssistError,
    protocol::{AssistRequest, AssistResponse, ProviderDescriptor},
};

const MAX_PROVIDER_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_PROVIDER_RESPONSE_BYTES: usize = 1024 * 1024;
const MIN_TIMEOUT_MS: u64 = 100;
const MAX_TIMEOUT_MS: u64 = 60_000;
const MAX_ENDPOINT_BYTES: usize = 2048;
const MAX_CREDENTIAL_BYTES: usize = 8 * 1024;

pub trait SuggestionProvider {
    fn suggest(&self, request: &AssistRequest) -> Result<AssistResponse, AssistError>;
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    endpoint: String,
    credential_env: Option<String>,
    connect_timeout: Duration,
    timeout: Duration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    schema: u32,
    enabled: bool,
    provider: RawProvider,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawProvider {
    kind: String,
    endpoint: String,
    credential_env: Option<String>,
    connect_timeout_ms: u64,
    timeout_ms: u64,
}

impl ProviderConfig {
    pub fn load(path: &Path) -> Result<Self, AssistError> {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|_| AssistError::disabled("explicit provider config is not accessible"))?
            .take((MAX_PROVIDER_CONFIG_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| AssistError::disabled("could not read explicit provider config"))?;
        if bytes.len() > MAX_PROVIDER_CONFIG_BYTES {
            return Err(AssistError::disabled(format!(
                "provider config exceeds the {MAX_PROVIDER_CONFIG_BYTES}-byte limit"
            )));
        }
        let raw: RawConfig = serde_json::from_slice(&bytes)
            .map_err(|_| AssistError::disabled("provider config is not strict schema-1 JSON"))?;
        if raw.schema != 1 {
            return Err(AssistError::disabled(format!(
                "unsupported provider config schema {}; expected 1",
                raw.schema
            )));
        }
        if !raw.enabled {
            return Err(AssistError::disabled(
                "authoring assistance is disabled by provider config",
            ));
        }
        if raw.provider.kind != "http-json" {
            return Err(AssistError::disabled("provider kind must be `http-json`"));
        }
        validate_endpoint(&raw.provider.endpoint)?;
        validate_timeout(raw.provider.connect_timeout_ms, "connect")?;
        validate_timeout(raw.provider.timeout_ms, "overall")?;
        if raw.provider.connect_timeout_ms > raw.provider.timeout_ms {
            return Err(AssistError::disabled(
                "provider connect timeout cannot exceed overall timeout",
            ));
        }
        if let Some(name) = &raw.provider.credential_env
            && !valid_environment_name(name)
        {
            return Err(AssistError::disabled(
                "credential environment name is invalid",
            ));
        }
        Ok(Self {
            endpoint: raw.provider.endpoint,
            credential_env: raw.provider.credential_env,
            connect_timeout: Duration::from_millis(raw.provider.connect_timeout_ms),
            timeout: Duration::from_millis(raw.provider.timeout_ms),
        })
    }

    pub fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: "http-json".to_owned(),
            endpoint: self.endpoint.clone(),
            credential_source: self
                .credential_env
                .as_ref()
                .map(|name| format!("environment:{name}")),
        }
    }
}

impl SuggestionProvider for ProviderConfig {
    fn suggest(&self, request: &AssistRequest) -> Result<AssistResponse, AssistError> {
        let body = serde_json::to_vec(request)
            .map_err(|_| AssistError::provider("could not serialize provider request"))?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(AssistError::provider(
                "provider request exceeds the bounded request limit",
            ));
        }

        let mut easy = Easy::new();
        easy.url(&self.endpoint)
            .map_err(|_| AssistError::provider("provider endpoint is invalid"))?;
        easy.proxy("")
            .map_err(|_| AssistError::provider("could not disable inherited provider proxies"))?;
        easy.follow_location(false)
            .and_then(|()| easy.max_redirections(0))
            .map_err(|_| AssistError::provider("could not disable provider redirects"))?;
        easy.connect_timeout(self.connect_timeout)
            .and_then(|()| easy.timeout(self.timeout))
            .map_err(|_| AssistError::provider("could not configure provider timeouts"))?;
        easy.post(true)
            .and_then(|()| easy.post_fields_copy(&body))
            .map_err(|_| AssistError::provider("could not configure provider request"))?;
        easy.useragent(concat!("krit-assist/", env!("CARGO_PKG_VERSION")))
            .map_err(|_| AssistError::provider("could not configure provider user agent"))?;

        let mut headers = List::new();
        headers
            .append("Accept: application/json")
            .and_then(|()| headers.append("Content-Type: application/json"))
            .map_err(|_| AssistError::provider("could not configure provider headers"))?;
        let credential_header = self.credential_header()?;
        if let Some(header) = credential_header.as_ref() {
            headers
                .append(header)
                .map_err(|_| AssistError::provider("could not configure provider credential"))?;
        }
        easy.http_headers(headers)
            .map_err(|_| AssistError::provider("could not install provider headers"))?;

        let mut response = Vec::new();
        let mut exceeded = false;
        let perform = {
            let mut transfer = easy.transfer();
            transfer
                .write_function(|chunk| {
                    let Some(next) = response.len().checked_add(chunk.len()) else {
                        exceeded = true;
                        return Ok(0);
                    };
                    if next > MAX_PROVIDER_RESPONSE_BYTES {
                        exceeded = true;
                        return Ok(0);
                    }
                    response.extend_from_slice(chunk);
                    Ok(chunk.len())
                })
                .map_err(|_| AssistError::provider("could not bound provider response"))?;
            transfer.perform()
        };
        if exceeded {
            return Err(AssistError::provider(format!(
                "provider response exceeds the {MAX_PROVIDER_RESPONSE_BYTES}-byte limit"
            )));
        }
        perform.map_err(|_| AssistError::provider("provider request failed"))?;
        let status = easy
            .response_code()
            .map_err(|_| AssistError::provider("could not read provider status"))?;
        if !(200..=299).contains(&status) {
            return Err(AssistError::provider(format!(
                "provider returned HTTP status {status}"
            )));
        }
        decode_response(&response)
    }
}

impl ProviderConfig {
    fn credential_header(&self) -> Result<Option<Zeroizing<String>>, AssistError> {
        let Some(name) = &self.credential_env else {
            return Ok(None);
        };
        let credential =
            Zeroizing::new(env::var(name).map_err(|_| {
                AssistError::provider("configured provider credential is unavailable")
            })?);
        if credential.is_empty()
            || credential.len() > MAX_CREDENTIAL_BYTES
            || credential.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
        {
            return Err(AssistError::provider(
                "configured provider credential is invalid",
            ));
        }
        Ok(Some(Zeroizing::new(format!(
            "Authorization: Bearer {}",
            credential.as_str()
        ))))
    }
}

pub fn decode_response(bytes: &[u8]) -> Result<AssistResponse, AssistError> {
    if bytes.len() > MAX_PROVIDER_RESPONSE_BYTES {
        return Err(AssistError::provider(format!(
            "provider response exceeds the {MAX_PROVIDER_RESPONSE_BYTES}-byte limit"
        )));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| AssistError::provider("provider response is not strict schema-1 JSON"))
}

fn validate_endpoint(value: &str) -> Result<(), AssistError> {
    if value.len() > MAX_ENDPOINT_BYTES {
        return Err(AssistError::disabled("provider endpoint is too long"));
    }
    let endpoint = url::Url::parse(value)
        .map_err(|_| AssistError::disabled("provider endpoint is invalid"))?;
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(AssistError::disabled(
            "provider endpoint cannot contain credentials, query, or fragment",
        ));
    }
    match endpoint.scheme() {
        "https" => Ok(()),
        "http" if endpoint.host_str().is_some_and(is_loopback_host) => Ok(()),
        _ => Err(AssistError::disabled(
            "provider endpoint must use HTTPS or loopback HTTP",
        )),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_timeout(value: u64, label: &str) -> Result<(), AssistError> {
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&value) {
        return Err(AssistError::disabled(format!(
            "provider {label} timeout must be {MIN_TIMEOUT_MS}-{MAX_TIMEOUT_MS} milliseconds"
        )));
    }
    Ok(())
}

fn valid_environment_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}
