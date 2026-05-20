use std::env;

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
    pub paypal_client_id: Option<String>,
    pub paypal_client_secret: Option<String>,
    pub braintree_client_id: Option<String>,
    pub braintree_client_secret: Option<String>,
    pub plaid_client_id: Option<String>,
    pub plaid_secret: Option<String>,

    pub oauth_redirect_base: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolanaCluster {
    Mainnet,
    Devnet,
    Localnet,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();

        Ok(Self {
            host: env::var("BILLING_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("BILLING_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8087),
            database_url: env::var("BILLING_DATABASE_URL")
                .or_else(|_| env::var("DATABASE_URL"))
                .map_err(|_| anyhow::anyhow!("BILLING_DATABASE_URL or DATABASE_URL must be set"))?,
            run_migrations: env::var("BILLING_RUN_MIGRATIONS")
                .map(|s| s != "0" && s.to_ascii_lowercase() != "false")
                .unwrap_or(true),

            master_seal_key_b64: env::var("BILLING_MASTER_SEAL_KEY")
                .map_err(|_| anyhow::anyhow!(
                    "BILLING_MASTER_SEAL_KEY must be set (base64 of a 32-byte key, \
                     normally provided by KMS/SealedSecrets)"
                ))?,

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
            paypal_client_id: env::var("PAYPAL_CLIENT_ID").ok(),
            paypal_client_secret: env::var("PAYPAL_CLIENT_SECRET").ok(),
            braintree_client_id: env::var("BRAINTREE_CLIENT_ID").ok(),
            braintree_client_secret: env::var("BRAINTREE_CLIENT_SECRET").ok(),
            plaid_client_id: env::var("PLAID_CLIENT_ID").ok(),
            plaid_secret: env::var("PLAID_SECRET").ok(),

            oauth_redirect_base: env::var("BILLING_OAUTH_REDIRECT_BASE")
                .unwrap_or_else(|_| "http://localhost:8087".into()),
        })
    }
}
