//! Minting unified OreSoftware JWTs and publishing the JWKS that verifies them.
//!
//! Downstream services trust *this server's* signature (fetched once from
//! `/.well-known/jwks.json`) instead of each re-implementing Supabase
//! verification. That is the whole point of centralizing: one verifier to audit,
//! one key to rotate.

mod assurance;
mod claims;
mod jwks;
mod minter;

pub use assurance::{AuthenticationAssurance, ACR_LOA1, ACR_LOA2};
pub use claims::OreClaims;
pub use jwks::PublicJwks;
pub use minter::{MintContext, MintedToken, TokenMinter};
