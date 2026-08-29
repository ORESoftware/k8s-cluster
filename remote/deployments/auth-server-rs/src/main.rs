use std::{
    collections::HashMap,
    env,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{ConnectInfo, Extension, Form, Query},
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
static AUTH_RATE_LIMITED_TOTAL: AtomicU64 = AtomicU64::new(0);

const DEFAULT_AUTH_FAILURE_LIMIT: u32 = 5;
const DEFAULT_AUTH_LOCKOUT_SECONDS: u64 = 15 * 60;
const MAX_AUTH_FAILURE_LIMIT: u32 = 20;
const MIN_AUTH_LOCKOUT_SECONDS: u64 = 60;
const MAX_AUTH_LOCKOUT_SECONDS: u64 = 24 * 60 * 60;
const AUTH_ATTEMPT_ENTRY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone)]
struct AuthAttemptLimiter {
    entries: Arc<Mutex<HashMap<IpAddr, AuthAttempt>>>,
    failure_limit: u32,
    lockout: Duration,
}

struct AuthAttempt {
    failures: u32,
    locked_until: Option<Instant>,
    last_seen: Instant,
}

impl AuthAttemptLimiter {
    fn from_env() -> Self {
        let failure_limit = env::var("DD_AUTH_FAILURE_LIMIT")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|value| *value > 0 && *value <= MAX_AUTH_FAILURE_LIMIT)
            .unwrap_or(DEFAULT_AUTH_FAILURE_LIMIT);
        let lockout_seconds = env::var("DD_AUTH_LOCKOUT_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| {
                *value >= MIN_AUTH_LOCKOUT_SECONDS && *value <= MAX_AUTH_LOCKOUT_SECONDS
            })
            .unwrap_or(DEFAULT_AUTH_LOCKOUT_SECONDS);
        Self::new(failure_limit, Duration::from_secs(lockout_seconds))
    }

    fn new(failure_limit: u32, lockout: Duration) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            failure_limit,
            lockout,
        }
    }

    fn retry_after(&self, client_ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.retain(|_, entry| {
            entry.locked_until.is_some_and(|until| until > now)
                || now.duration_since(entry.last_seen) <= AUTH_ATTEMPT_ENTRY_TTL
        });
        let entry = entries.get_mut(&client_ip)?;
        entry.last_seen = now;
        entry
            .locked_until
            .filter(|until| *until > now)
            .map(|until| until.duration_since(now))
    }

    fn record_failure(&self, client_ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.retain(|_, entry| {
            entry.locked_until.is_some_and(|until| until > now)
                || now.duration_since(entry.last_seen) <= AUTH_ATTEMPT_ENTRY_TTL
        });
        let entry = entries.entry(client_ip).or_insert(AuthAttempt {
            failures: 0,
            locked_until: None,
            last_seen: now,
        });
        entry.last_seen = now;
        entry.failures = entry.failures.saturating_add(1);
        if entry.failures >= self.failure_limit {
            entry.failures = 0;
            entry.locked_until = Some(now + self.lockout);
            Some(self.lockout)
        } else {
            None
        }
    }

    fn record_success(&self, client_ip: IpAddr) {
        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.remove(&client_ip);
    }
}

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
    // Cap and default to 3 days. The operator passphrase + optional TOTP gate
    // is still the primary trust boundary; 3 days is the longest "I logged in
    // earlier this week" window we want to honor without forcing a re-auth.
    const THREE_DAYS_SECONDS: u64 = 3 * 24 * 60 * 60;
    env::var("DD_AUTH_COOKIE_MAX_AGE_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= THREE_DAYS_SECONDS)
        .unwrap_or(THREE_DAYS_SECONDS)
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
    // Must be a local absolute path. Reject `//host` (protocol-relative) and also
    // `/\host` (and `/\/host`): a backslash right after the leading slash is
    // folded to `/` by browsers, so `/\evil.com` resolves to `//evil.com` ->
    // https://evil.com — an open redirect off the auth gate.
    if value.starts_with('/') && !value.starts_with("//") && !value.starts_with("/\\") {
        value
    } else {
        "/home".to_string()
    }
}

// The public gateway appends the observed peer to X-Forwarded-For, so the last
// valid address cannot be supplied by the browser. If the service is reached
// directly, fall back to the TCP peer address.
fn client_ip(headers: &HeaderMap, peer_ip: IpAddr) -> IpAddr {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .rsplit(',')
                .map(str::trim)
                .find_map(|value| value.parse::<IpAddr>().ok())
        })
        .unwrap_or(peer_ip)
}

fn too_many_attempts(retry_after: Duration) -> Response {
    AUTH_RATE_LIMITED_TOTAL.fetch_add(1, Ordering::Relaxed);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        "Too many failed sign-in attempts. Please try again later.",
    )
        .into_response();
    let seconds = retry_after.as_secs().max(1).to_string();
    if let Ok(value) = HeaderValue::from_str(&seconds) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
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

