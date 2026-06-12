// dd-email-sms-contact-rs — sends email (SendGrid primary; SES-ready), SMS (Twilio), and push
// notifications (Web Push/VAPID, Firebase Cloud Messaging, Expo, Apple APNs) for the remote
// runtime, with per-process rate limiting and a shared-secret auth gate. Reachable two ways:
//
//   HTTP:  GET /healthz, GET /readyz, POST /send/email, POST /send/sms, POST /send/push
//          (the /send/* routes are x-server-auth gated)
//   NATS:  queue-subscribes (group dd-email-sms-contact) to the contact send lanes defined in
//          remote/libs/nats/subject-defs, handles each request once across replicas, and publishes
//          a per-send result summary. Subjects come from the generated dd-nats-subject-defs crate.
//
// Env: NATS_URL (enables the NATS consumer), SERVER_AUTH_SECRET, HOST, PORT (default 8120),
//      EMAIL_RATE_PER_MIN (60), SMS_RATE_PER_MIN (30), PUSH_RATE_PER_MIN (60).
//   email:   SENDGRID_API_KEY (needs mail.send scope), EMAIL_FROM (verified sender).
//   sms:     TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, TWILIO_FROM_NUMBER.
//   push:    VAPID_PRIVATE_KEY (EC PEM) + VAPID_SUBJECT (mailto:…) for Web Push; FCM_SERVICE_ACCOUNT_JSON
//            (+ optional FCM_PROJECT_ID override) for Firebase; APNS_KEY_P8 (EC .p8 PEM) + APNS_KEY_ID
//            + APNS_TEAM_ID + APNS_TOPIC (+ APNS_USE_SANDBOX) for Apple; EXPO_ACCESS_TOKEN (optional)
//            for Expo. Each transport is enabled only when its credentials are present (Expo always on).

use std::env;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    CONTACT_EMAIL_SEND_QUEUE_GROUP, CONTACT_EMAIL_SEND_SUBJECT, CONTACT_PUSH_SEND_SUBJECT,
    CONTACT_SEND_RESULTS_SUBJECT, CONTACT_SMS_SEND_SUBJECT,
};
use futures::StreamExt;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

#[derive(Clone)]
struct AppState {
    http: reqwest::Client,
    auth_secret: Option<String>,
    // Optional shared secret required in the `auth` field of NATS send-requests. When unset the lane
    // is open (trusted bus); when set, handlers reject messages whose `auth` does not match.
    nats_secret: Option<String>,
    sendgrid_key: Option<String>,
    email_from: String,
    twilio_sid: Option<String>,
    twilio_token: Option<String>,
    twilio_from: Option<String>,
    // push transports — each Some only when configured (Expo is always available).
    webpush: Option<WebPushConfig>,
    fcm: Option<FcmConfig>,
    apns: Option<ApnsConfig>,
    expo: ExpoConfig,
    // SSRF guard for the user-supplied webpush endpoint URL.
    webpush_policy: Arc<WebPushHostPolicy>,
    email_bucket: Arc<Mutex<TokenBucket>>,
    sms_bucket: Arc<Mutex<TokenBucket>>,
    push_bucket: Arc<Mutex<TokenBucket>>,
}

// Controls which hosts the webpush transport will POST a subscription to. The endpoint is
// attacker-influenced (it arrives in the request / on the NATS lane, which has no auth gate), so an
// unrestricted client would be an SSRF primitive into the cluster. Default: an allowlist of the
// known browser push services. Operators can extend it or open it up via WEBPUSH_ALLOWED_HOSTS.
enum WebPushHostPolicy {
    // Host must end with one of these suffixes (case-insensitive). Always also requires https + a
    // non-private destination.
    Allowlist(Vec<String>),
    // WEBPUSH_ALLOWED_HOSTS=* — any https host that does not resolve to a private/loopback literal.
    AnyPublic,
}

// Suffixes covering Chrome/Edge (FCM), Firefox (Mozilla autopush), Windows (WNS), and Safari/Apple
// web push. Subdomains are matched, so e.g. updates.push.services.mozilla.com is accepted.
const DEFAULT_WEBPUSH_HOSTS: &[&str] = &[
    "fcm.googleapis.com",
    "push.services.mozilla.com",
    "notify.windows.com",
    "push.apple.com",
];

#[derive(Clone)]
struct WebPushConfig {
    vapid_pem: String,
    subject: String,
    ttl: u32,
}

#[derive(Clone)]
struct FcmConfig {
    project_id: String,
    client_email: String,
    private_key: String,
    token_uri: String,
    token: Arc<Mutex<Option<CachedToken>>>,
}

#[derive(Clone)]
struct ApnsConfig {
    key_p8: String,
    key_id: String,
    team_id: String,
    topic: String,
    host: &'static str,
    token: Arc<Mutex<Option<CachedToken>>>,
}

