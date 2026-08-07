//! First-party browser authorization-code + PKCE flow.
//!
//! The central browser cookie never crosses a registrable-domain boundary.
//! Applications receive a short-lived, single-use authorization code and
//! exchange it server-to-server with an S256 verifier. Access/refresh tokens
//! are never placed in redirect URLs.

use std::{collections::HashSet, sync::OnceLock};

use axum::{
    extract::{Form, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{time::Duration as CookieDuration, Cookie, CookieJar, SameSite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::TimeDelta;
use maud::{html, DOCTYPE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    db::{AuthenticatedIdentity, OauthApplicationSession, OauthAuthorizationRequest, SessionRecord},
    error::AuthError,
    session::{hash_otp, hash_token, hashed_identifier, MagicLinkToken, MAGIC_LINK_TOKEN_PREFIX},
    state::AppState,
    token::AuthenticationAssurance,
};

use super::{
    local::{enforce_limit, normalize_email},
    session_tokens,
};

const AUTHORIZATION_TTL_SECS: i64 = 600;
const AUTHORIZATION_CODE_TTL_SECS: i64 = 90;
const MAX_RECENT_EMAILS: usize = 5;
const MAX_STATE_BYTES: usize = 1024;
const MAX_NONCE_BYTES: usize = 512;
const MAX_SCOPE_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PasswordlessForm {
    request_id: Uuid,
    email: String,
}

#[derive(Debug, Deserialize)]
pub struct OtpForm {
    request_id: Uuid,
    email: String,
    otp: String,
}

#[derive(Debug, Deserialize)]
pub struct ConsumeLinkQuery {
    request_id: Uuid,
    token: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenForm {
    grant_type: String,
    code: String,
    client_id: String,
    redirect_uri: String,
    code_verifier: String,
}

#[derive(Debug, Serialize)]
pub struct OAuthTokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: u64,
    expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_expires_at: Option<u64>,
    shared_user_id: String,
    email: Option<String>,
    provider: String,
    roles: Vec<String>,
    amr: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    acr: Option<String>,
    audience: String,
    scope: String,
}

pub async fn authorize(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    match authorize_inner(&state, &jar, query).await {
        Ok(response) => response,
        Err(error) => browser_error(error),
    }
}

async fn authorize_inner(
    state: &AppState,
    jar: &CookieJar,
    query: AuthorizeQuery,
) -> Result<Response, AuthError> {
    validate_authorize_query(&query)?;
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let scopes = parse_scopes(&query.scope)?;
    let client = db
        .oauth_client_for_authorization(&query.client_id, &query.redirect_uri, &scopes)
        .await?;
    let transaction_secret = random_token();
    let request = db
        .create_oauth_authorization_request(
            &query.client_id,
            &query.redirect_uri,
            &query.state,
            &query.code_challenge,
            &scopes,
            query.nonce.as_deref(),
            &hash_token(&transaction_secret),
            chrono::Utc::now().fixed_offset() + TimeDelta::seconds(AUTHORIZATION_TTL_SECS),
        )
        .await?;

    if let Some(session) = browser_session(state, jar).await? {
        return finish_authorization(state, jar.clone(), request, session.session_id).await;
    }

    let recent = recent_emails(jar);
    let jar = jar.clone().add(
        Cookie::build((transaction_cookie_name(request.request_id)?, transaction_secret))
            .path("/authorize")
            .secure(true)
            .http_only(true)
            .same_site(SameSite::Lax)
            .max_age(CookieDuration::seconds(AUTHORIZATION_TTL_SECS))
            .build(),
    );
    Ok((
        jar,
        Html(sign_in_page(
            &client.display_name,
            request.request_id,
            &recent,
            None,
        )),
    )
        .into_response())
}

pub async fn request_passwordless(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<PasswordlessForm>,
) -> Response {
    match request_passwordless_inner(&state, jar, form).await {
        Ok(response) => response,
        Err(error) => browser_error(error),
    }
}

async fn request_passwordless_inner(
    state: &AppState,
    jar: CookieJar,
    form: PasswordlessForm,
) -> Result<Response, AuthError> {
    let oauth_link_base = oauth_magic_link_base_url(&state.config.magic_links)?;
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let transaction_hash = transaction_cookie_hash(&jar, form.request_id)?;
    let request = db
        .oauth_authorization_request(form.request_id, Some(&transaction_hash))
        .await?;
    let email = normalize_email(&form.email)?;
    enforce_limit(state, "oauth_passwordless", &email, 5, 900).await?;

    let token = MagicLinkToken::generate();
    let pepper = state
        .config
        .magic_links
        .otp_pepper
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let should_send = db
        .prepare_magic_link(
            &email,
            state.config.magic_links.allow_signup,
            &token.hash,
            &hash_otp(pepper, &email, &token.otp),
            &hashed_identifier(&email),
            chrono::Utc::now().fixed_offset()
                + TimeDelta::seconds(state.config.magic_links.ttl_secs as i64),
        )
        .await?;
    if should_send {
        db.bind_magic_link_to_oauth_request(&token.hash, request.request_id)
            .await?;
        crate::email::send_magic_link(
            &state.http,
            &state.config.magic_links,
            &email,
            &token.plaintext,
            &token.otp,
            Some(&oauth_link_base),
            Some(request.request_id),
        )
        .await?;
    }

    let jar = remember_email(jar, &email);
    Ok((jar, Html(otp_page(request.request_id, &email, None))).into_response())
}

pub async fn consume_otp(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<OtpForm>,
) -> Response {
    match consume_otp_inner(&state, jar, form).await {
        Ok(response) => response,
        Err(AuthError::Unauthorized) => {
            browser_error_message(StatusCode::UNAUTHORIZED, "The code was not accepted or expired.")
        }
        Err(error) => browser_error(error),
    }
}

async fn consume_otp_inner(
    state: &AppState,
    jar: CookieJar,
    form: OtpForm,
) -> Result<Response, AuthError> {
    if form.otp.len() != 6 || !form.otp.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AuthError::BadRequest("invalid one-time code"));
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let transaction_hash = transaction_cookie_hash(&jar, form.request_id)?;
    let request = db
        .oauth_authorization_request(form.request_id, Some(&transaction_hash))
        .await?;
    let email = normalize_email(&form.email)?;
    let pepper = state
        .config
        .magic_links
        .otp_pepper
        .as_deref()
        .ok_or(AuthError::Unavailable)?;
    let identity = db
        .consume_oauth_email_otp(
            &hashed_identifier(&email),
            &hash_otp(pepper, &email, &form.otp),
            request.request_id,
        )
        .await?;
    start_browser_session_and_finish(state, jar, request, identity).await
}

pub async fn consume_link(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<ConsumeLinkQuery>,
) -> Response {
    match consume_link_inner(&state, jar, query).await {
        Ok(response) => response,
        Err(AuthError::Unauthorized) => browser_error_message(
            StatusCode::UNAUTHORIZED,
            "This sign-in link was already used or has expired.",
        ),
        Err(error) => browser_error(error),
    }
}

async fn consume_link_inner(
    state: &AppState,
    jar: CookieJar,
    query: ConsumeLinkQuery,
) -> Result<Response, AuthError> {
    if !query.token.starts_with(MAGIC_LINK_TOKEN_PREFIX) || query.token.len() > 128 {
        return Err(AuthError::Unauthorized);
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let request = db
        .oauth_authorization_request(query.request_id, None)
        .await?;
    let identity = db
        .consume_oauth_magic_link(&hash_token(&query.token), request.request_id)
        .await?;
    start_browser_session_and_finish(state, jar, request, identity).await
}

async fn start_browser_session_and_finish(
    state: &AppState,
    jar: CookieJar,
    request: OauthAuthorizationRequest,
    identity: AuthenticatedIdentity,
) -> Result<Response, AuthError> {
    let issued = session_tokens::issue_with_assurance(
        state,
        identity,
        AuthenticationAssurance::from_level_and_methods(1, &["email".to_owned()]),
    )
    .await?;
    let session_id = issued.session_id.ok_or(AuthError::Internal)?;
    let refresh_token = issued.refresh_token.ok_or(AuthError::Internal)?;
    let refresh_expires_at = issued.refresh_expires_at.ok_or(AuthError::Internal)?;
    let max_age = i64::try_from(refresh_expires_at)
        .ok()
        .and_then(|expires| expires.checked_sub(chrono::Utc::now().timestamp()))
        .unwrap_or(0)
        .clamp(0, state.config.sessions.refresh_ttl_secs as i64);
    let jar = jar.add(
        Cookie::build((browser_session_cookie_name()?, refresh_token))
            .path("/")
            .secure(true)
            .http_only(true)
            .same_site(SameSite::Lax)
            .max_age(CookieDuration::seconds(max_age))
            .build(),
    );
    finish_authorization(state, jar, request, session_id).await
}

async fn finish_authorization(
    state: &AppState,
    jar: CookieJar,
    request: OauthAuthorizationRequest,
    session_id: Uuid,
) -> Result<Response, AuthError> {
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let raw_code = random_token();
    let redirect = db
        .create_oauth_authorization_code(
            request.request_id,
            session_id,
            &hash_token(&raw_code),
            chrono::Utc::now().fixed_offset() + TimeDelta::seconds(AUTHORIZATION_CODE_TTL_SECS),
        )
        .await?;
    let mut url = reqwest::Url::parse(&redirect.redirect_uri).map_err(|_| AuthError::Internal)?;
    url.query_pairs_mut()
        .append_pair("code", &raw_code)
        .append_pair("state", &redirect.state);
    let jar = clear_transaction_cookie(jar, request.request_id)?;
    Ok((jar, Redirect::to(url.as_str())).into_response())
}

pub async fn token(
    State(state): State<AppState>,
    Form(form): Form<TokenForm>,
) -> Result<axum::Json<OAuthTokenResponse>, AuthError> {
    if form.grant_type != "authorization_code" {
        return Err(AuthError::BadRequest("unsupported grant type"));
    }
    validate_code_verifier(&form.code_verifier)?;
    if form.code.len() > 256 || form.client_id.len() > 128 || form.redirect_uri.len() > 2048 {
        return Err(AuthError::BadRequest("invalid authorization code request"));
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    let computed_challenge = pkce_challenge(&form.code_verifier);
    let grant = db
        .consume_oauth_authorization_code(
            &hash_token(&form.code),
            &form.client_id,
            &form.redirect_uri,
            &computed_challenge,
        )
        .await?;
    let identity = grant.browser_session.identity.clone();
    let offline_access = grant.scopes.iter().any(|scope| scope == "offline_access");
    let include_email = grant.scopes.iter().any(|scope| scope == "email");
    let application = OauthApplicationSession {
        client_id: grant.client.client_id.clone(),
        audience: grant.client.audience.clone(),
        scopes: grant.scopes.clone(),
    };
    let issued = session_tokens::issue_for_application(
        &state,
        grant.browser_session.identity,
        AuthenticationAssurance::from_level_and_methods(
            grant.browser_session.auth_level,
            &grant.browser_session.auth_methods,
        ),
        application,
        offline_access,
    )
    .await?;
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    Ok(axum::Json(OAuthTokenResponse {
        access_token: issued.access.token,
        token_type: "Bearer",
        expires_in: issued.access.expires_at.saturating_sub(now),
        expires_at: issued.access.expires_at,
        refresh_token: issued.refresh_token,
        refresh_expires_at: issued.refresh_expires_at,
        shared_user_id: identity.shared_user_id.to_string(),
        email: if include_email { identity.email } else { None },
        provider: identity.provider,
        roles: identity.roles,
        amr: issued.access.amr,
        acr: issued.access.acr,
        audience: grant.client.audience,
        scope: grant.scopes.join(" "),
    }))
}

async fn browser_session(
    state: &AppState,
    jar: &CookieJar,
) -> Result<Option<SessionRecord>, AuthError> {
    let cookie_name = browser_session_cookie_name()?;
    let Some(cookie) = jar.get(&cookie_name) else {
        return Ok(None);
    };
    if cookie.value().len() > 256 {
        return Ok(None);
    }
    let db = state.db.as_ref().ok_or(AuthError::Unavailable)?;
    match db.session_for_refresh_hash(&hash_token(cookie.value())).await {
        Ok(session) if session.oauth.is_none() => Ok(Some(session.session)),
        Ok(_) => Ok(None),
        Err(AuthError::Unauthorized) => Ok(None),
        Err(error) => Err(error),
    }
}

fn validate_authorize_query(query: &AuthorizeQuery) -> Result<(), AuthError> {
    if query.response_type != "code" || query.code_challenge_method != "S256" {
        return Err(AuthError::BadRequest(
            "authorization code with S256 PKCE is required",
        ));
    }
    if query.client_id.len() < 3
        || query.client_id.len() > 128
        || !query.client_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
        || query.redirect_uri.is_empty()
        || query.redirect_uri.len() > 2048
        || query.state.is_empty()
        || query.state.len() > MAX_STATE_BYTES
        || query.code_challenge.len() != 43
        || !valid_pkce_challenge(&query.code_challenge)
        || query.scope.len() > MAX_SCOPE_BYTES
        || query.nonce.as_ref().is_some_and(|nonce| {
            nonce.is_empty()
                || nonce.len() > MAX_NONCE_BYTES
                || nonce.chars().any(char::is_control)
        })
    {
        return Err(AuthError::BadRequest("invalid authorization request"));
    }
    validate_redirect_uri(&query.redirect_uri)
}

fn validate_redirect_uri(value: &str) -> Result<(), AuthError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| AuthError::BadRequest("invalid redirect uri"))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(AuthError::BadRequest("invalid redirect uri"));
    }
    let allowed_scheme = match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => url.host_str().is_some_and(is_loopback_host),
        "javascript" | "data" | "file" | "blob" => false,
        _ => true,
    };
    if !allowed_scheme
        || url.query_pairs().any(|(name, _)| {
            matches!(
                name.as_ref(),
                "code" | "state" | "error" | "error_description"
            )
        })
    {
        return Err(AuthError::BadRequest("invalid redirect uri"));
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn valid_pkce_challenge(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_code_verifier(value: &str) -> Result<(), AuthError> {
    if !(43..=128).contains(&value.len()) || !valid_urlsafe_token(value) {
        return Err(AuthError::BadRequest("invalid PKCE verifier"));
    }
    Ok(())
}

fn valid_urlsafe_token(value: &str) -> bool {
    value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
    })
}

fn parse_scopes(value: &str) -> Result<Vec<String>, AuthError> {
    let mut scopes = Vec::new();
    let mut seen = HashSet::new();
    for scope in value.split_ascii_whitespace() {
        if scope.is_empty()
            || scope.len() > 128
            || !scope.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.')
            })
            || !seen.insert(scope)
        {
            return Err(AuthError::BadRequest("invalid scope"));
        }
        scopes.push(scope.to_owned());
    }
    if scopes.is_empty() || scopes.len() > 16 || !seen.contains("openid") {
        return Err(AuthError::BadRequest("invalid scope"));
    }
    scopes.sort_unstable();
    Ok(scopes)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn recent_emails(jar: &CookieJar) -> Vec<String> {
    let Ok(cookie_name) = recent_emails_cookie_name() else {
        return vec![];
    };
    let Some(raw) = jar.get(&cookie_name).map(|cookie| cookie.value()) else {
        return vec![];
    };
    let Ok(decoded) = URL_SAFE_NO_PAD.decode(raw) else {
        return vec![];
    };
    let Ok(values) = serde_json::from_slice::<Vec<String>>(&decoded) else {
        return vec![];
    };
    values
        .into_iter()
        .filter(|email| normalize_email(email).is_ok())
        .take(MAX_RECENT_EMAILS)
        .collect()
}

