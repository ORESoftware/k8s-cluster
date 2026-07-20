//! Shared application state, cloned cheaply into every request (all heavy
//! members are behind `Arc` or are themselves clone-by-handle).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::config::AppConfig;
use crate::db::UserStore;
use crate::supabase::ProjectRegistry;
use crate::token::TokenMinter;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    /// Verifies incoming Supabase tokens, routing by issuer to the right project.
    pub supabase: Arc<ProjectRegistry>,
    /// Mints and exposes our own unified OreSoftware JWTs.
    pub minter: Arc<TokenMinter>,
    /// Optional RDS identity mirror.
    pub db: Option<UserStore>,
    /// Outbound client for JWKS fetches (kept warm; connection-pooled).
    pub http: reqwest::Client,
}

impl AppState {
    pub async fn build(config: AppConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .user_agent("shared-auth-server/0.1")
            .build()
            .context("building http client")?;

        let supabase = ProjectRegistry::from_projects(&config.projects)
            .context("building project registry")?;

        let minter = TokenMinter::from_config(&config.signing).context("building token minter")?;

        let db = match &config.db {
            Some(db_cfg) => Some(
                UserStore::connect(db_cfg)
                    .await
                    .context("connecting to RDS")?,
            ),
            None => {
                tracing::warn!("AUTH_DATABASE_URL unset — identity mirroring disabled");
                None
            }
        };

        Ok(Self {
            config: Arc::new(config),
            supabase: Arc::new(supabase),
            minter: Arc::new(minter),
            db,
            http,
        })
    }
}
