use axum::{http::StatusCode, routing::get, Json, Router};
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

const SERVICE: &str = "canonical-backend";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Directory of the built Astro site. Defaults to the bundled `static/`
    // (populated from canonical-frontend's `dist/` at build time), but can be
    // pointed straight at the frontend dist via STATIC_DIR for local dev.
    let static_dir: PathBuf = std::env::var("STATIC_DIR")
        .unwrap_or_else(|_| "static".to_string())
        .into();

    // JSON API. Mounted under /api so it takes precedence over the static site.
    let api = Router::new()
        .route("/health", get(health))
        .route("/info", get(info));

    // Everything else is served from the static Astro build. Requests for
    // directories resolve to index.html, and unknown paths fall back to the
    // SPA-style index so client routing keeps working.
    let serve_dir = ServeDir::new(&static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(static_dir.join("index.html")));

    let app = Router::new()
        // Plain liveness/readiness endpoint for k8s probes (served at root,
        // bypassing the gateway prefix so probes hit the pod directly).
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback_service(serve_dir)
        .layer(TraceLayer::new_for_http());

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8081);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!(
        "{SERVICE} listening on http://{addr} (serving {})",
        static_dir.display()
    );
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": SERVICE }))
}

async fn info() -> Json<serde_json::Value> {
    Json(json!({
        "service": SERVICE,
        "version": env!("CARGO_PKG_VERSION"),
        "domain": "canonical.cloud",
    }))
}
