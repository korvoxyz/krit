use serde::{Deserialize, Serialize};

use crate::{
    AiAdapterConfig, HttpHeader, HttpJsonAdapterConfig, HttpRequest, HttpResponse, RuntimeError,
};

pub(crate) trait AiAdapter {
    fn origin(&self) -> &str;
    fn secret_name(&self) -> Option<&str>;
    fn timeout(&self) -> std::time::Duration;
    fn max_input_bytes(&self) -> usize;
    fn max_response_bytes(&self) -> usize;
    fn build_request(&self, input: &str, idempotency_key: &str) -> Result<HttpRequest, String>;
    fn parse_response(&self, response: HttpResponse) -> Result<String, String>;
}

pub(crate) enum Adapter {
    HttpJson(HttpJsonAdapter),
}

impl Adapter {
    pub(crate) fn from_config(config: &AiAdapterConfig) -> Result<Self, RuntimeError> {
        match config {
            AiAdapterConfig::HttpJson(config) => {
                Ok(Self::HttpJson(HttpJsonAdapter::new(config.clone())))
            }
        }
    }
}

impl AiAdapter for Adapter {
    fn origin(&self) -> &str {
        match self {
            Self::HttpJson(adapter) => adapter.origin(),
        }
    }

    fn secret_name(&self) -> Option<&str> {
        match self {
            Self::HttpJson(adapter) => adapter.secret_name(),
        }
    }

    fn timeout(&self) -> std::time::Duration {
        match self {
            Self::HttpJson(adapter) => adapter.timeout(),
        }
    }

    fn max_input_bytes(&self) -> usize {
        match self {
            Self::HttpJson(adapter) => adapter.max_input_bytes(),
        }
    }

    fn max_response_bytes(&self) -> usize {
        match self {
            Self::HttpJson(adapter) => adapter.max_response_bytes(),
        }
    }

    fn build_request(&self, input: &str, idempotency_key: &str) -> Result<HttpRequest, String> {
        match self {
            Self::HttpJson(adapter) => adapter.build_request(input, idempotency_key),
        }
    }

    fn parse_response(&self, response: HttpResponse) -> Result<String, String> {
        match self {
            Self::HttpJson(adapter) => adapter.parse_response(response),
        }
    }
}

pub(crate) struct HttpJsonAdapter {
    config: HttpJsonAdapterConfig,
}

impl HttpJsonAdapter {
    fn new(config: HttpJsonAdapterConfig) -> Self {
        Self { config }
    }
}

#[derive(Serialize)]
struct HttpJsonRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpJsonResponse {
    output: String,
}

impl AiAdapter for HttpJsonAdapter {
    fn origin(&self) -> &str {
        &self.config.origin
    }

    fn secret_name(&self) -> Option<&str> {
        self.config.secret.as_deref()
    }

    fn timeout(&self) -> std::time::Duration {
        self.config.timeout
    }

    fn max_input_bytes(&self) -> usize {
        self.config.max_input_bytes
    }

    fn max_response_bytes(&self) -> usize {
        self.config.max_response_bytes
    }

    fn build_request(&self, input: &str, idempotency_key: &str) -> Result<HttpRequest, String> {
        let body = serde_json::to_string(&HttpJsonRequest {
            model: &self.config.model,
            input,
        })
        .map_err(|_| "AI adapter could not encode its provider request".to_owned())?;
        Ok(HttpRequest {
            method: "POST".to_owned(),
            path: self.config.path.clone(),
            query: String::new(),
            headers: vec![
                HttpHeader {
                    name: "content-type".to_owned(),
                    value: "application/json".to_owned(),
                },
                HttpHeader {
                    name: "idempotency-key".to_owned(),
                    value: idempotency_key.to_owned(),
                },
            ],
            body,
        })
    }

    fn parse_response(&self, response: HttpResponse) -> Result<String, String> {
        if !(200..=299).contains(&response.status) {
            return Err(format!(
                "AI provider returned HTTP status {}",
                response.status
            ));
        }
        if response.body.len() > self.config.max_response_bytes {
            return Err("AI provider response exceeded the configured size limit".to_owned());
        }
        let parsed: HttpJsonResponse = serde_json::from_str(&response.body)
            .map_err(|_| "AI provider response was not valid strict http-json output".to_owned())?;
        if parsed.output.len() > self.config.max_response_bytes {
            return Err("AI model output exceeded the configured size limit".to_owned());
        }
        Ok(parsed.output)
    }
}
