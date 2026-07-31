//! Atomic sibling-device public-bundle retrieval for Signal Protocol session setup.
//!
//! The backend returns public identity/signed/PQ prekeys and, when available,
//! consumes exactly one public one-time prekey in the same PostgreSQL statement.
//! Private keys, ratchet state, vault keys, PINs, OTP codes, biometric templates,
//! recovery keys, and plaintext mutations are never represented here.

use crate::device_sync_protocol::{SignalDevicePreKeyBundle, SignalProtocolValidationError};
use crate::signal_store::SignalStoreError;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, Value};
use uuid::Uuid;

/// Ask a device to replenish before its public one-time-prekey pool reaches zero.
pub const LOW_PREKEY_THRESHOLD: i64 = 20;
const _: () = assert!(LOW_PREKEY_THRESHOLD > 0 && LOW_PREKEY_THRESHOLD < 1_000);

const CLAIM_BUNDLE_SQL: &str = r#"
WITH bundle AS (
    SELECT
        account.signal_device_revision,
        public_bundle.device_id,
        public_bundle.protocol_version,
        public_bundle.registration_id,
        public_bundle.identity_key_base64,
        public_bundle.signed_prekey_id,
        public_bundle.signed_prekey_base64,
        public_bundle.signed_prekey_signature_base64,
        public_bundle.pq_signed_prekey_id,
        public_bundle.pq_signed_prekey_base64,
        public_bundle.pq_signed_prekey_signature_base64
    FROM threefa.devices AS target
    JOIN threefa.accounts AS account
      ON account.id = target.account_id
    JOIN threefa.devices AS requester
      ON requester.id = $3
     AND requester.account_id = target.account_id
     AND requester.revoked = false
     AND requester.lifecycle_state = 'active'
    JOIN threefa.device_prekey_bundles AS public_bundle
      ON public_bundle.device_id = target.id
     AND public_bundle.expires_at > clock_timestamp()
    WHERE target.account_id = $1
      AND target.id = $2
      AND target.id <> requester.id
      AND target.revoked = false
      AND target.lifecycle_state = 'active'
    FOR UPDATE OF public_bundle
), candidate AS (
    SELECT prekey.device_id, prekey.prekey_id
    FROM threefa.device_one_time_prekeys AS prekey
    JOIN bundle ON bundle.device_id = prekey.device_id
    WHERE prekey.claimed_at IS NULL
      AND prekey.claimed_by_device_id IS NULL
    ORDER BY prekey.prekey_id
    FOR UPDATE OF prekey SKIP LOCKED
    LIMIT 1
), claimed AS (
    UPDATE threefa.device_one_time_prekeys AS prekey
    SET claimed_at = now(),
        claimed_by_device_id = $3
    FROM candidate
    WHERE prekey.device_id = candidate.device_id
      AND prekey.prekey_id = candidate.prekey_id
    RETURNING prekey.prekey_id, prekey.public_key_base64
), remaining_before AS (
    SELECT count(*)::bigint AS count
    FROM threefa.device_one_time_prekeys AS prekey
    JOIN bundle ON bundle.device_id = prekey.device_id
    WHERE prekey.claimed_at IS NULL
      AND prekey.claimed_by_device_id IS NULL
)
SELECT
    bundle.signal_device_revision,
    bundle.device_id,
    bundle.protocol_version,
    bundle.registration_id,
    bundle.identity_key_base64,
    bundle.signed_prekey_id,
    bundle.signed_prekey_base64,
    bundle.signed_prekey_signature_base64,
    bundle.pq_signed_prekey_id,
    bundle.pq_signed_prekey_base64,
    bundle.pq_signed_prekey_signature_base64,
    claimed.prekey_id AS one_time_prekey_id,
    claimed.public_key_base64 AS one_time_prekey_base64,
    remaining_before.count AS remaining_before_claim
FROM bundle
LEFT JOIN claimed ON true
CROSS JOIN remaining_before
"#;

const DEVICE_REVISION_SQL: &str = r#"
SELECT account.signal_device_revision
FROM threefa.accounts AS account
JOIN threefa.devices AS requester
  ON requester.account_id = account.id
 AND requester.id = $2
 AND requester.revoked = false
 AND requester.lifecycle_state = 'active'
WHERE account.id = $1
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedPreKeyBundle {
    pub device_revision: i64,
    pub bundle: SignalDevicePreKeyBundle,
    pub low_prekeys: bool,
}

