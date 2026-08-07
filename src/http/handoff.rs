//! Browser-facing authorization and backend-only code redemption endpoints.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    Form, Json,
};
use axum_extra::extract::cookie::{time::Duration as CookieDuration, Cookie, CookieJar, SameSite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use maud::{html, Markup};
use serde::{Deserialize, Serialize};

use crate::{
    error::AuthError,
    handoff::{
        AuthorizationRequest, IssueError, RedeemAuthorizationCode, SupabaseHandoffTokens,
        ValidatedAuthorization,
    },
    state::AppState,
    views,
};

const CSRF_COOKIE_SECURE: &str = "__Host-shared_auth_authorize_csrf";
const CSRF_COOKIE_LOCAL: &str = "shared_auth_authorize_csrf";

#[derive(Deserialize)]
pub struct AuthorizeForm {
    client_id: String,
    redirect_uri: String,
    return_to: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
    csrf: String,
    email: String,
    password: String,
}

#[derive(Serialize)]
pub struct RedeemResponse {
    access_token: String,
    refresh_token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
    user: crate::handoff::SupabaseHandoffUser,
    return_to: String,
    supabase_project: String,
}

pub async fn authorize(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(request): Query<AuthorizationRequest>,
) -> Result<Response, AuthError> {
    let service = state.handoff.as_ref().ok_or(AuthError::Unavailable)?;
    let authorization = service.validate_authorization(request)?;
    let csrf = random_token();
    let cookie_name = csrf_cookie_name(&state);
    let jar = jar.add(
        Cookie::build((cookie_name, csrf.clone()))
            .path("/")
            .secure(cookie_name == CSRF_COOKIE_SECURE)
            .http_only(true)
            .same_site(SameSite::Lax)
            .max_age(CookieDuration::minutes(10))
            .build(),
    );
    Ok((jar, Html(authorize_page(&authorization, &csrf, None).into_string())).into_response())
}

pub async fn authorize_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<AuthorizeForm>,
) -> Result<Response, AuthError> {
    require_same_origin(&state, &headers)?;
    let cookie_name = csrf_cookie_name(&state);
    if !jar
        .get(cookie_name)
        .is_some_and(|cookie| cookie.value() == form.csrf)
    {
        return Err(AuthError::Forbidden);
    }
    let service = state.handoff.as_ref().ok_or(AuthError::Unavailable)?;
    let authorization = service.validate_authorization(AuthorizationRequest {
        client_id: form.client_id,
        redirect_uri: form.redirect_uri,
        return_to: form.return_to,
        state: form.state,
        code_challenge: form.code_challenge,
        code_challenge_method: form.code_challenge_method,
    })?;
    match service
        .sign_in_and_issue(
            &state.http,
            &state.supabase,
            &state.config.projects,
            &authorization,
            &form.email,
            &form.password,
        )
        .await
    {
        Ok(location) => {
            let jar = jar.remove(removal_cookie(cookie_name, cookie_name == CSRF_COOKIE_SECURE));
            Ok((jar, Redirect::to(&location)).into_response())
        }
        Err(IssueError::InvalidCredentials) => Ok((
            StatusCode::UNAUTHORIZED,
            Html(
                authorize_page(
                    &authorization,
                    &form.csrf,
                    Some("Email or password was not accepted."),
                )
                .into_string(),
            ),
        )
            .into_response()),
        Err(IssueError::Request(error)) => Err(error),
    }
}

pub async fn redeem(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RedeemAuthorizationCode>,
) -> Result<Json<RedeemResponse>, AuthError> {
    let service = state.handoff.as_ref().ok_or(AuthError::Unavailable)?;
    let secret = super::bearer(&headers).ok_or(AuthError::Unauthorized)?;
    let SupabaseHandoffTokens {
        access_token,
        refresh_token,
        expires_at,
        user,
        return_to,
        supabase_project,
    } = service.redeem(request, secret).await?;
    Ok(Json(RedeemResponse {
        access_token,
        refresh_token,
        expires_at,
        user,
        return_to,
        supabase_project,
    }))
}

fn authorize_page(
    authorization: &ValidatedAuthorization,
    csrf: &str,
    error: Option<&str>,
) -> Markup {
    views::page(
        "sign in",
        html! {
            h1 { "Sign in to continue" }
            p class="muted" {
                "Continue to " code { (authorization.client_id) }
                ". shared-auth sends credentials only to the Supabase project registered for this application."
            }
            @if let Some(error) = error {
                p class="err" role="alert" { (error) }
            }
            form method="post" action="/authorize" {
                input type="hidden" name="client_id" value=(authorization.client_id);
                input type="hidden" name="redirect_uri" value=(authorization.redirect_uri);
                input type="hidden" name="return_to" value=(authorization.return_to);
                input type="hidden" name="state" value=(authorization.state);
                input type="hidden" name="code_challenge" value=(authorization.code_challenge);
                input type="hidden" name="code_challenge_method" value="S256";
                input type="hidden" name="csrf" value=(csrf);
                p {
                    label for="email" { "Email" }
                    input id="email" type="email" name="email" autocomplete="email" required;
                }
                p {
                    label for="password" { "Password" }
                    input id="password" type="password" name="password" autocomplete="current-password" required;
                }
                p { button type="submit" { "Sign in and continue" } }
            }
            p class="muted" {
                "The authorization code is single-use, expires quickly, and is bound to this application with PKCE."
            }
        },
    )
}

fn csrf_cookie_name(state: &AppState) -> &'static str {
    if state.config.signing.issuer.starts_with("https://") {
        CSRF_COOKIE_SECURE
    } else {
        CSRF_COOKIE_LOCAL
    }
}

fn removal_cookie(name: &str, secure: bool) -> Cookie<'static> {
    Cookie::build((name.to_owned(), String::new()))
        .path("/")
        .secure(secure)
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(CookieDuration::ZERO)
        .build()
}

fn require_same_origin(state: &AppState, headers: &HeaderMap) -> Result<(), AuthError> {
    let expected = url::Url::parse(&state.config.signing.issuer)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .ok_or(AuthError::Internal)?;
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or(AuthError::Forbidden)?;
    if origin == expected {
        Ok(())
    } else {
        Err(AuthError::Forbidden)
    }
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
