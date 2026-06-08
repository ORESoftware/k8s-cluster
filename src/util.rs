use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::HeaderMap;
use serde_json::Value;

pub(crate) fn clean_identifier(input: &str) -> Option<String> {
    let cleaned = input
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('$')
        .to_string();
    if cleaned.is_empty()
        || cleaned.len() > 128
        || !cleaned
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        None
    } else {
        Some(cleaned)
    }
}

pub(crate) fn clean_field(input: &str) -> Option<String> {
    let trimmed = input
        .trim()
        .trim_matches(',')
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('$');
    let suffix = trimmed
        .rsplit('.')
        .next()
        .unwrap_or(trimmed)
        .trim_matches(')')
        .trim_matches('(');
    clean_identifier(suffix)
}

pub(crate) fn find_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

pub(crate) fn scalar_to_label(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

pub(crate) fn env_flag(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub(crate) fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

pub(crate) fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn xml_escape(input: &str) -> String {
    html_escape(input).replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_ascii_and_safe() {
        assert_eq!(
            clean_identifier(" sales-lab "),
            Some("sales-lab".to_string())
        );
        assert_eq!(clean_field("table.revenue"), Some("revenue".to_string()));
        assert!(clean_identifier("bad value").is_none());
        assert!(clean_identifier("../bad").is_none());
    }

    #[test]
    fn scalar_labels_are_stable() {
        assert_eq!(scalar_to_label(&Value::from("north")), "north");
        assert_eq!(scalar_to_label(&Value::from(42)), "42");
        assert_eq!(scalar_to_label(&Value::Null), "null");
    }
}
