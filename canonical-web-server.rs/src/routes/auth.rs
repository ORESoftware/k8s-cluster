use crate::{
    auth::{
        require_csrf, require_origin, AuthProviderError, OptionalAuthenticated,
        SessionAuthenticated,
    },
    error::AppError,
    views, AppState,
};
use axum::{
    extract::{DefaultBodyLimit, Form, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;

const LOGIN_CSRF_COOKIE: &str = "canonical_login_csrf";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .fallback(auth_not_found)
}

pub async fn login_page(
    State(state): State<AppState>,
    jar: CookieJar,
    OptionalAuthenticated(existing): OptionalAuthenticated,
) -> Response {
    if existing.is_some() {
        return Redirect::to("/app").into_response();
    }
    let csrf = random_token();
    let jar = jar.add(
        Cookie::build((LOGIN_CSRF_COOKIE, csrf.clone()))
            .path("/")
            .secure(state.config.cookie_secure)
            .http_only(true)
            .same_site(SameSite::Lax)
            .build(),
    );
    (jar, views::login(&csrf, None)).into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    csrf: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    require_origin(&headers, &state)?;
    let csrf_matches = jar
        .get(LOGIN_CSRF_COOKIE)
        .map(|cookie| cookie.value() == form.csrf)
        .unwrap_or(false);
    if !csrf_matches {
        return Err(AppError::Forbidden);
    }
    if let Err(retry_after_seconds) = state.login_rate_limiter.check(&form.email).await {
        tracing::warn!(retry_after_seconds, "login attempt rate limited");
        return Err(AppError::RateLimited {
            retry_after_seconds,
        });
    }

    // Reject immediately instead of allowing an unbounded queue to accumulate
    // behind Supabase or the session database. The attempt budget is consumed
    // first so saturation cannot be used to bypass the account/global throttle.
    // The owned permit remains live through both upstream authentication and
    // the local session insert, including across cloned router state.
    let login_auth_permit = state
        .login_auth_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            tracing::warn!("password login capacity exhausted");
            AppError::LoginAuthBusy
        })?;

    let tokens = match state
        .auth
        .password_sign_in(&form.email, &form.password)
        .await
    {
        Ok(tokens) => tokens,
        Err(AuthProviderError::InvalidCredentials) => {
            let message = "Email or password was not accepted.";
            if headers.contains_key("hx-request") {
                let mut response = (StatusCode::OK, views::login_error(message)).into_response();
                response
                    .headers_mut()
                    .insert("hx-retarget", HeaderValue::from_static("#login-result"));
                return Ok(response);
            }
            return Ok((
                StatusCode::UNAUTHORIZED,
                views::login(&form.csrf, Some(message)),
            )
                .into_response());
        }
        Err(error) => {
            tracing::warn!(%error, "Supabase login request failed");
            return Err(AppError::AuthUpstream);
        }
    };
    let created = state.sessions.create(tokens).await?;
    drop(login_auth_permit);
    tracing::info!(user_id = %created.context.user_id, "password login succeeded");
    let max_age = time::Duration::seconds(
        state
            .config
            .session_ttl
            .as_secs()
            .try_into()
            .unwrap_or(i64::MAX),
    );
    let jar = jar
        .remove(removal_cookie(
            LOGIN_CSRF_COOKIE,
            state.config.cookie_secure,
        ))
        .add(
            Cookie::build((state.config.session_cookie.clone(), created.raw_id))
                .path("/")
                .secure(state.config.cookie_secure)
                .http_only(true)
                .same_site(SameSite::Lax)
                .max_age(max_age)
                .build(),
        );

    if headers.contains_key("hx-request") {
        let mut response = (jar, StatusCode::OK).into_response();
        response
            .headers_mut()
            .insert("hx-redirect", HeaderValue::from_static("/app"));
        Ok(response)
    } else {
        Ok((jar, Redirect::to("/app")).into_response())
    }
}

#[derive(Deserialize)]
pub struct LogoutForm {
    csrf: String,
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    SessionAuthenticated(actor): SessionAuthenticated,
    Form(form): Form<LogoutForm>,
) -> Result<Response, AppError> {
    require_origin(&headers, &state)?;
    require_csrf(&actor, &headers, Some(&form.csrf))?;
    if let Some(cookie) = jar.get(&state.config.session_cookie) {
        state.sessions.revoke(cookie.value()).await?;
    }
    tracing::info!(user_id = %actor.user_id, "session logout completed");
    let jar = jar.remove(removal_cookie(
        state.config.session_cookie.clone(),
        state.config.cookie_secure,
    ));
    let mut response = (jar, Redirect::to("/login")).into_response();
    // The application intentionally stores offline customer drafts in
    // IndexedDB. Revoking only the server session would leave that origin data
    // available to the next person using the browser profile. Clear the
    // application cache, cookies, and storage at the same security boundary as
    // session revocation. The explicit Set-Cookie deletion remains for clients
    // that do not implement Clear-Site-Data.
    response.headers_mut().insert(
        HeaderName::from_static("clear-site-data"),
        HeaderValue::from_static("\"cache\", \"cookies\", \"storage\""),
    );
    Ok(response)
}

async fn auth_not_found() -> Response {
    (StatusCode::NOT_FOUND, views::html_not_found()).into_response()
}

fn removal_cookie(name: impl Into<String>, secure: bool) -> Cookie<'static> {
    Cookie::build((name.into(), String::new()))
        .path("/")
        .secure(secure)
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::ZERO)
        .build()
}

fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
