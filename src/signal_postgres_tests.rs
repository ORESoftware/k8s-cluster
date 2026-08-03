//! Real PostgreSQL tests for the Signal transaction boundaries.
//!
//! These tests are ignored by the ordinary unit-test job and run in the
//! dedicated PostgreSQL CI job with `--ignored --test-threads=1`.

use crate::device_sync_protocol::SignalCiphertextEnvelope;
use crate::signal_bundle_store::claim_prekey_bundle;
use crate::signal_maintenance::{acknowledge_batch, cleanup_expired_state};
use crate::signal_store::{enqueue_envelope, pull_mailbox, revoke_device, SignalStoreError};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sea_orm::{
    ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, QueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::sync::Barrier;
use tokio::time::timeout;
use uuid::Uuid;

const TEST_DATABASE_ENV: &str = "THREEFA_SIGNAL_TEST_DATABASE_URL";

async fn database() -> DatabaseConnection {
    let url = std::env::var(TEST_DATABASE_ENV)
        .unwrap_or_else(|_| panic!("{TEST_DATABASE_ENV} is required for ignored PostgreSQL tests"));
    Database::connect(url)
        .await
        .expect("connect test PostgreSQL")
}

async fn reset_schema(db: &DatabaseConnection) {
    db.execute_unprepared(
        r#"
DROP SCHEMA IF EXISTS threefa CASCADE;
CREATE SCHEMA threefa;

CREATE TABLE threefa.accounts (
    id UUID PRIMARY KEY,
    signal_device_revision BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE threefa.devices (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    lifecycle_state TEXT NOT NULL DEFAULT 'active',
    suspended_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE TABLE threefa.device_prekey_bundles (
    device_id UUID PRIMARY KEY,
    bundle_revision BIGINT NOT NULL,
    protocol_version SMALLINT NOT NULL,
    registration_id BIGINT NOT NULL,
    identity_key_base64 TEXT NOT NULL,
    signed_prekey_id BIGINT NOT NULL,
    signed_prekey_base64 TEXT NOT NULL,
    signed_prekey_signature_base64 TEXT NOT NULL,
    pq_signed_prekey_id BIGINT NOT NULL,
    pq_signed_prekey_base64 TEXT NOT NULL,
    pq_signed_prekey_signature_base64 TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE threefa.device_one_time_prekeys (
    device_id UUID NOT NULL,
    prekey_id BIGINT NOT NULL,
    public_key_base64 TEXT NOT NULL,
    claimed_at TIMESTAMPTZ,
    claimed_by_device_id UUID,
    PRIMARY KEY (device_id, prekey_id)
);

CREATE TABLE threefa.device_mailbox (
    mailbox_seq BIGSERIAL UNIQUE,
    envelope_id UUID PRIMARY KEY,
    account_id UUID NOT NULL,
    sender_device_id UUID,
    recipient_device_id UUID NOT NULL,
    protocol_version SMALLINT,
    session_id TEXT,
    message_number NUMERIC(20, 0),
    kind TEXT,
    client_created_at_ms BIGINT,
    expires_at_ms BIGINT NOT NULL,
    ciphertext_base64 TEXT,
    ciphertext_size_bytes INTEGER,
    delivered_at TIMESTAMPTZ,
    acknowledged_at TIMESTAMPTZ
);

CREATE TABLE threefa.device_security_events (
    event_seq BIGSERIAL PRIMARY KEY,
    account_id UUID NOT NULL,
    actor_device_id UUID,
    subject_device_id UUID,
    device_revision BIGINT NOT NULL,
    event_kind TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#,
    )
    .await
    .expect("create isolated Signal test schema");
}

async fn execute(db: &DatabaseConnection, sql: String) {
    db.execute_unprepared(&sql)
        .await
        .expect("execute fixture SQL");
}

fn required_i64(row: &QueryResult, column: &str) -> i64 {
    row.try_get("", column).expect("read i64 column")
}

async fn wait_for_lock_wait(db: &DatabaseConnection, query_fragment: &str) {
    timeout(Duration::from_secs(2), async {
        loop {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_stat_activity
    WHERE pid <> pg_backend_pid()
      AND datname = current_database()
      AND wait_event_type = 'Lock'
      AND query LIKE $1
) AS blocked
"#,
                    vec![format!("%{query_fragment}%").into()],
                ))
                .await
                .expect("inspect PostgreSQL lock waits")
                .expect("lock-wait query returns a row");
            if row.try_get::<bool>("", "blocked").unwrap() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("query never entered a lock wait: {query_fragment}"));
}

fn canonical_envelope() -> SignalCiphertextEnvelope {
    let fixture: Value = serde_json::from_str(include_str!("../fixtures/signal-http-wire.json"))
        .expect("canonical Signal fixture is JSON");
    serde_json::from_value(fixture["queue_envelope"]["request"]["envelope"].clone())
        .expect("canonical queue envelope decodes")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn concurrent_prekey_claims_return_distinct_one_time_keys() {
    let db = Arc::new(database().await);
    reset_schema(db.as_ref()).await;

    let account_id = Uuid::new_v4();
    let target_id = Uuid::new_v4();
    let requester_a = Uuid::new_v4();
    let requester_b = Uuid::new_v4();
    let public_key = BASE64.encode([7_u8; 32]);
    let signature = BASE64.encode([8_u8; 64]);

    execute(
        db.as_ref(),
        format!(
            "INSERT INTO threefa.accounts (id, signal_device_revision) VALUES ('{account_id}', 12);"
        ),
    )
    .await;
    for device_id in [target_id, requester_a, requester_b] {
        execute(
            db.as_ref(),
            format!(
                "INSERT INTO threefa.devices (id, account_id) VALUES ('{device_id}', '{account_id}');"
            ),
        )
        .await;
    }
    execute(
        db.as_ref(),
        format!(
            "INSERT INTO threefa.device_prekey_bundles (device_id, bundle_revision, protocol_version, registration_id, identity_key_base64, signed_prekey_id, signed_prekey_base64, signed_prekey_signature_base64, pq_signed_prekey_id, pq_signed_prekey_base64, pq_signed_prekey_signature_base64, expires_at) VALUES ('{target_id}', 1, 1, 42, '{public_key}', 1, '{public_key}', '{signature}', 2, '{public_key}', '{signature}', clock_timestamp() + interval '1 day');"
        ),
    )
    .await;
    for prekey_id in [101, 102] {
        execute(
            db.as_ref(),
            format!(
                "INSERT INTO threefa.device_one_time_prekeys (device_id, prekey_id, public_key_base64) VALUES ('{target_id}', {prekey_id}, '{public_key}');"
            ),
        )
        .await;
    }

    let barrier = Arc::new(Barrier::new(3));
    let task = |requester_id: Uuid| {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            claim_prekey_bundle(db.as_ref(), account_id, target_id, requester_id)
                .await
                .expect("claim public bundle")
        })
    };
    let task_a = task(requester_a);
    let task_b = task(requester_b);
    barrier.wait().await;

    let claimed_a = task_a.await.expect("requester A task");
    let claimed_b = task_b.await.expect("requester B task");
    let id_a = claimed_a
        .bundle
        .one_time_pre_key_id
        .expect("requester A receives one-time key");
    let id_b = claimed_b
        .bundle
        .one_time_pre_key_id
        .expect("requester B receives one-time key");

    assert_ne!(id_a, id_b, "concurrent claims must never reuse a prekey");
    assert_eq!(claimed_a.device_revision, 12);
    assert_eq!(claimed_b.device_revision, 12);

    let row = db
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS claimed, count(DISTINCT claimed_by_device_id)::bigint AS claimants FROM threefa.device_one_time_prekeys WHERE claimed_at IS NOT NULL",
        ))
        .await
        .expect("query claimed prekeys")
        .expect("claim count row");
    assert_eq!(required_i64(&row, "claimed"), 2);
    assert_eq!(required_i64(&row, "claimants"), 2);
}

#[tokio::test]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn envelope_idempotency_and_mailbox_cursors_are_postgres_enforced() {
    let db = database().await;
    reset_schema(&db).await;

    let mut envelope = canonical_envelope();
    let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1_000;
    envelope.metadata.created_at_ms = now_ms;
    envelope.metadata.expires_at_ms = now_ms + 60_000;
    let account_id = Uuid::parse_str(&envelope.metadata.account_id).unwrap();
    let sender_id = Uuid::parse_str(&envelope.metadata.sender_device_id).unwrap();
    let recipient_id = Uuid::parse_str(&envelope.metadata.recipient_device_id).unwrap();
    execute(
        &db,
        format!(
            "INSERT INTO threefa.accounts (id) VALUES ('{account_id}'); INSERT INTO threefa.devices (id, account_id) VALUES ('{sender_id}', '{account_id}'), ('{recipient_id}', '{account_id}');"
        ),
    )
    .await;

    let inserted = enqueue_envelope(&db, &envelope)
        .await
        .expect("insert canonical-derived envelope");
    assert!(inserted.inserted);
    let duplicate = enqueue_envelope(&db, &envelope)
        .await
        .expect("exact retry is idempotent");
    assert!(!duplicate.inserted);
    assert_eq!(duplicate.mailbox_seq, inserted.mailbox_seq);

    let mut conflicting = envelope.clone();
    conflicting.ciphertext[0] ^= 0xff;
    assert!(matches!(
        enqueue_envelope(&db, &conflicting).await,
        Err(SignalStoreError::IdempotencyConflict)
    ));

    let page = pull_mailbox(&db, account_id, recipient_id, 0, 50)
        .await
        .expect("pull recipient mailbox");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].mailbox_seq, inserted.mailbox_seq);
    assert_eq!(page[0].envelope, envelope);

    let delivered_retry = pull_mailbox(&db, account_id, recipient_id, 0, 50)
        .await
        .expect("delivery without acknowledgement remains retryable");
    assert_eq!(delivered_retry.len(), 1);
    assert_eq!(delivered_retry[0].mailbox_seq, inserted.mailbox_seq);

    assert!(
        pull_mailbox(&db, account_id, recipient_id, inserted.mailbox_seq, 50)
            .await
            .expect("cursor after delivered row")
            .is_empty()
    );
    assert!(pull_mailbox(&db, Uuid::new_v4(), recipient_id, 0, 50)
        .await
        .expect("cross-account pull closes to an empty page")
        .is_empty());
}

