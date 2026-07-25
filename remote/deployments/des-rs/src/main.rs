//! `dd-des-rs` — HTTP server that runs the `discrete-event-system.rs` engine as
//! a library and serves the HTML result pages its simulations render.
//!
//! Unlike `dd-des-simulator` (which has its own generic event-queue engine and
//! serves the *TypeScript* submodule's pre-committed `out/`), this service
//! imports the real Rust `des_engine` crate (the `discrete-event-system.rs`
//! submodule) and *runs* its simulation catalogue on demand. Each simulation
//! writes its artifacts (`out/*.html`, `out/*-framework.json`, JSONL frames,
//! …) into a writable working directory, and the service serves them.
//!
//! ## HTTP API
//!
//! - `GET /healthz` — readiness/liveness probe.
//! - `GET /` — interactive landing page with per-simulation "Run" buttons.
//! - `GET /info` — service info + endpoint map (JSON).
//! - `GET /simulations` — the engine's full simulation catalogue.
//! - `POST /simulate` — run sims by `name` (filter, or exact via `{"exact":true}`), in series.
//! - `GET /simulations/:name/run` — convenience GET form (`?exact=1` for one entry).
//! - `GET /models` — first-class model registry with example specs.
//! - `GET /models/:kind/run` — run a kind's example spec and render an interactive player (`?format=json` for the raw artifact).
//! - `POST /models/:kind/run` — run a user-supplied JSON spec for a kind (renders a player; `?format=json` for the artifact).
//! - `GET /streaming` — JSONL streaming-solver contracts (lp, milp, mdp, pomdp, soccer-planner).
//! - `POST /streaming/:name` — stream JSONL commands to a solver; responds with a JSONL frame stream.
//! - `GET /soccer/planner` — interactive 11-a-side rotation planner UI.
//! - `POST /soccer/planner/solve` — re-solve the planner request with the Rust IP/MIP solver.
//! - `POST /soccer/planner/stream` — soccer planner JSONL stream alias.
//! - `GET /soccer/live` — live 2D 11v11 soccer UI with soft-real-time controls.
//! - `GET|POST /api/*` — live soccer bridge API used by `/soccer/live`.
//! - `GET /music` — generative music production workbench UI.
//! - `POST /music/sample-seed` — upload or link a 10-50s MP4 plus a prompt and render a WAV variation.
//!   Public and authenticated social/media links are supported via direct HTTP
//!   headers or `yt-dlp` cookies.
//! - `GET /delivery-planner.html` — friendly redirect to the delivery planner artifact.
//! - `GET /deliver-planner.html` — typo-compatible redirect to the delivery planner artifact.
//! - `GET /elevator-fel` — the new next-event (FEL) elevator simulation, animated.
//! - `GET /elevator-mdp` — elevator-dispatch MDP player (value-iterated).
//! - `GET /elevator-pomdp` — elevator-dispatch POMDP player (noisy call button; belief-tracked).
//! - `GET /out`, `/out/`, `/out/*path` — serve rendered artifacts (curated `index.html` if present, else a listing).
//! - `GET /docs/api`, `/api/docs` — generated HTML API docs.
//! - `GET /api/docs.json` — machine-readable API docs.
//!
//! Simulations are serialized behind a single lock: the engine drives a
//! process-global clock / RNG and `println!`s its report, so running two at
//! once would interleave output and race shared state (the engine's own
//! `run_all_simulations` is likewise strictly serial).

use std::{env, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::DefaultBodyLimit,
    routing::{any, get, post},
    Router,
};
use tokio::sync::Mutex;

use des_engine::des::bathrooms::render_default_bathrooms_html;
use des_engine::des::fel::elevator::{
    elevator_mdp_spec, elevator_pomdp_spec, render_elevator_html, run_fel_elevator, ElevatorConfig,
};
use des_engine::des::two_bathrooms::render_default_two_bathrooms_html;
use soccer_engine::soccer::{SoccerLiveHttpBridge, SoccerLiveServerConfig};
use soccer_engine::soccer_planner::planner_page_html;

mod docs;
mod models;
mod music;
mod output;
mod pages;
mod service;
mod showcase;
mod sims;
mod soccer;
mod state;

use crate::docs::{api_docs_html, api_docs_json, build_descriptor, render_docs_html};
use crate::models::{
    list_models, list_streaming, model_run_example, model_run_post, streaming_run,
};
use crate::music::{music_production_page, music_sample_seed_render, MAX_MUSIC_UPLOAD_BYTES};
use crate::output::{delivery_planner_redirect, out_file, out_index, out_redirect};
use crate::service::{healthz, info, root};
use crate::showcase::{
    bathrooms, elevator_fel, elevator_mdp, elevator_pomdp, render_model_player, two_bathrooms,
};
use crate::sims::{
    checked_sim_names, list_simulations, run_filter, run_named, simulate, SimMatchError,
    DEFAULT_STARTUP_SIMS,
};
use crate::soccer::{
    soccer_live_bridge_request, soccer_live_ws, soccer_planner_page, soccer_planner_solve,
    soccer_planner_stream, SoccerLiveWsHub,
};
use crate::state::{env_value, env_value_or_empty, work_dir, AppState};

