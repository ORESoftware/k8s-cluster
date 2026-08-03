//! Isolated compiler harness for the production Shared Auth startup contract.
//!
//! The billing binary's main manifest has private sibling path dependencies.
//! This crate supplies only the two configuration types `src/shared_auth.rs`
//! consumes, then compiles and runs that production module's own unit tests.

pub mod supabase_auth {
    #[derive(Clone, Debug, Default)]
    pub struct SupabaseConfig {
        pub url: Option<String>,
        pub audience: String,
        pub issuer: Option<String>,
        pub jwks_url: Option<String>,
        pub jwt_secret: Option<String>,
    }
}

pub mod config {
    use crate::supabase_auth::SupabaseConfig;

    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub supabase: SupabaseConfig,
        pub tenant_routes_require_user_jwt: bool,
        pub step_up_required_for_mutations: bool,
    }
}

#[path = "../../../src/shared_auth.rs"]
pub mod shared_auth;
