//! First-party browser ceremony for magic-link/email-OTP sign-in.
//!
//! Product deployments expose this module through their own `/shared-auth/`
//! gateway prefix. The resulting `__Host-` cookies therefore stay scoped to the
//! product origin; no parent-domain or cross-product session cookie exists.

use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::{
    aead::{Aead, KeyInit as AeadKeyInit, Payload},
    Aes256Gcm, Nonce,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::SysRng, TryRng};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    db::AuthenticatedIdentity,
    error::AuthError,
    state::AppState,
    token::AuthenticationAssurance,
    views,
};

use super::{passwordless, session_tokens};

const MAX_RETURN_BYTES: usize = 2048;
const MAX_REMEMBERED_EMAILS: usize = 5;
const REMEMBERED_EMAILS_TTL_SECS: u64 = 31_536_000;
const STATE_AAD: &[u8] = b"shared-auth:browser-return:v1";
const EMAILS_AAD: &[u8] = b"shared-auth:remembered-emails:v1";

#[derive(Clone)]
struct BrowserConfig {
    seal_secret: String,
    public_prefix: String,
    session_cookie: String,
    refresh_cookie: String,
    remembered_emails_cookie: String,
}

#[derive(Debug, Deserialize)]
pub struct SignInQuery {
    #[serde(default, rename = "return")]
    return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SignInForm {
    email: String,
    #[serde(default, rename = "return")]
    return_to: String,
}

#[derive(Debug, Deserialize)]
pub struct ConsumeQuery {
    token: String,
    state: String,
}

#[derive(Debug, Deserialize)]
pub struct OtpForm {
    email: String,
    otp: String,
    state: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReturnState {
    return_to: String,
    expires_at: u64,
}

pub async fn sign_in(
    State(_state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SignInQuery>,
) -> Response {
    let config = match BrowserConfig::from_env() {
        Ok(config) => config,
        Err(error) => return error.into_response(),
    };
    let return_to = safe_return_path(query.return_to.as_deref()).to_owned();
    let remembered = remembered_emails(&headers, &config);
    views::browser_sign_in(
        &remembered,
        &return_to,
        &format!("{}/auth/browser/sign-in", config.public_prefix),
    )
    .into_response()
}

pub async fn request_link(
    State(state): State<AppState>,
    Form(form): Form<SignInForm>,
) -> Result<Response, AuthError> {
    let config = BrowserConfig::from_env()?;
    let return_to = safe_return_path(Some(&form.return_to)).to_owned();
    let expires_at = now_secs().saturating_add(state.config.magic_links.ttl_secs);
    let sealed_state = seal_json(
        &config.seal_secret,
        STATE_AAD,
        &ReturnState {
            return_to,
            expires_at,
        },
    )?;
    let email = passwordless::request_magic_link(&state, &form.email, Some(&sealed_state)).await?;
    Ok(views::browser_link_sent(
        &email,
        &sealed_state,
        &format!("{}/auth/browser/otp", config.public_prefix),
        &format!("{}/auth/browser/sign-in", config.public_prefix),
    )
    .into_response())
}

pub async fn consume_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ConsumeQuery>,
) -> Result<Response, AuthError> {
    let config = BrowserConfig::from_env()?;
    let return_state = open_return_state(&config, &query.state)?;
    let identity = passwordless::consume_magic_link_identity(&state, &query.token).await?;
    issue_browser_session(&state, &config, &headers, identity, &return_state.return_to).await
}

pub async fn consume_otp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<OtpForm>,
) -> Result<Response, AuthError> {
    let config = BrowserConfig::from_env()?;
    let return_state = open_return_state(&config, &form.state)?;
    let identity =
        passwordless::consume_email_otp_identity(&state, &form.email, &form.otp).await?;
    issue_browser_session(&state, &config, &headers, identity, &return_state.return_to).await
}

async fn issue_browser_session(
    state: &AppState,
    config: &BrowserConfig,
    request_headers: &HeaderMap,
    identity: AuthenticatedIdentity,
    return_to: &str,
) -> Result<Response, AuthError> {
    let verified_email = identity.email.clone();
    let methods = vec!["email".to_owned()];
    let issued = session_tokens::issue_with_assurance(
        state,
        identity,
        AuthenticationAssurance::from_level_and_methods(1, &methods),
    )
    .await?;
    let refresh_token = issued.refresh_token.ok_or(AuthError::Internal)?;
    let refresh_expires_at = issued.refresh_expires_at.ok_or(AuthError::Internal)?;
    let now = now_secs();

    let mut response = Redirect::to(return_to).into_response();
    append_cookie(
        &mut response,
        &config.session_cookie,
        &issued.access.token,
        issued.access.expires_at.saturating_sub(now).max(1),
    )?;
    append_cookie(
        &mut response,
        &config.refresh_cookie,
        &refresh_token,
        refresh_expires_at.saturating_sub(now).max(1),
    )?;

    if let Some(email) = verified_email {
        let mut emails = remembered_emails(request_headers, config);
        emails.retain(|candidate| candidate != &email);
        emails.insert(0, email);
        emails.truncate(MAX_REMEMBERED_EMAILS);
        let sealed = seal_json(&config.seal_secret, EMAILS_AAD, &emails)?;
        append_cookie(
            &mut response,
            &config.remembered_emails_cookie,
            &sealed,
            REMEMBERED_EMAILS_TTL_SECS,
        )?;
    }
    Ok(response)
}

fn open_return_state(config: &BrowserConfig, sealed: &str) -> Result<ReturnState, AuthError> {
    let state: ReturnState = open_json(&config.seal_secret, STATE_AAD, sealed)?;
    let now = now_secs();
    if state.expires_at < now
        || state.expires_at > now.saturating_add(3_660)
        || safe_return_path(Some(&state.return_to)) != state.return_to
    {
        return Err(AuthError::Unauthorized);
    }
    Ok(state)
}

fn remembered_emails(headers: &HeaderMap, config: &BrowserConfig) -> Vec<String> {
    let Some(cookie) = cookie_value(headers, &config.remembered_emails_cookie) else {
        return Vec::new();
    };
    let Ok(mut emails) = open_json::<Vec<String>>(&config.seal_secret, EMAILS_AAD, &cookie)
    else {
        return Vec::new();
    };
    emails.retain(|email| super::local::normalize_email(email).is_ok());
    emails.sort();
    emails.dedup();
    emails.truncate(MAX_REMEMBERED_EMAILS);
    emails
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

fn append_cookie(
    response: &mut Response,
    name: &str,
    value: &str,
    max_age: u64,
) -> Result<(), AuthError> {
    let value = format!(
        "{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}",
        max_age.min(REMEMBERED_EMAILS_TTL_SECS)
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&value).map_err(|_| AuthError::Internal)?,
    );
    Ok(())
}

fn safe_return_path(value: Option<&str>) -> &str {
    let value = value.unwrap_or("/");
    if value.starts_with('/')
        && !value.starts_with("//")
        && value.len() <= MAX_RETURN_BYTES
        && !value
            .chars()
            .any(|character| matches!(character, '\\' | '\r' | '\n' | '\0'))
    {
        value
    } else {
        "/"
    }
}

fn seal_json<T: Serialize>(
    secret: &str,
    aad: &[u8],
    value: &T,
) -> Result<String, AuthError> {
    let plaintext = serde_json::to_vec(value).map_err(|_| AuthError::Internal)?;
    if plaintext.len() > 3_072 {
        return Err(AuthError::BadRequest("browser state is too large"));
    }
    let key = Sha256::digest(secret.as_bytes());
    let cipher = <Aes256Gcm as AeadKeyInit>::new_from_slice(&key)
        .map_err(|_| AuthError::Internal)?;
    let mut nonce_bytes = [0_u8; 12];
    SysRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|_| AuthError::Internal)?;
    let nonce = Nonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad,
            },
        )
        .map_err(|_| AuthError::Internal)?;
    let mut sealed = nonce_bytes.to_vec();
    sealed.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