fn remember_email(jar: CookieJar, email: &str) -> CookieJar {
    let mut values = recent_emails(&jar);
    values.retain(|existing| existing != email);
    values.insert(0, email.to_owned());
    values.truncate(MAX_RECENT_EMAILS);
    let value = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&values).unwrap_or_default());
    let cookie_name = recent_emails_cookie_name()
        .unwrap_or_else(|_| "__Host-shared-auth-recent-emails".to_owned());
    jar.add(
        Cookie::build((cookie_name, value))
            .path("/")
            .secure(true)
            .http_only(true)
            .same_site(SameSite::Lax)
            .max_age(CookieDuration::days(180))
            .build(),
    )
}

fn browser_session_cookie_name() -> Result<String, AuthError> {
    static NAME: OnceLock<Result<String, ()>> = OnceLock::new();
    match NAME.get_or_init(|| {
        let name = std::env::var("AUTH_SESSION_COOKIE_NAME")
            .unwrap_or_else(|_| "__Host-shared-auth-session".to_owned());
        if name.starts_with("__Host-")
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            Ok(name)
        } else {
            Err(())
        }
    }) {
        Ok(name) => Ok(name.clone()),
        Err(()) => Err(AuthError::Internal),
    }
}

fn recent_emails_cookie_name() -> Result<String, AuthError> {
    Ok(format!("{}-recent", browser_session_cookie_name()?))
}

