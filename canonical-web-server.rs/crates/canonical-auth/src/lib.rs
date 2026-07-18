use async_trait::async_trait;
use base64::{
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, TimeZone, Utc};
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    SessionCookie,
    Bearer,
}

/// Identity established by either an opaque web session or a verified bearer
/// token. Transport-specific extractors live in the web crate; keeping this
/// value here lets the session state machine remain independent of Axum.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthContext {
    pub user_id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub source: CredentialSource,
    /// Supabase session identity extracted only after the bearer token was
    /// accepted by the configured Auth provider. Opaque alternate-provider
    /// tokens deliberately leave this unset.
    #[serde(skip_serializing)]
    pub supabase_session_id: Option<Uuid>,
    #[serde(skip_serializing)]
    pub session_hash: Option<String>,
    #[serde(skip_serializing)]
    pub csrf_token: Option<String>,
    #[serde(skip_serializing)]
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SupabaseUser {
    pub id: Uuid,
    pub email: Option<String>,
}

#[derive(Clone)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub user: SupabaseUser,
}

impl fmt::Debug for AuthTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthTokens")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("user", &self.user)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum AuthProviderError {
    #[error("credentials were rejected")]
    InvalidCredentials,
    #[error("authentication service is unavailable")]
    Unavailable,
    #[error("authentication service returned an invalid response")]
    InvalidResponse,
}

#[derive(Debug, Error)]
pub enum SupabaseAuthBuildError {
    #[error("Supabase Auth requires a publishable key; secret/service-role keys are forbidden")]
    PrivilegedKey,
    #[error("HTTP client configuration failed")]
    Http(#[from] reqwest::Error),
}

#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn password_sign_in(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthTokens, AuthProviderError>;
    async fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthProviderError>;
    async fn user_for_token(&self, access_token: &str) -> Result<SupabaseUser, AuthProviderError>;
    async fn sign_out(&self, access_token: &str) -> Result<(), AuthProviderError>;
}

#[derive(Clone)]
pub struct SupabaseAuth {
    client: reqwest::Client,
    base_url: String,
    publishable_key: String,
}

impl SupabaseAuth {
    pub fn new(base_url: String, publishable_key: String) -> Result<Self, SupabaseAuthBuildError> {
        validate_publishable_key(&publishable_key)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            base_url,
            publishable_key,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{}", self.base_url, path))
            .header("apikey", &self.publishable_key)
    }

    async fn token_request(
        &self,
        grant_type: &str,
        body: serde_json::Value,
    ) -> Result<AuthTokens, AuthProviderError> {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/auth/v1/token?grant_type={grant_type}"),
            )
            .json(&body)
            .send()
            .await
            .map_err(|_| AuthProviderError::Unavailable)?;

        if matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED
        ) {
            return Err(AuthProviderError::InvalidCredentials);
        }
        if !response.status().is_success() {
            return Err(AuthProviderError::Unavailable);
        }

        let response: TokenResponse = response
            .json()
            .await
            .map_err(|_| AuthProviderError::InvalidResponse)?;
        validated_session_id(&response.access_token, response.user.id)?
            .ok_or(AuthProviderError::InvalidResponse)?;
        let expires_at = response
            .expires_at
            .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
            .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(response.expires_in));

        Ok(AuthTokens {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_at,
            user: response.user,
        })
    }
}

fn validate_publishable_key(value: &str) -> Result<(), SupabaseAuthBuildError> {
    let value = value.trim();
    if value
        .strip_prefix("sb_publishable_")
        .is_some_and(|suffix| !suffix.is_empty())
    {
        return Ok(());
    }
    if value.starts_with("sb_secret_") {
        return Err(SupabaseAuthBuildError::PrivilegedKey);
    }

    let mut segments = value.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(SupabaseAuthBuildError::PrivilegedKey);
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(SupabaseAuthBuildError::PrivilegedKey);
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .map_err(|_| SupabaseAuthBuildError::PrivilegedKey)?;
    let claims: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|_| SupabaseAuthBuildError::PrivilegedKey)?;
    if claims.get("role").and_then(serde_json::Value::as_str) != Some("anon") {
        return Err(SupabaseAuthBuildError::PrivilegedKey);
    }
    Ok(())
}

