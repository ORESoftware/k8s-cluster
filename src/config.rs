use std::env;

use dd_nats_subject_defs::{
    FABRICATION_REQUESTS_QUEUE_GROUP, FABRICATION_REQUESTS_SUBJECT, FABRICATION_RESULTS_SUBJECT,
    MDP_OPTIMIZE_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};

#[derive(Debug, Clone)]
pub(crate) struct ServiceConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tcp_port: u16,
    /// The raw newline-delimited-JSON TCP transport streams full plan payloads
    /// to anyone who can open a socket and cannot carry a bearer token in any
    /// useful way, so it is opt-in and off by default. It is a
    /// trusted-network-only debug transport; the NetworkPolicy already confines
    /// it to the `daedalus` namespace, and this flag means it is not even
    /// listening unless an operator asks for it.
    pub(crate) tcp_enabled: bool,
    pub(crate) request_subject: String,
    pub(crate) queue_group: String,
    pub(crate) result_subject: String,
    pub(crate) event_subject: String,
    pub(crate) mdp_subject: String,
    pub(crate) mdp_autopublish: bool,
    pub(crate) nats_max_inflight: usize,
    pub(crate) realtime_buffer: usize,
    pub(crate) auth: AuthConfig,
    pub(crate) fiducia: FiduciaConfig,
}

impl ServiceConfig {
    pub(crate) fn from_env() -> Result<Self, std::num::ParseIntError> {
        Ok(Self {
            host: env_value("HOST", "0.0.0.0"),
            port: env_value("PORT", "8113").parse::<u16>()?,
            tcp_port: env_value("FABRICATION_TCP_PORT", "8114").parse::<u16>()?,
            request_subject: env_value("FABRICATION_REQUEST_SUBJECT", FABRICATION_REQUESTS_SUBJECT),
            queue_group: env_value("FABRICATION_QUEUE_GROUP", FABRICATION_REQUESTS_QUEUE_GROUP),
            result_subject: env_value("FABRICATION_RESULT_SUBJECT", FABRICATION_RESULTS_SUBJECT),
            event_subject: env_value("FABRICATION_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
            mdp_subject: env_value("FABRICATION_MDP_OPTIMIZE_SUBJECT", MDP_OPTIMIZE_SUBJECT),
            mdp_autopublish: env_bool("FABRICATION_MDP_AUTOPUBLISH", false),
            nats_max_inflight: env_u64("FABRICATION_NATS_MAX_INFLIGHT", 8, 1, 128) as usize,
            realtime_buffer: env_u64("FABRICATION_REALTIME_BUFFER", 256, 8, 4_096) as usize,
            tcp_enabled: env_bool("FABRICATION_TCP_ENABLED", false),
            auth: AuthConfig::from_env(),
            fiducia: FiduciaConfig::from_env(),
        })
    }
}

/// Fiducia settings for both things this service uses fiducia for.
///
/// They are separate credentials on purpose. The KV path reads secrets and
/// needs only read scope; the lock path mutates `/v1/locks/*` and needs
/// `locks:write`. Handing the lock scope to the pod that only needs to read
/// `secrets/daedalus/*` — or, worse, discovering at runtime that the KV key was
/// silently used for locks and 403s — is the failure this split prevents.
#[derive(Debug, Clone, Default)]
pub(crate) struct FiduciaConfig {
    /// `FIDUCIA_URL` — the fiducia load balancer. Shared by both paths.
    pub(crate) url: Option<String>,
    /// The KV-read credential, accepted under either org spelling.
    ///
    /// `FIDUCIA_API_KEY` is what this service and its manifests use;
    /// `FIDUCIA_TOKEN` is what the Daedalus MCP server calls the same thing.
    /// Both are accepted, `FIDUCIA_API_KEY` wins, and [`crate::secrets`]
    /// resolves it identically so the two modules cannot drift.
    pub(crate) kv_api_key: Option<String>,
    /// `FIDUCIA_LOCKS_API_KEY` — a distinct, least-privilege credential that
    /// carries the `locks:write` scope. The KV key does not, and will 403
    /// `insufficient_scope` if it is used here.
    pub(crate) locks_api_key: Option<String>,
    /// Lease lifetime; renewals run at a third of it.
    pub(crate) lease_ttl_ms: u64,
}

impl FiduciaConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            url: optional_env("FIDUCIA_URL"),
            kv_api_key: optional_env("FIDUCIA_API_KEY").or_else(|| optional_env("FIDUCIA_TOKEN")),
            locks_api_key: optional_env("FIDUCIA_LOCKS_API_KEY"),
            lease_ttl_ms: env_u64(
                "FABRICATION_LEASE_TTL_MS",
                crate::coordination::DEFAULT_LEASE_TTL_MS,
                5_000,
                300_000,
            ),
        }
    }

    /// Distributed leases are live only when a URL *and* a lock-scoped key are
    /// both present. Anything less runs on `NoopCoordination`, which is correct
    /// for the single-replica deployment and is never a silent upgrade: the URL
    /// alone must not enable leases, because the KV key would 403 on every
    /// acquire and NetworkPolicy egress to fiducia is not open.
    pub(crate) fn leases_enabled(&self) -> bool {
        self.url.is_some() && self.locks_api_key.is_some()
    }
}

