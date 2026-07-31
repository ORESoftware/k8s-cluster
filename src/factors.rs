//! Durable MFA factor enrollment, OTP step-up, and WebAuthn/passkey ceremonies.
//!
//! All ceremony state is server-owned and consumed exactly once. TOTP seeds are
//! encrypted before persistence. WebAuthn stores only public credentials and
//! opaque server-side ceremony state; face and fingerprint templates never
//! leave the platform authenticator.

use std::sync::Arc;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use hmac::{Hmac, Mac};
use rand::{rngs::SysRng, TryRng};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::Sha256;
use url::Url;
use uuid::Uuid;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Webauthn, WebauthnBuilder,
};

use crate::config::DbConfig;
use crate::error::AuthError;
use crate::state::AppState;
use crate::token::OreClaims;

use crate::http::bearer;
use crate::http::introspect::active_claims;
use crate::http::session_tokens;

const OTP_TTL_MINUTES: i64 = 10;
const PASSKEY_TTL_MINUTES: i64 = 5;
const TOTP_STEP_SECONDS: u64 = 30;
const TOTP_DIGITS: u32 = 1_000_000;
const MAX_OTP_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct FactorService {
    db: Arc<DatabaseConnection>,
    totp_key: Option<[u8; 32]>,
    webauthn: Option<Arc<Webauthn>>,
}

impl FactorService {
    pub async fn connect(config: &DbConfig) -> anyhow::Result<Self> {
        let mut options = ConnectOptions::new(config.url.clone());
        options
            .max_connections(config.max_connections.min(3).max(1))
            .min_connections(1)
            .connect_timeout(std::time::Duration::from_secs(5))
            .acquire_timeout(std::time::Duration::from_secs(5))
            .idle_timeout(std::time::Duration::from_secs(300))
            .sqlx_logging(false);
        let db = Database::connect(options).await?;
        let totp_key = optional_hex_key("AUTH_FACTOR_ENCRYPTION_KEY_HEX")?;
        let webauthn = build_webauthn()?;
        Ok(Self {
            db: Arc::new(db),
            totp_key,
            webauthn,
        })
    }

    fn supports_totp(&self) -> bool {
        self.totp_key.is_some()
    }

    fn supports_passkeys(&self) -> bool {
        self.webauthn.is_some()
    }

    async fn list_factors(&self, user_id: Uuid) -> Result<Vec<Factor>, AuthError> {
        let rows = self
            .db
            .query_all(statement(
                "SELECT factor_id, kind, label, enabled, confirmed_at, last_used_at, created_at FROM shared_auth.auth_factors WHERE shared_user_id = $1 ORDER BY created_at ASC",
                vec![user_id.into()],
            ))
            .await
            .map_err(db_error)?;
        rows.iter().map(factor_from_row).collect()
    }

    async fn delete_factor(&self, user_id: Uuid, factor_id: Uuid) -> Result<(), AuthError> {
        let row = self
            .db
            .query_one(statement(
                "SELECT enabled FROM shared_auth.auth_factors WHERE shared_user_id = $1 AND factor_id = $2",
                vec![user_id.into(), factor_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or(AuthError::BadRequest("unknown factor"))?;
        let enabled: bool = row.try_get("", "enabled").map_err(db_error)?;
        if enabled {
            let count = self
                .db
                .query_one(statement(
                    "SELECT count(*)::bigint AS count FROM shared_auth.auth_factors WHERE shared_user_id = $1 AND enabled = true",
                    vec![user_id.into()],
                ))
                .await
                .map_err(db_error)?
                .ok_or(AuthError::Internal)?;
            let count: i64 = count.try_get("", "count").map_err(db_error)?;
            if count <= 1 {
                return Err(AuthError::Conflict);
            }
        }
        let result = self
            .db
            .execute(statement(
                "DELETE FROM shared_auth.auth_factors WHERE shared_user_id = $1 AND factor_id = $2",
                vec![user_id.into(), factor_id.into()],
            ))
            .await
            .map_err(db_error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AuthError::BadRequest("unknown factor"))
        }
    }

    async fn enroll_totp(
        &self,
        user_id: Uuid,
        account_name: &str,
        label: Option<&str>,
    ) -> Result<TotpEnrollment, AuthError> {
        let key = self.totp_key.ok_or(AuthError::Unavailable)?;
        let label = normalize_label(label)?;
        let mut secret = [0u8; 20];
        SysRng.try_fill_bytes(&mut secret).map_err(|_| AuthError::Internal)?;
        let mut nonce = [0u8; 12];
        SysRng.try_fill_bytes(&mut nonce).map_err(|_| AuthError::Internal)?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| AuthError::Internal)?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), secret.as_ref())
            .map_err(|_| AuthError::Internal)?;
        let factor_id = Uuid::new_v4();
        let public_data = json!({"algorithm":"SHA1","digits":6,"period":TOTP_STEP_SECONDS,"last_counter":-1});
        self.db
            .execute(statement(
                "INSERT INTO shared_auth.auth_factors (factor_id, shared_user_id, kind, label, secret_ciphertext, secret_nonce, public_data) VALUES ($1, $2, 'totp', $3, $4, $5, $6)",
                vec![factor_id.into(), user_id.into(), label.clone().into(), ciphertext.into(), nonce.to_vec().into(), public_data.into()],
            ))
            .await
            .map_err(db_error)?;