#[derive(Clone)]
struct ExpoConfig {
    access_token: Option<String>,
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

struct TokenBucket {
    capacity: f64,
    tokens: f64,
    per_sec: f64,
    last: Instant,
}
impl TokenBucket {
    fn new(per_min: f64) -> Self {
        let cap = per_min.max(1.0);
        Self { capacity: cap, tokens: cap, per_sec: per_min / 60.0, last: Instant::now() }
    }
    fn try_take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens =
            (self.tokens + now.duration_since(self.last).as_secs_f64() * self.per_sec).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct Outcome {
    ok: bool,
    transport: &'static str,
    upstream_status: Option<u16>,
    error: Option<String>,
    rate_limited: bool,
}

// `auth` carries the optional NATS shared secret (see AppState::nats_secret); it is ignored on the
// HTTP path, which is gated by the x-server-auth header instead.
#[derive(Deserialize)]
struct EmailReq {
    to: String,
    subject: String,
    html: String,
    text: Option<String>,
    from: Option<String>,
    auth: Option<String>,
}
#[derive(Deserialize)]
struct SmsReq {
    to: String,
    body: String,
    auth: Option<String>,
}
#[derive(Deserialize)]
struct PushReq {
    transport: String, // webpush | fcm | expo | apns
    title: Option<String>,
    body: Option<String>,
    data: Option<Value>,
    token: Option<String>,                  // fcm / expo / apns device token
    subscription: Option<PushSubscription>, // webpush
    auth: Option<String>,
}
#[derive(Deserialize)]
struct PushSubscription {
    endpoint: String,
    keys: PushSubKeys,
}
#[derive(Deserialize)]
struct PushSubKeys {
    p256dh: String,
    auth: String,
}

// Service-account JSON (Google) for FCM HTTP v1 OAuth.
#[derive(Deserialize)]
struct FcmServiceAccount {
    #[serde(default)]
    client_email: String,
    #[serde(default)]
    private_key: String,
    #[serde(default = "default_fcm_token_uri")]
    token_uri: String,
    project_id: Option<String>,
}
fn default_fcm_token_uri() -> String {
    "https://oauth2.googleapis.com/token".to_string()
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-email-sms-contact-rs");

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8120u16);
    let email_per_min = env::var("EMAIL_RATE_PER_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
    let sms_per_min = env::var("SMS_RATE_PER_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);
    let push_per_min = env::var("PUSH_RATE_PER_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);

    let state = AppState {
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
        auth_secret: non_empty(env::var("SERVER_AUTH_SECRET").ok()),
        nats_secret: non_empty(env::var("NATS_SHARED_SECRET").ok()),
        sendgrid_key: non_empty(env::var("SENDGRID_API_KEY").ok()).filter(|k| !k.contains("REPLACE")),
        email_from: env::var("EMAIL_FROM").unwrap_or_else(|_| "outreach@dancingdragons.cc".to_string()),
        twilio_sid: non_empty(env::var("TWILIO_ACCOUNT_SID").ok()),
        twilio_token: non_empty(env::var("TWILIO_AUTH_TOKEN").ok()),
        twilio_from: non_empty(env::var("TWILIO_FROM_NUMBER").ok()),
        webpush: build_webpush_config(),
        fcm: build_fcm_config(),
        apns: build_apns_config(),
        expo: ExpoConfig { access_token: non_empty(env::var("EXPO_ACCESS_TOKEN").ok()) },
        webpush_policy: Arc::new(build_webpush_policy()),
        email_bucket: Arc::new(Mutex::new(TokenBucket::new(email_per_min))),
        sms_bucket: Arc::new(Mutex::new(TokenBucket::new(sms_per_min))),
        push_bucket: Arc::new(Mutex::new(TokenBucket::new(push_per_min))),
    };

    // Optional NATS consumer: handles send-requests published onto the contact lanes.
    if let Some(url) = non_empty(env::var("NATS_URL").ok()) {
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = run_nats_consumer(st, url).await {
                tracing::error!("nats consumer stopped: {e}");
            }
        });
    } else {
        tracing::info!("NATS_URL unset — NATS consumer disabled (HTTP send still available)");
    }

    let app = Router::new()
        .route("/healthz", get(|| async { Json(json!({"ok": true, "service": "dd-email-sms-contact-rs"})) }))
        .route("/readyz", get(readyz))
        .route("/send/email", post(http_send_email))
        .route("/send/sms", post(http_send_sms))
        // Cap the push body well below the email route's default: the payload validation ceiling is
        // 8 KiB, so 64 KiB bounds buffering with generous headroom and rejects oversized bodies before
        // we deserialize them.
        .route("/send/push", post(http_send_push).layer(DefaultBodyLimit::max(64 * 1024)))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse().expect("bind addr");
    tracing::info!("dd-email-sms-contact-rs listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer())).with_graceful_shutdown(shutdown_signal()).await.expect("server");
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn build_webpush_config() -> Option<WebPushConfig> {
    let vapid_pem = non_empty(env::var("VAPID_PRIVATE_KEY").ok())?;
    let subject = non_empty(env::var("VAPID_SUBJECT").ok())
        .unwrap_or_else(|| "mailto:outreach@dancingdragons.cc".to_string());
    // TTL the push service holds an undelivered message; default 12h, matching the VAPID claim window.
    let ttl = env::var("WEBPUSH_TTL_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(43_200u32);
    Some(WebPushConfig { vapid_pem, subject, ttl })
}

fn build_fcm_config() -> Option<FcmConfig> {
    let raw = non_empty(env::var("FCM_SERVICE_ACCOUNT_JSON").ok())?;
    let sa: FcmServiceAccount = match serde_json::from_str(&raw) {
        Ok(sa) => sa,
        Err(e) => {
            tracing::error!("FCM_SERVICE_ACCOUNT_JSON parse failed: {e} — FCM disabled");
            return None;
        }
    };
    let project_id = non_empty(env::var("FCM_PROJECT_ID").ok()).or(sa.project_id).unwrap_or_default();
    if project_id.is_empty() || sa.client_email.is_empty() || sa.private_key.is_empty() {
        tracing::error!("FCM_SERVICE_ACCOUNT_JSON missing project_id/client_email/private_key — FCM disabled");
        return None;
    }
    Some(FcmConfig {
        project_id,
        client_email: sa.client_email,
        private_key: sa.private_key,
        token_uri: sa.token_uri,
        token: Arc::new(Mutex::new(None)),
    })
}

fn build_webpush_policy() -> WebPushHostPolicy {
    match non_empty(env::var("WEBPUSH_ALLOWED_HOSTS").ok()) {
        None => WebPushHostPolicy::Allowlist(DEFAULT_WEBPUSH_HOSTS.iter().map(|h| h.to_string()).collect()),
        Some(v) if v.trim() == "*" => WebPushHostPolicy::AnyPublic,
        Some(v) => WebPushHostPolicy::Allowlist(
            v.split(',').map(|h| h.trim().to_ascii_lowercase()).filter(|h| !h.is_empty()).collect(),
        ),
    }
}

fn build_apns_config() -> Option<ApnsConfig> {
    let key_p8 = non_empty(env::var("APNS_KEY_P8").ok())?;
    let key_id = non_empty(env::var("APNS_KEY_ID").ok())?;
    let team_id = non_empty(env::var("APNS_TEAM_ID").ok())?;
    let topic = non_empty(env::var("APNS_TOPIC").ok())?;
    let sandbox = env::var("APNS_USE_SANDBOX").map(|v| v == "true" || v == "1").unwrap_or(false);
    Some(ApnsConfig {
        key_p8,
        key_id,
        team_id,
        topic,
        host: if sandbox { "api.sandbox.push.apple.com" } else { "api.push.apple.com" },
        token: Arc::new(Mutex::new(None)),
    })
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ── shared transports ──────────────────────────────────────────────────────────

async fn email_send(s: &AppState, to: &str, subject: &str, html: &str, text: Option<&str>, from: Option<&str>) -> Outcome {
    let Some(key) = s.sendgrid_key.clone() else {
        return Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some("SENDGRID_API_KEY not configured (needs a key with the mail.send scope)".into()), rate_limited: false };
    };
    if !s.email_bucket.lock().await.try_take() {
        return Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some("email rate limit exceeded".into()), rate_limited: true };
    }
    // Only the configured verified sender may be used — don't let callers (HTTP or the NATS lane)
    // pick an arbitrary `from` (open-relay / spoofing primitive). Change it via EMAIL_FROM.
    let from = match from {
        Some(f) if f == s.email_from => f.to_string(),
        Some(_) => return Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some("from not allowed (must equal configured EMAIL_FROM)".into()), rate_limited: false },
        None => s.email_from.clone(),
    };
    let mut content = vec![json!({"type": "text/html", "value": html})];
    if let Some(t) = text {
        content.insert(0, json!({"type": "text/plain", "value": t}));
    }
    let body = json!({
        "personalizations": [{"to": [{"email": to}]}],
        "from": {"email": from},
        "subject": subject,
        "content": content,
    });
    match s.http.post("https://api.sendgrid.com/v3/mail/send").bearer_auth(key).json(&body).send().await {
        Ok(r) if r.status().is_success() => Outcome { ok: true, transport: "sendgrid", upstream_status: Some(r.status().as_u16()), error: None, rate_limited: false },
        Ok(r) => {
            let code = r.status().as_u16();
            let txt = cap(r.text().await.unwrap_or_default());
            Outcome { ok: false, transport: "sendgrid", upstream_status: Some(code), error: Some(txt), rate_limited: false }
        }
        Err(e) => Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some(format!("request failed: {e}")), rate_limited: false },
    }
}

async fn sms_send(s: &AppState, to: &str, sms_body: &str) -> Outcome {
    let (Some(sid), Some(token), Some(from)) = (s.twilio_sid.clone(), s.twilio_token.clone(), s.twilio_from.clone()) else {
        return Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some("Twilio not configured".into()), rate_limited: false };
    };
    if !s.sms_bucket.lock().await.try_take() {
        return Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some("sms rate limit exceeded".into()), rate_limited: true };
    }
    let url = format!("https://api.twilio.com/2010-04-01/Accounts/{sid}/Messages.json");
    let form = [("To", to), ("From", from.as_str()), ("Body", sms_body)];
    match s.http.post(url).basic_auth(sid, Some(token)).form(&form).send().await {
        Ok(r) if r.status().is_success() => Outcome { ok: true, transport: "twilio", upstream_status: Some(r.status().as_u16()), error: None, rate_limited: false },
        Ok(r) => {
            let code = r.status().as_u16();
            let txt = cap(r.text().await.unwrap_or_default());
            Outcome { ok: false, transport: "twilio", upstream_status: Some(code), error: Some(txt), rate_limited: false }
        }
        Err(e) => Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some(format!("request failed: {e}")), rate_limited: false },
    }
}

