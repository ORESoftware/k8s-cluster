use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use chrono::{Duration, Utc};
use rand::{rng, RngExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::{paypal, stripe, braintree};
use crate::state::AppState;

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
    // CSRF state nonce, persisted with provider + tenant for callback validation.
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
    pub connection_id: Option<Uuid>,
    pub status: &'static str,
    pub message: Option<String>,
    pub return_to: Option<String>,
}

pub async fn callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> AppResult<Json<CallbackResp>> {
    if let Some(err) = q.error {
        return Ok(Json(CallbackResp {
            provider,
            connection_id: None,
            status: "user_denied_or_error",
            message: Some(err),
            return_to: None,
        }));
    }

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
    .bind(provider.as_str())
    .fetch_optional(&state.pool)
    .await?;

    let row = row.ok_or_else(|| AppError::BadRequest(
        "oauth state unknown, expired, or provider mismatch".into(),
    ))?;
    use sqlx::Row;
    let _tenant_id: Uuid = row.try_get("tenant_id")?;
    let return_to: Option<String> = row.try_get("return_to")?;

    let _code = q.code.ok_or_else(|| AppError::BadRequest("no code in callback".into()))?;

    // TODO(real impl): for each provider, exchange code -> credential, then
    // call state.connections.attach_credential(...). Returning a stub status
    // here so the wire-up is verifiable end-to-end without external creds.
    Ok(Json(CallbackResp {
        provider,
        connection_id: None,
        status: "exchange_not_implemented",
        message: Some("token exchange stubbed; provider impl pending".into()),
        return_to,
    }))
}
