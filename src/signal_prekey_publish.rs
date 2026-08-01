//! Transactional publication of public Signal prekey material.
//!
//! The device supplies a strictly increasing bundle revision. Exact retries of
//! the same revision are read-only and idempotent; stale revisions, conflicting
//! same-revision payloads, and reused one-time-prekey identifiers fail closed.
//! Only public key material is accepted here.

use crate::device_sync_protocol::{
    SignalDevicePreKeyBundle, SignalProtocolValidationError, MAX_SIGNAL_PUBLIC_KEY_LEN,
};
use crate::signal_store::SignalStoreError;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait, Value,
};
use std::collections::HashSet;
use uuid::Uuid;

const MAX_ONE_TIME_PREKEY_BATCH: usize = 100;

const ACTIVE_DEVICE_SQL: &str = r#"
SELECT 1
FROM threefa.devices
WHERE account_id = $1
  AND id = $2
  AND revoked = false
  AND lifecycle_state = 'active'
FOR UPDATE
"#;

const SELECT_BUNDLE_FOR_UPDATE_SQL: &str = r#"
SELECT bundle_revision, identity_key_base64
FROM threefa.device_prekey_bundles
WHERE device_id = $1
FOR UPDATE
"#;

const MATCH_BUNDLE_SQL: &str = r#"
SELECT 1
FROM threefa.device_prekey_bundles
WHERE device_id = $1
  AND bundle_revision = $2
  AND protocol_version = $3
  AND registration_id = $4
  AND identity_key_base64 = $5
  AND signed_prekey_id = $6
  AND signed_prekey_base64 = $7
  AND signed_prekey_signature_base64 = $8
  AND pq_signed_prekey_id = $9
  AND pq_signed_prekey_base64 = $10
  AND pq_signed_prekey_signature_base64 = $11
  AND expires_at = to_timestamp($12::double precision / 1000.0)
"#;

const INSERT_BUNDLE_SQL: &str = r#"
INSERT INTO threefa.device_prekey_bundles (
    device_id,
    bundle_revision,
    protocol_version,
    registration_id,
    identity_key_base64,
    signed_prekey_id,
    signed_prekey_base64,
    signed_prekey_signature_base64,
    pq_signed_prekey_id,
    pq_signed_prekey_base64,
    pq_signed_prekey_signature_base64,
    published_at,
    expires_at
) VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
    now(), to_timestamp($12::double precision / 1000.0)
)
RETURNING bundle_revision
"#;

const UPDATE_BUNDLE_SQL: &str = r#"
UPDATE threefa.device_prekey_bundles
SET bundle_revision = $2,
    protocol_version = $3,
    registration_id = $4,
    identity_key_base64 = $5,
    signed_prekey_id = $6,
    signed_prekey_base64 = $7,
    signed_prekey_signature_base64 = $8,
    pq_signed_prekey_id = $9,
    pq_signed_prekey_base64 = $10,
    pq_signed_prekey_signature_base64 = $11,
    published_at = now(),
    expires_at = to_timestamp($12::double precision / 1000.0)
WHERE device_id = $1
  AND bundle_revision < $2
RETURNING bundle_revision
"#;

const INSERT_FRESH_ONE_TIME_PREKEY_SQL: &str = r#"
INSERT INTO threefa.device_one_time_prekeys (
    device_id,
    prekey_id,
    public_key_base64
) VALUES ($1, $2, $3)
ON CONFLICT (device_id, prekey_id) DO NOTHING
"#;

const MATCH_ONE_TIME_PREKEY_SQL: &str = r#"
SELECT 1
FROM threefa.device_one_time_prekeys
WHERE device_id = $1
  AND prekey_id = $2
  AND public_key_base64 = $3
"#;

const COUNT_UNCLAIMED_PREKEYS_SQL: &str = r#"
SELECT count(*)::bigint AS unclaimed_prekey_count
FROM threefa.device_one_time_prekeys
WHERE device_id = $1
  AND claimed_at IS NULL
  AND claimed_by_device_id IS NULL
