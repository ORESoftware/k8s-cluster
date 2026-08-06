//! Browser-e2e authentication provider.
//!
//! This module is compiled only with the non-default `test-auth` feature. The
//! crate root rejects that feature when debug assertions are disabled, and
//! serving still requires an exact runtime opt-in. Production images therefore
//! cannot contain or activate this provider.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use uuid::Uuid;

use super::{AuthProvider, AuthProviderError, AuthTokens, SupabaseUser};

pub(crate) const ENABLE_ENV: &str = "CANONICAL_TEST_AUTH_ENABLED";
pub(crate) const EMAIL: &str = "browser-e2e@canonical.invalid";
pub(crate) const PASSWORD: &str = "browser-e2e-only";

const ACCESS_TOKEN: &str = "canonical-browser-e2e-access";
const REFRESH_TOKEN: &str = "canonical-browser-e2e-refresh";
const USER_ID: Uuid = Uuid::from_u128(0x7f0a6ff9_94e7_4f6e_a3d7_38df501792b4);

#[derive(Debug, Default)]
pub(crate) struct BrowserTestAuth;

impl BrowserTestAuth {
    pub(crate) fn is_enabled() -> bool {
        std::env::var(ENABLE_ENV).as_deref() == Ok("1")
    }

    fn user() -> SupabaseUser {
        SupabaseUser {
            id: USER_ID,
            email: Some(EMAIL.to_owned()),
        }
    }

    fn tokens() -> AuthTokens {
        AuthTokens {
            access_token: ACCESS_TOKEN.to_owned(),
            refresh_token: REFRESH_TOKEN.to_owned(),
            expires_at: Utc::now() + Duration::hours(1),
            user: Self::user(),
        }
    }
}

#[async_trait]
impl AuthProvider for BrowserTestAuth {
    async fn password_sign_in(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthTokens, AuthProviderError> {
        if email == EMAIL && password == PASSWORD {
            Ok(Self::tokens())
        } else {
            Err(AuthProviderError::InvalidCredentials)
        }
    }

    async fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
        if refresh_token == REFRESH_TOKEN {
            Ok(Self::tokens())
        } else {
            Err(AuthProviderError::InvalidCredentials)
        }
    }

    async fn user_for_token(&self, access_token: &str) -> Result<SupabaseUser, AuthProviderError> {
        if access_token == ACCESS_TOKEN {
            Ok(Self::user())
        } else {
            Err(AuthProviderError::InvalidCredentials)
        }
    }

    async fn sign_out(&self, access_token: &str) -> Result<(), AuthProviderError> {
        if access_token == ACCESS_TOKEN {
            Ok(())
        } else {
            Err(AuthProviderError::InvalidCredentials)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepts_only_the_fixed_browser_fixture() {
        let auth = BrowserTestAuth;
        assert!(auth.password_sign_in(EMAIL, PASSWORD).await.is_ok());
        assert!(matches!(
            auth.password_sign_in(EMAIL, "wrong").await,
            Err(AuthProviderError::InvalidCredentials)
        ));
        assert!(matches!(
            auth.user_for_token("wrong").await,
            Err(AuthProviderError::InvalidCredentials)
        ));
    }
}