fn transaction_cookie_name(request_id: Uuid) -> Result<String, AuthError> {
    Ok(format!(
        "{}-oauth-{}",
        browser_session_cookie_name()?,
        request_id.simple()
    ))
}

fn transaction_cookie_hash(jar: &CookieJar, request_id: Uuid) -> Result<String, AuthError> {
    let name = transaction_cookie_name(request_id)?;
    let value = jar
        .get(&name)
        .map(|cookie| cookie.value())
        .filter(|value| value.len() == 43 && valid_urlsafe_token(value))
        .ok_or(AuthError::Unauthorized)?;
    Ok(hash_token(value))
}

fn clear_transaction_cookie(
    jar: CookieJar,
    request_id: Uuid,
) -> Result<CookieJar, AuthError> {
    Ok(jar.remove(
        Cookie::build((transaction_cookie_name(request_id)?, ""))
            .path("/authorize")
            .secure(true)
            .http_only(true)
            .same_site(SameSite::Lax)
            .build(),
    ))
}

fn oauth_magic_link_base_url(
    config: &crate::config::MagicLinkConfig,
) -> Result<String, AuthError> {
    if config.sendgrid_api_key.is_none()
        || config.otp_pepper.is_none()
        || config.from_email.is_none()
    {
        return Err(AuthError::Unavailable);
    }
    let value = std::env::var("AUTH_OAUTH_MAGIC_LINK_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(AuthError::Unavailable)?;
    let url = reqwest::Url::parse(&value).map_err(|_| AuthError::Unavailable)?;
    let loopback_http = url.scheme() == "http" && url.host_str().is_some_and(is_loopback_host);
    if (url.scheme() != "https" && !loopback_http)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AuthError::Unavailable);
    }
    Ok(value)
}

