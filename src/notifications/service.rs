use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use uuid::Uuid;

use crate::db::{decode_enum, require_row, stmt};
use crate::entity::{notification_dispatches, notification_rules};
use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

use super::types::*;

#[derive(Clone)]
pub struct NotificationService {
    pool: DatabaseConnection,
}

impl NotificationService {
    pub fn new(pool: DatabaseConnection) -> Self {
        Self { pool }
    }

    pub async fn create_rule(
        &self,
        tenant_id: Uuid,
        region: Region,
        input: CreateNotificationRule,
    ) -> AppResult<NotificationRule> {
        let shard = ShardKey::derive(tenant_id, region).0;

        // Raw SQL (SeaORM Statement): DO UPDATE SET ... = EXCLUDED.* with
        // `updated_at = now()`. Only change from the sqlx original: the
        // RETURNING `channel` column gains a `::TEXT` cast for decoding.
        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            INSERT INTO notification_rules
                (tenant_id, shard_key, kind, name, params, channel, target,
                 template_id, throttle_per_day, enabled)
            VALUES ($1, $2, $3, $4, $5, $6::notification_channel, $7, $8, $9, $10)
            ON CONFLICT (tenant_id, kind, name) DO UPDATE
                SET params           = EXCLUDED.params,
                    channel          = EXCLUDED.channel,
                    target           = EXCLUDED.target,
                    template_id      = EXCLUDED.template_id,
                    throttle_per_day = EXCLUDED.throttle_per_day,
                    enabled          = EXCLUDED.enabled,
                    updated_at       = now()
            RETURNING id, tenant_id, kind, name, params,
                      channel::TEXT AS channel,
                      target, template_id, throttle_per_day, enabled, created_at
            "#,
                    [
                        tenant_id.into(),
                        shard.into(),
                        input.kind.clone().into(),
                        input.name.clone().into(),
                        input.params.clone().into(),
                        channel_tag(input.channel).into(),
                        input.target.clone().into(),
                        input.template_id.clone().into(),
                        input.throttle_per_day.max(0).into(),
                        input.enabled.into(),
                    ],
                ))
                .await?,
        )?;

        let channel_label: String = row.try_get("", "channel")?;
        Ok(NotificationRule {
            id: row.try_get("", "id")?,
            tenant_id: row.try_get("", "tenant_id")?,
            kind: row.try_get("", "kind")?,
            name: row.try_get("", "name")?,
            params: row.try_get("", "params")?,
            channel: decode_enum("channel", &channel_label)?,
            target: row.try_get("", "target")?,
            template_id: row.try_get("", "template_id")?,
            throttle_per_day: row.try_get("", "throttle_per_day")?,
            enabled: row.try_get("", "enabled")?,
            created_at: row.try_get("", "created_at")?,
        })
    }

    pub async fn list_rules(&self, tenant_id: Uuid) -> AppResult<Vec<NotificationRule>> {
        let models = notification_rules::Entity::find()
            .filter(notification_rules::Column::TenantId.eq(tenant_id))
            .order_by_asc(notification_rules::Column::Kind)
            .order_by_asc(notification_rules::Column::Name)
            .all(&self.pool)
            .await?;

        models
            .into_iter()
            .map(|m| {
                Ok(NotificationRule {
                    id: m.id,
                    tenant_id: m.tenant_id,
                    kind: m.kind,
                    name: m.name,
                    params: m.params,
                    channel: decode_enum("channel", &m.channel)?,
                    target: m.target,
                    template_id: m.template_id,
                    throttle_per_day: m.throttle_per_day,
                    enabled: m.enabled,
                    created_at: m.created_at.with_timezone(&chrono::Utc),
                })
            })
            .collect()
    }

    pub async fn list_dispatches(
        &self,
        tenant_id: Uuid,
        limit: i64,
    ) -> AppResult<Vec<NotificationDispatch>> {
        let models = notification_dispatches::Entity::find()
            .filter(notification_dispatches::Column::TenantId.eq(tenant_id))
            .order_by_desc(notification_dispatches::Column::CreatedAt)
            .limit(limit.max(0) as u64)
            .all(&self.pool)
            .await?;

        models
            .into_iter()
            .map(|m| {
                Ok(NotificationDispatch {
                    id: m.id,
                    rule_id: m.rule_id,
                    tenant_id: m.tenant_id,
                    target_resource: m.target_resource,
                    channel: decode_enum("channel", &m.channel)?,
                    target: m.target,
                    payload: m.payload,
                    status: decode_enum("status", &m.status)?,
                    provider_message_id: m.provider_message_id,
                    error: m.error,
                    sent_at: m.sent_at.map(|t| t.with_timezone(&chrono::Utc)),
                    created_at: m.created_at.with_timezone(&chrono::Utc),
                })
            })
            .collect()
    }

    /// Insert a dispatch row in `pending` status. The actual send is the
    /// channel driver's job (called by the evaluator). Returns the row id.
    pub async fn create_dispatch(
        &self,
        rule_id: Uuid,
        tenant_id: Uuid,
        region: Region,
        target_resource: Option<&str>,
        channel: NotificationChannel,
        target: &str,
        payload: serde_json::Value,
    ) -> AppResult<i64> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            INSERT INTO notification_dispatches
                (rule_id, tenant_id, shard_key, target_resource, channel, target, payload)
            VALUES ($1, $2, $3, $4, $5::notification_channel, $6, $7)
            RETURNING id
            "#,
                    [
                        rule_id.into(),
                        tenant_id.into(),
                        shard.into(),
                        target_resource.into(),
                        channel_tag(channel).into(),
                        target.into(),
                        payload.into(),
                    ],
                ))
                .await?,
        )?;
        let id: i64 = row.try_get("", "id")?;
        Ok(id)
    }

    pub async fn mark_dispatch_sent(
        &self,
        id: i64,
        provider_message_id: Option<&str>,
    ) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"
            UPDATE notification_dispatches
            SET status = 'sent'::notification_dispatch_status,
                sent_at = now(),
                provider_message_id = $2
            WHERE id = $1
            "#,
                [id.into(), provider_message_id.into()],
            ))
            .await?;
        Ok(())
    }

    pub async fn mark_dispatch_failed(&self, id: i64, error: &str) -> AppResult<()> {
        self.pool
            .execute(stmt(
                r#"
            UPDATE notification_dispatches
            SET status = 'failed'::notification_dispatch_status,
                error = $2
            WHERE id = $1
            "#,
                [id.into(), error.into()],
            ))
            .await?;
        Ok(())
    }

    /// Throttle check — returns true if `(rule_id, target_resource)` has
    /// already produced >= `throttle_per_day` dispatches today.
    pub async fn would_throttle(
        &self,
        rule_id: Uuid,
        target_resource: Option<&str>,
        throttle_per_day: i32,
    ) -> AppResult<bool> {
        if throttle_per_day <= 0 {
            return Ok(false); // throttling disabled
        }
        let row = require_row(
            self.pool
                .query_one(stmt(
                    r#"
            SELECT COUNT(*)::BIGINT FROM notification_dispatches
            WHERE rule_id = $1
              AND ($2::TEXT IS NULL OR target_resource = $2)
              AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              AND created_at <  (date_trunc('day', now() AT TIME ZONE 'UTC') + interval '1 day') AT TIME ZONE 'UTC'
              AND status IN ('sent', 'pending', 'sending')
            "#,
                    [rule_id.into(), target_resource.into()],
                ))
                .await?,
        )?;
        let count: i64 = row.try_get_by_index(0)?;
        Ok(count >= throttle_per_day as i64)
    }
}

fn channel_tag(c: NotificationChannel) -> &'static str {
    match c {
        NotificationChannel::Email => "email",
        NotificationChannel::Webhook => "webhook",
        NotificationChannel::Slack => "slack",
        NotificationChannel::Sms => "sms",
    }
}

// Silence unused-import warnings for symbols re-exported but not used elsewhere yet.
#[allow(dead_code)]
fn _unused(_: AppError) {}
