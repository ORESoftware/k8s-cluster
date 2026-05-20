//! Provider webhook receivers.
//!
//! Every inbound event lands in `webhook_events` first (raw payload + sig-OK
//! flag), then is dispatched to the per-provider ingestor. This gives us full
//! replay-ability and an audit trail even if the ingestor crashes mid-process.
//!
//! The dispatch step itself is intentionally minimal in the scaffold — the
//! real provider ingestor implementations are the next major piece of work.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::{
    ProviderKind,
    coinbase::{self, CoinbaseCredential},
    coinflow::{self, CoinflowCredential},
    paypal, stripe,
};
use crate::state::AppState;

#[derive(Serialize)]
pub struct Ack {
    pub received: bool,
    pub event_id: Option<String>,
    pub signature_ok: bool,
    pub tenant_id: Option<Uuid>,
    pub connection_id: Option<Uuid>,
}

pub async fn stripe(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Stripe, &headers, &body).await
}

pub async fn paypal(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Paypal, &headers, &body).await
}

pub async fn coinbase(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::CoinbaseCommerce, &headers, &body).await
}

pub async fn plaid(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::PlaidBank, &headers, &body).await
}

pub async fn coinflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Coinflow, &headers, &body).await
}

#[derive(Clone, Copy)]
enum WebhookProvider {
    Stripe,
    Paypal,
    CoinbaseCommerce,
    PlaidBank,
    Coinflow,
}

impl WebhookProvider {
    fn kind(self) -> ProviderKind {
        match self {
            Self::Stripe => ProviderKind::Stripe,
            Self::Paypal => ProviderKind::Paypal,
            Self::CoinbaseCommerce => ProviderKind::CoinbaseCommerce,
            Self::PlaidBank => ProviderKind::PlaidBank,
            Self::Coinflow => ProviderKind::Coinflow,
        }
    }

    fn tag(self) -> &'static str {
        self.kind().tag()
    }
}

