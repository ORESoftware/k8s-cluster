//! Browser authorization-code handoff for product applications.
//!
//! The browser never receives a Supabase access or refresh token. Shared-auth
//! signs the user into the Supabase project assigned to the registered client,
//! verifies that project's returned access token, encrypts both upstream tokens,
//! and stores them behind a short-lived, single-use opaque code. The product
//! backend redeems the code over an authenticated server-to-server request and
//! proves possession of the browser-generated PKCE verifier.

mod config;
mod crypto;

use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::{
    config::{AppConfig, SupabaseProject},
    error::AuthError,
    supabase::ProjectRegistry,
};

use self::{
    config::{load_clients, BrowserClient},
    crypto::{
        constant_time_secret_eq, handoff_aad, is_base64url, pkce_challenge, random_code,
        token_hash, TokenCipher,
    },
};

const CODE_PREFIX: &str = "sac_";
const DEFAULT_CODE_TTL_SECONDS: u64 = 90;
const MAX_AUTH_VALUE_BYTES: usize = 512;

#[derive(Clone)]
pub struct HandoffService {
    db: DatabaseConnection,
    clients: Arc<HashMap<String, BrowserClient>>,
    cipher: Arc<TokenCipher>,
    code_ttl: Duration,
}

#[derive(Clone, Deserialize)]
pub struct AuthorizationRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub return_to: String,
    pub state: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
}

#[derive(Clone)]
pub struct ValidatedAuthorization {
    pub client_id: String,
    pub redirect_uri: String,
    pub return_to: String,
    pub state: String,
    pub code_challenge: String,
    supabase_project: String,
}

#[derive(Deserialize)]
pub struct RedeemAuthorizationCode {
    pub client_id: String,
    pub code: String,
    pub redirect_uri: String,
    pub code_verifier: String,
}

#[derive(Serialize, Deserialize)]
pub struct SupabaseHandoffTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub user: SupabaseHandoffUser,
    pub return_to: String,
    pub supabase_project: String,
}

#[derive(Serialize, Deserialize)]
pub struct SupabaseHandoffUser {
    pub id: Uuid,
    pub email: Option<String>,
}

#[derive(Debug)]
pub enum IssueError {
    InvalidCredentials,
    Request(AuthError),
}

impl From<AuthError> for IssueError {
    fn from(value: AuthError) -> Self {
        Self::Request(value)
    }
}

#[derive(Deserialize)]
struct SupabaseTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    #[serde(default)]
    expires_at: Option<i64>,
    user: SupabaseHandoffUser,
}

impl HandoffService {
    /// Build only when browser clients are configured. API-only deployments
    /// retain the same behavior and connection footprint as before.
    pub async fn build(config: &AppConfig) -> anyhow::Result<Option<Self>> {
        let raw_clients = match std::env::var("AUTH_BROWSER_CLIENTS") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Ok(None),
        };
        let clients = load_clients(config, &raw_clients)?;

        let encoded_key = std::env::var("AUTH_HANDOFF_ENCRYPTION_KEY")
            .map_err(|_| anyhow::anyhow!("AUTH_HANDOFF_ENCRYPTION_KEY is required"))?;
        let cipher = TokenCipher::from_encoded_key(&encoded_key)?;