async fn auth_submit(
    Extension(attempts): Extension<AuthAttemptLimiter>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<PinForm>,
) -> Response {
    HTTP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let client_ip = client_ip(&headers, peer.ip());
    if let Some(retry_after) = attempts.retry_after(client_ip) {
        return too_many_attempts(retry_after);
    }
    let return_to = safe_return_to(form.return_to.clone());
    let already_authenticated = caller_is_authenticated(&headers);
    if !auth_form_is_valid(&form) {
        AUTH_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
        if let Some(retry_after) = attempts.record_failure(client_ip) {
            return too_many_attempts(retry_after);
        }
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
    attempts.record_success(client_ip);

    let cookie = format!(
        "{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax; Secure",
        cookie_name(),
        cookie_value(),
        cookie_max_age_seconds()
    );
    let cookie_header = HeaderValue::from_str(&cookie).expect("auth cookie header should be valid");

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
        response
            .headers_mut()
            .insert(header::SET_COOKIE, cookie_header);
        return response;
    }

    let mut response = success_page(&return_to).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie_header);
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
            "dd_remote_auth_failures_total {}\n",
            "# HELP dd_remote_auth_rate_limited_total Authentication submissions rejected by the server-side lockout.\n",
            "# TYPE dd_remote_auth_rate_limited_total counter\n",
            "dd_remote_auth_rate_limited_total {}\n"
        ),
        HTTP_REQUESTS_TOTAL.load(Ordering::Relaxed),
        AUTH_SUCCESSES_TOTAL.load(Ordering::Relaxed),
        AUTH_FAILURES_TOTAL.load(Ordering::Relaxed),
        AUTH_RATE_LIMITED_TOTAL.load(Ordering::Relaxed)
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

