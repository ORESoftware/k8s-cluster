use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{MAX_LIST_ITEMS, MAX_SHORT_TEXT_LEN, MAX_TOKEN_LEN};

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn request_id(input: Option<&String>, fallback: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

pub(crate) fn clean_text(value: &str, max_len: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .take(max_len)
        .collect()
}

pub(crate) fn clean_optional(value: Option<String>, max_len: usize) -> Option<String> {
    value
        .map(|item| clean_text(&item, max_len))
        .filter(|item| !item.is_empty())
}

pub(crate) fn split_lines(value: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for raw in value.lines().flat_map(|line| line.split(';')) {
        let item = clean_text(raw, MAX_SHORT_TEXT_LEN);
        if !item.is_empty() && seen.insert(item.to_ascii_lowercase()) {
            items.push(item);
        }
        if items.len() >= MAX_LIST_ITEMS {
            break;
        }
    }
    items
}

pub(crate) fn slugify(value: &str) -> String {
    let slug = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

pub(crate) fn normalize_track(value: Option<&String>) -> String {
    let label = value
        .map(|item| item.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "provisional".to_string());
    match label.as_str() {
        "provisional" | "non-provisional" | "design" | "pct" => label,
        _ => "provisional".to_string(),
    }
}
