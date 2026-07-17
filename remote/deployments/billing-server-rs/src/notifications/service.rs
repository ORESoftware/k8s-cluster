use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::shard::{Region, ShardKey};

use super::types::*;

#[derive(Clone)]
pub struct NotificationService {
    pool: PgPool,
}

impl NotificationService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create_rule(
        &self,
        tenant_id: Uuid,
        region: Region,
        input: CreateNotificationRule,
    ) -> AppResult<NotificationRule> {
        let shard = ShardKey::derive(tenant_id, region).0;

        let row = sqlx::query(
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
                      channel,
                      target, template_id, throttle_per_day, enabled, created_at
            "#,
        )
        .bind(tenant_id)
        .bind(shard)
        .bind(&input.kind)
        .bind(&input.name)
        .bind(&input.params)
        .bind(channel_tag(input.channel))
        .bind(&input.target)
        .bind(&input.template_id)
        .bind(input.throttle_per_day.max(0))
        .bind(input.enabled)
        .fetch_one(&self.pool)
        .await?;

        row_to_rule(&row)
    }

    pub async fn list_rules(&self, tenant_id: Uuid) -> AppResult<Vec<NotificationRule>> {
        let rows = sqlx::query(
            r#"
            SELECT id, tenant_id, kind, name, params,
                   channel,
                   target, template_id, throttle_per_day, enabled, created_at
            FROM notification_rules
            WHERE tenant_id = $1
            ORDER BY kind, name
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_rule).collect()
    }

    pub async fn list_dispatches(
        &self,
        tenant_id: Uuid,
        limit: i64,
    ) -> AppResult<Vec<NotificationDispatch>> {
        let rows = sqlx::query(
            r#"
            SELECT id, rule_id, tenant_id, target_resource,
                   channel,
                   target, payload,
                   status,
                   provider_message_id, error, sent_at, created_at
            FROM notification_dispatches
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_dispatch).collect()
    }

    /// Atomically enforce the daily throttle and insert a `pending` dispatch.
    /// A transaction-scoped advisory lock serializes every target for a rule,
    /// preventing concurrent evaluators from both passing a count-then-insert
    /// check. `None` means the rule is already at its daily limit.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_dispatch_unless_throttled(
        &self,
        rule_id: Uuid,
        tenant_id: Uuid,
        region: Region,
        target_resource: Option<&str>,
        channel: NotificationChannel,
        target: &str,
        payload: serde_json::Value,
        throttle_per_day: i32,
    ) -> AppResult<Option<i64>> {
        let shard = ShardKey::derive(tenant_id, region).0;
        let mut tx = self.pool.begin().await?;
        let lock_identity = format!("billing-notification-throttle:{rule_id}");
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(lock_identity)
            .execute(&mut *tx)
            .await?;

        if throttle_per_day > 0 {
            let count: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)::BIGINT FROM notification_dispatches
                WHERE rule_id = $1
                  AND ($2::TEXT IS NULL OR target_resource = $2)
                  AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  AND created_at <  (date_trunc('day', now() AT TIME ZONE 'UTC') + interval '1 day') AT TIME ZONE 'UTC'
                  AND status IN ('sent', 'pending', 'sending')
                "#,
            )
            .bind(rule_id)
            .bind(target_resource)
            .fetch_one(&mut *tx)
            .await?;
            if count >= i64::from(throttle_per_day) {
                tx.commit().await?;
                return Ok(None);
            }
        }

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO notification_dispatches
                (rule_id, tenant_id, shard_key, target_resource, channel, target, payload)
            VALUES ($1, $2, $3, $4, $5::notification_channel, $6, $7)
            RETURNING id
            "#,
        )
        .bind(rule_id)
        .bind(tenant_id)
        .bind(shard)
        .bind(target_resource)
        .bind(channel_tag(channel))
        .bind(target)
        .bind(&payload)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(id))
    }

    pub async fn mark_dispatch_sent(
        &self,
        id: i64,
        provider_message_id: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE notification_dispatches
            SET status = 'sent'::notification_dispatch_status,
                sent_at = now(),
                provider_message_id = $2
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(provider_message_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_dispatch_failed(&self, id: i64, error: &str) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE notification_dispatches
            SET status = 'failed'::notification_dispatch_status,
                error = $2
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
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

fn row_to_rule(row: &sqlx::postgres::PgRow) -> AppResult<NotificationRule> {
    Ok(NotificationRule {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        kind: row.try_get("kind")?,
        name: row.try_get("name")?,
        params: row.try_get("params")?,
        channel: row.try_get("channel")?,
        target: row.try_get("target")?,
        template_id: row.try_get("template_id")?,
        throttle_per_day: row.try_get("throttle_per_day")?,
        enabled: row.try_get("enabled")?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_dispatch(row: &sqlx::postgres::PgRow) -> AppResult<NotificationDispatch> {
    Ok(NotificationDispatch {
        id: row.try_get("id")?,
        rule_id: row.try_get("rule_id")?,
        tenant_id: row.try_get("tenant_id")?,
        target_resource: row.try_get("target_resource")?,
        channel: row.try_get("channel")?,
        target: row.try_get("target")?,
        payload: row.try_get("payload")?,
        status: row.try_get("status")?,
        provider_message_id: row.try_get("provider_message_id")?,
        error: row.try_get("error")?,
        sent_at: row.try_get("sent_at")?,
        created_at: row.try_get("created_at")?,
    })
}

// Silence unused-import warnings for symbols re-exported but not used elsewhere yet.
#[allow(dead_code)]
fn _unused(_: AppError) {}