// ── push transports ─────────────────────────────────────────────────────────────

const MAX_ERR_BYTES: usize = 1024; // upstream errors flow onto the results bus / HTTP body — keep them bounded

fn not_configured(transport: &'static str, msg: &str) -> Outcome {
    Outcome { ok: false, transport, upstream_status: None, error: Some(msg.to_string()), rate_limited: false }
}
fn transport_err(transport: &'static str, e: reqwest::Error) -> Outcome {
    Outcome { ok: false, transport, upstream_status: None, error: Some(cap(format!("request failed: {e}"))), rate_limited: false }
}
fn auth_err(transport: &'static str, e: String) -> Outcome {
    Outcome { ok: false, transport, upstream_status: None, error: Some(cap(format!("auth failed: {e}"))), rate_limited: false }
}
async fn result_from(r: reqwest::Response, transport: &'static str) -> Outcome {
    let code = r.status().as_u16();
    if (200..300).contains(&code) {
        Outcome { ok: true, transport, upstream_status: Some(code), error: None, rate_limited: false }
    } else {
        let txt = cap(r.text().await.unwrap_or_default());
        Outcome { ok: false, transport, upstream_status: Some(code), error: Some(txt), rate_limited: false }
    }
}
fn stringify_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// Truncate on a char boundary so error text echoed to callers / the bus can't be used to bloat
// messages and stays valid UTF-8.
fn cap(mut s: String) -> String {
    if s.len() > MAX_ERR_BYTES {
        let mut end = MAX_ERR_BYTES;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("…[truncated]");
    }
    s
}

