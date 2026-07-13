//! `domain_status`: registrar-side state via public RDAP plus live DNS via
//! DNS-over-HTTPS.
//!
//! Squarespace (where the domains are registered) has no public domains
//! API, so RDAP is the honest registrar-side integration. rdap.org
//! redirects to the authoritative RDAP server for the TLD.

use serde_json::{json, Value};

use super::error_chain;

/// Domain reported on when the `domain` parameter is omitted.
pub const DEFAULT_DOMAIN: &str = "canonical.cloud";

const RDAP_ROOT: &str = "https://rdap.org/domain";
const DOH_ROOT: &str = "https://cloudflare-dns.com/dns-query";

/// Validate a domain name parameter: bare DNS name, no scheme/path/flags.
pub fn validate_domain(domain: &str) -> Result<&str, String> {
    let trimmed = domain.trim().trim_end_matches('.');
    let valid = !trimmed.is_empty()
        && trimmed.len() <= 253
        && trimmed.contains('.')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !trimmed.starts_with('-');
    if valid {
        Ok(trimmed)
    } else {
        Err(format!("invalid domain name: {domain:?}"))
    }
}

fn vcard_fn(entity: &Value) -> Option<String> {
    let items = entity.get("vcardArray")?.get(1)?.as_array()?;
    items.iter().find_map(|item| {
        let parts = item.as_array()?;
        if parts.first()?.as_str()? == "fn" {
            Some(parts.get(3)?.as_str()?.to_string())
        } else {
            None
        }
    })
}

/// Registrar name from RDAP `entities` (the entity with role "registrar").
pub fn registrar_from_rdap(body: &Value) -> Option<String> {
    let entities = body.get("entities")?.as_array()?;
    entities
        .iter()
        .find(|entity| {
            entity
                .get("roles")
                .and_then(Value::as_array)
                .is_some_and(|roles| roles.iter().any(|role| role.as_str() == Some("registrar")))
        })
        .and_then(vcard_fn)
}

/// Compact `{action, date}` list from RDAP `events`.
pub fn events_from_rdap(body: &Value) -> Vec<Value> {
    let Some(events) = body.get("events").and_then(Value::as_array) else {
        return Vec::new();
    };
    events
        .iter()
        .filter_map(|event| {
            let action = event.get("eventAction")?.as_str()?;
            let date = event.get("eventDate")?.as_str()?;
            Some(json!({ "action": action, "date": date }))
        })
        .collect()
}

/// Nameserver host names from RDAP `nameservers`.
pub fn nameservers_from_rdap(body: &Value) -> Vec<String> {
    let Some(nameservers) = body.get("nameservers").and_then(Value::as_array) else {
        return Vec::new();
    };
    nameservers
        .iter()
        .filter_map(|ns| ns.get("ldhName").and_then(Value::as_str))
        .map(|name| name.to_lowercase())
        .collect()
}

