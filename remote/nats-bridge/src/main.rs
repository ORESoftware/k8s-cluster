//! dd-nats-bridge — hardened HTTP→NATS publish bridge.
//!
//! Security model (the NATS bus itself is unauthenticated today — see
//! `remote/argocd/messaging/readme.md`), so this bridge is a *narrowing*
//! chokepoint, never a widening one:
//!
//! - callers must present the bridge token (fail-closed at startup unless
//!   `BRIDGE_ALLOW_INSECURE=true` is set explicitly for local dev);
//! - only subjects under the configured allowlist prefixes are publishable —
//!   never `$SYS.>`/`$JS.>`, wildcards, or arbitrary fleet subjects;
//! - bodies must be JSON and are capped well below the NATS `max_payload`;
//! - publishes go through JetStream so a work-queue message is acknowledged
//!   durable before the HTTP caller gets a 2xx (subjects not bound to any
//!   stream fall back to core NATS and are reported `durable: false`).
//!
//! Env:
//!   NATS_URL                 default nats://127.0.0.1:4222
//!   NATS_TOKEN / NATS_USER+NATS_PASSWORD   bus credentials (for when bus auth lands)
//!   BRIDGE_TOKEN             shared secret callers send as `Authorization: Bearer`
//!                            or `x-bridge-token` (required)
//!   BRIDGE_ALLOW_INSECURE    "true" to run without BRIDGE_TOKEN (dev only)
//!   BRIDGE_SUBJECT_PREFIXES  comma-separated allowlist, e.g. "dd.vapi.tasks.,vxl."
//!                            (required; there is no permit-all default)
//!   BRIDGE_MAX_BODY_BYTES    default 262144
//!   PORT                     default 3004

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_MAX_BODY: usize = 256 * 1024;
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);

struct AppState {
    nats: async_nats::Client,
    jetstream: async_nats::jetstream::Context,
    bridge_token: Option<String>,
    subject_prefixes: Vec<String>,
    published_total: AtomicU64,
    rejected_total: AtomicU64,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nats_bridge=debug,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let bridge_token = std::env::var("BRIDGE_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let allow_insecure = std::env::var("BRIDGE_ALLOW_INSECURE")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if bridge_token.is_none() && !allow_insecure {
        // Fail closed: an unauthenticated bridge on an unauthenticated bus is
        // an open relay for the whole cluster.
        eprintln!("Fatal: BRIDGE_TOKEN is not set (set BRIDGE_ALLOW_INSECURE=true only for local dev)");
        std::process::exit(1);
    }
    if let Some(t) = &bridge_token {
        if t.len() < 16 {
            eprintln!("Fatal: BRIDGE_TOKEN must be at least 16 characters");
            std::process::exit(1);
        }
    }

    let subject_prefixes = parse_prefixes(
        std::env::var("BRIDGE_SUBJECT_PREFIXES")
            .unwrap_or_default()
            .as_str(),
    );
    if subject_prefixes.is_empty() {
        eprintln!("Fatal: BRIDGE_SUBJECT_PREFIXES is not set; refusing to run as an any-subject relay");
        std::process::exit(1);
    }

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
    tracing::info!(%nats_url, prefixes = ?subject_prefixes, "connecting to NATS");

    let mut opts = async_nats::ConnectOptions::new()
        .name("dd-nats-bridge")
        .retry_on_initial_connect()
        .max_reconnects(None);
    if let Ok(token) = std::env::var("NATS_TOKEN") {
        if !token.trim().is_empty() {
            opts = opts.token(token.trim().to_string());
        }
    } else if let (Ok(user), Ok(pass)) = (std::env::var("NATS_USER"), std::env::var("NATS_PASSWORD"))
    {
        opts = opts.user_and_password(user, pass);
    }
    let nats = match opts.connect(&nats_url).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Fatal: could not start NATS connection to {nats_url}: {e}");
            std::process::exit(1);
        }
    };
    let jetstream = async_nats::jetstream::new(nats.clone());

    let max_body: usize = std::env::var("BRIDGE_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_BODY);

    let state = Arc::new(AppState {
        nats,
        jetstream,
        bridge_token,
        subject_prefixes,
        published_total: AtomicU64::new(0),
        rejected_total: AtomicU64::new(0),
    });

    let app = Router::new()
        .route("/health", get(healthz)) // legacy path, kept for old probes
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/publish/:subject", post(publish_handler))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3004);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = ctrl_c => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await.ok();
    tracing::info!("shutting down");
}

async fn healthz(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "nats": format!("{:?}", state.nats.connection_state()),
        "published_total": state.published_total.load(Ordering::Relaxed),
        "rejected_total": state.rejected_total.load(Ordering::Relaxed),
    }))
}

