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
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Directory of the built Astro site. Defaults to the bundled `static/`
    // (populated from canonical-frontend's `dist/` at build time), but can be
    // pointed straight at the frontend dist via STATIC_DIR for local dev.
    let static_dir: PathBuf = std::env::var("STATIC_DIR")
        .unwrap_or_else(|_| "static".to_string())
        .into();

    let app = build_router(&static_dir).layer(TraceLayer::new_for_http());

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

/// Builds the application router. Kept separate from `main` so unit tests can
/// exercise the routes without binding a socket.
///
/// `/api/*` is mounted first so it takes precedence over the static site.
/// Everything else is served from `static_dir`: directory requests resolve to
/// `index.html`, and unknown paths fall back to the SPA-style index so client
/// routing keeps working.
fn build_router(static_dir: &std::path::Path) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/info", get(info));

    let serve_dir = ServeDir::new(static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(static_dir.join("index.html")));

    Router::new()
        // Plain liveness/readiness endpoint for k8s probes (served at root,
        // bypassing the gateway prefix so probes hit the pod directly).
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback_service(serve_dir)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    fn router() -> Router {
        // Point the static fallback at a directory that does not exist; these
        // tests only exercise the API/probe routes, which take precedence.
        build_router(std::path::Path::new("static"))
    }

    async fn get_json(path: &str) -> (StatusCode, serde_json::Value) {
        let res = router()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn healthz_is_bare_ok() {
        let res = router()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_health_reports_service() {
        let (status, body) = get_json("/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], SERVICE);
    }

    #[tokio::test]
    async fn api_info_reports_version_and_domain() {
        let (status, body) = get_json("/api/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["service"], SERVICE);
        assert_eq!(body["domain"], "canonical.cloud");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }
}
