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
//! - durable subject families fail closed unless JetStream acknowledges the
//!   write; explicitly non-durable subjects may fall back to core NATS;
//! - publish concurrency is bounded and optional message IDs map to
//!   `Nats-Msg-Id` for JetStream de-duplication.
//!
//! Env:
//!   NATS_URL                 default nats://127.0.0.1:4222
//!   NATS_TOKEN / NATS_USER+NATS_PASSWORD   bus credentials (for when bus auth lands)
//!   BRIDGE_TOKEN             shared secret callers send as `Authorization: Bearer`
//!                            or `x-bridge-token` (required)
//!   BRIDGE_ALLOW_INSECURE    "true" to run without BRIDGE_TOKEN (dev only)
//!   BRIDGE_SUBJECT_PREFIXES  comma-separated allowlist, e.g. "dd.vapi.tasks.,vxl."
//!                            (required; there is no permit-all default)
//!   BRIDGE_DURABLE_SUBJECT_PREFIXES  subset that must receive a JetStream ACK
//!   BRIDGE_MAX_BODY_BYTES    default 262144
//!   BRIDGE_MAX_IN_FLIGHT     default 64; excess requests receive HTTP 429
//!   BRIDGE_PUBLISH_TIMEOUT_MS default 5000
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
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_MAX_BODY: usize = 256 * 1024;
const DEFAULT_MAX_IN_FLIGHT: usize = 64;
const DEFAULT_PUBLISH_TIMEOUT_MS: u64 = 5_000;

