//! Solana integration: anchor service + read-only RPC client.
//!
//! Under Model A this module's only on-chain WRITE responsibility is publishing
//! Merkle roots of ledger postings to a Solana memo. Everything else is reads.
//!
//! The anchoring wallet is operationally separate from any tenant wallet — it
//! exists solely to pay the trivial transaction fee for publishing the memo,
//! and holds no customer funds.

pub mod anchor;
pub mod client;
pub mod merkle;
pub mod verify;

pub use anchor::AnchorService;
pub use client::SolanaClient;
#[allow(unused_imports)]
pub use merkle::{merkle_root, posting_leaf_hash, MerkleProof};
