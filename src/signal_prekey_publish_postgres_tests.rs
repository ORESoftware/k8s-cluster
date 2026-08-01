//! Real PostgreSQL coverage for device-supplied prekey bundle revisions.

use crate::device_sync_protocol::SignalDevicePreKeyBundle;
use crate::signal_prekey_publish::{publish_prekeys, OneTimePreKey, PublishPreKeys};
use crate::signal_store::SignalStoreError;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
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
    lifecycle_state TEXT NOT NULL DEFAULT 'active'
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
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
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

CREATE TABLE threefa.device_security_events (
    event_seq BIGSERIAL PRIMARY KEY,
    account_id UUID NOT NULL,
    actor_device_id UUID,
    subject_device_id UUID,
    device_revision BIGINT NOT NULL,
    event_kind TEXT NOT NULL
);
"#,
    )
    .await
    .expect("create isolated prekey publication schema");
}

fn request(
    account_id: Uuid,
    device_id: Uuid,
    revision: u64,
    signed_key_byte: u8,
    prekeys: &[(u32, u8)],
) -> PublishPreKeys {
    PublishPreKeys {
        account_id,
        device_id,
        bundle_revision: revision,
        bundle: SignalDevicePreKeyBundle {
            version: 1,
            device_id: device_id.to_string(),
            registration_id: 42,
            identity_key: vec![1; 32],
            signed_pre_key_id: revision as u32,
            signed_pre_key: vec![signed_key_byte; 32],
            signed_pre_key_signature: vec![3; 64],
            pq_signed_pre_key_id: 11,
            pq_signed_pre_key: vec![4; 32],
            pq_signed_pre_key_signature: vec![5; 64],
            one_time_pre_key_id: None,
            one_time_pre_key: None,
        },
        one_time_prekeys: prekeys
            .iter()
            .map(|(prekey_id, byte)| OneTimePreKey {
                prekey_id: *prekey_id,
                public_key: vec![*byte; 32],
            })
            .collect(),
        expires_at_ms: 4_102_444_800_000,
    }
}

#[tokio::test]
#[ignore = "requires THREEFA_SIGNAL_TEST_DATABASE_URL"]
async fn publish_revision_retry_stale_conflict_and_replenishment_are_atomic() {
    let db = database().await;
    reset_schema(&db).await;
    let account_id = Uuid::new_v4();
    let device_id = Uuid::new_v4();
    db.execute_unprepared(&format!(
        "INSERT INTO threefa.accounts (id) VALUES ('{account_id}'); \
         INSERT INTO threefa.devices (id, account_id) VALUES ('{device_id}', '{account_id}');"
    ))
    .await
    .expect("seed active device");

    let revision_seven = request(account_id, device_id, 7, 2, &[(100, 6)]);
    let first = publish_prekeys(&db, revision_seven.clone())
        .await
        .expect("publish initial public bundle");
    assert_eq!(first.bundle_revision, 7);
    assert_eq!(first.device_revision, 1);
    assert_eq!(first.unclaimed_prekey_count, 1);

    let retry = publish_prekeys(&db, revision_seven.clone())
        .await
        .expect("exact same-revision retry is idempotent");
    assert_eq!(retry, first, "retry must not bump account revision");

    assert!(matches!(
        publish_prekeys(&db, request(account_id, device_id, 6, 2, &[])).await,
        Err(SignalStoreError::RevisionConflict)
    ));
    assert!(matches!(
        publish_prekeys(&db, request(account_id, device_id, 7, 9, &[(100, 6)])).await,
        Err(SignalStoreError::IdempotencyConflict)
    ));

    let revision_eight = publish_prekeys(&db, request(account_id, device_id, 8, 8, &[(101, 7)]))
        .await
        .expect("advance revision and replenish one fresh key");
    assert_eq!(revision_eight.bundle_revision, 8);
    assert_eq!(revision_eight.device_revision, 2);
    assert_eq!(revision_eight.unclaimed_prekey_count, 2);

    assert!(matches!(
        publish_prekeys(&db, request(account_id, device_id, 9, 9, &[(100, 6)]),).await,
        Err(SignalStoreError::IdempotencyConflict)
    ));

    let row = db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "SELECT bundle_revision, (SELECT signal_device_revision FROM threefa.accounts WHERE id = '{account_id}') AS device_revision FROM threefa.device_prekey_bundles WHERE device_id = '{device_id}'"
            ),
        ))
        .await
        .expect("query committed revision")
        .expect("bundle row remains");
    assert_eq!(row.try_get::<i64>("", "bundle_revision").unwrap(), 8);
    assert_eq!(row.try_get::<i64>("", "device_revision").unwrap(), 2);
}
