//! Privacy-preserving account-recovery orchestration.
//!
//! A signed-in user can enroll government-ID/face proofing and an optional
//! Voxletra voice reference. Recovery later runs short-lived provider ceremonies
//! and requires document authenticity, face match + liveness, and a random voice
//! challenge with liveness/anti-replay. Speaker comparison is advisory only and
//! never authorizes recovery. Shared-auth stores only opaque provider references
//! and normalized verdicts; raw biometric media and reusable templates remain
//! with providers.

mod config;
mod provider;
mod store;

use std::sync::Arc;

use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, FixedOffset, TimeDelta};
use hmac::{Hmac, KeyInit, Mac};
use rand::{rngs::SysRng, TryRng};
use serde::Serialize;
use sha2::Sha256;
use uuid::Uuid;

use crate::config::DbConfig;
use crate::error::AuthError;
use crate::session::hash_token;

use self::config::RecoveryConfig;
use self::provider::{
    IdentityClient, IdentityVerification, ProviderMode, ProviderStatus, VoiceClient,
    VoiceVerification,
};
use self::store::{
    CeremonyRecord, EvidenceSnapshot, ManualReviewDecision, NewCeremony, RecoveryStore,
};

pub const RECOVERY_TOKEN_PREFIX: &str = "sat_recovery_";
const RECOVERY_TOKEN_BYTES: usize = 32;
const RECOVERY_TOKEN_LEN: usize = RECOVERY_TOKEN_PREFIX.len() + 43;
const ENROLLMENT_DAILY_LIMIT: i64 = 3;
const RECOVERY_DAILY_LIMIT: i64 = 3;