struct AppState {
    nats: async_nats::Client,
    jetstream: async_nats::jetstream::Context,
    bridge_token: Option<String>,
    subject_prefixes: Vec<String>,
    durable_subject_prefixes: Vec<String>,
    publish_timeout: Duration,
    publish_slots: Arc<Semaphore>,
    published_total: AtomicU64,
    durable_published_total: AtomicU64,
    core_published_total: AtomicU64,
    duplicate_total: AtomicU64,
    overloaded_total: AtomicU64,
    durability_rejected_total: AtomicU64,
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
        eprintln!(
            "Fatal: BRIDGE_TOKEN is not set (set BRIDGE_ALLOW_INSECURE=true only for local dev)"
        );
        std::process::exit(1);
    }
    if bridge_token.as_ref().is_some_and(|t| t.len() < 16) {
        eprintln!("Fatal: BRIDGE_TOKEN must be at least 16 characters");
        std::process::exit(1);
    }

    let subject_prefixes = parse_prefixes(
        std::env::var("BRIDGE_SUBJECT_PREFIXES")
            .unwrap_or_default()
            .as_str(),
    );
    if subject_prefixes.is_empty() {
        eprintln!(
            "Fatal: BRIDGE_SUBJECT_PREFIXES is not set; refusing to run as an any-subject relay"
        );
        std::process::exit(1);
    }
    let durable_subject_prefixes = parse_prefixes(
        std::env::var("BRIDGE_DURABLE_SUBJECT_PREFIXES")
            .unwrap_or_default()
            .as_str(),
    );
    if let Err(error) = validate_durable_prefixes(&subject_prefixes, &durable_subject_prefixes) {
        eprintln!("Fatal: {error}");
        std::process::exit(1);
    }

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
    tracing::info!(
        prefixes = ?subject_prefixes,
        durable_prefixes = ?durable_subject_prefixes,
        "connecting to configured NATS endpoint"
    );

    let mut opts = async_nats::ConnectOptions::new()
        .name("dd-nats-bridge")
        .retry_on_initial_connect()
        .max_reconnects(None);
    if let Ok(token) = std::env::var("NATS_TOKEN") {
        if !token.trim().is_empty() {
            opts = opts.token(token.trim().to_string());
        }
    } else if let (Ok(user), Ok(pass)) =
        (std::env::var("NATS_USER"), std::env::var("NATS_PASSWORD"))
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
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_BODY);
    let max_in_flight = std::env::var("BRIDGE_MAX_IN_FLIGHT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=4096).contains(value))
        .unwrap_or(DEFAULT_MAX_IN_FLIGHT);
    let publish_timeout_ms = std::env::var("BRIDGE_PUBLISH_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (100..=30_000).contains(value))
        .unwrap_or(DEFAULT_PUBLISH_TIMEOUT_MS);

    let state = Arc::new(AppState {
        nats,
        jetstream,
        bridge_token,
        subject_prefixes,
        durable_subject_prefixes,
        publish_timeout: Duration::from_millis(publish_timeout_ms),
        publish_slots: Arc::new(Semaphore::new(max_in_flight)),
        published_total: AtomicU64::new(0),
        durable_published_total: AtomicU64::new(0),
        core_published_total: AtomicU64::new(0),
        duplicate_total: AtomicU64::new(0),
        overloaded_total: AtomicU64::new(0),
        durability_rejected_total: AtomicU64::new(0),
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
        "durable_published_total": state.durable_published_total.load(Ordering::Relaxed),
        "core_published_total": state.core_published_total.load(Ordering::Relaxed),
        "duplicate_total": state.duplicate_total.load(Ordering::Relaxed),
        "overloaded_total": state.overloaded_total.load(Ordering::Relaxed),
        "durability_rejected_total": state.durability_rejected_total.load(Ordering::Relaxed),
        "rejected_total": state.rejected_total.load(Ordering::Relaxed),
        "publish_slots_available": state.publish_slots.available_permits(),
        "publish_timeout_ms": state.publish_timeout.as_millis(),
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
        return reject(
            &state,
            StatusCode::UNAUTHORIZED,
            "invalid bridge token".into(),
        );
    }

    if let Err(e) = validate_subject(&subject, &state.subject_prefixes) {
        tracing::warn!(%subject, "rejected publish: {e}");
        return reject(&state, StatusCode::FORBIDDEN, e);
    }

    let message_id = match request_message_id(&headers) {
        Ok(value) => value,
        Err(error) => return reject(&state, StatusCode::BAD_REQUEST, error),
    };

    // The bridge relays JSON only; reject other payloads early.
    if serde_json::from_slice::<serde_json::Value>(&body).is_err() {
        return reject(
            &state,
            StatusCode::BAD_REQUEST,
            "body must be valid JSON".into(),
        );
    }

    // Shed excess work before it can allocate an unbounded queue of HTTP tasks
    // waiting on JetStream ACKs. Callers get an explicit retryable 429.
    let _permit = match state.publish_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            state.overloaded_total.fetch_add(1, Ordering::Relaxed);
            return reject(
                &state,
                StatusCode::TOO_MANY_REQUESTS,
                "bridge publish concurrency limit reached; retry with backoff".into(),
            );
        }
    };

    let durable_required = subject_matches_prefixes(&subject, &state.durable_subject_prefixes);
    let js_result = tokio::time::timeout(
        state.publish_timeout,
        publish_jetstream(&state, &subject, body.clone(), message_id.as_deref()),
    )
    .await;

    match js_result {
        Ok(Ok(ack)) => {
            state.published_total.fetch_add(1, Ordering::Relaxed);
            state
                .durable_published_total
                .fetch_add(1, Ordering::Relaxed);
            if ack.duplicate {
                state.duplicate_total.fetch_add(1, Ordering::Relaxed);
            }
            tracing::info!(
                %subject,
                stream = %ack.stream,
                sequence = ack.sequence,
                duplicate = ack.duplicate,
                "durably published"
            );
            Ok(Json(json!({
                "ok": true,
                "subject": subject,
                "durable": true,
                "idempotent": message_id.is_some(),
                "messageId": message_id,
                "stream": ack.stream,
                "sequence": ack.sequence,
                "duplicate": ack.duplicate,
            })))
        }
        Ok(Err(PublishError::NoStream)) if durable_required => {
            state
                .durability_rejected_total
                .fetch_add(1, Ordering::Relaxed);
            tracing::error!(%subject, "durable subject is not bound to a JetStream stream");
            reject(
                &state,
                StatusCode::SERVICE_UNAVAILABLE,
                "durable subject is not bound to a JetStream stream".into(),
            )
        }
        Ok(Err(PublishError::NoStream)) => {
            let core = tokio::time::timeout(state.publish_timeout, async {
                state
                    .nats
                    .publish(subject.clone(), body)
                    .await
                    .map_err(|e| e.to_string())?;
                state.nats.flush().await.map_err(|e| e.to_string())
            })
            .await;
            match core {
                Ok(Ok(())) => {
                    state.published_total.fetch_add(1, Ordering::Relaxed);
                    state.core_published_total.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(%subject, "published through explicitly non-durable core NATS fallback");
                    Ok(Json(json!({
                        "ok": true,
                        "subject": subject,
                        "durable": false,
                        "idempotent": false,
                        "messageId": message_id,
                        "duplicate": false,
                    })))
                }
                Ok(Err(e)) => {
                    tracing::error!(%subject, "core publish failed: {e}");
                    reject(&state, StatusCode::BAD_GATEWAY, "publish failed".into())
                }
                Err(_) => reject(
                    &state,
                    StatusCode::GATEWAY_TIMEOUT,
                    "publish timed out".into(),
                ),
            }
        }
        Ok(Err(PublishError::Other(e))) => {
            tracing::error!(%subject, "jetstream publish failed: {e}");
            reject(&state, StatusCode::BAD_GATEWAY, "publish failed".into())
        }
        Err(_) => reject(
            &state,
            StatusCode::GATEWAY_TIMEOUT,
            "publish timed out".into(),
        ),
    }
}

