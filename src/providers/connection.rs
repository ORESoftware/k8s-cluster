//! Per-tenant provider connection storage.
//!
//! All secret credential material is sealed via [`crate::crypto::Sealer`]
//! before it touches the database. The plaintext shape inside the seal is
//! provider-specific (see each provider module for the corresponding
//! `Credential` struct).

use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QueryResult,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{SealedEnvelope, Sealer};
use crate::db::{decode_enum, require_row, stmt};
use crate::entity::provider_connections;
use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

use super::{ConnectionStatus, ProviderAuthKind, ProviderKind};

#[derive(Clone, Debug, Serialize)]
pub struct ProviderConnection {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub provider: ProviderKind,
    pub auth_kind: ProviderAuthKind,
    pub external_account_id: Option<String>,
    pub display_label: String,
    pub status: ConnectionStatus,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_sync_cursor: Option<String>,
    pub last_error: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// `(total, active, failing)` connection counts for a tenant or globally.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ConnectionCounts {
    pub total: i64,
    pub active: i64,
    pub failing: i64,
}

/// Newly-issued / freshly-refreshed credential material to seal and persist.
#[derive(Clone, Debug)]
pub struct UpsertCredential {
    pub plaintext: Vec<u8>,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateConnection {
    pub provider: ProviderKind,
    pub display_label: String,
    pub external_account_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone)]
pub struct ConnectionService {
    pool: DatabaseConnection,
    sealer: std::sync::Arc<Sealer>,
    events: std::sync::Arc<crate::events::EventBus>,
}

impl ConnectionService {
    pub fn new(
        pool: DatabaseConnection,
        sealer: std::sync::Arc<Sealer>,
        events: std::sync::Arc<crate::events::EventBus>,
    ) -> Self {
        Self {
            pool,
            sealer,
            events,
        }
    }

    /// Create a fresh pending connection (status = pending; no credential yet).
    pub async fn create(
        &self,
        tenant_id: Uuid,
        region: Region,
        input: CreateConnection,
    ) -> AppResult<ProviderConnection> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let auth_kind = input.provider.auth_kind();

