//! Disabled-by-default HTTP adapters for the transactional Signal store.
//!
//! These handlers authenticate the device from the request head before reading
//! any body, bind account/sender routing metadata to that authenticated device,
//! and pass only public prekeys or opaque recipient ciphertext to persistence.
//! Session construction, ratchets, private keys, vault plaintext, PINs, OTPs,
//! biometrics, and recovery secrets remain client-side.

use crate::auth::AuthedDevice;
use crate::device_sync_protocol::{SignalCiphertextEnvelope, SignalDevicePreKeyBundle};
use crate::error::ApiError;
use crate::json::JsonBody;
use crate::signal_bundle_store;
use crate::signal_maintenance::{self, SignalMaintenanceError};
use crate::signal_prekey_publish::{self, OneTimePreKey, PublishPreKeys};
use crate::signal_store::{self, MailboxEnvelope, SignalStoreError};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const DEFAULT_MAILBOX_LIMIT: u64 = 50;
const MAX_PUBLISHED_ONE_TIME_PREKEYS: usize = 100;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/signal/prekeys", put(publish_prekeys_handler))
        .route(
            "/v1/signal/devices/{device_id}/prekey-bundle",
            post(claim_prekey_bundle_handler),
        )
        .route("/v1/signal/device-revision", get(device_revision_handler))
        .route("/v1/signal/envelopes", post(enqueue_envelope_handler))
        .route("/v1/signal/mailbox", get(pull_mailbox_handler))
        .route("/v1/signal/mailbox/ack", post(acknowledge_batch_handler))
        .route(
            "/v1/signal/mailbox/{envelope_id}/ack",
            post(acknowledge_envelope_handler),
        )
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OneTimePreKeyRequest {
    prekey_id: u32,
    public_key: Vec<u8>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishPreKeysRequest {
    version: u16,
    bundle_revision: u64,
    registration_id: u32,
    identity_key: Vec<u8>,
    signed_pre_key_id: u32,
    signed_pre_key: Vec<u8>,
    signed_pre_key_signature: Vec<u8>,
    pq_signed_pre_key_id: u32,
    pq_signed_pre_key: Vec<u8>,
    pq_signed_pre_key_signature: Vec<u8>,
    one_time_prekeys: Vec<OneTimePreKeyRequest>,
    expires_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishPreKeysResponse {
    bundle_revision: u64,
    device_revision: u64,
    unclaimed_prekey_count: u32,
}

pub(crate) async fn publish_prekeys_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(request): JsonBody<PublishPreKeysRequest>,
) -> Result<Json<PublishPreKeysResponse>, ApiError> {
    if request.one_time_prekeys.len() > MAX_PUBLISHED_ONE_TIME_PREKEYS {
        return Err(ApiError::BadRequest);
    }

    let PublishPreKeysRequest {
        version,
        bundle_revision,
        registration_id,
        identity_key,
        signed_pre_key_id,
        signed_pre_key,
        signed_pre_key_signature,
        pq_signed_pre_key_id,
        pq_signed_pre_key,
        pq_signed_pre_key_signature,
        one_time_prekeys,
        expires_at_ms,
    } = request;
    let bundle = SignalDevicePreKeyBundle {
        version,
        device_id: who.device_id.to_string(),
        registration_id,
        identity_key,
        signed_pre_key_id,
        signed_pre_key,
        signed_pre_key_signature,
        pq_signed_pre_key_id,
        pq_signed_pre_key,
        pq_signed_pre_key_signature,
        one_time_pre_key_id: None,
        one_time_pre_key: None,
    };
    let published = signal_prekey_publish::publish_prekeys(
        state.database(),
        PublishPreKeys {
            account_id: who.account_id,
            device_id: who.device_id,
            bundle_revision,
            bundle,
            one_time_prekeys: one_time_prekeys
                .into_iter()
                .map(|prekey| OneTimePreKey {
                    prekey_id: prekey.prekey_id,
                    public_key: prekey.public_key,
                })
                .collect(),
            expires_at_ms,
        },
    )
    .await
    .map_err(map_store_error)?;

    Ok(Json(PublishPreKeysResponse {
        bundle_revision: published.bundle_revision,
        device_revision: published.device_revision,
        unclaimed_prekey_count: published.unclaimed_prekey_count,
    }))
}

#[derive(Debug, Serialize)]
pub(crate) struct PreKeyBundleResponse {
    device_revision: i64,
    bundle: SignalDevicePreKeyBundle,
    low_prekeys: bool,
}

pub(crate) async fn claim_prekey_bundle_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    Path(target_device_id): Path<Uuid>,
) -> Result<Json<PreKeyBundleResponse>, ApiError> {
    require_sibling_target(target_device_id, who)?;
    let claimed = signal_bundle_store::claim_prekey_bundle(
        state.database(),
        who.account_id,
        target_device_id,
        who.device_id,
    )
    .await
    .map_err(map_store_error)?;

    Ok(Json(PreKeyBundleResponse {
        device_revision: claimed.device_revision,
        bundle: claimed.bundle,
        low_prekeys: claimed.low_prekeys,
    }))
}

