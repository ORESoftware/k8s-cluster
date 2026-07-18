//! Opaque sealed-vault storage with version-vector reconciliation.
//!
//! The server treats the payload as ciphertext (zero-knowledge). All it reasons
//! about is the **version vector**: a per-device logical clock used for
//! last-writer-wins-with-merge. A push is accepted only if the client has
//! already observed the server's current version (its `base_version` dominates
//! the stored one); otherwise the client must pull, merge locally, and retry.

use crate::auth::AuthedDevice;
use crate::entity::{account, vault_blob};
use crate::error::ApiError;
use crate::protocol::{
    PullResponse, PushRequest, PushResponse, SealedBlob, VersionEntry, VersionVector,
};
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use base64::Engine;
use sea_orm::sea_query::LockType;
use sea_orm::{
    ActiveModelTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QuerySelect, Set,
    TransactionTrait,
};
use serde::de::DeserializeOwned;
use uuid::Uuid;

pub(crate) async fn pull_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PullResponse>, ApiError> {
    let who = crate::auth::authenticate(state.database(), &headers).await?;
    Ok(Json(load(state.database(), who.account_id).await?))
}

pub(crate) async fn push_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let who = crate::auth::authenticate(state.database(), &headers).await?;
    let response = store(state.database(), who, &request).await?;
    if matches!(&response, PushResponse::Conflict { .. }) {
        state.metrics.vault_conflicts.inc();
    }
    Ok(Json(response))
}

// ---- pure version-vector logic (unit-tested without a DB) ----

fn counter_for(v: &VersionVector, device: &str) -> u64 {
    v.iter()
        .find(|e| e.device_id == device)
        .map(|e| e.counter)
        .unwrap_or(0)
}

/// True if `a` has observed everything `b` has (a[dev] >= b[dev] for all dev).
pub fn dominates(a: &VersionVector, b: &VersionVector) -> bool {
    b.iter().all(|e| counter_for(a, &e.device_id) >= e.counter)
}

/// Return `base` with `device`'s counter incremented by one (added if absent).
pub fn bump(base: &VersionVector, device: &str) -> VersionVector {
    let mut out = base.clone();
    match out.iter_mut().find(|e| e.device_id == device) {
        Some(e) => e.counter += 1,
        None => out.push(VersionEntry {
            device_id: device.to_string(),
            counter: 1,
        }),
    }
    out
}

/// Decide the outcome of a push given the currently-stored version.
pub fn reconcile(
    stored: &VersionVector,
    base_version: &VersionVector,
    pushing_device: &str,
) -> Result<VersionVector, VersionVector> {
    // The client must have seen the server's latest before overwriting it.
    if dominates(base_version, stored) {
        Ok(bump(base_version, pushing_device))
    } else {
        Err(stored.clone())
    }
}

// ---- DB-backed handlers ----

pub async fn load(db: &DatabaseConnection, account_id: Uuid) -> Result<PullResponse, ApiError> {
    let row = vault_blob::Entity::find_by_id(account_id).one(db).await?;

    Ok(match row {
        Some(blob) => PullResponse {
            blob: Some(SealedBlob {
                ciphertext: base64::engine::general_purpose::STANDARD
                    .decode(blob.ciphertext)
                    .map_err(|_| ApiError::Internal)?,
                nonce: base64::engine::general_purpose::STANDARD
                    .decode(blob.nonce)
                    .map_err(|_| ApiError::Internal)?,
                kdf_salt: base64::engine::general_purpose::STANDARD
                    .decode(blob.kdf_salt)
                    .map_err(|_| ApiError::Internal)?,
                kdf_params: decode_json(blob.kdf_params, "kdf_params")?,
            }),
            version: decode_json(blob.version, "version")?,
        },
        None => PullResponse {
            blob: None,
            version: Vec::new(),
        },
    })
}

pub async fn store(
    db: &DatabaseConnection,
    who: AuthedDevice,
    req: &PushRequest,
) -> Result<PushResponse, ApiError> {
    // Reject malformed or hostile envelopes *before* touching the DB: enforce the
    // crypto shape (nonce/salt/ciphertext bounds) and sane KDF params (so a peer
    // device can't be made to allocate gigabytes on the next pull), and bound the
    // client-supplied device id (charset + length) so it can't inject junk into
    // the version vector. The server still never decrypts — this is pure shape.
    if !req.blob.is_well_formed()
        || !crate::protocol::device_id_is_valid(&req.device_id)
        || !crate::protocol::version_vector_is_well_formed(&req.base_version)
        || req.device_id != who.device_id.to_string()
    {
        return Err(ApiError::BadRequest);
    }

    // Lock the always-present account row so concurrent first pushes serialize
    // even before a vault row exists, then reconcile and write atomically.
    let transaction = db.begin().await?;
    account::Entity::find_by_id(who.account_id)
        .lock(LockType::Update)
        .one(&transaction)
        .await?
        .ok_or(ApiError::Internal)?;
    let current = vault_blob::Entity::find_by_id(who.account_id)
        .one(&transaction)
        .await?;
    let stored = current
        .as_ref()
        .map(|blob| decode_json(blob.version.clone(), "version"))
        .transpose()?
        .unwrap_or_default();

    let new_version = match reconcile(&stored, &req.base_version, &req.device_id) {
        Ok(v) => v,
        Err(server_version) => {
            transaction.rollback().await?;
            return Ok(PushResponse::Conflict { server_version });
        }
    };

    // Cap version-vector cardinality so a spoofed stream of distinct device ids
    // can't grow the stored JSON without bound (each push rewrites it under lock).
    if new_version.len() > crate::protocol::MAX_VERSION_ENTRIES {
        transaction.rollback().await?;
        return Err(ApiError::BadRequest);
    }

    let ciphertext = base64::engine::general_purpose::STANDARD.encode(&req.blob.ciphertext);
    let nonce = base64::engine::general_purpose::STANDARD.encode(&req.blob.nonce);
    let kdf_salt = base64::engine::general_purpose::STANDARD.encode(&req.blob.kdf_salt);

    let kdf_params = encode_json(&req.blob.kdf_params, "kdf_params")?;
    let version = encode_json(&new_version, "version")?;
    match current {
        Some(model) => {
            let mut active = model.into_active_model();
            active.ciphertext = Set(ciphertext);
            active.nonce = Set(nonce);
            active.kdf_salt = Set(kdf_salt);
            active.kdf_params = Set(kdf_params);
            active.version = Set(version);
            active.updated_at = Set(time::OffsetDateTime::now_utc());
            active.update(&transaction).await?;
        }
        None => {
            vault_blob::ActiveModel {
                account_id: Set(who.account_id),
                ciphertext: Set(ciphertext),
                nonce: Set(nonce),
                kdf_salt: Set(kdf_salt),
                kdf_params: Set(kdf_params),
                version: Set(version),
                ..Default::default()
            }
            .insert(&transaction)
            .await?;
        }
    }

    transaction.commit().await?;
    Ok(PushResponse::Ok {
        version: new_version,
    })
}

