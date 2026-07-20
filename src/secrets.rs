//! Environment-first secret overlay backed by Fiducia KV.
//!
//! Only the NATS connection allowlist below is eligible. Process environment
//! always wins, so an external Vault/Secrets-Store CSI injector can populate
//! ordinary env vars without coupling the service to one cloud. When configured,
//! Fiducia fills only missing values from `secrets/daedalus/<ENV_NAME>`.
//! Fiducia transparently returns values stored as either encrypted or explicitly
//! plaintext KV entries; this module never logs or exposes their contents.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use reqwest::Url;
use serde_json::Value;

pub const MANAGED_KEYS: &[&str] = &["NATS_URL", "NATS_TOKEN", "NATS_NKEY"];
const MAX_SECRET_BYTES: usize = 64 * 1024;
/// Ceiling on a whole KV response body. Generous relative to a single 64 KiB
/// secret plus its JSON envelope, but bounded — the point is that "unbounded"
/// is not a size.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Default)]
pub struct SecretOverlay {
    values: HashMap<String, String>,
}

struct FiduciaClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

impl SecretOverlay {
    /// Load a Fiducia overlay when both configuration values are present. A
    /// partial or invalid configuration is an operator error, never permission
    /// to silently use a second source.
    pub async fn load() -> Result<Self, String> {
        let url = nonempty_env("FIDUCIA_URL");
        let api_key = kv_api_key();
        match (url, api_key) {
            (None, None) => Ok(Self::default()),
            (Some(url), Some(api_key)) => FiduciaClient::new(&url, api_key)?.load().await,
            _ => Err(
                "FIDUCIA_URL and FIDUCIA_API_KEY (or FIDUCIA_TOKEN) must be configured together \
                 for the Daedalus secret overlay"
                    .to_string(),
            ),
        }
    }

    /// Environment wins over Fiducia. Returned values are owned so callers can
    /// hand credentials to libraries without extending an overlay borrow.
    pub fn get(&self, name: &str) -> Option<String> {
        self.resolve_with(name, nonempty_env)
    }

    fn resolve_with<F>(&self, name: &str, env_lookup: F) -> Option<String>
    where
        F: FnOnce(&str) -> Option<String>,
    {
        env_lookup(name).or_else(|| self.values.get(name).cloned())
    }
}

impl FiduciaClient {
    fn new(raw_url: &str, api_key: String) -> Result<Self, String> {
        let base_url = normalize_base_url(raw_url)?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "could not construct the Fiducia HTTP client".to_string())?;
        Ok(Self {
            base_url,
            api_key,
            http,
        })
    }

    async fn load(&self) -> Result<SecretOverlay, String> {
        let mut values = HashMap::new();
        for name in MANAGED_KEYS {
            if nonempty_env(name).is_some() {
                continue;
            }
            if let Some(value) = self.lookup(name).await? {
                values.insert((*name).to_string(), value);
            }
        }
        Ok(SecretOverlay { values })
    }

    async fn lookup(&self, name: &str) -> Result<Option<String>, String> {
        let key = format!("secrets/daedalus/{name}");
        let response = self
            .http
            .get(format!("{}/v1/kv", self.base_url))
            .bearer_auth(&self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .query(&[("key", key.as_str())])
            .send()
            .await
            .map_err(|_| format!("Fiducia KV lookup failed for managed key {name}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "Fiducia KV lookup for managed key {name} returned HTTP {}",
                response.status().as_u16()
            ));
        }
        // Refuse an oversized body before buffering it. The MAX_SECRET_BYTES
        // check below runs on the parsed value, which is too late: `json()`
        // buffers the whole response first, so a compromised or misbehaving
        // endpoint could make this process allocate arbitrarily inside the 10s
        // timeout. Content-Length is advisory, so this is a cheap early reject,
        // not the only guard — the parsed-value check still applies.
        if let Some(declared) = response.content_length() {
            if declared > MAX_RESPONSE_BYTES {
                return Err(format!(
                    "Fiducia KV response for managed key {name} declared {declared} bytes, \
                     over the {MAX_RESPONSE_BYTES}-byte ceiling"
                ));
            }
        }
        let body = response
            .json::<Value>()
            .await
            .map_err(|_| format!("Fiducia KV response for managed key {name} was not JSON"))?;
        parse_lookup(name, &body)
    }
}

