use std::{
    env,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Form, Query},
    http::{header, HeaderValue, StatusCode},
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
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
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

fn login_page(return_to: &str, error: Option<&str>) -> Html<String> {
    let escaped_return = escape_html(return_to);
    let error_html = error
        .map(|message| format!(r#"<p class="error">{}</p>"#, escape_html(message)))
        .unwrap_or_default();
    Html(format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd remote auth</title>
    <style>
      :root {{ color-scheme: dark; }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: #0b1117;
        color: #eef2f6;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
      }}
      main {{
        width: min(420px, calc(100vw - 32px));
        border: 1px solid rgba(148, 163, 184, 0.24);
        border-radius: 8px;
        background: #111923;
        padding: 22px;
      }}
      h1 {{ margin: 0 0 8px; font-size: 22px; }}
      p {{ margin: 0 0 16px; color: #a8b3c1; line-height: 1.5; }}
      label {{ display: grid; gap: 8px; margin-bottom: 14px; }}
      input {{
        width: 100%;
        border: 1px solid rgba(148, 163, 184, 0.35);
        border-radius: 6px;
        background: #0a1017;
        color: #eef2f6;
        padding: 10px 12px;
        font: inherit;
      }}
      button {{
        border: 0;
        border-radius: 6px;
        background: #5eead4;
        color: #051014;
        cursor: pointer;
        font-weight: 700;
        padding: 10px 14px;
      }}
      code {{
        display: inline-block;
        max-width: 100%;
        overflow-wrap: anywhere;
        color: #d7fbf4;
      }}
      .error {{ color: #fca5a5; }}
    </style>
  </head>
  <body>
    <main>
      <h1>Remote runtime auth</h1>
      <p>Enter the operator passphrase to set the browser cookie and return to <code>{escaped_return}</code>.</p>
      {error_html}
      <form method="post" action="/auth">
        <input type="hidden" name="return_to" value="{escaped_return}" />
        <label>
          Operator passphrase
          <input name="pin" autocomplete="current-password" autofocus />
        </label>
        <label>
          One-time code
          <input name="totp" inputmode="numeric" autocomplete="one-time-code" />
        </label>
        <button type="submit">Continue</button>
      </form>
    </main>
  </body>
</html>"#
    ))
}

async fn auth_form(Query(query): Query<AuthQuery>) -> impl IntoResponse {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let return_to = safe_return_to(query.return_to);
    login_page(&return_to, None)
}

async fn auth_submit(Form(form): Form<PinForm>) -> Response {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let return_to = safe_return_to(form.return_to.clone());
    if !auth_form_is_valid(&form) {
        AUTH_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::UNAUTHORIZED,
            login_page(&return_to, Some("Incorrect passphrase or one-time code")),
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
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::SEE_OTHER;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&return_to).unwrap_or_else(|_| HeaderValue::from_static("/home")),
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("auth cookie header should be valid"),
    );
    response
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
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics));

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