// SSRF guard: validate a user-supplied webpush subscription endpoint before we POST to it. Returns
// Some(reason) when the endpoint must be rejected. Enforces https, blocks private/loopback IP
// literals, and (unless WEBPUSH_ALLOWED_HOSTS=*) requires a known push-service host suffix.
fn validate_webpush_endpoint(endpoint: &str, policy: &WebPushHostPolicy) -> Option<String> {
    let url = match reqwest::Url::parse(endpoint) {
        Ok(u) => u,
        Err(e) => return Some(format!("invalid webpush endpoint url: {e}")),
    };
    if url.scheme() != "https" {
        return Some("webpush endpoint must use https".into());
    }
    // Embedded credentials are never part of a real push endpoint and can be used to obscure the
    // true target; reject them outright.
    if !url.username().is_empty() || url.password().is_some() {
        return Some("webpush endpoint must not embed credentials".into());
    }
    // Push services always listen on 443; pinning the port stops an allowlisted host from being
    // used to reach a non-TLS/admin port on that same host.
    if let Some(port) = url.port() {
        if port != 443 {
            return Some("webpush endpoint must use port 443".into());
        }
    }
    let Some(host_raw) = url.host_str() else {
        return Some("webpush endpoint missing host".into());
    };
    let host = host_raw.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return Some("webpush endpoint host not allowed".into());
    }
    // Block IP-literal endpoints that point at private/internal ranges regardless of policy.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip_is_blocked(&ip) {
            return Some("webpush endpoint points at a non-public address".into());
        }
    }
    match policy {
        WebPushHostPolicy::AnyPublic => None,
        WebPushHostPolicy::Allowlist(allowed) => {
            let ok = allowed.iter().any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));
            if ok {
                None
            } else {
                Some("webpush endpoint host is not in the allowed push-service list".into())
            }
        }
    }
}

// Conservative private/internal address check used as an SSRF floor. Not exhaustive, but covers the
// ranges that matter for reaching cluster/link-local/loopback services.
fn ip_is_blocked(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0 // 0.0.0.0/8
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
                || v6.to_ipv4_mapped().map(|m| ip_is_blocked(&IpAddr::V4(m))).unwrap_or(false)
        }
    }
}

// Device tokens are interpolated into provider request paths (APNs) and bodies; reject anything that
// could break out of a URL path segment or carry control characters, and bound the length. The
// allowed set still covers FCM (`:-_` + alnum), Expo (`ExponentPushToken[…]`), and APNs (hex).
fn validate_push_token(token: &str) -> Option<&'static str> {
    if token.len() > 4096 {
        return Some("device token too long");
    }
    if token.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '?' | '#' | '\\' | '%')) {
        return Some("device token contains invalid characters");
    }
    None
}

// SSRF defense-in-depth for WEBPUSH_ALLOWED_HOSTS=*: resolve the (non-literal) host and reject if any
// answer is a private/internal address — this is what catches `metadata.google.internal` and other
// names that point inward. It does not close DNS rebinding (the resolver could return a different
// answer when reqwest connects); the host allowlist is the real boundary, and the open mode is opt-in.
async fn endpoint_resolves_to_public(endpoint: &str) -> Option<String> {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return None; // already shape-validated; nothing more to check here
    };
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').to_string();
    if host.parse::<IpAddr>().is_ok() {
        return None; // literal IPs are vetted synchronously
    }
    let port = url.port_or_known_default().unwrap_or(443);
    // Owned "host:port" so nothing is borrowed across the resolver await.
    match tokio::net::lookup_host(format!("{host}:{port}")).await {
        Ok(addrs) => {
            let mut any = false;
            for a in addrs {
                any = true;
                if ip_is_blocked(&a.ip()) {
                    return Some("webpush endpoint resolves to a non-public address".into());
                }
            }
            if any {
                None
            } else {
                Some("webpush endpoint host did not resolve".into())
            }
        }
        Err(e) => Some(cap(format!("webpush endpoint host resolution failed: {e}"))),
    }
}

// Dispatches a push request to the requested transport. Rate-limited once across all push channels.
async fn push_send(s: &AppState, req: &PushReq) -> Outcome {
    let transport: &'static str = match req.transport.as_str() {
        "webpush" => "webpush",
        "fcm" => "fcm",
        "expo" => "expo",
        "apns" => "apns",
        other => {
            return Outcome {
                ok: false,
                transport: "push",
                upstream_status: None,
                error: Some(format!("unknown push transport '{other}' (expected webpush|fcm|expo|apns)")),
                rate_limited: false,
            };
        }
    };
    // Reject a disabled transport before spending a rate-limit token, so callers hitting an
    // unconfigured channel can't drain the shared push bucket.
    let configured = match transport {
        "webpush" => s.webpush.is_some(),
        "fcm" => s.fcm.is_some(),
        "apns" => s.apns.is_some(),
        _ => true, // expo needs no credentials
    };
    if !configured {
        return not_configured(transport, &format!("{transport} not configured"));
    }
    if !s.push_bucket.lock().await.try_take() {
        return Outcome { ok: false, transport, upstream_status: None, error: Some("push rate limit exceeded".into()), rate_limited: true };
    }
    match transport {
        "webpush" => webpush_send(s, req).await,
        "fcm" => fcm_send(s, req).await,
        "expo" => expo_send(s, req).await,
        "apns" => apns_send(s, req).await,
        _ => unreachable!(),
    }
}