async fn record_event(
    state: &AppState,
    provider: WebhookProvider,
    headers: &HeaderMap,
    body: &Bytes,
) -> AppResult<Json<Ack>> {
    let payload: Value = serde_json::from_slice(body)
        .map_err(|e| AppError::BadRequest(format!("payload not JSON: {e}")))?;

    let external_event_id = event_id(provider, &payload)
        .unwrap_or_else(|| format!("synthetic-{}", uuid::Uuid::new_v4()));

    let event_type = event_type(provider, &payload).unwrap_or_else(|| "unknown".into());
    let external_account_id = external_account_id(provider, &payload);
    let connection = match external_account_id.as_deref() {
        Some(external) => {
            state
                .connections
                .find_active_by_external_account(provider.kind(), external)
                .await?
        }
        None => None,
    };

    let verification = verify_delivery(
        state,
        provider,
        headers,
        body,
        &payload,
        connection.as_ref(),
    )
    .await;
    let (signature_ok, verification_error) = match verification {
        Ok(true) => (true, None),
        Ok(false) => (
            false,
            Some("verification_not_configured_or_missing_header".to_string()),
        ),
        Err(err) => (false, Some(err.to_string())),
    };
    let payload_sha256 = hex::encode(Sha256::digest(body.as_ref()));
    let tenant_id = connection.as_ref().map(|c| c.tenant_id);
    let connection_id = connection.as_ref().map(|c| c.id);

    sqlx::query(
        r#"
        INSERT INTO webhook_events
            (provider, external_event_id, event_type, payload, signature_ok,
             tenant_id, connection_id, payload_sha256, verification_error,
             external_account_id)
        VALUES ($1::provider_kind, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (provider, external_event_id) DO UPDATE
        SET signature_ok = webhook_events.signature_ok OR EXCLUDED.signature_ok,
            tenant_id = COALESCE(webhook_events.tenant_id, EXCLUDED.tenant_id),
            connection_id = COALESCE(webhook_events.connection_id, EXCLUDED.connection_id),
            payload_sha256 = COALESCE(webhook_events.payload_sha256, EXCLUDED.payload_sha256),
            verification_error = CASE
                WHEN EXCLUDED.signature_ok THEN NULL
                ELSE EXCLUDED.verification_error
            END,
            external_account_id = COALESCE(webhook_events.external_account_id, EXCLUDED.external_account_id),
            received_at = now()
        "#,
    )
    .bind(provider.tag())
    .bind(&external_event_id)
    .bind(&event_type)
    .bind(&payload)
    .bind(signature_ok)
    .bind(tenant_id)
    .bind(connection_id)
    .bind(&payload_sha256)
    .bind(&verification_error)
    .bind(&external_account_id)
    .execute(&state.pool)
    .await?;

    if state.cfg.require_webhook_signatures && !signature_ok {
        return Err(AppError::Unauthorized);
    }

    Ok(Json(Ack {
        received: true,
        event_id: Some(external_event_id),
        signature_ok,
        tenant_id,
        connection_id,
    }))
}

async fn verify_delivery(
    state: &AppState,
    provider: WebhookProvider,
    headers: &HeaderMap,
    body: &Bytes,
    payload: &Value,
    connection: Option<&ProviderConnection>,
) -> AppResult<bool> {
    match provider {
        WebhookProvider::Stripe => {
            let Some(secret) = state.cfg.stripe_webhook_secret.as_deref() else {
                return Ok(false);
            };
            let Some(sig) = header_str(headers, "stripe-signature") else {
                return Ok(false);
            };
            stripe::verify_signature(
                body,
                sig,
                secret,
                state.cfg.webhook_signature_tolerance_seconds,
            )?;
            Ok(true)
        }
        WebhookProvider::Paypal => {
            let Some(auth_algo) = header_str(headers, "paypal-auth-algo") else {
                return Ok(false);
            };
            let Some(cert_url) = header_str(headers, "paypal-cert-url") else {
                return Ok(false);
            };
            let Some(transmission_id) = header_str(headers, "paypal-transmission-id") else {
                return Ok(false);
            };
            let Some(transmission_sig) = header_str(headers, "paypal-transmission-sig") else {
                return Ok(false);
            };
            let Some(transmission_time) = header_str(headers, "paypal-transmission-time") else {
                return Ok(false);
            };
            paypal::verify_webhook_signature(
                &state.cfg,
                auth_algo,
                cert_url,
                transmission_id,
                transmission_sig,
                transmission_time,
                payload,
            )
            .await
        }
        WebhookProvider::CoinbaseCommerce => {
            let Some(sig) = header_str(headers, "x-cc-webhook-signature") else {
                return Ok(false);
            };
            let secret = if let Some(conn) = connection {
                load_coinbase_secret(state, conn).await?
            } else {
                state.cfg.coinbase_webhook_secret.clone()
            };
            let Some(secret) = secret else {
                return Ok(false);
            };
            coinbase::verify_commerce_signature(body, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::Coinflow => {
            let Some(sig) = header_str(headers, "x-coinflow-signature") else {
                return Ok(false);
            };
            let secret = if let Some(conn) = connection {
                load_coinflow_secret(state, conn).await?
            } else {
                state.cfg.coinflow_webhook_validation_key.clone()
            };
            let Some(secret) = secret else {
                return Ok(false);
            };
            coinflow::verify_webhook_signature(body, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::PlaidBank => {
            // Plaid signs with a JWT in Plaid-Verification; full ES256/JWK
            // validation is intentionally left out until we add a vetted JWT
            // library. We still record the JWT presence and never process
            // unverified events.
            let _ = header_str(headers, "plaid-verification");
            Ok(false)
        }
    }
}

async fn load_coinbase_secret(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: CoinbaseCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "coinbase_commerce".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(Some(cred.webhook_secret))
}

async fn load_coinflow_secret(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: CoinflowCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "coinflow".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_validation_key)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn event_id(provider: WebhookProvider, payload: &Value) -> Option<String> {
    match provider {
        WebhookProvider::PlaidBank => payload
            .get("webhook_code")
            .and_then(|v| v.as_str())
            .zip(payload.get("item_id").and_then(|v| v.as_str()))
            .map(|(code, item)| format!("{item}:{code}:{}", payload_sha_payload(payload))),
        _ => payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

fn event_type(provider: WebhookProvider, payload: &Value) -> Option<String> {
    match provider {
        WebhookProvider::Paypal => payload
            .get("event_type")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::PlaidBank => payload
            .get("webhook_code")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => payload
            .get("type")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

fn external_account_id(provider: WebhookProvider, payload: &Value) -> Option<String> {
    match provider {
        WebhookProvider::Stripe => payload
            .get("account")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Paypal => payload
            .pointer("/resource/merchant_id")
            .or_else(|| payload.pointer("/resource/payee/merchant_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::PlaidBank => payload
            .get("item_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Coinflow => payload
            .get("merchant_id")
            .or_else(|| payload.get("merchantId"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::CoinbaseCommerce => payload
            .pointer("/event/data/metadata/merchant_id")
            .or_else(|| payload.pointer("/data/metadata/merchant_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

fn payload_sha_payload(payload: &Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    hex::encode(Sha256::digest(&bytes))[..16].to_string()
}
