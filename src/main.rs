//! 3FA web app — Supabase login + TOTP enrollment demo.
//!
//! MASH stack: maud (HTML) + axum (HTTP) + htmx (interactivity). SeaORM is the
//! house ORM, but this v1 is deliberately database-less — see readme.md.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Form, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use hmac::{Hmac, Mac};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use rand::RngCore;
use serde::Deserialize;
use sha1::Sha1;

mod totp;

const SESSION_COOKIE: &str = "threefa_session";
const EMAIL_COOKIE: &str = "threefa_email";
const ENROLL_COOKIE: &str = "threefa_enroll";

#[derive(Clone)]
struct SupabaseConfig {
    url: String,
    anon_key: String,
}

/// Build a Supabase config from the two env values; both must be non-empty.
fn supabase_config(url: Option<String>, anon_key: Option<String>) -> Option<SupabaseConfig> {
    let url = url.map(|v| v.trim().trim_end_matches('/').to_string())?;
    let anon_key = anon_key.map(|v| v.trim().to_string())?;
    if url.is_empty() || anon_key.is_empty() {
        return None;
    }
    Some(SupabaseConfig { url, anon_key })
}

#[derive(Clone)]
struct AppState {
    supabase: Option<SupabaseConfig>,
    /// Key for HMAC-signing the enrollment-secret cookie. From SERVER_SECRET,
    /// or random at boot (enrollments then don't survive restarts — fine for
    /// a demo flow).
    server_secret: Arc<Vec<u8>>,
    http: reqwest::Client,
}

impl AppState {
    fn new(supabase: Option<SupabaseConfig>, server_secret: Vec<u8>) -> Self {
        Self {
            supabase,
            server_secret: Arc::new(server_secret),
            http: reqwest::Client::new(),
        }
    }

    fn from_env() -> Self {
        let supabase = supabase_config(
            std::env::var("SUPABASE_URL").ok(),
            std::env::var("SUPABASE_ANON_KEY").ok(),
        );
        let server_secret = match std::env::var("SERVER_SECRET") {
            Ok(v) if !v.trim().is_empty() => v.into_bytes(),
            _ => {
                let mut buf = vec![0u8; 32];
                rand::thread_rng().fill_bytes(&mut buf);
                buf
            }
        };
        Self::new(supabase, server_secret)
    }
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(login_page))
        .route("/login", get(login_page).post(login_submit))
        .route("/enroll", get(enroll_page))
        .route("/enroll/verify", post(enroll_verify))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("bind");
    axum::serve(listener, app(AppState::from_env()))
        .await
        .expect("serve");
}

// --- cookies -----------------------------------------------------------------

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

fn set_cookie(name: &str, value: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Secure"
    ))
    .expect("cookie header value")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        out.push_str(&format!("{b:02x}"));
        out
    })
}

fn sign(key: &[u8], message: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    hex(&mac.finalize().into_bytes())
}

// --- page shell --------------------------------------------------------------

