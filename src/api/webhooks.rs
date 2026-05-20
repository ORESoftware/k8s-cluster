//! Provider webhook receivers.
//!
//! Every inbound event lands in `webhook_events` first (raw payload + sig-OK
//! flag), then is dispatched to the per-provider ingestor. This gives us full
//! replay-ability and an audit trail even if the ingestor crashes mid-process.
//!
//! The dispatch step itself is intentionally minimal in the scaffold — the
//! real provider ingestor implementations are the next major piece of work.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct Ack {
    pub received: bool,
    pub event_id: Option<String>,
}

pub async fn stripe(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, "stripe", &headers, &body, "stripe-signature").await
}

pub async fn paypal(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, "paypal", &headers, &body, "paypal-transmission-sig").await
}

pub async fn coinbase(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, "coinbase_commerce", &headers, &body, "x-cc-webhook-signature").await
}

pub async fn plaid(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, "plaid_bank", &headers, &body, "plaid-verification").await
}

pub async fn coinflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, "coinflow", &headers, &body, "x-coinflow-signature").await
}

async fn record_event(
    state: &AppState,
    provider_tag: &str,
    headers: &HeaderMap,
    body: &Bytes,
    sig_header: &str,
) -> AppResult<Json<Ack>> {
    let payload: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("payload not JSON: {e}")))?;

    let external_event_id = payload
        .get("id").and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("synthetic-{}", uuid::Uuid::new_v4()));

    let event_type = payload
        .get("type").and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // TODO(real impl): actually verify the signature with the provider's
    // shared secret stored alongside the connection. Until then we mark
    // signature_ok = false and the ingestor must not act on unverified events.
    let _ = headers.get(sig_header);
    let signature_ok = false;

    sqlx::query(
        r#"
        INSERT INTO webhook_events
            (provider, external_event_id, event_type, payload, signature_ok)
        VALUES ($1::provider_kind, $2, $3, $4, $5)
        ON CONFLICT (provider, external_event_id) DO NOTHING
        "#,
    )
    .bind(provider_tag)
    .bind(&external_event_id)
    .bind(&event_type)
    .bind(&payload)
    .bind(signature_ok)
    .execute(&state.pool)
    .await?;

    Ok(Json(Ack { received: true, event_id: Some(external_event_id) }))
}