fn sign_in_page(
    application_name: &str,
    request_id: Uuid,
    recent: &[String],
    error: Option<&str>,
) -> String {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Sign in — shared-auth" }
                style { (PAGE_STYLE) }
            }
            body {
                main {
                    p class="eyebrow" { "shared-auth" }
                    h1 { "Sign in to " (application_name) }
                    p { "We’ll email a one-time link and six-digit code. No password is required." }
                    @if let Some(message) = error { p class="error" role="alert" { (message) } }
                    form method="post" action="/authorize/passwordless/request" {
                        input type="hidden" name="request_id" value=(request_id);
                        label for="email" { "Email" }
                        input id="email" name="email" type="email" list="recent-emails" autocomplete="email" maxlength="320" required;
                        datalist id="recent-emails" {
                            @for email in recent { option value=(email) {} }
                        }
                        button type="submit" { "Email me a sign-in link" }
                    }
                    p class="privacy" { "Recent addresses are kept only in a secure cookie in this browser." }
                }
            }
        }
    }
    .into_string()
}

fn otp_page(request_id: Uuid, email: &str, error: Option<&str>) -> String {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Check your email — shared-auth" }
                style { (PAGE_STYLE) }
            }
            body {
                main {
                    p class="eyebrow" { "shared-auth" }
                    h1 { "Check your email" }
                    p { "Use the link we sent to " strong { (email) } ", or enter the six-digit code." }
                    @if let Some(message) = error { p class="error" role="alert" { (message) } }
                    form method="post" action="/authorize/passwordless/consume" {
                        input type="hidden" name="request_id" value=(request_id);
                        input type="hidden" name="email" value=(email);
                        label for="otp" { "Six-digit code" }
                        input id="otp" name="otp" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9]{6}" minlength="6" maxlength="6" required;
                        button type="submit" { "Continue" }
                    }
                }
            }
        }
    }
    .into_string()
}

