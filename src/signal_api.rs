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
use crate::signal_store::{self, MailboxEnvelope, OneTimePreKey, PublishPreKeys, SignalStoreError};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const DEFAULT_MAILBOX_LIMIT: u64 = 50;
const MAX_PUBLISHED_ONE_TIME_PREKEYS: usize = 1_000;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/signal/prekeys", put(publish_prekeys_handler))
        .route("/v1/signal/envelopes", post(enqueue_envelope_handler))
        .route("/v1/signal/mailbox", get(pull_mailbox_handler))
        .route(
            "/v1/signal/mailbox/{envelope_id}/ack",
            post(acknowledge_envelope_handler),
        )
}

#[derive(Debug, Deserialize)]
pub(crate) struct OneTimePreKeyRequest {
    prekey_id: u32,
    public_key: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublishPreKeysRequest {
    bundle: SignalDevicePreKeyBundle,
    #[serde(default)]
    one_time_prekeys: Vec<OneTimePreKeyRequest>,
    expires_at_ms: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct PublishPreKeysResponse {
    bundle_revision: i64,
    device_list_revision: i64,
    inserted_one_time_prekeys: u64,
}

pub(crate) async fn publish_prekeys_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(request): JsonBody<PublishPreKeysRequest>,
) -> Result<Json<PublishPreKeysResponse>, ApiError> {
    if request.bundle.device_id != who.device_id.to_string()
        || request.one_time_prekeys.len() > MAX_PUBLISHED_ONE_TIME_PREKEYS
    {
        return Err(ApiError::BadRequest);
    }

    let published = signal_store::publish_prekeys(
        state.database(),
        PublishPreKeys {
            account_id: who.account_id,
            device_id: who.device_id,
            bundle: request.bundle,
            one_time_prekeys: request
                .one_time_prekeys
                .into_iter()
                .map(|prekey| OneTimePreKey {
                    prekey_id: prekey.prekey_id,
                    public_key: prekey.public_key,
                })
                .collect(),
            expires_at_ms: request.expires_at_ms,
        },
    )
    .await
    .map_err(map_store_error)?;

    Ok(Json(PublishPreKeysResponse {
        bundle_revision: published.bundle_revision,
        device_list_revision: published.device_list_revision,
        inserted_one_time_prekeys: published.inserted_one_time_prekeys,
    }))
}

#[derive(Debug, Serialize)]
pub(crate) struct EnqueueEnvelopeResponse {
    mailbox_seq: i64,
    inserted: bool,
}

pub(crate) async fn enqueue_envelope_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(envelope): JsonBody<SignalCiphertextEnvelope>,
) -> Result<(StatusCode, Json<EnqueueEnvelopeResponse>), ApiError> {
    require_authenticated_sender(&envelope, who)?;
    let result = signal_store::enqueue_envelope(state.database(), &envelope)
        .await
        .map_err(map_store_error)?;
    let status = if result.inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(EnqueueEnvelopeResponse {
            mailbox_seq: result.mailbox_seq,
            inserted: result.inserted,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PullMailboxQuery {
    #[serde(default)]
    after_mailbox_seq: i64,
    limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MailboxEnvelopeResponse {
    mailbox_seq: i64,
    envelope: SignalCiphertextEnvelope,
}

pub(crate) async fn pull_mailbox_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    Query(query): Query<PullMailboxQuery>,
) -> Result<Json<Vec<MailboxEnvelopeResponse>>, ApiError> {
    if query.after_mailbox_seq < 0 || query.limit == Some(0) {
        return Err(ApiError::BadRequest);
    }
    let rows = signal_store::pull_mailbox(
        state.database(),
        who.account_id,
        who.device_id,
        query.after_mailbox_seq,
        query.limit.unwrap_or(DEFAULT_MAILBOX_LIMIT),
    )
    .await
    .map_err(map_store_error)?;
    Ok(Json(rows.into_iter().map(mailbox_response).collect()))
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

fn map_store_error(error: SignalStoreError) -> ApiError {
    match error {
        SignalStoreError::Validation(_)
        | SignalStoreError::InvalidUuid(_)
        | SignalStoreError::DeviceUnavailable => ApiError::BadRequest,
        SignalStoreError::RevisionConflict | SignalStoreError::IdempotencyConflict => {
            ApiError::Conflict
        }
        SignalStoreError::Database(error) => error.into(),
        SignalStoreError::InvalidStoredCiphertext(_)
        | SignalStoreError::InvalidStoredMessageNumber(_)
        | SignalStoreError::InvalidStoredKind => {
            tracing::error!("invalid Signal mailbox state read from database");
            ApiError::Internal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_sync_protocol::{SignalEnvelopeKind, SignalEnvelopeMetadata};

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
}
