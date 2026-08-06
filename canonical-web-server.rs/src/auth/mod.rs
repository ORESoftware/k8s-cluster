mod extractor;
mod rate_limit;
#[cfg(feature = "test-auth")]
pub(crate) mod test_provider;

pub use canonical_auth::{
    validated_session_id, AuthContext, AuthProvider, AuthProviderError, AuthTokens,
    CredentialSource, SupabaseAuth, SupabaseAuthBuildError, SupabaseUser,
};
pub use canonical_session::{CreatedSession, SessionService};
pub use extractor::{
    require_csrf, require_origin, Authenticated, OptionalAuthenticated, SessionAuthenticated,
};
pub use rate_limit::LoginRateLimiter;