        let code_ttl_seconds = std::env::var("AUTH_HANDOFF_CODE_TTL_SECS")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|_| anyhow::anyhow!("AUTH_HANDOFF_CODE_TTL_SECS must be an integer"))?
            .unwrap_or(DEFAULT_CODE_TTL_SECONDS);
        if !(30..=300).contains(&code_ttl_seconds) {
            anyhow::bail!("AUTH_HANDOFF_CODE_TTL_SECS must be between 30 and 300");
        }

        let db_config = config
            .db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("browser handoff requires AUTH_DATABASE_URL"))?;
        let mut options = ConnectOptions::new(db_config.url.clone());
        options
            .max_connections(2)
            .min_connections(1)
            .connect_timeout(Duration::from_secs(5))
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(300))
            .sqlx_logging(false);
        let db = Database::connect(options).await?;

        Ok(Some(Self {
            db,
            clients: Arc::new(clients),
            cipher: Arc::new(cipher),
            code_ttl: Duration::from_secs(code_ttl_seconds),
        }))
    }

    pub fn validate_authorization(
        &self,
        request: AuthorizationRequest,
    ) -> Result<ValidatedAuthorization, AuthError> {
        let client = self
            .clients
            .get(&request.client_id)
            .ok_or(AuthError::BadRequest("unknown browser client"))?;
        if request.client_id.len() > 128
            || request.redirect_uri.len() > MAX_AUTH_VALUE_BYTES
            || request.return_to.len() > MAX_AUTH_VALUE_BYTES
            || request.state.len() > MAX_AUTH_VALUE_BYTES
            || request.code_challenge.len() > MAX_AUTH_VALUE_BYTES
        {
            return Err(AuthError::BadRequest("authorization parameter is too long"));
        }
        if !client.redirect_uris.contains(&request.redirect_uri) {
            return Err(AuthError::BadRequest("redirect_uri is not registered"));
        }
        if !client.return_paths.contains(&request.return_to) {
            return Err(AuthError::BadRequest("return_to is not registered"));
        }
        if request.code_challenge_method != "S256"
            || request.code_challenge.len() != 43
            || !is_base64url(&request.code_challenge)
        {
            return Err(AuthError::BadRequest("PKCE S256 challenge is required"));
        }
        if !(16..=256).contains(&request.state.len()) || !is_base64url(&request.state) {
            return Err(AuthError::BadRequest("state must be opaque base64url"));
        }
        Ok(ValidatedAuthorization {
            client_id: client.client_id.clone(),
            redirect_uri: request.redirect_uri,
            return_to: request.return_to,
            state: request.state,
            code_challenge: request.code_challenge,
            supabase_project: client.supabase_project.clone(),
        })
    }

    pub async fn sign_in_and_issue(
        &self,
        http: &reqwest::Client,
        registry: &ProjectRegistry,
        projects: &[SupabaseProject],
        authorization: &ValidatedAuthorization,
        email: &str,
        password: &str,
    ) -> Result<String, IssueError> {
        let email = email.trim();
        if email.is_empty() || email.len() > 320 || password.is_empty() || password.len() > 1024 {
            return Err(IssueError::InvalidCredentials);
        }
        let project = projects
            .iter()
            .find(|project| project.name == authorization.supabase_project)
            .ok_or(AuthError::Unavailable)?;
        let publishable_key = project
            .api_keys
            .publishable_key
            .as_deref()
            .ok_or(AuthError::Unavailable)?;
        let endpoint = format!(
            "https://{}.supabase.co/auth/v1/token?grant_type=password",
            project.project_ref
        );
        let response = http
            .post(endpoint)
            .header("apikey", publishable_key)
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, project = %project.name, "Supabase browser sign-in failed");
                AuthError::Upstream
            })?;
        if matches!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNAUTHORIZED
        ) {
            return Err(IssueError::InvalidCredentials);
        }
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), project = %project.name, "Supabase browser sign-in returned an error");
            return Err(AuthError::Upstream.into());
        }
        let response: SupabaseTokenResponse = response.json().await.map_err(|error| {
            tracing::warn!(%error, project = %project.name, "Supabase browser sign-in returned invalid JSON");
            AuthError::Upstream
        })?;
        let verified = registry.verify(http, &response.access_token).await?;
        if verified.project != project.name
            || verified.supabase_user_id != response.user.id.to_string()
        {
            tracing::error!(project = %project.name, "Supabase browser sign-in identity mismatch");
            return Err(AuthError::Unauthorized.into());
        }

        let expires_at = response
            .expires_at
            .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
            .unwrap_or_else(|| Utc::now() + TimeDelta::seconds(response.expires_in.max(1)));
        let tokens = SupabaseHandoffTokens {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_at,
            user: response.user,
            return_to: authorization.return_to.clone(),
            supabase_project: authorization.supabase_project.clone(),
        };
        let code = random_code();
        let code_hash = token_hash(&code);
        let aad = handoff_aad(&authorization.client_id, &code_hash);
        let encrypted_tokens = self.cipher.encrypt(&tokens, &aad)?;
        let expires_at = Utc::now()
            + TimeDelta::seconds(self.code_ttl.as_secs().try_into().unwrap_or(i64::MAX));
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO shared_auth.browser_authorization_codes
                     (code_hash, client_id, redirect_uri, return_path, supabase_project,
                      code_challenge, encrypted_tokens, expires_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                vec![
                    code_hash.into(),
                    authorization.client_id.clone().into(),
                    authorization.redirect_uri.clone().into(),
                    authorization.return_to.clone().into(),
                    authorization.supabase_project.clone().into(),
                    authorization.code_challenge.clone().into(),
                    encrypted_tokens.into(),
                    expires_at.fixed_offset().into(),
                ],
            ))
            .await
            .map_err(|error| {
                tracing::error!(%error, "storing browser authorization code failed");
                AuthError::Upstream
            })?;

        let mut redirect = Url::parse(&authorization.redirect_uri).map_err(|_| AuthError::Internal)?;
        redirect
            .query_pairs_mut()
            .append_pair("code", &code)
            .append_pair("state", &authorization.state);
        Ok(redirect.to_string())
    }

    pub async fn redeem(
        &self,
        request: RedeemAuthorizationCode,
        provided_secret: &str,
    ) -> Result<SupabaseHandoffTokens, AuthError> {
        let client = self
            .clients
            .get(&request.client_id)
            .ok_or(AuthError::Unauthorized)?;
        if !constant_time_secret_eq(&client.client_secret, provided_secret) {
            return Err(AuthError::Unauthorized);
        }
        if !client.redirect_uris.contains(&request.redirect_uri)
            || !request.code.starts_with(CODE_PREFIX)
            || request.code.len() > 128
            || !(43..=128).contains(&request.code_verifier.len())
            || !is_base64url(&request.code_verifier)
        {
            return Err(AuthError::Unauthorized);
        }
        let code_hash = token_hash(&request.code);
        let verifier_challenge = pkce_challenge(&request.code_verifier);
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE shared_auth.browser_authorization_codes
                      SET consumed_at = now()
                    WHERE code_hash = $1
                      AND client_id = $2
                      AND redirect_uri = $3
                      AND consumed_at IS NULL
                      AND expires_at > now()
                      AND code_challenge = $4
                  RETURNING encrypted_tokens, supabase_project"#,
                vec![
                    code_hash.clone().into(),
                    request.client_id.clone().into(),
                    request.redirect_uri.into(),
                    verifier_challenge.into(),
                ],
            ))
            .await
            .map_err(|error| {
                tracing::error!(%error, "redeeming browser authorization code failed");
                AuthError::Upstream
            })?
            .ok_or(AuthError::Unauthorized)?;
        let encrypted_tokens: String = row
            .try_get("", "encrypted_tokens")
            .map_err(|_| AuthError::Internal)?;
        let project: String = row
            .try_get("", "supabase_project")
            .map_err(|_| AuthError::Internal)?;
        if project != client.supabase_project {
            tracing::error!(client_id = %request.client_id, "browser authorization project mismatch");
            return Err(AuthError::Unauthorized);
        }
        let aad = handoff_aad(&request.client_id, &code_hash);
        self.cipher.decrypt(&encrypted_tokens, &aad)
    }
}