fn decode_json<T: DeserializeOwned>(
    value: serde_json::Value,
    field: &'static str,
) -> Result<T, ApiError> {
    serde_json::from_value(value).map_err(|error| {
        tracing::error!(error = %error, db.field = field, "invalid JSON in vault row");
        ApiError::Internal
    })
}

fn encode_json<T: serde::Serialize>(
    value: &T,
    field: &'static str,
) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(value).map_err(|error| {
        tracing::error!(error = %error, db.field = field, "failed to encode vault JSON");
        ApiError::Internal
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DatabaseBackend, MockDatabase};

    fn vv(pairs: &[(&str, u64)]) -> VersionVector {
        pairs
            .iter()
            .map(|(d, c)| VersionEntry {
                device_id: d.to_string(),
                counter: *c,
            })
            .collect()
    }

    #[test]
    fn first_push_accepted() {
        let stored = vv(&[]);
        let base = vv(&[]);
        let out = reconcile(&stored, &base, "devA").unwrap();
        assert_eq!(out, vv(&[("devA", 1)]));
    }

    #[test]
    fn sequential_push_from_same_device() {
        let stored = vv(&[("devA", 1)]);
        let base = vv(&[("devA", 1)]);
        let out = reconcile(&stored, &base, "devA").unwrap();
        assert_eq!(counter_for(&out, "devA"), 2);
    }

    #[test]
    fn stale_push_conflicts() {
        // Server advanced to devB:1 but client only saw devA:1.
        let stored = vv(&[("devA", 1), ("devB", 1)]);
        let base = vv(&[("devA", 1)]);
        let err = reconcile(&stored, &base, "devA").unwrap_err();
        assert_eq!(err, stored);
    }

    #[test]
    fn merged_client_push_accepted() {
        // Client pulled, now its base dominates the server's version.
        let stored = vv(&[("devA", 1), ("devB", 1)]);
        let base = vv(&[("devA", 1), ("devB", 1)]);
        let out = reconcile(&stored, &base, "devA").unwrap();
        assert_eq!(counter_for(&out, "devA"), 2);
        assert_eq!(counter_for(&out, "devB"), 1);
    }

    #[test]
    fn dominates_basics() {
        assert!(dominates(&vv(&[("a", 2)]), &vv(&[("a", 1)])));
        assert!(!dominates(&vv(&[("a", 1)]), &vv(&[("a", 2)])));
        assert!(dominates(&vv(&[("a", 1), ("b", 1)]), &vv(&[("a", 1)])));
    }

    #[tokio::test]
    async fn load_maps_an_absent_seaorm_row_to_an_empty_vault() {
        let database = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<crate::entity::vault_blob::Model>::new()])
            .into_connection();
        let response = load(&database, Uuid::new_v4()).await.unwrap();
        assert_eq!(response.blob, None);
        assert!(response.version.is_empty());
    }

    #[tokio::test]
    async fn load_decodes_the_seaorm_entity_into_protocol_types() {
        let params = crate::protocol::KdfParams::default();
        let version = vv(&[("device-a", 4)]);
        let model = crate::entity::vault_blob::Model {
            account_id: Uuid::new_v4(),
            ciphertext: base64::engine::general_purpose::STANDARD.encode([1, 2, 3]),
            nonce: base64::engine::general_purpose::STANDARD.encode([4; 24]),
            kdf_salt: base64::engine::general_purpose::STANDARD.encode([5; 16]),
            kdf_params: serde_json::to_value(params).unwrap(),
            version: serde_json::to_value(&version).unwrap(),
            updated_at: time::OffsetDateTime::now_utc(),
        };
        let database = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![model.clone()]])
            .into_connection();

        let response = load(&database, model.account_id).await.unwrap();
        let blob = response.blob.expect("stored blob");
        assert_eq!(blob.ciphertext, vec![1, 2, 3]);
        assert_eq!(blob.nonce, vec![4; 24]);
        assert_eq!(blob.kdf_salt, vec![5; 16]);
        assert_eq!(blob.kdf_params, params);
        assert_eq!(response.version, version);
    }
}
