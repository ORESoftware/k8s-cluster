use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::ProviderAuthKind;
use crate::providers::connection::{CreateConnection, ProviderConnection, UpsertCredential};
use crate::providers::{
    ProviderKind, coinbase::CoinbaseCredential, coinflow::CoinflowCredential, wise::WiseCredential,
};
use crate::scheduler::{CreateScheduledJob, ScheduleKind, ScheduledJob};
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<ProviderConnection>>> {
    let rows = state.connections.list_for_tenant(tenant_id).await?;
    Ok(Json(rows))
}

pub async fn create(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<CreateConnection>,
) -> AppResult<Json<ProviderConnection>> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let conn = state.connections.create(tenant_id, region, input).await?;
    Ok(Json(conn))
}

#[derive(Debug, Default, Deserialize)]
pub struct SyncNowRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    /// "user", "webhook", "api", etc. Recorded on the lease + run for audit.
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Serialize)]
pub struct SyncNowResponse {
    /// The one-shot scheduled job that will execute the sync.
    pub job: ScheduledJob,
    /// Convenience hint for clients: poll `runs_url` to see the result.
    pub runs_url: String,
}

/// On-demand sync trigger. This is the *primary* sync mechanism — the
/// backstop poller (default 5x/day) only catches what this missed. Returns
/// quickly with a job handle the client can poll for results.
pub async fn sync_now(
    State(state): State<AppState>,
    Path((tenant_id, connection_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SyncNowRequest>,
) -> AppResult<(StatusCode, Json<SyncNowResponse>)> {
    // Validate the connection exists for this tenant up front so we don't
    // queue garbage. The job handler also validates.
    let _conn = state.connections.get(tenant_id, connection_id).await?;
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;

    let payload = serde_json::json!({
        "connection_id": connection_id,
        "cursor": req.cursor,
        "trigger": req.trigger.unwrap_or_else(|| "on_demand".into()),
    });

    let job = state
        .scheduler
        .enqueue_one_shot(
            tenant_id,
            region,
            "sync.connection",
            format!("on-demand-conn-{}", connection_id),
            payload,
        )
        .await?;

    let runs_url = format!(
        "/v1/tenants/{tenant_id}/scheduled-jobs/{}/runs?limit=1",
        job.id
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(SyncNowResponse { job, runs_url }),
    ))
}

// --- API-key attach (Coinflow / Coinbase / Wise / any non-OAuth provider) --

#[derive(Debug, Deserialize)]
pub struct AttachApiKeyRequest {
    /// Provider-specific credential payload, as JSON. The shape depends on
    /// the provider (see each provider's `*Credential` struct, e.g.
    /// `CoinflowCredential { api_key, merchant_id, environment,
    /// webhook_validation_key }`). We seal these bytes as-is.
    pub credential: serde_json::Value,
    /// Optional: lets the caller stamp the connection with the merchant id
    /// they just pasted, before sync ever runs.
    pub external_account_id: Option<String>,
    /// "production" | "sandbox" — recorded as connection metadata for ops
    /// visibility; the provider's own credential payload is the actual
    /// source of truth.
    pub environment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttachApiKeyResponse {
    pub connection_id: Uuid,
    pub status: &'static str,
    pub backstop_job_id: Uuid,
}

/// `POST /v1/tenants/{tenant_id}/connections/{connection_id}/attach-api-key`
///
/// For non-OAuth providers (Coinflow, Coinbase, Wise, etc.), the tenant
/// pastes their API key + merchant id into our dashboard. We seal the
/// provider-specific credential JSON, flip the connection to `active`,
/// and auto-register the backstop sync job. Mirror of what the OAuth
/// callback does for OAuth providers — same end state.
pub async fn attach_api_key(
    State(state): State<AppState>,
    Path((tenant_id, connection_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<AttachApiKeyRequest>,
) -> AppResult<(StatusCode, Json<AttachApiKeyResponse>)> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;

    let conn = state.connections.get(tenant_id, connection_id).await?;

    if conn.auth_kind != ProviderAuthKind::ApiKey {
        return Err(AppError::BadRequest(format!(
            "connection {connection_id} ({}) does not use api_key auth; \
             use the OAuth flow instead",
            conn.provider.tag()
        )));
    }

    let derived_external_account_id = validate_api_key_credential(conn.provider, &req.credential)?;

    let plaintext = serde_json::to_vec(&req.credential)
        .map_err(|e| AppError::BadRequest(format!("credential must be a JSON object: {e}")))?;

    let _ = state
        .connections
        .attach_credential(
            tenant_id,
            connection_id,
            UpsertCredential {
                plaintext,
                scopes: vec![],
                expires_at: None,
            },
        )
        .await?;

    let external_account_id = req
        .external_account_id
        .as_deref()
        .map(str::to_string)
        .or(derived_external_account_id);

    if let Some(ext) = external_account_id.as_deref() {
        let _ = state
            .connections
            .set_external_account(connection_id, ext)
            .await;
    }

    if let Some(env) = req.environment.as_deref() {
        let _ = state
            .connections
            .merge_metadata(connection_id, serde_json::json!({ "environment": env }))
            .await;
    }

    let backstop = state
        .scheduler
        .create(
            Some(tenant_id),
            Some(region),
            CreateScheduledJob {
                kind: "sync.connection".into(),
                name: format!("backstop-conn-{}", connection_id),
                schedule_kind: ScheduleKind::Interval,
                cron_expr: None,
                interval_seconds: Some(18_000),
                one_shot_at: None,
                timezone: "UTC".into(),
                payload: serde_json::json!({
                    "connection_id": connection_id,
                    "trigger": "backstop"
                }),
                enabled: true,
                max_attempts: 3,
                retry_backoff_secs: 300,
                timeout_seconds: 600,
            },
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(AttachApiKeyResponse {
            connection_id,
            status: "active",
            backstop_job_id: backstop.id,
        }),
    ))
}

fn validate_api_key_credential(
    provider: ProviderKind,
    credential: &serde_json::Value,
) -> AppResult<Option<String>> {
    match provider {
        ProviderKind::Coinflow => {
            let cred: CoinflowCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid coinflow credential: {e}")))?;
            require_non_empty("coinflow.api_key", &cred.api_key)?;
            require_non_empty("coinflow.merchant_id", &cred.merchant_id)?;
            validate_environment("coinflow.environment", &cred.environment)?;
            Ok(Some(cred.merchant_id))
        }
        ProviderKind::CoinbaseCommerce | ProviderKind::CoinbasePrime => {
            let cred: CoinbaseCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid coinbase credential: {e}")))?;
            require_non_empty("coinbase.api_key", &cred.api_key)?;
            require_non_empty("coinbase.webhook_secret", &cred.webhook_secret)?;
            Ok(None)
        }
        ProviderKind::Wise => {
            let cred: WiseCredential = serde_json::from_value(credential.clone())
                .map_err(|e| AppError::BadRequest(format!("invalid wise credential: {e}")))?;
            require_non_empty("wise.api_token", &cred.api_token)?;
            require_non_empty("wise.profile_id", &cred.profile_id)?;
            validate_environment("wise.environment", &cred.environment)?;
            Ok(Some(cred.profile_id))
        }
        ProviderKind::Revolut => {
            let cred: crate::providers::revolut::RevolutCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid revolut credential: {e}"))
                })?;
            require_non_empty("revolut.access_token", &cred.access_token)?;
            validate_environment("revolut.environment", &cred.environment)?;
            Ok(None)
        }
        ProviderKind::Mercury => {
            let cred: crate::providers::mercury::MercuryCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid mercury credential: {e}"))
                })?;
            require_non_empty("mercury.api_key", &cred.api_key)?;
            Ok(None)
        }
        ProviderKind::Bridge => {
            let cred: crate::providers::bridge::BridgeCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid bridge credential: {e}"))
                })?;
            require_non_empty("bridge.api_key", &cred.api_key)?;
            validate_environment("bridge.environment", &cred.environment)?;
            Ok(None)
        }
        ProviderKind::GoCardless => {
            let cred: crate::providers::gocardless::GoCardlessCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid gocardless credential: {e}"))
                })?;
            require_non_empty("gocardless.access_token", &cred.access_token)?;
            // gocardless uses "live"/"sandbox", not "production"/"sandbox" —
            // accept whatever the tenant sends and validate against its own list
            let env = cred.environment.trim().to_lowercase();
            if !matches!(env.as_str(), "live" | "sandbox") {
                return Err(AppError::BadRequest(format!(
                    "gocardless.environment must be 'live' or 'sandbox' (got {env})"
                )));
            }
            Ok(None)
        }
        ProviderKind::Remitly => {
            let _cred: crate::providers::remitly::RemitlyCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid remitly credential: {e}"))
                })?;
            // No required fields — Remitly is limited_fit; we accept the
            // attach so tenants can register intent, but sync is a no-op.
            Ok(None)
        }
        ProviderKind::Robinhood => {
            let _cred: crate::providers::robinhood::RobinhoodCredential =
                serde_json::from_value(credential.clone()).map_err(|e| {
                    AppError::BadRequest(format!("invalid robinhood credential: {e}"))
                })?;
            Ok(None)
        }
        ProviderKind::Stripe
        | ProviderKind::Paypal
        | ProviderKind::Braintree
        | ProviderKind::PlaidBank
        | ProviderKind::SwiftWire
        | ProviderKind::AchDirect
        | ProviderKind::SolanaWallet => Ok(None),
    }
}

