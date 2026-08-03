//! Optional Twilio Verify adapter for SMS second-factor challenges.

use serde::Deserialize;

use crate::config::TwilioVerifyConfig;
use crate::error::AuthError;

const TWILIO_VERIFY_BASE_URL: &str = "https://verify.twilio.com/v2";

#[derive(Deserialize)]
struct VerificationResponse {
    status: String,
}

pub async fn start_sms_verification(
    http: &reqwest::Client,
    config: &TwilioVerifyConfig,
    phone_e164: &str,
) -> Result<(), AuthError> {
    start_sms_verification_at(http, config, TWILIO_VERIFY_BASE_URL, phone_e164).await
}

async fn start_sms_verification_at(
    http: &reqwest::Client,
    config: &TwilioVerifyConfig,
    base_url: &str,
    phone_e164: &str,
) -> Result<(), AuthError> {
    let (account_sid, auth_token, service_sid) = credentials(config)?;
    let url = format!(
        "{}/Services/{service_sid}/Verifications",
        base_url.trim_end_matches('/')
    );
    let response = http
        .post(url)
        .basic_auth(account_sid, Some(auth_token))
        .form(&[("To", phone_e164), ("Channel", "sms")])
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Twilio Verify start request failed");
            AuthError::Upstream
        })?;
    if !response.status().is_success() {
        tracing::warn!(
            status = response.status().as_u16(),
            "Twilio Verify rejected SMS challenge"
        );
        return Err(AuthError::Upstream);
    }
    let verification = response
        .json::<VerificationResponse>()
        .await
        .map_err(|_| AuthError::Upstream)?;
    if !matches!(verification.status.as_str(), "pending" | "approved") {
        return Err(AuthError::Upstream);
    }
    Ok(())
}

pub async fn check_sms_verification(
    http: &reqwest::Client,
    config: &TwilioVerifyConfig,
    phone_e164: &str,
    code: &str,
) -> Result<bool, AuthError> {
    check_sms_verification_at(http, config, TWILIO_VERIFY_BASE_URL, phone_e164, code).await
}

async fn check_sms_verification_at(
    http: &reqwest::Client,
    config: &TwilioVerifyConfig,
    base_url: &str,
    phone_e164: &str,
    code: &str,
) -> Result<bool, AuthError> {
    let (account_sid, auth_token, service_sid) = credentials(config)?;
    let url = format!(
        "{}/Services/{service_sid}/VerificationCheck",
        base_url.trim_end_matches('/')
    );
    let response = http
        .post(url)
        .basic_auth(account_sid, Some(auth_token))
        .form(&[("To", phone_e164), ("Code", code)])
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(%error, "Twilio Verify check request failed");
            AuthError::Upstream
        })?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        tracing::warn!(
            status = response.status().as_u16(),
            "Twilio Verify rejected code check"
        );
        return Err(AuthError::Upstream);
    }
    response
        .json::<VerificationResponse>()
        .await
        .map(|verification| verification.status == "approved")
        .map_err(|_| AuthError::Upstream)
}

fn credentials(config: &TwilioVerifyConfig) -> Result<(&str, &str, &str), AuthError> {
    if !config.is_enabled() {
        return Err(AuthError::Unavailable);
    }
    match (
        config.account_sid.as_deref().map(str::trim),
        config.auth_token.as_deref().map(str::trim),
        config.service_sid.as_deref().map(str::trim),
    ) {
        (Some(account_sid), Some(auth_token), Some(service_sid)) => {
            Ok((account_sid, auth_token, service_sid))
        }
        _ => Err(AuthError::Unavailable),
    }
}

#[cfg(test)]
#[path = "twilio_tests.rs"]
mod tests;