        // Raw SQL (SeaORM Statement): INSERT with the `'pending'` enum
        // literal; the RETURNING enum columns gain `::TEXT` casts for
        // decoding (labels unchanged).
        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            INSERT INTO provider_connections
                (tenant_id, shard_key, provider, auth_kind, external_account_id,
                 display_label, status, metadata)
            VALUES ($1, $2, $3::provider_kind, $4::provider_auth_kind, $5, $6,
                    'pending'::connection_status, $7)
            RETURNING id, tenant_id, provider::TEXT AS provider,
                      auth_kind::TEXT AS auth_kind,
                      external_account_id, display_label,
                      status::TEXT AS status, scopes,
                      expires_at, refreshed_at, last_sync_at, last_sync_cursor, last_error,
                      metadata, created_at
            "#,
                    [
                        tenant_id.into(),
                        shard.into(),
                        input.provider.tag().into(),
                        auth_kind_tag(auth_kind).into(),
                        input.external_account_id.clone().into(),
                        input.display_label.clone().into(),
                        input.metadata.clone().into(),
                    ],
                ))
                .await?,
        )?;

        let conn = row_to_connection(&row)?;
        self.events
            .publish_connection_event(conn.tenant_id, conn.id, conn.provider.tag(), "created")
            .await;
        Ok(conn)
    }

    /// Seal + persist credential material for an existing connection and flip
    /// status to `active`. Used by OAuth callbacks and API-key upserts.
    pub async fn attach_credential(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        cred: UpsertCredential,
    ) -> AppResult<ProviderConnection> {
        let provider: ProviderKind = {
            let row = self
                .pool
                .query_one(stmt(
                    r#"SELECT provider::TEXT AS provider
               FROM provider_connections
               WHERE id = $1 AND tenant_id = $2"#,
                    [connection_id.into(), tenant_id.into()],
                ))
                .await?
                .ok_or_else(|| AppError::NotFound(format!("connection {connection_id}")))?;
            let label: String = row.try_get("", "provider")?;
            decode_enum("provider", &label)?
        };

        let envelope = self
            .sealer
            .seal(tenant_id, provider.tag(), &cred.plaintext)?;
        let sealed_json =
            serde_json::to_value(&envelope).map_err(|e| AppError::Other(anyhow::anyhow!(e)))?;

        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            UPDATE provider_connections
            SET sealed_credential = $3,
                scopes            = $4,
                expires_at        = $5,
                refreshed_at      = now(),
                status            = 'active'::connection_status,
                last_error        = NULL,
                updated_at        = now()
            WHERE id = $1 AND tenant_id = $2
            RETURNING id, tenant_id, provider::TEXT AS provider,
                      auth_kind::TEXT AS auth_kind,
                      external_account_id, display_label,
                      status::TEXT AS status, scopes,
                      expires_at, refreshed_at, last_sync_at, last_sync_cursor, last_error,
                      metadata, created_at
            "#,
                    [
                        connection_id.into(),
                        tenant_id.into(),
                        sealed_json.into(),
                        cred.scopes.clone().into(),
                        cred.expires_at.into(),
                    ],
                ))
                .await?,
        )?;

        let conn = row_to_connection(&row)?;
        self.events
            .publish_connection_event(conn.tenant_id, conn.id, conn.provider.tag(), "attached")
            .await;
        Ok(conn)
    }

    /// Decrypt the credential for an active connection. Returns plaintext bytes
    /// the caller must zeroize / drop quickly. Never log this.
    pub async fn load_credential(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
    ) -> AppResult<Vec<u8>> {
        let row = self
            .pool
            .query_one(stmt(
                r#"
            SELECT provider::TEXT AS provider, sealed_credential
            FROM provider_connections
            WHERE id = $1 AND tenant_id = $2
              AND status = 'active'::connection_status
            "#,
                [connection_id.into(), tenant_id.into()],
            ))
            .await?
            .ok_or_else(|| AppError::NotFound(format!("active connection {connection_id}")))?;

        let provider_label: String = row.try_get("", "provider")?;
        let provider: ProviderKind = decode_enum("provider", &provider_label)?;
        let sealed_json: Option<serde_json::Value> = row.try_get("", "sealed_credential")?;
        let sealed_json = sealed_json
            .ok_or_else(|| AppError::BadRequest("connection has no credential".into()))?;
        let envelope: SealedEnvelope = serde_json::from_value(sealed_json)
            .map_err(|e| AppError::Crypto(format!("envelope decode: {e}")))?;

        self.sealer.unseal(tenant_id, provider.tag(), &envelope)
    }

    /// `(total, active, failing)` for the admin dashboard.
    pub async fn counts(&self, tenant_id: Option<Uuid>) -> AppResult<ConnectionCounts> {
        // Raw SQL (SeaORM Statement): `COUNT(*) FILTER (WHERE ...)` aggregates.
        let row = match tenant_id {
            Some(tid) => require_row(
                self.pool
                    .query_one(stmt(
                        r#"
                    SELECT COUNT(*)                                              AS total,
                           COUNT(*) FILTER (WHERE status = 'active')             AS active,
                           COUNT(*) FILTER (WHERE status = 'token_refresh_failed'
                                                  OR last_error IS NOT NULL)    AS failing
                    FROM provider_connections
                    WHERE tenant_id = $1
                    "#,
                        [tid.into()],
                    ))
                    .await?,
            )?,
            None => require_row(
                self.pool
                    .query_one(stmt(
                        r#"
                    SELECT COUNT(*)                                              AS total,
                           COUNT(*) FILTER (WHERE status = 'active')             AS active,
                           COUNT(*) FILTER (WHERE status = 'token_refresh_failed'
                                                  OR last_error IS NOT NULL)    AS failing
                    FROM provider_connections
                    "#,
                        [],
                    ))
                    .await?,
            )?,
        };
        Ok(ConnectionCounts {
            total: row.try_get("", "total")?,
            active: row.try_get("", "active")?,
            failing: row.try_get("", "failing")?,
        })
    }

    pub async fn list_for_tenant(&self, tenant_id: Uuid) -> AppResult<Vec<ProviderConnection>> {
        let models = provider_connections::Entity::find()
            .filter(provider_connections::Column::TenantId.eq(tenant_id))
            .order_by_desc(provider_connections::Column::CreatedAt)
            .all(&self.pool)
            .await?;

        models.into_iter().map(model_to_connection).collect()
    }

    /// Mark a connection as token-refresh-failed.
    ///
    /// All callers must pass the `tenant_id` we expect this connection
    /// to belong to. The UPDATE is filtered by both `id` and
    /// `tenant_id` as defense in depth against a caller that learned a
    /// connection UUID through a side channel.
    ///
    /// Currently no caller invokes this — the OAuth refresh path that
    /// would set `token_refresh_failed` is not yet wired. Keep the
    /// method (with the tenant-scoped signature) so the refresh worker
    /// lands the right way from day one.
    #[allow(dead_code)]
    pub async fn mark_failed(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        error: &str,
    ) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"
            UPDATE provider_connections
            SET status = 'token_refresh_failed'::connection_status,
                last_error = $3,
                updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [connection_id.into(), tenant_id.into(), error.into()],
            ))
            .await?;
        Ok(())
    }

    pub async fn mark_sync_failed(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        error: &str,
    ) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"
            UPDATE provider_connections
            SET last_error = $3,
                updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [connection_id.into(), tenant_id.into(), error.into()],
            ))
            .await?;
        Ok(())
    }

    /// Shallow-merge new keys into `metadata`. Used by sync handlers to
    /// persist cursors (e.g. `stripe_balance_cursor`) and small bits of
    /// non-secret state. Never use this for secret material — that belongs
    /// in `sealed_credential`.
    pub async fn merge_metadata(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        patch: serde_json::Value,
    ) -> AppResult<()> {
        // Raw SQL (SeaORM Statement): jsonb `||` shallow-merge.
        self.pool
            .execute(stmt(
                r#"
            UPDATE provider_connections
            SET metadata = metadata || $3,
                updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [connection_id.into(), tenant_id.into(), patch.into()],
            ))
            .await?;
        Ok(())
    }

    /// Update the connection's `external_account_id` (set when an OAuth
    /// callback first reveals e.g. the Stripe `stripe_user_id`).
    ///
    /// This UPDATE is the most sensitive of the lot: changing
    /// `external_account_id` rebinds webhook routing
    /// (`find_active_by_external_account`). Tenant-scope it strictly.
    pub async fn set_external_account(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        external_account_id: &str,
    ) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"
            UPDATE provider_connections
            SET external_account_id = $3,
                updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [
                    connection_id.into(),
                    tenant_id.into(),
                    external_account_id.into(),
                ],
            ))
            .await?;
        Ok(())
    }

    /// Look up the (single) pending connection for a tenant + provider, or
    /// fall back to most-recently-created of any status. Used by the OAuth
    /// callback to attach freshly-issued credentials to the connection the
    /// user just started.
    pub async fn find_pending_for_oauth(
        &self,
        tenant_id: Uuid,
        provider: ProviderKind,
    ) -> AppResult<Option<ProviderConnection>> {
        // Raw SQL (SeaORM Statement): `ORDER BY (status = 'pending') DESC`
        // preference expression.
        let row = self
            .pool
            .query_one(stmt(
                r#"
            SELECT id, tenant_id, provider::TEXT AS provider,
                   auth_kind::TEXT AS auth_kind,
                   external_account_id, display_label,
                   status::TEXT AS status, scopes,
                   expires_at, refreshed_at, last_sync_at, last_sync_cursor, last_error,
                   metadata, created_at
            FROM provider_connections
            WHERE tenant_id = $1 AND provider = $2::provider_kind
            ORDER BY (status = 'pending'::connection_status) DESC,
                     created_at DESC
            LIMIT 1
            "#,
                [tenant_id.into(), provider.tag().into()],
            ))
            .await?;
        row.as_ref().map(row_to_connection).transpose()
    }

    pub async fn find_active_by_external_account(
        &self,
        provider: ProviderKind,
        external_account_id: &str,
    ) -> AppResult<Option<ProviderConnection>> {
        let row = self
            .pool
            .query_one(stmt(
                r#"
            SELECT id, tenant_id, provider::TEXT AS provider,
                   auth_kind::TEXT AS auth_kind,
                   external_account_id, display_label,
                   status::TEXT AS status, scopes,
                   expires_at, refreshed_at, last_sync_at, last_sync_cursor, last_error,
                   metadata, created_at
            FROM provider_connections
            WHERE provider = $1::provider_kind
              AND external_account_id = $2
              AND status = 'active'::connection_status
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
                [provider.tag().into(), external_account_id.into()],
            ))
            .await?;
        row.as_ref().map(row_to_connection).transpose()
    }

    pub async fn mark_synced(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        next_cursor: Option<&str>,
    ) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"
            UPDATE provider_connections
            SET last_sync_at = now(),
                last_sync_cursor = COALESCE($3, last_sync_cursor),
                last_error = NULL,
                updated_at = now()
            WHERE id = $1 AND tenant_id = $2
            "#,
                [connection_id.into(), tenant_id.into(), next_cursor.into()],
            ))
            .await?;
        Ok(())
    }

    pub async fn get(&self, tenant_id: Uuid, connection_id: Uuid) -> AppResult<ProviderConnection> {
        let model = provider_connections::Entity::find()
            .filter(provider_connections::Column::Id.eq(connection_id))
            .filter(provider_connections::Column::TenantId.eq(tenant_id))
            .one(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("connection {connection_id}")))?;
        model_to_connection(model)
    }
}