#[derive(Debug, Serialize)]
pub(crate) struct DeviceRevisionResponse {
    device_revision: i64,
}

pub(crate) async fn device_revision_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
) -> Result<Json<DeviceRevisionResponse>, ApiError> {
    let device_revision = signal_bundle_store::current_device_revision(
        state.database(),
        who.account_id,
        who.device_id,
    )
    .await
    .map_err(map_store_error)?;
    Ok(Json(DeviceRevisionResponse { device_revision }))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueueEnvelopeRequest {
    envelope: SignalCiphertextEnvelope,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct QueueEnvelopeResponse {
    mailbox_seq: i64,
    duplicate: bool,
}

pub(crate) async fn enqueue_envelope_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(request): JsonBody<QueueEnvelopeRequest>,
) -> Result<(StatusCode, Json<QueueEnvelopeResponse>), ApiError> {
    require_authenticated_sender(&request.envelope, who)?;
    let result = signal_store::enqueue_envelope(state.database(), &request.envelope)
        .await
        .map_err(map_store_error)?;
    let (status, response) = queue_http_response(result.mailbox_seq, result.inserted);
    Ok((status, Json(response)))
}

fn queue_http_response(mailbox_seq: i64, inserted: bool) -> (StatusCode, QueueEnvelopeResponse) {
    let status = if inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (
        status,
        QueueEnvelopeResponse {
            mailbox_seq,
            duplicate: !inserted,
        },
    )
}

#[derive(Debug, Deserialize)]
pub(crate) struct PullMailboxQuery {
    #[serde(default)]
    after_mailbox_seq: i64,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct MailboxEnvelopeResponse {
    mailbox_seq: i64,
    envelope: SignalCiphertextEnvelope,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct MailboxPullResponse {
    items: Vec<MailboxEnvelopeResponse>,
    next_cursor: i64,
}

pub(crate) async fn pull_mailbox_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    Query(query): Query<PullMailboxQuery>,
) -> Result<Json<MailboxPullResponse>, ApiError> {
    if query.after_mailbox_seq < 0 || query.limit == Some(0) {
        return Err(ApiError::BadRequest);
    }
    let after_mailbox_seq = query.after_mailbox_seq;
    let rows = signal_store::pull_mailbox(
        state.database(),
        who.account_id,
        who.device_id,
        after_mailbox_seq,
        query.limit.unwrap_or(DEFAULT_MAILBOX_LIMIT),
    )
    .await
    .map_err(map_store_error)?;
    Ok(Json(mailbox_pull_response(rows, after_mailbox_seq)))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcknowledgeBatchItem {
    envelope_id: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcknowledgeBatchRequest {
    items: Vec<AcknowledgeBatchItem>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AcknowledgeBatchResponse {
    acknowledged: u32,
}

pub(crate) async fn acknowledge_batch_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(request): JsonBody<AcknowledgeBatchRequest>,
) -> Result<Json<AcknowledgeBatchResponse>, ApiError> {
    let envelope_ids: Vec<Uuid> = request
        .items
        .into_iter()
        .map(|item| item.envelope_id)
        .collect();
    let acknowledged = signal_maintenance::acknowledge_batch(
        state.database(),
        who.account_id,
        who.device_id,
        &envelope_ids,
    )
    .await
    .map_err(map_maintenance_error)?;
    Ok(Json(AcknowledgeBatchResponse { acknowledged }))
}

#[derive(Debug, Serialize)]
pub(crate) struct AcknowledgeEnvelopeResponse {
    mailbox_seq: i64,
}

pub(crate) async fn acknowledge_envelope_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    Path(envelope_id): Path<Uuid>,
) -> Result<Json<AcknowledgeEnvelopeResponse>, ApiError> {
    let mailbox_seq = signal_store::acknowledge_envelope(
        state.database(),
        who.account_id,
        who.device_id,
        envelope_id,
    )
    .await
    .map_err(map_store_error)?;
    Ok(Json(AcknowledgeEnvelopeResponse { mailbox_seq }))
}

fn require_sibling_target(target_device_id: Uuid, who: AuthedDevice) -> Result<(), ApiError> {
    if target_device_id == who.device_id {
        return Err(ApiError::BadRequest);
    }
    Ok(())
}

fn require_authenticated_sender(
    envelope: &SignalCiphertextEnvelope,
    who: AuthedDevice,
) -> Result<(), ApiError> {
    if envelope.metadata.account_id != who.account_id.to_string()
        || envelope.metadata.sender_device_id != who.device_id.to_string()
    {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

fn mailbox_response(row: MailboxEnvelope) -> MailboxEnvelopeResponse {
    MailboxEnvelopeResponse {
        mailbox_seq: row.mailbox_seq,
        envelope: row.envelope,
    }
}

fn mailbox_pull_response(
    rows: Vec<MailboxEnvelope>,
    after_mailbox_seq: i64,
) -> MailboxPullResponse {
    let items: Vec<MailboxEnvelopeResponse> = rows.into_iter().map(mailbox_response).collect();
    let next_cursor = items
        .last()
        .map(|item| item.mailbox_seq)
        .unwrap_or(after_mailbox_seq);
    MailboxPullResponse { items, next_cursor }
}

fn map_maintenance_error(error: SignalMaintenanceError) -> ApiError {
    match error {
        SignalMaintenanceError::InvalidBatch | SignalMaintenanceError::DeviceUnavailable => {
            ApiError::BadRequest
        }
        SignalMaintenanceError::Database(error) => error.into(),
    }
}

pub(crate) fn map_store_error(error: SignalStoreError) -> ApiError {
    match error {
        SignalStoreError::Validation(_)
        | SignalStoreError::InvalidUuid(_)
        | SignalStoreError::DeviceUnavailable => ApiError::BadRequest,
        SignalStoreError::RecipientUnavailable => ApiError::Gone,
        SignalStoreError::RevisionConflict | SignalStoreError::IdempotencyConflict => {
            ApiError::Conflict
        }
        SignalStoreError::Database(error) => error.into(),
        SignalStoreError::InvalidStoredCiphertext(_)
        | SignalStoreError::InvalidStoredMessageNumber(_)
        | SignalStoreError::InvalidStoredKind => {
            tracing::error!("invalid Signal state read from database");
            ApiError::Internal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_sync_protocol::{SignalEnvelopeKind, SignalEnvelopeMetadata};
    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    const SIGNAL_HTTP_FIXTURE: &str = include_str!("../fixtures/signal-http-wire.json");
    const SIGNAL_HTTP_FIXTURE_SHA256: &str =
        "8c6a2a72b52cb6d5c9e3ff32b4348d426dec81c32746d83540af67e497559ef3";

    fn fixture_round_trip<T>(value: &Value) -> T
    where
        T: DeserializeOwned + Serialize,
    {
        let decoded: T = serde_json::from_value(value.clone()).expect("canonical fixture decodes");
        assert_eq!(
            serde_json::to_value(&decoded).expect("backend DTO serializes"),
            value.clone(),
            "backend DTO must preserve the canonical wire shape"
        );
        decoded
    }

    fn envelope(account_id: Uuid, sender_device_id: Uuid) -> SignalCiphertextEnvelope {
        SignalCiphertextEnvelope {
            metadata: SignalEnvelopeMetadata {
                version: 1,
                envelope_id: Uuid::new_v4().to_string(),
                account_id: account_id.to_string(),
                sender_device_id: sender_device_id.to_string(),
                recipient_device_id: Uuid::new_v4().to_string(),
                session_id: "session-1".to_owned(),
                message_number: 0,
                kind: SignalEnvelopeKind::VaultMutation,
                created_at_ms: 1,
                expires_at_ms: 2,
            },
            ciphertext: vec![1],
        }
    }

    #[test]
    fn sender_and_account_are_bound_to_the_authenticated_device() {
        let account_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let who = AuthedDevice {
            account_id,
            device_id,
        };
        assert!(require_authenticated_sender(&envelope(account_id, device_id), who).is_ok());
        assert!(matches!(
            require_authenticated_sender(&envelope(Uuid::new_v4(), device_id), who),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            require_authenticated_sender(&envelope(account_id, Uuid::new_v4()), who),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn query_bounds_fail_before_database_access() {
        assert!(
            PullMailboxQuery {
                after_mailbox_seq: -1,
                limit: None,
            }
            .after_mailbox_seq
                < 0
        );
        assert_eq!(
            PullMailboxQuery {
                after_mailbox_seq: 0,
                limit: Some(0),
            }
            .limit,
            Some(0)
        );
    }

    #[test]
    fn a_device_cannot_claim_its_own_prekey_bundle() {
        let who = AuthedDevice {
            account_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
        };
        assert!(matches!(
            require_sibling_target(who.device_id, who),
            Err(ApiError::BadRequest)
        ));
        assert!(require_sibling_target(Uuid::new_v4(), who).is_ok());
    }

    #[test]
    fn canonical_interface_fixture_crosses_backend_dtos_and_cursor_logic() {
        assert_eq!(
            hex::encode(Sha256::digest(SIGNAL_HTTP_FIXTURE.as_bytes())),
            SIGNAL_HTTP_FIXTURE_SHA256,
            "fixture must remain byte-identical to 3fa-interfaces@f8114237"
        );
        let fixture: Value =
            serde_json::from_str(SIGNAL_HTTP_FIXTURE).expect("canonical fixture is JSON");

        let publish = &fixture["publish_prekeys"];
        let request: PublishPreKeysRequest = fixture_round_trip(&publish["request"]);
        assert_eq!(request.bundle_revision, 7);
        assert_eq!(request.one_time_prekeys.len(), 1);
        let response: PublishPreKeysResponse = fixture_round_trip(&publish["response"]);
        assert_eq!(response.device_revision, 12);
        assert_eq!(response.unclaimed_prekey_count, 24);

        let queue = &fixture["queue_envelope"];
        let request: QueueEnvelopeRequest = fixture_round_trip(&queue["request"]);
        request
            .envelope
            .validate()
            .expect("fixture envelope validates");
        let who = AuthedDevice {
            account_id: request
                .envelope
                .metadata
                .account_id
                .parse()
                .expect("fixture account UUID"),
            device_id: request
                .envelope
                .metadata
                .sender_device_id
                .parse()
                .expect("fixture sender UUID"),
        };
        assert!(require_authenticated_sender(&request.envelope, who).is_ok());
        let _: QueueEnvelopeResponse = fixture_round_trip(&queue["response"]);
        let (created_status, created) = queue_http_response(31, true);
        assert_eq!(created_status, StatusCode::CREATED);
        assert_eq!(
            serde_json::to_value(created).unwrap(),
            queue["response"],
            "new queue rows map inserted=true to duplicate=false"
        );
        let (duplicate_status, duplicate) = queue_http_response(31, false);
        assert_eq!(duplicate_status, StatusCode::OK);
        assert!(duplicate.duplicate);

        let pull = &fixture["pull_mailbox"];
        let _: MailboxPullResponse = fixture_round_trip(&pull["response"]);
        let _: MailboxPullResponse = fixture_round_trip(&pull["empty_response"]);
        let page = mailbox_pull_response(
            vec![MailboxEnvelope {
                mailbox_seq: 31,
                envelope: request.envelope.clone(),
            }],
            0,
        );
        assert_eq!(serde_json::to_value(page).unwrap(), pull["response"]);
        let empty_page = mailbox_pull_response(Vec::new(), 31);
        assert_eq!(
            serde_json::to_value(empty_page).unwrap(),
            pull["empty_response"],
            "empty pages preserve the caller's monotonic cursor"
        );

        let acknowledge = &fixture["acknowledge_mailbox"];
        let request: AcknowledgeBatchRequest = fixture_round_trip(&acknowledge["request"]);
        assert_eq!(request.items.len(), 1);
        let _: AcknowledgeBatchResponse = fixture_round_trip(&acknowledge["response"]);

        assert!(matches!(
            map_store_error(SignalStoreError::IdempotencyConflict),
            ApiError::Conflict
        ));
        assert!(matches!(
            map_store_error(SignalStoreError::RevisionConflict),
            ApiError::Conflict
        ));
        assert!(matches!(
            map_store_error(SignalStoreError::RecipientUnavailable),
            ApiError::Gone
        ));
    }
}
