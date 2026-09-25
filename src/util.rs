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

    #[test]
    fn clean_identifier_enforces_length_and_charset() {
        let max = "a".repeat(128);
        assert_eq!(clean_identifier(&max), Some(max.clone()));
        assert!(clean_identifier(&"a".repeat(129)).is_none());
        assert_eq!(clean_identifier("\"quoted\""), Some("quoted".to_string()));
        assert_eq!(
            clean_identifier("ns:table.col_1-x"),
            Some("ns:table.col_1-x".to_string())
        );
        assert!(clean_identifier("   ").is_none());
        assert!(clean_identifier("semi;colon").is_none());
        assert!(clean_identifier("emoji\u{1F600}").is_none());
    }

    #[test]
    fn clean_field_extracts_terminal_segment() {
        assert_eq!(
            clean_field("schema.table.revenue"),
            Some("revenue".to_string())
        );
        assert_eq!(clean_field("revenue,"), Some("revenue".to_string()));
        assert_eq!(clean_field("(region)"), Some("region".to_string()));
        assert_eq!(
            clean_field("\"schema\".\"col\""),
            Some("col".to_string())
        );
        assert!(clean_field("").is_none());
        assert!(clean_field("a.b c").is_none());
    }

    #[test]
    fn escaping_covers_html_and_xml_metacharacters() {
        assert_eq!(
            html_escape("<a href=\"x\">&"),
            "&lt;a href=&quot;x&quot;&gt;&amp;"
        );
        assert_eq!(html_escape("it's"), "it's");
        assert_eq!(xml_escape("it's <b>"), "it&apos;s &lt;b&gt;");
        assert_eq!(html_escape("plain text"), "plain text");
    }

    #[test]
    fn round4_and_case_insensitive_search_behave() {
        assert_eq!(round4(2.0 / 3.0), 0.6667);
        assert_eq!(round4(1.00006), 1.0001);
        assert_eq!(round4(1.00004), 1.0);
        assert_eq!(round4(-2.0 / 3.0), -0.6667);
        assert_eq!(round4(3.0), 3.0);

        assert_eq!(find_ascii_case("SELECT * FROM Sales", "from"), Some(9));
        assert_eq!(find_ascii_case("abc", "ABC"), Some(0));
        assert_eq!(find_ascii_case("abc", "abcd"), None);
    }
}
