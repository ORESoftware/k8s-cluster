use super::{AuthContext, AuthProvider, AuthProviderError, AuthTokens, CredentialSource};
use crate::{
    db::{
        begin_user_transaction,
        entity::{user_profile, web_session},
    },
    error::AppError,
};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration as ChronoDuration, Utc};
use sea_orm::{
    sea_query::OnConflict, ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait,
    QuerySelect, TransactionTrait,
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct SessionService {
    db: DatabaseConnection,
    auth: Arc<dyn AuthProvider>,
    cipher: Aes256Gcm,
    ttl: Duration,
}

pub struct CreatedSession {
    pub raw_id: String,
    pub csrf_token: String,
    pub context: AuthContext,
}

impl SessionService {
    pub fn new(
        db: DatabaseConnection,
        auth: Arc<dyn AuthProvider>,
        encryption_key: &[u8],
        ttl: Duration,
    ) -> Result<Self, AppError> {
        let cipher = Aes256Gcm::new_from_slice(encryption_key).map_err(|_| AppError::Crypto)?;
        Ok(Self {
            db,
            auth,
            cipher,
            ttl,
        })
    }

    pub async fn create(&self, tokens: AuthTokens) -> Result<CreatedSession, AppError> {
        let raw_id = random_token();
        let id_hash = hash_token(&raw_id);
        let csrf_token = random_token();
        let now = Utc::now();
        let expires_at = now
            + ChronoDuration::from_std(self.ttl)
                .map_err(|_| AppError::BadRequest("session TTL is too large".into()))?;
        let email = tokens.user.email.clone().unwrap_or_default();
        let transaction = begin_user_transaction(&self.db, tokens.user.id).await?;

        web_session::ActiveModel {
            id_hash: Set(id_hash.clone()),
            user_id: Set(tokens.user.id),
            email: Set(email.clone()),
            supabase_session_id: Set(None),
            encrypted_access_token: Set(self.encrypt(&tokens.access_token)?),
            encrypted_refresh_token: Set(self.encrypt(&tokens.refresh_token)?),
            access_expires_at: Set(tokens.expires_at),
            csrf_token: Set(csrf_token.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            expires_at: Set(expires_at),
            revoked_at: Set(None),
        }
        .insert(&transaction)
        .await?;

        user_profile::Entity::insert(user_profile::ActiveModel {
            user_id: Set(tokens.user.id),
            email: Set(email.clone()),
            display_name: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(
            OnConflict::column(user_profile::Column::UserId)
                .update_columns([user_profile::Column::Email, user_profile::Column::UpdatedAt])
                .to_owned(),
        )
        .exec(&transaction)
        .await?;
        transaction.commit().await?;

        Ok(CreatedSession {
            raw_id,
            csrf_token: csrf_token.clone(),
            context: AuthContext {
                user_id: tokens.user.id,
                email,
                source: CredentialSource::SessionCookie,
                session_hash: Some(id_hash),
                csrf_token: Some(csrf_token),
                expires_at,
            },
        })
    }

    pub async fn authenticate(&self, raw_id: &str) -> Result<AuthContext, AppError> {
        let id_hash = hash_token(raw_id);
        self.authenticate_hash(&id_hash).await
    }

    pub async fn revalidate(&self, context: &AuthContext) -> Result<(), AppError> {
        match context.session_hash.as_deref() {
            Some(id_hash) => {
                self.authenticate_hash(id_hash).await?;
                Ok(())
            }
            None if context.expires_at > Utc::now() => Ok(()),
            None => Err(AppError::Unauthorized),
        }
    }

    async fn authenticate_hash(&self, id_hash: &str) -> Result<AuthContext, AppError> {
        let model = web_session::Entity::find_by_id(id_hash)
            .one(&self.db)
            .await?
            .ok_or(AppError::Unauthorized)?;
        self.validate_model(&model)?;

        if model.access_expires_at <= Utc::now() + ChronoDuration::seconds(60) {
            return self.refresh_locked(id_hash).await;
        }
        Ok(context_from_model(model))
    }

    pub async fn authenticate_bearer(&self, token: &str) -> Result<AuthContext, AppError> {
        let user = self
            .auth
            .user_for_token(token)
            .await
            .map_err(map_auth_error)?;
        Ok(AuthContext {
            user_id: user.id,
            email: user.email.unwrap_or_default(),
            source: CredentialSource::Bearer,
            session_hash: None,
            csrf_token: None,
            // `/user` authenticates the token. The decoded `exp` is used only
            // to bound a WebSocket lifetime, never as proof of identity.
            expires_at: bearer_expiry(token)
                .unwrap_or_else(|| Utc::now() + ChronoDuration::minutes(5)),
        })
    }

    pub async fn revoke(&self, raw_id: &str) -> Result<(), AppError> {
        let id_hash = hash_token(raw_id);
        let transaction = self.db.begin().await?;
        let model = web_session::Entity::find_by_id(&id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?;
        let Some(model) = model else {
            transaction.commit().await?;
            return Ok(());
        };

        let access_token = self.decrypt(&model.encrypted_access_token).ok();
        let mut active: web_session::ActiveModel = model.into();
        active.revoked_at = Set(Some(Utc::now()));
        active.updated_at = Set(Utc::now());
        active.update(&transaction).await?;
        transaction.commit().await?;

        if let Some(access_token) = access_token {
            if let Err(error) = self.auth.sign_out(&access_token).await {
                tracing::warn!(%error, "Supabase logout failed after local session revocation");
            }
        }
        Ok(())
    }

    async fn refresh_locked(&self, id_hash: &str) -> Result<AuthContext, AppError> {
        let transaction = self.db.begin().await?;
        let model = web_session::Entity::find_by_id(id_hash)
            .lock_exclusive()
            .one(&transaction)
            .await?
            .ok_or(AppError::Unauthorized)?;
        self.validate_model(&model)?;

        if model.access_expires_at > Utc::now() + ChronoDuration::seconds(60) {
            transaction.commit().await?;
            return Ok(context_from_model(model));
        }

        let refresh_token = self.decrypt(&model.encrypted_refresh_token)?;
        let tokens = match self.auth.refresh(&refresh_token).await {
            Ok(tokens) => tokens,
            Err(AuthProviderError::InvalidCredentials) => {
                let now = Utc::now();
                let mut active: web_session::ActiveModel = model.into();
                active.revoked_at = Set(Some(now));
                active.updated_at = Set(now);
                active.update(&transaction).await?;
                transaction.commit().await?;
                return Err(AppError::Unauthorized);
            }
            Err(error) => {
                transaction.rollback().await?;
                return Err(map_auth_error(error));
            }
        };
        if tokens.user.id != model.user_id {
            return Err(AppError::Unauthorized);
        }

        let mut active: web_session::ActiveModel = model.into();
        active.email = Set(tokens.user.email.clone().unwrap_or_default());
        active.encrypted_access_token = Set(self.encrypt(&tokens.access_token)?);
        active.encrypted_refresh_token = Set(self.encrypt(&tokens.refresh_token)?);
        active.access_expires_at = Set(tokens.expires_at);
        active.updated_at = Set(Utc::now());
        let updated = active.update(&transaction).await?;
        transaction.commit().await?;
        Ok(context_from_model(updated))
    }

    fn validate_model(&self, model: &web_session::Model) -> Result<(), AppError> {
        if model.revoked_at.is_some() || model.expires_at <= Utc::now() {
            Err(AppError::Unauthorized)
        } else {
            Ok(())
        }
    }

    fn encrypt(&self, plaintext: &str) -> Result<String, AppError> {
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AppError::Crypto)?;
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| AppError::Crypto)?;
        let mut encoded = nonce_bytes.to_vec();
        encoded.extend(ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(encoded))
    }

    fn decrypt(&self, encoded: &str) -> Result<String, AppError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| AppError::Crypto)?;
        let (nonce_bytes, ciphertext) = bytes.split_at_checked(12).ok_or(AppError::Crypto)?;
        let nonce = nonce_bytes.try_into().map_err(|_| AppError::Crypto)?;
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| AppError::Crypto)?;
        String::from_utf8(plaintext).map_err(|_| AppError::Crypto)
    }
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn hash_token(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn context_from_model(model: web_session::Model) -> AuthContext {
    AuthContext {
        user_id: model.user_id,
        email: model.email,
        source: CredentialSource::SessionCookie,
        session_hash: Some(model.id_hash),
        csrf_token: Some(model.csrf_token),
        expires_at: model.expires_at,
    }
}

fn map_auth_error(error: AuthProviderError) -> AppError {
    match error {
        AuthProviderError::InvalidCredentials => AppError::Unauthorized,
        AuthProviderError::Unavailable | AuthProviderError::InvalidResponse => {
            AppError::AuthUpstream
        }
    }
}

fn bearer_expiry(token: &str) -> Option<chrono::DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let timestamp = value.get("exp")?.as_i64()?;
    chrono::TimeZone::timestamp_opt(&Utc, timestamp, 0).single()
}
