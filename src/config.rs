use std::{env, fmt};

use crate::supabase_auth::SupabaseConfig;

pub const DEFAULT_STRIPE_API_VERSION: &str = "2026-04-22.dahlia";

#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    /// Connection string for the service's OWN database (separate from the
    /// shared pg-defs RDS contract). Schema changes are managed out-of-band
    /// via `scripts/dpm.sh`; there is no boot-time migration switch.
    pub database_url: String,

    pub master_seal_key_b64: String,

    pub solana_rpc_url: String,
    pub solana_anchor_keypair_b58: Option<String>,
    pub solana_cluster: SolanaCluster,

    pub stripe_client_id: Option<String>,
    pub stripe_client_secret: Option<String>,
    pub stripe_api_key: Option<String>,
    pub stripe_api_version: String,
    pub stripe_webhook_secret: Option<String>,
    pub paypal_client_id: Option<String>,
    pub paypal_client_secret: Option<String>,
    pub paypal_env: ProviderEnvironment,
    pub paypal_webhook_id: Option<String>,
    pub paypal_api_base_override: Option<String>,
    pub paypal_connect_base_override: Option<String>,
    pub braintree_client_id: Option<String>,
    pub braintree_client_secret: Option<String>,
    pub braintree_env: ProviderEnvironment,
    pub braintree_api_base_override: Option<String>,
    pub plaid_client_id: Option<String>,
    pub plaid_secret: Option<String>,
    pub plaid_env: PlaidEnvironment,
    pub plaid_api_base_override: Option<String>,
    pub coinbase_webhook_secret: Option<String>,
    pub coinflow_webhook_validation_key: Option<String>,
    pub revolut_webhook_secret: Option<String>,
    pub gocardless_webhook_secret: Option<String>,
    pub mercury_webhook_secret: Option<String>,

    pub oauth_redirect_base: String,
    pub oauth_return_to_allowed_prefixes: Vec<String>,
    pub require_webhook_signatures: bool,
    pub webhook_signature_tolerance_seconds: i64,

    /// Mount the read-mostly HTMX admin UI at `/admin`. Defaults to ON, but the
    /// server refuses to boot with the admin UI enabled unless
    /// `admin_auth_bearer` is also set (or `BILLING_ALLOW_INSECURE_DEV=1` is
    /// explicitly given for local dev). Production deployments behind public
    /// gateways should either disable this (`BILLING_ADMIN_UI_ENABLED=false`)
    /// or front it with `dd-remote-auth` per the access-posture rule in
    /// `AGENTS.md`.
    pub admin_ui_enabled: bool,

    /// When set, every `/admin/*` request must present
    /// `Authorization: Bearer <this value>`. Constant-time compared. Required
    /// whenever the admin UI is enabled (boot fails otherwise unless
    /// `BILLING_ALLOW_INSECURE_DEV=1`). In production this should be a
    /// high-entropy random string injected via SealedSecrets / the External
    /// Secrets stack, mirroring how other webhook secrets land in `BILLING_*`
    /// env vars.
    pub admin_auth_bearer: Option<String>,

    /// Cross-origin `Origin` values explicitly allowed to perform admin
    /// writes. Same-origin (Origin host matches request Host) is always
    /// allowed and does not need an entry here. Wire via the comma-
    /// separated `BILLING_ADMIN_ALLOWED_ORIGINS` env var when an
    /// operator dashboard hosted elsewhere needs to embed admin actions.
    pub admin_allowed_origins: Vec<String>,

    /// **Service-to-service** bearer token for the JSON API (`/v1/...`).
    ///
    /// This is a single process-wide shared secret. It authenticates *a
    /// caller*, and deliberately carries no identity beyond "something holding
    /// the shared token". It must therefore never be handed to an end user or
    /// embedded in a client application: anyone holding it is, as far as this
    /// token is concerned, every tenant at once.
    ///
    /// Required to boot: without it the entire API is open to anyone who can
    /// reach the listener, so the server refuses to start unless this is set
    /// (or `BILLING_ALLOW_INSECURE_DEV=1` is given for local dev).
    ///
    /// Per-*user* authentication and per-*tenant* authorization are handled
    /// separately, by [`Self::supabase`] and
    /// [`Self::tenant_routes_require_user_jwt`].
    pub api_auth_bearer: Option<String>,

    /// Supabase wiring for per-user JWT verification. See
    /// [`crate::supabase_auth`].
    pub supabase: SupabaseConfig,

    /// Require a verified Supabase JWT (not merely the shared service bearer)
    /// on every tenant-scoped `/v1/tenants/{tenant_id}/...` route, and check
    /// that the caller is entitled to that specific tenant.
    ///
    /// **Defaults to `true` — fail-closed.** With it off, the shared service
    /// bearer alone is sufficient on tenant-scoped routes, which is precisely
    /// the IDOR this setting exists to close: any holder of that one token can
    /// operate on any tenant by editing the path. `false` is a *migration
    /// window*, not a supported steady state. Set
    /// `BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false` only while callers are
    /// being moved onto Supabase tokens; the server logs a WARN every boot for
    /// as long as it is off.
    pub tenant_routes_require_user_jwt: bool,

    /// Require a *fresh AAL2* session and an explicit financial scope for every
    /// human-initiated **mutation** of a tenant's ledger state (`POST`/`PUT`/
    /// `PATCH`/`DELETE` on `/v1/tenants/{tenant_id}/...`). Reads, unscoped
    /// provisioning calls, and provider webhooks are unaffected.
    ///
    /// Defaults to `false` — a *migration window*, like
    /// [`Self::tenant_routes_require_user_jwt`]: existing user tokens may not
    /// carry `aal2`/`financial_scopes` yet, so flipping it on before the issuer
    /// stamps them would lock out legitimate callers. The server logs a WARN
    /// every boot while it is off. Turn it on once Shared Auth issues stepped-up,
    /// scoped tokens.
    pub step_up_required_for_mutations: bool,

    /// Refuse outbound HTTP to private / loopback / link-local IPs.
    /// Protects `tenant.webhook` jobs and notification channels from
    /// being weaponized into an SSRF probe of the cluster's internal
    /// services. Defaults to `true`; set
    /// `BILLING_ALLOW_PRIVATE_OUTBOUND=true` to opt out (for dev /
    /// integration tests against a local mock server).
    pub block_private_outbound: bool,

    /// Use fiducia.cloud as the fenced coordination authority for customer
    /// snapshot locks and tenant leases. Defaults off for local development;
    /// production manifests turn this on and must provide credentials.
    pub fiducia_enabled: bool,

    /// Fiducia edge/load-balancer base URL. The in-cluster service is the
    /// production default; public deployments should use HTTPS.
    pub fiducia_base_url: String,

    /// Scoped public API key sent as a bearer token. The key resolves its own
    /// org scope and must grant `locks:write` (which also authorizes elections).
    pub fiducia_api_key: Option<String>,

    /// TTL used for short customer snapshot/write critical sections.
    pub fiducia_lock_ttl_ms: u64,

    /// Timeout budget for Fiducia acquire/release/lease operations.
    pub fiducia_request_timeout_ms: u64,

    /// NATS server URL for the domain-event feed + inbound sync commands.
    /// `BILLING_NATS_URL`, falling back to the shared `NATS_URL`. When unset
    /// the [`crate::events::EventBus`] runs as a silent no-op (publishes are
    /// dropped, no subscriber loop is started), mirroring the CDC consumer.
    pub nats_url: Option<String>,

    /// Master switch for the NATS event layer. Defaults `false` so the
    /// server carries no messaging dependency unless an operator opts in.
    /// Connecting (and the inbound sync-command subscriber) only happens
    /// when this is true AND `nats_url` resolves.
    pub nats_publish_enabled: bool,

    /// Queue group for the inbound `dd.remote.billing.commands.sync`
    /// subscription so replicas load-balance commands. Defaults to the
    /// generated `BILLING_SYNC_COMMANDS_QUEUE_GROUP` (`dd-billing-server`).
    pub nats_queue_group: Option<String>,

    /// Hard ceiling on published payload bytes and accepted inbound command
    /// bytes (defense against a malformed / hostile message). Default 1 MiB.
    pub nats_max_payload_bytes: usize,
}

