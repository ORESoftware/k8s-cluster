//! Transactional persistence for Signal Protocol public material and opaque mail.
//!
//! This module deliberately operates below the HTTP layer. It owns the database
//! transactions that make one-time prekey claims, envelope idempotency, mailbox
//! delivery, acknowledgement, and terminal device revocation atomic. It never
//! receives private keys, ratchet state, vault keys, PINs, OTP codes, biometric
//! templates, or plaintext vault mutations.

use crate::device_sync_protocol::{
    SignalCiphertextEnvelope, SignalDevicePreKeyBundle, SignalEnvelopeKind, SignalEnvelopeMetadata,
    SignalProtocolValidationError, MAX_SIGNAL_PUBLIC_KEY_LEN,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait, Value,
};
use thiserror::Error;
use uuid::Uuid;

const MAX_ONE_TIME_PREKEY_BATCH: usize = 1_000;
const MAX_MAILBOX_PULL: u64 = 200;

const ACTIVE_DEVICE_SQL: &str = r#"
SELECT 1
FROM threefa.devices
WHERE account_id = $1
  AND id = $2
  AND revoked = false
  AND lifecycle_state = 'active'
FOR UPDATE
"#;

const UPSERT_BUNDLE_SQL: &str = r#"
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
    $1, 1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
    now(), to_timestamp($11::double precision / 1000.0)
)
ON CONFLICT (device_id) DO UPDATE SET
    bundle_revision = threefa.device_prekey_bundles.bundle_revision + 1,
    protocol_version = EXCLUDED.protocol_version,
    registration_id = EXCLUDED.registration_id,
    identity_key_base64 = EXCLUDED.identity_key_base64,
    signed_prekey_id = EXCLUDED.signed_prekey_id,
    signed_prekey_base64 = EXCLUDED.signed_prekey_base64,
    signed_prekey_signature_base64 = EXCLUDED.signed_prekey_signature_base64,
    pq_signed_prekey_id = EXCLUDED.pq_signed_prekey_id,
    pq_signed_prekey_base64 = EXCLUDED.pq_signed_prekey_base64,
    pq_signed_prekey_signature_base64 = EXCLUDED.pq_signed_prekey_signature_base64,
    published_at = now(),
    expires_at = EXCLUDED.expires_at
RETURNING bundle_revision
"#;

const INSERT_ONE_TIME_PREKEY_SQL: &str = r#"
INSERT INTO threefa.device_one_time_prekeys (
    device_id,
    prekey_id,
    public_key_base64
) VALUES ($1, $2, $3)
ON CONFLICT (device_id, prekey_id) DO NOTHING
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

const CLAIM_ONE_TIME_PREKEY_SQL: &str = r#"
WITH eligible AS (
    SELECT prekey.device_id, prekey.prekey_id
    FROM threefa.device_one_time_prekeys AS prekey
    JOIN threefa.devices AS target
      ON target.id = prekey.device_id
     AND target.account_id = $1
     AND target.revoked = false
     AND target.lifecycle_state = 'active'
    JOIN threefa.devices AS requester
      ON requester.id = $3
     AND requester.account_id = $1
     AND requester.revoked = false
     AND requester.lifecycle_state = 'active'
    WHERE prekey.device_id = $2
      AND prekey.claimed_at IS NULL
      AND prekey.claimed_by_device_id IS NULL
      AND prekey.device_id <> $3
    ORDER BY prekey.prekey_id
    FOR UPDATE OF prekey SKIP LOCKED
    LIMIT 1
)
UPDATE threefa.device_one_time_prekeys AS prekey
SET claimed_at = now(),
    claimed_by_device_id = $3
FROM eligible
WHERE prekey.device_id = eligible.device_id
  AND prekey.prekey_id = eligible.prekey_id
RETURNING prekey.prekey_id, prekey.public_key_base64
"#;

