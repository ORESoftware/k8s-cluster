//! Opaque sealed-vault storage with version-vector reconciliation.
//!
//! The server treats the payload as ciphertext (zero-knowledge). All it reasons
//! about is the **version vector**: a per-device logical clock used for
//! last-writer-wins-with-merge. A push is accepted only if the client has
//! already observed the server's current version (its `base_version` dominates
//! the stored one); otherwise the client must pull, merge locally, and retry.

use crate::auth::AuthedDevice;
use crate::error::ApiError;
use sqlx::types::Json;
use sqlx::PgPool;
use crate::protocol::{KdfParams, PullResponse, PushRequest, PushResponse, SealedBlob, VersionEntry,
    VersionVector};
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

type BlobRow = (Vec<u8>, Vec<u8>, Vec<u8>, Json<KdfParams>, Json<VersionVector>);

pub async fn load(pool: &PgPool, account_id: Uuid) -> Result<PullResponse, ApiError> {
    let row: Option<BlobRow> = sqlx::query_as(
        "SELECT ciphertext, nonce, kdf_salt, kdf_params, version \
         FROM vault_blobs WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some((ciphertext, nonce, kdf_salt, params, version)) => PullResponse {
            blob: Some(SealedBlob {
                ciphertext,
                nonce,
                kdf_salt,
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
    if !req.blob.is_well_formed() || !crate::protocol::device_id_is_valid(&req.device_id) {
        return Err(ApiError::BadRequest);
    }

    // Read current version (default empty), reconcile, then upsert atomically.
    let mut tx = pool.begin().await?;

    let current: Option<Json<VersionVector>> =
        sqlx::query_scalar("SELECT version FROM vault_blobs WHERE account_id = $1 FOR UPDATE")
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

    sqlx::query(
        "INSERT INTO vault_blobs (account_id, ciphertext, nonce, kdf_salt, kdf_params, version, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now()) \
         ON CONFLICT (account_id) DO UPDATE SET \
           ciphertext = EXCLUDED.ciphertext, nonce = EXCLUDED.nonce, \
           kdf_salt = EXCLUDED.kdf_salt, kdf_params = EXCLUDED.kdf_params, \
           version = EXCLUDED.version, updated_at = now()",
    )
    .bind(who.account_id)
    .bind(&req.blob.ciphertext)
    .bind(&req.blob.nonce)
    .bind(&req.blob.kdf_salt)
    .bind(Json(req.blob.kdf_params))
    .bind(Json(&new_version))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(PushResponse::Ok { version: new_version })
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
    fn dominates_basics() {
        assert!(dominates(&vv(&[("a", 2)]), &vv(&[("a", 1)])));
        assert!(!dominates(&vv(&[("a", 1)]), &vv(&[("a", 2)])));
        assert!(dominates(&vv(&[("a", 1), ("b", 1)]), &vv(&[("a", 1)])));
    }
}