#[async_trait]
impl AuthProvider for SupabaseAuth {
    async fn password_sign_in(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthTokens, AuthProviderError> {
        self.token_request(
            "password",
            serde_json::json!({ "email": email, "password": password }),
        )
        .await
    }

    async fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
        self.token_request(
            "refresh_token",
            serde_json::json!({ "refresh_token": refresh_token }),
        )
        .await
    }

    async fn user_for_token(&self, access_token: &str) -> Result<SupabaseUser, AuthProviderError> {
        let response = self
            .request(reqwest::Method::GET, "/auth/v1/user")
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| AuthProviderError::Unavailable)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(AuthProviderError::InvalidCredentials);
        }
        if !response.status().is_success() {
            return Err(AuthProviderError::Unavailable);
        }
        response
            .json()
            .await
            .map_err(|_| AuthProviderError::InvalidResponse)
    }

    async fn sign_out(&self, access_token: &str) -> Result<(), AuthProviderError> {
        let response = self
            .request(reqwest::Method::POST, "/auth/v1/logout?scope=local")
            .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
            .send()
            .await
            .map_err(|_| AuthProviderError::Unavailable)?;
        if response.status().is_success() {
            Ok(())
        } else if response.status() == StatusCode::UNAUTHORIZED {
            Err(AuthProviderError::InvalidCredentials)
        } else {
            Err(AuthProviderError::Unavailable)
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    expires_at: Option<i64>,
    user: SupabaseUser,
}

/// Extracts the session identifier only after the caller has established that
/// the token came from, or was accepted by, the configured Supabase Auth
/// service. This validates claim shape and user binding; JWT signature
/// verification remains Supabase's responsibility.
pub fn validated_session_id(
    access_token: &str,
    expected_user_id: Uuid,
) -> Result<Option<Uuid>, AuthProviderError> {
    let mut segments = access_token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        // Test and alternate AuthProvider implementations may use opaque access
        // tokens. Supabase's own token endpoint is checked above and always
        // returns a JWT, so this compatibility path never weakens that client.
        return Ok(None);
    };
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| AuthProviderError::InvalidResponse)?;
    let claims: serde_json::Value =
        serde_json::from_slice(&payload).map_err(|_| AuthProviderError::InvalidResponse)?;
    let subject = claims
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(AuthProviderError::InvalidResponse)?;
    if subject != expected_user_id {
        return Err(AuthProviderError::InvalidResponse);
    }
    let session_id = claims
        .get("session_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(AuthProviderError::InvalidResponse)?;
    Ok(Some(session_id))
}

#[cfg(test)]
mod tests {
    use super::{validated_session_id, AuthTokens, SupabaseAuth, SupabaseUser};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use chrono::Utc;
    use uuid::Uuid;

    fn jwt(user_id: Uuid, session_id: Option<Uuid>) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let mut claims = serde_json::json!({ "sub": user_id });
        if let Some(session_id) = session_id {
            claims["session_id"] = serde_json::Value::String(session_id.to_string());
        }
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn token_debug_redacts_bearer_material() {
        let tokens = AuthTokens {
            access_token: "access-secret".into(),
            refresh_token: "refresh-secret".into(),
            expires_at: Utc::now(),
            user: SupabaseUser {
                id: Uuid::new_v4(),
                email: Some("user@example.com".into()),
            },
        };
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn auth_client_rejects_privileged_api_keys() {
        assert!(
            SupabaseAuth::new("https://example.supabase.co".into(), "sb_secret_x".into(),).is_err()
        );

        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"role":"service_role"}"#);
        let service_role = format!("{header}.{payload}.signature");
        assert!(SupabaseAuth::new("https://example.supabase.co".into(), service_role,).is_err());
        assert!(SupabaseAuth::new(
            "https://example.supabase.co".into(),
            "sb_publishable_test_only".into(),
        )
        .is_ok());
    }

    #[test]
    fn session_id_is_bound_to_the_verified_user() {
        let user_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        assert_eq!(
            validated_session_id(&jwt(user_id, Some(session_id)), user_id).unwrap(),
            Some(session_id)
        );
        assert!(validated_session_id(&jwt(user_id, Some(session_id)), Uuid::new_v4()).is_err());
        assert!(validated_session_id(&jwt(user_id, None), user_id).is_err());
        assert_eq!(
            validated_session_id("opaque-test-token", user_id).unwrap(),
            None
        );
    }
}
