//! Canonical leaf-hashing + Merkle root over a range of postings.
//!
//! The canonical encoding is deterministic and versioned so anchors remain
//! verifiable even as the rest of the schema evolves. Anyone with a posting
//! row + the anchor row can independently:
//!
//!   1. Recompute the leaf hash via [`posting_leaf_hash`].
//!   2. Recompute the Merkle path (returned by [`merkle_root_with_proofs`]).
//!   3. Walk the path against the on-chain anchor memo to verify inclusion.

use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::str::FromStr;

const LEAF_DOMAIN: &[u8] = b"billing-server-rs/leaf/v1";
const NODE_DOMAIN: &[u8] = b"billing-server-rs/node/v1";

/// Stable hash of a single posting. The fields are concatenated with explicit
/// length prefixes so injection across boundaries is impossible.
pub fn posting_leaf_hash(
    posting_id: i64,
    transaction_id: uuid::Uuid,
    account_id: uuid::Uuid,
    direction: &str,           // "debit" | "credit"
    amount_minor: &str,        // canonical decimal string from NUMERIC(38,0)
    currency: &str,            // 3-letter uppercase
    source: &str,
    source_event_id: &str,
    posted_at_unix_ms: i64,
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(LEAF_DOMAIN);
    write_len(&mut h, posting_id.to_be_bytes().as_slice());
    write_len(&mut h, transaction_id.as_bytes());
    write_len(&mut h, account_id.as_bytes());
    write_len(&mut h, direction.as_bytes());
    write_len(&mut h, amount_minor.as_bytes());
    write_len(&mut h, currency.as_bytes());
    write_len(&mut h, source.as_bytes());
    write_len(&mut h, source_event_id.as_bytes());
    write_len(&mut h, posted_at_unix_ms.to_be_bytes().as_slice());
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

fn write_len<H: Digest>(h: &mut H, bytes: &[u8]) {
    let len = (bytes.len() as u64).to_be_bytes();
    h.update(len);
    h.update(bytes);
}

/// Build a Merkle root over `leaves`. Returns the 32-byte root.
///
/// Duplicates the last leaf at each level if the count is odd (standard
/// "Bitcoin-style" padding — simple, well-understood).
pub fn merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        let mut h = Sha256::new();
        h.update(NODE_DOMAIN);
        h.update(b"empty");
        let out = h.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&out);
        return arr;
    }

    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().unwrap());
        }
        let mut next = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks(2) {
            let mut h = Sha256::new();
            h.update(NODE_DOMAIN);
            h.update(pair[0]);
            h.update(pair[1]);
            let out = h.finalize();
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&out);
            next.push(arr);
        }
        level = next;
    }
    level[0]
}

/// A Merkle inclusion proof. `path[i].0` is the sibling hash at level i,
/// `path[i].1` is true if the sibling sits on the right (i.e. the leaf's
/// running hash is the LEFT operand at that level).
#[derive(Clone, Debug)]
pub struct MerkleProof {
    pub leaf_index: usize,
    pub leaf_count: usize,
    pub path: Vec<([u8; 32], bool)>,
}

impl MerkleProof {
    /// Verify the proof against a known leaf and root.
    pub fn verify(&self, leaf: [u8; 32], root: [u8; 32]) -> bool {
        let mut acc = leaf;
        for (sibling, sibling_on_right) in &self.path {
            let mut h = Sha256::new();
            h.update(NODE_DOMAIN);
            if *sibling_on_right {
                h.update(acc);
                h.update(sibling);
            } else {
                h.update(sibling);
                h.update(acc);
            }
            let out = h.finalize();
            acc.copy_from_slice(&out);
        }
        acc == root
    }
}

/// Helper to parse the canonical amount_minor representation we use in the
/// schema (Postgres NUMERIC(38,0) -> string). Kept here for symmetry with
/// the verification endpoint.
pub fn canonical_amount(amount_minor_text: &str) -> String {
    Decimal::from_str(amount_minor_text)
        .map(|d| d.to_string())
        .unwrap_or_else(|_| amount_minor_text.to_string())
}
