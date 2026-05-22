use std::{
    env,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Form, Query},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use data_encoding::{BASE32, BASE32_NOPAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;

static HTTP_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static AUTH_SUCCESSES_TOTAL: AtomicU64 = AtomicU64::new(0);
static AUTH_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize)]
struct AuthQuery {
    #[serde(rename = "return")]
    return_to: Option<String>,
}

#[derive(Deserialize)]
struct PinForm {
    pin: String,
    totp: Option<String>,
    return_to: Option<String>,
    immediate: Option<String>,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
}

#[derive(Serialize)]
struct AuthStatusResponse {
    authenticated: bool,
    #[serde(rename = "totpRequired")]
    totp_required: bool,
    #[serde(rename = "cookieName")]
    cookie_name: String,
}

fn required_env(name: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => panic!("{name} must be configured"),
    }
}

fn auth_pin() -> String {
    required_env("DD_AUTH_PIN")
}

fn cookie_name() -> String {
    env::var("DD_AUTH_COOKIE_NAME").unwrap_or_else(|_| "dd_auth".to_string())
}

fn cookie_value() -> String {
    required_env("DD_AUTH_COOKIE_VALUE")
}

fn cookie_max_age_seconds() -> u64 {
    env::var("DD_AUTH_COOKIE_MAX_AGE_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= 86_400)
        .unwrap_or(3600)
}

fn totp_secret_base32() -> Option<String> {
    env::var("DD_AUTH_TOTP_SECRET_BASE32")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_required_config() {
    let _ = auth_pin();
    let _ = cookie_value();
    if let Some(secret) = totp_secret_base32() {
        decode_totp_secret(&secret).expect("DD_AUTH_TOTP_SECRET_BASE32 must be valid base32");
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..max_len {
        let left_byte = *left.get(index).unwrap_or(&0);
        let right_byte = *right.get(index).unwrap_or(&0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

fn normalize_totp_secret(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn decode_totp_secret(value: &str) -> Result<Vec<u8>, String> {
    let normalized = normalize_totp_secret(value);
    BASE32_NOPAD
        .decode(normalized.as_bytes())
        .or_else(|_| BASE32.decode(normalized.as_bytes()))
        .map_err(|error| format!("invalid TOTP secret: {error}"))
}

fn current_totp_counter() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 30
}

fn totp_code(secret: &[u8], counter: u64) -> Option<String> {
    type HmacSha1 = Hmac<Sha1>;
    let mut mac = HmacSha1::new_from_slice(secret).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    Some(format!("{:06}", binary % 1_000_000))
}

fn valid_totp_code(submitted: Option<&str>, secret_base32: &str) -> bool {
    let Some(submitted) = submitted.map(str::trim).filter(|value| value.len() == 6) else {
        return false;
    };
    if !submitted.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(secret) = decode_totp_secret(secret_base32) else {
        return false;
    };
    let counter = current_totp_counter();
    let window = env::var("DD_AUTH_TOTP_WINDOW_STEPS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0 && *value <= 2)
        .unwrap_or(1);
    for offset in -window..=window {
        let Some(candidate_counter) = counter.checked_add_signed(offset) else {
            continue;
        };
        if let Some(candidate) = totp_code(&secret, candidate_counter) {
            if constant_time_eq(submitted, &candidate) {
                return true;
            }
        }
    }
    false
}

fn auth_form_is_valid(form: &PinForm) -> bool {
    let pin_ok = constant_time_eq(form.pin.trim(), auth_pin().trim());
    let totp_ok = match totp_secret_base32() {
        Some(secret) => valid_totp_code(form.totp.as_deref(), &secret),
        None => true,
    };
    pin_ok && totp_ok
}

fn totp_required() -> bool {
    totp_secret_base32().is_some()
}

// Returns true when the caller already has a valid `dd_auth` cookie that
// matches the configured gateway value. This is what gates the "currently
// signed in" banner on the form and the /auth/status endpoint, so operators
// can confirm whether the browser cookie is actually set without having to
// poke a downstream protected route.
fn caller_is_authenticated(headers: &HeaderMap) -> bool {
    let expected_name = cookie_name();
    let expected_value = cookie_value();
    let Some(cookie_header) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name == expected_name && constant_time_eq(value, &expected_value) {
            return true;
        }
    }
    false
}

fn is_truthy_flag(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn safe_return_to(value: Option<String>) -> String {
    let Some(value) = value else {
        return "/home".to_string();
    };
    if value.starts_with('/') && !value.starts_with("//") {
        value
    } else {
        "/home".to_string()
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn shared_styles() -> &'static str {
    r#"
      :root { color-scheme: dark; }
      body {
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: #0b1117;
        color: #eef2f6;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
      }
      main {
        width: min(440px, calc(100vw - 32px));
        border: 1px solid rgba(148, 163, 184, 0.24);
        border-radius: 8px;
        background: #111923;
        padding: 22px;
      }
      h1 { margin: 0 0 8px; font-size: 22px; }
      p { margin: 0 0 16px; color: #a8b3c1; line-height: 1.5; }
      label { display: grid; gap: 8px; margin-bottom: 14px; }
      label .hint { font-size: 12px; color: #94a3b8; font-weight: 400; }
      input {
        width: 100%;
        border: 1px solid rgba(148, 163, 184, 0.35);
        border-radius: 6px;
        background: #0a1017;
        color: #eef2f6;
        padding: 10px 12px;
        font: inherit;
      }
      button {
        border: 0;
        border-radius: 6px;
        background: #5eead4;
        color: #051014;
        cursor: pointer;
        font-weight: 700;
        padding: 10px 14px;
      }
      code {
        display: inline-block;
        max-width: 100%;
        overflow-wrap: anywhere;
        color: #d7fbf4;
      }
      .banner {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 10px 12px;
        border-radius: 6px;
        margin-bottom: 16px;
        font-size: 13px;
        line-height: 1.4;
      }
      .banner a { color: inherit; text-decoration: underline; }
      .banner.signed-in {
        background: rgba(94, 234, 212, 0.12);
        border: 1px solid rgba(94, 234, 212, 0.45);
        color: #5eead4;
      }
      .banner.signed-out {
        background: rgba(148, 163, 184, 0.08);
        border: 1px solid rgba(148, 163, 184, 0.24);
        color: #cbd5e1;
      }
      .banner.error {
        background: rgba(248, 113, 113, 0.12);
        border: 1px solid rgba(248, 113, 113, 0.5);
        color: #fca5a5;
        font-weight: 600;
      }
      .banner.success {
        background: rgba(94, 234, 212, 0.14);
        border: 1px solid rgba(94, 234, 212, 0.55);
        color: #5eead4;
        font-weight: 600;
      }
      .totp-required { color: #fbbf24; }
      .totp-optional { color: #94a3b8; }
      .actions { display: flex; gap: 12px; align-items: center; }
      .meta { margin-top: 18px; font-size: 12px; color: #64748b; }
      .meta a { color: #94a3b8; }
    "#
}

fn session_banner_html(is_authenticated: bool, return_to_escaped: &str) -> String {
    if is_authenticated {
        format!(
            r#"<div class="banner signed-in" role="status">
              <span>✓ You are currently signed in.</span>
              <a href="{return_to_escaped}">Continue to <code>{return_to_escaped}</code> →</a>
            </div>"#
        )
    } else {
        r#"<div class="banner signed-out" role="status">
              <span>You are not currently signed in. Enter the operator passphrase below.</span>
            </div>"#
            .to_string()
    }
}

fn totp_label_html(totp_required: bool) -> &'static str {
    if totp_required {
        r#"One-time code <span class="hint totp-required">(required — 6-digit TOTP)</span>"#
    } else {
        r#"One-time code <span class="hint totp-optional">(not required — leave blank)</span>"#
    }
}

fn login_page(
    return_to: &str,
    error: Option<&str>,
    is_authenticated: bool,
    totp_required: bool,
) -> Html<String> {
    let escaped_return = escape_html(return_to);
    let styles = shared_styles();
    let session_banner = session_banner_html(is_authenticated, &escaped_return);
    let totp_label = totp_label_html(totp_required);
    let error_html = error
        .map(|message| {
            format!(
                r#"<div class="banner error" role="alert">✗ {}</div>"#,
                escape_html(message)
            )
        })
        .unwrap_or_default();
    Html(format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd remote auth</title>
    <style>{styles}</style>
  </head>
  <body>
    <main>
      <h1>Remote runtime auth</h1>
      <p>Enter the operator passphrase to set the browser cookie and return to <code>{escaped_return}</code>.</p>
      {session_banner}
      {error_html}
      <form method="post" action="/auth">
        <input type="hidden" name="return_to" value="{escaped_return}" />
        <label>
          Operator passphrase
          <input name="pin" type="password" autocomplete="current-password" autofocus />
        </label>
        <label>
          {totp_label}
          <input name="totp" inputmode="numeric" autocomplete="one-time-code" maxlength="6" />
        </label>
        <button type="submit">Continue</button>
      </form>
      <p class="meta">Check current state at <a href="/auth/status">/auth/status</a>.</p>
    </main>
  </body>
</html>"#
    ))
}

fn success_page(return_to: &str) -> Html<String> {
    let escaped_return = escape_html(return_to);
    let styles = shared_styles();
    Html(format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd remote auth — signed in</title>
    <meta http-equiv="refresh" content="2; url={escaped_return}" />
    <style>{styles}</style>
  </head>
  <body>
    <main>
      <h1>Signed in</h1>
      <div class="banner success" role="status">
        <span>✓ Logged in successfully. Browser cookie was set.</span>
      </div>
      <p>Redirecting to <code>{escaped_return}</code> in 2 seconds.</p>
      <p class="actions">
        <a href="{escaped_return}"><button type="button">Continue now</button></a>
      </p>
      <p class="meta">Re-check at any time via <a href="/auth/status">/auth/status</a>.</p>
    </main>
  </body>
</html>"#
    ))
}

async fn auth_form(Query(query): Query<AuthQuery>, headers: HeaderMap) -> impl IntoResponse {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let return_to = safe_return_to(query.return_to);
    login_page(
        &return_to,
        None,
        caller_is_authenticated(&headers),
        totp_required(),
    )
}

async fn auth_submit(headers: HeaderMap, Form(form): Form<PinForm>) -> Response {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let return_to = safe_return_to(form.return_to.clone());
    let already_authenticated = caller_is_authenticated(&headers);
    if !auth_form_is_valid(&form) {
        AUTH_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
        let error_message = if totp_required() {
            "Incorrect operator passphrase or one-time code. Please try again."
        } else {
            "Incorrect operator passphrase. Please try again."
        };
        return (
            StatusCode::UNAUTHORIZED,
            login_page(
                &return_to,
                Some(error_message),
                already_authenticated,
                totp_required(),
            ),
        )
            .into_response();
    }
    AUTH_SUCCESSES_TOTAL.fetch_add(1, Ordering::Relaxed);

    let cookie = format!(
        "{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax; Secure",
        cookie_name(),
        cookie_value(),
        cookie_max_age_seconds()
    );
    let cookie_header =
        HeaderValue::from_str(&cookie).expect("auth cookie header should be valid");

    // Programmatic callers (curl, scripts) can keep the old immediate-redirect
    // behavior by posting `immediate=1`. The default browser flow is now a
    // visible "Signed in" confirmation page that sets the cookie and auto-
    // redirects via meta refresh, so operators can actually see whether login
    // succeeded instead of staring at a silent 3xx.
    if is_truthy_flag(form.immediate.as_deref()) {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = StatusCode::SEE_OTHER;
        response.headers_mut().insert(
            header::LOCATION,
            HeaderValue::from_str(&return_to).unwrap_or_else(|_| HeaderValue::from_static("/home")),
        );
        response.headers_mut().insert(header::SET_COOKIE, cookie_header);
        return response;
    }

    let mut response = success_page(&return_to).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cookie_header);
    response
}

async fn auth_status(headers: HeaderMap) -> Json<AuthStatusResponse> {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    Json(AuthStatusResponse {
        authenticated: caller_is_authenticated(&headers),
        totp_required: totp_required(),
        cookie_name: cookie_name(),
    })
}

async fn healthz() -> impl IntoResponse {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    Json(HealthResponse {
        ok: true,
        service: "dd-remote-auth",
    })
}

async fn metrics() -> Response {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let body = format!(
        concat!(
            "# HELP dd_remote_auth_build_info Remote auth build metadata.\n",
            "# TYPE dd_remote_auth_build_info gauge\n",
            "dd_remote_auth_build_info{{service=\"dd-remote-auth\"}} 1\n",
            "# HELP dd_remote_auth_http_requests_total HTTP requests handled by remote auth.\n",
            "# TYPE dd_remote_auth_http_requests_total counter\n",
            "dd_remote_auth_http_requests_total {}\n",
            "# HELP dd_remote_auth_successes_total Successful auth submissions.\n",
            "# TYPE dd_remote_auth_successes_total counter\n",
            "dd_remote_auth_successes_total {}\n",
            "# HELP dd_remote_auth_failures_total Failed auth submissions.\n",
            "# TYPE dd_remote_auth_failures_total counter\n",
            "dd_remote_auth_failures_total {}\n"
        ),
        HTTP_REQUESTS_TOTAL.load(Ordering::Relaxed),
        AUTH_SUCCESSES_TOTAL.load(Ordering::Relaxed),
        AUTH_FAILURES_TOTAL.load(Ordering::Relaxed)
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    validate_required_config();

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8083);

    let app = Router::new()
        .route("/auth", get(auth_form).post(auth_submit))
        .route("/auth/", get(auth_form).post(auth_submit))
        .route("/auth/status", get(auth_status))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("dd-remote-auth listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