enum PublishError {
    /// No stream is bound to this subject ("no responders" from the JS API).
    NoStream,
    Other(String),
}

struct DurableAck {
    stream: String,
    sequence: u64,
    duplicate: bool,
}

async fn publish_jetstream(
    state: &AppState,
    subject: &str,
    body: Bytes,
    message_id: Option<&str>,
) -> Result<DurableAck, PublishError> {
    let ack = if let Some(message_id) = message_id {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", message_id);
        state
            .jetstream
            .publish_with_headers(subject.to_string(), headers, body)
            .await
    } else {
        state.jetstream.publish(subject.to_string(), body).await
    }
    .map_err(|e| classify_js_error(&e.to_string()))?;
    let ack = ack.await.map_err(|e| classify_js_error(&e.to_string()))?;
    Ok(DurableAck {
        stream: ack.stream,
        sequence: ack.sequence,
        duplicate: ack.duplicate,
    })
}

/// async-nats surfaces "subject not bound to a stream" as either
/// "no responders" (JS API request layer) or "no stream found for given
/// subject" (publish ack), depending on the path.
fn classify_js_error(msg: &str) -> PublishError {
    if msg.contains("no responders") || msg.contains("no stream found") {
        PublishError::NoStream
    } else {
        PublishError::Other(msg.to_string())
    }
}

fn subject_matches_prefixes(subject: &str, prefixes: &[String]) -> bool {
    prefixes
        .iter()
        .any(|prefix| subject_in_prefix(subject, prefix))
}

fn namespace_prefix_contains(container: &str, candidate: &str) -> bool {
    let container = container.trim_end_matches('.');
    let candidate = candidate.trim_end_matches('.');
    if container.is_empty() || candidate.is_empty() {
        return false;
    }
    match candidate.strip_prefix(container) {
        Some(rest) => rest.is_empty() || rest.starts_with('.'),
        None => false,
    }
}

fn validate_durable_prefixes(allowed: &[String], durable: &[String]) -> Result<(), String> {
    for prefix in durable {
        if !allowed
            .iter()
            .any(|allowed_prefix| namespace_prefix_contains(allowed_prefix, prefix))
        {
            return Err(format!(
                "durable prefix '{prefix}' is outside BRIDGE_SUBJECT_PREFIXES"
            ));
        }
    }
    Ok(())
}

fn request_message_id(headers: &HeaderMap) -> Result<Option<String>, String> {
    let value = ["x-message-id", "idempotency-key", "nats-msg-id"]
        .into_iter()
        .find_map(|name| headers.get(name));
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| "message id header must be valid ASCII".to_string())?
        .trim();
    if value.is_empty() || value.len() > 128 {
        return Err("message id must be 1-128 characters".into());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
    {
        return Err(
            "message id may contain only ASCII alphanumerics, '-', '_', '.', ':', or '/'".into(),
        );
    }
    Ok(Some(value.to_string()))
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
    if !prefixes.iter().any(|p| subject_in_prefix(subject, p)) {
        return Err("subject is outside the bridge allowlist".into());
    }
    Ok(())
}

