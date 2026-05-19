use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{Duration, Utc};
use rand::{rng, RngExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::{braintree, paypal, stripe, ProviderKind};
use crate::providers::connection::{CreateConnection, UpsertCredential};
use crate::providers::oauth_common::CodeExchangeResult;
use crate::scheduler::{CreateScheduledJob, ScheduleKind};
use crate::shard::Region;
use crate::state::AppState;

/// Default backstop poll cadence: 5x/day. Tenants can change this any time
/// via the standard scheduler API (`PATCH .../scheduled-jobs/{id}` once we
/// add it; meanwhile they can disable + re-create with a different cadence).
const BACKSTOP_SYNC_INTERVAL_SECONDS: i32 = 18_000;

#[derive(Deserialize)]
pub struct StartQuery {
    pub tenant_id: Uuid,
    pub return_to: Option<String>,
}

pub async fn start(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(q): Query<StartQuery>,
) -> AppResult<Response> {
    let mut nonce = [0u8; 16];
    rng().fill(&mut nonce[..]);
    let state_token = hex::encode(nonce);

    let provider_tag = provider.as_str();
    sqlx::query(
        r#"
        INSERT INTO oauth_states (state, tenant_id, provider, return_to, expires_at)
        VALUES ($1, $2, $3::provider_kind, $4, $5)
        "#,
    )
    .bind(&state_token)
    .bind(q.tenant_id)
    .bind(provider_tag)
    .bind(&q.return_to)
    .bind(Utc::now() + Duration::minutes(15))
    .execute(&state.pool)
    .await?;

    let url = match provider.as_str() {
        "stripe"    => stripe::StripeOAuth::new(&state.cfg).authorize_url(&state_token)?,
        "paypal"    => paypal::PaypalOAuth::new(&state.cfg).authorize_url(&state_token)?,
        "braintree" => braintree::BraintreeOAuth::new(&state.cfg).authorize_url(&state_token)?,
        other => return Err(AppError::BadRequest(format!("unsupported provider: {other}"))),
    };

    Ok(Redirect::to(&url).into_response())
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
}

#[derive(serde::Serialize)]
pub struct CallbackResp {
    pub provider: String,
    pub tenant_id: Uuid,
    pub connection_id: Option<Uuid>,
    pub status: &'static str,
    pub message: Option<String>,
    pub return_to: Option<String>,
    pub backstop_job_id: Option<Uuid>,
}

pub async fn callback(
    State(state): State<AppState>,
    Path(provider_str): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> AppResult<Json<CallbackResp>> {
    if let Some(err) = q.error {
        return Ok(Json(CallbackResp {
            provider: provider_str,
            tenant_id: Uuid::nil(),
            connection_id: None,
            status: "user_denied_or_error",
            message: Some(err),
            return_to: None,
            backstop_job_id: None,
        }));
    }

    // Consume the one-time CSRF state row. Same-tx delete-returning gives
    // single-use semantics.
    let row = sqlx::query(
        r#"
        DELETE FROM oauth_states
        WHERE state = $1
          AND provider = $2::provider_kind
          AND expires_at > now()
        RETURNING tenant_id, return_to
        "#,
    )
    .bind(&q.state)
    .bind(provider_str.as_str())
    .fetch_optional(&state.pool)
    .await?;

    let row = row.ok_or_else(|| AppError::BadRequest(
        "oauth state unknown, expired, or provider mismatch".into(),
    ))?;
    use sqlx::Row;
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let return_to: Option<String> = row.try_get("return_to")?;

    let code = q.code.ok_or_else(|| AppError::BadRequest("no code in callback".into()))?;

    let provider = parse_provider(&provider_str)?;

    // Exchange code -> sealed credential material.
    let exchanged: CodeExchangeResult = match provider {
        ProviderKind::Stripe => stripe::StripeOAuth::new(&state.cfg).exchange_code(&code).await?,
        ProviderKind::Paypal => paypal::PaypalOAuth::new(&state.cfg).exchange_code(&code).await?,
        ProviderKind::Braintree => braintree::BraintreeOAuth::new(&state.cfg).exchange_code(&code).await?,
        other => {
            return Err(AppError::BadRequest(format!(
                "{} is not a redirect-OAuth provider; use its dedicated endpoint",
                other.tag()
            )));
        }
    };

    let outcome = persist_and_schedule(
        &state,
        tenant_id,
        provider,
        exchanged,
    )
    .await?;

    Ok(Json(CallbackResp {
        provider: provider_str,
        tenant_id,
        connection_id: Some(outcome.connection_id),
        status: "active",
        message: Some("connection active; backstop sync scheduled".into()),
        return_to,
        backstop_job_id: Some(outcome.backstop_job_id),
    }))
}

// --- Plaid Link --------------------------------------------------------------

#[derive(Deserialize)]
pub struct PlaidLinkTokenReq {
    pub tenant_id: Uuid,
}

#[derive(serde::Serialize)]
pub struct PlaidLinkTokenResp {
    pub link_token: String,
}

/// `POST /v1/plaid/link-token` — frontend calls this to mint a token for
/// Plaid Link to use. Plaid Link returns a `public_token` to the frontend
/// which then POSTs to `/v1/plaid/exchange`.
pub async fn plaid_link_token(
    State(state): State<AppState>,
    Json(req): Json<PlaidLinkTokenReq>,
) -> AppResult<Json<PlaidLinkTokenResp>> {
    let plaid = crate::providers::plaid::PlaidLink::new(&state.cfg);
    let token = plaid.create_link_token(req.tenant_id).await?;
    Ok(Json(PlaidLinkTokenResp { link_token: token }))
}

#[derive(Deserialize)]
pub struct PlaidExchangeReq {
    pub tenant_id: Uuid,
    pub public_token: String,
    pub institution_id: Option<String>,
    pub institution_name: Option<String>,
}

pub async fn plaid_exchange(
    State(state): State<AppState>,
    Json(req): Json<PlaidExchangeReq>,
) -> AppResult<Json<CallbackResp>> {
    let plaid = crate::providers::plaid::PlaidLink::new(&state.cfg);
    let exchanged = plaid
        .exchange_public_token(
            &req.public_token,
            req.institution_id.as_deref(),
            req.institution_name.as_deref(),
        )
        .await?;

    let outcome = persist_and_schedule(
        &state,
        req.tenant_id,
        ProviderKind::PlaidBank,
        exchanged,
    )
    .await?;

    Ok(Json(CallbackResp {
        provider: "plaid_bank".into(),
        tenant_id: req.tenant_id,
        connection_id: Some(outcome.connection_id),
        status: "active",
        message: Some("plaid bank connected; backstop sync scheduled".into()),
        return_to: None,
        backstop_job_id: Some(outcome.backstop_job_id),
    }))
}

// --- Common persist + schedule helpers --------------------------------------

struct PersistOutcome {
    connection_id: Uuid,
    backstop_job_id: Uuid,
}

async fn persist_and_schedule(
    state: &AppState,
    tenant_id: Uuid,
    provider: ProviderKind,
    exchanged: CodeExchangeResult,
) -> AppResult<PersistOutcome> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region: Region = tenant.region()?;

    // Find-or-create the connection row. We prefer the most recently created
    // pending row for this tenant+provider so a user's "Connect Stripe" click
    // flows into the same row through the redirect.
    let conn = match state
        .connections
        .find_pending_for_oauth(tenant_id, provider)
        .await?
    {
        Some(existing) => existing,
        None => {
            state
                .connections
                .create(
                    tenant_id,
                    region,
                    CreateConnection {
                        provider,
                        display_label: exchanged
                            .display_label_suggestion
                            .clone()
                            .unwrap_or_else(|| provider.tag().to_string()),
                        external_account_id: Some(exchanged.external_account_id.clone()),
                        metadata: serde_json::json!({}),
                    },
                )
                .await?
        }
    };

    // Seal + persist credential material; flips status to active.
    let _ = state
        .connections
        .attach_credential(
            tenant_id,
            conn.id,
            UpsertCredential {
                plaintext: exchanged.sealed_plaintext,
                scopes: exchanged.scopes,
                expires_at: exchanged.expires_at,
            },
        )
        .await?;

    // If the OAuth response revealed a real external account id (e.g.
    // Stripe Connect's `stripe_user_id`), persist it now so sync can scope.
    if !exchanged.external_account_id.is_empty()
        && exchanged.external_account_id != "pending"
    {
        let _ = state
            .connections
            .set_external_account(conn.id, &exchanged.external_account_id)
            .await;
    }

    // Auto-register the backstop sync.connection scheduled job. Default
    // cadence 5x/day; tenants override per-connection.
    let backstop = state
        .scheduler
        .create(
            Some(tenant_id),
            Some(region),
            CreateScheduledJob {
                kind: "sync.connection".into(),
                name: format!("backstop-conn-{}", conn.id),
                schedule_kind: ScheduleKind::Interval,
                cron_expr: None,
                interval_seconds: Some(BACKSTOP_SYNC_INTERVAL_SECONDS),
                one_shot_at: None,
                timezone: "UTC".into(),
                payload: serde_json::json!({
                    "connection_id": conn.id,
                    "trigger": "backstop"
                }),
                enabled: true,
                max_attempts: 3,
                retry_backoff_secs: 300,
                timeout_seconds: 600,
            },
        )
        .await?;

    Ok(PersistOutcome {
        connection_id: conn.id,
        backstop_job_id: backstop.id,
    })
}

fn parse_provider(s: &str) -> AppResult<ProviderKind> {
    match s {
        "stripe"            => Ok(ProviderKind::Stripe),
        "paypal"            => Ok(ProviderKind::Paypal),
        "braintree"         => Ok(ProviderKind::Braintree),
        "coinbase_commerce" => Ok(ProviderKind::CoinbaseCommerce),
        "coinbase_prime"    => Ok(ProviderKind::CoinbasePrime),
        "coinflow"          => Ok(ProviderKind::Coinflow),
        "plaid_bank"        => Ok(ProviderKind::PlaidBank),
        "swift_wire"        => Ok(ProviderKind::SwiftWire),
        "ach_direct"        => Ok(ProviderKind::AchDirect),
        "wise"              => Ok(ProviderKind::Wise),
        "solana_wallet"     => Ok(ProviderKind::SolanaWallet),
        other => Err(AppError::BadRequest(format!("unknown provider: {other}"))),
    }
}