// Generous enough for model specs and JSONL streaming command batches, while
// still bounding memory per request (simulations themselves take no body).
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Plain stdout tracing subscriber (see Cargo.toml: dd-telemetry lives in a
    // private submodule the in-pod build cannot fetch). Mirrors dd-soccer-rs.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8112").parse::<u16>()?;

    // The engine writes artifacts to `out/` relative to the process CWD. Point
    // the process at a writable working dir so this works under a read-only
    // root filesystem (k8s mounts the repo read-only and gives us /tmp).
    let work = work_dir();
    std::fs::create_dir_all(work.join("out"))?;
    env::set_current_dir(&work)?;
    let out_dir = work
        .join("out")
        .canonicalize()
        .unwrap_or_else(|_| work.join("out"));

    // Build the machine-readable service descriptor once (the JSON-first
    // contract owned by the engine library), then precompute the HTML view and
    // the discovery headers so request handlers stay allocation-light.
    let descriptor = build_descriptor();
    let link_header: Arc<str> = Arc::from(descriptor.link_header_relative());
    let dd_docs_header: Arc<str> = Arc::from(descriptor.dd_api_docs_relative());
    let docs_html: Arc<str> = Arc::from(render_docs_html(&descriptor));
    let docs_json: Arc<str> = Arc::from(descriptor.to_json_string());

    // Pre-render the (deterministic) FEL elevator artifacts once. Done before
    // the server starts serving and before the startup catalogue task spawns, so
    // there is no contention on the engine's process-global clock/RNG.
    let elevator_fel_html: Arc<str> = Arc::from(render_elevator_html(&run_fel_elevator(
        &ElevatorConfig::default(),
    )));
    let elevator_mdp_html: Arc<str> = Arc::from(render_model_player("mdp", &elevator_mdp_spec()));
    let elevator_pomdp_html: Arc<str> =
        Arc::from(render_model_player("pomdp", &elevator_pomdp_spec()));
    let soccer_planner_html: Arc<str> = Arc::from(planner_page_html());
    // Household bathroom occupancy study (8 people, 2 bathrooms) — a blocking
    // Monte-Carlo DES with a self-contained animation. Deterministic, so render
    // once at startup like the elevator artifacts.
    let bathrooms_html: Arc<str> = Arc::from(render_default_bathrooms_html());
    // Framework-based variant: entity/visual-block animation player.
    let two_bathrooms_html: Arc<str> = Arc::from(render_default_two_bathrooms_html());
    let soccer_live_bridge = Arc::new(SoccerLiveHttpBridge::new(SoccerLiveServerConfig::default()));

    let state = AppState {
        out_dir: Arc::new(out_dir),
        sim_lock: Arc::new(Mutex::new(())),
        link_header,
        dd_docs_header,
        docs_html,
        docs_json,
        elevator_fel_html,
        elevator_mdp_html,
        elevator_pomdp_html,
        bathrooms_html,
        two_bathrooms_html,
        soccer_planner_html,
        soccer_live_bridge,
        soccer_live_ws: Arc::new(SoccerLiveWsHub::default()),
    };

    // Populate `out/` in the background so /healthz comes up immediately while
    // the startup catalogue renders.
    let startup = env_value_or_empty("DES_STARTUP_SIMS", DEFAULT_STARTUP_SIMS);
    if !startup.is_empty() {
        let startup_state = state.clone();
        tokio::spawn(async move {
            for needle in startup
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                if let Err(SimMatchError::TooMany { count, .. }) = checked_sim_names(&needle, false)
                {
                    tracing::error!(
                        "[dd-des-rs] startup `{needle}` skipped: filter matches {count} sim(s); use narrower DES_STARTUP_SIMS entries"
                    );
                    continue;
                }
                let outcomes = run_filter(&startup_state, needle.clone(), false).await;
                tracing::info!(
                    "[dd-des-rs] startup `{needle}`: ran {} sim(s)",
                    outcomes.len()
                );
            }
            tracing::info!("[dd-des-rs] startup catalogue complete");
        });
    }

    let app = Router::new()
        .route("/", get(root))
        .route("/info", get(info))
        .route("/healthz", get(healthz))
        .route("/simulations", get(list_simulations))
        .route("/simulate", post(simulate))
        .route("/simulations/:name/run", get(run_named))
        .route("/models", get(list_models))
        .route(
            "/models/:kind/run",
            get(model_run_example).post(model_run_post),
        )
        .route("/streaming", get(list_streaming))
        .route("/streaming/:name", post(streaming_run))
        .route("/elevator-fel", get(elevator_fel))
        .route("/elevator-mdp", get(elevator_mdp))
        .route("/elevator-pomdp", get(elevator_pomdp))
        .route("/bathrooms", get(bathrooms))
        .route("/two-bathrooms", get(two_bathrooms))
        .route("/soccer/planner", get(soccer_planner_page))
        .route("/soccer/planner/solve", post(soccer_planner_solve))
        .route("/soccer/planner/stream", post(soccer_planner_stream))
        .route("/api/ws", get(soccer_live_ws))
        .route("/soccer/live", any(soccer_live_bridge_request))
        .route("/soccer/live/*path", any(soccer_live_bridge_request))
        .route("/fresh", any(soccer_live_bridge_request))
        .route("/new-match", any(soccer_live_bridge_request))
        .route("/new_match", any(soccer_live_bridge_request))
        .route("/reset", any(soccer_live_bridge_request))
        .route("/api/*path", any(soccer_live_bridge_request))
        .route("/music", get(music_production_page))
        .route(
            "/music/sample-seed",
            post(music_sample_seed_render).layer(DefaultBodyLimit::max(MAX_MUSIC_UPLOAD_BYTES)),
        )
        .route("/delivery-planner.html", get(delivery_planner_redirect))
        .route("/deliver-planner.html", get(delivery_planner_redirect))
        .route("/out", get(out_redirect))
        .route("/out/", get(out_index))
        .route("/out/*path", get(out_file))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!(
        "dd-des-rs listening on http://{addr} (out dir: {})",
        work.join("out").display()
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}
