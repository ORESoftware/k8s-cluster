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
        let api_key = nonempty_env("FIDUCIA_API_KEY");
        match (url, api_key) {
            (None, None) => Ok(Self::default()),
            (Some(url), Some(api_key)) => FiduciaClient::new(&url, api_key)?.load().await,
            _ => Err(
                "FIDUCIA_URL and FIDUCIA_API_KEY must be configured together for the Daedalus secret overlay"
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

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_base_url(raw: &str) -> Result<String, String> {
    let mut url = Url::parse(raw.trim()).map_err(|_| "FIDUCIA_URL is not valid".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "FIDUCIA_URL must include a host".to_string())?;
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

fn cleartext_internal_host_allowed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(address) => {
                address.is_loopback() || address.is_private() || address.is_link_local()
            }
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
}
