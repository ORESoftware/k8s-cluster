use std::{
    collections::BTreeSet,
    net::IpAddr,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;

use crate::state::{MAX_TAGS, MAX_TEXT_LEN, MAX_TOKEN_LEN};
use crate::types::{PipelineOptions, ScrapeRequest, UiScrapeForm};

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

pub(crate) fn clean_text(value: Option<&String>, max_len: usize) -> Option<String> {
    value
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(max_len)
                .collect()
        })
}

pub(crate) fn clean_required(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_TOKEN_LEN {
        return Err(format!("{label} must be at most {MAX_TOKEN_LEN} bytes"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn clean_tags(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let normalized = value
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_' || *ch == ' ')
            .take(80)
            .collect::<String>();
        let normalized = normalized.split_whitespace().collect::<Vec<_>>().join("-");
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            out.push(normalized);
        }
        if out.len() >= MAX_TAGS {
            break;
        }
    }
    out
}

pub(crate) fn form_text(value: Option<String>, max_len: usize) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(max_len).collect())
}

pub(crate) fn form_csv(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

impl UiScrapeForm {
    pub(crate) fn into_scrape_request(self) -> Result<ScrapeRequest, String> {
        let url = self.url.trim();
        if url.is_empty() {
            return Err("url must not be empty".to_string());
        }
        if url.len() > MAX_TEXT_LEN {
            return Err(format!("url must be at most {MAX_TEXT_LEN} bytes"));
        }
        if url.chars().any(char::is_control) {
            return Err("url must not contain control characters".to_string());
        }
        let url = url.to_string();
        validate_public_url(&url)?;
        let source = form_text(self.source, MAX_TOKEN_LEN)
            .map(|value| clean_required(&value, "source"))
            .transpose()?
            .unwrap_or_else(|| "operator-scrape".to_string());
        let dataset_id = form_text(self.dataset_id, MAX_TOKEN_LEN)
            .map(|value| clean_required(&value, "datasetId"))
            .transpose()?;
        let strategy = form_text(self.strategy, MAX_TOKEN_LEN);
        let selector = form_text(self.selector, MAX_TOKEN_LEN);
        let tags = clean_tags(form_csv(self.tags));
        let pipeline = self.pipeline_enabled.as_ref().map(|_| PipelineOptions {
            enabled: Some(true),
            job_type: Some("spark-etl".to_string()),
            sink: dataset_id
                .as_ref()
                .map(|dataset| format!("minio://public-data/bronze/{dataset}")),
            airflow_dag: None,
            spark_app: Some("public-data-normalize".to_string()),
            parameters: Some(json!({ "submittedBy": "public-data-ui" })),
        });
        Ok(ScrapeRequest {
            request_id: None,
            source,
            url,
            dataset_id,
            strategy,
            render_javascript: Some(self.render_javascript.is_some()),
            selector,
            selectors: None,
            include_links: Some(self.include_links.is_some()),
            tags: if tags.is_empty() { None } else { Some(tags) },
            pipeline,
        })
    }
}

pub(crate) fn durable_token(prefix: &str, source: &str, suffix: &str) -> String {
    let source = source
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let source = if source.is_empty() {
        "unknown".to_string()
    } else {
        source
    };
    format!("{prefix}-{source}-{suffix}")
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

pub(crate) fn validate_public_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("invalid url: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("url scheme must be http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("url credentials are not allowed".to_string());
    }
    let Some(host) = url.host_str() else {
        return Err("url must include a host".to_string());
    };
    if blocked_public_data_host(host) {
        return Err("private or local targets are not allowed".to_string());
    }
    Ok(())
}

pub(crate) fn blocked_public_data_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(addr)) => {
            addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_broadcast()
                || addr.is_documentation()
                || addr.is_unspecified()
        }
        Ok(IpAddr::V6(addr)) => {
            addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_multicast()
        }
        Err(_) => false,
    }
}