fn postgres(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

pub async fn claim_prekey_bundle(
    db: &DatabaseConnection,
    account_id: Uuid,
    target_device_id: Uuid,
    requester_device_id: Uuid,
) -> Result<ClaimedPreKeyBundle, SignalStoreError> {
    if target_device_id == requester_device_id {
        return Err(SignalStoreError::DeviceUnavailable);
    }

    let row = db
        .query_one_raw(postgres(
            CLAIM_BUNDLE_SQL,
            vec![
                account_id.into(),
                target_device_id.into(),
                requester_device_id.into(),
            ],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;

    let protocol_version: i16 = row.try_get("", "protocol_version")?;
    let registration_id: i64 = row.try_get("", "registration_id")?;
    let signed_prekey_id: i64 = row.try_get("", "signed_prekey_id")?;
    let pq_signed_prekey_id: i64 = row.try_get("", "pq_signed_prekey_id")?;
    let one_time_prekey_id: Option<i64> = row.try_get("", "one_time_prekey_id")?;
    let one_time_prekey_base64: Option<String> = row.try_get("", "one_time_prekey_base64")?;

    if one_time_prekey_id.is_some() != one_time_prekey_base64.is_some() {
        return Err(SignalStoreError::Validation(
            SignalProtocolValidationError::IncompleteOneTimePreKey,
        ));
    }

    let bundle = SignalDevicePreKeyBundle {
        version: u16::try_from(protocol_version).map_err(|_| {
            SignalStoreError::Validation(SignalProtocolValidationError::UnsupportedVersion)
        })?,
        device_id: row.try_get::<Uuid>("", "device_id")?.to_string(),
        registration_id: to_u32(registration_id)?,
        identity_key: decode_required(&row, "identity_key_base64")?,
        signed_pre_key_id: to_u32(signed_prekey_id)?,
        signed_pre_key: decode_required(&row, "signed_prekey_base64")?,
        signed_pre_key_signature: decode_required(&row, "signed_prekey_signature_base64")?,
        pq_signed_pre_key_id: to_u32(pq_signed_prekey_id)?,
        pq_signed_pre_key: decode_required(&row, "pq_signed_prekey_base64")?,
        pq_signed_pre_key_signature: decode_required(&row, "pq_signed_prekey_signature_base64")?,
        one_time_pre_key_id: one_time_prekey_id.map(to_u32).transpose()?,
        one_time_pre_key: one_time_prekey_base64
            .map(|value| BASE64.decode(value))
            .transpose()?,
    };
    bundle.validate()?;

    let remaining_before_claim: i64 = row.try_get("", "remaining_before_claim")?;
    let claimed_count = if bundle.one_time_pre_key_id.is_some() {
        1
    } else {
        0
    };
    let remaining_after_claim = remaining_before_claim.saturating_sub(claimed_count);

    Ok(ClaimedPreKeyBundle {
        device_revision: row.try_get("", "signal_device_revision")?,
        bundle,
        low_prekeys: remaining_after_claim < LOW_PREKEY_THRESHOLD,
    })
}

pub async fn current_device_revision(
    db: &DatabaseConnection,
    account_id: Uuid,
    requester_device_id: Uuid,
) -> Result<i64, SignalStoreError> {
    let row = db
        .query_one_raw(postgres(
            DEVICE_REVISION_SQL,
            vec![account_id.into(), requester_device_id.into()],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    Ok(row.try_get("", "signal_device_revision")?)
}

fn decode_required(row: &sea_orm::QueryResult, column: &str) -> Result<Vec<u8>, SignalStoreError> {
    let encoded: String = row.try_get("", column)?;
    Ok(BASE64.decode(encoded)?)
}

fn to_u32(value: i64) -> Result<u32, SignalStoreError> {
    u32::try_from(value).map_err(|_| {
        SignalStoreError::Validation(SignalProtocolValidationError::InvalidPreKeyIdentifier)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_is_same_account_active_device_scoped_and_atomic() {
        assert!(CLAIM_BUNDLE_SQL.contains("requester.account_id = target.account_id"));
        assert!(CLAIM_BUNDLE_SQL.contains("target.lifecycle_state = 'active'"));
        assert!(CLAIM_BUNDLE_SQL.contains("requester.lifecycle_state = 'active'"));
        assert!(CLAIM_BUNDLE_SQL.contains("target.id <> requester.id"));
        assert!(CLAIM_BUNDLE_SQL.contains("FOR UPDATE OF prekey SKIP LOCKED"));
        assert!(CLAIM_BUNDLE_SQL.contains("claimed_by_device_id = $3"));
    }

    #[test]
    fn expired_public_bundles_are_not_returned() {
        assert!(CLAIM_BUNDLE_SQL.contains("public_bundle.expires_at > clock_timestamp()"));
    }

    #[test]
    fn revision_read_requires_an_active_authenticated_device() {
        assert!(DEVICE_REVISION_SQL.contains("requester.revoked = false"));
        assert!(DEVICE_REVISION_SQL.contains("requester.lifecycle_state = 'active'"));
    }
}
