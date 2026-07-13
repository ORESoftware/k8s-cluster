use std::env;

pub const DEFAULT_STRIPE_API_VERSION: &str = "2026-04-22.dahlia";

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub run_migrations: bool,

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

    /// Mount the read-mostly HTMX admin UI at `/admin`. Defaults to ON for
    /// dev convenience; production deployments behind public gateways should
    /// either disable this (`BILLING_ADMIN_UI_ENABLED=false`) or front it
    /// with `dd-remote-auth` per the access-posture rule in `AGENTS.md`.
    pub admin_ui_enabled: bool,

    /// When set, every `/admin/*` request must present
    /// `Authorization: Bearer <this value>`. Constant-time compared. Leave
    /// unset (default) for unauthenticated local dev. In production this
    /// should be a high-entropy random string injected via SealedSecrets /
    /// the External Secrets stack, mirroring how other webhook secrets
    /// land in `BILLING_*` env vars.
    pub admin_auth_bearer: Option<String>,

    /// Cross-origin `Origin` values explicitly allowed to perform admin
    /// writes. Same-origin (Origin host matches request Host) is always
    /// allowed and does not need an entry here. Wire via the comma-
    /// separated `BILLING_ADMIN_ALLOWED_ORIGINS` env var when an
    /// operator dashboard hosted elsewhere needs to embed admin actions.
    pub admin_allowed_origins: Vec<String>,

    /// Bearer token for the JSON API (`/v1/...`). When set, every
    /// tenant-scoped route requires `Authorization: Bearer <token>`
    /// (constant-time compared). Leave unset for unauthenticated local
    /// dev, but **always** set this in production — without it the
    /// entire API is open to anyone who can reach the listener.
    ///
    /// Production deployments should additionally front the listener
    /// with `dd-remote-auth` or another gateway that enforces tenant
    /// ownership; this token is the "fail-closed" floor.
    pub api_auth_bearer: Option<String>,

    /// Refuse outbound HTTP to private / loopback / link-local IPs.
    /// Protects `tenant.webhook` jobs and notification channels from
    /// being weaponized into an SSRF probe of the cluster's internal
    /// services. Defaults to `true`; set
    /// `BILLING_ALLOW_PRIVATE_OUTBOUND=true` to opt out (for dev /
    /// integration tests against a local mock server).
    pub block_private_outbound: bool,

    /// Gate customer billing-state snapshots and customer-account ledger
    /// writes with the external live-mutex-rs broker. Defaults off for local
    /// development; production manifests turn this on.
    pub customer_snapshot_lock_enabled: bool,

    /// live-mutex TCP broker address, usually
    /// `dd-rust-network-mutex.default.svc.cluster.local:6970`.
    pub live_mutex_addr: String,

    /// TTL hint sent to the broker for customer snapshot/write locks.
    pub live_mutex_lock_ttl_ms: u64,

    /// Timeout for connect/acquire/release broker operations.
    pub live_mutex_request_timeout_ms: u64,

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

        Ok(Self {
            host: env::var("BILLING_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("BILLING_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8087),
            database_url: env::var("BILLING_DATABASE_URL")
                .or_else(|_| env::var("DATABASE_URL"))
                .map_err(|_| anyhow::anyhow!("BILLING_DATABASE_URL or DATABASE_URL must be set"))?,
            run_migrations: env::var("BILLING_RUN_MIGRATIONS")
                .map(|s| s != "0" && !s.eq_ignore_ascii_case("false"))
                .unwrap_or(true),

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
            require_webhook_signatures: env_bool("BILLING_REQUIRE_WEBHOOK_SIGNATURES", false),
            webhook_signature_tolerance_seconds: env::var(
                "BILLING_WEBHOOK_SIGNATURE_TOLERANCE_SECONDS",
            )
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300),

            admin_ui_enabled: env_bool("BILLING_ADMIN_UI_ENABLED", true),
            admin_auth_bearer: env::var("BILLING_ADMIN_AUTH_BEARER").ok().and_then(|s| {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }),
            admin_allowed_origins: parse_csv_env("BILLING_ADMIN_ALLOWED_ORIGINS"),
            api_auth_bearer: env::var("BILLING_API_AUTH_BEARER").ok().and_then(|s| {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }),
            // Default fail-closed: the only legitimate use for outbound
            // private-IP traffic is dev/integration. Production callers
            // should hit the public webhook URL of their tenant.
            block_private_outbound: env_bool("BILLING_BLOCK_PRIVATE_OUTBOUND", true),
            customer_snapshot_lock_enabled: env_bool(
                "BILLING_CUSTOMER_SNAPSHOT_LOCK_ENABLED",
                false,
            ),
            live_mutex_addr: env::var("BILLING_LIVE_MUTEX_ADDR")
                .unwrap_or_else(|_| "dd-rust-network-mutex.default.svc.cluster.local:6970".into()),
            live_mutex_lock_ttl_ms: env_u64("BILLING_LIVE_MUTEX_LOCK_TTL_MS", 60_000)
                .clamp(1_000, 60_000),
            live_mutex_request_timeout_ms: env_u64("BILLING_LIVE_MUTEX_REQUEST_TIMEOUT_MS", 30_000)
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
            run_migrations: false,
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
            // Tests sometimes hit localhost; default-allow keeps them simple.
            block_private_outbound: false,
            customer_snapshot_lock_enabled: false,
            live_mutex_addr: "127.0.0.1:6970".into(),
            live_mutex_lock_ttl_ms: 60_000,
            live_mutex_request_timeout_ms: 30_000,
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