"#;

const CURRENT_DEVICE_REVISION_SQL: &str = r#"
SELECT signal_device_revision
FROM threefa.accounts
WHERE id = $1
FOR UPDATE
"#;

const BUMP_DEVICE_REVISION_SQL: &str = r#"
UPDATE threefa.accounts
SET signal_device_revision = signal_device_revision + 1
WHERE id = $1
RETURNING signal_device_revision
"#;

const INSERT_PREKEY_EVENT_SQL: &str = r#"
INSERT INTO threefa.device_security_events (
    account_id,
    actor_device_id,
    subject_device_id,
    device_revision,
    event_kind
) VALUES ($1, $2, $2, $3, $4)
"#;

#[derive(Clone, PartialEq, Eq)]
pub struct OneTimePreKey {
    pub prekey_id: u32,
    pub public_key: Vec<u8>,
}

#[derive(Clone)]
pub struct PublishPreKeys {
    pub account_id: Uuid,
    pub device_id: Uuid,
    pub bundle_revision: u64,
    pub bundle: SignalDevicePreKeyBundle,
    pub one_time_prekeys: Vec<OneTimePreKey>,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPreKeys {
    pub bundle_revision: u64,
    pub device_revision: u64,
    pub unclaimed_prekey_count: u32,
}

impl PublishPreKeys {
    fn validate(&self) -> Result<(), SignalStoreError> {
        self.bundle.validate()?;
        if self.bundle.device_id != self.device_id.to_string()
            || self.bundle_revision == 0
            || self.bundle_revision > i64::MAX as u64
            || self.expires_at_ms == 0
            || self.expires_at_ms > i64::MAX as u64
            || self.one_time_prekeys.len() > MAX_ONE_TIME_PREKEY_BATCH
            || self.bundle.one_time_pre_key_id.is_some()
            || self.bundle.one_time_pre_key.is_some()
        {
            return Err(SignalStoreError::Validation(
                SignalProtocolValidationError::InvalidPreKeyIdentifier,
            ));
        }

        let mut ids = HashSet::with_capacity(self.one_time_prekeys.len());
        for prekey in &self.one_time_prekeys {
            if !ids.insert(prekey.prekey_id) {
                return Err(SignalStoreError::IdempotencyConflict);
            }
            if prekey.public_key.len() < 32 || prekey.public_key.len() > MAX_SIGNAL_PUBLIC_KEY_LEN {
                return Err(SignalStoreError::Validation(
                    SignalProtocolValidationError::InvalidByteLength {
                        field: "one_time_prekey",
                    },
                ));
            }
        }
        Ok(())
    }
}

pub async fn publish_prekeys(
    db: &DatabaseConnection,
    request: PublishPreKeys,
) -> Result<PublishedPreKeys, SignalStoreError> {
    request.validate()?;
    let transaction = db.begin().await?;

    let active = transaction
        .query_one_raw(postgres(
            ACTIVE_DEVICE_SQL,
            vec![request.account_id.into(), request.device_id.into()],
        ))
        .await?;
    if active.is_none() {
        return Err(SignalStoreError::DeviceUnavailable);
    }

    let requested_revision =
        i64::try_from(request.bundle_revision).map_err(|_| SignalStoreError::RevisionConflict)?;
    let expires_at_ms = i64::try_from(request.expires_at_ms).map_err(|_| {
        SignalStoreError::Validation(SignalProtocolValidationError::InvalidTimestamps)
    })?;
    let existing = transaction
        .query_one_raw(postgres(
            SELECT_BUNDLE_FOR_UPDATE_SQL,
            vec![request.device_id.into()],
        ))
        .await?;

    let bundle_values = bundle_values(&request, requested_revision, expires_at_ms);
    let existing_identity = existing
        .as_ref()
        .map(|row| row.try_get::<String>("", "identity_key_base64"))
        .transpose()?;

    if let Some(row) = existing {
        let current_revision: i64 = row.try_get("", "bundle_revision")?;
        if requested_revision < current_revision {
            return Err(SignalStoreError::RevisionConflict);
        }
        if requested_revision == current_revision {
            ensure_exact_retry(&transaction, &request, bundle_values).await?;
            let device_revision = current_device_revision(&transaction, request.account_id).await?;
            let unclaimed_prekey_count =
                unclaimed_prekey_count(&transaction, request.device_id).await?;
            transaction.commit().await?;
            return Ok(PublishedPreKeys {
                bundle_revision: request.bundle_revision,
                device_revision,
                unclaimed_prekey_count,
            });
        }

        transaction
            .query_one_raw(postgres(UPDATE_BUNDLE_SQL, bundle_values))
            .await?
            .ok_or(SignalStoreError::RevisionConflict)?;
    } else {
        transaction
            .query_one_raw(postgres(INSERT_BUNDLE_SQL, bundle_values))
            .await?
            .ok_or(SignalStoreError::DeviceUnavailable)?;
    }

    for prekey in &request.one_time_prekeys {
        let result = transaction
            .execute_raw(postgres(
                INSERT_FRESH_ONE_TIME_PREKEY_SQL,
                vec![
                    request.device_id.into(),
                    i64::from(prekey.prekey_id).into(),
                    BASE64.encode(&prekey.public_key).into(),
                ],
            ))
            .await?;
        if result.rows_affected() != 1 {
            return Err(SignalStoreError::IdempotencyConflict);
        }
    }

    let device_revision = bump_device_revision(&transaction, request.account_id).await?;
    let unclaimed_prekey_count = unclaimed_prekey_count(&transaction, request.device_id).await?;
    let identity_changed = existing_identity
        .as_deref()
        .is_some_and(|stored| stored != BASE64.encode(&request.bundle.identity_key));
    let event_kind = if identity_changed {
        "identity_key_changed"
    } else if request.one_time_prekeys.is_empty() {
        "signed_prekey_rotated"
    } else {
        "prekeys_replenished"
    };
    transaction
        .execute_raw(postgres(
            INSERT_PREKEY_EVENT_SQL,
            vec![
                request.account_id.into(),
                request.device_id.into(),
                i64::try_from(device_revision)
                    .map_err(|_| SignalStoreError::RevisionConflict)?
                    .into(),
                event_kind.into(),
            ],
        ))
        .await?;

    transaction.commit().await?;
    Ok(PublishedPreKeys {
        bundle_revision: request.bundle_revision,
        device_revision,
        unclaimed_prekey_count,
    })
}

async fn ensure_exact_retry<C: ConnectionTrait>(
    connection: &C,
    request: &PublishPreKeys,
    bundle_values: Vec<Value>,
) -> Result<(), SignalStoreError> {
    if connection
        .query_one_raw(postgres(MATCH_BUNDLE_SQL, bundle_values))
        .await?
        .is_none()
    {
        return Err(SignalStoreError::IdempotencyConflict);
    }

    for prekey in &request.one_time_prekeys {
        let matched = connection
            .query_one_raw(postgres(
                MATCH_ONE_TIME_PREKEY_SQL,
                vec![
                    request.device_id.into(),
                    i64::from(prekey.prekey_id).into(),
                    BASE64.encode(&prekey.public_key).into(),
                ],
            ))
            .await?;
        if matched.is_none() {
            return Err(SignalStoreError::IdempotencyConflict);
        }
    }
    Ok(())
}

async fn current_device_revision<C: ConnectionTrait>(
    connection: &C,
    account_id: Uuid,
) -> Result<u64, SignalStoreError> {
    let row = connection
        .query_one_raw(postgres(
            CURRENT_DEVICE_REVISION_SQL,
            vec![account_id.into()],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    nonnegative_u64(row.try_get("", "signal_device_revision")?)
}

async fn bump_device_revision<C: ConnectionTrait>(
    connection: &C,
    account_id: Uuid,
) -> Result<u64, SignalStoreError> {
    let row = connection
        .query_one_raw(postgres(BUMP_DEVICE_REVISION_SQL, vec![account_id.into()]))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    nonnegative_u64(row.try_get("", "signal_device_revision")?)
}

async fn unclaimed_prekey_count<C: ConnectionTrait>(
    connection: &C,
    device_id: Uuid,
) -> Result<u32, SignalStoreError> {
    let row = connection
        .query_one_raw(postgres(
            COUNT_UNCLAIMED_PREKEYS_SQL,
            vec![device_id.into()],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    let count: i64 = row.try_get("", "unclaimed_prekey_count")?;
    u32::try_from(count).map_err(|_| {
        SignalStoreError::Validation(SignalProtocolValidationError::InvalidPreKeyIdentifier)
    })
}

fn bundle_values(
    request: &PublishPreKeys,
    requested_revision: i64,
    expires_at_ms: i64,
) -> Vec<Value> {
    let bundle = &request.bundle;
    vec![
        request.device_id.into(),
        requested_revision.into(),
        i64::from(bundle.version).into(),
        i64::from(bundle.registration_id).into(),
        BASE64.encode(&bundle.identity_key).into(),
        i64::from(bundle.signed_pre_key_id).into(),
        BASE64.encode(&bundle.signed_pre_key).into(),
        BASE64.encode(&bundle.signed_pre_key_signature).into(),
        i64::from(bundle.pq_signed_pre_key_id).into(),
        BASE64.encode(&bundle.pq_signed_pre_key).into(),
        BASE64.encode(&bundle.pq_signed_pre_key_signature).into(),
        expires_at_ms.into(),
    ]
}

fn nonnegative_u64(value: i64) -> Result<u64, SignalStoreError> {
    u64::try_from(value).map_err(|_| SignalStoreError::RevisionConflict)
}

fn postgres(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PublishPreKeys {
        PublishPreKeys {
            account_id: Uuid::new_v4(),
            device_id: Uuid::nil(),
            bundle_revision: 7,
            bundle: SignalDevicePreKeyBundle {
                version: 1,
                device_id: Uuid::nil().to_string(),
                registration_id: 42,
                identity_key: vec![1; 32],
                signed_pre_key_id: 10,
                signed_pre_key: vec![2; 32],
                signed_pre_key_signature: vec![3; 64],
                pq_signed_pre_key_id: 11,
                pq_signed_pre_key: vec![4; 32],
                pq_signed_pre_key_signature: vec![5; 64],
                one_time_pre_key_id: None,
                one_time_pre_key: None,
            },
            one_time_prekeys: vec![OneTimePreKey {
                prekey_id: 100,
                public_key: vec![6; 32],
            }],
            expires_at_ms: 2_000_000,
        }
    }

    #[test]
    fn publication_requires_unique_fresh_prekey_ids_and_bounded_revisions() {
        assert!(request().validate().is_ok());
        let mut duplicate = request();
        duplicate
            .one_time_prekeys
            .push(duplicate.one_time_prekeys[0].clone());
        assert!(matches!(
            duplicate.validate(),
            Err(SignalStoreError::IdempotencyConflict)
        ));
        let mut overflow = request();
        overflow.bundle_revision = u64::MAX;
        assert!(overflow.validate().is_err());
    }

    #[test]
    fn sql_contract_locks_revisions_and_fails_closed_on_reuse() {
        assert!(SELECT_BUNDLE_FOR_UPDATE_SQL.contains("FOR UPDATE"));
        assert!(UPDATE_BUNDLE_SQL.contains("bundle_revision < $2"));
        assert!(INSERT_FRESH_ONE_TIME_PREKEY_SQL.contains("ON CONFLICT"));
        assert!(MATCH_ONE_TIME_PREKEY_SQL.contains("public_key_base64 = $3"));
        assert!(COUNT_UNCLAIMED_PREKEYS_SQL.contains("claimed_at IS NULL"));
    }
}