        let secret_base32 = encode_base32(&secret);
        let issuer = "OreSoftware";
        let account = if account_name.trim().is_empty() { user_id.to_string() } else { account_name.trim().to_owned() };
        let path_label = percent_encode(&format!("{issuer}:{account}"));
        let issuer_query = percent_encode(issuer);
        let otpauth_uri = format!("otpauth://totp/{path_label}?secret={secret_base32}&issuer={issuer_query}&algorithm=SHA1&digits=6&period={TOTP_STEP_SECONDS}");
        Ok(TotpEnrollment { factor_id: factor_id.to_string(), secret_base32, threefa_import_uri: otpauth_uri.clone(), otpauth_uri })
    }

    async fn confirm_totp(&self, user_id: Uuid, factor_id: Uuid, code: &str) -> Result<(), AuthError> {
        validate_otp(code)?;
        let key = self.totp_key.ok_or(AuthError::Unavailable)?;
        let row = self.db.query_one(statement(
            "SELECT secret_ciphertext, secret_nonce, coalesce((public_data ->> 'last_counter')::bigint, -1) AS last_counter FROM shared_auth.auth_factors WHERE factor_id = $1 AND shared_user_id = $2 AND kind = 'totp'",
            vec![factor_id.into(), user_id.into()],
        )).await.map_err(db_error)?.ok_or(AuthError::Unauthorized)?;
        let ciphertext: Vec<u8> = row.try_get("", "secret_ciphertext").map_err(db_error)?;
        let nonce: Vec<u8> = row.try_get("", "secret_nonce").map_err(db_error)?;
        let last_counter: i64 = row.try_get("", "last_counter").map_err(db_error)?;
        if nonce.len() != 12 { return Err(AuthError::Internal); }
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| AuthError::Internal)?;
        let secret = cipher.decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref()).map_err(|_| AuthError::Internal)?;
        let current = now_secs() / TOTP_STEP_SECONDS;
        let matched = [current.saturating_sub(1), current, current.saturating_add(1)]
            .into_iter()
            .find(|counter| (*counter as i64) > last_counter && constant_time_code_eq(&totp_code(&secret, *counter), code, &secret))
            .ok_or(AuthError::Unauthorized)?;
        let result = self.db.execute(statement(
            "UPDATE shared_auth.auth_factors SET enabled = true, confirmed_at = coalesce(confirmed_at, now()), last_used_at = now(), updated_at = now(), public_data = jsonb_set(public_data, '{last_counter}', to_jsonb($3::bigint), true) WHERE factor_id = $1 AND shared_user_id = $2 AND kind = 'totp' AND coalesce((public_data ->> 'last_counter')::bigint, -1) < $3",
            vec![factor_id.into(), user_id.into(), (matched as i64).into()],
        )).await.map_err(db_error)?;
        if result.rows_affected() == 1 { Ok(()) } else { Err(AuthError::Unauthorized) }
    }

    async fn start_passkey_registration(&self, claims: &OreClaims, label: Option<&str>) -> Result<CeremonyStart, AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let existing = self.passkeys_for(user_id).await?;
        let exclude = (!existing.is_empty()).then(|| existing.iter().map(|(_, passkey)| passkey.cred_id().clone()).collect());
        let username = claims.email.as_deref().filter(|value| !value.trim().is_empty()).unwrap_or(&claims.sub);
        let display_name = normalize_label(label)?.unwrap_or_else(|| username.to_owned());
        let (options, registration) = webauthn.start_passkey_registration(user_id, username, &display_name, exclude).map_err(|error| {
            tracing::warn!(error = %error, "passkey registration start failed");
            AuthError::BadRequest("unable to start passkey registration")
        })?;
        let state = serde_json::to_value(&registration).map_err(|_| AuthError::Internal)?;
        let expires_at = Utc::now().fixed_offset() + TimeDelta::minutes(PASSKEY_TTL_MINUTES);
        let challenge_id = self.insert_challenge(user_id, session_id, "passkey_register", None, state, 1, expires_at).await?;
        Ok(CeremonyStart { challenge_id: challenge_id.to_string(), options: serde_json::to_value(options).map_err(|_| AuthError::Internal)?, expires_at: expires_at.to_rfc3339() })
    }

    async fn finish_passkey_registration(&self, claims: &OreClaims, challenge_id: Uuid, credential: Value, label: Option<&str>) -> Result<Factor, AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let state = self.take_challenge(user_id, session_id, challenge_id, "passkey_register").await?;
        let registration: PasskeyRegistration = serde_json::from_value(state).map_err(|_| AuthError::Internal)?;
        let external_id = credential_id(&credential)?;
        let response: RegisterPublicKeyCredential = serde_json::from_value(credential).map_err(|_| AuthError::BadRequest("invalid passkey credential"))?;
        let passkey = webauthn.finish_passkey_registration(&response, &registration).map_err(|error| {
            tracing::info!(error = %error, "passkey registration rejected");
            AuthError::Unauthorized
        })?;
        let factor_id = Uuid::new_v4();
        let public_data = serde_json::to_value(passkey).map_err(|_| AuthError::Internal)?;
        let label = normalize_label(label)?;
        let row = self.db.query_one(statement(
            "INSERT INTO shared_auth.auth_factors (factor_id, shared_user_id, kind, label, public_data, external_id, enabled, confirmed_at) VALUES ($1, $2, 'passkey', $3, $4, $5, true, now()) RETURNING factor_id, kind, label, enabled, confirmed_at, last_used_at, created_at",
            vec![factor_id.into(), user_id.into(), label.into(), public_data.into(), external_id.into()],
        )).await.map_err(|error| { tracing::info!(%error, "duplicate or invalid passkey registration"); AuthError::Conflict })?.ok_or(AuthError::Internal)?;
        factor_from_row(&row)
    }

    async fn start_passkey_authentication(&self, claims: &OreClaims) -> Result<CeremonyStart, AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let stored = self.passkeys_for(user_id).await?;
        if stored.is_empty() { return Err(AuthError::BadRequest("no passkeys are enrolled")); }
        let passkeys = stored.into_iter().map(|(_, passkey)| passkey).collect::<Vec<_>>();
        let (options, authentication) = webauthn.start_passkey_authentication(&passkeys).map_err(|error| {
            tracing::warn!(error = %error, "passkey authentication start failed");
            AuthError::BadRequest("unable to start passkey authentication")
        })?;
        let state = serde_json::to_value(authentication).map_err(|_| AuthError::Internal)?;
        let expires_at = Utc::now().fixed_offset() + TimeDelta::minutes(PASSKEY_TTL_MINUTES);
        let challenge_id = self.insert_challenge(user_id, session_id, "passkey_auth", None, state, 1, expires_at).await?;
        Ok(CeremonyStart { challenge_id: challenge_id.to_string(), options: serde_json::to_value(options).map_err(|_| AuthError::Internal)?, expires_at: expires_at.to_rfc3339() })
    }

    async fn finish_passkey_authentication(&self, claims: &OreClaims, challenge_id: Uuid, credential: Value) -> Result<(), AuthError> {
        let webauthn = self.webauthn.as_ref().ok_or(AuthError::Unavailable)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let state = self.take_challenge(user_id, session_id, challenge_id, "passkey_auth").await?;
        let authentication: PasskeyAuthentication = serde_json::from_value(state).map_err(|_| AuthError::Internal)?;
        let external_id = credential_id(&credential)?;
        let response: PublicKeyCredential = serde_json::from_value(credential).map_err(|_| AuthError::BadRequest("invalid passkey credential"))?;
        let result = webauthn.finish_passkey_authentication(&response, &authentication).map_err(|error| {
            tracing::info!(error = %error, "passkey authentication rejected");
            AuthError::Unauthorized
        })?;
        let mut stored = self.passkey_by_external_id(user_id, &external_id).await?;
        let _ = stored.update_credential(&result);
        let public_data = serde_json::to_value(stored).map_err(|_| AuthError::Internal)?;
        let result = self.db.execute(statement(
            "UPDATE shared_auth.auth_factors SET public_data = $3, last_used_at = now(), updated_at = now() WHERE shared_user_id = $1 AND external_id = $2 AND kind = 'passkey' AND enabled = true",
            vec![user_id.into(), external_id.into(), public_data.into()],
        )).await.map_err(db_error)?;
        if result.rows_affected() == 1 { Ok(()) } else { Err(AuthError::Unauthorized) }
    }

    async fn passkeys_for(&self, user_id: Uuid) -> Result<Vec<(String, Passkey)>, AuthError> {
        let rows = self.db.query_all(statement(
            "SELECT external_id, public_data FROM shared_auth.auth_factors WHERE shared_user_id = $1 AND kind = 'passkey' AND enabled = true",
            vec![user_id.into()],
        )).await.map_err(db_error)?;
        rows.into_iter().map(|row| {
            let external_id: String = row.try_get("", "external_id").map_err(db_error)?;
            let data: Value = row.try_get("", "public_data").map_err(db_error)?;
            let passkey = serde_json::from_value(data).map_err(|_| AuthError::Internal)?;
            Ok((external_id, passkey))
        }).collect()
    }

    async fn passkey_by_external_id(&self, user_id: Uuid, external_id: &str) -> Result<Passkey, AuthError> {
        let row = self.db.query_one(statement(
            "SELECT public_data FROM shared_auth.auth_factors WHERE shared_user_id = $1 AND kind = 'passkey' AND external_id = $2 AND enabled = true",
            vec![user_id.into(), external_id.to_owned().into()],
        )).await.map_err(db_error)?.ok_or(AuthError::Unauthorized)?;
        let data: Value = row.try_get("", "public_data").map_err(db_error)?;
        serde_json::from_value(data).map_err(|_| AuthError::Internal)
    }

    async fn insert_challenge(&self, user_id: Uuid, session_id: Uuid, kind: &str, code_tag: Option<Vec<u8>>, state: Value, max_attempts: i32, expires_at: DateTime<FixedOffset>) -> Result<Uuid, AuthError> {
        let challenge_id = Uuid::new_v4();
        self.db.execute(statement(
            "INSERT INTO shared_auth.auth_challenges (challenge_id, shared_user_id, session_id, kind, code_tag, state, max_attempts, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            vec![challenge_id.into(), user_id.into(), session_id.into(), kind.to_owned().into(), code_tag.into(), state.into(), max_attempts.into(), expires_at.into()],
        )).await.map_err(db_error)?;
        Ok(challenge_id)
    }

    async fn take_challenge(&self, user_id: Uuid, session_id: Uuid, challenge_id: Uuid, kind: &str) -> Result<Value, AuthError> {
        let row = self.db.query_one(statement(
            "UPDATE shared_auth.auth_challenges SET consumed_at = now(), attempts = attempts + 1 WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 AND kind = $4 AND consumed_at IS NULL AND expires_at > now() AND attempts < max_attempts RETURNING state",
            vec![challenge_id.into(), user_id.into(), session_id.into(), kind.to_owned().into()],
        )).await.map_err(db_error)?.ok_or(AuthError::Unauthorized)?;
        row.try_get("", "state").map_err(db_error)
    }

    async fn create_otp_challenge(&self, claims: &OreClaims, kind: ChallengeKind, pepper: &[u8]) -> Result<(ChallengeStart, String, String), AuthError> {
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let code = generate_code()?;
        let expires_at = Utc::now().fixed_offset() + TimeDelta::minutes(OTP_TTL_MINUTES);
        let (db_kind, destination, delivery) = match kind {
            ChallengeKind::EmailOtp => {
                if !claims.email_verified { return Err(AuthError::Forbidden); }
                let email = claims.email.clone().filter(|value| !value.trim().is_empty()).ok_or(AuthError::BadRequest("verified email is required"))?;
                ("email_otp", email, "email")
            }
            ChallengeKind::SmsOtp => {
                let phone = self.verified_phone(user_id).await?;
                ("sms_otp", phone, "sms")
            }
        };
        let challenge_id = Uuid::new_v4();
        let tag = otp_tag(pepper, challenge_id, &code)?;
        self.db.execute(statement(
            "INSERT INTO shared_auth.auth_challenges (challenge_id, shared_user_id, session_id, kind, destination_hint, code_tag, state, max_attempts, expires_at) VALUES ($1, $2, $3, $4, $5, $6, '{}'::jsonb, $7, $8)",
            vec![challenge_id.into(), user_id.into(), session_id.into(), db_kind.to_owned().into(), mask_destination(&destination).into(), tag.into(), MAX_OTP_ATTEMPTS.into(), expires_at.into()],
        )).await.map_err(db_error)?;
        Ok((ChallengeStart { challenge_id: challenge_id.to_string(), expires_at: expires_at.to_rfc3339(), delivery: delivery.to_owned() }, destination, code))
    }

    async fn verify_otp_challenge(&self, claims: &OreClaims, challenge_id: Uuid, code: &str, pepper: &[u8], externally_verified: bool) -> Result<&'static str, AuthError> {
        validate_otp(code)?;
        let user_id = claim_user_id(claims)?;
        let session_id = claim_session_id(claims)?;
        let row = self.db.query_one(statement(
            "SELECT kind, code_tag FROM shared_auth.auth_challenges WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 AND kind IN ('email_otp', 'sms_otp') AND consumed_at IS NULL AND expires_at > now() AND attempts < max_attempts",
            vec![challenge_id.into(), user_id.into(), session_id.into()],
        )).await.map_err(db_error)?.ok_or(AuthError::Unauthorized)?;
        let kind: String = row.try_get("", "kind").map_err(db_error)?;
        let expected: Vec<u8> = row.try_get("", "code_tag").map_err(db_error)?;
        let presented = otp_tag(pepper, challenge_id, code)?;
        if !externally_verified && !constant_time_bytes_eq(&expected, &presented, pepper) {
            self.db.execute(statement("UPDATE shared_auth.auth_challenges SET attempts = attempts + 1 WHERE challenge_id = $1 AND consumed_at IS NULL", vec![challenge_id.into()])).await.map_err(db_error)?;
            return Err(AuthError::Unauthorized);
        }
        let result = self.db.execute(statement("UPDATE shared_auth.auth_challenges SET consumed_at = now(), attempts = attempts + 1 WHERE challenge_id = $1 AND consumed_at IS NULL", vec![challenge_id.into()])).await.map_err(db_error)?;
        if result.rows_affected() != 1 { return Err(AuthError::Unauthorized); }
        match kind.as_str() { "email_otp" => Ok("email_otp"), "sms_otp" => Ok("sms_otp"), _ => Err(AuthError::Unauthorized) }
    }

    async fn verified_phone(&self, user_id: Uuid) -> Result<String, AuthError> {
        let row = self.db.query_one(statement(
            "SELECT phone FROM shared_auth.principals WHERE shared_user_id = $1 AND status = 'active' AND phone_verified = true",
            vec![user_id.into()],
        )).await.map_err(db_error)?.ok_or(AuthError::BadRequest("verified phone is required"))?;
        let phone: Option<String> = row.try_get("", "phone").map_err(db_error)?;
        phone.ok_or(AuthError::BadRequest("verified phone is required"))
    }
}