const CSS: &str = r#"
:root { color-scheme: dark; }
* { box-sizing: border-box; }
body {
  margin: 0; min-height: 100vh; display: grid; place-items: center;
  background: #0b1020; color: #e6e9f2;
  font: 16px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
}
main { width: min(26rem, 92vw); padding: 2rem 0 3rem; }
.card {
  background: #141a30; border: 1px solid #26304f; border-radius: 12px;
  padding: 1.75rem; box-shadow: 0 8px 30px rgba(0, 0, 0, 0.35);
}
h1 { font-size: 1.35rem; margin: 0 0 0.5rem; }
.brand { letter-spacing: 0.08em; font-weight: 700; color: #7aa2ff; margin: 0 0 1.25rem; }
p.sub { color: #9aa4c0; margin: 0 0 1.25rem; }
label { display: block; font-size: 0.85rem; color: #9aa4c0; margin: 0.75rem 0 0.25rem; }
input {
  width: 100%; padding: 0.6rem 0.7rem; border-radius: 8px;
  border: 1px solid #2c375f; background: #0d1226; color: inherit; font: inherit;
}
button {
  margin-top: 1.1rem; width: 100%; padding: 0.65rem; border: 0; border-radius: 8px;
  background: #3b6cff; color: #fff; font: inherit; font-weight: 600; cursor: pointer;
}
button:hover { background: #2f57d6; }
.error { color: #ff8f8f; margin: 0.75rem 0 0; }
.notice { color: #ffd479; margin: 0.75rem 0 0; }
.success { color: #7fe0a7; margin: 0.75rem 0 0; }
.qr { background: #fff; border-radius: 8px; padding: 0.75rem; margin: 1rem 0; }
.qr svg { display: block; width: 100%; height: auto; }
code.secret {
  display: block; word-break: break-all; background: #0d1226; border-radius: 8px;
  padding: 0.6rem 0.7rem; font-size: 0.85rem; color: #b7c2e0;
}
"#;

fn page(title: &str, body: Markup) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (title) " — 3FA" }
                    style { (PreEscaped(CSS)) }
                    script defer="defer" src="https://unpkg.com/htmx.org@2.0.4" {}
                }
                body {
                    main {
                        p class="brand" { "3FA" }
                        (body)
                    }
                }
            }
        }
        .into_string(),
    )
}

// --- login -------------------------------------------------------------------

fn login_form(error: Option<&str>, configured: bool) -> Markup {
    html! {
        div class="card" id="login-box" {
            h1 { "Sign in to 3FA" }
            p class="sub" { "Use your 3FA account to open the web authenticator." }
            form hx-post="/login" hx-target="#login-box" hx-swap="outerHTML" {
                label for="email" { "Email" }
                input id="email" name="email" type="email" required autocomplete="username";
                label for="password" { "Password" }
                input id="password" name="password" type="password" required autocomplete="current-password";
                button type="submit" { "Sign in" }
            }
            @if let Some(error) = error {
                p class="error" { (error) }
            }
            @if !configured {
                p class="notice" {
                    "Supabase not configured — set SUPABASE_URL and SUPABASE_ANON_KEY to enable sign-in."
                }
            }
        }
    }
}

async fn login_page(State(state): State<AppState>) -> impl IntoResponse {
    page("Sign in", login_form(None, state.supabase.is_some()))
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

async fn login_submit(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let Some(supabase) = &state.supabase else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(
                html! {
                    div class="card" id="login-box" {
                        h1 { "Sign in to 3FA" }
                        p class="error" {
                            "Supabase not configured — set SUPABASE_URL and SUPABASE_ANON_KEY on the server."
                        }
                    }
                }
                .into_string(),
            ),
        )
            .into_response();
    };

    let result = state
        .http
        .post(format!(
            "{}/auth/v1/token?grant_type=password",
            supabase.url
        ))
        .header("apikey", &supabase.anon_key)
        .bearer_auth(&supabase.anon_key)
        .json(&serde_json::json!({ "email": form.email, "password": form.password }))
        .send()
        .await;

    let response = match result {
        Ok(response) => response,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                page(
                    "Sign in",
                    login_form(Some("Could not reach the auth service — try again."), true),
                ),
            )
                .into_response();
        }
    };

    if !response.status().is_success() {
        return (
            StatusCode::UNAUTHORIZED,
            Html(login_form(Some("Invalid email or password."), true).into_string()),
        )
            .into_response();
    }

    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let Some(access_token) = body["access_token"].as_str().filter(|t| !t.is_empty()) else {
        return (
            StatusCode::BAD_GATEWAY,
            Html(login_form(Some("Unexpected auth response — try again."), true).into_string()),
        )
            .into_response();
    };
    let email = body["user"]["email"].as_str().unwrap_or_default();

    let mut response = (
        StatusCode::OK,
        Html(
            html! {
                div class="card" id="login-box" {
                    p class="success" { "Signed in — loading enrollment…" }
                }
            }
            .into_string(),
        ),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.append(header::SET_COOKIE, set_cookie(SESSION_COOKIE, access_token));
    if !email.is_empty() {
        headers.append(header::SET_COOKIE, set_cookie(EMAIL_COOKIE, email));
    }
    // htmx follows this client-side; plain form posts still get the fragment.
    headers.insert("HX-Redirect", HeaderValue::from_static("/enroll"));
    response
}

// --- enrollment --------------------------------------------------------------

/// Session gate: cookie must be present and non-empty. Full JWT verification
/// (signature against the Supabase JWKS, expiry, and a SeaORM-backed user
/// table) is a TODO for when this service grows a database.
fn session_token(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, SESSION_COOKIE).filter(|token| !token.trim().is_empty())
}

async fn enroll_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if session_token(&headers).is_none() {
        return Redirect::to("/login").into_response();
    }

    let mut secret = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut secret);
    let secret_b32 = totp::base32_encode(&secret);

    let email = cookie_value(&headers, EMAIL_COOKIE)
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "user".to_string());
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
struct VerifyForm {
    code: String,
}

async fn enroll_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<VerifyForm>,
) -> Response {
    let fragment =
        |class: &str, text: &str| Html(html! { p class=(class) { (text) } }.into_string());

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
    if totp::verify_totp(&secret, &form.code, now) {
        fragment("success", "Device enrolled — your authenticator is linked.").into_response()
    } else {
        fragment(
            "error",
            "That code didn't match — wait for a fresh code and try again.",
        )
        .into_response()
    }
}