async fn readyz(State(state): State<Arc<AppState>>) -> Result<&'static str, StatusCode> {
    match state.nats.connection_state() {
        async_nats::connection::State::Connected => Ok("ok"),
        _ => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn publish_handler(
    Path(subject): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let reject = |state: &AppState, status: StatusCode, msg: String| {
        state.rejected_total.fetch_add(1, Ordering::Relaxed);
        Err((status, Json(json!({ "ok": false, "error": msg }))))
    };

    if !caller_authorized(&headers, state.bridge_token.as_deref()) {
        return reject(&state, StatusCode::UNAUTHORIZED, "invalid bridge token".into());
    }

    if let Err(e) = validate_subject(&subject, &state.subject_prefixes) {
        tracing::warn!(%subject, "rejected publish: {e}");
        return reject(&state, StatusCode::FORBIDDEN, e);
    }

    // The bridge relays JSON only; reject other payloads early.
    if serde_json::from_slice::<serde_json::Value>(&body).is_err() {
        return reject(&state, StatusCode::BAD_REQUEST, "body must be valid JSON".into());
    }

    // JetStream first so work-queue messages are durably acked before we
    // return 2xx; subjects not bound to a stream fall back to core NATS.
    let js_result = tokio::time::timeout(
        PUBLISH_TIMEOUT,
        publish_jetstream(&state, &subject, body.clone()),
    )
    .await;

    let durable = match js_result {
        Ok(Ok(())) => true,
        Ok(Err(PublishError::NoStream)) => {
            let core = tokio::time::timeout(PUBLISH_TIMEOUT, async {
                state
                    .nats
                    .publish(subject.clone(), body)
                    .await
                    .map_err(|e| e.to_string())?;
                state.nats.flush().await.map_err(|e| e.to_string())
            })
            .await;
            match core {
                Ok(Ok(())) => false,
                Ok(Err(e)) => {
                    tracing::error!(%subject, "core publish failed: {e}");
                    return reject(&state, StatusCode::BAD_GATEWAY, "publish failed".into());
                }
                Err(_) => {
                    return reject(&state, StatusCode::GATEWAY_TIMEOUT, "publish timed out".into())
                }
            }
        }
        Ok(Err(PublishError::Other(e))) => {
            tracing::error!(%subject, "jetstream publish failed: {e}");
            return reject(&state, StatusCode::BAD_GATEWAY, "publish failed".into());
        }
        Err(_) => return reject(&state, StatusCode::GATEWAY_TIMEOUT, "publish timed out".into()),
    };

    state.published_total.fetch_add(1, Ordering::Relaxed);
    tracing::info!(%subject, durable, "published");
    Ok(Json(json!({ "ok": true, "subject": subject, "durable": durable })))
}

enum PublishError {
    /// No stream is bound to this subject ("no responders" from the JS API).
    NoStream,
    Other(String),
}

async fn publish_jetstream(
    state: &AppState,
    subject: &str,
    body: Bytes,
) -> Result<(), PublishError> {
    let ack = state
        .jetstream
        .publish(subject.to_string(), body)
        .await
        .map_err(|e| classify_js_error(&e.to_string()))?;
    ack.await.map_err(|e| classify_js_error(&e.to_string()))?;
    Ok(())
}

fn classify_js_error(msg: &str) -> PublishError {
    if msg.contains("no responders") {
        PublishError::NoStream
    } else {
        PublishError::Other(msg.to_string())
    }
}

fn caller_authorized(headers: &HeaderMap, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true; // BRIDGE_ALLOW_INSECURE was explicitly set at startup.
    };
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-bridge-token").and_then(|v| v.to_str().ok()));
    match presented {
        Some(got) => constant_time_eq(got.trim(), expected),
        None => false,
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn parse_prefixes(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

fn validate_subject(subject: &str, prefixes: &[String]) -> Result<(), String> {
    if subject.is_empty() || subject.len() > 255 {
        return Err("subject must be 1-255 characters".into());
    }
    if subject.starts_with('$') {
        return Err("system subjects are not publishable through the bridge".into());
    }
    for token in subject.split('.') {
        if token.is_empty() {
            return Err("subject has an empty token".into());
        }
        if token == "*" || token == ">" {
            return Err("wildcard subjects are not publishable".into());
        }
        if !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!("subject token '{token}' has invalid characters"));
        }
    }
    if !prefixes.iter().any(|p| subject.starts_with(p.as_str())) {
        return Err("subject is outside the bridge allowlist".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> Vec<String> {
        parse_prefixes("dd.vapi.tasks.,vxl.")
    }

    #[test]
    fn allows_allowlisted_subject() {
        assert!(validate_subject("dd.vapi.tasks.call", &prefixes()).is_ok());
        assert!(validate_subject("vxl.events.stt", &prefixes()).is_ok());
    }

    #[test]
    fn rejects_out_of_allowlist_subject() {
        assert!(validate_subject("dd.remote.contracts.solana.settle", &prefixes()).is_err());
        assert!(validate_subject("dd.vapi.other", &prefixes()).is_err());
    }

    #[test]
    fn rejects_system_and_wildcard_subjects() {
        assert!(validate_subject("$JS.API.STREAM.DELETE.DD_REMOTE_TASKS", &prefixes()).is_err());
        assert!(validate_subject("$SYS.REQ.SERVER.PING", &prefixes()).is_err());
        assert!(validate_subject("dd.vapi.tasks.>", &prefixes()).is_err());
        assert!(validate_subject("dd.vapi.tasks.*", &prefixes()).is_err());
        assert!(validate_subject("vxl..events", &prefixes()).is_err());
    }

    #[test]
    fn rejects_invalid_characters_and_lengths() {
        assert!(validate_subject("vxl.ev ents", &prefixes()).is_err());
        assert!(validate_subject("", &prefixes()).is_err());
        let long = format!("vxl.{}", "a".repeat(300));
        assert!(validate_subject(&long, &prefixes()).is_err());
    }

    #[test]
    fn token_comparison_is_exact() {
        assert!(constant_time_eq("secret-token-1234", "secret-token-1234"));
        assert!(!constant_time_eq("secret-token-1234", "secret-token-1235"));
        assert!(!constant_time_eq("short", "secret-token-1234"));
    }

    #[test]
    fn bearer_and_header_tokens_are_accepted() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer tok-abcdef123456".parse().unwrap());
        assert!(caller_authorized(&h, Some("tok-abcdef123456")));

        let mut h2 = HeaderMap::new();
        h2.insert("x-bridge-token", "tok-abcdef123456".parse().unwrap());
        assert!(caller_authorized(&h2, Some("tok-abcdef123456")));

        assert!(!caller_authorized(&HeaderMap::new(), Some("tok-abcdef123456")));
    }
}