fn require_non_empty(field: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    Ok(())
}

fn validate_environment(field: &str, value: &str) -> AppResult<()> {
    match value.to_ascii_lowercase().as_str() {
        "production" | "sandbox" => Ok(()),
        other => Err(AppError::BadRequest(format!(
            "{field} must be production or sandbox, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_coinflow_and_derives_merchant_id() {
        let credential = serde_json::json!({
            "api_key": "cf_test",
            "merchant_id": "merchant_123",
            "environment": "sandbox",
            "webhook_validation_key": "hook_secret"
        });

        let derived = validate_api_key_credential(ProviderKind::Coinflow, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("merchant_123"));
    }

    #[test]
    fn validates_wise_and_derives_profile_id() {
        let credential = serde_json::json!({
            "api_token": "wise_test",
            "profile_id": "profile_456",
            "environment": "production"
        });

        let derived = validate_api_key_credential(ProviderKind::Wise, &credential).unwrap();

        assert_eq!(derived.as_deref(), Some("profile_456"));
    }

    #[test]
    fn rejects_empty_coinbase_webhook_secret() {
        let credential = serde_json::json!({
            "api_key": "coinbase_test",
            "webhook_secret": "",
            "variant": "commerce"
        });

        let err =
            validate_api_key_credential(ProviderKind::CoinbaseCommerce, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_unknown_environment() {
        let credential = serde_json::json!({
            "api_token": "wise_test",
            "profile_id": "profile_456",
            "environment": "staging"
        });

        let err = validate_api_key_credential(ProviderKind::Wise, &credential).unwrap_err();

        assert!(matches!(err, AppError::BadRequest(_)));
    }
}
