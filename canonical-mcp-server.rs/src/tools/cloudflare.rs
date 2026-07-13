//! `cloudflare_dns`: read-only DNS record listing via the Cloudflare API.
//!
//! Requires `CLOUDFLARE_API_TOKEN`. Strictly read-only: only zone lookup
//! and record listing are implemented — no write/update endpoints.

use serde::Serialize;
use serde_json::Value;

use super::error_chain;

const API_ROOT: &str = "https://api.cloudflare.com/client/v4";

/// Environment variable holding the Cloudflare API token.
pub const TOKEN_ENV: &str = "CLOUDFLARE_API_TOKEN";

/// Compact summary of one DNS record.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DnsRecord {
    pub r#type: String,
    pub name: String,
    pub content: String,
    pub proxied: Option<bool>,
    pub ttl: Option<u64>,
}

/// Collect `errors[].message` from a Cloudflare `success: false` response.
pub fn cf_error_messages(body: &Value) -> Vec<String> {
    let Some(errors) = body.get("errors").and_then(Value::as_array) else {
        return Vec::new();
    };
    errors
        .iter()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn ensure_success(body: &Value, what: &str) -> Result<(), String> {
    if body.get("success").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!(
            "Cloudflare API {what} failed: {:?}",
            cf_error_messages(body)
        ))
    }
}

/// Extract the zone id from a `GET /zones?name={domain}` response.
pub fn zone_id_from_response(body: &Value) -> Option<String> {
    body.get("result")?
        .as_array()?
        .first()?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// Summarize a `GET /zones/{id}/dns_records` response body.
pub fn summarize_dns_records(body: &Value) -> Vec<DnsRecord> {
    let Some(records) = body.get("result").and_then(Value::as_array) else {
        return Vec::new();
    };
    records
        .iter()
        .filter_map(|record| {
            Some(DnsRecord {
                r#type: record.get("type")?.as_str()?.to_string(),
                name: record.get("name")?.as_str()?.to_string(),
                content: record
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                proxied: record.get("proxied").and_then(Value::as_bool),
                ttl: record.get("ttl").and_then(Value::as_u64),
            })
        })
        .collect()
}

async fn get_json(client: &reqwest::Client, token: &str, path: &str) -> Result<Value, String> {
    let url = format!("{API_ROOT}{path}");
    let response = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("GET {url} failed: {}", error_chain(&error)))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("GET {url}: invalid JSON: {}", error_chain(&error)))?;
    if !status.is_success() {
        return Err(format!(
            "GET {url} returned {status}: {:?}",
            cf_error_messages(&body)
        ));
    }
    Ok(body)
}

/// `cloudflare_dns`: list DNS records for the zone matching `domain`.
pub async fn dns_records_report(client: &reqwest::Client, domain: &str) -> Result<Value, String> {
    let domain = super::domain::validate_domain(domain)?;
    let Ok(token) = std::env::var(TOKEN_ENV) else {
        return Err(format!(
            "{TOKEN_ENV} is not set. Create a read-only Cloudflare API token \
             (Zone.Zone:Read, Zone.DNS:Read) and export it as {TOKEN_ENV}."
        ));
    };

    let zones = get_json(client, &token, &format!("/zones?name={domain}")).await?;
    ensure_success(&zones, "zone lookup")?;
    let Some(zone_id) = zone_id_from_response(&zones) else {
        return Err(format!(
            "no Cloudflare zone named {domain:?} is visible to this token"
        ));
    };

    let records = get_json(
        client,
        &token,
        &format!("/zones/{zone_id}/dns_records?per_page=100"),
    )
    .await?;
    ensure_success(&records, "dns_records listing")?;

    serde_json::to_value(serde_json::json!({
        "domain": domain,
        "zone_id": zone_id,
        "records": summarize_dns_records(&records),
    }))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zone_id_extraction_happy_path() {
        let body = json!({
            "success": true,
            "errors": [],
            "result": [{ "id": "023e105f4ecef8ad9ca31a8372d0c353", "name": "canonical.cloud" }]
        });
        assert_eq!(
            zone_id_from_response(&body).as_deref(),
            Some("023e105f4ecef8ad9ca31a8372d0c353")
        );
    }

    #[test]
    fn zone_id_extraction_handles_empty_or_malformed() {
        assert_eq!(
            zone_id_from_response(&json!({ "success": true, "result": [] })),
            None
        );
        assert_eq!(zone_id_from_response(&json!({ "result": "nope" })), None);
        assert_eq!(zone_id_from_response(&json!({})), None);
    }

    #[test]
    fn summarize_dns_records_happy_path() {
        let body = json!({
            "success": true,
            "result": [
                {
                    "type": "A",
                    "name": "canonical.cloud",
                    "content": "203.0.113.7",
                    "proxied": true,
                    "ttl": 1
                },
                {
                    "type": "TXT",
                    "name": "_dmarc.canonical.cloud",
                    "content": "v=DMARC1; p=reject",
                    "ttl": 300
                }
            ]
        });
        let records = summarize_dns_records(&body);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].r#type, "A");
        assert_eq!(records[0].content, "203.0.113.7");
        assert_eq!(records[0].proxied, Some(true));
        assert_eq!(records[1].proxied, None);
        assert_eq!(records[1].ttl, Some(300));
    }

    #[test]
    fn summarize_dns_records_skips_malformed_entries() {
        let body = json!({
            "result": [
                { "name": "missing-type.example" },
                { "type": "A", "name": "ok.example", "content": "192.0.2.1" }
            ]
        });
        let records = summarize_dns_records(&body);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "ok.example");
        assert!(summarize_dns_records(&json!({})).is_empty());
    }

    #[test]
    fn cf_error_messages_extraction() {
        let body = json!({
            "success": false,
            "errors": [
                { "code": 10000, "message": "Authentication error" },
                { "code": 7003, "message": "Could not route" }
            ]
        });
        assert_eq!(
            cf_error_messages(&body),
            vec!["Authentication error", "Could not route"]
        );
        assert!(cf_error_messages(&json!({})).is_empty());
    }
}