async fn webpush_send(s: &AppState, req: &PushReq) -> Outcome {
    let Some(cfg) = &s.webpush else {
        return not_configured("webpush", "Web Push (VAPID) not configured");
    };
    let Some(sub) = &req.subscription else {
        return not_configured("webpush", "webpush requires a subscription");
    };
    let info = SubscriptionInfo::new(sub.endpoint.clone(), sub.keys.p256dh.clone(), sub.keys.auth.clone());
    let payload = serde_json::to_vec(&json!({ "title": req.title, "body": req.body, "data": req.data }))
        .unwrap_or_default();

    let signature = match VapidSignatureBuilder::from_pem(cfg.vapid_pem.as_bytes(), &info) {
        Ok(mut b) => {
            b.add_claim("sub", cfg.subject.clone());
            match b.build() {
                Ok(sig) => sig,
                Err(e) => return Outcome { ok: false, transport: "webpush", upstream_status: None, error: Some(cap(format!("vapid signing failed: {e}"))), rate_limited: false },
            }
        }
        Err(e) => return Outcome { ok: false, transport: "webpush", upstream_status: None, error: Some(cap(format!("vapid key invalid: {e}"))), rate_limited: false },
    };

    let mut builder = WebPushMessageBuilder::new(&info);
    builder.set_ttl(cfg.ttl);
    builder.set_payload(ContentEncoding::Aes128Gcm, &payload);
    builder.set_vapid_signature(signature);
    let message = match builder.build() {
        Ok(m) => m,
        Err(e) => return Outcome { ok: false, transport: "webpush", upstream_status: None, error: Some(cap(format!("encrypt failed: {e}"))), rate_limited: false },
    };

    let endpoint = message.endpoint.to_string();
    let mut rb = s.http.post(&endpoint).header("TTL", message.ttl.to_string());
    if let Some(p) = message.payload {
        for (k, v) in &p.crypto_headers {
            rb = rb.header(*k, v);
        }
        rb = rb.header("Content-Encoding", p.content_encoding.to_str()).body(p.content);
    }
    match rb.send().await {
        Ok(r) => result_from(r, "webpush").await,
        Err(e) => transport_err("webpush", e),
    }
}

async fn expo_send(s: &AppState, req: &PushReq) -> Outcome {
    let token = req.token.as_deref().unwrap_or_default();
    let mut msg = Map::new();
    msg.insert("to".into(), json!(token));
    if let Some(t) = &req.title {
        msg.insert("title".into(), json!(t));
    }
    if let Some(b) = &req.body {
        msg.insert("body".into(), json!(b));
    }
    if let Some(d) = &req.data {
        msg.insert("data".into(), d.clone());
    }
    let body = json!([Value::Object(msg)]);
    let mut rb = s.http.post("https://exp.host/--/api/v2/push/send").json(&body);
    if let Some(at) = &s.expo.access_token {
        rb = rb.bearer_auth(at);
    }
    match rb.send().await {
        Ok(r) => {
            let code = r.status().as_u16();
            let txt = r.text().await.unwrap_or_default();
            if !(200..300).contains(&code) {
                return Outcome { ok: false, transport: "expo", upstream_status: Some(code), error: Some(cap(txt)), rate_limited: false };
            }
            // Expo returns HTTP 200 even for per-ticket failures: the ticket carries
            // {"status":"error", "message": …} inside the "data" array (or a top-level "errors").
            // Parse the body rather than substring-matching so we surface the real reason.
            match serde_json::from_str::<Value>(&txt) {
                Ok(v) => {
                    let ticket_err = v
                        .get("data")
                        .and_then(|d| d.as_array())
                        .and_then(|arr| arr.iter().find(|t| t.get("status").and_then(|s| s.as_str()) == Some("error")))
                        .and_then(|t| t.get("message").and_then(|m| m.as_str()))
                        .map(|m| m.to_string());
                    let top_err = v.get("errors").filter(|e| !e.is_null()).map(|e| e.to_string());
                    match ticket_err.or(top_err) {
                        Some(msg) => Outcome { ok: false, transport: "expo", upstream_status: Some(code), error: Some(cap(msg)), rate_limited: false },
                        None => Outcome { ok: true, transport: "expo", upstream_status: Some(code), error: None, rate_limited: false },
                    }
                }
                // Non-JSON 2xx — treat as success but keep the body for debugging.
                Err(_) => Outcome { ok: true, transport: "expo", upstream_status: Some(code), error: None, rate_limited: false },
            }
        }
        Err(e) => transport_err("expo", e),
    }
}

async fn fcm_send(s: &AppState, req: &PushReq) -> Outcome {
    let Some(cfg) = &s.fcm else {
        return not_configured("fcm", "FCM not configured");
    };
    let token = match fcm_access_token(s, cfg).await {
        Ok(t) => t,
        Err(e) => return auth_err("fcm", e),
    };
    let device = req.token.as_deref().unwrap_or_default();
    let mut message = Map::new();
    message.insert("token".into(), json!(device));
    let mut notif = Map::new();
    if let Some(t) = &req.title {
        notif.insert("title".into(), json!(t));
    }
    if let Some(b) = &req.body {
        notif.insert("body".into(), json!(b));
    }
    if !notif.is_empty() {
        message.insert("notification".into(), Value::Object(notif));
    }
    // FCM data values must be strings — coerce non-string JSON values.
    if let Some(Value::Object(d)) = &req.data {
        let data: Map<String, Value> = d.iter().map(|(k, v)| (k.clone(), Value::String(stringify_value(v)))).collect();
        message.insert("data".into(), Value::Object(data));
    }
    let body = json!({ "message": Value::Object(message) });
    let url = format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", cfg.project_id);
    match s.http.post(&url).bearer_auth(token).json(&body).send().await {
        Ok(r) => result_from(r, "fcm").await,
        Err(e) => transport_err("fcm", e),
    }
}

