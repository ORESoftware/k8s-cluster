//! Opaque sealed-vault storage with version-vector reconciliation.
//!
//! The server treats the payload as ciphertext (zero-knowledge). All it reasons
//! about is the **version vector**: a per-device logical clock used for
//! last-writer-wins-with-merge. A push is accepted only if the client has
//! already observed the server's current version (its `base_version` dominates
//! the stored one); otherwise the client must pull, merge locally, and retry.

use crate::auth::AuthedDevice;
use crate::error::ApiError;
use crate::protocol::{
    KdfParams, PullResponse, PushRequest, PushResponse, SealedBlob, VersionEntry, VersionVector,
};
use base64::Engine;
use sqlx::types::Json;
use sqlx::PgPool;
use uuid::Uuid;

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

type BlobRow = (String, String, String, Json<KdfParams>, Json<VersionVector>);

pub async fn load(pool: &PgPool, account_id: Uuid) -> Result<PullResponse, ApiError> {
    let row: Option<BlobRow> = sqlx::query_as(
        "SELECT ciphertext, nonce, kdf_salt, kdf_params, version \
         FROM threefa.vault_blobs WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some((ciphertext, nonce, kdf_salt, params, version)) => PullResponse {
            blob: Some(SealedBlob {
                ciphertext: base64::engine::general_purpose::STANDARD
                    .decode(ciphertext)
                    .map_err(|_| ApiError::Internal)?,
                nonce: base64::engine::general_purpose::STANDARD
                    .decode(nonce)
                    .map_err(|_| ApiError::Internal)?,
                kdf_salt: base64::engine::general_purpose::STANDARD
                    .decode(kdf_salt)
                    .map_err(|_| ApiError::Internal)?,
                kdf_params: params.0,
            }),
            version: version.0,
        },
        None => PullResponse {
            blob: None,
            version: Vec::new(),
        },
    })
}

pub async fn store(
    pool: &PgPool,
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

    // Read current version (default empty), reconcile, then upsert atomically.
    let mut tx = pool.begin().await?;

    let current: Option<Json<VersionVector>> = sqlx::query_scalar(
        "SELECT version FROM threefa.vault_blobs WHERE account_id = $1 FOR UPDATE",
    )
    .bind(who.account_id)
    .fetch_optional(&mut *tx)
    .await?;
    let stored = current.map(|j| j.0).unwrap_or_default();

    let new_version = match reconcile(&stored, &req.base_version, &req.device_id) {
        Ok(v) => v,
        Err(server_version) => {
            tx.rollback().await?;
            return Ok(PushResponse::Conflict { server_version });
        }
    };

    // Cap version-vector cardinality so a spoofed stream of distinct device ids
    // can't grow the stored JSON without bound (each push rewrites it under lock).
    if new_version.len() > crate::protocol::MAX_VERSION_ENTRIES {
        tx.rollback().await?;
        return Err(ApiError::BadRequest);
    }

    let ciphertext = base64::engine::general_purpose::STANDARD.encode(&req.blob.ciphertext);
    let nonce = base64::engine::general_purpose::STANDARD.encode(&req.blob.nonce);
    let kdf_salt = base64::engine::general_purpose::STANDARD.encode(&req.blob.kdf_salt);

    sqlx::query(
        "INSERT INTO threefa.vault_blobs (account_id, ciphertext, nonce, kdf_salt, kdf_params, version, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now()) \
         ON CONFLICT (account_id) DO UPDATE SET \
           ciphertext = EXCLUDED.ciphertext, nonce = EXCLUDED.nonce, \
           kdf_salt = EXCLUDED.kdf_salt, kdf_params = EXCLUDED.kdf_params, \
           version = EXCLUDED.version, updated_at = now()",
    )
    .bind(who.account_id)
    .bind(ciphertext)
    .bind(nonce)
    .bind(kdf_salt)
    .bind(Json(req.blob.kdf_params))
    .bind(Json(&new_version))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(PushResponse::Ok {
        version: new_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