const ENQUEUE_ENVELOPE_SQL: &str = r#"
INSERT INTO threefa.device_mailbox (
    envelope_id,
    account_id,
    sender_device_id,
    recipient_device_id,
    protocol_version,
    session_id,
    message_number,
    kind,
    client_created_at_ms,
    expires_at_ms,
    ciphertext_base64,
    ciphertext_size_bytes
)
SELECT
    $4, $1, $2, $3, $5, $6, $7::numeric, $8, $9, $10, $11, $12
FROM threefa.devices AS sender
JOIN threefa.devices AS recipient
  ON recipient.id = $3
 AND recipient.account_id = $1
 AND recipient.revoked = false
 AND recipient.lifecycle_state = 'active'
WHERE sender.id = $2
  AND sender.account_id = $1
  AND sender.revoked = false
  AND sender.lifecycle_state = 'active'
ON CONFLICT (envelope_id) DO NOTHING
RETURNING mailbox_seq
"#;

const MATCH_ENVELOPE_SQL: &str = r#"
SELECT mailbox_seq
FROM threefa.device_mailbox
WHERE envelope_id = $4
  AND account_id = $1
  AND sender_device_id = $2
  AND recipient_device_id = $3
  AND protocol_version = $5
  AND session_id = $6
  AND message_number = $7::numeric
  AND kind = $8
  AND client_created_at_ms = $9
  AND expires_at_ms = $10
  AND ciphertext_base64 = $11
  AND ciphertext_size_bytes = $12
"#;

const PULL_MAILBOX_SQL: &str = r#"
WITH selected AS (
    SELECT mailbox.envelope_id
    FROM threefa.device_mailbox AS mailbox
    JOIN threefa.devices AS recipient
      ON recipient.id = mailbox.recipient_device_id
     AND recipient.account_id = $1
     AND recipient.revoked = false
     AND recipient.lifecycle_state = 'active'
    WHERE mailbox.account_id = $1
      AND mailbox.recipient_device_id = $2
      AND mailbox.mailbox_seq > $3
      AND mailbox.acknowledged_at IS NULL
      AND mailbox.expires_at_ms > (extract(epoch FROM clock_timestamp()) * 1000)::bigint
    ORDER BY mailbox.mailbox_seq
    FOR UPDATE OF mailbox
    LIMIT $4
), delivered AS (
    UPDATE threefa.device_mailbox AS mailbox
    SET delivered_at = COALESCE(mailbox.delivered_at, now())
    FROM selected
    WHERE mailbox.envelope_id = selected.envelope_id
    RETURNING
        mailbox.mailbox_seq,
        mailbox.envelope_id,
        mailbox.account_id,
        mailbox.sender_device_id,
        mailbox.recipient_device_id,
        mailbox.protocol_version,
        mailbox.session_id,
        mailbox.message_number::text AS message_number_text,
        mailbox.kind,
        mailbox.client_created_at_ms,
        mailbox.expires_at_ms,
        mailbox.ciphertext_base64
)
SELECT *
FROM delivered
ORDER BY mailbox_seq
"#;

const ACKNOWLEDGE_ENVELOPE_SQL: &str = r#"
UPDATE threefa.device_mailbox AS mailbox
SET acknowledged_at = COALESCE(mailbox.acknowledged_at, now())
FROM threefa.devices AS recipient
WHERE mailbox.envelope_id = $3
  AND mailbox.account_id = $1
  AND mailbox.recipient_device_id = $2
  AND recipient.id = $2
  AND recipient.account_id = $1
  AND recipient.revoked = false
  AND recipient.lifecycle_state = 'active'
RETURNING mailbox.mailbox_seq
"#;

const CHECK_ACTOR_SQL: &str = r#"
SELECT 1
FROM threefa.devices
WHERE account_id = $1
  AND id = $2
  AND revoked = false
  AND lifecycle_state = 'active'
FOR UPDATE
"#;

const LOCK_DEVICE_STATE_SQL: &str = r#"
SELECT revoked, lifecycle_state
FROM threefa.devices
WHERE account_id = $1
  AND id = $2
FOR UPDATE
"#;

const CURRENT_DEVICE_REVISION_SQL: &str = r#"
SELECT signal_device_revision
FROM threefa.accounts
WHERE id = $1
FOR UPDATE
"#;