#[derive(Clone)]
pub struct RecoveryService {
    config: Arc<RecoveryConfig>,
    store: RecoveryStore,
    identity: IdentityClient,
    voice: VoiceClient,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecoveryCapabilities {
    pub enabled: bool,
    pub consent_version: String,
    pub government_id_required: bool,
    pub face_match_required: bool,
    pub face_liveness_required: bool,
    pub voice_liveness_required: bool,
    pub voice_phrase_required: bool,
    pub voice_speaker_match_advisory_only: bool,
    pub automatic_recovery_requires_prior_identity_proofing: bool,
    pub raw_biometrics_stored_by_shared_auth: bool,
    pub cooldown_seconds: u64,
    pub manual_review_available: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct CeremonyLaunch {
    pub ceremony_id: Uuid,
    pub ceremony_token: String,
    pub expires_at: u64,
    pub identity_capture_url: String,
    pub voice_capture_url: String,
    pub voice_challenge_phrase: String,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicCeremonyStatus {
    Pending,
    PendingReview,
    Cooldown,
    Ready,
    Rejected,
    Enrolled,
    Consumed,
    Expired,
}

#[derive(Clone, Debug, Serialize)]
pub struct CeremonyView {
    pub ceremony_id: Uuid,
    pub status: PublicCeremonyStatus,
    pub expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvaluationDecision {
    Pending,
    PendingReview(&'static str),
    Rejected(&'static str),
    Cooldown,
    EnrollmentComplete,
}

struct CeremonySecret {
    plaintext: String,
    hash: String,
}

impl RecoveryService {
    pub async fn from_env(
        db: Option<&DbConfig>,
        http: reqwest::Client,
    ) -> anyhow::Result<Option<Self>> {
        let Some(config) = RecoveryConfig::from_env().context("loading account recovery config")?
        else {
            return Ok(None);
        };
        let db = db.context("account recovery requires AUTH_DATABASE_URL")?;
        let store = RecoveryStore::connect(db)
            .await
            .context("connecting account recovery store")?;
        let identity = IdentityClient::new(http.clone(), &config);
        let voice = VoiceClient::new(http, &config);
        Ok(Some(Self {
            config: Arc::new(config),
            store,
            identity,
            voice,
        }))
    }

    pub fn capabilities(&self) -> RecoveryCapabilities {
        RecoveryCapabilities {
            enabled: true,
            consent_version: self.config.consent_version.clone(),
            government_id_required: true,
            face_match_required: true,
            face_liveness_required: true,
            voice_liveness_required: true,
            voice_phrase_required: true,
            voice_speaker_match_advisory_only: true,
            automatic_recovery_requires_prior_identity_proofing: true,
            raw_biometrics_stored_by_shared_auth: false,
            cooldown_seconds: self.config.cooldown_secs,
            manual_review_available: self.config.review_secret.is_some(),
        }
    }

    pub fn validate_consent(
        &self,
        accepted_biometric_processing: bool,
        consent_version: &str,
    ) -> Result<(), AuthError> {
        if !accepted_biometric_processing || consent_version != self.config.consent_version {
            return Err(AuthError::BadRequest(
                "current biometric-processing consent is required",
            ));
        }
        Ok(())
    }

    pub async fn begin_enrollment(
        &self,
        shared_user_id: Uuid,
        consent_version: &str,
    ) -> Result<CeremonyLaunch, AuthError> {
        if !self.store.active_user(shared_user_id).await? {
            return Err(AuthError::Unauthorized);
        }
        let identifier_hash = self.identifier_hash(shared_user_id.to_string().as_bytes());
        self.store
            .enforce_daily_limit(&identifier_hash, "enrollment", ENROLLMENT_DAILY_LIMIT)
            .await?;
        let ceremony_id = Uuid::new_v4();
        let secret = generate_secret();
        let subject_reference = self.user_subject_reference(shared_user_id);
        let (identity, voice) = tokio::try_join!(
            self.identity.create_session(
                ProviderMode::Enroll,
                &subject_reference,
                ceremony_id,
                self.config.ceremony_ttl_secs,
            ),
            self.voice.create_session(
                ProviderMode::Enroll,
                &subject_reference,
                ceremony_id,
                self.config.ceremony_ttl_secs,
            ),
        )?;
        let expires_at = launch_expiry(
            identity.expires_at,
            voice.expires_at,
            self.config.ceremony_ttl_secs,
        )?;
        self.store
            .insert_ceremony(NewCeremony {
                ceremony_id,
                purpose: "enrollment",
                shared_user_id: Some(shared_user_id),
                identifier_hash,
                ceremony_secret_hash: secret.hash,
                identity_session_id: identity.session_id,
                voice_session_id: voice.session_id,
                identity_binding_present: false,
                requires_manual_review: false,
                consent_version: consent_version.to_owned(),
                expires_at,
            })
            .await?;
        Ok(CeremonyLaunch {
            ceremony_id,
            ceremony_token: secret.plaintext,
            expires_at: expires_at.timestamp() as u64,
            identity_capture_url: identity.capture_url,
            voice_capture_url: voice.capture_url,
            voice_challenge_phrase: voice.challenge_phrase,
        })
    }

    pub async fn begin_recovery(
        &self,
        normalized_email: &str,
        consent_version: &str,
    ) -> Result<CeremonyLaunch, AuthError> {
        let identifier_hash = self.identifier_hash(normalized_email.as_bytes());
        self.store
            .enforce_daily_limit(&identifier_hash, "recovery", RECOVERY_DAILY_LIMIT)
            .await?;
        let account = self.store.account_for_email(normalized_email).await?;
        let ceremony_id = Uuid::new_v4();
        let secret = generate_secret();

        let identity_binding_present = account
            .as_ref()
            .is_some_and(|account| account.identity_reference_id.is_some());
        let shared_user_id = account.as_ref().map(|account| account.shared_user_id);
        let fallback_subject = match account.as_ref() {
            Some(account) => self.user_subject_reference(account.shared_user_id),
            None => self.decoy_subject_reference(&identifier_hash, ceremony_id),
        };
        let identity_reference = account
            .as_ref()
            .and_then(|account| account.identity_reference_id.clone());
        let voice_reference = account
            .as_ref()
            .and_then(|account| account.voice_reference_id.clone());
        let identity_mode = match (&account, &identity_reference) {
            (_, Some(_)) => ProviderMode::Verify,
            (Some(_), None) => ProviderMode::Enroll,
            (None, None) => ProviderMode::Decoy,
        };
        let voice_mode = if voice_reference.is_some() {
            ProviderMode::Verify
        } else {
            // Voice comparison is advisory only, so recovery never creates a
            // persistent voice reference. Enrollment is restricted to the
            // authenticated AAL2 enrollment flow.
            ProviderMode::Decoy
        };
        let identity_subject = identity_reference.unwrap_or_else(|| fallback_subject.clone());
        let voice_subject = voice_reference.unwrap_or(fallback_subject);

        // Unknown and unenrolled accounts receive real, short-lived capture
        // sessions too. Unknown accounts use the providers' non-retaining decoy
        // mode. The response shape and timing therefore do not reveal account
        // existence; only the server-side decision can approve recovery.
        let (identity, voice) = tokio::try_join!(
            self.identity.create_session(
                identity_mode,
                &identity_subject,
                ceremony_id,
                self.config.ceremony_ttl_secs,
            ),
            self.voice.create_session(
                voice_mode,
                &voice_subject,
                ceremony_id,
                self.config.ceremony_ttl_secs,
            ),
        )?;
        let expires_at = launch_expiry(
            identity.expires_at,
            voice.expires_at,
            self.config.ceremony_ttl_secs,
        )?;
        self.store
            .insert_ceremony(NewCeremony {
                ceremony_id,
                purpose: "recovery",
                shared_user_id,
                identifier_hash,
                ceremony_secret_hash: secret.hash,
                identity_session_id: identity.session_id,
                voice_session_id: voice.session_id,
                identity_binding_present,
                requires_manual_review: self.config.always_manual_review || !identity_binding_present,
                consent_version: consent_version.to_owned(),
                expires_at,
            })
            .await?;
        Ok(CeremonyLaunch {
            ceremony_id,
            ceremony_token: secret.plaintext,
            expires_at: expires_at.timestamp() as u64,
            identity_capture_url: identity.capture_url,
            voice_capture_url: voice.capture_url,
            voice_challenge_phrase: voice.challenge_phrase,
        })
    }

    pub async fn ceremony_status(
        &self,
        ceremony_id: Uuid,
        ceremony_token: &str,
        expected_purpose: &str,
        expected_user: Option<Uuid>,
    ) -> Result<CeremonyView, AuthError> {
        let token_hash = validate_and_hash_token(ceremony_token)?;
        let mut record = self.store.load_ceremony(ceremony_id, &token_hash).await?;
        authorize_record(&record, expected_purpose, expected_user)?;
        if should_expire(&record) {
            self.store.mark_expired(ceremony_id, &token_hash).await?;
            record = self.store.load_ceremony(ceremony_id, &token_hash).await?;
        }
        view_from_record(&record)
    }

    pub async fn evaluate(
        &self,
        ceremony_id: Uuid,
        ceremony_token: &str,
        expected_purpose: &str,
        expected_user: Option<Uuid>,
    ) -> Result<CeremonyView, AuthError> {
        let token_hash = validate_and_hash_token(ceremony_token)?;
        let mut record = self.store.load_ceremony(ceremony_id, &token_hash).await?;
        authorize_record(&record, expected_purpose, expected_user)?;
        if should_expire(&record) {
            self.store.mark_expired(ceremony_id, &token_hash).await?;
            record = self.store.load_ceremony(ceremony_id, &token_hash).await?;
            return view_from_record(&record);
        }
        if record.status != "pending" {
            return view_from_record(&record);
        }
        self.store
            .record_evaluation_attempt(ceremony_id, &token_hash)
            .await?;

        let (identity, voice) = tokio::try_join!(
            self.identity.status(&record.identity_session_id),
            self.voice.status(&record.voice_session_id),
        )?;
        let mut evidence = evidence_from(&identity, &voice);
        if record.shared_user_id.is_none() {
            // Decoy ceremonies must not retain provider references even if a
            // misconfigured provider returns one.
            evidence.identity_reference_id = None;
            evidence.voice_reference_id = None;
        }
        match evaluate_results(&self.config, &record, &identity, &voice) {
            EvaluationDecision::Pending => {
                self.store
                    .save_evidence(
                        ceremony_id,
                        &token_hash,
                        "pending",
                        "providers_pending",
                        None,
                        None,
                        &evidence,
                    )
                    .await?;
            }
            EvaluationDecision::PendingReview(reason) => {
                let review_expires_at = chrono::Utc::now().fixed_offset()
                    + TimeDelta::seconds(self.config.redeem_ttl_secs as i64);
                self.store
                    .save_evidence(
                        ceremony_id,
                        &token_hash,
                        "pending_review",
                        reason,
                        None,
                        Some(review_expires_at),
                        &evidence,
                    )
                    .await?;
            }
            EvaluationDecision::Rejected(reason) => {
                self.store
                    .save_evidence(
                        ceremony_id,
                        &token_hash,
                        "rejected",
                        reason,
                        None,
                        None,
                        &evidence,
                    )
                    .await?;
            }
            EvaluationDecision::Cooldown => {
                let available_at = chrono::Utc::now().fixed_offset()
                    + TimeDelta::seconds(self.config.cooldown_secs as i64);
                let expires_at = available_at
                    + TimeDelta::seconds(self.config.redeem_ttl_secs as i64);
                self.store
                    .save_evidence(
                        ceremony_id,
                        &token_hash,
                        "cooldown",
                        "automatic_verification_passed",
                        Some(available_at),
                        Some(expires_at),
                        &evidence,
                    )
                    .await?;
            }
            EvaluationDecision::EnrollmentComplete => {
                let user = expected_user.ok_or(AuthError::Unauthorized)?;
                self.store
                    .complete_enrollment(ceremony_id, &token_hash, user, &evidence)
                    .await?;
            }
        }
        record = self.store.load_ceremony(ceremony_id, &token_hash).await?;
        view_from_record(&record)
    }

    pub async fn redeem(
        &self,
        ceremony_id: Uuid,
        ceremony_token: &str,
        new_password: String,
    ) -> Result<(), AuthError> {
        validate_new_password(&new_password)?;
        let token_hash = validate_and_hash_token(ceremony_token)?;
        let record = self.store.load_ceremony(ceremony_id, &token_hash).await?;
        authorize_record(&record, "recovery", None)?;
        if view_from_record(&record)?.status != PublicCeremonyStatus::Ready {
            return Err(AuthError::Conflict);
        }
        // Argon2 work happens after the cheap token/state checks but before the
        // transaction, so random requests cannot force password hashing and a
        // legitimate hash never holds a database row lock.
        let password_hash = crate::password::hash(new_password).await?;
        self.store
            .redeem(ceremony_id, &token_hash, &password_hash)
            .await
    }

    pub async fn revoke_enrollment(&self, shared_user_id: Uuid) -> Result<(), AuthError> {
        self.store.revoke_binding(shared_user_id).await
    }

    pub fn authorize_reviewer(&self, presented: Option<&str>) -> Result<(), AuthError> {
        let expected = self
            .config
            .review_secret
            .as_deref()
            .ok_or(AuthError::Unavailable)?;
        match presented {
            Some(presented) if constant_time_eq(expected, presented) => Ok(()),
            _ => Err(AuthError::Unauthorized),
        }
    }

    pub async fn review(
        &self,
        ceremony_id: Uuid,
        decision: ReviewDecision,
        reviewer: &str,
    ) -> Result<(), AuthError> {
        validate_reviewer(reviewer)?;
        self.store
            .apply_manual_review(
                ceremony_id,
                match decision {
                    ReviewDecision::Approve => ManualReviewDecision::Approve,
                    ReviewDecision::Reject => ManualReviewDecision::Reject,
                },
                reviewer,
                self.config.cooldown_secs,
                self.config.redeem_ttl_secs,
            )
            .await
    }
}

fn authorize_record(
    record: &CeremonyRecord,
    expected_purpose: &str,
    expected_user: Option<Uuid>,
) -> Result<(), AuthError> {
    if record.purpose != expected_purpose {
        return Err(AuthError::Unauthorized);
    }
    if expected_user.is_some() && record.shared_user_id != expected_user {
        return Err(AuthError::Unauthorized);
    }
    Ok(())
}

fn evidence_from(
    identity: &IdentityVerification,
    voice: &VoiceVerification,
) -> EvidenceSnapshot {
    EvidenceSnapshot {
        identity_result_id: identity.result_id.clone(),
        voice_result_id: voice.result_id.clone(),
        identity_reference_id: identity.reference_id.clone(),
        voice_reference_id: voice.reference_id.clone(),
        document_verified: identity.document_verified,
        document_confidence: identity.document_confidence,
        face_match: identity.face_match,
        face_liveness: identity.face_liveness,
        face_confidence: identity.face_confidence,
        advisory_speaker_match: voice.speaker_match,
        voice_liveness: voice.liveness,
        phrase_match: voice.phrase_match,
        voice_liveness_confidence: voice.liveness_confidence,
        advisory_speaker_confidence: voice.speaker_confidence,
    }
}

fn evaluate_results(
    config: &RecoveryConfig,
    record: &CeremonyRecord,
    identity: &IdentityVerification,
    voice: &VoiceVerification,
) -> EvaluationDecision {
    if identity.status == ProviderStatus::Pending || voice.status == ProviderStatus::Pending {
        return EvaluationDecision::Pending;
    }
    if matches!(
        identity.status,
        ProviderStatus::Failed | ProviderStatus::Expired
    ) || matches!(voice.status, ProviderStatus::Failed | ProviderStatus::Expired)
    {
        return EvaluationDecision::Rejected("provider_rejected");
    }
    if identity.status == ProviderStatus::Review || voice.status == ProviderStatus::Review {
        return if record.purpose == "recovery" {
            EvaluationDecision::PendingReview("provider_review")
        } else {
            EvaluationDecision::Rejected("provider_review")
        };
    }

    let identity_ok = identity.status == ProviderStatus::Passed
        && identity.document_verified == Some(true)
        && identity.face_match == Some(true)
        && identity.face_liveness == Some(true)
        && meets(identity.document_confidence, config.document_threshold)
        && meets(identity.face_confidence, config.face_threshold);
    // Voice analysis is deliberately non-authoritative for identity. It can
    // establish a live, challenge-responsive human and provide an advisory
    // speaker-comparison signal, but it never grants or denies recovery.
    let voice_challenge_ok = voice.status == ProviderStatus::Passed
        && voice.liveness == Some(true)
        && voice.phrase_match == Some(true)
        && meets(
            voice.liveness_confidence,
            config.voice_liveness_threshold,
        );
    let identity_reference_ok =
        record.identity_binding_present || identity.reference_id.is_some();

    if !(identity_ok && voice_challenge_ok && identity_reference_ok) {
        return if record.purpose == "recovery" {
            EvaluationDecision::PendingReview("confidence_or_signal_inconclusive")
        } else {
            EvaluationDecision::Rejected("confidence_or_signal_inconclusive")
        };
    }

    if record.purpose == "enrollment" {
        return EvaluationDecision::EnrollmentComplete;
    }
    if record.shared_user_id.is_none() {
        return EvaluationDecision::PendingReview("manual_review_required");
    }
    if record.requires_manual_review {
        EvaluationDecision::PendingReview("manual_review_required")
    } else {
        EvaluationDecision::Cooldown
    }
}

fn meets(value: Option<f64>, threshold: f64) -> bool {
    value.is_some_and(|value| value.is_finite() && value >= threshold)
}

fn should_expire(record: &CeremonyRecord) -> bool {
    matches!(record.status.as_str(), "pending" | "pending_review")
        && record.expires_at <= chrono::Utc::now().fixed_offset()
}

fn view_from_record(record: &CeremonyRecord) -> Result<CeremonyView, AuthError> {
    let now = chrono::Utc::now().fixed_offset();
    let status = match record.status.as_str() {
        "pending" => PublicCeremonyStatus::Pending,
        "pending_review" => PublicCeremonyStatus::PendingReview,
        "cooldown" if record.expires_at <= now => PublicCeremonyStatus::Expired,
        "cooldown" if record.available_at.is_some_and(|available_at| available_at <= now) => {
            PublicCeremonyStatus::Ready
        }
        "cooldown" => PublicCeremonyStatus::Cooldown,
        "rejected" => PublicCeremonyStatus::Rejected,
        "enrolled" => PublicCeremonyStatus::Enrolled,
        "consumed" => PublicCeremonyStatus::Consumed,
        "expired" => PublicCeremonyStatus::Expired,
        _ => return Err(AuthError::Internal),
    };
    Ok(CeremonyView {
        ceremony_id: record.ceremony_id,
        status,
        expires_at: record.expires_at.timestamp().max(0) as u64,
        available_at: record
            .available_at
            .map(|available_at| available_at.timestamp().max(0) as u64),
    })
}

fn launch_expiry(
    identity_expires_at: u64,
    voice_expires_at: u64,
    configured_ttl: u64,
) -> Result<DateTime<FixedOffset>, AuthError> {
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let expires_at = identity_expires_at
        .min(voice_expires_at)
        .min(now + configured_ttl);
    chrono::DateTime::<chrono::Utc>::from_timestamp(expires_at as i64, 0)
        .map(|value| value.fixed_offset())
        .ok_or(AuthError::Upstream)
}

fn generate_secret() -> CeremonySecret {
    let mut entropy = [0_u8; RECOVERY_TOKEN_BYTES];
    SysRng
        .try_fill_bytes(&mut entropy)
        .expect("operating system randomness is required for recovery ceremonies");
    let plaintext = format!("{RECOVERY_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(entropy));
    let hash = hash_token(&plaintext);
    CeremonySecret { plaintext, hash }
}

fn validate_and_hash_token(token: &str) -> Result<String, AuthError> {
    if token.len() != RECOVERY_TOKEN_LEN || !token.starts_with(RECOVERY_TOKEN_PREFIX) {
        return Err(AuthError::Unauthorized);
    }
    Ok(hash_token(token))
}

fn validate_new_password(password: &str) -> Result<(), AuthError> {
    if !(12..=1024).contains(&password.len()) {
        return Err(AuthError::BadRequest(
            "password must be between 12 and 1024 bytes",
        ));
    }
    Ok(())
}

fn validate_reviewer(reviewer: &str) -> Result<(), AuthError> {
    if reviewer.is_empty()
        || reviewer.len() > 128
        || !reviewer.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@' | b':')
        })
    {
        return Err(AuthError::BadRequest("invalid reviewer identifier"));
    }
    Ok(())
}

fn constant_time_eq(expected: &str, presented: &str) -> bool {
    let mut key = [0_u8; 32];
    if SysRng.try_fill_bytes(&mut key).is_err() {
        return false;
    }
    let tag = |value: &[u8]| {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key)
            .expect("HMAC accepts any key length");
        mac.update(value);
        mac.finalize().into_bytes()
    };
    tag(expected.as_bytes()) == tag(presented.as_bytes())
}

impl RecoveryService {
    fn identifier_hash(&self, value: &[u8]) -> String {
        self.keyed_digest(b"identifier", value)
    }

    fn user_subject_reference(&self, shared_user_id: Uuid) -> String {
        self.subject_reference(b"user", shared_user_id.as_bytes())
    }

    fn decoy_subject_reference(&self, identifier_hash: &str, ceremony_id: Uuid) -> String {
        let mut data = Vec::with_capacity(identifier_hash.len() + 17);
        data.extend_from_slice(identifier_hash.as_bytes());
        data.push(0);
        data.extend_from_slice(ceremony_id.as_bytes());
        self.subject_reference(b"decoy", &data)
    }

    fn subject_reference(&self, domain: &[u8], value: &[u8]) -> String {
        format!("sar_{}", self.keyed_digest(domain, value))
    }

    fn keyed_digest(&self, domain: &[u8], value: &[u8]) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(
            self.config.subject_pepper.as_bytes(),
        )
        .expect("HMAC accepts any key length");
        mac.update(b"shared-auth-recovery-v1");
        mac.update(&[0]);
        mac.update(domain);
        mac.update(&[0]);
        mac.update(value);
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(purpose: &str, identity_binding_present: bool, user: bool) -> CeremonyRecord {
        CeremonyRecord {
            ceremony_id: Uuid::nil(),
            purpose: purpose.to_owned(),
            shared_user_id: user.then(Uuid::nil),
            status: "pending".to_owned(),
            identity_session_id: "identity_012345".to_owned(),
            voice_session_id: "voice_012345678".to_owned(),
            identity_binding_present,
            requires_manual_review: !identity_binding_present,
            consent_version: "2026-08-04".to_owned(),
            expires_at: chrono::Utc::now().fixed_offset() + TimeDelta::minutes(10),
            available_at: None,
            consumed_at: None,
            identity_reference_id: None,
            voice_reference_id: None,
        }
    }

    fn config() -> RecoveryConfig {
        RecoveryConfig {
            identity_base: reqwest::Url::parse("https://identity.example").unwrap(),
            identity_token: "i".repeat(32),
            voxletra_base: reqwest::Url::parse("https://voice.example").unwrap(),
            voxletra_token: "v".repeat(32),
            subject_pepper: "p".repeat(32),
            review_secret: Some("r".repeat(32)),
            ceremony_ttl_secs: 900,
            cooldown_secs: 86_400,
            redeem_ttl_secs: 86_400,
            document_threshold: 0.85,
            face_threshold: 0.90,
            voice_liveness_threshold: 0.90,
            always_manual_review: false,
            consent_version: "2026-08-04".to_owned(),
        }
    }

    fn identity(reference: bool) -> IdentityVerification {
        IdentityVerification {
            session_id: "identity_012345".to_owned(),
            status: ProviderStatus::Passed,
            result_id: Some("identity_result_01".to_owned()),
            reference_id: reference.then(|| "identity_reference_01".to_owned()),
            document_verified: Some(true),
            document_confidence: Some(0.97),
            face_match: Some(true),
            face_liveness: Some(true),
            face_confidence: Some(0.98),
            expires_at: 4_000_000_000,
        }
    }

    fn voice(reference: bool, speaker_match: Option<bool>) -> VoiceVerification {
        VoiceVerification {
            session_id: "voice_012345678".to_owned(),
            status: ProviderStatus::Passed,
            result_id: Some("voice_result_0001".to_owned()),
            reference_id: reference.then(|| "voice_reference_01".to_owned()),
            speaker_match,
            liveness: Some(true),
            phrase_match: Some(true),
            liveness_confidence: Some(0.97),
            speaker_confidence: Some(0.93),
            expires_at: 4_000_000_000,
        }
    }

    #[test]
    fn prior_identity_proofing_can_enter_automatic_cooldown() {
        let record = record("recovery", true, true);
        assert_eq!(
            evaluate_results(&config(), &record, &identity(false), &voice(false, Some(true))),
            EvaluationDecision::Cooldown
        );
    }

    #[test]
    fn bootstrap_recovery_always_requires_manual_review() {
        let record = record("recovery", false, true);
        assert!(matches!(
            evaluate_results(&config(), &record, &identity(true), &voice(true, None)),
            EvaluationDecision::PendingReview(_)
        ));
    }

    #[test]
    fn speaker_match_is_advisory_not_an_authorization_factor() {
        let record = record("recovery", true, true);
        let approved = evaluate_results(
            &config(),
            &record,
            &identity(false),
            &voice(false, Some(true)),
        );
        let mismatch = evaluate_results(
            &config(),
            &record,
            &identity(false),
            &voice(false, Some(false)),
        );
        assert_eq!(approved, EvaluationDecision::Cooldown);
        assert_eq!(mismatch, approved);
    }

    #[test]
    fn unknown_account_never_auto_approves_and_does_not_reveal_existence() {
        let record = record("recovery", false, false);
        assert_eq!(
            evaluate_results(&config(), &record, &identity(true), &voice(true, None)),
            EvaluationDecision::PendingReview("manual_review_required")
        );
    }

    #[test]
    fn recovery_tokens_are_random_and_stored_only_as_hashes() {
        let first = generate_secret();
        let second = generate_secret();
        assert_ne!(first.plaintext, second.plaintext);
        assert_eq!(first.plaintext.len(), RECOVERY_TOKEN_LEN);
        assert_eq!(first.hash.len(), 43);
        assert_eq!(first.hash, hash_token(&first.plaintext));
    }
}
