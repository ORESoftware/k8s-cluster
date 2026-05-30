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
//! - `GET /` — service info + endpoint map.
//! - `GET /simulations` — the engine's full simulation catalogue.
//! - `POST /simulate` — run sims whose name contains `name`, in series, e.g. `{"name":"electric_circuit"}`.
//! - `GET /simulations/:name/run` — convenience GET form of `/simulate`.
//! - `GET /out`, `/out/`, `/out/*path` — serve rendered artifacts (curated `index.html` if present, else a listing).
//! - `GET /docs/api`, `/api/docs` — generated HTML API docs.
//! - `GET /api/docs.json` — machine-readable API docs.
//!
//! Simulations are serialized behind a single lock: the engine drives a
//! process-global clock / RNG and `println!`s its report, so running two at
//! once would interleave output and race shared state (the engine's own
//! `run_all_simulations` is likewise strictly serial).

use std::{
    env,
    net::SocketAddr,
    path::{Path as StdPath, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use des_engine::des::simulations::{run_simulations_matching, simulation_catalogue, SimOutcome};

/// Fast, HTML-producing simulations run once at startup so `/out/` has content
/// immediately. `main_build_site` is run last because it assembles the curated
/// `out/index.html` from whatever HTML the earlier sims rendered. Heavy sims
/// (e.g. `main_dispatch_combo`, `main_stochastic_sde*`) are intentionally
/// excluded; trigger those on demand via `/simulate`. Override with
/// `DES_STARTUP_SIMS` (comma-separated name filters), or set it empty to skip.
const DEFAULT_STARTUP_SIMS: &str = "main_wind_mppt_anim,main_temp_control_anim,main_observability_controllability_anim,main_empirical_control_report,main_elevator_highrise,main_two_disease,main_build_site";

const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;
const MAX_FILTER_LEN: usize = 96;

#[derive(Clone)]
struct AppState {
    /// Absolute path to the directory the engine writes artifacts into
    /// (`<work>/out`). Held as an absolute path so request handlers are immune
    /// to the process `chdir` done at startup.
    out_dir: Arc<PathBuf>,
    /// Serializes simulation runs (the engine is single-clock / single-RNG).
    sim_lock: Arc<Mutex<()>>,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// All simulation names from the engine catalogue, in catalogue order.
fn sim_names() -> Vec<&'static str> {
    simulation_catalogue()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn outcome_json(outcomes: &[SimOutcome]) -> Vec<Value> {
    outcomes
        .iter()
        .map(|o| json!({ "name": o.name, "ok": o.ok, "millis": o.millis }))
        .collect()
}

/// Run every catalogue sim whose name contains `needle`, in series, on a
/// blocking thread, while holding the serial simulation lock.
async fn run_filter(state: &AppState, needle: String) -> Vec<SimOutcome> {
    let _guard = state.sim_lock.lock().await;
    tokio::task::spawn_blocking(move || run_simulations_matching(&needle))
        .await
        .unwrap_or_default()
}

// =============================================================================
// JSON / control routes
// =============================================================================

async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": "dd-des-rs", "atMs": now_ms() }))
}

async fn root() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-des-rs",
        "mode": "runs the discrete-event-system.rs engine (library) and serves rendered HTML",
        "engineSimulations": sim_names().len(),
        "endpoints": {
            "healthz": "GET /healthz",
            "simulations": "GET /simulations",
            "simulate": "POST /simulate  {\"name\":\"<filter>\"}",
            "runNamed": "GET /simulations/:name/run",
            "renderedOutputIndex": "GET /out/",
            "renderedOutputFile": "GET /out/*path",
            "apiDocs": "GET /docs/api",
            "apiDocsJson": "GET /api/docs.json"
        },
        "atMs": now_ms()
    }))
}

async fn list_simulations() -> impl IntoResponse {
    let names = sim_names();
    Json(json!({
        "ok": true,
        "count": names.len(),
        "simulations": names,
    }))
}

#[derive(Debug, Deserialize)]
struct SimulateRequest {
    name: String,
}

fn validate_filter(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("name (a simulation-name filter) must not be empty".to_string());
    }
    if trimmed.len() > MAX_FILTER_LEN {
        return Err(format!("name must be at most {MAX_FILTER_LEN} bytes"));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("name must not contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}

async fn run_response(state: &AppState, needle: String) -> Response {
    let outcomes = run_filter(state, needle.clone()).await;
    if outcomes.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("no simulation name contains `{needle}`"),
                "simulations": sim_names(),
            })),
        )
            .into_response();
    }
    let all_ok = outcomes.iter().all(|o| o.ok);
    Json(json!({
        "ok": all_ok,
        "filter": needle,
        "ran": outcome_json(&outcomes),
        "outputIndex": "/out/",
        "atMs": now_ms(),
    }))
    .into_response()
}