const CAS_DEVICE_REVISION_SQL: &str = r#"
UPDATE threefa.accounts
SET signal_device_revision = signal_device_revision + 1
WHERE id = $1
  AND signal_device_revision = $2
RETURNING signal_device_revision
"#;

const REVOKE_DEVICE_SQL: &str = r#"
UPDATE threefa.devices
SET revoked = true,
    lifecycle_state = 'revoked',
    revoked_at = COALESCE(revoked_at, now()),
    suspended_at = NULL
WHERE account_id = $1
  AND id = $2
  AND lifecycle_state <> 'revoked'
RETURNING id
"#;

const DELETE_REVOKED_PREKEYS_SQL: &str = r#"
WITH deleted_bundles AS (
    DELETE FROM threefa.device_prekey_bundles
    WHERE device_id = $1
    RETURNING device_id
)
DELETE FROM threefa.device_one_time_prekeys
WHERE device_id = $1
"#;

const DELETE_REVOKED_MAIL_SQL: &str = r#"
DELETE FROM threefa.device_mailbox
WHERE account_id = $1
  AND acknowledged_at IS NULL
  AND (sender_device_id = $2 OR recipient_device_id = $2)
"#;

const INSERT_REVOCATION_EVENT_SQL: &str = r#"
INSERT INTO threefa.device_security_events (
    account_id,
    actor_device_id,
    subject_device_id,
    device_revision,
    event_kind
) VALUES ($1, $2, $3, $4, 'device_revoked')
"#;

