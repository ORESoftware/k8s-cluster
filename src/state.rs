//! Shared application state, cloned cheaply into every request (all heavy
//! members are behind `Arc` or are themselves clone-by-handle).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::cache::Cache;
use crate::config::AppConfig;
use crate::db::DbStore;
use crate::factors::FactorService;
use crate::metrics::Metrics;
use crate::supabase::ProjectRegistry;
use crate::token::TokenMinter;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    /// Verifies incoming Supabase tokens, routing by issuer to the right project.
    pub supabase: Arc<ProjectRegistry>,
    /// Mints and exposes our own unified OreSoftware JWTs.
    pub minter: Arc<TokenMinter>,
    /// Postgres identity/session authority (optional only in explicit DB-less development).
    pub db: Option<DbStore>,
    /// Durable factor and ceremony authority. It is absent only in explicit
    /// DB-less development mode; individual methods remain disabled unless
    /// their encryption or WebAuthn configuration is present.
    pub factors: Option<FactorService>,
    /// Optional, non-authoritative Redis/Valkey acceleration.
    pub cache: Option<Cache>,
    /// Outbound client for JWKS fetches (kept warm; connection-pooled).
    pub http: reqwest::Client,
    /// Prometheus counters.
    pub metrics: Metrics,
}

impl AppState {
    pub async fn build(config: AppConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("shared-auth-server/0.1")
            .build()
            .context("building http client")?;

        let supabase = ProjectRegistry::from_projects(&config.projects)
            .context("building project registry")?;

        let minter = TokenMinter::from_config(&config.signing).context("building token minter")?;

        let (db, factors) = match &config.db {
            Some(db_cfg) => {
                let db = DbStore::connect(db_cfg)
                    .await
                    .context("connecting to RDS")?;
                let factors = FactorService::connect(db_cfg)
                    .await
                    .context("initializing durable factor service")?;
                (Some(db), Some(factors))
            }
            None => {
                tracing::warn!("AUTH_DATABASE_URL unset — explicit DB-less development mode");
                (None, None)
            }
        };

        let cache = match &config.redis {
            Some(redis_config) => match Cache::connect(redis_config).await {
                Ok(cache) => Some(cache),
                Err(error) => {
                    tracing::warn!(%error, "Redis unavailable; continuing with Postgres as authority");
                    None
                }
            },
            None => None,
        };

        Ok(Self {
            config: Arc::new(config),
            supabase: Arc::new(supabase),
            minter: Arc::new(minter),
            db,
            factors,
            cache,
            http,
            metrics: Metrics::new(),
        })
    }
}
