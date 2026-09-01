use serde::Serialize;

use crate::{RuntimeLimits, SecretStore};

pub const MAX_LOG_NAME_BYTES: usize = 64;
pub const REDACTED_VALUE: &str = "[REDACTED]";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogField {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogEvent {
    pub sequence: u64,
    pub level: LogLevel,
    pub event: String,
    pub fields: Vec<LogField>,
}

pub(crate) fn validate_and_redact(
    sequence: u64,
    level: LogLevel,
    event: String,
    fields: Vec<LogField>,
    secrets: &SecretStore,
    limits: RuntimeLimits,
    current_bytes: usize,
) -> Result<(LogEvent, usize), String> {
    if !krit_capability::is_valid_resource_name(&event) || event.len() > MAX_LOG_NAME_BYTES {
        return Err("log event name is invalid".to_owned());
    }
    if fields.len() > limits.log_fields() {
        return Err(format!(
            "log field count exceeds the {}-field limit",
            limits.log_fields()
        ));
    }
    let mut encoded_bytes = event.len();
    let mut redacted = Vec::new();
    redacted
        .try_reserve(fields.len())
        .map_err(|_| "log field allocation exceeded host resources".to_owned())?;
    for field in fields {
        if !valid_field_name(&field.name) {
            return Err("log field name is invalid".to_owned());
        }
        if field.value.len() > limits.log_value_bytes() {
            return Err(format!(
                "log field value exceeds the {}-byte limit",
                limits.log_value_bytes()
            ));
        }
        encoded_bytes = encoded_bytes
            .checked_add(field.name.len())
            .and_then(|bytes| bytes.checked_add(field.value.len()))
            .ok_or_else(|| "log byte count overflowed".to_owned())?;
        let value =
            if redacted_key(&field.name) || secrets.contains_exact_value(field.value.as_bytes()) {
                REDACTED_VALUE.to_owned()
            } else {
                field.value
            };
        redacted.push(LogField {
            name: field.name,
            value,
        });
    }
    let total = current_bytes
        .checked_add(encoded_bytes)
        .ok_or_else(|| "log byte count overflowed".to_owned())?;
    if total > limits.log_bytes() {
        return Err(format!(
            "structured logs exceed the {}-byte invocation limit",
            limits.log_bytes()
        ));
    }
    Ok((
        LogEvent {
            sequence,
            level,
            event,
            fields: redacted,
        },
        total,
    ))
}

fn redacted_key(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace('_', "-");
    normalized.contains("token")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("api-key")
}

fn valid_field_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_LOG_NAME_BYTES
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && name
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn redacts_sensitive_keys_and_exact_secret_values_without_reordering() {
        let secrets = SecretStore::new(BTreeMap::from([(
            "service-token".to_owned(),
            b"private-value".to_vec(),
        )]))
        .expect("secret store should be valid");
        let (event, _) = validate_and_redact(
            0,
            LogLevel::Info,
            "request.started".to_owned(),
            vec![
                LogField {
                    name: "api_key".to_owned(),
                    value: "ordinary".to_owned(),
                },
                LogField {
                    name: "message".to_owned(),
                    value: "private-value".to_owned(),
                },
            ],
            &secrets,
            RuntimeLimits::default(),
            0,
        )
        .expect("log should validate");
        assert_eq!(event.fields[0].value, REDACTED_VALUE);
        assert_eq!(event.fields[1].value, REDACTED_VALUE);
    }

    #[test]
    fn rejects_invalid_names_and_limits_before_buffering() {
        let secrets = SecretStore::default();
        let invalid = validate_and_redact(
            0,
            LogLevel::Info,
            "Invalid".to_owned(),
            Vec::new(),
            &secrets,
            RuntimeLimits::default(),
            0,
        )
        .expect_err("invalid event must fail");
        assert!(invalid.contains("event name"));

        let mut limits = RuntimeLimits::default();
        limits
            .narrow_log_fields(0)
            .expect("field count should narrow");
        let limited = validate_and_redact(
            0,
            LogLevel::Info,
            "valid.event".to_owned(),
            vec![LogField {
                name: "field".to_owned(),
                value: "value".to_owned(),
            }],
            &secrets,
            limits,
            0,
        )
        .expect_err("field limit must fail");
        assert!(limited.contains("field count"));
    }
}
