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
    pub braintree_client_id: Option<String>,
    pub braintree_client_secret: Option<String>,
    pub braintree_env: ProviderEnvironment,
    pub plaid_client_id: Option<String>,
    pub plaid_secret: Option<String>,
    pub plaid_env: PlaidEnvironment,
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
            braintree_client_id: env::var("BRAINTREE_CLIENT_ID").ok(),
            braintree_client_secret: env::var("BRAINTREE_CLIENT_SECRET").ok(),
            braintree_env: ProviderEnvironment::from_env("BRAINTREE_ENV"),
            plaid_client_id: env::var("PLAID_CLIENT_ID").ok(),
            plaid_secret: env::var("PLAID_SECRET").ok(),
            plaid_env: PlaidEnvironment::from_env("PLAID_ENV"),
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
        })
    }

    pub fn stripe_api_key(&self) -> Option<&String> {
        self.stripe_api_key
            .as_ref()
            .or(self.stripe_client_secret.as_ref())
    }

    pub fn paypal_api_base(&self) -> &'static str {
        match self.paypal_env {
            ProviderEnvironment::Production => "https://api-m.paypal.com",
            ProviderEnvironment::Sandbox => "https://api-m.sandbox.paypal.com",
        }
    }

    pub fn paypal_connect_base(&self) -> &'static str {
        match self.paypal_env {
            ProviderEnvironment::Production => "https://www.paypal.com",
            ProviderEnvironment::Sandbox => "https://www.sandbox.paypal.com",
        }
    }

    pub fn braintree_api_base(&self) -> &'static str {
        match self.braintree_env {
            ProviderEnvironment::Production => "https://api.braintreegateway.com",
            ProviderEnvironment::Sandbox => "https://api.sandbox.braintreegateway.com",
        }
    }

    pub fn plaid_api_base(&self) -> &'static str {
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
            braintree_client_id: None,
            braintree_client_secret: None,
            braintree_env: ProviderEnvironment::Sandbox,
            plaid_client_id: None,
            plaid_secret: None,
            plaid_env: PlaidEnvironment::Sandbox,
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