fn open_json<T: DeserializeOwned>(
    secret: &str,
    aad: &[u8],
    sealed: &str,
) -> Result<T, AuthError> {
    if sealed.len() > 4_096 {
        return Err(AuthError::Unauthorized);
    }
    let raw = URL_SAFE_NO_PAD
        .decode(sealed)
        .map_err(|_| AuthError::Unauthorized)?;
    if raw.len() < 12 + 16 {
        return Err(AuthError::Unauthorized);
    }
    let nonce_bytes: [u8; 12] = raw[..12]
        .try_into()
        .map_err(|_| AuthError::Unauthorized)?;
    let nonce = Nonce::from(nonce_bytes);
    let key = Sha256::digest(secret.as_bytes());
    let cipher = <Aes256Gcm as AeadKeyInit>::new_from_slice(&key)
        .map_err(|_| AuthError::Internal)?;
    let plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &raw[12..],
                aad,
            },
        )
        .map_err(|_| AuthError::Unauthorized)?;
    serde_json::from_slice(&plaintext).map_err(|_| AuthError::Unauthorized)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

impl BrowserConfig {
    fn from_env() -> Result<Self, AuthError> {
        let seal_secret = std::env::var("AUTH_BROWSER_SEAL_SECRET")
            .ok()
            .filter(|value| value.len() >= 32)
            .ok_or(AuthError::Unavailable)?;
        let public_prefix = normalize_public_prefix(
            &std::env::var("AUTH_BROWSER_PUBLIC_PREFIX").unwrap_or_default(),
        )?;
        let session_cookie = std::env::var("AUTH_SESSION_COOKIE_NAME")
            .ok()
            .filter(|value| valid_cookie_name(value))
            .ok_or(AuthError::Unavailable)?;
        let refresh_cookie = format!("{session_cookie}-refresh");
        let remembered_emails_cookie = format!("{session_cookie}-emails");
        if !valid_cookie_name(&refresh_cookie) || !valid_cookie_name(&remembered_emails_cookie) {
            return Err(AuthError::Unavailable);
        }
        Ok(Self {
            seal_secret,
            public_prefix,
            session_cookie,
            refresh_cookie,
            remembered_emails_cookie,
        })
    }
}