#[derive(Debug, Error)]
pub enum SignalStoreError {
    #[error(transparent)]
    Validation(#[from] SignalProtocolValidationError),
    #[error(transparent)]
    Database(#[from] DbErr),
    #[error("{0} is not a UUID")]
    InvalidUuid(&'static str),
    #[error("device is unavailable")]
    DeviceUnavailable,
    #[error("recipient device is unavailable")]
    RecipientUnavailable,
    #[error("device-list revision conflict")]
    RevisionConflict,
    #[error("envelope idempotency conflict")]
    IdempotencyConflict,
    #[error("stored ciphertext is not valid base64")]
    InvalidStoredCiphertext(#[from] base64::DecodeError),
    #[error("stored message number is invalid")]
    InvalidStoredMessageNumber(#[from] std::num::ParseIntError),
    #[error("stored envelope kind is invalid")]
    InvalidStoredKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneTimePreKey {
    pub prekey_id: u32,
    pub public_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PublishPreKeys {
    pub account_id: Uuid,
    pub device_id: Uuid,
    pub bundle: SignalDevicePreKeyBundle,
    pub one_time_prekeys: Vec<OneTimePreKey>,
    pub expires_at_ms: i64,
}

impl PublishPreKeys {
    fn validate(&self) -> Result<(), SignalStoreError> {
        self.bundle.validate()?;
        if self.bundle.device_id != self.device_id.to_string() {
            return Err(SignalStoreError::DeviceUnavailable);
        }
        if self.expires_at_ms <= 0 || self.one_time_prekeys.len() > MAX_ONE_TIME_PREKEY_BATCH {
            return Err(SignalStoreError::Validation(
                SignalProtocolValidationError::InvalidTimestamps,
            ));
        }
        for prekey in &self.one_time_prekeys {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPreKeys {
    pub bundle_revision: i64,
    pub device_list_revision: i64,
    pub inserted_one_time_prekeys: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedOneTimePreKey {
    pub prekey_id: u32,
    pub public_key: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueResult {
    pub mailbox_seq: i64,
    pub inserted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxEnvelope {
    pub mailbox_seq: i64,
    pub envelope: SignalCiphertextEnvelope,
}

fn postgres(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn required_i64(row: &sea_orm::QueryResult, column: &str) -> Result<i64, SignalStoreError> {
    Ok(row.try_get("", column)?)
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

    let bundle = &request.bundle;
    let row = transaction
        .query_one_raw(postgres(
            UPSERT_BUNDLE_SQL,
            vec![
                request.device_id.into(),
                i64::from(bundle.version).into(),
                i64::from(bundle.registration_id).into(),
                BASE64.encode(&bundle.identity_key).into(),
                i64::from(bundle.signed_pre_key_id).into(),
                BASE64.encode(&bundle.signed_pre_key).into(),
                BASE64.encode(&bundle.signed_pre_key_signature).into(),
                i64::from(bundle.pq_signed_pre_key_id).into(),
                BASE64.encode(&bundle.pq_signed_pre_key).into(),
                BASE64.encode(&bundle.pq_signed_pre_key_signature).into(),
                request.expires_at_ms.into(),
            ],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    let bundle_revision = required_i64(&row, "bundle_revision")?;

    let mut inserted_one_time_prekeys = 0_u64;
    let mut all_prekeys = request.one_time_prekeys;
    if let (Some(prekey_id), Some(public_key)) =
        (bundle.one_time_pre_key_id, bundle.one_time_pre_key.clone())
    {
        all_prekeys.push(OneTimePreKey {
            prekey_id,
            public_key,
        });
    }
    for prekey in all_prekeys {
        let result = transaction
            .execute_raw(postgres(
                INSERT_ONE_TIME_PREKEY_SQL,
                vec![
                    request.device_id.into(),
                    i64::from(prekey.prekey_id).into(),
                    BASE64.encode(prekey.public_key).into(),
                ],
            ))
            .await?;
        inserted_one_time_prekeys += result.rows_affected();
    }

    let revision_row = transaction
        .query_one_raw(postgres(
            BUMP_DEVICE_REVISION_SQL,
            vec![request.account_id.into()],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    let device_list_revision = required_i64(&revision_row, "signal_device_revision")?;
    let event_kind = if inserted_one_time_prekeys > 0 {
        "prekeys_replenished"
    } else {
        "signed_prekey_rotated"
    };
    transaction
        .execute_raw(postgres(
            INSERT_PREKEY_EVENT_SQL,
            vec![
                request.account_id.into(),
                request.device_id.into(),
                device_list_revision.into(),
                event_kind.into(),
            ],
        ))
        .await?;

    transaction.commit().await?;
    Ok(PublishedPreKeys {
        bundle_revision,
        device_list_revision,
        inserted_one_time_prekeys,
    })
}

pub async fn claim_one_time_prekey(
    db: &DatabaseConnection,
    account_id: Uuid,
    target_device_id: Uuid,
    requester_device_id: Uuid,
) -> Result<Option<ClaimedOneTimePreKey>, SignalStoreError> {
    if target_device_id == requester_device_id {
        return Err(SignalStoreError::DeviceUnavailable);
    }
    let transaction = db.begin().await?;
    let row = transaction
        .query_one_raw(postgres(
            CLAIM_ONE_TIME_PREKEY_SQL,
            vec![
                account_id.into(),
                target_device_id.into(),
                requester_device_id.into(),
            ],
        ))
        .await?;
    let claimed = row
        .map(|row| -> Result<ClaimedOneTimePreKey, SignalStoreError> {
            let prekey_id: i64 = row.try_get("", "prekey_id")?;
            let public_key_base64: String = row.try_get("", "public_key_base64")?;
            let prekey_id = u32::try_from(prekey_id).map_err(|_| {
                SignalStoreError::Validation(SignalProtocolValidationError::InvalidPreKeyIdentifier)
            })?;
            Ok(ClaimedOneTimePreKey {
                prekey_id,
                public_key: BASE64.decode(public_key_base64)?,
            })
        })
        .transpose()?;
    transaction.commit().await?;
    Ok(claimed)
}

pub async fn enqueue_envelope(
    db: &DatabaseConnection,
    envelope: &SignalCiphertextEnvelope,
) -> Result<EnqueueResult, SignalStoreError> {
    envelope.validate()?;
    let metadata = &envelope.metadata;
    let account_id = parse_uuid(&metadata.account_id, "account_id")?;
    let sender_device_id = parse_uuid(&metadata.sender_device_id, "sender_device_id")?;
    let recipient_device_id = parse_uuid(&metadata.recipient_device_id, "recipient_device_id")?;
    let envelope_id = parse_uuid(&metadata.envelope_id, "envelope_id")?;
    let ciphertext_base64 = BASE64.encode(&envelope.ciphertext);

    let values = vec![
        account_id.into(),
        sender_device_id.into(),
        recipient_device_id.into(),
        envelope_id.into(),
        i64::from(metadata.version).into(),
        metadata.session_id.clone().into(),
        metadata.message_number.to_string().into(),
        kind_wire_name(metadata.kind).into(),
        metadata.created_at_ms.into(),
        metadata.expires_at_ms.into(),
        ciphertext_base64.into(),
        i64::try_from(envelope.ciphertext.len())
            .map_err(|_| {
                SignalStoreError::Validation(SignalProtocolValidationError::InvalidCiphertextLength)
            })?
            .into(),
    ];
    if let Some(row) = db
        .query_one_raw(postgres(ENQUEUE_ENVELOPE_SQL, values.clone()))
        .await?
    {
        return Ok(EnqueueResult {
            mailbox_seq: required_i64(&row, "mailbox_seq")?,
            inserted: true,
        });
    }

    // A filtered INSERT has two distinct terminal outcomes: the recipient is
    // no longer active, or the envelope id already belongs to another payload.
    // Keep them distinct so clients stop retrying a revoked recipient but may
    // safely replace an id that genuinely collided.
    if db
        .query_one_raw(postgres(
            ACTIVE_DEVICE_SQL,
            vec![account_id.into(), recipient_device_id.into()],
        ))
        .await?
        .is_none()
    {
        return Err(SignalStoreError::RecipientUnavailable);
    }
    if db
        .query_one_raw(postgres(
            ACTIVE_DEVICE_SQL,
            vec![account_id.into(), sender_device_id.into()],
        ))
        .await?
        .is_none()
    {
        return Err(SignalStoreError::DeviceUnavailable);
    }

    // PostgreSQL may detect a concurrently committed ON CONFLICT row that was
    // invisible to the INSERT statement's initial READ COMMITTED snapshot.
    // Match in a fresh statement so an exact concurrent retry remains
    // idempotent; a missing or non-identical row is a genuine id collision.
    let row = db
        .query_one_raw(postgres(MATCH_ENVELOPE_SQL, values))
        .await?
        .ok_or(SignalStoreError::IdempotencyConflict)?;
    Ok(EnqueueResult {
        mailbox_seq: required_i64(&row, "mailbox_seq")?,
        inserted: false,
    })
}

pub async fn pull_mailbox(
    db: &DatabaseConnection,
    account_id: Uuid,
    recipient_device_id: Uuid,
    after_mailbox_seq: i64,
    limit: u64,
) -> Result<Vec<MailboxEnvelope>, SignalStoreError> {
    let limit = limit.clamp(1, MAX_MAILBOX_PULL);
    let rows = db
        .query_all_raw(postgres(
            PULL_MAILBOX_SQL,
            vec![
                account_id.into(),
                recipient_device_id.into(),
                after_mailbox_seq.max(0).into(),
                i64::try_from(limit).expect("mailbox limit fits i64").into(),
            ],
        ))
        .await?;

    rows.into_iter().map(mailbox_row).collect()
}

pub async fn acknowledge_envelope(
    db: &DatabaseConnection,
    account_id: Uuid,
    recipient_device_id: Uuid,
    envelope_id: Uuid,
) -> Result<i64, SignalStoreError> {
    let row = db
        .query_one_raw(postgres(
            ACKNOWLEDGE_ENVELOPE_SQL,
            vec![
                account_id.into(),
                recipient_device_id.into(),
                envelope_id.into(),
            ],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    required_i64(&row, "mailbox_seq")
}

pub async fn revoke_device(
    db: &DatabaseConnection,
    account_id: Uuid,
    actor_device_id: Uuid,
    subject_device_id: Uuid,
    expected_revision: Option<i64>,
) -> Result<i64, SignalStoreError> {
    if expected_revision.is_some_and(|revision| revision < 0) {
        return Err(SignalStoreError::RevisionConflict);
    }
    let transaction = db.begin().await?;
    let actor = transaction
        .query_one_raw(postgres(
            CHECK_ACTOR_SQL,
            vec![account_id.into(), actor_device_id.into()],
        ))
        .await?;
    if actor.is_none() {
        return Err(SignalStoreError::DeviceUnavailable);
    }

    let subject = transaction
        .query_one_raw(postgres(
            LOCK_DEVICE_STATE_SQL,
            vec![account_id.into(), subject_device_id.into()],
        ))
        .await?
        .ok_or(SignalStoreError::DeviceUnavailable)?;
    let already_revoked = subject.try_get::<bool>("", "revoked")?
        && subject.try_get::<String>("", "lifecycle_state")? == "revoked";
    if already_revoked {
        let revision = transaction
            .query_one_raw(postgres(
                CURRENT_DEVICE_REVISION_SQL,
                vec![account_id.into()],
            ))
            .await?
            .ok_or(SignalStoreError::RevisionConflict)?;
        let revision = required_i64(&revision, "signal_device_revision")?;
        transaction.commit().await?;
        return Ok(revision);
    }

    let revision = match expected_revision {
        Some(expected_revision) => transaction
            .query_one_raw(postgres(
                CAS_DEVICE_REVISION_SQL,
                vec![account_id.into(), expected_revision.into()],
            ))
            .await?
            .ok_or(SignalStoreError::RevisionConflict)?,
        None => transaction
            .query_one_raw(postgres(BUMP_DEVICE_REVISION_SQL, vec![account_id.into()]))
            .await?
            .ok_or(SignalStoreError::RevisionConflict)?,
    };
    let new_revision = required_i64(&revision, "signal_device_revision")?;

    let revoked = transaction
        .query_one_raw(postgres(
            REVOKE_DEVICE_SQL,
            vec![account_id.into(), subject_device_id.into()],
        ))
        .await?;
    if revoked.is_none() {
        return Err(SignalStoreError::DeviceUnavailable);
    }

    transaction
        .execute_raw(postgres(
            DELETE_REVOKED_PREKEYS_SQL,
            vec![subject_device_id.into()],
        ))
        .await?;
    transaction
        .execute_raw(postgres(
            DELETE_REVOKED_MAIL_SQL,
            vec![account_id.into(), subject_device_id.into()],
        ))
        .await?;
    transaction
        .execute_raw(postgres(
            INSERT_REVOCATION_EVENT_SQL,
            vec![
                account_id.into(),
                actor_device_id.into(),
                subject_device_id.into(),
                new_revision.into(),
            ],
        ))
        .await?;
    transaction.commit().await?;
    Ok(new_revision)
}

fn mailbox_row(row: sea_orm::QueryResult) -> Result<MailboxEnvelope, SignalStoreError> {
    let kind: String = row.try_get("", "kind")?;
    let metadata = SignalEnvelopeMetadata {
        version: u16::try_from(row.try_get::<i16>("", "protocol_version")?).map_err(|_| {
            SignalStoreError::Validation(SignalProtocolValidationError::UnsupportedVersion)
        })?,
        envelope_id: row.try_get::<Uuid>("", "envelope_id")?.to_string(),
        account_id: row.try_get::<Uuid>("", "account_id")?.to_string(),
        sender_device_id: row.try_get::<Uuid>("", "sender_device_id")?.to_string(),
        recipient_device_id: row.try_get::<Uuid>("", "recipient_device_id")?.to_string(),
        session_id: row.try_get("", "session_id")?,
        message_number: row.try_get::<String>("", "message_number_text")?.parse()?,
        kind: parse_kind(&kind)?,
        created_at_ms: row.try_get("", "client_created_at_ms")?,
        expires_at_ms: row.try_get("", "expires_at_ms")?,
    };
    let ciphertext_base64: String = row.try_get("", "ciphertext_base64")?;
    let envelope = SignalCiphertextEnvelope {
        metadata,
        ciphertext: BASE64.decode(ciphertext_base64)?,
    };
    envelope.validate()?;
    Ok(MailboxEnvelope {
        mailbox_seq: row.try_get("", "mailbox_seq")?,
        envelope,
    })
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, SignalStoreError> {
    Uuid::parse_str(value).map_err(|_| SignalStoreError::InvalidUuid(field))
}

fn kind_wire_name(kind: SignalEnvelopeKind) -> &'static str {
    match kind {
        SignalEnvelopeKind::VaultSnapshot => "vault_snapshot",
        SignalEnvelopeKind::VaultMutation => "vault_mutation",
        SignalEnvelopeKind::VaultKeyTransfer => "vault_key_transfer",
        SignalEnvelopeKind::DeviceControl => "device_control",
    }
}

fn parse_kind(value: &str) -> Result<SignalEnvelopeKind, SignalStoreError> {
    match value {
        "vault_snapshot" => Ok(SignalEnvelopeKind::VaultSnapshot),
        "vault_mutation" => Ok(SignalEnvelopeKind::VaultMutation),
        "vault_key_transfer" => Ok(SignalEnvelopeKind::VaultKeyTransfer),
        "device_control" => Ok(SignalEnvelopeKind::DeviceControl),
        _ => Err(SignalStoreError::InvalidStoredKind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_time_prekey_claim_is_atomic_and_nonblocking() {
        assert!(CLAIM_ONE_TIME_PREKEY_SQL.contains("FOR UPDATE OF prekey SKIP LOCKED"));
        assert!(CLAIM_ONE_TIME_PREKEY_SQL.contains("claimed_by_device_id = $3"));
        assert!(CLAIM_ONE_TIME_PREKEY_SQL.contains("lifecycle_state = 'active'"));
    }

    #[test]
    fn envelope_idempotency_requires_an_exact_ciphertext_match() {
        assert!(ENQUEUE_ENVELOPE_SQL.contains("ON CONFLICT (envelope_id) DO NOTHING"));
        assert!(MATCH_ENVELOPE_SQL.contains("ciphertext_base64 = $11"));
        assert!(MATCH_ENVELOPE_SQL.contains("message_number = $7::numeric"));
        assert!(ENQUEUE_ENVELOPE_SQL.contains("recipient.lifecycle_state = 'active'"));
    }

    #[test]
    fn acknowledgement_is_separate_from_delivery() {
        assert!(PULL_MAILBOX_SQL.contains("delivered_at = COALESCE"));
        assert!(!PULL_MAILBOX_SQL.contains("acknowledged_at ="));
        assert!(PULL_MAILBOX_SQL.contains("FOR UPDATE OF mailbox"));
        assert!(!PULL_MAILBOX_SQL.contains("SKIP LOCKED"));
        assert!(ACKNOWLEDGE_ENVELOPE_SQL.contains("acknowledged_at = COALESCE"));
    }

    #[test]
    fn revocation_is_terminal_across_legacy_and_signal_state() {
        assert!(REVOKE_DEVICE_SQL.contains("revoked = true"));
        assert!(REVOKE_DEVICE_SQL.contains("lifecycle_state = 'revoked'"));
        assert!(DELETE_REVOKED_PREKEYS_SQL.contains("device_one_time_prekeys"));
        assert!(DELETE_REVOKED_MAIL_SQL.contains("sender_device_id = $2"));
        assert!(DELETE_REVOKED_MAIL_SQL.contains("recipient_device_id = $2"));
    }

    #[test]
    fn store_types_cannot_represent_private_signal_or_recovery_material() {
        let source = include_str!("signal_store.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes tests");
        for forbidden in [
            "identity_private",
            "signed_prekey_private",
            "ratchet_state",
            "vault_plaintext",
            "pin_verifier",
            "biometric_template",
            "otp_code",
        ] {
            assert!(!source.contains(forbidden), "store contains {forbidden}");
        }
    }
}
