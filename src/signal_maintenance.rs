//! Bounded mailbox acknowledgement and deterministic Signal-state retention.
//!
//! Acknowledgement is recorded only after the client has decrypted, validated,
//! and atomically persisted an application mutation. The backend never treats
//! delivery as acceptance. Cleanup deletes only expired public material and
//! opaque mailbox rows; it never handles client private keys or plaintext.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait, Value,
};
use std::collections::HashSet;
use thiserror::Error;
use uuid::Uuid;

pub const MAX_SIGNAL_ACK_BATCH: usize = 250;
pub const ACKNOWLEDGED_MAIL_RETENTION_DAYS: i64 = 7;
pub const CLAIMED_PREKEY_RETENTION_DAYS: i64 = 30;
const _: () = assert!(
    ACKNOWLEDGED_MAIL_RETENTION_DAYS > 0
        && CLAIMED_PREKEY_RETENTION_DAYS >= ACKNOWLEDGED_MAIL_RETENTION_DAYS
);

const ACTIVE_RECIPIENT_SQL: &str = r#"
SELECT 1
FROM threefa.devices
WHERE account_id = $1
  AND id = $2
  AND revoked = false
  AND lifecycle_state = 'active'
FOR UPDATE
"#;

const ACKNOWLEDGE_ONE_SQL: &str = r#"
UPDATE threefa.device_mailbox
SET acknowledged_at = now()
WHERE account_id = $1
  AND recipient_device_id = $2
  AND envelope_id = $3
  AND acknowledged_at IS NULL
"#;

const CLEANUP_SQL: &str = r#"
WITH deleted_mail AS (
    DELETE FROM threefa.device_mailbox
    WHERE expires_at_ms <= $1
       OR (
            acknowledged_at IS NOT NULL
            AND acknowledged_at < clock_timestamp() - make_interval(days => $2::int)
       )
    RETURNING 1
), deleted_bundles AS (
    DELETE FROM threefa.device_prekey_bundles
    WHERE expires_at <= clock_timestamp()
    RETURNING 1
), deleted_claimed_prekeys AS (
    DELETE FROM threefa.device_one_time_prekeys
    WHERE claimed_at IS NOT NULL
      AND claimed_at < clock_timestamp() - make_interval(days => $3::int)
    RETURNING 1
)
SELECT
    (SELECT count(*)::bigint FROM deleted_mail) AS mailbox_rows,
    (SELECT count(*)::bigint FROM deleted_bundles) AS bundle_rows,
    (SELECT count(*)::bigint FROM deleted_claimed_prekeys) AS prekey_rows
"#;

#[derive(Debug, Error)]
pub enum SignalMaintenanceError {
    #[error(transparent)]
    Database(#[from] DbErr),
    #[error("acknowledgement batch is invalid")]
    InvalidBatch,
    #[error("recipient device is unavailable")]
    DeviceUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupCounts {
    pub mailbox_rows: u64,
    pub bundle_rows: u64,
    pub prekey_rows: u64,
}

fn postgres(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// Atomically acknowledge a bounded set of envelopes for one authenticated
/// recipient. Duplicate and unknown identifiers do not increase the count.
pub async fn acknowledge_batch(
    db: &DatabaseConnection,
    account_id: Uuid,
    recipient_device_id: Uuid,
    envelope_ids: &[Uuid],
) -> Result<u32, SignalMaintenanceError> {
    let envelope_ids = validate_ack_batch(envelope_ids)?;
    let transaction = db.begin().await?;

    let active = transaction
        .query_one_raw(postgres(
            ACTIVE_RECIPIENT_SQL,
            vec![account_id.into(), recipient_device_id.into()],
        ))
        .await?;
    if active.is_none() {
        return Err(SignalMaintenanceError::DeviceUnavailable);
    }

    let mut acknowledged = 0_u64;
    for envelope_id in envelope_ids {
        acknowledged += transaction
            .execute_raw(postgres(
                ACKNOWLEDGE_ONE_SQL,
                vec![
                    account_id.into(),
                    recipient_device_id.into(),
                    envelope_id.into(),
                ],
            ))
            .await?
            .rows_affected();
    }

    transaction.commit().await?;
    u32::try_from(acknowledged).map_err(|_| SignalMaintenanceError::InvalidBatch)
}

/// Delete expired public bundles, old claimed public one-time prekeys, expired
/// ciphertext, and acknowledged ciphertext past its bounded retry window.
pub async fn cleanup_expired_state(
    db: &DatabaseConnection,
    now_ms: i64,
) -> Result<CleanupCounts, SignalMaintenanceError> {
    if now_ms < 0 {
        return Err(SignalMaintenanceError::InvalidBatch);
    }
    let row = db
        .query_one_raw(postgres(
            CLEANUP_SQL,
            vec![
                now_ms.into(),
                ACKNOWLEDGED_MAIL_RETENTION_DAYS.into(),
                CLAIMED_PREKEY_RETENTION_DAYS.into(),
            ],
        ))
        .await?
        .ok_or(SignalMaintenanceError::InvalidBatch)?;

    Ok(CleanupCounts {
        mailbox_rows: to_u64(row.try_get("", "mailbox_rows")?)?,
        bundle_rows: to_u64(row.try_get("", "bundle_rows")?)?,
        prekey_rows: to_u64(row.try_get("", "prekey_rows")?)?,
    })
}

fn validate_ack_batch(envelope_ids: &[Uuid]) -> Result<Vec<Uuid>, SignalMaintenanceError> {
    if envelope_ids.is_empty() || envelope_ids.len() > MAX_SIGNAL_ACK_BATCH {
        return Err(SignalMaintenanceError::InvalidBatch);
    }
    let mut seen = HashSet::with_capacity(envelope_ids.len());
    Ok(envelope_ids
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect())
}

fn to_u64(value: i64) -> Result<u64, SignalMaintenanceError> {
    u64::try_from(value).map_err(|_| SignalMaintenanceError::InvalidBatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_batch_is_bounded_and_deduplicated() {
        assert!(matches!(
            validate_ack_batch(&[]),
            Err(SignalMaintenanceError::InvalidBatch)
        ));
        let id = Uuid::new_v4();
        assert_eq!(validate_ack_batch(&[id, id]).unwrap(), vec![id]);
        let too_many = vec![Uuid::nil(); MAX_SIGNAL_ACK_BATCH + 1];
        assert!(matches!(
            validate_ack_batch(&too_many),
            Err(SignalMaintenanceError::InvalidBatch)
        ));
    }

    #[test]
    fn acknowledgement_requires_active_recipient_and_unaccepted_rows() {
        assert!(ACTIVE_RECIPIENT_SQL.contains("revoked = false"));
        assert!(ACTIVE_RECIPIENT_SQL.contains("lifecycle_state = 'active'"));
        assert!(ACKNOWLEDGE_ONE_SQL.contains("acknowledged_at IS NULL"));
        assert!(ACKNOWLEDGE_ONE_SQL.contains("recipient_device_id = $2"));
    }

    #[test]
    fn cleanup_has_explicit_retention_windows() {
        assert!(CLEANUP_SQL.contains("expires_at_ms <= $1"));
        assert!(CLEANUP_SQL.contains("acknowledged_at IS NOT NULL"));
        assert!(CLEANUP_SQL.contains("claimed_at IS NOT NULL"));
    }
}