/// True if `subject` is within the namespace named by `prefix`, anchored to a
/// subject-token boundary. Prefix `vxl` matches `vxl` and `vxl.events` but NOT
/// the sibling `vxlmalicious.foo`; a prefix that already ends in `.` is matched
/// verbatim. This makes the allowlist safe regardless of whether an operator
/// remembered the trailing dot in `BRIDGE_SUBJECT_PREFIXES` — a plain
/// `starts_with` would otherwise widen `vxl` to every `vxl*` sibling subject.
fn subject_in_prefix(subject: &str, prefix: &str) -> bool {
    if prefix.ends_with('.') {
        return subject.starts_with(prefix);
    }
    match subject.strip_prefix(prefix) {
        Some(rest) => rest.is_empty() || rest.starts_with('.'),
        None => false,
    }
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
    fn classifies_no_stream_errors_for_core_fallback() {
        assert!(matches!(
            classify_js_error("no stream found for given subject"),
            PublishError::NoStream
        ));
        assert!(matches!(
            classify_js_error("503 no responders available"),
            PublishError::NoStream
        ));
        assert!(matches!(
            classify_js_error("timed out"),
            PublishError::Other(_)
        ));
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

        assert!(!caller_authorized(
            &HeaderMap::new(),
            Some("tok-abcdef123456")
        ));
    }

    // ---------------------------------------------------------------------
    // Added security-surface tests (2026-07-25).
    //
    // These pin the *current on-disk* behavior of the hardened bridge's
    // authorization surface: the subject allowlist (`validate_subject`), the
    // bearer/`x-bridge-token` auth (`caller_authorized` / `constant_time_eq`),
    // and the allowlist-config parser (`parse_prefixes`). They assert genuine
    // invariants and document actual behavior on adversarial inputs. Where a
    // test documents a latent sharp edge rather than a bug, it says so inline.
    // ---------------------------------------------------------------------

    /// The token the hardened deployment ships (≥16 chars, per startup check).
    const TOK: &str = "tok-abcdef123456";

    /// Build a `HeaderMap` from `(name, value)` pairs (test helper).
    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for &(k, v) in pairs {
            h.insert(k, v.parse().unwrap());
        }
        h
    }

    // ---- subject authorization: JetStream / system / settlement -----------

    /// Finding #1 (audit): the pre-hardening relay could POST to `$JS.API.>`
    /// and delete/purge streams. Every JetStream-API control subject must be
    /// denied (all are `$`-prefixed, so the dedicated leading-`$` guard fires).
    #[test]
    fn denies_all_jetstream_api_subjects() {
        for s in [
            "$JS.API.>",
            "$JS.API.STREAM.DELETE.DD_VAPI_TASKS",
            "$JS.API.STREAM.DELETE.*",
            "$JS.API.STREAM.PURGE.DD_VAPI_TASKS",
            "$JS.API.STREAM.CREATE.EVIL",
            "$JS.API.CONSUMER.DELETE.DD_VAPI_TASKS.dd-vapi-phone-worker",
            "$JS.API.CONSUMER.CREATE.DD_VAPI_TASKS",
            "$JS.ACK.DD_VAPI_TASKS.>",
            "$JSC.>",
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "JetStream control subject must be denied: {s}"
            );
        }
    }

    /// System subjects (`$SYS.>`) leak fleet/account telemetry and server
    /// control; all must be denied.
    #[test]
    fn denies_all_system_subjects() {
        for s in [
            "$SYS.>",
            "$SYS.REQ.SERVER.PING",
            "$SYS.REQ.ACCOUNT.PING",
            "$SYS.ACCOUNT.DD.CONNS",
            "$SYS.SERVER.>",
            "$",
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "system subject must be denied: {s}"
            );
        }
    }

    /// The messaging readme flags `dd.remote.contracts.solana.{settle,resolve}`
    /// as on-chain broadcast triggers, and `dd.remote.thread.*.tasks` as the
    /// separate remote work-queue. None share the bridge's allowlist prefixes
    /// (`dd.vapi.tasks.`, `vxl.`), so all must be denied — the bridge must not
    /// be a path to trigger settlement or enqueue remote-thread work. Also
    /// confirms the allowlist is `dd.vapi.tasks.` specifically, not `dd.vapi.`.
    #[test]
    fn denies_settlement_and_remote_thread_subjects() {
        for s in [
            "dd.remote.contracts.solana.settle",
            "dd.remote.contracts.solana.resolve",
            "dd.remote.contracts.solana.settle.mainnet",
            "dd.remote.thread.abc123.tasks",
            "dd.remote.thread.abc.results",
            "dd.vapi.status",
            "dd.vapi.results",
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "off-allowlist / settlement subject must be denied: {s}"
            );
        }
    }

    // ---- subject authorization: wildcards, `$`, dots, encoding ------------

    /// Wildcards are denied both as standalone tokens (`*`/`>`, the NATS
    /// wildcard guard) and when embedded in a token (caught by the char
    /// allowlist). Either way a caller cannot fan-out a publish.
    #[test]
    fn denies_wildcards_standalone_and_embedded() {
        for s in [
            // standalone wildcard tokens
            "dd.vapi.tasks.>",
            "dd.vapi.tasks.*",
            "dd.vapi.tasks.a.>",
            "dd.vapi.tasks.*.b",
            "vxl.>",
            ">",
            "*",
            // embedded wildcards -> rejected by the character allowlist
            "dd.vapi.tasks.a>b",
            "dd.vapi.tasks.a*b",
            "dd.vapi.tasks.pre*",
            "dd.vapi.tasks.>suffix",
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "wildcard subject must be denied: {s}"
            );
        }
    }

    /// A leading `$` is denied by the dedicated system-subject guard; a `$`
    /// anywhere else is denied by the character allowlist. So `$` can never be
    /// smuggled into a subject regardless of position.
    #[test]
    fn denies_dollar_injection_leading_and_embedded() {
        for s in [
            "$JS.API.STREAM.INFO", // leading -> system-subject guard
            "$",                   // leading
            "dd.vapi.tasks.$JS",   // embedded -> char guard
            "dd.vapi.tasks.a$b",   // embedded -> char guard
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "`$` injection must be denied: {s}"
            );
        }
    }

    /// Leading dot, trailing dot, and double dots all produce an empty token
    /// and are denied. Critically, the bare allowlist prefix itself
    /// (`dd.vapi.tasks.` / `vxl.`) has a trailing empty token and is denied —
    /// a caller cannot publish to the namespace root.
    #[test]
    fn denies_empty_leading_trailing_double_dots_and_bare_prefix() {
        for s in [
            ".dd.vapi.tasks.call", // leading dot
            "dd.vapi.tasks.call.", // trailing dot
            "dd.vapi.tasks..call", // double dot
            "dd.vapi.tasks.",      // bare prefix (trailing empty token)
            "vxl.",                // bare prefix
            "vxl..events",
            ".",
            "..",
            "dd.vapi.tasks", // prefix minus its trailing dot: shorter than the
                             // configured prefix, so it is outside the allowlist
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "empty-token / bare-prefix subject must be denied: {s}"
            );
        }
    }

    /// Unicode homoglyphs, control bytes, zero-width chars, whitespace, and
    /// percent-encoded payloads are all denied by the ASCII char allowlist.
    /// The last two rows show that whether or not the HTTP layer percent-decodes
    /// the path, both the encoded and decoded forms of a `$JS` attack are denied.
    #[test]
    fn denies_unicode_control_and_encoding_tricks() {
        for s in [
            "dd.vapi.tasks.c\u{0430}ll", // Cyrillic 'а' homoglyph
            "dd.vapi.tasks.caf\u{00E9}", // 'é'
            "dd.vapi.tasks.a\u{0000}b",  // null byte
            "dd.vapi.tasks.a\nb",        // newline
            "dd.vapi.tasks.a\tb",        // tab
            "dd.vapi.tasks.a b",         // space
            "dd.vapi.tasks.\u{200B}x",   // zero-width space
            "dd.vapi.tasks.\u{1F4A9}",   // emoji
            "%24JS.API.%3E",             // percent-encoded `$JS.API.>` (undecoded)
            "$JS.API.>",                 // ...and its decoded form
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "encoding/unicode trick must be denied: {s}"
            );
        }
    }

    // ---- subject authorization: allowlist semantics ----------------------

    /// The allowlist match is case-sensitive and anchored to the START of the
    /// subject. Uppercased or mixed-case variants of the prefix, and subjects
    /// that merely *contain* an allowed prefix, all fail closed.
    #[test]
    fn allowlist_is_case_sensitive_and_prefix_anchored() {
        for s in [
            // case-sensitivity (fails closed: never widens)
            "DD.VAPI.TASKS.call",
            "VXL.events",
            "Vxl.events",
            "Dd.vapi.tasks.call",
            "$js.api.foo", // lowercased `$js` still hits the leading-`$` guard
            // must START with a prefix, not contain / be suffixed by one
            "evil.vxl.events",
            "prefix.dd.vapi.tasks.call",
            "xvxl.events",
            "myvxl.events",
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_err(),
                "case/anchor variant must be denied: {s}"
            );
        }
    }

    /// The allowlist match is anchored to a subject-token boundary
    /// (`subject_in_prefix`), so it is safe whether or not a configured prefix
    /// ends in `.`. A prefix without the trailing dot (`vxl`) still matches only
    /// `vxl` and `vxl.*`, never the sibling `vxlmalicious.*`. (Previously the
    /// match was a raw `str::starts_with`, which widened `vxl` to every textual
    /// sibling — a latent, config-dependent allowlist bypass; now closed.)
    #[test]
    fn allowlist_prefix_boundary_is_token_anchored() {
        // Prefixes ending in '.' behave exactly as before (shipped config).
        let safe = parse_prefixes("dd.vapi.tasks.,vxl.");
        assert!(validate_subject("vxl.events", &safe).is_ok());
        assert!(validate_subject("vxlmalicious.foo", &safe).is_err());
        assert!(validate_subject("dd.vapi.tasksX.y", &safe).is_err());

        // Prefixes WITHOUT a trailing '.' are now anchored too: the exact subject
        // and its children pass; textual siblings are denied.
        let no_dot = parse_prefixes("vxl,dd.vapi.tasks");
        assert!(validate_subject("vxl", &no_dot).is_ok());
        assert!(validate_subject("vxl.events", &no_dot).is_ok());
        assert!(validate_subject("dd.vapi.tasks.call", &no_dot).is_ok());
        assert!(
            validate_subject("vxlmalicious.foo", &no_dot).is_err(),
            "prefix 'vxl' must NOT widen to sibling 'vxlmalicious.*'"
        );
        assert!(
            validate_subject("dd.vapi.tasksX.y", &no_dot).is_err(),
            "prefix 'dd.vapi.tasks' must NOT widen to 'dd.vapi.tasksX.*'"
        );
    }

    /// Direct unit coverage of the boundary matcher.
    #[test]
    fn subject_in_prefix_anchors_at_token_boundary() {
        assert!(subject_in_prefix("vxl", "vxl"));
        assert!(subject_in_prefix("vxl.events", "vxl"));
        assert!(subject_in_prefix("vxl.events", "vxl."));
        assert!(!subject_in_prefix("vxlmalicious.foo", "vxl"));
        assert!(!subject_in_prefix("vxlmalicious.foo", "vxl."));
        assert!(!subject_in_prefix("vx", "vxl"));
    }

    /// Legitimate vapi/vxl traffic must still pass — the narrowing must not
    /// over-block. Note uppercase/digits/`_`/`-` are valid *within* tokens
    /// (only the leading prefix match is case-sensitive).
    #[test]
    fn allows_legitimate_vapi_and_vxl_subjects() {
        for s in [
            "dd.vapi.tasks.call",
            "dd.vapi.tasks.outbound-call",
            "dd.vapi.tasks.setup_refresh",
            "dd.vapi.tasks.a.b.c.d",
            "dd.vapi.tasks.CALL_123",
            "vxl.events.stt",
            "vxl.a",
            "vxl.events.v2",
            "vxl.EVENT-1_x",
        ] {
            assert!(
                validate_subject(s, &prefixes()).is_ok(),
                "legitimate subject must be allowed: {s}"
            );
        }
    }

    /// Length bounds: 1..=255 chars; empty and >255 are denied. The upper
    /// bound is checked before per-token work, so an oversized subject can't
    /// burn cycles. 255 is the inclusive max.
    #[test]
    fn subject_length_bounds_enforced() {
        let at_limit = format!("vxl.{}", "a".repeat(251)); // 4 + 251 = 255
        assert_eq!(at_limit.len(), 255);
        assert!(validate_subject(&at_limit, &prefixes()).is_ok());

        let over_limit = format!("vxl.{}", "a".repeat(252)); // 256
        assert_eq!(over_limit.len(), 256);
        assert!(validate_subject(&over_limit, &prefixes()).is_err());

        assert!(validate_subject("", &prefixes()).is_err());
        assert!(validate_subject(&"a".repeat(1000), &prefixes()).is_err());
    }

    // ---- auth: caller_authorized / constant_time_eq ----------------------

    /// `BRIDGE_ALLOW_INSECURE` maps to `expected = None`, which bypasses ALL
    /// auth (returns true for any/no credentials). This is all-or-nothing —
    /// there is no partial auth — so it must never be set in production. Pinned
    /// so the bypass is explicit and visible to reviewers.
    #[test]
    fn insecure_mode_bypasses_all_auth() {
        assert!(caller_authorized(&HeaderMap::new(), None));
        assert!(caller_authorized(
            &headers(&[("authorization", "Bearer whatever")]),
            None
        ));
        assert!(caller_authorized(&headers(&[("x-bridge-token", "")]), None));
    }

    /// With a token configured, missing / empty / malformed / wrong
    /// credentials are all denied. Note a same-length wrong token
    /// (`wrong-token-0000`) is denied on content, and `Bearer ` with an empty
    /// token is denied.
    #[test]
    fn missing_empty_and_malformed_tokens_denied() {
        let cases: &[&[(&'static str, &str)]] = &[
            &[],
            &[("authorization", "")],
            &[("authorization", "Bearer ")],
            &[("authorization", "Bearer wrong-token-0000")],
            &[("authorization", "Bearer")],
            &[("authorization", "Basic dXNlcjpwYXNz")],
            &[("x-bridge-token", "")],
            &[("x-bridge-token", "wrong-token-0000")],
        ];
        for c in cases {
            assert!(
                !caller_authorized(&headers(c), Some(TOK)),
                "credentials must be denied: {c:?}"
            );
        }
    }

    /// The correct token is accepted via either channel, and surrounding
    /// whitespace on the presented token is trimmed before comparison.
    #[test]
    fn correct_token_accepted_and_whitespace_trimmed() {
        assert!(caller_authorized(
            &headers(&[("authorization", "Bearer tok-abcdef123456")]),
            Some(TOK)
        ));
        assert!(caller_authorized(
            &headers(&[("x-bridge-token", "tok-abcdef123456")]),
            Some(TOK)
        ));
        // trimmed
        assert!(caller_authorized(
            &headers(&[("authorization", "Bearer tok-abcdef123456 ")]),
            Some(TOK)
        ));
        assert!(caller_authorized(
            &headers(&[("x-bridge-token", "  tok-abcdef123456  ")]),
            Some(TOK)
        ));
        assert!(caller_authorized(
            &headers(&[("authorization", "Bearer  tok-abcdef123456")]),
            Some(TOK)
        ));
    }

    /// The `Bearer ` scheme is matched exactly (case- and format-sensitive):
    /// lowercase/uppercase scheme, or a missing space, all fall through and are
    /// denied. This fails closed (never widens), so it is a strictness note,
    /// not a bypass.
    #[test]
    fn bearer_scheme_is_case_and_format_sensitive() {
        for c in [
            "bearer tok-abcdef123456",
            "BEARER tok-abcdef123456",
            "Bearertok-abcdef123456",
            "Token tok-abcdef123456",
        ] {
            assert!(
                !caller_authorized(&headers(&[("authorization", c)]), Some(TOK)),
                "non-exact scheme must be denied: {c}"
            );
        }
    }

    /// Header precedence: a `Bearer `-prefixed value is always consumed (even
    /// when wrong), so a wrong bearer SHADOWS a correct `x-bridge-token` and
    /// the request is denied (fails closed). The `x-bridge-token` fallback is
    /// only consulted when the authorization header is absent or not
    /// `Bearer `-prefixed.
    #[test]
    fn wrong_bearer_shadows_xbridge_but_nonbearer_falls_back() {
        // wrong bearer shadows correct x-bridge-token -> denied
        assert!(!caller_authorized(
            &headers(&[
                ("authorization", "Bearer wrong-token-0000"),
                ("x-bridge-token", "tok-abcdef123456"),
            ]),
            Some(TOK)
        ));
        // non-Bearer authorization -> fallback to correct x-bridge-token -> ok
        assert!(caller_authorized(
            &headers(&[
                ("authorization", "Basic zzz"),
                ("x-bridge-token", "tok-abcdef123456"),
            ]),
            Some(TOK)
        ));
        // absent authorization -> fallback -> ok
        assert!(caller_authorized(
            &headers(&[("x-bridge-token", "tok-abcdef123456")]),
            Some(TOK)
        ));
        // correct bearer wins; wrong x-bridge-token is ignored -> ok
        assert!(caller_authorized(
            &headers(&[
                ("authorization", "Bearer tok-abcdef123456"),
                ("x-bridge-token", "nope"),
            ]),
            Some(TOK)
        ));
    }

    /// `constant_time_eq` rejects on both length and content mismatch, is
    /// case-sensitive, and treats prefix relationships (`tok` vs `token`) as
    /// unequal. Equal-length equal-content is the only accepting case.
    #[test]
    fn constant_time_eq_rejects_length_and_content_mismatch() {
        assert!(constant_time_eq("tok-abcdef123456", "tok-abcdef123456"));
        assert!(!constant_time_eq("tok-abcdef123456", "tok-abcdef123457")); // content
        assert!(!constant_time_eq("tok", "tok-abcdef123456")); // shorter presented
        assert!(!constant_time_eq(
            "tok-abcdef123456xxxx",
            "tok-abcdef123456"
        )); // longer
        assert!(!constant_time_eq("tok-abcdef12345", "tok-abcdef123456")); // off-by-one len
        assert!(constant_time_eq("", "")); // degenerate equal
        assert!(!constant_time_eq("", "x"));
        assert!(!constant_time_eq("ABCDEF", "abcdef")); // case-sensitive
    }

    // ---- allowlist config parsing (security-critical) --------------------

    /// `parse_prefixes` filters empty tokens, so it can NEVER emit an
    /// empty-string prefix. This is load-bearing: an empty prefix would make
    /// `starts_with("")` true for every subject — i.e. permit-all (see
    /// `empty_string_prefix_would_be_permit_all`). Doubled commas, trailing
    /// commas, and whitespace-only tokens are all dropped.
    #[test]
    fn parse_prefixes_cannot_produce_permit_all_empty_prefix() {
        assert!(parse_prefixes("").is_empty());
        assert!(parse_prefixes("   ").is_empty());
        assert!(parse_prefixes(", ,\t,").is_empty());

        let p = parse_prefixes("dd.vapi.tasks.,,vxl.");
        assert_eq!(p, vec!["dd.vapi.tasks.".to_string(), "vxl.".to_string()]);
        assert!(p.iter().all(|s| !s.is_empty()));

        assert_eq!(
            parse_prefixes(" vxl. , dd.vapi.tasks. "),
            vec!["vxl.".to_string(), "dd.vapi.tasks.".to_string()]
        );
        assert_eq!(parse_prefixes("vxl.,"), vec!["vxl.".to_string()]);

        // No parse of any garbage config can yield the permit-all empty prefix.
        for raw in ["", "   ", ",,,", ", ,\t,", "vxl.,,", ",dd.vapi.tasks."] {
            assert!(
                !parse_prefixes(raw).iter().any(|s| s.is_empty()),
                "parse_prefixes must never emit an empty prefix: {raw:?}"
            );
        }
    }

    /// An empty allowlist fails closed: `validate_subject` denies everything,
    /// including otherwise-legal subjects. (`main()` also refuses to start with
    /// an empty allowlist, but the function itself must not fall open.)
    #[test]
    fn empty_allowlist_fails_closed() {
        assert!(validate_subject("dd.vapi.tasks.call", &[]).is_err());
        assert!(validate_subject("vxl.events", &[]).is_err());
        assert!(validate_subject("dd.vapi.tasks.call", &parse_prefixes("")).is_err());
    }

    /// Even a manually-constructed empty-string prefix is no longer permit-all:
    /// the token-anchored matcher only treats a subject as inside `""` if the
    /// remainder after stripping the (empty) prefix is empty or starts with `.`,
    /// which a normal dotted subject never is. So an empty prefix denies real
    /// subjects rather than waving them through. (`parse_prefixes` still filters
    /// empties, and `main()` refuses an empty allowlist — this is the third,
    /// independent layer.)
    #[test]
    fn empty_string_prefix_is_not_permit_all() {
        let empties = vec![String::new()];
        // A settlement subject is denied even with an empty prefix present.
        assert!(validate_subject("dd.remote.contracts.solana.settle", &empties).is_err());
        assert!(validate_subject("dd.vapi.tasks.call", &empties).is_err());
        // The independent `$`/wildcard guards also still hold.
        assert!(validate_subject("$JS.API.STREAM.DELETE.DD_VAPI_TASKS", &empties).is_err());
        assert!(validate_subject("dd.vapi.tasks.>", &empties).is_err());
    }

    // ---- publish-path fallback routing (durability, not authz) ------------

    /// `classify_js_error` decides JetStream-vs-core fallback via a naive,
    /// case-sensitive substring match. Documented here: any error message
    /// containing "no responders"/"no stream found" routes to the core-NATS
    /// fallback. This is a durability concern only — the subject is already
    /// authorized before publish — but the substring behavior is a sharp edge
    /// worth pinning.
    #[test]
    fn classify_js_error_substring_and_case_sensitivity() {
        assert!(matches!(
            classify_js_error("no stream found for given subject foo"),
            PublishError::NoStream
        ));
        assert!(matches!(
            classify_js_error("nats: no responders"),
            PublishError::NoStream
        ));
        assert!(matches!(
            classify_js_error("fatal: no stream found; giving up"),
            PublishError::NoStream
        ));
        // case-sensitive: capitalized variants are NOT treated as no-stream
        assert!(matches!(
            classify_js_error("No Responders"),
            PublishError::Other(_)
        ));
        assert!(matches!(
            classify_js_error("permission denied"),
            PublishError::Other(_)
        ));
    }

    #[test]
    fn durable_prefixes_must_be_inside_publish_allowlist() {
        let allowed = parse_prefixes("dd.vapi.tasks.,vxl.");
        assert!(validate_durable_prefixes(&allowed, &parse_prefixes("dd.vapi.tasks.")).is_ok());
        assert!(validate_durable_prefixes(&allowed, &parse_prefixes("vxl.events.")).is_ok());
        assert!(validate_durable_prefixes(&allowed, &parse_prefixes("dd.remote.")).is_err());
        assert!(namespace_prefix_contains("dd.vapi", "dd.vapi.tasks."));
        assert!(!namespace_prefix_contains(
            "dd.vapi.tasks.",
            "dd.vapi.other."
        ));
    }

    #[test]
    fn message_id_headers_are_validated_and_precedence_is_stable() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", "task/123:attempt-1".parse().unwrap());
        assert_eq!(
            request_message_id(&headers).unwrap().as_deref(),
            Some("task/123:attempt-1")
        );

        headers.insert("x-message-id", "preferred-id".parse().unwrap());
        assert_eq!(
            request_message_id(&headers).unwrap().as_deref(),
            Some("preferred-id")
        );

        let mut invalid = HeaderMap::new();
        invalid.insert("x-message-id", "contains space".parse().unwrap());
        assert!(request_message_id(&invalid).is_err());
        invalid.insert("x-message-id", "".parse().unwrap());
        assert!(request_message_id(&invalid).is_err());
        invalid.insert("x-message-id", "a".repeat(129).parse().unwrap());
        assert!(request_message_id(&invalid).is_err());
    }

    #[test]
    fn durable_subject_matching_is_token_anchored() {
        let durable = parse_prefixes("dd.vapi.tasks.,vxl.orders");
        assert!(subject_matches_prefixes("dd.vapi.tasks.call", &durable));
        assert!(subject_matches_prefixes("vxl.orders.created", &durable));
        assert!(!subject_matches_prefixes(
            "dd.vapi.tasksEvil.call",
            &durable
        ));
        assert!(!subject_matches_prefixes("vxl.ordersEvil", &durable));
    }
}