/// Shared-auth authority and application authorization policy.
///
/// The shared-auth library races the central authority against Supabase, while
/// this service keeps the final Daedalus operator policy explicit. An empty
/// email/role policy disables the guard and therefore fails closed.
#[derive(Debug, Clone)]
pub(crate) struct AuthConfig {
    pub(crate) shared_auth_base: String,
    pub(crate) issuer: String,
    pub(crate) audience: String,
    pub(crate) supabase_url: Option<String>,
    pub(crate) supabase_api_key: Option<String>,
    pub(crate) provider_tenant: String,
    pub(crate) allowed_emails: Vec<String>,
    pub(crate) allowed_roles: Vec<String>,
    pub(crate) arm_timeout_ms: u64,
    pub(crate) deadline_ms: u64,
}

impl AuthConfig {
    pub(crate) fn from_env() -> Self {
        let legacy_issuer = optional_env("FABRICATION_SUPABASE_ISSUER");
        let supabase_url = optional_env("FABRICATION_SUPABASE_URL")
            .or_else(|| legacy_issuer.as_deref().and_then(supabase_url_from_issuer));
        Self {
            shared_auth_base: env_value(
                "FABRICATION_SHARED_AUTH_BASE",
                "http://dd-shared-auth.shared-auth.svc.cluster.local:8120",
            ),
            issuer: env_value(
                "FABRICATION_SHARED_AUTH_ISSUER",
                "https://auth.oresoftware.dev",
            ),
            audience: env_value("FABRICATION_SHARED_AUTH_AUDIENCE", "oresoftware"),
            provider_tenant: optional_env("FABRICATION_AUTH_PROVIDER_TENANT")
                .or_else(|| supabase_url.as_deref().and_then(supabase_project_from_url))
                .unwrap_or_default(),
            supabase_url,
            supabase_api_key: optional_env("FABRICATION_SUPABASE_PUBLISHABLE_KEY")
                .or_else(|| optional_env("FABRICATION_SUPABASE_ANON_KEY")),
            allowed_emails: optional_env("FABRICATION_ALLOWED_EMAILS")
                .map(|raw| normalized_csv(&raw, true))
                .unwrap_or_default(),
            allowed_roles: optional_env("FABRICATION_ALLOWED_ROLES")
                .map(|raw| normalized_csv(&raw, false))
                .unwrap_or_default(),
            arm_timeout_ms: env_u64("FABRICATION_AUTH_ARM_TIMEOUT_MS", 1_200, 100, 10_000),
            deadline_ms: env_u64("FABRICATION_AUTH_DEADLINE_MS", 1_500, 100, 15_000),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        !self.shared_auth_base.trim().is_empty()
            && self.supabase_url.is_some()
            && !self.provider_tenant.trim().is_empty()
            && (!self.allowed_emails.is_empty() || !self.allowed_roles.is_empty())
    }
}

fn normalized_csv(raw: &str, lowercase: bool) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if lowercase {
                entry.to_ascii_lowercase()
            } else {
                entry.to_string()
            }
        })
        .collect()
}

fn supabase_url_from_issuer(issuer: &str) -> Option<String> {
    let url = issuer.trim().trim_end_matches('/');
    url.strip_suffix("/auth/v1")
        .filter(|base| base.starts_with("https://"))
        .map(str::to_string)
}

