//! Shared application dependencies.

use crate::config::{Config, SharedAuthConfig, SupabaseConfig};
use crate::metrics::Metrics;
use crate::shared_auth::SharedAuthClient;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct AppState {
    pub supabase: Option<SupabaseConfig>,
    pub shared_auth: Option<SharedAuthClient>,
    pub server_secret: Arc<Vec<u8>>,
    pub http: reqwest::Client,
    pub metrics: Arc<Metrics>,
    /// Highest HOTP counter accepted per enrollment secret (base32), so an
    /// already-verified code cannot be replayed (RFC 6238 §5.2). Entries are
    /// pruned once they fall outside the ±skew verification window, which
    /// bounds the map by the number of successful enrollments per window.
    /// In-memory, so per-replica; fine for the current single-replica deploy.
    pub used_totp_counters: Arc<Mutex<HashMap<String, u64>>>,
}

impl AppState {
    pub fn from_config(config: Config) -> Result<Self, prometheus::Error> {
        Self::new(config.supabase, config.shared_auth, config.server_secret)
    }

    pub fn new(
        supabase: Option<SupabaseConfig>,
        shared_auth: Option<SharedAuthConfig>,
        server_secret: Vec<u8>,
    ) -> Result<Self, prometheus::Error> {
        // Bound every outbound auth hop. Without these, a silently dropped
        // packet (a NetworkPolicy denying the shared-auth hop drops rather than
        // rejects) parks an axum worker on the OS TCP timeout instead of
        // failing fast into the Degraded/503 path the handlers already have.
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Ok(Self {
            supabase,
            shared_auth: shared_auth.map(|config| SharedAuthClient::new(config, http.clone())),
            server_secret: Arc::new(server_secret),
            http,
            metrics: Arc::new(Metrics::new()?),
            used_totp_counters: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    #[cfg(test)]
    pub fn accepting_for_tests(
        supabase: Option<SupabaseConfig>,
        server_secret: Vec<u8>,
    ) -> Result<Self, prometheus::Error> {
        let mut state = Self::new(supabase, None, server_secret)?;
        state.shared_auth = Some(SharedAuthClient::accepting_for_tests());
        Ok(state)
    }
}
