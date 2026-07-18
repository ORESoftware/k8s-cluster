//! Postgres persistence for build jobs, webhook deliveries, and secret-sync
//! audit rows, against the build server's OWN database (`dd_build_server` on
//! Amazon RDS — its own namespace, separate from the shared pg-defs contract).
//!
//! Schema management is declarative: the contract lives at
//! remote/libs/pg-defs/schema/databases/dd_build_server/schema.sql and
//! operators converge the live database with scripts/dpm.sh (dpm). The server
//! NEVER migrates at boot.
//!
//! Persistence is optional and fail-open for availability: when
//! BUILD_SERVER_DATABASE_URL / DATABASE_URL is unset the server runs with the
//! in-memory job map only (current behavior), and a persistence write failure
//! degrades to a logged warning instead of failing builds.

use chrono::{DateTime, TimeZone, Utc};
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectOptions,
    Database, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use std::time::Duration;

use crate::entity::{build_jobs, gh_secret_sync_runs, webhook_deliveries};
use crate::{BuildJobRecord, BuildStatus};

pub async fn connect(database_url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut opts = ConnectOptions::new(database_url.to_string());
    opts.max_connections(8)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .sqlx_logging(true)
        .sqlx_logging_level(log::LevelFilter::Debug);
    let db = Database::connect(opts).await?;
    tracing::info!(
        "build-server database connected; migrations are not run at boot — converge the schema \
         with scripts/dpm.sh (contract: remote/libs/pg-defs/schema/databases/dd_build_server)"
    );
    Ok(db)
}

fn ms_to_datetime(ms: u128) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Utc::now)
}

fn status_str(status: &BuildStatus) -> &'static str {
    match status {
        BuildStatus::Queued => "queued",
        BuildStatus::Running => "running",
        BuildStatus::Succeeded => "succeeded",
        BuildStatus::Failed => "failed",
    }
}

fn job_active_model(job: &BuildJobRecord) -> build_jobs::ActiveModel {
    let request_json = serde_json::to_value(&job.request).unwrap_or(serde_json::Value::Null);
    build_jobs::ActiveModel {
        id: Set(job.id.clone()),
        status: Set(status_str(&job.status).to_string()),
        job_kind: Set(job
            .request
            .job_kind
            .clone()
            .unwrap_or_else(|| "build-image".to_string())),
        source: Set(job.source.clone()),
        executor: Set(job.executor.clone()),
        repo_url: Set(job.request.repo_url.clone()),
        git_ref: Set(job.request.git_ref.clone()),
        image: Set(job.request.image.clone()),
        request: Set(request_json),
        error: Set(job.error.clone()),
        log_path: Set(Some(job.log_path.clone())),
        lock_key: Set(job.lock_key.clone()),
        fencing_token: Set(job.fencing_token.map(|token| token as i64)),
        created_at: Set(ms_to_datetime(job.created_at_ms).into()),
        started_at: Set(job.started_at_ms.map(|ms| ms_to_datetime(ms).into())),
        finished_at: Set(job.finished_at_ms.map(|ms| ms_to_datetime(ms).into())),
    }
}

/// Upsert the full job row. Called on submit and on every status transition;
/// failures are logged and swallowed so persistence problems never fail builds.
pub async fn persist_job(db: &DatabaseConnection, job: &BuildJobRecord) {
    let model = job_active_model(job);
    let on_conflict = OnConflict::column(build_jobs::Column::Id)
        .update_columns([
            build_jobs::Column::Status,
            build_jobs::Column::Error,
            build_jobs::Column::LogPath,
            build_jobs::Column::LockKey,
            build_jobs::Column::FencingToken,
            build_jobs::Column::StartedAt,
            build_jobs::Column::FinishedAt,
        ])
        .to_owned();
    if let Err(error) = build_jobs::Entity::insert(model)
        .on_conflict(on_conflict)
        .exec(db)
        .await
    {
        tracing::warn!("failed to persist build job row: {error}");
    }
}