fn supabase_project_from_url(url: &str) -> Option<String> {
    url.trim()
        .strip_prefix("https://")?
        .split('.')
        .next()
        .filter(|project| !project.is_empty())
        .map(str::to_string)
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

pub(crate) fn env_u64(key: &str, fallback: u64, min: u64, max: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(fallback)
        .clamp(min, max)
}

pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_kubernetes_service_contract() {
        assert_eq!(
            env_value("DAEDALUS_MISSING_TEST_VALUE", "fallback"),
            "fallback"
        );
        assert!(!env_bool("DAEDALUS_MISSING_TEST_BOOL", false));
        assert_eq!(env_u64("DAEDALUS_MISSING_TEST_U64", 8, 1, 128), 8);
        assert_eq!(optional_env("DAEDALUS_MISSING_TEST_OPT"), None);
    }

    #[test]
    fn the_raw_tcp_transport_is_off_unless_explicitly_enabled() {
        // The TCP stream cannot carry a bearer token, so "not listening" is the
        // only safe default. A missing or unparsable flag must not open it.
        assert!(!env_bool("FABRICATION_TCP_ENABLED", false));
    }

    #[test]
    fn distributed_leases_require_both_a_url_and_a_lock_scoped_key() {
        let mut fiducia = FiduciaConfig {
            url: None,
            kv_api_key: Some("kv-read".to_string()),
            locks_api_key: None,
            lease_ttl_ms: 30_000,
        };
        // A KV credential is not a lock credential: it lacks `locks:write` and
        // would 403 on every acquire, so it must not enable leases.
        assert!(!fiducia.leases_enabled());
        fiducia.url = Some("http://fiducia.daedalus.svc:8088".to_string());
        assert!(!fiducia.leases_enabled());
        // A lock key with no endpoint is equally unusable.
        fiducia.url = None;
        fiducia.locks_api_key = Some("locks-write".to_string());
        assert!(!fiducia.leases_enabled());

        fiducia.url = Some("http://fiducia.daedalus.svc:8088".to_string());
        assert!(fiducia.leases_enabled());
    }

    #[test]
    fn an_unconfigured_fiducia_defaults_to_local_coordination() {
        // The whole default path: nothing set, nothing enabled, no error.
        let fiducia = FiduciaConfig::default();
        assert!(!fiducia.leases_enabled());
        assert_eq!(fiducia.url, None);
        assert_eq!(fiducia.locks_api_key, None);
    }

    fn config_with(allowed: &[&str], roles: &[&str]) -> AuthConfig {
        AuthConfig {
            shared_auth_base: "http://dd-shared-auth.shared-auth.svc:8120".to_string(),
            issuer: "https://auth.oresoftware.dev".to_string(),
            audience: "oresoftware".to_string(),
            supabase_url: Some("https://proj.supabase.co".to_string()),
            supabase_api_key: Some("publishable-test-key".to_string()),
            provider_tenant: "proj".to_string(),
            allowed_emails: allowed.iter().map(|e| e.to_string()).collect(),
            allowed_roles: roles.iter().map(|role| role.to_string()).collect(),
            arm_timeout_ms: 1_200,
            deadline_ms: 1_500,
        }
    }

    #[test]
    fn auth_requires_two_authorities_and_an_explicit_policy() {
        assert!(!config_with(&[], &[]).is_enabled());
        assert!(config_with(&["a@b.com"], &[]).is_enabled());
        assert!(config_with(&[], &["daedalus-operator"]).is_enabled());

        let mut no_shared_auth = config_with(&["a@b.com"], &[]);
        no_shared_auth.shared_auth_base.clear();
        assert!(!no_shared_auth.is_enabled());

        let mut no_provider = config_with(&["a@b.com"], &[]);
        no_provider.supabase_url = None;
        assert!(!no_provider.is_enabled());
    }

    #[test]
    fn legacy_supabase_issuer_derives_provider_url_and_tenant() {
        let url = supabase_url_from_issuer("https://project-ref.supabase.co/auth/v1/")
            .expect("valid legacy issuer");
        assert_eq!(url, "https://project-ref.supabase.co");
        assert_eq!(
            supabase_project_from_url(&url).as_deref(),
            Some("project-ref")
        );
        assert_eq!(supabase_url_from_issuer("https://example.com/oidc"), None);
    }

    #[test]
    fn authorization_csv_normalizes_emails_but_preserves_role_ids() {
        assert_eq!(
            normalized_csv(" Operator@Example.COM, second@example.com ", true),
            ["operator@example.com", "second@example.com"]
        );
        assert_eq!(
            normalized_csv("Daedalus.Admin, fabrication-operator", false),
            ["Daedalus.Admin", "fabrication-operator"]
        );
    }
}
