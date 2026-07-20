//! Supabase-side verification: per-project JWKS verifiers, an issuer-keyed
//! registry that routes an incoming token to the project that signed it, and a
//! Management-API client used only by the offline `discover` subcommand.

mod claims;
pub mod management;
mod registry;
mod verifier;

pub use claims::{SupabaseClaims, VerifiedIdentity};
pub use registry::ProjectRegistry;
pub use verifier::SupabaseVerifier;