async fn simulate(State(state): State<AppState>, Json(req): Json<SimulateRequest>) -> Response {
    match validate_filter(&req.name) {
        Ok(needle) => run_response(&state, needle).await,
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

async fn run_named(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    match validate_filter(&name) {
        Ok(needle) => run_response(&state, needle).await,
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

// =============================================================================
// Rendered-output serving (HTML / JSON / SVG / PNG / JSONL / CSV …).
//
// The artifacts live in `state.out_dir` (a writable working dir the engine
// renders into). Requests are confined to that directory via canonicalized
// path checks so `..` / symlinks cannot escape it.
// =============================================================================

fn content_type(path: &StdPath) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("jsonl") => "application/x-ndjson; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("md") => "text/markdown; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Canonicalize `requested` and return it only if it stays inside the
/// canonicalized `base`. `None` for traversal, escaping symlinks, or missing
/// paths.
fn resolve_within(base: &StdPath, requested: &StdPath) -> Option<PathBuf> {
    let canon_base = base.canonicalize().ok()?;
    let canon_req = requested.canonicalize().ok()?;
    canon_req.starts_with(&canon_base).then_some(canon_req)
}

fn serve_file(path: &StdPath) -> Response {
    match std::fs::read(path) {
        Ok(bytes) => (
            [
                ("content-type", content_type(path)),
                ("x-content-type-options", "nosniff"),
                ("cache-control", "public, max-age=30"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

const LISTED_EXTENSIONS: [&str; 6] = ["html", "json", "csv", "jsonl", "svg", "png"];

/// Recursively collect servable artifacts under `dir`, returned as
/// forward-slash relative paths sorted alphabetically for a stable listing.
fn collect_artifacts(dir: &StdPath, base: &StdPath, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_artifacts(&path, base, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| LISTED_EXTENSIONS.contains(&e))
            .unwrap_or(false)
        {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn out_redirect() -> Response {
    Redirect::permanent("out/").into_response()
}

async fn out_index(State(state): State<AppState>) -> Response {
    let base: &StdPath = state.out_dir.as_path();

    // Prefer the curated landing page (out/index.html, written by build_site).
    if let Some(index) = resolve_within(base, &base.join("index.html")) {
        if index.is_file() {
            return serve_file(&index);
        }
    }

    // Fallback: generated listing of every rendered artifact.
    let mut files = Vec::new();
    collect_artifacts(base, base, &mut files);
    files.sort();

    let mut items = String::new();
    if files.is_empty() {
        items.push_str(
            "<p class=\"empty\">No artifacts yet. Run a simulation, e.g. \
             <code>curl -X POST :PORT/simulate -H 'content-type: application/json' \
             -d '{\"name\":\"electric_circuit\"}'</code> or \
             <code>GET /simulations/build_site/run</code>.</p>",
        );
    } else {
        items.push_str("<ul>");
        for file in &files {
            let safe = html_escape(file);
            items.push_str(&format!(
                "<li><a href=\"{href}\">{label}</a></li>",
                href = safe,
                label = safe
            ));
        }
        items.push_str("</ul>");
    }

    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>discrete-event-system.rs output</title><style>\
         body{{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;margin:0;\
         background:#0d1117;color:#e6edf3;}}\
         main{{max-width:960px;margin:0 auto;padding:24px 20px 64px;}}\
         h1{{font-size:1.5rem;margin:0 0 4px;}}\
         p.sub{{color:#8b949e;margin:0 0 20px;font-size:.9rem;}}\
         code{{background:#161b22;padding:1px 5px;border-radius:4px;}}\
         ul{{list-style:none;padding:0;margin:0;}}\
         li{{border-bottom:1px solid #21262d;}}\
         li a{{display:block;padding:10px 8px;color:#58a6ff;text-decoration:none;\
         font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.9rem;}}\
         li a:hover{{background:#161b22;}}\
         p.empty{{color:#8b949e;padding:16px 8px;}}</style></head><body><main>\
         <h1>discrete-event-system.rs output</h1>\
         <p class=\"sub\">Artifacts rendered by the Rust DES engine ({count} files).</p>\
         {items}</main></body></html>",
        count = files.len(),
        items = items
    );

    Html(body).into_response()
}

async fn out_file(State(state): State<AppState>, Path(rel_path): Path<String>) -> Response {
    let base: &StdPath = state.out_dir.as_path();

    let Some(target) = resolve_within(base, &base.join(&rel_path)) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    if target.is_dir() {
        if let Some(index) = resolve_within(base, &target.join("index.html")) {
            if index.is_file() {
                return serve_file(&index);
            }
        }
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    if !target.is_file() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    serve_file(&target)
}

// =============================================================================
// API docs — derived at runtime from the route descriptor below (the same
// table the router is built from), per the repo's "no hand-maintained route
// inventory" contract.
// =============================================================================

fn api_routes() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("GET", "/healthz", "Readiness/liveness probe."),
        ("GET", "/", "Service info and endpoint map."),
        (
            "GET",
            "/simulations",
            "List the engine's simulation catalogue.",
        ),
        (
            "POST",
            "/simulate",
            "Run sims whose name contains `name`, in series.",
        ),
        (
            "GET",
            "/simulations/:name/run",
            "Convenience GET form of /simulate.",
        ),
        (
            "GET",
            "/out/",
            "Curated index.html, else a listing of rendered artifacts.",
        ),
        (
            "GET",
            "/out/*path",
            "Serve an individual rendered artifact.",
        ),
        ("GET", "/docs/api", "This HTML API documentation."),
        (
            "GET",
            "/api/docs.json",
            "Machine-readable API documentation.",
        ),
    ]
}

fn api_docs_value() -> Value {
    json!({
        "service": "dd-des-rs",
        "description": "Runs the discrete-event-system.rs engine as a library and serves rendered HTML.",
        "routes": api_routes()
            .into_iter()
            .map(|(method, path, desc)| json!({ "method": method, "path": path, "description": desc }))
            .collect::<Vec<_>>(),
    })
}

async fn api_docs_html() -> Html<String> {
    let mut rows = String::new();
    for (method, path, desc) in api_routes() {
        rows.push_str(&format!(
            "<tr><td>{m}</td><td><code>{p}</code></td><td>{d}</td></tr>",
            m = method,
            p = html_escape(path),
            d = html_escape(desc)
        ));
    }
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>dd-des-rs API</title><style>\
         body{{font-family:system-ui,sans-serif;margin:0;background:#0d1117;color:#e6edf3;}}\
         main{{max-width:900px;margin:0 auto;padding:24px 20px;}}\
         table{{border-collapse:collapse;width:100%;}}\
         td,th{{text-align:left;padding:8px;border-bottom:1px solid #21262d;font-size:.9rem;}}\
         code{{color:#58a6ff;}}</style></head><body><main>\
         <h1>dd-des-rs API</h1><table><tr><th>Method</th><th>Path</th><th>Description</th></tr>\
         {rows}</table></main></body></html>"
    ))
}

async fn api_docs_json() -> impl IntoResponse {
    Json(api_docs_value())
}

// =============================================================================

/// Resolve the writable working directory the engine renders artifacts into.
/// Honors `DES_WORK_DIR`, else a per-process temp dir (the engine writes
/// CWD-relative `out/`, so the process `chdir`s here at startup).
fn work_dir() -> PathBuf {
    if let Ok(dir) = env::var("DES_WORK_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    env::temp_dir().join("dd-des-rs")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    let state = AppState {
        out_dir: Arc::new(out_dir),
        sim_lock: Arc::new(Mutex::new(())),
    };

    // Populate `out/` in the background so /healthz comes up immediately while
    // the startup catalogue renders.
    let startup = env_value("DES_STARTUP_SIMS", DEFAULT_STARTUP_SIMS);
    if !startup.is_empty() {
        let startup_state = state.clone();
        tokio::spawn(async move {
            for needle in startup
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                let outcomes = run_filter(&startup_state, needle.clone()).await;
                println!(
                    "[dd-des-rs] startup `{needle}`: ran {} sim(s)",
                    outcomes.len()
                );
            }
            println!("[dd-des-rs] startup catalogue complete");
        });
    }

    let app = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/simulations", get(list_simulations))
        .route("/simulate", post(simulate))
        .route("/simulations/:name/run", get(run_named))
        .route("/out", get(out_redirect))
        .route("/out/", get(out_index))
        .route("/out/*path", get(out_file))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_exposed_and_nonempty() {
        let names = sim_names();
        assert!(names.len() >= 56, "expected the full engine catalogue");
        assert!(names.contains(&"main_build_site"));
        assert!(names.contains(&"main_electric_circuit"));
    }

    #[test]
    fn filter_validation_rejects_empty_and_oversize() {
        assert!(validate_filter("  ").is_err());
        assert!(validate_filter(&"x".repeat(MAX_FILTER_LEN + 1)).is_err());
        assert_eq!(
            validate_filter("  electric_circuit ").unwrap(),
            "electric_circuit"
        );
    }

    #[test]
    fn content_type_maps_known_and_unknown_extensions() {
        assert_eq!(
            content_type(StdPath::new("a/b.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(StdPath::new("a.svg")), "image/svg+xml");
        assert_eq!(
            content_type(StdPath::new("a.json")),
            "application/json; charset=utf-8"
        );
        assert_eq!(
            content_type(StdPath::new("a.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn resolve_within_confines_to_base_and_blocks_traversal() {
        let root =
            std::env::temp_dir().join(format!("des-rs-test-{}-{}", std::process::id(), now_ms()));
        let base = root.join("out");
        std::fs::create_dir_all(base.join("sub")).expect("create base");
        std::fs::write(base.join("index.html"), b"<h1>ok</h1>").expect("write index");
        std::fs::write(base.join("sub/page.html"), b"<h1>sub</h1>").expect("write sub");
        std::fs::write(root.join("secret.txt"), b"secret").expect("write secret");

        assert!(resolve_within(&base, &base.join("index.html")).is_some());
        assert!(resolve_within(&base, &base.join("sub/page.html")).is_some());
        assert!(resolve_within(&base, &base.join("../secret.txt")).is_none());
        assert!(resolve_within(&base, &base.join("nope.html")).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}
