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

use crate::error::{AppError, AppResult};
use crate::providers::connection::ProviderConnection;
use crate::providers::{
    ProviderKind,
    adyen::{self, AdyenCredential},
    bridge,
    circle::{self, CircleCredential},
    coinbase::{self, CoinbaseCredential},
    coinflow::{self, CoinflowCredential},
    dwolla::{self, DwollaCredential},
    fireblocks::{self, FireblocksCredential},
    gocardless::{self, GoCardlessCredential},
    mercury::{self, MercuryCredential},
    modern_treasury::{self, ModernTreasuryCredential},
    paypal,
    revolut::{self, RevolutCredential},
    square::{self, SquareCredential},
    stripe,
};
use crate::state::AppState;

/// Public webhook acknowledgement. We deliberately do NOT include the
/// resolved `tenant_id`, `connection_id`, or even the synthesized
/// event-id in the response: returning those let a probing attacker
/// enumerate valid identifiers by sending crafted bodies.
///
/// Internally the full mapping is logged via tracing for ops, and
/// stored in `webhook_events` for the dispatcher. Providers only need
/// "did you receive this?" — `{"received": true}`.
#[derive(Serialize)]
pub struct Ack {
    pub received: bool,
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

pub async fn revolut(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Revolut, &headers, &body).await
}

pub async fn gocardless(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::GoCardless, &headers, &body).await
}

pub async fn mercury(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Mercury, &headers, &body).await
}

pub async fn bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Bridge, &headers, &body).await
}

pub async fn fireblocks(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Fireblocks, &headers, &body).await
}

pub async fn circle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Circle, &headers, &body).await
}

pub async fn adyen(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Adyen, &headers, &body).await
}

pub async fn square(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Square, &headers, &body).await
}

pub async fn modern_treasury(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::ModernTreasury, &headers, &body).await
}