#[derive(Serialize)]
pub struct Capabilities { mfa_enabled: bool, methods: Vec<String>, #[serde(skip_serializing_if = "Option::is_none")] threefa_import_scheme: Option<&'static str>, #[serde(skip_serializing_if = "Option::is_none")] biometric_model: Option<&'static str> }
#[derive(Serialize)]
pub struct Factor { factor_id: String, kind: String, #[serde(skip_serializing_if = "Option::is_none")] label: Option<String>, enabled: bool, #[serde(skip_serializing_if = "Option::is_none")] confirmed_at: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] last_used_at: Option<String>, created_at: String }
#[derive(Deserialize)]
pub struct TotpEnrollRequest { #[serde(default)] label: Option<String> }
#[derive(Serialize)]
pub struct TotpEnrollment { factor_id: String, secret_base32: String, otpauth_uri: String, threefa_import_uri: String }
#[derive(Deserialize)]
pub struct TotpConfirmRequest { factor_id: String, code: String }
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeKind { EmailOtp, SmsOtp }
#[derive(Deserialize)]
pub struct ChallengeRequest { kind: ChallengeKind }
#[derive(Serialize)]
pub struct ChallengeStart { challenge_id: String, expires_at: String, delivery: String }
#[derive(Deserialize)]
pub struct ChallengeVerifyRequest { code: String }
#[derive(Serialize)]
pub struct CeremonyStart { challenge_id: String, options: Value, expires_at: String }
#[derive(Deserialize)]
pub struct PasskeyStartRequest { #[serde(default)] label: Option<String> }
#[derive(Deserialize)]
pub struct PasskeyFinishRequest { challenge_id: String, credential: Value, #[serde(default)] label: Option<String> }
#[derive(Serialize)]
pub struct StepUpResponse { access_token: String, token_type: &'static str, expires_at: u64, amr: Vec<String>, #[serde(skip_serializing_if = "Option::is_none")] acr: Option<String> }

pub async fn capabilities(State(state): State<AppState>) -> Json<Capabilities> {
    let factors = state.factors.as_ref();
    let mut methods = Vec::new();
    if state.config.magic_links.is_enabled() { methods.push("email_otp".to_owned()); }
    if state.config.twilio_verify.is_enabled() { methods.push("sms_otp".to_owned()); }
    if factors.is_some_and(FactorService::supports_totp) { methods.push("totp".to_owned()); }
    if factors.is_some_and(FactorService::supports_passkeys) { methods.push("passkey".to_owned()); }
    Json(Capabilities { mfa_enabled: !methods.is_empty(), threefa_import_scheme: methods.iter().any(|method| method == "totp").then_some("otpauth"), biometric_model: methods.iter().any(|method| method == "passkey").then_some("platform_authenticator_webauthn"), methods })
}

pub async fn list(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<Factor>>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(service.list_factors(claim_user_id(&claims)?).await?))
}

pub async fn delete(State(state): State<AppState>, headers: HeaderMap, Path(raw_factor_id): Path<String>) -> Result<StatusCode, AuthError> {
    let claims = claims(&state, &headers).await?;
    let factor_id = parse_uuid(&raw_factor_id, "invalid factor id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    service.delete_factor(claim_user_id(&claims)?, factor_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn enroll_totp(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<TotpEnrollRequest>) -> Result<(StatusCode, Json<TotpEnrollment>), AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    let account = claims.email.as_deref().unwrap_or(&claims.sub);
    let enrollment = service.enroll_totp(claim_user_id(&claims)?, account, request.label.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(enrollment)))
}

pub async fn confirm_totp(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<TotpConfirmRequest>) -> Result<Json<StepUpResponse>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let factor_id = parse_uuid(&request.factor_id, "invalid factor id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    service.confirm_totp(claim_user_id(&claims)?, factor_id, &request.code).await?;
    Ok(Json(step_up(&state, &claims, "totp")?))
}

pub async fn create_challenge(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<ChallengeRequest>) -> Result<(StatusCode, Json<ChallengeStart>), AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    let pepper = state.config.magic_links.otp_pepper.as_deref().ok_or(AuthError::Unavailable)?;
    let (response, destination, code) = service.create_otp_challenge(&claims, request.kind, pepper.as_bytes()).await?;
    match request.kind {
        ChallengeKind::EmailOtp => send_email_otp(&state, &destination, &code).await?,
        ChallengeKind::SmsOtp => {
            if !state.config.twilio_verify.is_enabled() { return Err(AuthError::Unavailable); }
            crate::twilio::start_sms_verification(&state.http, &state.config.twilio_verify, &destination).await?;
        }
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}

pub async fn verify_challenge(State(state): State<AppState>, headers: HeaderMap, Path(raw_challenge_id): Path<String>, Json(request): Json<ChallengeVerifyRequest>) -> Result<Json<StepUpResponse>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let challenge_id = parse_uuid(&raw_challenge_id, "invalid challenge id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    let pepper = state.config.magic_links.otp_pepper.as_deref().ok_or(AuthError::Unavailable)?;
    let row = service.db.query_one(statement(
        "SELECT kind FROM shared_auth.auth_challenges WHERE challenge_id = $1 AND shared_user_id = $2 AND session_id = $3 AND consumed_at IS NULL AND expires_at > now()",
        vec![challenge_id.into(), claim_user_id(&claims)?.into(), claim_session_id(&claims)?.into()],
    )).await.map_err(db_error)?.ok_or(AuthError::Unauthorized)?;
    let kind: String = row.try_get("", "kind").map_err(db_error)?;
    if kind == "sms_otp" {
        let phone = service.verified_phone(claim_user_id(&claims)?).await?;
        let valid = crate::twilio::check_sms_verification(&state.http, &state.config.twilio_verify, &phone, &request.code).await?;
        if !valid { return Err(AuthError::Unauthorized); }
    }
    let method = service.verify_otp_challenge(&claims, challenge_id, &request.code, pepper.as_bytes(), kind == "sms_otp").await?;
    Ok(Json(step_up(&state, &claims, method)?))
}

pub async fn start_passkey_registration(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<PasskeyStartRequest>) -> Result<Json<CeremonyStart>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(service.start_passkey_registration(&claims, request.label.as_deref()).await?))
}

pub async fn finish_passkey_registration(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<PasskeyFinishRequest>) -> Result<Json<Factor>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let challenge_id = parse_uuid(&request.challenge_id, "invalid challenge id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(service.finish_passkey_registration(&claims, challenge_id, request.credential, request.label.as_deref()).await?))
}

pub async fn start_passkey_authentication(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<CeremonyStart>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    Ok(Json(service.start_passkey_authentication(&claims).await?))
}

pub async fn finish_passkey_authentication(State(state): State<AppState>, headers: HeaderMap, Json(request): Json<PasskeyFinishRequest>) -> Result<Json<StepUpResponse>, AuthError> {
    let claims = claims(&state, &headers).await?;
    let challenge_id = parse_uuid(&request.challenge_id, "invalid challenge id")?;
    let service = state.factors.as_ref().ok_or(AuthError::Unavailable)?;
    service.finish_passkey_authentication(&claims, challenge_id, request.credential).await?;
    Ok(Json(step_up(&state, &claims, "passkey")?))
}

async fn claims(state: &AppState, headers: &HeaderMap) -> Result<OreClaims, AuthError> { active_claims(state, bearer(headers).ok_or(AuthError::Unauthorized)?).await }
fn step_up(state: &AppState, claims: &OreClaims, method: &str) -> Result<StepUpResponse, AuthError> {
    let minted = session_tokens::mint_step_up(state, claims, method)?;
    Ok(StepUpResponse { access_token: minted.token, token_type: "Bearer", expires_at: minted.expires_at, amr: minted.amr, acr: minted.acr })
}

async fn send_email_otp(state: &AppState, recipient: &str, code: &str) -> Result<(), AuthError> {
    let config = &state.config.magic_links;
    let api_key = config.sendgrid_api_key.as_deref().ok_or(AuthError::Unavailable)?;
    let from_email = config.from_email.as_deref().ok_or(AuthError::Unavailable)?;
    let payload = json!({
        "personalizations": [{"to": [{"email": recipient}]}],
        "from": {"email": from_email, "name": config.from_name},
        "subject": "Your verification code",
        "content": [
            {"type": "text/plain", "value": format!("Your one-time verification code is {code}. It expires in {OTP_TTL_MINUTES} minutes.")},
            {"type": "text/html", "value": format!("<p>Your one-time verification code is <strong>{code}</strong>.</p><p>It expires in {OTP_TTL_MINUTES} minutes.</p>")}
        ]
    });
    let response = state.http.post("https://api.sendgrid.com/v3/mail/send").bearer_auth(api_key).json(&payload).send().await.map_err(|error| {
        tracing::warn!(%error, "SendGrid OTP request failed"); AuthError::Upstream
    })?;
    if response.status() == reqwest::StatusCode::ACCEPTED { Ok(()) } else { tracing::warn!(status = response.status().as_u16(), "SendGrid rejected OTP email"); Err(AuthError::Upstream) }
}

fn factor_from_row(row: &sea_orm::QueryResult) -> Result<Factor, AuthError> {
    let factor_id: Uuid = row.try_get("", "factor_id").map_err(db_error)?;
    let kind: String = row.try_get("", "kind").map_err(db_error)?;
    let label: Option<String> = row.try_get("", "label").map_err(db_error)?;
    let enabled: bool = row.try_get("", "enabled").map_err(db_error)?;
    let confirmed_at: Option<DateTime<FixedOffset>> = row.try_get("", "confirmed_at").map_err(db_error)?;
    let last_used_at: Option<DateTime<FixedOffset>> = row.try_get("", "last_used_at").map_err(db_error)?;
    let created_at: DateTime<FixedOffset> = row.try_get("", "created_at").map_err(db_error)?;
    Ok(Factor { factor_id: factor_id.to_string(), kind, label, enabled, confirmed_at: confirmed_at.map(|value| value.to_rfc3339()), last_used_at: last_used_at.map(|value| value.to_rfc3339()), created_at: created_at.to_rfc3339() })
}

fn claim_user_id(claims: &OreClaims) -> Result<Uuid, AuthError> { Uuid::parse_str(&claims.sub).map_err(|_| AuthError::Unauthorized) }
fn claim_session_id(claims: &OreClaims) -> Result<Uuid, AuthError> { claims.sid.as_deref().ok_or(AuthError::Unauthorized).and_then(|value| Uuid::parse_str(value).map_err(|_| AuthError::Unauthorized)) }
fn parse_uuid(value: &str, message: &'static str) -> Result<Uuid, AuthError> { Uuid::parse_str(value).map_err(|_| AuthError::BadRequest(message)) }
fn credential_id(credential: &Value) -> Result<String, AuthError> { credential.get("id").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty() && value.len() <= 2048).map(str::to_owned).ok_or(AuthError::BadRequest("invalid passkey credential id")) }
fn normalize_label(label: Option<&str>) -> Result<Option<String>, AuthError> { let label = label.map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned); if label.as_ref().is_some_and(|value| value.len() > 160) { Err(AuthError::BadRequest("factor label is too long")) } else { Ok(label) } }
fn validate_otp(code: &str) -> Result<(), AuthError> { if code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()) { Ok(()) } else { Err(AuthError::BadRequest("verification code must contain six digits")) } }
fn generate_code() -> Result<String, AuthError> { let mut bytes = [0u8; 4]; SysRng.try_fill_bytes(&mut bytes).map_err(|_| AuthError::Internal)?; Ok(format!("{:06}", u32::from_be_bytes(bytes) % TOTP_DIGITS)) }
fn totp_code(secret: &[u8], counter: u64) -> String {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret).expect("HMAC accepts arbitrary TOTP secret lengths");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24) | (u32::from(digest[offset + 1]) << 16) | (u32::from(digest[offset + 2]) << 8) | u32::from(digest[offset + 3]);
    format!("{:06}", binary % TOTP_DIGITS)
}
fn otp_tag(key: &[u8], challenge_id: Uuid, code: &str) -> Result<Vec<u8>, AuthError> { let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| AuthError::Internal)?; mac.update(challenge_id.as_bytes()); mac.update(code.as_bytes()); Ok(mac.finalize().into_bytes().to_vec()) }
fn constant_time_code_eq(expected: &str, presented: &str, key: &[u8]) -> bool { constant_time_bytes_eq(expected.as_bytes(), presented.as_bytes(), key) }
fn constant_time_bytes_eq(expected: &[u8], presented: &[u8], key: &[u8]) -> bool { let tag = |value: &[u8]| { let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts arbitrary comparison key lengths"); mac.update(value); mac.finalize().into_bytes() }; tag(expected) == tag(presented) }
fn encode_base32(input: &[u8]) -> String { const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"; let mut output = String::new(); let mut buffer = 0u32; let mut bits = 0u8; for byte in input { buffer = (buffer << 8) | u32::from(*byte); bits += 8; while bits >= 5 { bits -= 5; output.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char); } } if bits > 0 { output.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char); } output }
fn percent_encode(value: &str) -> String { url::form_urlencoded::byte_serialize(value.as_bytes()).collect() }
fn mask_destination(value: &str) -> String { if let Some((local, domain)) = value.split_once('@') { let prefix = local.chars().next().unwrap_or('•'); return format!("{prefix}•••@{domain}"); } let suffix = value.chars().rev().take(4).collect::<Vec<_>>(); format!("••••{}", suffix.into_iter().rev().collect::<String>()) }
fn optional_hex_key(name: &'static str) -> anyhow::Result<Option<[u8; 32]>> { let Some(raw) = std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()) else { return Ok(None); }; if raw.len() != 64 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) { anyhow::bail!("{name} must contain exactly 64 hexadecimal characters"); } let mut key = [0u8; 32]; for (index, pair) in raw.as_bytes().chunks_exact(2).enumerate() { key[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?; } Ok(Some(key)) }
fn hex_nibble(value: u8) -> anyhow::Result<u8> { match value { b'0'..=b'9' => Ok(value - b'0'), b'a'..=b'f' => Ok(value - b'a' + 10), b'A'..=b'F' => Ok(value - b'A' + 10), _ => anyhow::bail!("invalid hexadecimal digit") } }
fn build_webauthn() -> anyhow::Result<Option<Arc<Webauthn>>> { let rp_id = optional_env("AUTH_WEBAUTHN_RP_ID"); let origin = optional_env("AUTH_WEBAUTHN_RP_ORIGIN"); let rp_name = optional_env("AUTH_WEBAUTHN_RP_NAME"); match (rp_id, origin, rp_name) { (None, None, None) => Ok(None), (Some(rp_id), Some(origin), Some(rp_name)) => { let origin = Url::parse(&origin)?; let builder = WebauthnBuilder::new(&rp_id, &origin)?.rp_name(&rp_name); Ok(Some(Arc::new(builder.build()?))) }, _ => anyhow::bail!("AUTH_WEBAUTHN_RP_ID, AUTH_WEBAUTHN_RP_ORIGIN, and AUTH_WEBAUTHN_RP_NAME must be set together") } }
fn optional_env(name: &'static str) -> Option<String> { std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty()) }
fn now_secs() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or(0) }
fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement { Statement::from_sql_and_values(DbBackend::Postgres, sql, values) }
fn db_error(error: impl std::fmt::Display) -> AuthError { tracing::warn!(%error, "factor database operation failed"); AuthError::Upstream }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn base32_matches_rfc4648_vectors_without_padding() { assert_eq!(encode_base32(b"foo"), "MZXW6"); assert_eq!(encode_base32(b"foobar"), "MZXW6YTBOI"); }
    #[test] fn rfc6238_sha1_vector_is_correct() { let secret = b"12345678901234567890"; assert_eq!(totp_code(secret, 59 / 30), "287082"); }
    #[test] fn destination_masks_do_not_disclose_the_full_value() { assert_eq!(mask_destination("alex@example.com"), "a•••@example.com"); assert_eq!(mask_destination("+14155550100"), "••••0100"); }
    #[test] fn invalid_factor_key_is_rejected() { assert!(hex_nibble(b'g').is_err()); }
}
