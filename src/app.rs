use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) client: reqwest::Client,
    pub(crate) backend_url: String,
    pub(crate) base_path: String,
    pub(crate) supabase_url: Option<String>,
    pub(crate) supabase_anon_key: Option<String>,
    pub(crate) started: Instant,
}

impl AppState {
    pub(crate) fn from_env() -> Self {
        Self {
            client: reqwest::Client::new(),
            backend_url: env_string("AKRION_BACKEND_URL")
                .unwrap_or_else(|| "http://127.0.0.1:8113".to_string()),
            base_path: normalize_base_path(env_string("AKRION_WEB_BASE_PATH").unwrap_or_default()),
            supabase_url: env_string("SUPABASE_URL"),
            supabase_anon_key: env_string("SUPABASE_ANON_KEY"),
            started: Instant::now(),
        }
    }

    pub(crate) fn path(&self, path: &str) -> String {
        let suffix = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

        if self.base_path.is_empty() {
            suffix
        } else if suffix == "/" {
            self.base_path.clone()
        } else {
            format!("{}{}", self.base_path, suffix)
        }
    }

    pub(crate) fn supabase_ready(&self) -> bool {
        self.supabase_url.is_some() && self.supabase_anon_key.is_some()
    }

    pub(crate) fn public_config(&self) -> PublicConfig {
        PublicConfig {
            backend_url: self.backend_url.clone(),
            base_path: self.base_path.clone(),
            supabase: SupabasePublicConfig {
                enabled: self.supabase_ready(),
                url: self.supabase_url.clone(),
                anon_key: self.supabase_anon_key.clone(),
            },
        }
    }
}

#[derive(Serialize)]
pub(crate) struct PublicConfig {
    backend_url: String,
    base_path: String,
    supabase: SupabasePublicConfig,
}

#[derive(Serialize)]
struct SupabasePublicConfig {
    enabled: bool,
    url: Option<String>,
    anon_key: Option<String>,
}

pub(crate) fn asset_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

pub(crate) fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_base_path(value: String) -> String {
    let trimmed = value.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

pub(crate) fn init_tracing() {
    let filter = EnvFilter::try_from_env("AKRION_RUST_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,akrion_web_server=info,tower_http=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().compact())
        .try_init()
        .ok();
}

pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
