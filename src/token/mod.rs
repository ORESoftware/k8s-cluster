//! Minting unified OreSoftware JWTs and publishing the JWKS that verifies them.
//!
//! Downstream services trust *this server's* signature (fetched once from
//! `/.well-known/jwks.json`) instead of each re-implementing provider
//! verification. That is the whole point of centralizing: one verifier to audit,
//! one key to rotate.

mod claims;
mod jwks;
mod minter;

pub use claims::{OreClaims, ACR_BASE, ACR_STEP_UP};
pub use jwks::PublicJwks;
pub use minter::{MintContext, MintedToken, TokenMinter};