async fn add_security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        axum::http::HeaderName::from_static("strict-transport-security"),
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; object-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self' data:",
        ),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), geolocation=(), microphone=()"),
    );
    response
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-remote-auth");

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
        .merge(dd_runtime_config_client::router())
        .layer(Extension(AuthAttemptLimiter::from_env()))
        .layer(axum::middleware::map_response(add_security_headers));

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!("dd-remote-auth listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(
        listener,
        app.layer(dd_telemetry::http_trace_layer())
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{MutexGuard, OnceLock};

    // ------------------------------------------------------------------
    // Test scaffolding
    // ------------------------------------------------------------------

    // Several units read process-global environment variables. `cargo test`
    // runs tests in parallel threads, so any test that reads OR writes env
    // must serialize through this lock and restore prior values on drop.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        #[allow(dead_code)]
        _guard: MutexGuard<'static, ()>,
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new(vars: &[(&str, Option<&str>)]) -> Self {
            let guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
            let mut saved = Vec::new();
            for (key, value) in vars {
                saved.push(((*key).to_string(), env::var(key).ok()));
                match value {
                    Some(val) => env::set_var(key, val),
                    None => env::remove_var(key),
                }
            }
            Self {
                _guard: guard,
                saved,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(val) => env::set_var(key, val),
                    None => env::remove_var(key),
                }
            }
        }
    }

    fn cookie_headers(raw: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(raw).unwrap());
        headers
    }

    fn pin_form(pin: &str, totp: Option<&str>) -> PinForm {
        PinForm {
            pin: pin.to_string(),
            totp: totp.map(str::to_string),
            return_to: None,
            immediate: None,
        }
    }

    fn submit_form(pin: &str, return_to: &str, immediate: Option<&str>) -> PinForm {
        PinForm {
            pin: pin.to_string(),
            totp: None,
            return_to: Some(return_to.to_string()),
            immediate: immediate.map(str::to_string),
        }
    }

    fn test_socket() -> SocketAddr {
        "203.0.113.99:44444".parse().unwrap()
    }

    // RFC 6238 Appendix B SHA1 seed, ASCII "12345678901234567890".
    const RFC6238_SECRET_B32: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    // ==================================================================
    // Rate limiter / lockout  (pure, no env)
    // ==================================================================

    #[test]
    fn auth_attempt_limiter_locks_after_configured_failures() {
        let limiter = AuthAttemptLimiter::new(2, Duration::from_secs(60));
        let ip = "203.0.113.10".parse().unwrap();

        assert!(limiter.record_failure(ip).is_none());
        assert!(limiter.record_failure(ip).is_some());
        assert!(limiter.retry_after(ip).is_some());

        limiter.record_success(ip);
        assert!(limiter.retry_after(ip).is_none());
    }

    #[test]
    fn auth_attempt_limiter_tracks_ips_independently() {
        let limiter = AuthAttemptLimiter::new(2, Duration::from_secs(60));
        let a: IpAddr = "203.0.113.1".parse().unwrap();
        let b: IpAddr = "203.0.113.2".parse().unwrap();

        assert!(limiter.record_failure(a).is_none());
        assert!(limiter.record_failure(a).is_some()); // a is now locked
        assert!(limiter.retry_after(a).is_some());
        // A different attacker/IP must not inherit another IP's lockout.
        assert!(limiter.retry_after(b).is_none());
        assert!(limiter.record_failure(b).is_none());
    }

    #[test]
    fn auth_attempt_limiter_retry_after_is_none_for_unknown_ip() {
        let limiter = AuthAttemptLimiter::new(5, Duration::from_secs(60));
        assert!(limiter
            .retry_after("192.0.2.55".parse().unwrap())
            .is_none());
    }

    #[test]
    fn auth_attempt_limiter_success_resets_the_failure_counter() {
        let limiter = AuthAttemptLimiter::new(3, Duration::from_secs(60));
        let ip: IpAddr = "203.0.113.3".parse().unwrap();

        assert!(limiter.record_failure(ip).is_none());
        assert!(limiter.record_failure(ip).is_none());
        limiter.record_success(ip); // clears accrued failures
                                     // It should take the full limit (3) again to lock.
        assert!(limiter.record_failure(ip).is_none());
        assert!(limiter.record_failure(ip).is_none());
        assert!(limiter.record_failure(ip).is_some());
    }

    #[test]
    fn auth_attempt_limiter_from_env_clamps_out_of_range_values() {
        {
            // failure limit 0 (invalid) and lockout below the floor -> defaults.
            let _g = EnvGuard::new(&[
                ("DD_AUTH_FAILURE_LIMIT", Some("0")),
                ("DD_AUTH_LOCKOUT_SECONDS", Some("5")),
            ]);
            let limiter = AuthAttemptLimiter::from_env();
            assert_eq!(limiter.failure_limit, DEFAULT_AUTH_FAILURE_LIMIT);
            assert_eq!(
                limiter.lockout,
                Duration::from_secs(DEFAULT_AUTH_LOCKOUT_SECONDS)
            );
        }
        {
            // Over-large values are rejected -> defaults (bounds the DoS/skew).
            let _g = EnvGuard::new(&[
                ("DD_AUTH_FAILURE_LIMIT", Some("999")),
                ("DD_AUTH_LOCKOUT_SECONDS", Some("999999999")),
            ]);
            let limiter = AuthAttemptLimiter::from_env();
            assert_eq!(limiter.failure_limit, DEFAULT_AUTH_FAILURE_LIMIT);
            assert_eq!(
                limiter.lockout,
                Duration::from_secs(DEFAULT_AUTH_LOCKOUT_SECONDS)
            );
        }
        {
            // In-range overrides are honored.
            let _g = EnvGuard::new(&[
                ("DD_AUTH_FAILURE_LIMIT", Some("3")),
                ("DD_AUTH_LOCKOUT_SECONDS", Some("120")),
            ]);
            let limiter = AuthAttemptLimiter::from_env();
            assert_eq!(limiter.failure_limit, 3);
            assert_eq!(limiter.lockout, Duration::from_secs(120));
        }
    }

    // ==================================================================
    // client_ip trust boundary  (pure)
    // ==================================================================

    #[test]
    fn gateway_client_ip_uses_the_last_forwarded_hop() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.9, 203.0.113.25"),
        );
        let peer = "10.0.0.4".parse().unwrap();

        assert_eq!(
            client_ip(&headers, peer),
            "203.0.113.25".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_prefers_rightmost_hop_over_client_spoofed_left() {
        // The gateway appends the observed peer as the LAST XFF entry, so a
        // browser cannot forge the rate-limit key by prepending fakes. If it
        // could, an attacker would evade lockout (rotate the key) or lock out a
        // victim (spoof their IP). Rightmost-wins is the security property.
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("1.1.1.1, 10.0.0.5, 203.0.113.7"),
        );
        assert_eq!(
            client_ip(&headers, "10.0.0.9".parse().unwrap()),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_falls_back_to_peer_when_forwarded_absent_or_garbage() {
        let peer: IpAddr = "198.51.100.2".parse().unwrap();
        assert_eq!(client_ip(&HeaderMap::new(), peer), peer);

        let mut all_garbage = HeaderMap::new();
        all_garbage.insert(
            "x-forwarded-for",
            HeaderValue::from_static("not-an-ip, still-not"),
        );
        assert_eq!(client_ip(&all_garbage, peer), peer);

        // Trailing junk falls back to the last *parseable* hop.
        let mut trailing_junk = HeaderMap::new();
        trailing_junk.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.7, garbage"));
        assert_eq!(
            client_ip(&trailing_junk, peer),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_handles_ipv6_forwarded_hops() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("2001:db8::1, 2001:db8::2"),
        );
        assert_eq!(
            client_ip(&headers, "10.0.0.1".parse().unwrap()),
            "2001:db8::2".parse::<IpAddr>().unwrap()
        );
    }

    // ==================================================================
    // Open-redirect guard: safe_return_to  (pure)
    // ==================================================================

    #[test]
    fn safe_return_to_allows_legit_local_paths() {
        for path in [
            "/home",
            "/desired/path",
            "/a/b/c?x=1&y=2",
            "/",
            "/dashboard#frag",
        ] {
            assert_eq!(
                safe_return_to(Some(path.to_string())),
                path,
                "local path {path} should pass through"
            );
        }
    }

    #[test]
    fn safe_return_to_defaults_when_missing() {
        assert_eq!(safe_return_to(None), "/home");
    }

    #[test]
    fn safe_return_to_rejects_absolute_and_protocol_relative_urls() {
        for evil in [
            "https://evil.com",
            "http://evil.com/path",
            "https://evil.com/\\@good", // still absolute
            "//evil.com",
            "///evil.com",
        ] {
            assert_eq!(
                safe_return_to(Some(evil.to_string())),
                "/home",
                "{evil} must be neutralized to /home"
            );
        }
    }

    #[test]
    fn safe_return_to_rejects_non_slash_leading_values() {
        for evil in [
            "evil.com",
            "javascript:alert(1)",
            "",
            " /home", // leading space -> does not start with '/'
            "\\\\evil.com",
            "mailto:x@y.z",
        ] {
            assert_eq!(
                safe_return_to(Some(evil.to_string())),
                "/home",
                "{evil} must be neutralized to /home"
            );
        }
    }

    #[test]
    fn safe_return_to_backslash_bypass_is_not_sanitized_security_finding() {
        // Regression (open redirect): a backslash right after the leading slash
        // is folded to `/` by browsers, so `/\evil.com` would resolve to
        // https://evil.com. The guard now rejects `/\...` and falls back to the
        // safe default.
        assert_eq!(safe_return_to(Some("/\\evil.com".to_string())), "/home");
        assert_eq!(safe_return_to(Some("/\\/evil.com".to_string())), "/home");
    }

    // ==================================================================
    // Constant-time passphrase / secret comparison  (pure)
    // ==================================================================

    #[test]
    fn constant_time_eq_matches_only_identical_strings() {
        assert!(constant_time_eq("", ""));
        assert!(constant_time_eq("a", "a"));
        assert!(constant_time_eq("correct horse battery staple", "correct horse battery staple"));
        assert!(constant_time_eq("café ☕", "café ☕"));

        assert!(!constant_time_eq("secret", "Secret")); // case-sensitive
        assert!(!constant_time_eq("secret", "secreT")); // last byte differs
        assert!(!constant_time_eq("", "x"));
        assert!(!constant_time_eq("x", ""));
        assert!(!constant_time_eq("café ☕", "café ☙"));
    }

    #[test]
    fn constant_time_eq_handles_length_mismatch_and_oversize_without_panic() {
        // Length mismatch is folded into the diff from the start (no early
        // return that would panic or leak on out-of-range indices).
        assert!(!constant_time_eq("secret", "secretlonger"));
        assert!(!constant_time_eq("secretlonger", "secret"));

        // A pathologically oversized submission must be rejected, not panic.
        let huge = "A".repeat(100_000);
        assert!(!constant_time_eq(&huge, "secret"));
        assert!(constant_time_eq(&huge, &huge));
    }

    #[test]
    fn constant_time_eq_detects_a_single_byte_difference_at_every_position() {
        // The classic timing-attack shape: a mismatch must be reported no matter
        // WHERE it occurs, and the loop must scan the whole length (no early
        // return on first-differing byte).
        let base = "abcdefghij";
        for index in 0..base.len() {
            let mut bytes = base.as_bytes().to_vec();
            bytes[index] ^= 0x01; // flip one bit at position `index`
            let mutated = String::from_utf8(bytes).unwrap();
            assert!(
                !constant_time_eq(base, &mutated),
                "difference at index {index} must be detected"
            );
        }
    }

    // ==================================================================
    // TOTP: known-answer vectors, decoding, window bounds, replay  (mixed)
    // ==================================================================

    #[test]
    fn totp_code_matches_rfc6238_sha1_vectors() {
        // RFC 6238 Appendix B (SHA1). counter = floor(T / 30); the expected
        // string is the last 6 digits of the RFC's published 8-digit value.
        let secret = b"12345678901234567890";
        let cases = [
            (59u64 / 30, "287082"),        // 94287082
            (1111111109 / 30, "081804"),   // 07081804
            (1111111111 / 30, "050471"),   // 14050471
            (1234567890 / 30, "005924"),   // 89005924
            (2000000000 / 30, "279037"),   // 69279037
            (20000000000 / 30, "353130"),  // 65353130
        ];
        for (counter, expected) in cases {
            assert_eq!(
                totp_code(secret, counter).as_deref(),
                Some(expected),
                "counter {counter}"
            );
        }
    }

    #[test]
    fn decode_totp_secret_roundtrips_and_tolerates_formatting() {
        assert_eq!(
            decode_totp_secret(RFC6238_SECRET_B32).unwrap(),
            b"12345678901234567890".to_vec()
        );
        // Lowercase, spaces and dashes are normalized away before decoding.
        assert_eq!(
            decode_totp_secret("gezd gnbv gy3t qojq gezd-gnbv-gy3t-qojq").unwrap(),
            b"12345678901234567890".to_vec()
        );
    }

    #[test]
    fn normalize_totp_secret_strips_formatting_and_uppercases() {
        assert_eq!(normalize_totp_secret("gezd-gnbv gy3t qojq"), "GEZDGNBVGY3TQOJQ");
    }

    #[test]
    fn decode_totp_secret_rejects_invalid_base32_alphabet() {
        // '0', '1', '8', '9' are outside the RFC 4648 base32 alphabet.
        assert!(decode_totp_secret("10101010").is_err());
        assert!(decode_totp_secret("8888").is_err());
    }

    #[test]
    fn valid_totp_code_rejects_malformed_input() {
        // These reject before any secret/time work, deterministically.
        assert!(!valid_totp_code(None, RFC6238_SECRET_B32));
        assert!(!valid_totp_code(Some(""), RFC6238_SECRET_B32));
        assert!(!valid_totp_code(Some("12345"), RFC6238_SECRET_B32)); // 5 digits
        assert!(!valid_totp_code(Some("1234567"), RFC6238_SECRET_B32)); // 7 digits
        assert!(!valid_totp_code(Some("12ab56"), RFC6238_SECRET_B32)); // non-digit
        assert!(!valid_totp_code(Some("abcdef"), RFC6238_SECRET_B32)); // non-digit
    }

    #[test]
    fn valid_totp_code_rejects_unparseable_secret() {
        // A valid-shaped code with an undecodable secret must fail closed.
        assert!(!valid_totp_code(Some("287082"), "10101010"));
    }

    #[test]
    fn valid_totp_code_accepts_current_code_and_bounds_the_window() {
        // Use the maximum permitted window (2). Even so, a code that is not
        // within the accept set must be rejected -- proving the skew is bounded
        // and there is no "any recent code" acceptance.
        let _g = EnvGuard::new(&[("DD_AUTH_TOTP_WINDOW_STEPS", Some("2"))]);
        let secret = decode_totp_secret(RFC6238_SECRET_B32).unwrap();
        let counter = current_totp_counter();

        let current = totp_code(&secret, counter).unwrap();
        assert!(
            valid_totp_code(Some(&current), RFC6238_SECRET_B32),
            "current TOTP code must be accepted"
        );

        // Widen the "could-be-accepted" set by one extra step on each side to
        // tolerate a clock tick during the call, then pick a syntactically valid
        // code guaranteed to be OUTSIDE it.
        let mut in_window: HashSet<String> = HashSet::new();
        for offset in -3i64..=3 {
            if let Some(candidate_counter) = counter.checked_add_signed(offset) {
                if let Some(code) = totp_code(&secret, candidate_counter) {
                    in_window.insert(code);
                }
            }
        }
        let out_of_window = (0..2000u32)
            .map(|n| format!("{n:06}"))
            .find(|code| !in_window.contains(code))
            .expect("an out-of-window 6-digit code must exist");
        assert!(
            !valid_totp_code(Some(&out_of_window), RFC6238_SECRET_B32),
            "a code outside the bounded window must be rejected"
        );
    }

    #[test]
    fn totp_window_env_is_clamped_to_two_steps() {
        // A caller cannot widen the skew arbitrarily: values >2 fall back to the
        // default (1), so an unbounded-skew misconfig can't accept stale codes.
        // We assert the clamp indirectly: with a huge requested window, a code
        // 5 steps in the past is still rejected.
        let _g = EnvGuard::new(&[("DD_AUTH_TOTP_WINDOW_STEPS", Some("100000"))]);
        let secret = decode_totp_secret(RFC6238_SECRET_B32).unwrap();
        let counter = current_totp_counter();

        // Build the widest set the *clamped* window (<=2), plus a tick of slack,
        // could ever accept, then assert a 5-steps-old code is outside it AND is
        // rejected.
        let mut in_window: HashSet<String> = HashSet::new();
        for offset in -3i64..=3 {
            if let Some(c) = counter.checked_add_signed(offset) {
                if let Some(code) = totp_code(&secret, c) {
                    in_window.insert(code);
                }
            }
        }
        if let Some(old_counter) = counter.checked_add_signed(-5) {
            if let Some(old_code) = totp_code(&secret, old_counter) {
                if !in_window.contains(&old_code) {
                    assert!(
                        !valid_totp_code(Some(&old_code), RFC6238_SECRET_B32),
                        "5-steps-old code must be rejected despite a huge requested window"
                    );
                }
            }
        }
    }

    #[test]
    fn totp_code_can_be_replayed_within_window_security_note() {
        // SECURITY NOTE: there is no one-time-use / last-counter tracking, so a
        // captured code validates repeatedly for its whole ~90s lifetime. Basic
        // TOTP limitation; documented here so the gap is explicit.
        let _g = EnvGuard::new(&[("DD_AUTH_TOTP_WINDOW_STEPS", Some("1"))]);
        let secret = decode_totp_secret(RFC6238_SECRET_B32).unwrap();
        let code = totp_code(&secret, current_totp_counter()).unwrap();
        assert!(valid_totp_code(Some(&code), RFC6238_SECRET_B32));
        assert!(
            valid_totp_code(Some(&code), RFC6238_SECRET_B32),
            "same code accepted again: no replay protection"
        );
    }

    // ==================================================================
    // Config from env: TOTP requirement, cookie TTL  (env)
    // ==================================================================

    #[test]
    fn totp_required_reflects_secret_presence_and_trims() {
        {
            let _g = EnvGuard::new(&[("DD_AUTH_TOTP_SECRET_BASE32", None)]);
            assert!(!totp_required());
            assert!(totp_secret_base32().is_none());
        }
        {
            let _g = EnvGuard::new(&[("DD_AUTH_TOTP_SECRET_BASE32", Some("   "))]);
            assert!(!totp_required(), "whitespace-only secret is treated as unset");
        }
        {
            let _g = EnvGuard::new(&[("DD_AUTH_TOTP_SECRET_BASE32", Some("  GEZDGNBVGY3TQOJQ  "))]);
            assert!(totp_required());
            assert_eq!(
                totp_secret_base32().as_deref(),
                Some("GEZDGNBVGY3TQOJQ"),
                "secret is trimmed"
            );
        }
    }

    #[test]
    fn cookie_max_age_defaults_to_three_days_and_caps_overrides() {
        const THREE_DAYS: u64 = 3 * 24 * 60 * 60;
        {
            let _g = EnvGuard::new(&[("DD_AUTH_COOKIE_MAX_AGE_SECONDS", None)]);
            assert_eq!(cookie_max_age_seconds(), THREE_DAYS);
        }
        {
            let _g = EnvGuard::new(&[("DD_AUTH_COOKIE_MAX_AGE_SECONDS", Some("3600"))]);
            assert_eq!(cookie_max_age_seconds(), 3600);
        }
        for rejected in ["999999999", "0", "garbage", "-1"] {
            let _g = EnvGuard::new(&[("DD_AUTH_COOKIE_MAX_AGE_SECONDS", Some(rejected))]);
            assert_eq!(
                cookie_max_age_seconds(),
                THREE_DAYS,
                "invalid/oversized {rejected} must fall back to the 3-day cap"
            );
        }
    }

    // ==================================================================
    // Passphrase + TOTP form validation  (env)
    // ==================================================================

    #[test]
    fn auth_form_valid_with_correct_pin_and_no_totp() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("correct horse battery staple")),
            ("DD_AUTH_TOTP_SECRET_BASE32", None),
        ]);
        assert!(auth_form_is_valid(&pin_form("correct horse battery staple", None)));
        // Both sides are trimmed.
        assert!(auth_form_is_valid(&pin_form("  correct horse battery staple  ", None)));
        assert!(!auth_form_is_valid(&pin_form("wrong", None)));
        assert!(!auth_form_is_valid(&pin_form("", None)));
        assert!(!auth_form_is_valid(&pin_form("correct horse battery stapl", None)));
    }

    #[test]
    fn auth_form_requires_both_factors_when_totp_configured() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("hunter2")),
            ("DD_AUTH_TOTP_SECRET_BASE32", Some(RFC6238_SECRET_B32)),
            ("DD_AUTH_TOTP_WINDOW_STEPS", Some("1")),
        ]);
        let secret = decode_totp_secret(RFC6238_SECRET_B32).unwrap();
        let good = totp_code(&secret, current_totp_counter()).unwrap();

        assert!(auth_form_is_valid(&pin_form("hunter2", Some(&good)))); // both correct
        assert!(!auth_form_is_valid(&pin_form("hunter2", None))); // totp omitted
        assert!(!auth_form_is_valid(&pin_form("hunter2", Some("12345")))); // malformed totp
        assert!(!auth_form_is_valid(&pin_form("wrong", Some(&good)))); // wrong pin, right totp
        assert!(!auth_form_is_valid(&pin_form("wrong", None)));
    }

    // ==================================================================
    // Cookie verification: caller_is_authenticated  (env)
    // ==================================================================

    #[test]
    fn caller_is_authenticated_accepts_matching_cookie_among_many() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("s3cr3t-gateway-value")),
        ]);
        assert!(caller_is_authenticated(&cookie_headers(
            "theme=dark; dd_auth=s3cr3t-gateway-value; foo=bar"
        )));
    }

    #[test]
    fn caller_is_authenticated_rejects_tampered_truncated_or_forged_cookies() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("s3cr3t-gateway-value")),
        ]);
        // one byte flipped
        assert!(!caller_is_authenticated(&cookie_headers("dd_auth=s3cr3t-gateway-valuE")));
        // truncated (prefix of the real value)
        assert!(!caller_is_authenticated(&cookie_headers("dd_auth=s3cr3t-gateway-valu")));
        // extended (real value plus extra)
        assert!(!caller_is_authenticated(&cookie_headers("dd_auth=s3cr3t-gateway-value-extra")));
        // empty value
        assert!(!caller_is_authenticated(&cookie_headers("dd_auth=")));
        // wrong value
        assert!(!caller_is_authenticated(&cookie_headers("dd_auth=totally-wrong")));
        // no cookie header at all
        assert!(!caller_is_authenticated(&HeaderMap::new()));
        // right value under the wrong cookie name
        assert!(!caller_is_authenticated(&cookie_headers("other=s3cr3t-gateway-value")));
        // cookie name is case-sensitive
        assert!(!caller_is_authenticated(&cookie_headers("DD_AUTH=s3cr3t-gateway-value")));
    }

    // ==================================================================
    // HTML escaping of reflected content  (pure)
    // ==================================================================

    #[test]
    fn escape_html_neutralizes_html_metacharacters() {
        assert_eq!(
            escape_html("<script>alert('x')</script>"),
            "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"
        );
        assert_eq!(escape_html(r#""><img src=x>"#), "&quot;&gt;&lt;img src=x&gt;");
        assert_eq!(escape_html("a&b"), "a&amp;b"); // ampersand escaped, not double-escaped
    }

    #[test]
    fn login_page_escapes_reflected_return_path() {
        // Defense in depth: even a crafted return path that reached the render
        // layer must be HTML-escaped so it cannot break out of the value=""
        // attribute or inject markup (reflected XSS).
        let page = login_page("/a\"><script>alert(1)</script>", None, false, false).0;
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.contains("&quot;"));
    }

    // ==================================================================
    // Misc truthy-flag parsing  (pure)
    // ==================================================================

    #[test]
    fn is_truthy_flag_recognizes_affirmative_values_only() {
        for yes in ["1", "true", "TRUE", " yes ", "on", "On"] {
            assert!(is_truthy_flag(Some(yes)), "{yes} should be truthy");
        }
        for no in ["0", "false", "no", "", "2", "enable", "maybe"] {
            assert!(!is_truthy_flag(Some(no)), "{no} should not be truthy");
        }
        assert!(!is_truthy_flag(None));
    }

    // ==================================================================
    // Endpoint-level behavior via the real handlers  (async, env)
    // ==================================================================

    #[tokio::test]
    async fn add_security_headers_sets_defense_in_depth_headers() {
        let response = add_security_headers(Response::new(axum::body::Body::empty())).await;
        let headers = response.headers();
        assert_eq!(
            headers
                .get("strict-transport-security")
                .unwrap()
                .to_str()
                .unwrap(),
            "max-age=31536000; includeSubDomains"
        );
        assert!(headers
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'"));
        assert_eq!(headers.get("x-content-type-options").unwrap().to_str().unwrap(), "nosniff");
        assert_eq!(headers.get("x-frame-options").unwrap().to_str().unwrap(), "DENY");
        assert_eq!(headers.get("referrer-policy").unwrap().to_str().unwrap(), "no-referrer");
        assert!(headers.get("permissions-policy").is_some());
    }

    #[tokio::test]
    async fn healthz_reports_service_ok() {
        let response = healthz().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let text = body_text(response).await;
        assert!(text.contains("\"ok\":true"));
        assert!(text.contains("dd-remote-auth"));
    }

    #[tokio::test]
    async fn metrics_exposes_prometheus_counters() {
        let response = metrics().await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("text/plain"));
        let text = body_text(response).await;
        for name in [
            "dd_remote_auth_http_requests_total",
            "dd_remote_auth_successes_total",
            "dd_remote_auth_failures_total",
            "dd_remote_auth_rate_limited_total",
        ] {
            assert!(text.contains(name), "metrics missing {name}");
        }
    }

    #[tokio::test]
    async fn auth_status_json_shape_matches_camelcase_contract() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("gateway-cookie-value")),
            ("DD_AUTH_TOTP_SECRET_BASE32", Some(RFC6238_SECRET_B32)),
        ]);

        let unauth = auth_status(HeaderMap::new()).await;
        let value = serde_json::to_value(&unauth.0).unwrap();
        assert_eq!(value["authenticated"], serde_json::json!(false));
        assert_eq!(value["totpRequired"], serde_json::json!(true));
        assert_eq!(value["cookieName"], serde_json::json!("dd_auth"));
        // Exactly the three camelCase keys the gateway/JS depends on.
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 3);
        assert!(object.contains_key("authenticated"));
        assert!(object.contains_key("totpRequired"));
        assert!(object.contains_key("cookieName"));

        // authenticated flips true once a valid cookie is present.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("dd_auth=gateway-cookie-value"),
        );
        assert!(auth_status(headers).await.0.authenticated);
    }

    #[tokio::test]
    async fn auth_submit_sets_hardened_cookie_on_success() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("operator-pass")),
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("gateway-cookie-value")),
            ("DD_AUTH_TOTP_SECRET_BASE32", None),
            ("DD_AUTH_COOKIE_MAX_AGE_SECONDS", None),
        ]);
        let limiter = AuthAttemptLimiter::new(5, Duration::from_secs(60));
        let response = auth_submit(
            Extension(limiter),
            ConnectInfo(test_socket()),
            HeaderMap::new(),
            Form(submit_form("operator-pass", "/home", None)),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(set_cookie.contains("dd_auth=gateway-cookie-value"));
        assert!(set_cookie.contains("HttpOnly"), "cookie must be HttpOnly");
        assert!(set_cookie.contains("Secure"), "cookie must be Secure");
        assert!(set_cookie.contains("SameSite=Lax"), "cookie must set SameSite");
        assert!(set_cookie.contains("Path=/"));
        assert!(set_cookie.contains("Max-Age="));
    }

    #[tokio::test]
    async fn auth_submit_immediate_flag_redirects_with_cookie() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("operator-pass")),
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("gateway-cookie-value")),
            ("DD_AUTH_TOTP_SECRET_BASE32", None),
        ]);
        let limiter = AuthAttemptLimiter::new(5, Duration::from_secs(60));
        let response = auth_submit(
            Extension(limiter),
            ConnectInfo(test_socket()),
            HeaderMap::new(),
            Form(submit_form("operator-pass", "/dashboard", Some("1"))),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap().to_str().unwrap(),
            "/dashboard"
        );
        assert!(response.headers().get(header::SET_COOKIE).is_some());
    }

    #[tokio::test]
    async fn auth_submit_rejects_wrong_pin_without_setting_a_cookie() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("operator-pass")),
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("gateway-cookie-value")),
            ("DD_AUTH_TOTP_SECRET_BASE32", None),
        ]);
        let limiter = AuthAttemptLimiter::new(5, Duration::from_secs(60));
        let response = auth_submit(
            Extension(limiter),
            ConnectInfo(test_socket()),
            HeaderMap::new(),
            Form(submit_form("WRONG", "/home", None)),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response.headers().get(header::SET_COOKIE).is_none(),
            "no auth cookie may be issued on a failed submission"
        );
        let text = body_text(response).await;
        assert!(text.contains("Incorrect operator passphrase"));
    }

    #[tokio::test]
    async fn auth_submit_open_redirect_backslash_reaches_location_security_finding() {
        // Regression (open redirect, end-to-end): a valid login with
        // return_to = `/\evil.com` must NOT redirect off-site — the guard folds it
        // to the safe default, so `Location` is `/home`, driven through the real
        // handler (not just the helper).
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("operator-pass")),
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("gateway-cookie-value")),
            ("DD_AUTH_TOTP_SECRET_BASE32", None),
        ]);
        let limiter = AuthAttemptLimiter::new(5, Duration::from_secs(60));
        let response = auth_submit(
            Extension(limiter),
            ConnectInfo(test_socket()),
            HeaderMap::new(),
            Form(submit_form("operator-pass", "/\\evil.com", Some("1"))),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap().to_str().unwrap(),
            "/home"
        );
    }

    #[tokio::test]
    async fn auth_submit_locks_out_after_repeated_failures() {
        let _g = EnvGuard::new(&[
            ("DD_AUTH_PIN", Some("operator-pass")),
            ("DD_AUTH_COOKIE_NAME", Some("dd_auth")),
            ("DD_AUTH_COOKIE_VALUE", Some("gateway-cookie-value")),
            ("DD_AUTH_TOTP_SECRET_BASE32", None),
        ]);
        // limit = 2: first wrong attempt -> 401, second -> lockout (429).
        let limiter = AuthAttemptLimiter::new(2, Duration::from_secs(300));
        let call = |limiter: AuthAttemptLimiter| async move {
            auth_submit(
                Extension(limiter),
                ConnectInfo(test_socket()),
                HeaderMap::new(),
                Form(submit_form("WRONG", "/home", None)),
            )
            .await
        };

        let first = call(limiter.clone()).await;
        assert_eq!(first.status(), StatusCode::UNAUTHORIZED);

        let second = call(limiter.clone()).await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(second.headers().get(header::RETRY_AFTER).is_some());

        // Once locked, further attempts are rejected before validation.
        let third = call(limiter.clone()).await;
        assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