fn parse_lookup(name: &str, body: &Value) -> Result<Option<String>, String> {
    if body.get("found").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }
    let value = body
        .pointer("/entry/value")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Fiducia KV response for managed key {name} omitted entry.value"))?;
    if value.len() > MAX_SECRET_BYTES {
        return Err(format!(
            "Fiducia KV value for managed key {name} exceeds {MAX_SECRET_BYTES} bytes"
        ));
    }
    Ok(Some(value.to_string()))
}

/// The KV-read credential, under either of the two names the org uses.
///
/// The divergence is real and predates this function: this service and its
/// deployment manifests call it `FIDUCIA_API_KEY`, while the Daedalus MCP
/// server calls the same credential `FIDUCIA_TOKEN`. Accepting both — with
/// `FIDUCIA_API_KEY` winning when an operator has set both — means a pod that
/// inherits either spelling from a shared secret still gets its overlay, and
/// nobody has to remember which service reads which name.
///
/// This is deliberately *not* the credential used for `/v1/locks/*`; that path
/// needs the `locks:write` scope and reads `FIDUCIA_LOCKS_API_KEY`. See
/// [`crate::coordination`].
pub(crate) fn kv_api_key() -> Option<String> {
    nonempty_env("FIDUCIA_API_KEY").or_else(|| nonempty_env("FIDUCIA_TOKEN"))
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Validate and canonicalize a Fiducia base URL.
///
/// Shared with [`crate::coordination`] on purpose: the lock endpoint and the KV
/// endpoint are the same host, and two modules disagreeing about which hosts
/// are acceptable is exactly the gap the metadata ban below was written to
/// close.
pub(crate) fn normalize_base_url(raw: &str) -> Result<String, String> {
    let mut url = Url::parse(raw.trim()).map_err(|_| "FIDUCIA_URL is not valid".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "FIDUCIA_URL must include a host".to_string())?;
    // Metadata hosts are refused under BOTH schemes. Nothing legitimate serves
    // Fiducia there, and an https:// prefix is not evidence of anything.
    if is_metadata_host(host) {
        return Err("FIDUCIA_URL must not point at a cloud metadata endpoint".to_string());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || !matches!(url.scheme(), "http" | "https")
        || (url.scheme() == "http" && !cleartext_internal_host_allowed(host))
    {
        return Err(
            "FIDUCIA_URL must be HTTPS (or trusted internal HTTP) without credentials, path, query, or fragment"
                .to_string(),
        );
    }
    url.set_path("");
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// Hosts that must never be a Fiducia endpoint regardless of scheme.
///
/// Cloud instance-metadata services live on link-local addresses and on a few
/// well-known names, and they hand out role credentials to anything that can
/// reach them. This module previously *allowed* IPv4 link-local as "internal",
/// which meant `FIDUCIA_URL=http://169.254.169.254` was accepted — while the MCP
/// server's `safe_base_url` banned exactly that. Two modules in one org
/// disagreeing about whether link-local is trusted is the kind of gap that only
/// shows up once. They now agree: metadata endpoints are denied.
/// Parse a URL host as an IP address.
///
/// `Url::host_str()` returns an IPv6 literal wrapped in brackets (`[fe80::1]`),
/// which `IpAddr::from_str` rejects. Without stripping them every IPv6 branch
/// below silently never matches, and `[fe80::1]` instead falls through to the
/// "bare hostname, no dot" rule and is treated as a trusted internal host.
fn host_ip(host: &str) -> Option<IpAddr> {
    host.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<IpAddr>()
        .ok()
}

fn is_metadata_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if matches!(
        host.as_str(),
        "metadata.google.internal" | "metadata.goog" | "metadata.azure.internal" | "instance-data"
    ) {
        return true;
    }
    match host_ip(&host) {
        // 169.254.0.0/16 — AWS/GCP/Azure/OpenStack IMDS all live here.
        Some(IpAddr::V4(address)) => address.is_link_local() || address.is_unspecified(),
        // fe80::/10 link-local, plus the IPv6 IMDS address used by AWS.
        Some(IpAddr::V6(address)) => {
            let segments = address.segments();
            (segments[0] & 0xffc0) == 0xfe80
                || address.is_unspecified()
                || address == std::net::Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)
        }
        None => false,
    }
}

fn cleartext_internal_host_allowed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if is_metadata_host(&host) {
        return false;
    }
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Some(address) = host_ip(&host) {
        return match address {
            // Link-local is deliberately NOT allowed here — see is_metadata_host.
            IpAddr::V4(address) => address.is_loopback() || address.is_private(),
            IpAddr::V6(address) => {
                address.is_loopback() || (address.segments()[0] & 0xfe00) == 0xfc00
            }
        };
    }
    !host.contains('.') || host.ends_with(".svc") || host.ends_with(".svc.cluster.local")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_encrypted_and_plaintext_fiducia_values_without_policy_guessing() {
        let encrypted = json!({
            "found": true,
            "entry": {"value": "encrypted-result"},
            "protection": {"at_rest": "encrypted", "provider": "vault_transit", "key_version": 7}
        });
        let plaintext = json!({
            "found": true,
            "entry": {"value": "plaintext-result"},
            "protection": {"at_rest": "plaintext"}
        });
        assert_eq!(
            parse_lookup("NATS_TOKEN", &encrypted).unwrap().as_deref(),
            Some("encrypted-result")
        );
        assert_eq!(
            parse_lookup("NATS_TOKEN", &plaintext).unwrap().as_deref(),
            Some("plaintext-result")
        );
        assert_eq!(
            parse_lookup("NATS_TOKEN", &json!({"found": false})).unwrap(),
            None
        );
    }

    #[test]
    fn found_response_without_a_value_fails_closed() {
        let error = parse_lookup("NATS_TOKEN", &json!({"found": true}))
            .expect_err("found=true without a value must fail");
        assert!(error.contains("NATS_TOKEN"));
        assert!(!error.contains("entry\":{\"value"));
    }

    #[test]
    fn environment_precedence_is_explicit_and_allowlist_is_small() {
        let overlay = SecretOverlay {
            values: HashMap::from([("NATS_TOKEN".to_string(), "from-fiducia".to_string())]),
        };
        assert_eq!(
            overlay.resolve_with("NATS_TOKEN", |_| Some("from-environment".to_string())),
            Some("from-environment".to_string())
        );
        assert_eq!(MANAGED_KEYS, &["NATS_URL", "NATS_TOKEN", "NATS_NKEY"]);
    }

    #[test]
    fn rejects_public_cleartext_credentials_and_paths() {
        assert!(normalize_base_url("http://fiducia.example.com").is_err());
        assert!(normalize_base_url("https://user:pass@fiducia.example.com").is_err());
        assert!(normalize_base_url("https://fiducia.example.com/admin").is_err());
        assert_eq!(
            normalize_base_url("http://fiducia.default.svc:8080/").unwrap(),
            "http://fiducia.default.svc:8080"
        );
    }

    #[test]
    fn cloud_metadata_endpoints_are_refused_under_both_schemes() {
        // This module used to accept these: IPv4 link-local passed the
        // "internal host" test, so http://169.254.169.254 was a valid
        // FIDUCIA_URL — while the MCP server's validator banned it. An https
        // prefix is not evidence of anything either.
        for host in [
            "http://169.254.169.254",
            "https://169.254.169.254",
            "http://169.254.170.2",
            "https://metadata.google.internal",
            "http://metadata.google.internal",
            "https://metadata.azure.internal",
            "http://[fe80::1]",
            "http://[fd00:ec2::254]",
        ] {
            assert!(
                normalize_base_url(host).is_err(),
                "{host} must not be accepted as a Fiducia endpoint"
            );
        }
    }

    #[test]
    fn ordinary_internal_hosts_still_work_after_the_metadata_ban() {
        // The ban must not cost us legitimate in-cluster targets.
        for host in [
            "http://fiducia.default.svc.cluster.local:8088",
            "http://fiducia",
            "http://localhost:8088",
            "http://127.0.0.1:8088",
            "http://10.4.1.9:8088",
            "http://192.168.1.10",
            "https://fiducia.example.com",
        ] {
            assert!(
                normalize_base_url(host).is_ok(),
                "{host} should remain a valid Fiducia endpoint"
            );
        }
    }
}