fn auth_kind_tag(k: ProviderAuthKind) -> &'static str {
    match k {
        ProviderAuthKind::OAuth2 => "oauth2",
        ProviderAuthKind::ApiKey => "api_key",
        ProviderAuthKind::BankCoordinates => "bank_coordinates",
        ProviderAuthKind::WalletPubkey => "wallet_pubkey",
    }
}

fn model_to_connection(m: provider_connections::Model) -> AppResult<ProviderConnection> {
    Ok(ProviderConnection {
        id: m.id,
        tenant_id: m.tenant_id,
        provider: decode_enum("provider", &m.provider)?,
        auth_kind: decode_enum("auth_kind", &m.auth_kind)?,
        external_account_id: m.external_account_id,
        display_label: m.display_label,
        status: decode_enum("status", &m.status)?,
        scopes: m.scopes,
        expires_at: m.expires_at.map(|t| t.with_timezone(&Utc)),
        refreshed_at: m.refreshed_at.map(|t| t.with_timezone(&Utc)),
        last_sync_at: m.last_sync_at.map(|t| t.with_timezone(&Utc)),
        last_sync_cursor: m.last_sync_cursor,
        last_error: m.last_error,
        metadata: m.metadata,
        created_at: m.created_at.with_timezone(&Utc),
    })
}

fn row_to_connection(row: &QueryResult) -> AppResult<ProviderConnection> {
    let provider_label: String = row.try_get("", "provider")?;
    let auth_kind_label: String = row.try_get("", "auth_kind")?;
    let status_label: String = row.try_get("", "status")?;
    Ok(ProviderConnection {
        id: row.try_get("", "id")?,
        tenant_id: row.try_get("", "tenant_id")?,
        provider: decode_enum("provider", &provider_label)?,
        auth_kind: decode_enum("auth_kind", &auth_kind_label)?,
        external_account_id: row.try_get("", "external_account_id")?,
        display_label: row.try_get("", "display_label")?,
        status: decode_enum("status", &status_label)?,
        scopes: row.try_get("", "scopes")?,
        expires_at: row.try_get("", "expires_at")?,
        refreshed_at: row.try_get("", "refreshed_at")?,
        last_sync_at: row.try_get("", "last_sync_at")?,
        last_sync_cursor: row.try_get("", "last_sync_cursor")?,
        last_error: row.try_get("", "last_error")?,
        metadata: row.try_get("", "metadata")?,
        created_at: row.try_get("", "created_at")?,
    })
}