/// Boot recovery: any job left `queued`/`running` by a previous process is
/// terminal now (the in-memory execution task died with the old process).
pub async fn fail_interrupted_jobs(db: &DatabaseConnection) {
    let result = build_jobs::Entity::update_many()
        .col_expr(
            build_jobs::Column::Status,
            sea_orm::sea_query::Expr::value("failed"),
        )
        .col_expr(
            build_jobs::Column::Error,
            sea_orm::sea_query::Expr::value("interrupted by build-server restart"),
        )
        .col_expr(
            build_jobs::Column::FinishedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(build_jobs::Column::Status.is_in(["queued", "running"]))
        .exec(db)
        .await;
    match result {
        Ok(update) if update.rows_affected > 0 => {
            tracing::warn!(
                "marked {} interrupted build job(s) as failed after restart",
                update.rows_affected
            );
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("failed to mark interrupted build jobs: {error}"),
    }
}

/// Recent persisted jobs, newest first (for GET /builds continuity across
/// restarts; the in-memory map only holds jobs from the current process).
pub async fn recent_jobs(db: &DatabaseConnection, limit: u64) -> Vec<build_jobs::Model> {
    build_jobs::Entity::find()
        .order_by_desc(build_jobs::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!("failed to load recent build jobs: {error}");
            Vec::new()
        })
}

/// Insert a webhook delivery row. Returns false when the (provider,
/// delivery_id) pair already exists — the redelivery/dedupe signal.
pub async fn record_webhook_delivery(
    db: &DatabaseConnection,
    provider: &str,
    delivery_id: &str,
    event_kind: Option<&str>,
    repo: Option<&str>,
    git_ref: Option<&str>,
    action: &str,
) -> bool {
    let model = webhook_deliveries::ActiveModel {
        provider: Set(provider.to_string()),
        delivery_id: Set(delivery_id.to_string()),
        event_kind: Set(event_kind.map(ToString::to_string)),
        repo: Set(repo.map(ToString::to_string)),
        git_ref: Set(git_ref.map(ToString::to_string)),
        action: Set(action.to_string()),
        ..Default::default()
    };
    let insert = webhook_deliveries::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                webhook_deliveries::Column::Provider,
                webhook_deliveries::Column::DeliveryId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .do_nothing()
        .exec(db)
        .await;
    match insert {
        Ok(sea_orm::TryInsertResult::Inserted(_)) => true,
        Ok(_) => false,
        Err(error) => {
            tracing::warn!("failed to record webhook delivery: {error}");
            // Fail open: treat as new so a DB outage cannot drop webhooks.
            true
        }
    }
}

/// Latest synced value hash for a (repo, secret) pair, to skip unchanged values.
pub async fn last_synced_sha256(
    db: &DatabaseConnection,
    repo: &str,
    secret_name: &str,
) -> Option<String> {
    gh_secret_sync_runs::Entity::find()
        .filter(gh_secret_sync_runs::Column::Repo.eq(repo))
        .filter(gh_secret_sync_runs::Column::SecretName.eq(secret_name))
        .filter(gh_secret_sync_runs::Column::Status.is_in(["synced", "skipped-unchanged"]))
        .order_by_desc(gh_secret_sync_runs::Column::SyncedAt)
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|row| row.value_sha256)
}

pub async fn record_secret_sync(
    db: &DatabaseConnection,
    repo: &str,
    secret_name: &str,
    value_sha256: &str,
    status: &str,
    detail: Option<&str>,
) {
    let model = gh_secret_sync_runs::ActiveModel {
        repo: Set(repo.to_string()),
        secret_name: Set(secret_name.to_string()),
        value_sha256: Set(value_sha256.to_string()),
        status: Set(status.to_string()),
        detail: Set(detail.map(ToString::to_string)),
        ..Default::default()
    };
    if let Err(error) = model.insert(db).await {
        tracing::warn!("failed to record gh secret sync run: {error}");
    }
}

pub async fn recent_secret_sync_runs(
    db: &DatabaseConnection,
    limit: u64,
) -> Vec<gh_secret_sync_runs::Model> {
    gh_secret_sync_runs::Entity::find()
        .order_by_desc(gh_secret_sync_runs::Column::SyncedAt)
        .limit(limit)
        .all(db)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!("failed to load secret sync runs: {error}");
            Vec::new()
        })
}
