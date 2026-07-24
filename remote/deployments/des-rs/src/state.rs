use std::{
    env,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tokio::sync::Mutex;

use soccer_engine::soccer::SoccerLiveHttpBridge;

use crate::soccer::SoccerLiveWsHub;

#[derive(Clone)]
pub(crate) struct AppState {
    /// Absolute path to the directory the engine writes artifacts into
    /// (`<work>/out`). Held as an absolute path so request handlers are immune
    /// to the process `chdir` done at startup.
    pub(crate) out_dir: Arc<PathBuf>,
    /// Serializes simulation runs (the engine is single-clock / single-RNG).
    pub(crate) sim_lock: Arc<Mutex<()>>,
    /// Discovery `Link` header (relative RFC 8288 targets) emitted on the
    /// canonical landing routes so a machine can find the docs from `/` alone.
    pub(crate) link_header: Arc<str>,
    /// `dd-server-api-docs` discovery header value (relative).
    pub(crate) dd_docs_header: Arc<str>,
    /// The independently-rendered HTML docs page (a view over the descriptor).
    pub(crate) docs_html: Arc<str>,
    /// The canonical machine-readable descriptor JSON (`/api/docs.json`).
    pub(crate) docs_json: Arc<str>,
    /// Pre-rendered HTML for the new FEL elevator artifacts. These are
    /// deterministic (fixed seeds / tabular solves), so they are rendered once
    /// at startup and served verbatim — no per-request engine run, no lock.
    pub(crate) elevator_fel_html: Arc<str>,
    pub(crate) elevator_mdp_html: Arc<str>,
    pub(crate) elevator_pomdp_html: Arc<str>,
    /// Pre-rendered household bathroom occupancy Monte-Carlo animation. The
    /// study is self-contained and deterministic (fixed seeds), so it is
    /// rendered once at startup and served verbatim at `/bathrooms`.
    pub(crate) bathrooms_html: Arc<str>,
    /// Framework-based variant of the bathroom study (MovingEntity people,
    /// StationaryEntity bathrooms, visual-block animation player), served at
    /// `/two-bathrooms`. Also pre-rendered once at startup.
    pub(crate) two_bathrooms_html: Arc<str>,
    /// Interactive 11-a-side rotation planner (roster constraints + re-solve).
    pub(crate) soccer_planner_html: Arc<str>,
    /// Live 2D soccer gameplay bridge with its own shared session state.
    pub(crate) soccer_live_bridge: Arc<SoccerLiveHttpBridge>,
    /// WebSocket fan-out for `/api/ws`: per-game broadcast + single-driver
    /// election so spectators receive pushed frames instead of each re-stepping
    /// the shared session over HTTP. The HTTP `/api/*` path is unchanged.
    pub(crate) soccer_live_ws: Arc<SoccerLiveWsHub>,
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn env_value_or_empty(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn json_error(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(json!({ "ok": false, "error": error.into() }))).into_response()
}

// =============================================================================

/// Resolve the writable working directory the engine renders artifacts into.
/// Honors `DES_WORK_DIR`, else a per-process temp dir (the engine writes
/// CWD-relative `out/`, so the process `chdir`s here at startup).
pub(crate) fn work_dir() -> PathBuf {
    if let Ok(dir) = env::var("DES_WORK_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    env::temp_dir().join("dd-des-rs")
}