#[tokio::test]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn revoked_recipient_is_terminal_not_an_idempotency_collision() {
    let db = database().await;
    reset_schema(&db).await;

    let mut envelope = canonical_envelope();
    let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1_000;
    envelope.metadata.created_at_ms = now_ms;
    envelope.metadata.expires_at_ms = now_ms + 60_000;
    let account_id = Uuid::parse_str(&envelope.metadata.account_id).unwrap();
    let sender_id = Uuid::parse_str(&envelope.metadata.sender_device_id).unwrap();
    let recipient_id = Uuid::parse_str(&envelope.metadata.recipient_device_id).unwrap();
    execute(
        &db,
        format!(
            "INSERT INTO threefa.accounts (id) VALUES ('{account_id}'); INSERT INTO threefa.devices (id, account_id) VALUES ('{sender_id}', '{account_id}'), ('{recipient_id}', '{account_id}');"
        ),
    )
    .await;

    enqueue_envelope(&db, &envelope)
        .await
        .expect("initial envelope reaches an active recipient");
    execute(
        &db,
        format!(
            "UPDATE threefa.devices SET revoked = true, lifecycle_state = 'revoked', revoked_at = now() WHERE id = '{recipient_id}';"
        ),
    )
    .await;

    assert!(matches!(
        enqueue_envelope(&db, &envelope).await,
        Err(SignalStoreError::RecipientUnavailable)
    ));

    envelope.metadata.envelope_id = Uuid::new_v4().to_string();
    envelope.metadata.message_number += 1;
    assert!(matches!(
        enqueue_envelope(&db, &envelope).await,
        Err(SignalStoreError::RecipientUnavailable)
    ));
}

