//! `service_health`: probe a deployment's health endpoints.

use std::time::Duration;

use serde::Serialize;

/// Endpoints probed on the target deployment.
pub const HEALTH_PATHS: [&str; 3] = ["/healthz", "/readyz", "/api/v1/health"];

/// Per-request timeout for health probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of characters of response body returned per endpoint.
pub const MAX_BODY_CHARS: usize = 1024;

/// Result of probing one endpoint.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct EndpointReport {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Validate that a base URL is plain http(s) without trailing junk we
/// cannot reason about.
pub fn validate_base_url(base_url: &str) -> Result<&str, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed)
    } else {
        Err(format!(
            "base_url must start with http:// or https://, got {base_url:?}"
        ))
    }
}

/// Truncate a response body on a character boundary, marking elision.
pub fn truncate_body(body: &str, max_chars: usize) -> String {
    if body.chars().count() <= max_chars {
        return body.to_string();
    }
    let truncated: String = body.chars().take(max_chars).collect();
    format!("{truncated}… [truncated]")
}

/// Probe every health endpoint under `base_url` and collect reports.
pub async fn probe(
    client: &reqwest::Client,
    base_url: &str,
) -> Result<Vec<EndpointReport>, String> {
    let base = validate_base_url(base_url)?;
    let mut reports = Vec::with_capacity(HEALTH_PATHS.len());
    for path in HEALTH_PATHS {
        let url = format!("{base}{path}");
        let outcome = client.get(&url).timeout(PROBE_TIMEOUT).send().await;
        let report = match outcome {
            Ok(response) => {
                let status = response.status().as_u16();
                match response.text().await {
                    Ok(body) => EndpointReport {
                        url,
                        status: Some(status),
                        body: Some(truncate_body(&body, MAX_BODY_CHARS)),
                        error: None,
                    },
                    Err(error) => EndpointReport {
                        url,
                        status: Some(status),
                        body: None,
                        error: Some(format!(
                            "error reading body: {}",
                            super::error_chain(&error)
                        )),
                    },
                }
            }
            Err(error) => EndpointReport {
                url,
                status: None,
                body: None,
                error: Some(super::error_chain(&error)),
            },
        };
        reports.push(report);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_base_url_accepts_http_and_trims_trailing_slash() {
        assert_eq!(
            validate_base_url("https://canonical.cloud/"),
            Ok("https://canonical.cloud")
        );
        assert_eq!(
            validate_base_url("http://127.0.0.1:8081"),
            Ok("http://127.0.0.1:8081")
        );
    }

    #[test]
    fn validate_base_url_rejects_other_schemes() {
        assert!(validate_base_url("ftp://x").is_err());
        assert!(validate_base_url("canonical.cloud").is_err());
        assert!(validate_base_url("").is_err());
    }

    #[test]
    fn truncate_body_is_char_boundary_safe() {
        assert_eq!(truncate_body("ok", 10), "ok");
        assert_eq!(truncate_body("abcdef", 3), "abc… [truncated]");
        // Multibyte characters must not be split.
        assert_eq!(truncate_body("ééééé", 2), "éé… [truncated]");
    }

    #[test]
    fn truncate_body_exact_limit_is_untouched() {
        assert_eq!(truncate_body("abc", 3), "abc");
    }
}
