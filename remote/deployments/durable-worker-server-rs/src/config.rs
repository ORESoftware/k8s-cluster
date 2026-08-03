use std::{env, time::Duration};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const DEFAULT_PORT: u16 = 8152;
pub const DEFAULT_EVENT_SUBJECT: &str = "dd.durable.run.*.events";

#[derive(Clone)]
pub struct AuthSecret([u8; 32]);

impl AuthSecret {
    pub fn from_plain(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.len() < 16 {
            return Err("durable worker auth secret must contain at least 16 bytes".to_string());
        }
        Ok(Self(Sha256::digest(value.as_bytes()).into()))
    }

    pub fn verify(&self, candidate: &str) -> bool {
        let digest: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        self.0.ct_eq(&digest).into()
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub nats_url: String,
    pub nats_credentials_file: Option<String>,
    pub nats_token: Option<String>,
    pub nats_require_tls: bool,
    pub state_bucket: String,
    pub event_stream: String,
    pub event_subject: String,
    pub replicas: usize,
    pub poll_max_wait: Duration,
    pub scheduler_interval: Duration,
    pub auth_secret: AuthSecret,
    pub shadow_mode: bool,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, String> {
        let allow_insecure = env_bool("DURABLE_WORKER_ALLOW_INSECURE_LOCAL", false);
        let secret = first_env(&[
            "DURABLE_WORKER_AUTH_SECRET",
            "SERVER_AUTH_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
        ])
        .or_else(|| allow_insecure.then(|| "local-development-only".to_string()))
        .ok_or_else(|| {
            "DURABLE_WORKER_AUTH_SECRET (or SERVER_AUTH_SECRET) is required; set DURABLE_WORKER_ALLOW_INSECURE_LOCAL=true only for isolated local development".to_string()
        })?;
        let port = env_value("PORT", &DEFAULT_PORT.to_string())
            .parse::<u16>()
            .map_err(|error| format!("invalid PORT: {error}"))?;
        let replicas = env_value("DURABLE_WORKER_NATS_REPLICAS", "1")
            .parse::<usize>()
            .map_err(|error| format!("invalid DURABLE_WORKER_NATS_REPLICAS: {error}"))?
            .clamp(1, 5);
        Ok(Self {
            port,
            nats_url: env_value(
                "NATS_URL",
                "nats://dd-nats.messaging.svc.cluster.local:4222",
            ),
            nats_credentials_file: first_env(&["NATS_CREDENTIALS_FILE"]),
            nats_token: first_env(&["NATS_TOKEN"]),
            nats_require_tls: env_bool("NATS_REQUIRE_TLS", false),
            state_bucket: env_value("DURABLE_WORKER_STATE_BUCKET", "DD_DURABLE_WORKER_STATE"),
            event_stream: env_value("DURABLE_WORKER_EVENT_STREAM", "DD_DURABLE_WORKER_EVENTS"),
            event_subject: env_value("DURABLE_WORKER_EVENT_SUBJECT", DEFAULT_EVENT_SUBJECT),
            replicas,
            poll_max_wait: Duration::from_millis(
                env_u64("DURABLE_WORKER_POLL_MAX_WAIT_MS", 30_000).clamp(100, 60_000),
            ),
            scheduler_interval: Duration::from_millis(
                env_u64("DURABLE_WORKER_SCHEDULER_INTERVAL_MS", 1_000).clamp(100, 60_000),
            ),
            auth_secret: AuthSecret::from_plain(&secret)?,
            shadow_mode: env_bool("DURABLE_WORKER_SHADOW_MODE", true),
        })
    }
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_comparison_accepts_only_the_exact_secret() {
        let secret = AuthSecret::from_plain("0123456789abcdef").unwrap();
        assert!(secret.verify("0123456789abcdef"));
        assert!(!secret.verify("0123456789abcdeg"));
        assert!(!secret.verify(""));
    }
}