#[tokio::test]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn revocation_advances_revision_reaps_signal_state_and_records_audit_event() {
    let db = database().await;
    reset_schema(&db).await;

    let account_id = Uuid::new_v4();
    let actor_id = Uuid::new_v4();
    let subject_id = Uuid::new_v4();
    let envelope_id = Uuid::new_v4();
    let public_key = BASE64.encode([7_u8; 32]);
    let signature = BASE64.encode([8_u8; 64]);
    execute(
        &db,
        format!(
            "INSERT INTO threefa.accounts (id, signal_device_revision) VALUES ('{account_id}', 3); \
             INSERT INTO threefa.devices (id, account_id) VALUES ('{actor_id}', '{account_id}'), ('{subject_id}', '{account_id}'); \
             INSERT INTO threefa.device_prekey_bundles (device_id, bundle_revision, protocol_version, registration_id, identity_key_base64, signed_prekey_id, signed_prekey_base64, signed_prekey_signature_base64, pq_signed_prekey_id, pq_signed_prekey_base64, pq_signed_prekey_signature_base64, expires_at) VALUES ('{subject_id}', 1, 1, 42, '{public_key}', 1, '{public_key}', '{signature}', 2, '{public_key}', '{signature}', clock_timestamp() + interval '1 day'); \
             INSERT INTO threefa.device_one_time_prekeys (device_id, prekey_id, public_key_base64) VALUES ('{subject_id}', 101, '{public_key}'); \
             INSERT INTO threefa.device_mailbox (envelope_id, account_id, sender_device_id, recipient_device_id, expires_at_ms) VALUES ('{envelope_id}', '{account_id}', '{actor_id}', '{subject_id}', 9999999999999);"
        ),
    )
    .await;

    let new_revision = revoke_device(&db, account_id, actor_id, subject_id, Some(3))
        .await
        .expect("revoke active sibling at the expected revision");
    assert_eq!(new_revision, 4);

    let state = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT revoked, lifecycle_state, revoked_at IS NOT NULL AS has_revoked_at, suspended_at IS NULL AS cleared_suspension FROM threefa.devices WHERE id = $1",
            vec![subject_id.into()],
        ))
        .await
        .expect("query revoked device")
        .expect("revoked device remains auditable");
    assert!(state.try_get::<bool>("", "revoked").unwrap());
    assert_eq!(
        state.try_get::<String>("", "lifecycle_state").unwrap(),
        "revoked"
    );
    assert!(state.try_get::<bool>("", "has_revoked_at").unwrap());
    assert!(state.try_get::<bool>("", "cleared_suspension").unwrap());

    let counts = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT (SELECT count(*)::bigint FROM threefa.device_prekey_bundles WHERE device_id = $1) AS bundles, (SELECT count(*)::bigint FROM threefa.device_one_time_prekeys WHERE device_id = $1) AS prekeys, (SELECT count(*)::bigint FROM threefa.device_mailbox WHERE sender_device_id = $1 OR recipient_device_id = $1) AS mail",
            vec![subject_id.into()],
        ))
        .await
        .expect("query reaped Signal state")
        .expect("cleanup count row");
    assert_eq!(required_i64(&counts, "bundles"), 0);
    assert_eq!(required_i64(&counts, "prekeys"), 0);
    assert_eq!(required_i64(&counts, "mail"), 0);

    let event = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT actor_device_id, subject_device_id, device_revision, event_kind FROM threefa.device_security_events WHERE account_id = $1",
            vec![account_id.into()],
        ))
        .await
        .expect("query revocation audit event")
        .expect("revocation emits an audit event");
    assert_eq!(event.try_get::<Uuid>("", "actor_device_id").unwrap(), actor_id);
    assert_eq!(
        event.try_get::<Uuid>("", "subject_device_id").unwrap(),
        subject_id
    );
    assert_eq!(required_i64(&event, "device_revision"), 4);
    assert_eq!(
        event.try_get::<String>("", "event_kind").unwrap(),
        "device_revoked"
    );

    assert!(matches!(
        revoke_device(&db, account_id, actor_id, subject_id, Some(4)).await,
        Err(SignalStoreError::DeviceUnavailable)
    ));
    let revision = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT signal_device_revision FROM threefa.accounts WHERE id = $1",
            vec![account_id.into()],
        ))
        .await
        .expect("query revision after duplicate revoke")
        .expect("account remains present");
    assert_eq!(required_i64(&revision, "signal_device_revision"), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn concurrent_exact_retry_and_locked_cursor_cannot_skip_mail() {
    let db = Arc::new(database().await);
    reset_schema(db.as_ref()).await;

    let mut envelope = canonical_envelope();
    let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1_000;
    envelope.metadata.created_at_ms = now_ms;
    envelope.metadata.expires_at_ms = now_ms + 60_000;
    let envelope_id = Uuid::parse_str(&envelope.metadata.envelope_id).unwrap();
    let account_id = Uuid::parse_str(&envelope.metadata.account_id).unwrap();
    let sender_id = Uuid::parse_str(&envelope.metadata.sender_device_id).unwrap();
    let recipient_id = Uuid::parse_str(&envelope.metadata.recipient_device_id).unwrap();
    execute(
        db.as_ref(),
        format!(
            "INSERT INTO threefa.accounts (id) VALUES ('{account_id}'); INSERT INTO threefa.devices (id, account_id) VALUES ('{sender_id}', '{account_id}'), ('{recipient_id}', '{account_id}');"
        ),
    )
    .await;

    let transaction = db.begin().await.expect("begin uncommitted enqueue");
    let inserted = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
INSERT INTO threefa.device_mailbox (
    envelope_id, account_id, sender_device_id, recipient_device_id,
    protocol_version, session_id, message_number, kind,
    client_created_at_ms, expires_at_ms, ciphertext_base64,
    ciphertext_size_bytes
) VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8, $9, $10, $11, $12)
RETURNING mailbox_seq
"#,
            vec![
                envelope_id.into(),
                account_id.into(),
                sender_id.into(),
                recipient_id.into(),
                i64::from(envelope.metadata.version).into(),
                envelope.metadata.session_id.clone().into(),
                envelope.metadata.message_number.to_string().into(),
                "vault_mutation".into(),
                envelope.metadata.created_at_ms.into(),
                envelope.metadata.expires_at_ms.into(),
                BASE64.encode(&envelope.ciphertext).into(),
                i64::try_from(envelope.ciphertext.len()).unwrap().into(),
            ],
        ))
        .await
        .expect("insert uncommitted envelope")
        .expect("insert returns mailbox cursor");
    let first_cursor = required_i64(&inserted, "mailbox_seq");

    let barrier = Arc::new(Barrier::new(2));
    let retry_db = Arc::clone(&db);
    let retry_envelope = envelope.clone();
    let retry_barrier = Arc::clone(&barrier);
    let retry = tokio::spawn(async move {
        retry_barrier.wait().await;
        enqueue_envelope(retry_db.as_ref(), &retry_envelope).await
    });
    barrier.wait().await;
    wait_for_lock_wait(db.as_ref(), "INSERT INTO threefa.device_mailbox").await;
    transaction
        .commit()
        .await
        .expect("commit original envelope");
    let duplicate = retry
        .await
        .expect("retry task")
        .expect("concurrent exact retry is idempotent");
    assert!(!duplicate.inserted);
    assert_eq!(duplicate.mailbox_seq, first_cursor);

    envelope.metadata.envelope_id = Uuid::new_v4().to_string();
    envelope.metadata.message_number += 1;
    let second = enqueue_envelope(db.as_ref(), &envelope)
        .await
        .expect("insert later envelope");
    assert!(second.mailbox_seq > first_cursor);

    let locker = db.begin().await.expect("begin cursor lock");
    locker
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT mailbox_seq FROM threefa.device_mailbox WHERE mailbox_seq = $1 FOR UPDATE",
            vec![first_cursor.into()],
        ))
        .await
        .expect("lock first cursor")
        .expect("first cursor exists");

    let pull_db = Arc::clone(&db);
    let pull_barrier = Arc::new(Barrier::new(2));
    let task_barrier = Arc::clone(&pull_barrier);
    let pull = tokio::spawn(async move {
        task_barrier.wait().await;
        pull_mailbox(pull_db.as_ref(), account_id, recipient_id, 0, 1).await
    });
    pull_barrier.wait().await;
    wait_for_lock_wait(db.as_ref(), "WITH selected AS").await;
    locker.commit().await.expect("release first cursor");
    let page = pull
        .await
        .expect("pull task")
        .expect("pull after cursor lock");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].mailbox_seq, first_cursor);
}

