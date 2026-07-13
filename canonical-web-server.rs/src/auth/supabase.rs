use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SupabaseUser {
    pub id: Uuid,
    pub email: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub user: SupabaseUser,
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
    pub fn new(base_url: String, publishable_key: String) -> Result<Self, reqwest::Error> {
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
        if response.status().is_success() || response.status() == StatusCode::UNAUTHORIZED {
            Ok(())
        } else {
            Err(AuthProviderError::Unavailable)
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    expires_at: Option<i64>,
    user: SupabaseUser,
}