// Returns a cached FCM OAuth access token, minting + caching a fresh one (RS256 JWT bearer grant)
// when none is cached or the cached token is within 60s of expiry.
async fn fcm_access_token(s: &AppState, cfg: &FcmConfig) -> Result<String, String> {
    {
        let guard = cfg.token.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.expires_at > Instant::now() {
                return Ok(c.value.clone());
            }
        }
    }
    let now = unix_now();
    #[derive(Serialize)]
    struct OauthClaims<'a> {
        iss: &'a str,
        scope: &'a str,
        aud: &'a str,
        iat: u64,
        exp: u64,
    }
    let claims = OauthClaims {
        iss: &cfg.client_email,
        scope: "https://www.googleapis.com/auth/firebase.messaging",
        aud: &cfg.token_uri,
        iat: now,
        exp: now + 3600,
    };
    let key = EncodingKey::from_rsa_pem(cfg.private_key.as_bytes()).map_err(|e| e.to_string())?;
    let jwt = encode(&Header::new(Algorithm::RS256), &claims, &key).map_err(|e| e.to_string())?;
    let resp = s
        .http
        .post(&cfg.token_uri)
        .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", jwt.as_str())])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let code = resp.status().as_u16();
    let val: Value = resp.json().await.map_err(|e| e.to_string())?;
    let Some(access) = val.get("access_token").and_then(|v| v.as_str()) else {
        return Err(format!("token endpoint {code}: {val}"));
    };
    let expires_in = val.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    let token = access.to_string();
    let mut guard = cfg.token.lock().await;
    *guard = Some(CachedToken {
        value: token.clone(),
        expires_at: Instant::now() + Duration::from_secs(expires_in.saturating_sub(60)),
    });
    Ok(token)
}

async fn apns_send(s: &AppState, req: &PushReq) -> Outcome {
    let Some(cfg) = &s.apns else {
        return not_configured("apns", "APNs not configured");
    };
    let jwt = match apns_jwt(cfg).await {
        Ok(t) => t,
        Err(e) => return auth_err("apns", e),
    };
    let device = req.token.as_deref().unwrap_or_default();
    let mut alert = Map::new();
    if let Some(t) = &req.title {
        alert.insert("title".into(), json!(t));
    }
    if let Some(b) = &req.body {
        alert.insert("body".into(), json!(b));
    }
    let mut payload = Map::new();
    payload.insert("aps".into(), json!({ "alert": Value::Object(alert) }));
    // Custom keys ride alongside `aps` at the top level of the APNs payload.
    if let Some(Value::Object(d)) = &req.data {
        for (k, v) in d {
            if k != "aps" {
                payload.insert(k.clone(), v.clone());
            }
        }
    }
    let url = format!("https://{}/3/device/{}", cfg.host, device);
    match s
        .http
        .post(&url)
        .header("authorization", format!("bearer {jwt}"))
        .header("apns-topic", &cfg.topic)
        .header("apns-push-type", "alert")
        .header("apns-priority", "10")
        .json(&Value::Object(payload))
        .send()
        .await
    {
        Ok(r) => result_from(r, "apns").await,
        Err(e) => transport_err("apns", e),
    }
}

// Returns a cached APNs provider JWT (ES256), refreshing it roughly every 50 minutes (Apple
// rejects tokens older than 1h and rate-limits frequent regeneration).
async fn apns_jwt(cfg: &ApnsConfig) -> Result<String, String> {
    {
        let guard = cfg.token.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.expires_at > Instant::now() {
                return Ok(c.value.clone());
            }
        }
    }
    #[derive(Serialize)]
    struct ApnsClaims<'a> {
        iss: &'a str,
        iat: u64,
    }
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(cfg.key_id.clone());
    let key = EncodingKey::from_ec_pem(cfg.key_p8.as_bytes()).map_err(|e| e.to_string())?;
    let jwt = encode(&header, &ApnsClaims { iss: &cfg.team_id, iat: unix_now() }, &key).map_err(|e| e.to_string())?;
    let mut guard = cfg.token.lock().await;
    *guard = Some(CachedToken { value: jwt.clone(), expires_at: Instant::now() + Duration::from_secs(50 * 60) });
    Ok(jwt)
}

fn outcome_json(channel: &str, to: &str, o: &Outcome) -> Value {
    json!({"ok": o.ok, "channel": channel, "to": to, "transport": o.transport, "upstreamStatus": o.upstream_status, "error": o.error, "rateLimited": o.rate_limited})
}

// A non-sensitive label for the result summary. Device tokens and webpush endpoints embed the
// delivery capability (sending to them = pushing to that device), so we publish only a short,
// redacted hint onto the results bus rather than the full value.
fn push_target_label(req: &PushReq) -> String {
    if let Some(t) = req.token.as_deref().filter(|t| !t.is_empty()) {
        return redact_token(t);
    }
    if let Some(sub) = &req.subscription {
        return redact_endpoint(&sub.endpoint);
    }
    String::new()
}

fn redact_token(t: &str) -> String {
    let head: String = t.chars().take(6).collect();
    format!("{head}…({} chars)", t.chars().count())
}

// Keep scheme://host so operators can tell which push service was targeted; drop the path, which
// is the per-subscription secret.
fn redact_endpoint(endpoint: &str) -> String {
    match reqwest::Url::parse(endpoint) {
        Ok(u) => match u.host_str() {
            Some(h) => format!("{}://{}/…", u.scheme(), h),
            None => "webpush-subscription".to_string(),
        },
        Err(_) => "webpush-subscription".to_string(),
    }
}

// ── HTTP handlers ──────────────────────────────────────────────────────────────