// Config holds database credentials, the master sealing key, provider
// credentials, webhook secrets, API bearer tokens, and the Fiducia key. Keep
// its Debug surface deliberately small so an incidental structured log cannot
// exfiltrate any of them.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database_url", &"<redacted>")
            .field("oauth_redirect_base", &self.oauth_redirect_base)
            .field(
                "require_webhook_signatures",
                &self.require_webhook_signatures,
            )
            .field("admin_ui_enabled", &self.admin_ui_enabled)
            .field("block_private_outbound", &self.block_private_outbound)
            .field("fiducia_enabled", &self.fiducia_enabled)
            .field("fiducia_base_url", &self.fiducia_base_url)
            .field("fiducia_credentials", &"<redacted>")
            .field("fiducia_lock_ttl_ms", &self.fiducia_lock_ttl_ms)
            .field(
                "fiducia_request_timeout_ms",
                &self.fiducia_request_timeout_ms,
            )
            .field("nats_publish_enabled", &self.nats_publish_enabled)
            // SupabaseConfig has its own redacting Debug — the JWT secret is
            // never rendered.
            .field("supabase", &self.supabase)
            .field(
                "tenant_routes_require_user_jwt",
                &self.tenant_routes_require_user_jwt,
            )
            .field(
                "step_up_required_for_mutations",
                &self.step_up_required_for_mutations,
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolanaCluster {
    Mainnet,
    Devnet,
    Localnet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderEnvironment {
    Production,
    Sandbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaidEnvironment {
    Production,
    Development,
    Sandbox,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();

        let fiducia_enabled = env_bool("BILLING_FIDUCIA_ENABLED", false);
        let fiducia_base_url = env::var("BILLING_FIDUCIA_BASE_URL").unwrap_or_else(|_| {
            "http://fiducia-load-balance.fiducia.svc.cluster.local:8088".into()
        });
        let fiducia_api_key = optional_trimmed_env("BILLING_FIDUCIA_API_KEY")
            .or_else(|| optional_trimmed_env("FIDUCIA_API_KEY"));
        if fiducia_enabled {
            validate_fiducia_base_url(&fiducia_base_url)?;
            if fiducia_api_key.is_none() {
                anyhow::bail!(
                    "BILLING_FIDUCIA_ENABLED=true requires a locks:write-scoped BILLING_FIDUCIA_API_KEY/FIDUCIA_API_KEY"
                );
            }
        }

        // Fail-closed auth posture. The only legitimate reason to boot without
        // API/admin authentication is local development, which must be an
        // explicit, clearly-named opt-in — never a silent default.
        let allow_insecure_dev = env_bool("BILLING_ALLOW_INSECURE_DEV", false);
        let admin_ui_enabled = env_bool("BILLING_ADMIN_UI_ENABLED", true);
        let admin_auth_bearer = optional_trimmed_env("BILLING_ADMIN_AUTH_BEARER");
        let api_auth_bearer = optional_trimmed_env("BILLING_API_AUTH_BEARER");

        if !allow_insecure_dev {
            // Admin UI enabled without a bearer = no-op enforcement in
            // `admin/security.rs` (it always-passes when the bearer is unset).
            if admin_ui_enabled && admin_auth_bearer.is_none() {
                anyhow::bail!(
                    "refusing to boot: BILLING_ADMIN_UI_ENABLED is on but \
                     BILLING_ADMIN_AUTH_BEARER is unset, which mounts an \
                     unauthenticated admin UI. Set a high-entropy \
                     BILLING_ADMIN_AUTH_BEARER, disable the admin UI with \
                     BILLING_ADMIN_UI_ENABLED=false, or set \
                     BILLING_ALLOW_INSECURE_DEV=1 for local development."
                );
            }
            // An unset API bearer leaves the entire /v1 API open (the auth
            // middleware always-passes when the bearer is unset).
            if api_auth_bearer.is_none() {
                anyhow::bail!(
                    "refusing to boot: BILLING_API_AUTH_BEARER is unset, which \
                     leaves the entire /v1 API open to anyone who can reach the \
                     listener. Set a high-entropy BILLING_API_AUTH_BEARER, or set \
                     BILLING_ALLOW_INSECURE_DEV=1 for local development."
                );
            }
        }

        // Per-user Supabase auth. `BILLING_SUPABASE_URL` is the only required
        // value; the issuer and JWKS URL follow the hosted layout unless a
        // self-hosted GoTrue deployment overrides them.
        let supabase_url = optional_trimmed_env("BILLING_SUPABASE_URL");
        let supabase = SupabaseConfig {
            issuer: optional_trimmed_env("BILLING_SUPABASE_JWT_ISS")
                .or_else(|| supabase_url.as_deref().map(SupabaseConfig::issuer_for)),
            jwks_url: optional_trimmed_env("BILLING_SUPABASE_JWKS_URL")
                .or_else(|| supabase_url.as_deref().map(SupabaseConfig::jwks_url_for)),
            audience: optional_trimmed_env("BILLING_SUPABASE_JWT_AUD")
                .unwrap_or_else(|| "authenticated".to_string()),
            jwt_secret: optional_trimmed_env("BILLING_SUPABASE_JWT_SECRET"),
            url: supabase_url,
        };

        // Fail-closed by default: tenant-scoped routes require a per-user token.
        let tenant_routes_require_user_jwt =
            env_bool("BILLING_TENANT_ROUTES_REQUIRE_USER_JWT", true);

        if !allow_insecure_dev && tenant_routes_require_user_jwt && !supabase.is_enabled() {
            // Booting in this state would 503 every tenant-scoped request,
            // because the router would demand a JWT it has no way to verify.
            // Refuse loudly at boot instead of failing per-request in prod.
            anyhow::bail!(
                "refusing to boot: tenant-scoped /v1/tenants/{{tenant_id}}/... routes \
                 require a verified Supabase JWT, but Supabase is not configured. \
                 Set BILLING_SUPABASE_URL (and, for a self-hosted GoTrue, \
                 BILLING_SUPABASE_JWT_ISS / BILLING_SUPABASE_JWKS_URL). \
                 \n\nMigration path for existing service callers: set \
                 BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false to keep accepting \
                 the shared BILLING_API_AUTH_BEARER on tenant routes while those \
                 callers are moved onto per-user Supabase tokens. That leaves the \
                 cross-tenant IDOR open, so treat it as a time-boxed migration \
                 window — not a setting to leave in place. \
                 Local development can instead set BILLING_ALLOW_INSECURE_DEV=1."
            );
        }

        // Fail-closed *financial* posture, gated behind an explicit migration
        // flag: human mutations of tenant ledger state require fresh AAL2 + a
        // financial scope. Defaults off so callers whose tokens are not yet
        // stepped-up/scoped keep working; WARN every boot until it is on.
        let step_up_required_for_mutations =
            env_bool("BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS", false);
        if !allow_insecure_dev && !step_up_required_for_mutations {
            tracing::warn!(
                "BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS is off: human mutations of \
                 tenant financial state are not required to carry a fresh AAL2 \
                 session and an explicit financial scope. Treat this as a \
                 time-boxed migration window and enable it once Shared Auth issues \
                 stepped-up, scoped tokens."
            );
        }

        Ok(Self {
            host: env::var("BILLING_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("BILLING_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8087),
            database_url: env::var("BILLING_DATABASE_URL")
                .or_else(|_| env::var("DATABASE_URL"))
                .map_err(|_| anyhow::anyhow!("BILLING_DATABASE_URL or DATABASE_URL must be set"))?,

            master_seal_key_b64: env::var("BILLING_MASTER_SEAL_KEY").map_err(|_| {
                anyhow::anyhow!(
                    "BILLING_MASTER_SEAL_KEY must be set (base64 of a 32-byte key, \
                     normally provided by KMS/SealedSecrets)"
                )
            })?,

            solana_rpc_url: env::var("SOLANA_RPC_URL")
                .unwrap_or_else(|_| "https://api.devnet.solana.com".into()),
            solana_anchor_keypair_b58: env::var("SOLANA_ANCHOR_KEYPAIR_B58").ok(),
            solana_cluster: match env::var("SOLANA_CLUSTER")
                .unwrap_or_else(|_| "devnet".into())
                .as_str()
            {
                "mainnet" | "mainnet-beta" => SolanaCluster::Mainnet,
                "localnet" => SolanaCluster::Localnet,
                _ => SolanaCluster::Devnet,
            },

            stripe_client_id: env::var("STRIPE_CLIENT_ID").ok(),
            stripe_client_secret: env::var("STRIPE_CLIENT_SECRET").ok(),
            stripe_api_key: env::var("STRIPE_API_KEY").ok(),
            stripe_api_version: env::var("STRIPE_API_VERSION")
                .unwrap_or_else(|_| DEFAULT_STRIPE_API_VERSION.into()),
            stripe_webhook_secret: env::var("STRIPE_WEBHOOK_SECRET").ok(),
            paypal_client_id: env::var("PAYPAL_CLIENT_ID").ok(),
            paypal_client_secret: env::var("PAYPAL_CLIENT_SECRET").ok(),
            paypal_env: ProviderEnvironment::from_env("PAYPAL_ENV"),
            paypal_webhook_id: env::var("PAYPAL_WEBHOOK_ID").ok(),
            paypal_api_base_override: optional_trimmed_env("BILLING_PAYPAL_API_BASE"),
            paypal_connect_base_override: optional_trimmed_env("BILLING_PAYPAL_CONNECT_BASE"),
            braintree_client_id: env::var("BRAINTREE_CLIENT_ID").ok(),
            braintree_client_secret: env::var("BRAINTREE_CLIENT_SECRET").ok(),
            braintree_env: ProviderEnvironment::from_env("BRAINTREE_ENV"),
            braintree_api_base_override: optional_trimmed_env("BILLING_BRAINTREE_API_BASE"),
            plaid_client_id: env::var("PLAID_CLIENT_ID").ok(),
            plaid_secret: env::var("PLAID_SECRET").ok(),
            plaid_env: PlaidEnvironment::from_env("PLAID_ENV"),
            plaid_api_base_override: optional_trimmed_env("BILLING_PLAID_API_BASE"),
            coinbase_webhook_secret: env::var("COINBASE_WEBHOOK_SECRET").ok(),
            coinflow_webhook_validation_key: env::var("COINFLOW_WEBHOOK_VALIDATION_KEY").ok(),
            revolut_webhook_secret: env::var("REVOLUT_WEBHOOK_SECRET").ok(),
            gocardless_webhook_secret: env::var("GOCARDLESS_WEBHOOK_SECRET").ok(),
            mercury_webhook_secret: env::var("MERCURY_WEBHOOK_SECRET").ok(),

            oauth_redirect_base: env::var("BILLING_OAUTH_REDIRECT_BASE")
                .unwrap_or_else(|_| "http://localhost:8087".into()),
            oauth_return_to_allowed_prefixes: parse_csv_env(
                "BILLING_OAUTH_RETURN_TO_ALLOWED_PREFIXES",
            ),
            // Fail-closed: verify webhook signatures unless an operator
            // explicitly opts out. An unsigned/unverified webhook can forge
            // money-movement events, so the default must reject them.
            require_webhook_signatures: env_bool("BILLING_REQUIRE_WEBHOOK_SIGNATURES", true),
            webhook_signature_tolerance_seconds: env::var(
                "BILLING_WEBHOOK_SIGNATURE_TOLERANCE_SECONDS",
            )
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300),

            admin_ui_enabled,
            admin_auth_bearer,
            admin_allowed_origins: parse_csv_env("BILLING_ADMIN_ALLOWED_ORIGINS"),
            api_auth_bearer,
            supabase,
            tenant_routes_require_user_jwt,
            step_up_required_for_mutations,
            // Default fail-closed: the only legitimate use for outbound
            // private-IP traffic is dev/integration. Production callers
            // should hit the public webhook URL of their tenant.
            block_private_outbound: env_bool("BILLING_BLOCK_PRIVATE_OUTBOUND", true),
            fiducia_enabled,
            fiducia_base_url,
            fiducia_api_key,
            fiducia_lock_ttl_ms: env_u64("BILLING_FIDUCIA_LOCK_TTL_MS", 60_000)
                .clamp(1_000, 86_400_000),
            fiducia_request_timeout_ms: env_u64("BILLING_FIDUCIA_REQUEST_TIMEOUT_MS", 30_000)
                .clamp(100, 30_000),
            nats_url: optional_trimmed_env("BILLING_NATS_URL")
                .or_else(|| optional_trimmed_env("NATS_URL")),
            nats_publish_enabled: env_bool("BILLING_NATS_PUBLISH_ENABLED", false),
            nats_queue_group: optional_trimmed_env("BILLING_NATS_QUEUE_GROUP"),
            // 1 MiB default; clamp to a sane band so a typo can't set 0
            // (which would reject every message) or an absurd ceiling.
            nats_max_payload_bytes: env_u64("BILLING_NATS_MAX_PAYLOAD_BYTES", 1_048_576)
                .clamp(4_096, 8_388_608) as usize,
        })
    }

    pub fn stripe_api_key(&self) -> Option<&String> {
        self.stripe_api_key
            .as_ref()
            .or(self.stripe_client_secret.as_ref())
    }

    pub fn paypal_api_base(&self) -> &str {
        if let Some(base) = &self.paypal_api_base_override {
            return base;
        }
        match self.paypal_env {
            ProviderEnvironment::Production => "https://api-m.paypal.com",
            ProviderEnvironment::Sandbox => "https://api-m.sandbox.paypal.com",
        }
    }

    pub fn paypal_connect_base(&self) -> &str {
        if let Some(base) = &self.paypal_connect_base_override {
            return base;
        }
        match self.paypal_env {
            ProviderEnvironment::Production => "https://www.paypal.com",
            ProviderEnvironment::Sandbox => "https://www.sandbox.paypal.com",
        }
    }

    pub fn braintree_api_base(&self) -> &str {
        if let Some(base) = &self.braintree_api_base_override {
            return base;
        }
        match self.braintree_env {
            ProviderEnvironment::Production => "https://api.braintreegateway.com",
            ProviderEnvironment::Sandbox => "https://api.sandbox.braintreegateway.com",
        }
    }

    pub fn plaid_api_base(&self) -> &str {
        if let Some(base) = &self.plaid_api_base_override {
            return base;
        }
        match self.plaid_env {
            PlaidEnvironment::Production => "https://production.plaid.com",
            PlaidEnvironment::Development => "https://development.plaid.com",
            PlaidEnvironment::Sandbox => "https://sandbox.plaid.com",
        }
    }

    /// Build a minimally-populated Config suitable for unit tests that
    /// need to pass `&Config` somewhere but don't care about most
    /// fields. Optional provider creds are left empty.
    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 0,
            database_url: "postgres://test".into(),
            master_seal_key_b64: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
            solana_rpc_url: "http://localhost".into(),
            solana_anchor_keypair_b58: None,
            solana_cluster: SolanaCluster::Devnet,
            stripe_client_id: None,
            stripe_client_secret: None,
            stripe_api_key: None,
            stripe_api_version: DEFAULT_STRIPE_API_VERSION.into(),
            stripe_webhook_secret: None,
            paypal_client_id: None,
            paypal_client_secret: None,
            paypal_env: ProviderEnvironment::Sandbox,
            paypal_webhook_id: None,
            paypal_api_base_override: None,
            paypal_connect_base_override: None,
            braintree_client_id: None,
            braintree_client_secret: None,
            braintree_env: ProviderEnvironment::Sandbox,
            braintree_api_base_override: None,
            plaid_client_id: None,
            plaid_secret: None,
            plaid_env: PlaidEnvironment::Sandbox,
            plaid_api_base_override: None,
            coinbase_webhook_secret: None,
            coinflow_webhook_validation_key: None,
            revolut_webhook_secret: None,
            gocardless_webhook_secret: None,
            mercury_webhook_secret: None,
            oauth_redirect_base: "http://localhost".into(),
            oauth_return_to_allowed_prefixes: Vec::new(),
            require_webhook_signatures: false,
            webhook_signature_tolerance_seconds: 300,
            admin_ui_enabled: false,
            admin_auth_bearer: None,
            admin_allowed_origins: Vec::new(),
            api_auth_bearer: None,
            supabase: SupabaseConfig::default(),
            // Tests build routers without a Supabase project to talk to; the
            // per-tenant checks have their own focused tests in `api::auth`.
            tenant_routes_require_user_jwt: false,
            step_up_required_for_mutations: false,
            // Tests sometimes hit localhost; default-allow keeps them simple.
            block_private_outbound: false,
            fiducia_enabled: false,
            fiducia_base_url: "http://127.0.0.1:8090".into(),
            fiducia_api_key: None,
            fiducia_lock_ttl_ms: 60_000,
            fiducia_request_timeout_ms: 30_000,
            nats_url: None,
            nats_publish_enabled: false,
            nats_queue_group: None,
            nats_max_payload_bytes: 1_048_576,
        }
    }
}