pub async fn dwolla(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Ack>> {
    record_event(&state, WebhookProvider::Dwolla, &headers, &body).await
}

#[derive(Clone, Copy)]
enum WebhookProvider {
    Stripe,
    Paypal,
    CoinbaseCommerce,
    PlaidBank,
    Coinflow,
    Revolut,
    GoCardless,
    Mercury,
    Bridge,
    Fireblocks,
    Circle,
    Adyen,
    Square,
    ModernTreasury,
    Dwolla,
}

impl WebhookProvider {
    fn kind(self) -> ProviderKind {
        match self {
            Self::Stripe => ProviderKind::Stripe,
            Self::Paypal => ProviderKind::Paypal,
            Self::CoinbaseCommerce => ProviderKind::CoinbaseCommerce,
            Self::PlaidBank => ProviderKind::PlaidBank,
            Self::Coinflow => ProviderKind::Coinflow,
            Self::Revolut => ProviderKind::Revolut,
            Self::GoCardless => ProviderKind::GoCardless,
            Self::Mercury => ProviderKind::Mercury,
            Self::Bridge => ProviderKind::Bridge,
            Self::Fireblocks => ProviderKind::Fireblocks,
            Self::Circle => ProviderKind::Circle,
            Self::Adyen => ProviderKind::Adyen,
            Self::Square => ProviderKind::Square,
            Self::ModernTreasury => ProviderKind::ModernTreasury,
            Self::Dwolla => ProviderKind::Dwolla,
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

    // Encrypt the raw body at rest with the master sealer (AES-256-GCM). The
    // AAD binds the blob to its provider; we deliberately use the nil tenant
    // (not the resolved `tenant_id`) so the sealing is STABLE across
    // re-deliveries — a webhook can arrive unbound and later bind to a tenant,
    // and `ON CONFLICT` overwrites `payload_sealed`, so a tenant-dependent AAD
    // would make the row undecryptable. Tenant routing lives in the
    // `tenant_id` column; the at-rest crypto must not depend on it. We store
    // the sealed envelope in `payload_sealed` and never persist the plaintext
    // `payload` column. The integrity hash (`payload_sha256`) is kept in the
    // clear for dedup / correlation.
    let sealed = state
        .sealer
        .seal(uuid::Uuid::nil(), provider.tag(), body.as_ref())?;
    let payload_sealed =
        serde_json::to_value(&sealed).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

    sqlx::query(
        r#"
        INSERT INTO webhook_events
            (provider, external_event_id, event_type, payload_sealed, signature_ok,
             tenant_id, connection_id, payload_sha256, verification_error,
             external_account_id)
        VALUES ($1::provider_kind, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (provider, external_event_id) DO UPDATE
        SET signature_ok = webhook_events.signature_ok OR EXCLUDED.signature_ok,
            payload_sealed = EXCLUDED.payload_sealed,
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
    .bind(&payload_sealed)
    .bind(signature_ok)
    .bind(tenant_id)
    .bind(connection_id)
    .bind(&payload_sha256)
    .bind(&verification_error)
    .bind(&external_account_id)
    .execute(&state.pool)
    .await?;

    if state.cfg.require_webhook_signatures {
        if !signature_ok {
            return Err(AppError::Unauthorized);
        }
        // In strict mode we additionally insist on a tenant-bound
        // connection. Otherwise a globally-signed provider payload
        // (Stripe Connect, Plaid, …) could be accepted with
        // `tenant_id = NULL` and processed by a downstream system
        // that has no way to map it back to its owner.
        if connection_id.is_none() {
            tracing::warn!(
                provider = provider.tag(),
                event_id = %external_event_id,
                external_account_id = ?external_account_id,
                "strict mode: refused webhook with no tenant-bound connection"
            );
            return Err(AppError::Unauthorized);
        }
    }

    tracing::debug!(
        provider = provider.tag(),
        event_id = %external_event_id,
        signature_ok,
        tenant_id = ?tenant_id,
        connection_id = ?connection_id,
        "webhook accepted"
    );

    // Best-effort redacted receipt onto the event bus. Only the payload hash
    // travels — never the body or the verification-error detail.
    state
        .events
        .publish_webhook_receipt(
            provider.tag(),
            &external_event_id,
            &event_type,
            signature_ok,
            tenant_id,
            connection_id,
            &payload_sha256,
        )
        .await;

    Ok(Json(Ack { received: true }))
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
            let Some(jwt) = header_str(headers, "plaid-verification") else {
                return Ok(false);
            };
            state
                .plaid_webhook_verifier
                .verify(&state.cfg, jwt, body, 300)
                .await?;
            Ok(true)
        }
        WebhookProvider::Revolut => {
            let Some(sig) = header_str(headers, "revolut-signature") else {
                return Ok(false);
            };
            let Some(ts) = header_str(headers, "revolut-request-timestamp") else {
                return Ok(false);
            };
            let secret = if let Some(conn) = connection {
                load_revolut_secret(state, conn).await?
            } else {
                state.cfg.revolut_webhook_secret.clone()
            };
            let Some(secret) = secret else {
                return Ok(false);
            };
            revolut::verify_webhook_signature(body, ts, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::GoCardless => {
            let Some(sig) = header_str(headers, "webhook-signature") else {
                return Ok(false);
            };
            let secret = if let Some(conn) = connection {
                load_gocardless_secret(state, conn).await?
            } else {
                state.cfg.gocardless_webhook_secret.clone()
            };
            let Some(secret) = secret else {
                return Ok(false);
            };
            gocardless::verify_webhook_signature(body, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::Mercury => {
            let Some(sig) = header_str(headers, "x-mercury-signature") else {
                return Ok(false);
            };
            let Some(ts) = header_str(headers, "x-mercury-timestamp") else {
                return Ok(false);
            };
            let secret = if let Some(conn) = connection {
                load_mercury_secret(state, conn).await?
            } else {
                state.cfg.mercury_webhook_secret.clone()
            };
            let Some(secret) = secret else {
                return Ok(false);
            };
            mercury::verify_webhook_signature(body, ts, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::Bridge => {
            let Some(sig_header) = header_str(headers, "x-webhook-signature") else {
                return Ok(false);
            };
            let parsed = match bridge::parse_signature_header(sig_header) {
                Ok(p) => p,
                Err(_) => return Ok(false),
            };
            bridge::validate_timestamp_freshness(&parsed, 600, chrono::Utc::now())?;
            let Some(conn) = connection else {
                return Ok(false);
            };
            let Some(pem) = load_bridge_public_key(state, conn).await? else {
                return Ok(false);
            };
            bridge::verify_signature_rsa(body, &parsed, &pem)?;
            Ok(true)
        }
        WebhookProvider::Fireblocks => {
            let Some(sig) = header_str(headers, "fireblocks-signature") else {
                return Ok(false);
            };
            let Some(conn) = connection else {
                return Ok(false);
            };
            let Some(pem) = load_fireblocks_public_key(state, conn).await? else {
                return Ok(false);
            };
            fireblocks::verify_webhook_signature(body, sig, &pem)?;
            Ok(true)
        }
        WebhookProvider::Circle => {
            let Some(sig) = header_str(headers, "circle-signature") else {
                return Ok(false);
            };
            let Some(secret) = load_circle_secret(state, connection).await? else {
                return Ok(false);
            };
            circle::verify_webhook_signature(body, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::Adyen => {
            // Adyen carries the signature inside each notification item
            // (notificationItems[].NotificationRequestItem.additionalData
            // .hmacSignature), signing a `:`-joined field string with the
            // merchant HMAC key. Adyen batches multiple items per delivery, so
            // we must verify EVERY item — one validly-signed item must not
            // vouch for unsigned/forged siblings stored in the same row.
            let Some(key_hex) = load_adyen_hmac_key(state, connection).await? else {
                return Ok(false);
            };
            let Some(items) = payload.get("notificationItems").and_then(|v| v.as_array()) else {
                return Ok(false);
            };
            if items.is_empty() {
                return Ok(false);
            }
            for entry in items {
                let Some(item_val) = entry.pointer("/NotificationRequestItem") else {
                    return Ok(false);
                };
                let Some(sig) = item_val
                    .pointer("/additionalData/hmacSignature")
                    .and_then(|v| v.as_str())
                else {
                    return Ok(false);
                };
                let item: adyen::AdyenNotificationItem = serde_json::from_value(item_val.clone())
                    .map_err(|e| AppError::BadRequest(format!("adyen notification item: {e}")))?;
                // Err (wrong signature) propagates and rejects the whole batch.
                adyen::verify_item_signature(&item, sig, &key_hex)?;
            }
            Ok(true)
        }
        WebhookProvider::Square => {
            let Some(sig) = header_str(headers, "x-square-hmacsha256-signature") else {
                return Ok(false);
            };
            let Some(conn) = connection else {
                return Ok(false);
            };
            let Some((url, key)) = load_square_webhook_config(state, conn).await? else {
                return Ok(false);
            };
            square::verify_webhook_signature(&url, body, sig, &key)?;
            Ok(true)
        }
        WebhookProvider::ModernTreasury => {
            let Some(sig) = header_str(headers, "x-signature") else {
                return Ok(false);
            };
            let Some(secret) = load_modern_treasury_secret(state, connection).await? else {
                return Ok(false);
            };
            modern_treasury::verify_webhook_signature(body, sig, &secret)?;
            Ok(true)
        }
        WebhookProvider::Dwolla => {
            let Some(sig) = header_str(headers, "x-request-signature-sha-256") else {
                return Ok(false);
            };
            let Some(secret) = load_dwolla_secret(state, connection).await? else {
                return Ok(false);
            };
            dwolla::verify_webhook_signature(body, sig, &secret)?;
            Ok(true)
        }
    }
}

async fn load_adyen_hmac_key(
    state: &AppState,
    conn: Option<&ProviderConnection>,
) -> AppResult<Option<String>> {
    let Some(conn) = conn else {
        return Ok(None);
    };
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: AdyenCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "adyen".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.hmac_key_hex)
}

async fn load_square_webhook_config(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<(String, String)>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: SquareCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "square".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    match (cred.webhook_notification_url, cred.webhook_signature_key) {
        (Some(url), Some(key)) => Ok(Some((url, key))),
        _ => Ok(None),
    }
}

async fn load_modern_treasury_secret(
    state: &AppState,
    conn: Option<&ProviderConnection>,
) -> AppResult<Option<String>> {
    let Some(conn) = conn else {
        return Ok(None);
    };
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: ModernTreasuryCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "modern_treasury".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_secret)
}

async fn load_dwolla_secret(
    state: &AppState,
    conn: Option<&ProviderConnection>,
) -> AppResult<Option<String>> {
    let Some(conn) = conn else {
        return Ok(None);
    };
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: DwollaCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "dwolla".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_secret)
}

async fn load_fireblocks_public_key(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: FireblocksCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "fireblocks".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_public_key_pem)
}

async fn load_circle_secret(
    state: &AppState,
    conn: Option<&ProviderConnection>,
) -> AppResult<Option<String>> {
    let Some(conn) = conn else {
        return Ok(None);
    };
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: CircleCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "circle".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_secret)
}

async fn load_mercury_secret(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: MercuryCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "mercury".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_secret)
}

async fn load_revolut_secret(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: RevolutCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "revolut".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_secret)
}

async fn load_gocardless_secret(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: GoCardlessCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "gocardless".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_secret)
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

async fn load_bridge_public_key(
    state: &AppState,
    conn: &ProviderConnection,
) -> AppResult<Option<String>> {
    let plaintext = state
        .connections
        .load_credential(conn.tenant_id, conn.id)
        .await?;
    let cred: crate::providers::bridge::BridgeCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "bridge".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    Ok(cred.webhook_public_key_pem)
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
        WebhookProvider::Fireblocks => payload
            .pointer("/data/id")
            .or_else(|| payload.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Circle => payload
            .pointer("/notification/transfer/id")
            .or_else(|| payload.pointer("/notification/id"))
            .or_else(|| payload.get("subscriptionId"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Adyen => payload
            .pointer("/notificationItems/0/NotificationRequestItem/pspReference")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Square => payload
            .get("event_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
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
        WebhookProvider::Adyen => payload
            .pointer("/notificationItems/0/NotificationRequestItem/eventCode")
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
        WebhookProvider::Revolut => payload
            .pointer("/data/account_id")
            .or_else(|| payload.get("account_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::GoCardless => payload
            .pointer("/events/0/links/organisation")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Mercury => payload
            .pointer("/data/accountId")
            .or_else(|| payload.get("accountId"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Bridge => payload
            .pointer("/event_object/customer_id")
            .or_else(|| payload.get("customer_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Fireblocks => payload
            // Fireblocks sends `apiKey` or `workspaceId` in the wrapper
            // depending on the webhook type; we hash the API key on attach
            // so most workspace lookups fall back to the connection's
            // explicit external_account_id (the api_key uuid we stored).
            .get("apiKey")
            .or_else(|| payload.pointer("/data/apiKey"))
            .or_else(|| payload.get("workspaceId"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Circle => payload
            // Circle wraps the actual event inside `notification`; the
            // tenant-scoping field is `subscriptionId` (which maps 1:1
            // to a connection we registered the webhook URL with).
            .get("subscriptionId")
            .or_else(|| payload.pointer("/notification/subscriptionId"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Adyen => payload
            // The merchant account code (our stored external account id) is on
            // each notification item.
            .pointer("/notificationItems/0/NotificationRequestItem/merchantAccountCode")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        WebhookProvider::Square => payload
            // Square stamps the merchant id on every event envelope.
            .get("merchant_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        // Modern Treasury and Dwolla event bodies don't carry a stable
        // account-routing key; binding relies on the per-connection webhook
        // secret + (in strict mode) a resolvable connection.
        WebhookProvider::ModernTreasury => None,
        WebhookProvider::Dwolla => None,
    }
}

fn payload_sha_payload(payload: &Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    hex::encode(Sha256::digest(&bytes))[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- public ack shape -------------------------------------------

    /// The ack is the public face of every webhook endpoint. It is
    /// intentionally a single field so we don't leak resolved
    /// tenant/connection IDs to senders who may be probing.
    #[test]
    fn ack_serialises_to_received_only() {
        let ack = Ack { received: true };
        let s = serde_json::to_value(&ack).unwrap();
        assert_eq!(s, json!({ "received": true }));
        // Explicit defense: no tenant_id / connection_id keys, ever.
        assert!(s.get("tenant_id").is_none());
        assert!(s.get("connection_id").is_none());
        assert!(s.get("event_id").is_none());
        assert!(s.get("signature_ok").is_none());
    }

    // ---- event_id extraction ----------------------------------------

    #[test]
    fn event_id_stripe_uses_top_level_id() {
        let p = json!({"id": "evt_123", "type": "balance.available"});
        assert_eq!(
            event_id(WebhookProvider::Stripe, &p),
            Some("evt_123".into())
        );
    }

    #[test]
    fn event_id_paypal_uses_top_level_id() {
        let p = json!({"id": "WH-EVT-9", "event_type": "PAYMENT.SALE.COMPLETED"});
        assert_eq!(
            event_id(WebhookProvider::Paypal, &p),
            Some("WH-EVT-9".into())
        );
    }

    #[test]
    fn event_id_plaid_composes_item_code_and_hash() {
        // Plaid doesn't supply a stable event ID; we synthesize one
        // from (item_id, webhook_code, body_sha[..16]).
        let p = json!({
            "webhook_code": "SYNC_UPDATES_AVAILABLE",
            "item_id": "item_X",
            "new_transactions": 5,
        });
        let id = event_id(WebhookProvider::PlaidBank, &p).unwrap();
        assert!(id.starts_with("item_X:SYNC_UPDATES_AVAILABLE:"));
        assert_eq!(id.split(':').count(), 3);
    }

    #[test]
    fn event_id_plaid_none_when_missing_pieces() {
        assert!(event_id(WebhookProvider::PlaidBank, &json!({"item_id": "x"})).is_none());
        assert!(
            event_id(WebhookProvider::PlaidBank, &json!({"webhook_code": "x"})).is_none()
        );
    }

    #[test]
    fn event_id_fireblocks_pointer_then_top_level() {
        let nested = json!({"data": {"id": "fb_nested"}});
        assert_eq!(
            event_id(WebhookProvider::Fireblocks, &nested),
            Some("fb_nested".into())
        );
        let top = json!({"id": "fb_top"});
        assert_eq!(
            event_id(WebhookProvider::Fireblocks, &top),
            Some("fb_top".into())
        );
    }

    #[test]
    fn event_id_circle_prefers_transfer_id() {
        let p = json!({
            "notification": {
                "transfer": {"id": "transfer_42"},
                "id": "notif_99"
            },
            "subscriptionId": "sub_1"
        });
        assert_eq!(
            event_id(WebhookProvider::Circle, &p),
            Some("transfer_42".into())
        );
    }

    #[test]
    fn event_id_circle_falls_back_to_notification_id_then_sub() {
        let no_transfer = json!({
            "notification": {"id": "notif_99"},
            "subscriptionId": "sub_1"
        });
        assert_eq!(
            event_id(WebhookProvider::Circle, &no_transfer),
            Some("notif_99".into())
        );
        let only_sub = json!({"subscriptionId": "sub_1"});
        assert_eq!(
            event_id(WebhookProvider::Circle, &only_sub),
            Some("sub_1".into())
        );
    }

    // ---- event_type extraction --------------------------------------

    #[test]
    fn event_type_stripe_reads_type() {
        let p = json!({"type": "balance.available"});
        assert_eq!(
            event_type(WebhookProvider::Stripe, &p),
            Some("balance.available".into())
        );
    }

    #[test]
    fn event_type_paypal_reads_event_type() {
        let p = json!({"event_type": "PAYMENT.SALE.COMPLETED"});
        assert_eq!(
            event_type(WebhookProvider::Paypal, &p),
            Some("PAYMENT.SALE.COMPLETED".into())
        );
    }

    #[test]
    fn event_type_plaid_reads_webhook_code() {
        let p = json!({"webhook_code": "DEFAULT_UPDATE"});
        assert_eq!(
            event_type(WebhookProvider::PlaidBank, &p),
            Some("DEFAULT_UPDATE".into())
        );
    }

    // ---- external_account_id extraction -----------------------------

    #[test]
    fn external_account_id_stripe() {
        let p = json!({"account": "acct_AAA"});
        assert_eq!(
            external_account_id(WebhookProvider::Stripe, &p),
            Some("acct_AAA".into())
        );
    }

    #[test]
    fn external_account_id_paypal_prefers_merchant_id_then_payee() {
        let primary = json!({"resource": {"merchant_id": "M_PRIMARY"}});
        assert_eq!(
            external_account_id(WebhookProvider::Paypal, &primary),
            Some("M_PRIMARY".into())
        );
        let payee = json!({"resource": {"payee": {"merchant_id": "M_PAYEE"}}});
        assert_eq!(
            external_account_id(WebhookProvider::Paypal, &payee),
            Some("M_PAYEE".into())
        );
    }

    #[test]
    fn external_account_id_plaid_item_id() {
        let p = json!({"item_id": "item_X"});
        assert_eq!(
            external_account_id(WebhookProvider::PlaidBank, &p),
            Some("item_X".into())
        );
    }

    #[test]
    fn external_account_id_coinflow_camel_and_snake() {
        let snake = json!({"merchant_id": "m1"});
        assert_eq!(
            external_account_id(WebhookProvider::Coinflow, &snake),
            Some("m1".into())
        );
        let camel = json!({"merchantId": "m2"});
        assert_eq!(
            external_account_id(WebhookProvider::Coinflow, &camel),
            Some("m2".into())
        );
    }

    #[test]
    fn external_account_id_coinbase_commerce_nested() {
        let p = json!({"event": {"data": {"metadata": {"merchant_id": "cb_m"}}}});
        assert_eq!(
            external_account_id(WebhookProvider::CoinbaseCommerce, &p),
            Some("cb_m".into())
        );
    }

    #[test]
    fn external_account_id_gocardless_first_event_organisation() {
        let p = json!({
            "events": [
                {"id": "EV1", "links": {"organisation": "ORG_1"}},
                {"id": "EV2", "links": {"organisation": "ORG_2"}}
            ]
        });
        // Should always return the FIRST event's organisation — we
        // tenant-scope on connection, so any GoCardless batch is
        // already from a single tenant.
        assert_eq!(
            external_account_id(WebhookProvider::GoCardless, &p),
            Some("ORG_1".into())
        );
    }

    #[test]
    fn external_account_id_bridge_event_object_then_top() {
        let nested = json!({"event_object": {"customer_id": "cust_NEST"}});
        assert_eq!(
            external_account_id(WebhookProvider::Bridge, &nested),
            Some("cust_NEST".into())
        );
        let top = json!({"customer_id": "cust_TOP"});
        assert_eq!(
            external_account_id(WebhookProvider::Bridge, &top),
            Some("cust_TOP".into())
        );
    }

    #[test]
    fn external_account_id_fireblocks_falls_through_three_locations() {
        let api_key = json!({"apiKey": "AK"});
        assert_eq!(
            external_account_id(WebhookProvider::Fireblocks, &api_key),
            Some("AK".into())
        );
        let nested = json!({"data": {"apiKey": "AK2"}});
        assert_eq!(
            external_account_id(WebhookProvider::Fireblocks, &nested),
            Some("AK2".into())
        );
        let ws = json!({"workspaceId": "WS"});
        assert_eq!(
            external_account_id(WebhookProvider::Fireblocks, &ws),
            Some("WS".into())
        );
    }

    #[test]
    fn external_account_id_circle_subscription_top_then_nested() {
        let top = json!({"subscriptionId": "sub_top"});
        assert_eq!(
            external_account_id(WebhookProvider::Circle, &top),
            Some("sub_top".into())
        );
        let nested = json!({"notification": {"subscriptionId": "sub_nest"}});
        assert_eq!(
            external_account_id(WebhookProvider::Circle, &nested),
            Some("sub_nest".into())
        );
    }

    #[test]
    fn external_account_id_returns_none_when_missing() {
        let empty = json!({});
        assert!(external_account_id(WebhookProvider::Stripe, &empty).is_none());
        assert!(external_account_id(WebhookProvider::Paypal, &empty).is_none());
        assert!(external_account_id(WebhookProvider::Circle, &empty).is_none());
    }

    #[test]
    fn payload_sha_is_deterministic_and_truncated() {
        let p = json!({"foo": "bar"});
        let a = payload_sha_payload(&p);
        let b = payload_sha_payload(&p);
        assert_eq!(a, b);
        assert_eq!(a.len(), 16); // hex prefix
    }
}
