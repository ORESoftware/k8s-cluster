//! Provider adapters for government-ID/face verification and Voxletra-backed
//! voice analysis. Only opaque references and normalized verdicts cross
//! this boundary; raw images, video, audio, and biometric templates do not.

use std::sync::Arc;

use reqwest::{Response, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AuthError;

use super::config::RecoveryConfig;

const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024;
const MIN_REFERENCE_BYTES: usize = 8;
const MAX_REFERENCE_BYTES: usize = 512;
const MIN_SESSION_ID_BYTES: usize = 8;
const MAX_SESSION_ID_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    Enroll,
    Verify,
    /// Run a non-retaining ceremony for enumeration-resistant unknown accounts.
    Decoy,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Pending,
    Passed,
    Failed,
    Review,
    Expired,
}

#[derive(Serialize)]
struct CreateSessionRequest<'a> {
    mode: ProviderMode,
    subject_reference: &'a str,
    correlation_id: Uuid,
    expires_in_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySessionLaunch {
    pub session_id: String,
    pub capture_url: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceSessionLaunch {
    pub session_id: String,
    pub capture_url: String,
    pub challenge_phrase: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityVerification {
    pub session_id: String,
    pub status: ProviderStatus,
    #[serde(default)]
    pub result_id: Option<String>,
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub document_verified: Option<bool>,
    #[serde(default)]
    pub document_confidence: Option<f64>,
    #[serde(default)]
    pub face_match: Option<bool>,
    #[serde(default)]
    pub face_liveness: Option<bool>,
    #[serde(default)]
    pub face_confidence: Option<f64>,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceVerification {
    pub session_id: String,
    pub status: ProviderStatus,
    #[serde(default)]
    pub result_id: Option<String>,
    #[serde(default)]
    pub reference_id: Option<String>,
    #[serde(default)]
    pub speaker_match: Option<bool>,
    #[serde(default)]
    pub liveness: Option<bool>,
    #[serde(default)]
    pub phrase_match: Option<bool>,
    #[serde(default)]
    pub liveness_confidence: Option<f64>,
    #[serde(default)]
    pub speaker_confidence: Option<f64>,
    pub expires_at: u64,
}

#[derive(Clone)]
pub struct IdentityClient {
    http: reqwest::Client,
    base: Url,
    token: Arc<str>,
}

#[derive(Clone)]
pub struct VoiceClient {
    http: reqwest::Client,
    base: Url,
    token: Arc<str>,
}

impl IdentityClient {
    pub fn new(http: reqwest::Client, config: &RecoveryConfig) -> Self {
        Self {
            http,
            base: config.identity_base.clone(),
            token: Arc::from(config.identity_token.clone()),
        }
    }

    pub async fn create_session(
        &self,
        mode: ProviderMode,
        subject_reference: &str,
        correlation_id: Uuid,
        expires_in_seconds: u64,
    ) -> Result<IdentitySessionLaunch, AuthError> {
        validate_request_reference(subject_reference)?;
        let url = self
            .base
            .join("v1/identity-verification/sessions")
            .map_err(|_| AuthError::Internal)?;
        let request = CreateSessionRequest {
            mode,
            subject_reference,
            correlation_id,
            expires_in_seconds,
        };
        let response = self
            .http
            .post(url)
            .bearer_auth(self.token.as_ref())
            .header("idempotency-key", correlation_id.to_string())
            .json(&request)
            .send()
            .await
            .map_err(provider_transport)?;
        require_created(&response)?;
        let launch = parse_json_body::<IdentitySessionLaunch>(response).await?;
        validate_identity_launch(&launch, expires_in_seconds)?;
        Ok(launch)
    }

    pub async fn status(&self, session_id: &str) -> Result<IdentityVerification, AuthError> {
        validate_request_session_id(session_id)?;
        let url = self
            .base
            .join(&format!("v1/identity-verification/sessions/{session_id}"))
            .map_err(|_| AuthError::Internal)?;
        let response = self
            .http
            .get(url)
            .bearer_auth(self.token.as_ref())
            .send()
            .await
            .map_err(provider_transport)?;
        require_ok(&response)?;
        let status = parse_json_body::<IdentityVerification>(response).await?;
        validate_identity_status(&status, session_id)?;
        Ok(status)
    }
}

impl VoiceClient {
    pub fn new(http: reqwest::Client, config: &RecoveryConfig) -> Self {
        Self {
            http,
            base: config.voxletra_base.clone(),
            token: Arc::from(config.voxletra_token.clone()),
        }
    }

    pub async fn create_session(
        &self,
        mode: ProviderMode,
        subject_reference: &str,
        correlation_id: Uuid,
        expires_in_seconds: u64,
    ) -> Result<VoiceSessionLaunch, AuthError> {
        validate_request_reference(subject_reference)?;
        let url = self
            .base
            .join("v1/voice-verification/sessions")
            .map_err(|_| AuthError::Internal)?;
        let request = CreateSessionRequest {
            mode,
            subject_reference,
            correlation_id,
            expires_in_seconds,
        };
        let response = self
            .http
            .post(url)
            .bearer_auth(self.token.as_ref())
            .header("idempotency-key", correlation_id.to_string())
            .json(&request)
            .send()
            .await
            .map_err(provider_transport)?;
        require_created(&response)?;
        let launch = parse_json_body::<VoiceSessionLaunch>(response).await?;
        validate_voice_launch(&launch, expires_in_seconds)?;
        Ok(launch)
    }

    pub async fn status(&self, session_id: &str) -> Result<VoiceVerification, AuthError> {
        validate_request_session_id(session_id)?;
        let url = self
            .base
            .join(&format!("v1/voice-verification/sessions/{session_id}"))
            .map_err(|_| AuthError::Internal)?;
        let response = self
            .http
            .get(url)
            .bearer_auth(self.token.as_ref())
            .send()
            .await
            .map_err(provider_transport)?;
        require_ok(&response)?;
        let status = parse_json_body::<VoiceVerification>(response).await?;
        validate_voice_status(&status, session_id)?;
        Ok(status)
    }
}

fn require_created(response: &Response) -> Result<(), AuthError> {
    if matches!(
        response.status(),
        reqwest::StatusCode::CREATED | reqwest::StatusCode::ACCEPTED
    ) {
        Ok(())
    } else {
        tracing::warn!(
            status = response.status().as_u16(),
            "biometric provider rejected session creation"
        );
        Err(AuthError::Upstream)
    }
}

fn require_ok(response: &Response) -> Result<(), AuthError> {
    if response.status() == reqwest::StatusCode::OK {
        Ok(())
    } else {
        tracing::warn!(
            status = response.status().as_u16(),
            "biometric provider status request failed"
        );
        Err(AuthError::Upstream)
    }
}

fn provider_transport(_error: reqwest::Error) -> AuthError {
    tracing::warn!("biometric provider transport failed");
    AuthError::Upstream
}

fn validate_identity_launch(
    launch: &IdentitySessionLaunch,
    requested_ttl: u64,
) -> Result<(), AuthError> {
    validate_response_session_id(&launch.session_id)?;
    validate_capture_url(&launch.capture_url)?;
    validate_expiry(launch.expires_at, requested_ttl)
}

fn validate_voice_launch(launch: &VoiceSessionLaunch, requested_ttl: u64) -> Result<(), AuthError> {
    validate_response_session_id(&launch.session_id)?;
    validate_capture_url(&launch.capture_url)?;
    if !(4..=160).contains(&launch.challenge_phrase.len())
        || launch.challenge_phrase.chars().any(char::is_control)
    {
        return Err(AuthError::Upstream);
    }
    validate_expiry(launch.expires_at, requested_ttl)
}

fn validate_identity_status(
    status: &IdentityVerification,
    expected_session_id: &str,
) -> Result<(), AuthError> {
    validate_status_common(
        &status.session_id,
        expected_session_id,
        status.result_id.as_deref(),
        status.reference_id.as_deref(),
        status.expires_at,
    )?;
    validate_confidence(status.document_confidence)?;
    validate_confidence(status.face_confidence)
}

fn validate_voice_status(
    status: &VoiceVerification,
    expected_session_id: &str,
) -> Result<(), AuthError> {
    validate_status_common(
        &status.session_id,
        expected_session_id,
        status.result_id.as_deref(),
        status.reference_id.as_deref(),
        status.expires_at,
    )?;
    validate_confidence(status.liveness_confidence)?;
    validate_confidence(status.speaker_confidence)
}

fn validate_status_common(
    session_id: &str,
    expected_session_id: &str,
    result_id: Option<&str>,
    reference_id: Option<&str>,
    expires_at: u64,
) -> Result<(), AuthError> {
    validate_response_session_id(session_id)?;
    if session_id != expected_session_id || expires_at == 0 {
        return Err(AuthError::Upstream);
    }
    if let Some(value) = result_id {
        validate_response_reference(value)?;
    }
    if let Some(value) = reference_id {
        validate_response_reference(value)?;
    }
    Ok(())
}

fn validate_confidence(value: Option<f64>) -> Result<(), AuthError> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(AuthError::Upstream);
    }
    Ok(())
}

fn valid_reference(value: &str) -> bool {
    (MIN_REFERENCE_BYTES..=MAX_REFERENCE_BYTES).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn validate_request_reference(value: &str) -> Result<(), AuthError> {
    if valid_reference(value) {
        Ok(())
    } else {
        Err(AuthError::BadRequest(
            "invalid biometric provider reference",
        ))
    }
}

fn validate_response_reference(value: &str) -> Result<(), AuthError> {
    if valid_reference(value) {
        Ok(())
    } else {
        Err(AuthError::Upstream)
    }
}

fn valid_session_id(value: &str) -> bool {
    (MIN_SESSION_ID_BYTES..=MAX_SESSION_ID_BYTES).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn validate_request_session_id(value: &str) -> Result<(), AuthError> {
    if valid_session_id(value) {
        Ok(())
    } else {
        Err(AuthError::BadRequest("invalid provider session id"))
    }
}

fn validate_response_session_id(value: &str) -> Result<(), AuthError> {
    if valid_session_id(value) {
        Ok(())
    } else {
        Err(AuthError::Upstream)
    }
}

fn validate_capture_url(raw: &str) -> Result<(), AuthError> {
    let url = Url::parse(raw).map_err(|_| AuthError::Upstream)?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(AuthError::Upstream);
    }
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    );
    if url.scheme() != "https" && !(local && url.scheme() == "http") {
        return Err(AuthError::Upstream);
    }
    Ok(())
}

fn validate_expiry(expires_at: u64, requested_ttl: u64) -> Result<(), AuthError> {
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    if expires_at <= now || expires_at > now + requested_ttl + 60 {
        return Err(AuthError::Upstream);
    }
    Ok(())
}

async fn parse_json_body<T: DeserializeOwned>(mut response: Response) -> Result<T, AuthError> {
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_RESPONSE_BODY_BYTES)
    {
        return Err(AuthError::Upstream);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(provider_transport)? {
        if body.len() + chunk.len() > MAX_RESPONSE_BODY_BYTES {
            return Err(AuthError::Upstream);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|error| {
        tracing::warn!(%error, "biometric provider returned invalid JSON");
        AuthError::Upstream
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoy_mode_is_a_stable_wire_value() {
        assert_eq!(
            serde_json::to_string(&ProviderMode::Decoy).unwrap(),
            "\"decoy\""
        );
    }

    #[test]
    fn session_ids_cannot_escape_the_provider_path() {
        assert!(validate_request_session_id("session_01234567").is_ok());
        assert!(validate_request_session_id("../../admin").is_err());
        assert!(validate_request_session_id("has space").is_err());
    }

    #[test]
    fn capture_urls_are_https_or_loopback_only() {
        assert!(validate_capture_url("https://capture.example/s/abc?token=opaque").is_ok());
        assert!(validate_capture_url("http://127.0.0.1:9000/s/abc").is_ok());
        assert!(validate_capture_url("http://capture.example/s/abc").is_err());
        assert!(validate_capture_url("https://user:pass@capture.example/s").is_err());
    }

    #[test]
    fn confidence_values_are_finite_probabilities() {
        assert!(validate_confidence(Some(0.95)).is_ok());
        assert!(validate_confidence(None).is_ok());
        assert!(validate_confidence(Some(1.1)).is_err());
        assert!(validate_confidence(Some(f64::NAN)).is_err());
    }
}
