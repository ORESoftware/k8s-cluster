mod extractor;
mod rate_limit;
mod session;
mod supabase;

pub use extractor::{
    require_csrf, require_origin, Authenticated, OptionalAuthenticated, SessionAuthenticated,
};
pub use rate_limit::LoginRateLimiter;
pub use session::{CreatedSession, SessionService};
pub use supabase::{AuthProvider, AuthProviderError, AuthTokens, SupabaseAuth, SupabaseUser};

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    SessionCookie,
    Bearer,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthContext {
    pub user_id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub source: CredentialSource,
    #[serde(skip_serializing)]
    pub session_hash: Option<String>,
    #[serde(skip_serializing)]
    pub csrf_token: Option<String>,
    #[serde(skip_serializing)]
    pub expires_at: DateTime<Utc>,
}
