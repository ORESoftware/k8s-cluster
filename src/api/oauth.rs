use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{Duration, Utc};
use rand::{RngExt, rng};
use sea_orm::ConnectionTrait;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::auth::{self, MAX_STEP_UP_AGE_SECS, Principal};
use crate::error::{AppError, AppResult};
use crate::memberships::SCOPE_BILLING_WRITE;
use crate::providers::connection::{CreateConnection, UpsertCredential};
use crate::providers::oauth_common::CodeExchangeResult;
use crate::providers::{ProviderKind, braintree, paypal, stripe};
use crate::scheduler::{CreateScheduledJob, ScheduleKind};
use crate::shard::Region;
use crate::state::AppState;

/// Default backstop poll cadence: 5x/day. Tenants can change this any time
/// via the standard scheduler API (`PATCH .../scheduled-jobs/{id}` once we
/// add it; meanwhile they can disable + re-create with a different cadence).
const BACKSTOP_SYNC_INTERVAL_SECONDS: i32 = 18_000;
const OAUTH_STATE_TTL_MINUTES: i64 = 15;
const AUTH_TIME_FUTURE_SKEW_SECONDS: u64 = 30;

#[derive(Deserialize)]
pub struct StartQuery {
    pub tenant_id: Uuid,
    pub return_to: Option<String>,
}