async fn readyz(State(s): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "email": {"sendgrid_configured": s.sendgrid_key.is_some(), "from": s.email_from},
        "sms": {"twilio_configured": s.twilio_sid.is_some() && s.twilio_token.is_some() && s.twilio_from.is_some()},
        "push": {
            "webpush_configured": s.webpush.is_some(),
            "fcm_configured": s.fcm.is_some(),
            "expo_configured": true,
            "apns_configured": s.apns.is_some(),
        },
        "nats": {
            "consumer_enabled": env::var("NATS_URL").map(|v| !v.is_empty()).unwrap_or(false),
            "auth_required": s.nats_secret.is_some(),
        },
    }))
}

fn check_auth(s: &AppState, headers: &HeaderMap) -> bool {
    match &s.auth_secret {
        // Fail closed: with no shared secret configured we reject all /send/* calls rather than act
        // as an open in-cluster relay. (NATS send lanes rely on the trusted message bus instead.)
        None => false,
        Some(secret) => headers.get("x-server-auth").and_then(|v| v.to_str().ok()).map(|got| constant_time_eq(got.as_bytes(), secret.as_bytes())).unwrap_or(false),
    }
}

const MAX_HTML_BYTES: usize = 1024 * 1024; // 1 MiB — stays under axum's 2 MiB default body limit
const MAX_BODY_BYTES: usize = 1600; // ~10 SMS segments
const MAX_PUSH_BYTES: usize = 8 * 1024; // generous ceiling; FCM/APNs/Web Push payload caps are ~4KB

// Returns Some(error) when the request is malformed.
fn validate_email(to: &str, subject: &str, html: &str) -> Option<&'static str> {
    if to.len() < 3 || to.len() > 320 || !to.contains('@') || !to.split('@').nth(1).map(|d| d.contains('.')).unwrap_or(false) {
        return Some("invalid recipient email");
    }
    // reject control chars (CR/LF etc.) so they can't bleed into the outgoing MIME Subject header
    if subject.is_empty() || subject.len() > 1000 || subject.chars().any(|c| c.is_control()) {
        return Some("invalid subject");
    }
    if html.is_empty() || html.len() > MAX_HTML_BYTES {
        return Some("invalid html body");
    }
    None
}

fn validate_sms(to: &str, body: &str) -> Option<&'static str> {
    if to.len() < 5 || to.len() > 32 || !to.starts_with('+') || !to[1..].chars().all(|c| c.is_ascii_digit()) {
        return Some("invalid recipient phone (E.164 +<digits> required)");
    }
    if body.is_empty() || body.len() > MAX_BODY_BYTES {
        return Some("invalid sms body");
    }
    None
}

async fn validate_push(req: &PushReq, webpush_policy: &WebPushHostPolicy) -> Option<String> {
    match req.transport.as_str() {
        "webpush" => {
            let Some(sub) = &req.subscription else {
                return Some("webpush requires 'subscription' { endpoint, keys: { p256dh, auth } }".into());
            };
            if sub.endpoint.is_empty() || sub.keys.p256dh.is_empty() || sub.keys.auth.is_empty() {
                return Some("webpush subscription missing endpoint/p256dh/auth".into());
            }
            // SSRF guard — reject before any network call or rate-limit token is spent.
            if let Some(e) = validate_webpush_endpoint(&sub.endpoint, webpush_policy) {
                return Some(e);
            }
            // In the open-host mode there is no allowlist to lean on, so additionally vet the
            // resolved address (closes hostnames that point at internal/metadata IPs).
            if matches!(webpush_policy, WebPushHostPolicy::AnyPublic) {
                if let Some(e) = endpoint_resolves_to_public(&sub.endpoint).await {
                    return Some(e);
                }
            }
        }
        "fcm" | "expo" | "apns" => {
            let token = req.token.as_deref().unwrap_or_default();
            if token.is_empty() {
                return Some(format!("{} requires a device 'token'", req.transport));
            }
            if let Some(e) = validate_push_token(token) {
                return Some(e.to_string());
            }
        }
        other => return Some(format!("unknown push transport '{other}' (expected webpush|fcm|expo|apns)")),
    }
    let title_len = req.title.as_deref().unwrap_or_default().len();
    let body_len = req.body.as_deref().unwrap_or_default().len();
    if title_len == 0 && body_len == 0 {
        return Some("push requires at least a title or body".into());
    }
    let data_len = req.data.as_ref().map(|d| d.to_string().len()).unwrap_or(0);
    if title_len + body_len + data_len > MAX_PUSH_BYTES {
        return Some("push payload too large".into());
    }
    None
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn http_send_email(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<EmailReq>) -> (StatusCode, Json<Value>) {
    if !check_auth(&s, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false, "error": "unauthorized"})));
    }
    if let Some(e) = validate_email(&req.to, &req.subject, &req.html) {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e})));
    }
    let o = email_send(&s, &req.to, &req.subject, &req.html, req.text.as_deref(), req.from.as_deref()).await;
    (status_for(&o), Json(outcome_json("email", &req.to, &o)))
}

async fn http_send_sms(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<SmsReq>) -> (StatusCode, Json<Value>) {
    if !check_auth(&s, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false, "error": "unauthorized"})));
    }
    if let Some(e) = validate_sms(&req.to, &req.body) {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e})));
    }
    let o = sms_send(&s, &req.to, &req.body).await;
    (status_for(&o), Json(outcome_json("sms", &req.to, &o)))
}

async fn http_send_push(State(s): State<AppState>, headers: HeaderMap, Json(req): Json<PushReq>) -> (StatusCode, Json<Value>) {
    if !check_auth(&s, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false, "error": "unauthorized"})));
    }
    if let Some(e) = validate_push(&req, &s.webpush_policy).await {
        return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e})));
    }
    let o = push_send(&s, &req).await;
    (status_for(&o), Json(outcome_json("push", &push_target_label(&req), &o)))
}