fn normalize_public_prefix(value: &str) -> Result<String, AuthError> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.starts_with('/')
        && !value.starts_with("//")
        && value.len() <= 128
        && !value.chars().any(|character| {
            matches!(character, '\\' | '\r' | '\n' | '\0' | '?' | '#')
        })
    {
        Ok(value.to_owned())
    } else {
        Err(AuthError::Unavailable)
    }
}

fn valid_cookie_name(value: &str) -> bool {
    value.starts_with("__Host-")
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_paths_are_relative_and_header_safe() {
        assert_eq!(
            safe_return_path(Some("/u/quote?framework=soc2")),
            "/u/quote?framework=soc2"
        );
        for unsafe_value in [
            "https://evil.test",
            "//evil.test",
            "/ok\r\nset-cookie:x",
            "/bad\\path",
        ] {
            assert_eq!(safe_return_path(Some(unsafe_value)), "/");
        }
    }

    #[test]
    fn sealed_state_is_confidential_authenticated_and_expiring() {
        let secret = "browser-test-secret-at-least-thirty-two-bytes";
        let state = ReturnState {
            return_to: "/u/quote".into(),
            expires_at: now_secs() + 900,
        };
        let sealed = seal_json(secret, STATE_AAD, &state).unwrap();
        assert!(!sealed.contains("quote"));
        let opened: ReturnState = open_json(secret, STATE_AAD, &sealed).unwrap();
        assert_eq!(opened.return_to, "/u/quote");
        assert!(open_json::<ReturnState>(
            "different-secret-at-least-thirty-two",
            STATE_AAD,
            &sealed
        )
        .is_err());
    }

    #[test]
    fn browser_prefix_and_cookie_names_fail_closed() {
        assert_eq!(
            normalize_public_prefix("/shared-auth/").unwrap(),
            "/shared-auth"
        );
        assert!(normalize_public_prefix("https://auth.example").is_err());
        assert!(valid_cookie_name("__Host-canonical-customer-auth"));
        assert!(!valid_cookie_name("canonical-auth"));
    }
}