#[tokio::test]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn batch_acknowledgement_is_atomic_idempotent_and_cleanup_is_bounded() {
    let db = database().await;
    reset_schema(&db).await;

    let account_id = Uuid::new_v4();
    let recipient_id = Uuid::new_v4();
    let sender_id = Uuid::new_v4();
    let envelope_a = Uuid::new_v4();
    let envelope_b = Uuid::new_v4();
    let unknown = Uuid::new_v4();
    let now_ms = 2_000_000_i64;

    execute(
        &db,
        format!(
            "INSERT INTO threefa.accounts (id) VALUES ('{account_id}'); INSERT INTO threefa.devices (id, account_id) VALUES ('{recipient_id}', '{account_id}'), ('{sender_id}', '{account_id}'); INSERT INTO threefa.device_mailbox (envelope_id, account_id, recipient_device_id, expires_at_ms) VALUES ('{envelope_a}', '{account_id}', '{recipient_id}', {now_ms} + 1000), ('{envelope_b}', '{account_id}', '{recipient_id}', {now_ms} + 1000);"
        ),
    )
    .await;

    let acknowledged = acknowledge_batch(
        &db,
        account_id,
        recipient_id,
        &[envelope_a, envelope_a, envelope_b, unknown],
    )
    .await
    .expect("acknowledge bounded batch");
    assert_eq!(acknowledged, 2);
    assert_eq!(
        acknowledge_batch(&db, account_id, recipient_id, &[envelope_a, envelope_b])
            .await
            .expect("repeat acknowledgement is idempotent"),
        0
    );

    execute(
        &db,
        format!(
            "UPDATE threefa.device_mailbox SET acknowledged_at = clock_timestamp() - interval '8 days' WHERE envelope_id = '{envelope_a}'; UPDATE threefa.device_mailbox SET expires_at_ms = {now_ms} WHERE envelope_id = '{envelope_b}'; INSERT INTO threefa.device_prekey_bundles (device_id, bundle_revision, protocol_version, registration_id, identity_key_base64, signed_prekey_id, signed_prekey_base64, signed_prekey_signature_base64, pq_signed_prekey_id, pq_signed_prekey_base64, pq_signed_prekey_signature_base64, expires_at) VALUES ('{recipient_id}', 1, 1, 42, '{}', 1, '{}', '{}', 2, '{}', '{}', clock_timestamp() - interval '1 second'); INSERT INTO threefa.device_one_time_prekeys (device_id, prekey_id, public_key_base64, claimed_at, claimed_by_device_id) VALUES ('{recipient_id}', 7, '{}', clock_timestamp() - interval '31 days', '{sender_id}');",
            BASE64.encode([1_u8; 32]),
            BASE64.encode([2_u8; 32]),
            BASE64.encode([3_u8; 64]),
            BASE64.encode([4_u8; 32]),
            BASE64.encode([5_u8; 64]),
            BASE64.encode([6_u8; 32]),
        ),
    )
    .await;

    let cleanup = cleanup_expired_state(&db, now_ms)
        .await
        .expect("cleanup expired Signal state");
    assert_eq!(cleanup.mailbox_rows, 2);
    assert_eq!(cleanup.bundle_rows, 1);
    assert_eq!(cleanup.prekey_rows, 1);
}
