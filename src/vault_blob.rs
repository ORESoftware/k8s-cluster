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
use crate::json::JsonBody;
use crate::protocol::{
    PullResponse, PushRequest, PushResponse, SealedBlob, VersionEntry, VersionVector,
};
use crate::state::AppState;
use axum::extract::State;
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
    who: AuthedDevice,
) -> Result<Json<PullResponse>, ApiError> {
    Ok(Json(load(state.database(), who.account_id).await?))
}

// `who` is declared BEFORE the body: axum resolves `FromRequestParts`
// extractors in order and the body last, so the sync token is verified before a
// single byte of JSON is deserialized. See `auth::AuthedDevice`'s extractor.
pub(crate) async fn push_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(request): JsonBody<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
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

/// True if `base_version` is *causally reachable* by `pushing_device`: it may
/// advance its own counter freely but must not claim any counter for another
/// device beyond what the server already stored. Merely dominating the stored
/// vector is not enough — a device could otherwise inflate a sibling's counter
/// (e.g. `devB: 9999`), poisoning the vector so the sibling's next honest push is
/// forever seen as stale. This restores the vector-clock invariant that an entry
/// only ever grows via its own device.
fn is_causal(stored: &VersionVector, base_version: &VersionVector, pushing_device: &str) -> bool {
    base_version.iter().all(|entry| {
        entry.device_id == pushing_device || entry.counter <= counter_for(stored, &entry.device_id)
    })
}

/// Decide the outcome of a push given the currently-stored version.
pub fn reconcile(
    stored: &VersionVector,
    base_version: &VersionVector,
    pushing_device: &str,
) -> Result<VersionVector, VersionVector> {
    // The client must have seen the server's latest before overwriting it, and
    // may only advance its own counter (not fabricate a sibling's).
    if dominates(base_version, stored) && is_causal(stored, base_version, pushing_device) {
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
    fn cannot_inflate_a_sibling_counter() {
        // Stored has devA:1, devB:1. devA tries to push a base that dominates but
        // fabricates devB:9999 — this must be rejected as non-causal, not stored.
        let stored = vv(&[("devA", 1), ("devB", 1)]);
        let base = vv(&[("devA", 1), ("devB", 9999)]);
        let err = reconcile(&stored, &base, "devA").unwrap_err();
        assert_eq!(err, stored, "sibling-inflating push must conflict, not win");
    }

    #[test]
    fn may_still_advance_own_counter_past_stored() {
        // devA legitimately advancing only its own counter is causal and accepted.
        let stored = vv(&[("devA", 5), ("devB", 2)]);
        let base = vv(&[("devA", 5), ("devB", 2)]);
        let out = reconcile(&stored, &base, "devA").unwrap();
        assert_eq!(counter_for(&out, "devA"), 6);
        assert_eq!(counter_for(&out, "devB"), 2);
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

    #[test]
    fn concurrent_vectors_dominate_neither_way() {
        // Each side has an event the other has not observed: true concurrency.
        let left = vv(&[("a", 2), ("b", 1)]);
        let right = vv(&[("a", 1), ("b", 2)]);
        assert!(!dominates(&left, &right));
        assert!(!dominates(&right, &left));
    }

    #[test]
    fn empty_vector_dominance() {
        let empty = vv(&[]);
        let some = vv(&[("a", 1)]);
        // Everything (including empty) dominates the empty history...
        assert!(dominates(&empty, &empty));
        assert!(dominates(&some, &empty));
        // ...but the empty history dominates nothing non-empty.
        assert!(!dominates(&empty, &some));
    }

    #[test]
    fn bump_adds_absent_device_and_preserves_others() {
        let base = vv(&[("a", 3)]);
        let out = bump(&base, "b");
        assert_eq!(counter_for(&out, "a"), 3);
        assert_eq!(counter_for(&out, "b"), 1);
        assert_eq!(out.len(), 2);
        // Bumping an existing device increments only that entry.
        let again = bump(&out, "b");
        assert_eq!(counter_for(&again, "b"), 2);
        assert_eq!(counter_for(&again, "a"), 3);
    }

    #[test]
    fn concurrent_push_conflicts_even_if_client_advanced_itself() {
        // Client advanced its own counter but never observed devB's write:
        // its base does not dominate the stored vector, so it must pull+merge.
        let stored = vv(&[("devA", 1), ("devB", 1)]);
        let base = vv(&[("devA", 2)]);
        let err = reconcile(&stored, &base, "devA").unwrap_err();
        assert_eq!(err, stored);
    }

    #[test]
    fn reconcile_accepts_superset_base_from_new_device() {
        // A freshly-enrolled device pulled (observing devA/devB) and merged in
        // history from a third source; its base strictly dominates the stored
        // version, so the push lands and only its own counter is bumped.
        let stored = vv(&[("devA", 1)]);
        let base = vv(&[("devA", 1), ("devC", 5)]);
        let out = reconcile(&stored, &base, "devC").unwrap();
        assert_eq!(counter_for(&out, "devA"), 1);
        assert_eq!(counter_for(&out, "devC"), 6);
    }
}