fn browser_error(error: AuthError) -> Response {
    let status = match error {
        AuthError::BadRequest(_) => StatusCode::BAD_REQUEST,
        AuthError::Unauthorized => StatusCode::UNAUTHORIZED,
        AuthError::Forbidden => StatusCode::FORBIDDEN,
        AuthError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        AuthError::Conflict => StatusCode::CONFLICT,
        AuthError::Unavailable | AuthError::Upstream => StatusCode::SERVICE_UNAVAILABLE,
        AuthError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    if matches!(error, AuthError::Internal | AuthError::Upstream) {
        tracing::error!(%error, "browser authorization flow failed");
    }
    browser_error_message(
        status,
        "The sign-in request could not be completed. Start again from the application.",
    )
}

fn browser_error_message(status: StatusCode, message: &str) -> Response {
    let body = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Sign-in problem — shared-auth" }
                style { (PAGE_STYLE) }
            }
            body { main { h1 { "Sign-in problem" } p role="alert" { (message) } } }
        }
    };
    let mut response = (status, Html(body.into_string())).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

const PAGE_STYLE: &str = r#"
:root{font-family:Inter,ui-sans-serif,system-ui,sans-serif;color-scheme:dark;background:#07111f;color:#f6f7fb}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:radial-gradient(circle at top,#18304e,#07111f 55%)}
main{width:min(92vw,32rem);box-sizing:border-box;padding:2rem;border:1px solid #ffffff24;border-radius:1rem;background:#0d1a2be8;box-shadow:0 2rem 5rem #0008}
h1{font-size:clamp(1.8rem,5vw,2.6rem);margin:.25rem 0 1rem}.eyebrow{letter-spacing:.16em;text-transform:uppercase;color:#8fc8ff;font-weight:700}.privacy{font-size:.9rem;color:#aab8c8}
label{display:block;margin:1.25rem 0 .45rem;font-weight:700}input{width:100%;box-sizing:border-box;padding:.9rem 1rem;border-radius:.65rem;border:1px solid #ffffff30;background:#07111f;color:inherit;font:inherit}button{width:100%;margin-top:1rem;padding:.95rem;border:0;border-radius:.65rem;background:#dff365;color:#111827;font:inherit;font-weight:800;cursor:pointer}.error{padding:.8rem;border-radius:.5rem;background:#7f1d1d;color:#fee2e2}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_uses_s256_base64url_without_padding() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn return_state_and_scope_are_bounded() {
        let query = AuthorizeQuery {
            response_type: "code".into(),
            client_id: "canonical-web".into(),
            redirect_uri: "https://app.canonical.plus/auth/callback".into(),
            state: "s".repeat(MAX_STATE_BYTES + 1),
            code_challenge: "a".repeat(43),
            code_challenge_method: "S256".into(),
            scope: "openid email offline_access".into(),
            nonce: None,
        };
        assert!(validate_authorize_query(&query).is_err());
    }

    #[test]
    fn redirect_uri_rejects_dangerous_schemes_and_reserved_parameters() {
        assert!(validate_redirect_uri("javascript:alert(1)").is_err());
        assert!(validate_redirect_uri("https://app.example/callback?code=smuggled").is_err());
        assert!(validate_redirect_uri("http://example.com/callback").is_err());
        assert!(validate_redirect_uri("http://127.0.0.1:41000/callback").is_ok());
        assert!(validate_redirect_uri("canonical-app:/oauth/callback").is_ok());
    }

    #[test]
    fn scopes_require_openid_and_reject_duplicates() {
        assert!(parse_scopes("email quote:write").is_err());
        assert!(parse_scopes("openid email email").is_err());
        assert_eq!(
            parse_scopes("quote:write openid email").unwrap(),
            ["email", "openid", "quote:write"]
        );
    }
}
