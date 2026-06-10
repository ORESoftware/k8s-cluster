// dd-email-sms-contact-rs — sends email (SendGrid primary; SES-ready) and SMS (Twilio) for the
// remote runtime, with per-process rate limiting and a shared-secret auth gate. Reachable two ways:
//
//   HTTP:  GET /healthz, GET /readyz, POST /send/email, POST /send/sms (x-server-auth gated)
//   NATS:  queue-subscribes (group dd-email-sms-contact) to the contact send lanes defined in
//          remote/libs/nats/subject-defs, handles each request once across replicas, and publishes
//          a per-send result summary. Subjects come from the generated dd-nats-subject-defs crate.
//
// Env: NATS_URL (enables the NATS consumer), SENDGRID_API_KEY, EMAIL_FROM (verified sender, e.g.
//      outreach@dancingdragons.cc), TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, TWILIO_FROM_NUMBER,
//      SERVER_AUTH_SECRET, EMAIL_RATE_PER_MIN (default 60), SMS_RATE_PER_MIN (default 30), HOST,
//      PORT (default 8120).

use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    CONTACT_EMAIL_SEND_QUEUE_GROUP, CONTACT_EMAIL_SEND_SUBJECT, CONTACT_SEND_RESULTS_SUBJECT,
    CONTACT_SMS_SEND_SUBJECT,
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    http: reqwest::Client,
    auth_secret: Option<String>,
    sendgrid_key: Option<String>,
    email_from: String,
    twilio_sid: Option<String>,
    twilio_token: Option<String>,
    twilio_from: Option<String>,
    email_bucket: Arc<Mutex<TokenBucket>>,
    sms_bucket: Arc<Mutex<TokenBucket>>,
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

#[derive(Deserialize)]
struct EmailReq {
    to: String,
    subject: String,
    html: String,
    text: Option<String>,
    from: Option<String>,
}
#[derive(Deserialize)]
struct SmsReq {
    to: String,
    body: String,
}

#[tokio::main]
async fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8120u16);
    let email_per_min = env::var("EMAIL_RATE_PER_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
    let sms_per_min = env::var("SMS_RATE_PER_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);

    let state = AppState {
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
        auth_secret: non_empty(env::var("SERVER_AUTH_SECRET").ok()),
        sendgrid_key: non_empty(env::var("SENDGRID_API_KEY").ok()).filter(|k| !k.contains("REPLACE")),
        email_from: env::var("EMAIL_FROM").unwrap_or_else(|_| "outreach@dancingdragons.cc".to_string()),
        twilio_sid: non_empty(env::var("TWILIO_ACCOUNT_SID").ok()),
        twilio_token: non_empty(env::var("TWILIO_AUTH_TOKEN").ok()),
        twilio_from: non_empty(env::var("TWILIO_FROM_NUMBER").ok()),
        email_bucket: Arc::new(Mutex::new(TokenBucket::new(email_per_min))),
        sms_bucket: Arc::new(Mutex::new(TokenBucket::new(sms_per_min))),
    };

    // Optional NATS consumer: handles send-requests published onto the contact lanes.
    if let Some(url) = non_empty(env::var("NATS_URL").ok()) {
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = run_nats_consumer(st, url).await {
                eprintln!("nats consumer stopped: {e}");
            }
        });
    } else {
        println!("NATS_URL unset — NATS consumer disabled (HTTP send still available)");
    }

    let app = Router::new()
        .route("/healthz", get(|| async { Json(json!({"ok": true, "service": "dd-email-sms-contact-rs"})) }))
        .route("/readyz", get(readyz))
        .route("/send/email", post(http_send_email))
        .route("/send/sms", post(http_send_sms))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse().expect("bind addr");
    println!("dd-email-sms-contact-rs listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await.expect("server");
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

// ── shared transports ──────────────────────────────────────────────────────────

async fn email_send(s: &AppState, to: &str, subject: &str, html: &str, text: Option<&str>, from: Option<&str>) -> Outcome {
    let Some(key) = s.sendgrid_key.clone() else {
        return Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some("SENDGRID_API_KEY not configured (needs a key with the mail.send scope)".into()), rate_limited: false };
    };
    if !s.email_bucket.lock().await.try_take() {
        return Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some("email rate limit exceeded".into()), rate_limited: true };
    }
    let from = from.map(str::to_string).unwrap_or_else(|| s.email_from.clone());
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
            let txt = r.text().await.unwrap_or_default();
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
            let txt = r.text().await.unwrap_or_default();
            Outcome { ok: false, transport: "twilio", upstream_status: Some(code), error: Some(txt), rate_limited: false }
        }
        Err(e) => Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some(format!("request failed: {e}")), rate_limited: false },
    }
}