// --- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const ENROLL_HEADING: &str = "Scan with your authenticator";
    const ENROLL_SUBTEXT: &str = "Scan the QR code with Authy, Google Authenticator, 1Password, \
                                  or any TOTP app, then enter the 6-digit code it shows.";

    fn test_state() -> AppState {
        AppState::new(None, b"test-server-secret".to_vec())
    }

    async fn body_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn supabase_config_requires_both_values() {
        assert!(supabase_config(None, None).is_none());
        assert!(supabase_config(Some("https://x.supabase.co".into()), None).is_none());
        assert!(supabase_config(None, Some("anon".into())).is_none());
        assert!(supabase_config(Some("  ".into()), Some("anon".into())).is_none());
        let config = supabase_config(
            Some("https://x.supabase.co/".into()),
            Some("anon-key".into()),
        )
        .expect("configured");
        assert_eq!(config.url, "https://x.supabase.co");
        assert_eq!(config.anon_key, "anon-key");
    }

    #[tokio::test]
    async fn root_and_login_render_login_form() {
        for uri in ["/", "/login"] {
            let response = app(test_state())
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "status for {uri}");
            let body = body_string(response).await;
            assert!(body.contains(r#"hx-post="/login""#), "form post for {uri}");
            assert!(body.contains(r#"name="email""#), "email input for {uri}");
            assert!(
                body.contains(r#"name="password""#),
                "password input for {uri}"
            );
            // Unconfigured state must still render the page, with a notice.
            assert!(body.contains("Supabase not configured"), "notice for {uri}");
        }
    }

    #[tokio::test]
    async fn login_without_supabase_config_is_503() {
        let response = app(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("email=sam%40example.com&password=hunter2"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_string(response).await;
        assert!(body.contains("Supabase not configured"));
    }

    #[tokio::test]
    async fn enroll_without_session_redirects_to_login() {
        let response = app(test_state())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/login");
    }

    #[tokio::test]
    async fn enroll_with_session_shows_exact_copy_and_qr() {
        let response = app(test_state())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .header(header::COOKIE, "threefa_session=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .expect("enroll cookie set")
            .to_str()
            .unwrap()
            .to_string();
        assert!(set_cookie.starts_with("threefa_enroll="));
        let body = body_string(response).await;
        assert!(body.contains(ENROLL_HEADING), "missing heading");
        assert!(body.contains(ENROLL_SUBTEXT), "missing subtext");
        assert!(body.contains("<svg"), "missing inline SVG QR");
        assert!(
            body.contains("otpauth") || body.contains("secret"),
            "fallback secret"
        );
    }

    /// Drive the real flow: GET /enroll to obtain the signed secret cookie,
    /// compute the expected TOTP code from that secret, then POST it.
    async fn enroll_cookie() -> (String, Vec<u8>) {
        let response = app(test_state())
            .oneshot(
                Request::builder()
                    .uri("/enroll")
                    .header(header::COOKIE, "threefa_session=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        let value = set_cookie
            .strip_prefix("threefa_enroll=")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let secret_b32 = value.split('.').next().unwrap();
        let secret = totp::base32_decode(secret_b32).expect("cookie secret decodes");
        (value, secret)
    }

    async fn post_verify(cookie: &str, code: &str) -> (StatusCode, String) {
        let response = app(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/enroll/verify")
                    .header(
                        header::COOKIE,
                        format!("threefa_session=test-token; threefa_enroll={cookie}"),
                    )
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("code={code}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (status, body_string(response).await)
    }

    #[tokio::test]
    async fn verify_accepts_valid_code() {
        let (cookie, secret) = enroll_cookie().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let code = totp::totp_code(&secret, now);
        let (status, body) = post_verify(&cookie, &code).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Device enrolled"), "body: {body}");
    }

    #[tokio::test]
    async fn verify_rejects_wrong_code() {
        let (cookie, secret) = enroll_cookie().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Pick a 6-digit code that is not valid in any accepted skew window.
        let valid: Vec<String> = (-totp::SKEW_STEPS..=totp::SKEW_STEPS)
            .filter_map(|offset| (now / totp::STEP_SECONDS).checked_add_signed(offset))
            .map(|counter| totp::hotp(&secret, counter, 6))
            .collect();
        let wrong = (0..=3u32)
            .map(|n| format!("{n:06}"))
            .find(|candidate| !valid.contains(candidate))
            .unwrap();
        let (status, body) = post_verify(&cookie, &wrong).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("Device enrolled"));
        assert!(
            body.contains("didn't match") || body.contains("didn&#39;t match"),
            "body: {body}"
        );
    }

    #[tokio::test]
    async fn verify_rejects_tampered_cookie() {
        let (cookie, secret) = enroll_cookie().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let code = totp::totp_code(&secret, now);
        // Flip the secret portion; the HMAC no longer matches.
        let tampered = format!("AAAAAAAA{}", &cookie[8..]);
        let (status, _body) = post_verify(&tampered, &code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn healthz_ok() {
        let response = app(test_state())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, "ok");
    }
}