impl ProviderEnvironment {
    fn from_env(name: &str) -> Self {
        match env::var(name)
            .unwrap_or_else(|_| "production".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "sandbox" | "test" => Self::Sandbox,
            _ => Self::Production,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Sandbox => "sandbox",
        }
    }
}

impl PlaidEnvironment {
    fn from_env(name: &str) -> Self {
        match env::var(name)
            .unwrap_or_else(|_| "production".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "sandbox" | "test" => Self::Sandbox,
            "development" | "dev" => Self::Development,
            _ => Self::Production,
        }
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s == "1" || s == "true" || s == "yes" || s == "on"
        })
        .unwrap_or(default)
}

fn optional_trimmed_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn parse_csv_env(name: &str) -> Vec<String> {
    env::var(name)
        .ok()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn validate_fiducia_base_url(raw: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(raw)
        .map_err(|err| anyhow::anyhow!("invalid BILLING_FIDUCIA_BASE_URL: {err}"))?;
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        anyhow::bail!("BILLING_FIDUCIA_BASE_URL must not contain credentials, query, or fragment");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("BILLING_FIDUCIA_BASE_URL must include a host"))?;
    let local_http = host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.ends_with(".svc")
        || host.ends_with(".svc.cluster.local");
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local_http) {
        anyhow::bail!(
            "BILLING_FIDUCIA_BASE_URL must use HTTPS outside localhost or Kubernetes service DNS"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, validate_fiducia_base_url};

    #[test]
    fn fiducia_url_allows_https_and_private_cluster_dns() {
        validate_fiducia_base_url("https://api.fiducia.cloud").unwrap();
        validate_fiducia_base_url("http://fiducia-load-balance.fiducia.svc.cluster.local:8088")
            .unwrap();
        validate_fiducia_base_url("http://127.0.0.1:8090").unwrap();
    }

    #[test]
    fn fiducia_url_rejects_public_cleartext_and_embedded_secrets() {
        assert!(validate_fiducia_base_url("http://api.fiducia.cloud").is_err());
        assert!(validate_fiducia_base_url("https://token@api.fiducia.cloud?debug=1").is_err());
    }

    #[test]
    fn debug_output_redacts_every_credential_class() {
        let mut config = Config::for_tests();
        config.database_url = "postgres://billing:db-secret@example.invalid/billing".into();
        config.master_seal_key_b64 = "master-seal-secret".into();
        config.stripe_api_key = Some("stripe-secret".into());
        config.api_auth_bearer = Some("api-bearer-secret".into());
        config.admin_auth_bearer = Some("admin-bearer-secret".into());
        config.fiducia_api_key = Some("fiducia-secret".into());
        config.supabase.jwt_secret = Some("supabase-jwt-secret".into());

        let output = format!("{config:?}");
        for secret in [
            "db-secret",
            "master-seal-secret",
            "stripe-secret",
            "api-bearer-secret",
            "admin-bearer-secret",
            "fiducia-secret",
            "supabase-jwt-secret",
        ] {
            assert!(!output.contains(secret));
        }
        assert!(output.contains("<redacted>"));
    }
}
