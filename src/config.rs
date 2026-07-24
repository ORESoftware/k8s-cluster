//! Environment-driven configuration (flags-2-env applies `.cli-flags.toml`
//! overrides before this reads). Secrets are environment-only.

use anyhow::Context;
use serde::Deserialize;

/// One NATS→webhook delivery route: everything arriving on `subject` (NATS
/// wildcards allowed here) is POSTed to `webhook`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DeliveryRoute {
    pub subject: String,
    pub webhook: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: std::net::SocketAddr,
    /// Cluster NATS, e.g. `nats://dd-nats.messaging.svc.cluster.local:4222`.
    pub nats_url: String,
    /// Only subjects under this prefix may be published through the bridge.
    pub subject_prefix: String,
    /// Bearer token required on `POST /publish` (internal callers only).
    pub internal_token: String,
    /// NATS→HTTP fan-out routes (`BRIDGE_DELIVERIES`, JSON array).
    pub deliveries: Vec<DeliveryRoute>,
    /// Max accepted publish payload, bytes.
    pub max_payload_bytes: usize,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let internal_token = std::env::var("BRIDGE_INTERNAL_TOKEN")
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
            .context("BRIDGE_INTERNAL_TOKEN is required — /publish is a mutating surface")?;
        anyhow::ensure!(
            internal_token.len() >= 16,
            "BRIDGE_INTERNAL_TOKEN must be at least 16 characters"
        );

        let deliveries_raw = env_or("BRIDGE_DELIVERIES", "[]");
        let deliveries: Vec<DeliveryRoute> = serde_json::from_str(&deliveries_raw)
            .context("BRIDGE_DELIVERIES must be a JSON array of {subject, webhook}")?;
        for route in &deliveries {
            anyhow::ensure!(
                route.webhook.starts_with("http://") || route.webhook.starts_with("https://"),
                "delivery webhook must be http(s): {}",
                route.webhook
            );
            // Delivery subscriptions may use wildcards, but still only under
            // the bridge's own prefix — this bridge is not a generic NATS tap.
            anyhow::ensure!(
                route
                    .subject
                    .starts_with(env_or("BRIDGE_SUBJECT_PREFIX", "shared-auth.").as_str()),
                "delivery subject must stay under the bridge prefix: {}",
                route.subject
            );
        }

        Ok(Self {
            bind_addr: env_or("BRIDGE_BIND_ADDR", "0.0.0.0:8121")
                .parse()
                .context("BRIDGE_BIND_ADDR")?,
            nats_url: env_or(
                "BRIDGE_NATS_URL",
                "nats://dd-nats.messaging.svc.cluster.local:4222",
            ),
            subject_prefix: env_or("BRIDGE_SUBJECT_PREFIX", "shared-auth."),
            internal_token,
            deliveries,
            max_payload_bytes: 64 * 1024,
        })
    }
}

/// Validate a subject for PUBLISH: under the prefix, dot-separated
/// `[A-Za-z0-9_-]` tokens, no wildcards (a publisher must name one subject).
pub fn validate_publish_subject(subject: &str, prefix: &str) -> Result<(), &'static str> {
    if subject.len() > 255 {
        return Err("subject too long");
    }
    if !subject.starts_with(prefix) {
        return Err("subject outside the bridge prefix");
    }
    if subject.ends_with('.') {
        return Err("subject must not end with a dot");
    }
    for token in subject.split('.') {
        if token.is_empty() {
            return Err("empty subject token");
        }
        if token == "*" || token == ">" {
            return Err("wildcards are not publishable");
        }
        if !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("invalid subject characters");
        }
    }
    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_subject_rules() {
        let p = "shared-auth.";
        assert!(validate_publish_subject("shared-auth.events.identity", p).is_ok());
        assert!(validate_publish_subject("shared-auth.sync.outbox_flush", p).is_ok());
        // outside prefix
        assert!(validate_publish_subject("dd.events.x", p).is_err());
        // wildcards are never publishable
        assert!(validate_publish_subject("shared-auth.events.*", p).is_err());
        assert!(validate_publish_subject("shared-auth.>", p).is_err());
        // malformed tokens
        assert!(validate_publish_subject("shared-auth..double", p).is_err());
        assert!(validate_publish_subject("shared-auth.events.", p).is_err());
        assert!(validate_publish_subject("shared-auth.ev ents", p).is_err());
        assert!(validate_publish_subject(&format!("shared-auth.{}", "x".repeat(300)), p).is_err());
    }

    #[test]
    fn delivery_routes_parse() {
        let routes: Vec<DeliveryRoute> = serde_json::from_str(
            r#"[{"subject":"shared-auth.commands.>","webhook":"http://dd-shared-auth.shared-auth.svc.cluster.local:8120/internal/commands"}]"#,
        )
        .unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].subject, "shared-auth.commands.>");
    }
}
