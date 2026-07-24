use std::{
    env,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

pub(crate) fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

pub(crate) fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn env_u16(key: &str, fallback: u16) -> u16 {
    first_env(&[key])
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn env_usize(key: &str, fallback: usize) -> usize {
    first_env(&[key])
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn safe_slug(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 120
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

pub(crate) fn safe_env_key(input: &str) -> bool {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub(crate) fn safe_local_path(input: &str) -> bool {
    input.starts_with('/')
        && !input.starts_with("//")
        && !input.contains("://")
        && !input.contains('?')
        && !input.contains('#')
        && input.len() <= 256
        && !input
            .bytes()
            .any(|byte| byte <= 0x20 || byte == 0x7f || byte == b'\\')
        && !input
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
}

pub(crate) fn safe_container_image(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 512
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'@' | b'-')
        })
}

pub(crate) fn safe_config_id(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 120
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

pub(crate) fn safe_network_name(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

pub(crate) fn safe_resource_value(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_digit()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'.')
}

pub(crate) fn safe_nats_subject(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 256
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'*' | b'>')
        })
}

pub(crate) fn safe_nats_queue_group(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn safe_env_value(input: &str) -> bool {
    input.len() <= 8192 && !input.contains('\0')
}

fn string_vec_from_json(value: Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn command_vec_from_json(value: Value) -> Vec<String> {
    string_vec_from_json(value)
        .into_iter()
        .filter(|value| !value.contains('\0') && value.len() <= 512)
        .take(32)
        .collect()
}

pub(crate) fn json_string_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<String> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn json_u64_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<u64> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_u64)
}

pub(crate) fn json_bool_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<bool> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_bool)
}

pub(crate) fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn clamp_i32_to_usize(value: i32, fallback: usize, min: usize, max: usize) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|value| *value >= min)
        .unwrap_or(fallback)
        .min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nats_queue_groups_are_bounded_and_wildcard_free() {
        assert!(safe_nats_queue_group("dd-container-pool.v1"));
        assert!(!safe_nats_queue_group("dd.container.*"));
        assert!(!safe_nats_queue_group("contains whitespace"));
        assert!(!safe_nats_queue_group(""));
    }
}