pub async fn start(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(provider_str): Path<String>,
    Query(q): Query<StartQuery>,
) -> AppResult<Response> {
    // Reject unsupported/non-redirect providers before creating a state row.
    let provider = parse_redirect_provider(&provider_str)?;

    // This route carries tenant_id in the query rather than the canonical path,
    // so middleware cannot infer it. Resolve the exact membership explicitly
    // before minting state or contacting a provider.
    auth::require_embedded_tenant_write(&state, &principal, q.tenant_id).await?;
    let user = principal.user()?;
    let initiating_shared_user_id = user.identity.subject.clone();
    let auth_time_unix = i64::try_from(user.identity.issued_at).map_err(|_| AppError::Forbidden)?;

    // 256 bits keeps the callback capability comfortably above the strength of
    // the access token and provider authorization code it protects.
    let mut nonce = [0u8; 32];
    rng().fill(&mut nonce[..]);
    let state_token = hex::encode(nonce);
    let return_to = validate_return_to(&state, q.return_to.as_deref())?;

    state
        .pool
        .execute(crate::db::stmt(
            r#"
        INSERT INTO oauth_states
            (state, tenant_id, provider, return_to, expires_at,
             initiating_shared_user_id, auth_time_unix)
        VALUES ($1, $2, $3::provider_kind, $4, $5, $6, $7)
        "#,
            [
                state_token.clone().into(),
                q.tenant_id.into(),
                provider.tag().into(),
                return_to.clone().into(),
                (Utc::now() + Duration::minutes(OAUTH_STATE_TTL_MINUTES)).into(),
                initiating_shared_user_id.into(),
                auth_time_unix.into(),
            ],
        ))
        .await?;

    let url = match provider {
        ProviderKind::Stripe => stripe::StripeOAuth::new(&state.cfg).authorize_url(&state_token)?,
        ProviderKind::Paypal => paypal::PaypalOAuth::new(&state.cfg).authorize_url(&state_token)?,
        ProviderKind::Braintree => {
            braintree::BraintreeOAuth::new(&state.cfg).authorize_url(&state_token)?
        }
        _ => unreachable!("parse_redirect_provider returned a non-redirect provider"),
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
    let provider = parse_redirect_provider(&provider_str)?;

    // Consume the callback capability exactly once, including provider-denied
    // callbacks. The row binds this callback to the exact Shared Auth principal
    // and factor ceremony that authorized the flow. Legacy/unbound rows fail
    // closed after a rolling deployment.
    let row = state
        .pool
        .query_one(crate::db::stmt(
            r#"
        DELETE FROM oauth_states
        WHERE state = $1
          AND provider = $2::provider_kind
          AND expires_at > now()
        RETURNING tenant_id, return_to, initiating_shared_user_id, auth_time_unix
        "#,
            [q.state.clone().into(), provider.tag().into()],
        ))
        .await?;

    let row = row.ok_or_else(|| {
        AppError::BadRequest("oauth state unknown, expired, or provider mismatch".into())
    })?;
    let tenant_id: Uuid = row.try_get("", "tenant_id")?;
    let return_to: Option<String> = row.try_get("", "return_to")?;
    let initiating_shared_user_id: Option<String> = row.try_get("", "initiating_shared_user_id")?;
    let auth_time_unix: Option<i64> = row.try_get("", "auth_time_unix")?;
    let initiating_shared_user_id = initiating_shared_user_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .ok_or(AppError::Forbidden)?;
    let auth_time_unix = auth_time_unix
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(AppError::Forbidden)?;

    // A denial/error still consumes and validates state, but it stores no
    // provider credential and therefore does not need a live billing grant.
    if let Some(error) = q.error {
        return Ok(Json(CallbackResp {
            provider: provider_str,
            tenant_id,
            connection_id: None,
            status: "user_denied_or_error",
            message: Some(sanitize_provider_error(&error)),
            return_to,
            backstop_job_id: None,
        }));
    }

    require_callback_authorization(
        &state,
        tenant_id,
        &initiating_shared_user_id,
        auth_time_unix,
    )
    .await?;

    let code = q
        .code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty() && code.len() <= 8 * 1024)
        .ok_or_else(|| AppError::BadRequest("missing or oversized oauth code".into()))?;

    let exchanged: CodeExchangeResult = match provider {
        ProviderKind::Stripe => {
            stripe::StripeOAuth::new(&state.cfg)
                .exchange_code(code)
                .await?
        }
        ProviderKind::Paypal => {
            paypal::PaypalOAuth::new(&state.cfg)
                .exchange_code(code)
                .await?
        }
        ProviderKind::Braintree => {
            braintree::BraintreeOAuth::new(&state.cfg)
                .exchange_code(code)
                .await?
        }
        _ => unreachable!("parse_redirect_provider returned a non-redirect provider"),
    };

    // Provider exchange is a network operation. Recheck after it completes so
    // a grant revoked while the user was at the provider cannot be used to
    // attach a credential. This also re-evaluates freshness against the actual
    // factor ceremony rather than extending it from OAuth start time.
    require_callback_authorization(
        &state,
        tenant_id,
        &initiating_shared_user_id,
        auth_time_unix,
    )
    .await?;

    let outcome = persist_and_schedule(&state, tenant_id, provider, exchanged).await?;
    tracing::info!(
        tenant.id = %tenant_id,
        auth.subject = %initiating_shared_user_id,
        auth.auth_time = auth_time_unix,
        provider = provider.tag(),
        connection.id = %outcome.connection_id,
        "provider OAuth connection authorized by Shared Auth principal"
    );

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

async fn require_callback_authorization(
    state: &AppState,
    tenant_id: Uuid,
    shared_user_id: &str,
    auth_time_unix: u64,
) -> AppResult<()> {
    if !authorization_time_is_fresh(auth_time_unix, now_seconds()) {
        tracing::warn!(
            tenant.id = %tenant_id,
            auth.subject = %shared_user_id,
            auth.auth_time = auth_time_unix,
            "OAuth callback rejected: Shared Auth step-up is stale or future-dated"
        );
        return Err(AppError::Forbidden);
    }
    state
        .memberships
        .require_scope(tenant_id, shared_user_id, SCOPE_BILLING_WRITE)
        .await?;
    Ok(())
}

fn authorization_time_is_fresh(auth_time_unix: u64, now: u64) -> bool {
    auth_time_unix <= now.saturating_add(AUTH_TIME_FUTURE_SKEW_SECONDS)
        && now.saturating_sub(auth_time_unix) <= MAX_STEP_UP_AGE_SECS
}

fn sanitize_provider_error(error: &str) -> String {
    let value = error
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect::<String>();
    if value.is_empty() {
        "provider denied authorization".to_owned()
    } else {
        value
    }
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
    Extension(principal): Extension<Principal>,
    Json(req): Json<PlaidLinkTokenReq>,
) -> AppResult<Json<PlaidLinkTokenResp>> {
    auth::require_embedded_tenant_write(&state, &principal, req.tenant_id).await?;
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
    Extension(principal): Extension<Principal>,
    Json(req): Json<PlaidExchangeReq>,
) -> AppResult<Json<CallbackResp>> {
    auth::require_embedded_tenant_write(&state, &principal, req.tenant_id).await?;
    let plaid = crate::providers::plaid::PlaidLink::new(&state.cfg);
    let exchanged = plaid
        .exchange_public_token(
            &req.public_token,
            req.institution_id.as_deref(),
            req.institution_name.as_deref(),
        )
        .await?;

    // Plaid exchange remains authenticated end-to-end, but recheck immediately
    // before persistence because the upstream call may have taken long enough
    // for the tenant grant or step-up window to change.
    auth::require_embedded_tenant_write(&state, &principal, req.tenant_id).await?;
    let outcome =
        persist_and_schedule(&state, req.tenant_id, ProviderKind::PlaidBank, exchanged).await?;

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

    if !exchanged.external_account_id.is_empty() && exchanged.external_account_id != "pending" {
        let _ = state
            .connections
            .set_external_account(tenant_id, conn.id, &exchanged.external_account_id)
            .await;
    }

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

fn parse_redirect_provider(value: &str) -> AppResult<ProviderKind> {
    match value {
        "stripe" => Ok(ProviderKind::Stripe),
        "paypal" => Ok(ProviderKind::Paypal),
        "braintree" => Ok(ProviderKind::Braintree),
        other => Err(AppError::BadRequest(format!(
            "{other} is not a supported redirect-OAuth provider"
        ))),
    }
}

fn validate_return_to(state: &AppState, return_to: Option<&str>) -> AppResult<Option<String>> {
    let Some(return_to) = return_to.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };

    if return_to.starts_with("//") {
        return Err(AppError::BadRequest(
            "return_to must not be protocol-relative".into(),
        ));
    }

    if state
        .cfg
        .oauth_return_to_allowed_prefixes
        .iter()
        .any(|prefix| return_to.starts_with(prefix))
    {
        return Ok(Some(return_to.to_string()));
    }

    Err(AppError::BadRequest(
        "return_to must match BILLING_OAUTH_RETURN_TO_ALLOWED_PREFIXES \
         (explicit allowlist required; no implicit relative-path bypass)"
            .into(),
    ))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_cfg(allowed: Vec<&str>) -> std::sync::Arc<crate::config::Config> {
        let mut cfg = crate::config::Config::for_tests();
        cfg.oauth_return_to_allowed_prefixes = allowed.into_iter().map(str::to_string).collect();
        std::sync::Arc::new(cfg)
    }

    fn check(prefixes: &[&str], input: Option<&str>) -> Result<Option<String>, String> {
        let allowed: Vec<String> = prefixes.iter().map(|s| s.to_string()).collect();
        let Some(rt) = input.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if rt.starts_with("//") {
            return Err("protocol-relative".into());
        }
        if allowed.iter().any(|p| rt.starts_with(p)) {
            return Ok(Some(rt.to_string()));
        }
        Err("not in allowlist".into())
    }

    #[test]
    fn empty_or_none_returns_ok_none() {
        assert!(check(&[], None).unwrap().is_none());
        assert!(check(&[], Some("")).unwrap().is_none());
        assert!(check(&[], Some("   ")).unwrap().is_none());
    }

    #[test]
    fn relative_path_no_longer_auto_allowed() {
        assert!(check(&[], Some("/dashboard")).is_err());
    }

    #[test]
    fn relative_path_allowed_when_listed() {
        let result = check(&["/dashboard"], Some("/dashboard?ok=1"))
            .unwrap()
            .unwrap();
        assert_eq!(result, "/dashboard?ok=1");
    }

    #[test]
    fn protocol_relative_rejected() {
        assert!(check(&[], Some("//evil.example/path")).is_err());
        assert!(check(&["//evil.example"], Some("//evil.example/x")).is_err());
    }

    #[test]
    fn absolute_url_allowed_only_via_allowlist() {
        assert!(check(&[], Some("https://app.example/done")).is_err());
        let result = check(&["https://app.example/"], Some("https://app.example/done"))
            .unwrap()
            .unwrap();
        assert_eq!(result, "https://app.example/done");
    }

    #[test]
    fn for_tests_cfg_has_no_allow_list_by_default() {
        let cfg = mk_cfg(vec![]);
        assert!(cfg.oauth_return_to_allowed_prefixes.is_empty());
    }

    #[test]
    fn callback_step_up_freshness_uses_original_ceremony_time() {
        let now = 1_000_000;
        assert!(authorization_time_is_fresh(now, now));
        assert!(authorization_time_is_fresh(now - MAX_STEP_UP_AGE_SECS, now));
        assert!(!authorization_time_is_fresh(
            now - MAX_STEP_UP_AGE_SECS - 1,
            now
        ));
        assert!(!authorization_time_is_fresh(
            now + AUTH_TIME_FUTURE_SKEW_SECONDS + 1,
            now
        ));
    }

    #[test]
    fn provider_error_is_bounded_and_control_free() {
        let input = format!("denied\n{}", "x".repeat(1024));
        let output = sanitize_provider_error(&input);
        assert!(!output.contains('\n'));
        assert!(output.chars().count() <= 512);
    }
}