/// Status codes from RDAP `status`.
pub fn status_from_rdap(body: &Value) -> Vec<String> {
    let Some(status) = body.get("status").and_then(Value::as_array) else {
        return Vec::new();
    };
    status
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Summarize an RDAP domain response into one compact JSON object.
pub fn summarize_rdap(body: &Value) -> Value {
    json!({
        "registrar": registrar_from_rdap(body),
        "status": status_from_rdap(body),
        "events": events_from_rdap(body),
        "nameservers": nameservers_from_rdap(body),
    })
}

/// Answer record data (`data` fields) from a dns-json response.
/// Absent `Answer` (e.g. SERVFAIL) yields an empty list.
pub fn doh_answers(body: &Value) -> Vec<String> {
    let Some(answers) = body.get("Answer").and_then(Value::as_array) else {
        return Vec::new();
    };
    answers
        .iter()
        .filter_map(|answer| answer.get("data").and_then(Value::as_str))
        .map(|data| data.trim_end_matches('.').to_lowercase())
        .collect()
}

/// DNS RCODE (`Status`) from a dns-json response, as text where known.
pub fn doh_rcode(body: &Value) -> String {
    match body.get("Status").and_then(Value::as_u64) {
        Some(0) => "NOERROR".to_string(),
        Some(2) => "SERVFAIL".to_string(),
        Some(3) => "NXDOMAIN".to_string(),
        Some(other) => format!("RCODE {other}"),
        None => "unknown".to_string(),
    }
}

/// Whether every delegated nameserver looks like Cloudflare
/// (`*.ns.cloudflare.com`). False when the list is empty.
pub fn is_cloudflare_delegation(nameservers: &[String]) -> bool {
    !nameservers.is_empty()
        && nameservers
            .iter()
            .all(|ns| ns.trim_end_matches('.').ends_with(".ns.cloudflare.com"))
}

async fn get_json(client: &reqwest::Client, url: &str, accept: &str) -> Result<Value, String> {
    let response = client
        .get(url)
        .header("Accept", accept)
        .send()
        .await
        .map_err(|error| format!("GET {url} failed: {}", error_chain(&error)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GET {url} returned {status}"));
    }
    response
        .json()
        .await
        .map_err(|error| format!("GET {url}: invalid JSON: {}", error_chain(&error)))
}

async fn doh_query(client: &reqwest::Client, domain: &str, kind: &str) -> Result<Value, String> {
    let url = format!("{DOH_ROOT}?name={domain}&type={kind}");
    get_json(client, &url, "application/dns-json").await
}

/// `domain_status`: RDAP registrar-side report plus live DNS delegation.
pub async fn domain_status_report(client: &reqwest::Client, domain: &str) -> Result<Value, String> {
    let domain = validate_domain(domain)?;

    let rdap = get_json(
        client,
        &format!("{RDAP_ROOT}/{domain}"),
        "application/rdap+json, application/json",
    )
    .await;
    let rdap_summary = match &rdap {
        Ok(body) => summarize_rdap(body),
        Err(error) => json!({ "error": error }),
    };

    let mut dns = serde_json::Map::new();
    let mut delegated_ns = Vec::new();
    for kind in ["NS", "A", "AAAA"] {
        match doh_query(client, domain, kind).await {
            Ok(body) => {
                let answers = doh_answers(&body);
                if kind == "NS" {
                    delegated_ns = answers.clone();
                }
                dns.insert(
                    kind.to_lowercase(),
                    json!({ "rcode": doh_rcode(&body), "answers": answers }),
                );
            }
            Err(error) => {
                dns.insert(kind.to_lowercase(), json!({ "error": error }));
            }
        }
    }

    Ok(json!({
        "domain": domain,
        "rdap": rdap_summary,
        "dns": dns,
        "cloudflare_delegated": is_cloudflare_delegation(&delegated_ns),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rdap_fixture() -> Value {
        json!({
            "ldhName": "example.cloud",
            "status": ["client transfer prohibited"],
            "entities": [
                {
                    "roles": ["registrant"],
                    "vcardArray": ["vcard", [["fn", {}, "text", "Someone Else"]]]
                },
                {
                    "roles": ["registrar"],
                    "vcardArray": [
                        "vcard",
                        [
                            ["version", {}, "text", "4.0"],
                            ["fn", {}, "text", "Squarespace Domains II LLC"]
                        ]
                    ]
                }
            ],
            "events": [
                { "eventAction": "registration", "eventDate": "2016-02-16T15:00:30Z" },
                { "eventAction": "expiration", "eventDate": "2028-02-16T15:00:30Z" },
                { "eventAction": "bogus" }
            ],
            "nameservers": [
                { "ldhName": "ANA.NS.CLOUDFLARE.COM" },
                { "ldhName": "bob.ns.cloudflare.com" }
            ]
        })
    }

    #[test]
    fn summarize_rdap_happy_path() {
        let summary = summarize_rdap(&rdap_fixture());
        assert_eq!(
            summary["registrar"].as_str(),
            Some("Squarespace Domains II LLC")
        );
        assert_eq!(summary["status"], json!(["client transfer prohibited"]));
        // The event missing its date is dropped, not fabricated.
        assert_eq!(
            summary["events"],
            json!([
                { "action": "registration", "date": "2016-02-16T15:00:30Z" },
                { "action": "expiration", "date": "2028-02-16T15:00:30Z" }
            ])
        );
        assert_eq!(
            summary["nameservers"],
            json!(["ana.ns.cloudflare.com", "bob.ns.cloudflare.com"])
        );
    }

    #[test]
    fn summarize_rdap_tolerates_missing_fields() {
        let summary = summarize_rdap(&json!({ "objectClassName": "domain" }));
        assert_eq!(summary["registrar"], Value::Null);
        assert_eq!(summary["status"], json!([]));
        assert_eq!(summary["events"], json!([]));
        assert_eq!(summary["nameservers"], json!([]));
    }

    #[test]
    fn doh_answers_happy_path() {
        let body = json!({
            "Status": 0,
            "Answer": [
                { "name": "x.cloud", "type": 2, "TTL": 3600, "data": "Ana.NS.Cloudflare.com." },
                { "name": "x.cloud", "type": 2, "TTL": 3600, "data": "bob.ns.cloudflare.com." }
            ]
        });
        assert_eq!(
            doh_answers(&body),
            vec!["ana.ns.cloudflare.com", "bob.ns.cloudflare.com"]
        );
        assert_eq!(doh_rcode(&body), "NOERROR");
    }

    #[test]
    fn doh_answers_tolerates_servfail_without_answer() {
        // Real shape observed from cloudflare-dns.com for a lame delegation.
        let body = json!({
            "Status": 2,
            "TC": false,
            "Question": [{ "name": "canonical.cloud", "type": 2 }],
            "Comment": ["EDE(22): No Reachable Authority"]
        });
        assert!(doh_answers(&body).is_empty());
        assert_eq!(doh_rcode(&body), "SERVFAIL");
        assert_eq!(doh_rcode(&json!({})), "unknown");
    }

    #[test]
    fn cloudflare_delegation_detection() {
        let cf = vec![
            "ana.ns.cloudflare.com".to_string(),
            "bob.ns.cloudflare.com".to_string(),
        ];
        assert!(is_cloudflare_delegation(&cf));

        let mixed = vec![
            "ana.ns.cloudflare.com".to_string(),
            "ns1.canonical.com".to_string(),
        ];
        assert!(!is_cloudflare_delegation(&mixed));
        assert!(!is_cloudflare_delegation(&[]));
    }

    #[test]
    fn validate_domain_accepts_names_and_rejects_junk() {
        assert_eq!(validate_domain("canonical.cloud"), Ok("canonical.cloud"));
        assert_eq!(validate_domain("Canonical.Cloud."), Ok("Canonical.Cloud"));
        assert!(validate_domain("").is_err());
        assert!(validate_domain("nodots").is_err());
        assert!(validate_domain("https://canonical.cloud").is_err());
        assert!(validate_domain("-flag.example.com").is_err());
        assert!(validate_domain("a b.com").is_err());
    }
}
