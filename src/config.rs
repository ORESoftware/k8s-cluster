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
    pub(crate) supabase: SupabaseConfig,
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
            supabase: SupabaseConfig::from_env(),
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

/// Supabase access-token verification settings.
///
/// `allowed_emails` is the org's access gate: the Daedalus surfaces are
/// single-operator today, so a token that verifies cryptographically is still
/// rejected unless its `email` claim is on the list. An empty list means the
/// gate is unconfigured, which [`SupabaseConfig::is_enabled`] treats as
/// auth-disabled rather than allow-all — failing closed on misconfiguration.
#[derive(Debug, Clone)]
pub(crate) struct SupabaseConfig {
    pub(crate) audience: String,
    pub(crate) issuer: Option<String>,
    pub(crate) jwt_secret: Option<String>,
    pub(crate) jwks_url: Option<String>,
    pub(crate) allowed_emails: Vec<String>,
}

impl SupabaseConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            audience: env_value("FABRICATION_SUPABASE_AUDIENCE", "authenticated"),
            issuer: optional_env("FABRICATION_SUPABASE_ISSUER"),
            jwt_secret: optional_env("FABRICATION_SUPABASE_JWT_SECRET"),
            jwks_url: optional_env("FABRICATION_SUPABASE_JWKS_URL"),
            allowed_emails: optional_env("FABRICATION_ALLOWED_EMAILS")
                .map(|raw| {
                    raw.split(',')
                        .map(|entry| entry.trim().to_ascii_lowercase())
                        .filter(|entry| !entry.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Auth is live only when a verification key, an issuer, AND an allow-list
    /// are all configured. Verifying a signature without gating on identity
    /// would let any Supabase user in the project through.
    ///
    /// The issuer is required rather than optional: `aud` is the literal string
    /// `"authenticated"` on *every* Supabase project, so without `iss` pinning,
    /// a token minted by an unrelated project (whose JWKS an attacker can
    /// influence, or whose HS256 secret they know) satisfies every other check.
    /// `iss` is the only claim that binds a token to this project.
    pub(crate) fn is_enabled(&self) -> bool {
        (self.jwt_secret.is_some() || self.jwks_url.is_some())
            && self.issuer.is_some()
            && !self.allowed_emails.is_empty()
    }

    /// Case-insensitive allow-list membership. Callers must pass an email that
    /// came from a verified token, never a client-asserted header.
    pub(crate) fn permits(&self, email: Option<&str>) -> bool {
        match email {
            Some(email) => {
                let email = email.trim().to_ascii_lowercase();
                self.allowed_emails.contains(&email)
            }
            // A token with no email claim can never satisfy an email gate.
            None => false,
        }
    }
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

    fn config_with(allowed: &[&str], secret: Option<&str>) -> SupabaseConfig {
        SupabaseConfig {
            audience: "authenticated".to_string(),
            issuer: Some("https://proj.supabase.co/auth/v1".to_string()),
            jwt_secret: secret.map(str::to_string),
            jwks_url: None,
            allowed_emails: allowed.iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn auth_requires_a_key_an_issuer_and_an_allow_list() {
        // A key with no allow-list would authenticate any project user.
        assert!(!config_with(&[], Some("secret")).is_enabled());
        // An allow-list with no key cannot verify anything.
        assert!(!config_with(&["a@b.com"], None).is_enabled());
        // No issuer means no binding to *this* Supabase project — `aud` is the
        // literal "authenticated" everywhere, so it distinguishes nothing.
        let mut unpinned = config_with(&["a@b.com"], Some("secret"));
        unpinned.issuer = None;
        assert!(!unpinned.is_enabled());

        assert!(config_with(&["a@b.com"], Some("secret")).is_enabled());
    }

    #[test]
    fn allow_list_is_case_insensitive_and_rejects_missing_emails() {
        let config = config_with(&["alexander.d.mills@gmail.com"], Some("secret"));
        assert!(config.permits(Some("alexander.d.mills@gmail.com")));
        assert!(config.permits(Some("  Alexander.D.Mills@GMAIL.com  ")));
        assert!(!config.permits(Some("someone.else@gmail.com")));
        assert!(!config.permits(None));
        // Substring/prefix confusion must not pass the gate.
        assert!(!config.permits(Some("alexander.d.mills@gmail.com.evil.tld")));
        assert!(!config.permits(Some("alexander.d.mills@gmail.co")));
    }
}
