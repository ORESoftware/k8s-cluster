use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::providers::connection::{CreateConnection, ProviderConnection, UpsertCredential};
use crate::providers::ProviderAuthKind;
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
    Ok((StatusCode::ACCEPTED, Json(SyncNowResponse { job, runs_url })))
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

    let plaintext = serde_json::to_vec(&req.credential).map_err(|e| AppError::BadRequest(
        format!("credential must be a JSON object: {e}"),
    ))?;

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

    if let Some(ext) = req.external_account_id.as_deref() {
        let _ = state.connections.set_external_account(connection_id, ext).await;
    }

    if let Some(env) = req.environment.as_deref() {
        let _ = state
            .connections
            .merge_metadata(
                connection_id,
                serde_json::json!({ "environment": env }),
            )
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