fn status_for(o: &Outcome) -> StatusCode {
    if o.ok {
        StatusCode::OK
    } else if o.rate_limited {
        StatusCode::TOO_MANY_REQUESTS
    } else if o.error.as_deref().map(|e| e.contains("not configured")).unwrap_or(false) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_GATEWAY
    }
}

// ── NATS consumer ──────────────────────────────────────────────────────────────

async fn run_nats_consumer(s: AppState, url: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect(&url)
        .await?;
    tracing::info!("nats consumer connected to {url}; subscribing {CONTACT_EMAIL_SEND_SUBJECT} + {CONTACT_SMS_SEND_SUBJECT} + {CONTACT_PUSH_SEND_SUBJECT} (group {CONTACT_EMAIL_SEND_QUEUE_GROUP})");
    loop {
        let mut email_sub = match client.queue_subscribe(CONTACT_EMAIL_SEND_SUBJECT, CONTACT_EMAIL_SEND_QUEUE_GROUP.to_string()).await {
            Ok(sub) => sub,
            Err(error) => {
                tracing::error!("nats consumer subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let mut sms_sub = match client.queue_subscribe(CONTACT_SMS_SEND_SUBJECT, CONTACT_EMAIL_SEND_QUEUE_GROUP.to_string()).await {
            Ok(sub) => sub,
            Err(error) => {
                tracing::error!("nats consumer subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let mut push_sub = match client.queue_subscribe(CONTACT_PUSH_SEND_SUBJECT, CONTACT_EMAIL_SEND_QUEUE_GROUP.to_string()).await {
            Ok(sub) => sub,
            Err(error) => {
                tracing::error!("nats consumer subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        loop {
            tokio::select! {
                Some(msg) = email_sub.next() => {
                    let (s2, c2) = (s.clone(), client.clone());
                    tokio::spawn(async move { handle_email_msg(&s2, &c2, &msg.payload).await; });
                }
                Some(msg) = sms_sub.next() => {
                    let (s2, c2) = (s.clone(), client.clone());
                    tokio::spawn(async move { handle_sms_msg(&s2, &c2, &msg.payload).await; });
                }
                Some(msg) = push_sub.next() => {
                    let (s2, c2) = (s.clone(), client.clone());
                    tokio::spawn(async move { handle_push_msg(&s2, &c2, &msg.payload).await; });
                }
                else => break,
            }
        }
        tracing::error!("nats consumer subscriptions ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn publish_result(client: &async_nats::Client, value: Value) {
    if let Ok(bytes) = serde_json::to_vec(&value) {
        let _ = client.publish(CONTACT_SEND_RESULTS_SUBJECT, bytes.into()).await;
    }
}

// Enforces the optional NATS shared secret. Open (returns true) when no secret is configured;
// otherwise the message's `auth` must match in constant time.
fn nats_authorized(s: &AppState, auth: Option<&str>) -> bool {
    match &s.nats_secret {
        None => true,
        Some(secret) => auth.map(|got| constant_time_eq(got.as_bytes(), secret.as_bytes())).unwrap_or(false),
    }
}

async fn handle_email_msg(s: &AppState, client: &async_nats::Client, payload: &[u8]) {
    let Ok(req) = serde_json::from_slice::<EmailReq>(payload) else {
        publish_result(client, json!({"ok": false, "channel": "email", "error": "invalid payload"})).await;
        return;
    };
    if !nats_authorized(s, req.auth.as_deref()) {
        publish_result(client, json!({"ok": false, "channel": "email", "error": "unauthorized"})).await;
        return;
    }
    if let Some(e) = validate_email(&req.to, &req.subject, &req.html) {
        publish_result(client, json!({"ok": false, "channel": "email", "to": req.to, "error": e})).await;
        return;
    }
    let o = email_send(s, &req.to, &req.subject, &req.html, req.text.as_deref(), req.from.as_deref()).await;
    publish_result(client, outcome_json("email", &req.to, &o)).await;
}

async fn handle_sms_msg(s: &AppState, client: &async_nats::Client, payload: &[u8]) {
    let Ok(req) = serde_json::from_slice::<SmsReq>(payload) else {
        publish_result(client, json!({"ok": false, "channel": "sms", "error": "invalid payload"})).await;
        return;
    };
    if !nats_authorized(s, req.auth.as_deref()) {
        publish_result(client, json!({"ok": false, "channel": "sms", "error": "unauthorized"})).await;
        return;
    }
    if let Some(e) = validate_sms(&req.to, &req.body) {
        publish_result(client, json!({"ok": false, "channel": "sms", "to": req.to, "error": e})).await;
        return;
    }
    let o = sms_send(s, &req.to, &req.body).await;
    publish_result(client, outcome_json("sms", &req.to, &o)).await;
}

async fn handle_push_msg(s: &AppState, client: &async_nats::Client, payload: &[u8]) {
    let Ok(req) = serde_json::from_slice::<PushReq>(payload) else {
        publish_result(client, json!({"ok": false, "channel": "push", "error": "invalid payload"})).await;
        return;
    };
    if !nats_authorized(s, req.auth.as_deref()) {
        publish_result(client, json!({"ok": false, "channel": "push", "error": "unauthorized"})).await;
        return;
    }
    if let Some(e) = validate_push(&req, &s.webpush_policy).await {
        publish_result(client, json!({"ok": false, "channel": "push", "to": push_target_label(&req), "error": e})).await;
        return;
    }
    let o = push_send(s, &req).await;
    publish_result(client, outcome_json("push", &push_target_label(&req), &o)).await;
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sig) = signal(SignalKind::terminate()) {
            let _ = sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
