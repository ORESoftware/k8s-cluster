//! TOTP enrollment and verification routes.

use crate::cookies::{cookie_value, set_cookie, sign, EMAIL_COOKIE, ENROLL_COOKIE, SESSION_COOKIE};
use crate::shared_auth::AuthVerdict;
use crate::state::AppState;
use crate::totp;
use crate::views::page;
use axum::extract::{Form, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use maud::{html, PreEscaped};
use rand::RngCore;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

fn session_token(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, SESSION_COOKIE).filter(|token| !token.trim().is_empty())
}

async fn session_verdict(state: &AppState, headers: &HeaderMap) -> AuthVerdict {
    let Some(token) = session_token(headers) else {
        return AuthVerdict::Unauthenticated;
    };
    let Some(shared_auth) = &state.shared_auth else {
        return AuthVerdict::Degraded;
    };
    shared_auth.verify_access_token(&token).await
}

#[tracing::instrument(
    name = "enrollment.page",
    skip_all,
    fields(auth.authority = "shared-auth")
)]
pub(crate) async fn page_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match session_verdict(&state, &headers).await {
        AuthVerdict::Authenticated => {}
        AuthVerdict::Unauthenticated => return Redirect::to("/login").into_response(),
        AuthVerdict::Degraded => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                page(
                    "Temporarily unavailable",
                    html! {
                        div class="card" {
                            h1 { "Enrollment temporarily unavailable" }
                            p class="error" { "Your session could not be verified. Try again shortly." }
                        }
                    },
                ),
            )
                .into_response();
        }
    }

    let mut secret = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut secret);
    let secret_b32 = totp::base32_encode(&secret);
    let email = cookie_value(&headers, EMAIL_COOKIE)
        .filter(|email| !email.is_empty())
        .unwrap_or_else(|| "user".to_owned());
    let uri = totp::otpauth_uri(&email, &secret_b32);
    let qr_svg = qrcode::QrCode::new(uri.as_bytes())
        .map(|code| {
            code.render::<qrcode::render::svg::Color>()
                .min_dimensions(200, 200)
                .build()
        })
        .unwrap_or_default();

    let body = html! {
        div class="card" id="enroll-box" {
            h1 { "Scan with your authenticator" }
            p class="sub" {
                "Scan the QR code with Authy, Google Authenticator, 1Password, or any TOTP app, then enter the 6-digit code it shows."
            }
            div class="qr" { (PreEscaped(qr_svg)) }
            p class="sub" { "Can't scan? Enter this secret manually:" }
            code class="secret" { (secret_b32) }
            form hx-post="/enroll/verify" hx-target="#verify-result" hx-swap="innerHTML" {
                label for="code" { "6-digit code" }
                input id="code" name="code" inputmode="numeric" pattern="[0-9]{6}" minlength="6" maxlength="6" required;
                button type="submit" { "Verify" }
            }
            div id="verify-result" {}
        }
    };

    let mut response = page("Enroll", body).into_response();
    let signed = format!("{secret_b32}.{}", sign(&state.server_secret, &secret_b32));
    response
        .headers_mut()
        .append(header::SET_COOKIE, set_cookie(ENROLL_COOKIE, &signed));
    response
}

#[derive(Deserialize)]
pub(crate) struct VerifyForm {
    code: String,
}

#[tracing::instrument(
    name = "enrollment.verify",
    skip_all,
    fields(auth.authority = "shared-auth")
)]
pub(crate) async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<VerifyForm>,
) -> Response {
    let fragment =
        |class: &str, text: &str| Html(html! { p class=(class) { (text) } }.into_string());
    match session_verdict(&state, &headers).await {
        AuthVerdict::Authenticated => {}
        AuthVerdict::Unauthenticated => {
            return (
                StatusCode::UNAUTHORIZED,
                fragment("error", "Your session expired — sign in again."),
            )
                .into_response();
        }
        AuthVerdict::Degraded => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                fragment(
                    "error",
                    "Your session could not be verified — try again shortly.",
                ),
            )
                .into_response();
        }
    }
    let Some(cookie) = cookie_value(&headers, ENROLL_COOKIE) else {
        return (
            StatusCode::BAD_REQUEST,
            fragment("error", "No enrollment in progress — reload the page."),
        )
            .into_response();
    };
    let Some((secret_b32, mac)) = cookie.split_once('.') else {
        return (
            StatusCode::BAD_REQUEST,
            fragment("error", "Enrollment cookie is malformed — reload the page."),
        )
            .into_response();
    };
    if sign(&state.server_secret, secret_b32) != mac {
        return (
            StatusCode::BAD_REQUEST,
            fragment(
                "error",
                "Enrollment cookie failed verification — reload the page.",
            ),
        )
            .into_response();
    }
    let Some(secret) = totp::base32_decode(secret_b32) else {
        return (
            StatusCode::BAD_REQUEST,
            fragment("error", "Enrollment cookie is malformed — reload the page."),
        )
            .into_response();
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let Some(counter) = totp::matching_counter(&secret, &form.code, now) else {
        tracing::info!(enrollment.outcome = "rejected", "TOTP enrollment rejected");
        return fragment(
            "error",
            "That code didn't match — wait for a fresh code and try again.",
        )
        .into_response();
    };

    // RFC 6238 §5.2: never accept a second attempt of an already-verified
    // code. Track the highest accepted counter per secret and reject any
    // counter at or below it; prune entries once every code they could gate
    // has aged out of the ±skew verification window.
    let now_counter = now / totp::STEP_SECONDS;
    let replayed = {
        let mut used = state
            .used_totp_counters
            .lock()
            .expect("used-counter mutex is never poisoned");
        used.retain(|_, last| last.saturating_add(totp::SKEW_STEPS as u64) >= now_counter);
        match used.get(secret_b32) {
            Some(&last) if counter <= last => true,
            _ => {
                used.insert(secret_b32.to_owned(), counter);
                false
            }
        }
    };
    if replayed {
        tracing::info!(enrollment.outcome = "replayed", "TOTP code replay rejected");
        fragment(
            "error",
            "That code was already used — wait for a fresh code and try again.",
        )
        .into_response()
    } else {
        tracing::info!(enrollment.outcome = "accepted", "TOTP enrollment verified");
        fragment("success", "Device enrolled — your authenticator is linked.").into_response()
    }
}