fn outcome_json(channel: &str, to: &str, o: &Outcome) -> Value {
    json!({"ok": o.ok, "channel": channel, "to": to, "transport": o.transport, "upstreamStatus": o.upstream_status, "error": o.error, "rateLimited": o.rate_limited})
}

// ── HTTP handlers ──────────────────────────────────────────────────────────────

async fn readyz(State(s): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "email": {"sendgrid_configured": s.sendgrid_key.is_some(), "from": s.email_from},
        "sms": {"twilio_configured": s.twilio_sid.is_some() && s.twilio_token.is_some() && s.twilio_from.is_some()},
        "nats": {"consumer_enabled": env::var("NATS_URL").map(|v| !v.is_empty()).unwrap_or(false)},
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

const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;
const MAX_BODY_BYTES: usize = 1600; // ~10 SMS segments

// Returns Some(error) when the request is malformed.
fn validate_email(to: &str, subject: &str, html: &str) -> Option<&'static str> {
    if to.len() < 3 || to.len() > 320 || !to.contains('@') || !to.split('@').nth(1).map(|d| d.contains('.')).unwrap_or(false) {
        return Some("invalid recipient email");
    }
    if subject.is_empty() || subject.len() > 1000 {
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

fn status_for(o: &Outcome) -> StatusCode {
    if o.ok {
        StatusCode::OK
    } else if o.rate_limited {
        StatusCode::TOO_MANY_REQUESTS
    } else if o.error.as_deref() == Some("Twilio not configured") || o.error.as_deref().map(|e| e.contains("not configured")).unwrap_or(false) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_GATEWAY
    }
}

// ── NATS consumer ──────────────────────────────────────────────────────────────

async fn run_nats_consumer(s: AppState, url: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = async_nats::connect(&url).await?;
    println!("nats consumer connected to {url}; subscribing {CONTACT_EMAIL_SEND_SUBJECT} + {CONTACT_SMS_SEND_SUBJECT} (group {CONTACT_EMAIL_SEND_QUEUE_GROUP})");
    let mut email_sub = client.queue_subscribe(CONTACT_EMAIL_SEND_SUBJECT, CONTACT_EMAIL_SEND_QUEUE_GROUP.to_string()).await?;
    let mut sms_sub = client.queue_subscribe(CONTACT_SMS_SEND_SUBJECT, CONTACT_EMAIL_SEND_QUEUE_GROUP.to_string()).await?;
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
            else => break,
        }
    }
    Ok(())
}

async fn publish_result(client: &async_nats::Client, value: Value) {
    if let Ok(bytes) = serde_json::to_vec(&value) {
        let _ = client.publish(CONTACT_SEND_RESULTS_SUBJECT, bytes.into()).await;
    }
}

async fn handle_email_msg(s: &AppState, client: &async_nats::Client, payload: &[u8]) {
    let Ok(req) = serde_json::from_slice::<EmailReq>(payload) else {
        publish_result(client, json!({"ok": false, "channel": "email", "error": "invalid payload"})).await;
        return;
    };
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
    if let Some(e) = validate_sms(&req.to, &req.body) {
        publish_result(client, json!({"ok": false, "channel": "sms", "to": req.to, "error": e})).await;
        return;
    }
    let o = sms_send(s, &req.to, &req.body).await;
    publish_result(client, outcome_json("sms", &req.to, &o)).await;
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
