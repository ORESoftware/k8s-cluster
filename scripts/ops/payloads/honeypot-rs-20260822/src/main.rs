use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use ipnet::IpNet;
use leptos::{prelude::*, ssr::render_to_string};
use serde::Serialize;
use serde_json::json;
use sha2::Sha256;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    env,
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::{signal, sync::{Mutex, Semaphore}, time::timeout};
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const MIN_KEY_BYTES: usize = 32;
const EVIDENCE_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
const LURE_IDS: &[&str] = &[
    "admin-console",
    "admin-login",
    "env-admin",
    "git-mirror",
    "backup-config",
    "api-auth",
    "api-backup",
];
const PAGE_CSS: &str = r#"
:root{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif}body{margin:0;min-height:100vh;background:#111827;color:#e5e7eb}main{max-width:760px;margin:0 auto;padding:56px 24px}.panel{background:#1f2937;border:1px solid #374151;border-radius:12px;padding:28px;box-shadow:0 18px 50px rgba(0,0,0,.28)}label{display:block;margin-top:16px;color:#cbd5e1}input{box-sizing:border-box;width:100%;margin-top:6px;padding:11px 12px;border-radius:8px;border:1px solid #4b5563;background:#111827;color:#f9fafb}button{margin-top:22px;padding:11px 18px;border:0;border-radius:8px;background:#2563eb;color:white;font-weight:650}small{color:#94a3b8}.status{display:inline-block;padding:4px 9px;border-radius:999px;background:#064e3b;color:#a7f3d0}
"#;

#[derive(Clone)]
struct SecretMaterial {
    honeytoken_key: Vec<u8>,
    pseudonym_key: Vec<u8>,
    event_key: Vec<u8>,
}

#[derive(Clone)]
struct Settings {
    bind_addr: SocketAddr,
    public_origin: String,
    lure_generation: String,
    trust_cloudflare_headers: bool,
    trusted_proxy_cidrs: Arc<Vec<IpNet>>,
    max_request_bytes: usize,
    max_concurrent_requests: usize,
    request_timeout_seconds: u64,
    secrets: Arc<SecretMaterial>,
}

#[derive(Debug, Error)]
enum ConfigError {
    #[error("{name} is required")]
    Missing { name: &'static str },
    #[error("{name} must contain at least {minimum} bytes")]
    WeakSecret { name: &'static str, minimum: usize },
    #[error("invalid {name}: {value}")]
    Invalid { name: &'static str, value: String },
}

impl Settings {
    fn from_env() -> Result<Self, ConfigError> {
        let bind_raw = env_or("BIND_ADDR", "0.0.0.0:8080");
        let bind_addr = bind_raw.parse().map_err(|_| ConfigError::Invalid {
            name: "BIND_ADDR",
            value: bind_raw,
        })?;
        Ok(Self {
            bind_addr,
            public_origin: env_or("PUBLIC_ORIGIN", "https://admin.example.invalid"),
            lure_generation: env_or("LURE_GENERATION", "v1"),
            trust_cloudflare_headers: parse_bool("TRUST_CLOUDFLARE_HEADERS", false)?,
            trusted_proxy_cidrs: Arc::new(parse_cidrs(&env_or("TRUSTED_PROXY_CIDRS", ""))?),
            max_request_bytes: parse_number("MAX_REQUEST_BYTES", 8_192usize)?,
            max_concurrent_requests: parse_number("MAX_CONCURRENT_REQUESTS", 64usize)?,
            request_timeout_seconds: parse_number("REQUEST_TIMEOUT_SECONDS", 3u64)?,
            secrets: Arc::new(SecretMaterial {
                honeytoken_key: required_secret("HONEYTOKEN_HMAC_KEY")?,
                pseudonym_key: required_secret("PSEUDONYM_HMAC_KEY")?,
                event_key: required_secret("EVENT_HMAC_KEY")?,
            }),
        })
    }

    #[cfg(test)]
    fn for_test() -> Self {
        Self {
            bind_addr: "127.0.0.1:0".parse().expect("test address must parse"),
            public_origin: "https://admin.example.invalid".to_owned(),
            lure_generation: "test-generation".to_owned(),
            trust_cloudflare_headers: false,
            trusted_proxy_cidrs: Arc::new(Vec::new()),
            max_request_bytes: 8_192,
            max_concurrent_requests: 8,
            request_timeout_seconds: 2,
            secrets: Arc::new(SecretMaterial {
                honeytoken_key: b"test-honeytoken-key-with-at-least-32-bytes".to_vec(),
                pseudonym_key: b"test-pseudonym-key-with-at-least-32-bytes".to_vec(),
                event_key: b"test-event-signing-key-with-at-least-32-bytes".to_vec(),
            }),
        }
    }
}

fn env_or(name: &'static str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn required_secret(name: &'static str) -> Result<Vec<u8>, ConfigError> {
    let value = env::var(name).map_err(|_| ConfigError::Missing { name })?;
    if value.len() < MIN_KEY_BYTES {
        return Err(ConfigError::WeakSecret { name, minimum: MIN_KEY_BYTES });
    }
    Ok(value.into_bytes())
}

fn parse_bool(name: &'static str, default: bool) -> Result<bool, ConfigError> {
    let value = env::var(name).unwrap_or_else(|_| default.to_string());
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid { name, value }),
    }
}

fn parse_number<T>(name: &'static str, default: T) -> Result<T, ConfigError>
where
    T: FromStr + ToString + Copy,
{
    let value = env::var(name).unwrap_or_else(|_| default.to_string());
    value.parse().map_err(|_| ConfigError::Invalid { name, value })
}

fn parse_cidrs(value: &str) -> Result<Vec<IpNet>, ConfigError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.parse().map_err(|_| ConfigError::Invalid {
            name: "TRUSTED_PROXY_CIDRS",
            value: item.to_owned(),
        }))
        .collect()
}

fn hmac_hex(key: &[u8], domain: &[u8], value: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(domain);
    mac.update(&[0]);
    mac.update(value);
    hex::encode(mac.finalize().into_bytes())
}

fn safe_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else {
            '_'
        })
        .collect();
    cleaned.trim_matches('_').chars().take(32).collect()
}

fn honeytoken(key: &[u8], lure_id: &str, generation: &str) -> String {
    let lure = safe_component(lure_id);
    let generation = safe_component(generation);
    let digest = hmac_hex(key, b"honeytoken", format!("{lure}:{generation}").as_bytes());
    format!("ores_hp_v1_{lure}_{generation}_{}", &digest[..24])
}

fn pseudonymize(key: &[u8], domain: &[u8], value: &[u8], prefix: &str) -> String {
    let digest = hmac_hex(key, domain, value);
    format!("{prefix}_{}", &digest[..24])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SignalKind {
    LureViewed,
    AuthAttempt,
    CredentialUsed,
    ExploitProbe,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResponseAction {
    Observe,
    RateLimit,
    ManagedChallenge,
    TemporaryBlock,
    HumanReview,
}

#[derive(Clone, Debug, Serialize)]
struct PolicyDecision {
    action: ResponseAction,
    ttl_seconds: Option<u64>,
    requires_human_review: bool,
    reason: &'static str,
}

#[derive(Default)]
struct EvidenceSnapshot {
    total: usize,
    auth_attempts: usize,
    credential_uses: usize,
    exploit_probes: usize,
    distinct_lures: usize,
}

struct EvidenceRecord {
    observed_at: Instant,
    signal: SignalKind,
    lure_id: String,
}

#[derive(Default)]
struct EvidenceLedger {
    records: Mutex<HashMap<String, VecDeque<EvidenceRecord>>>,
}

impl EvidenceLedger {
    async fn record(&self, subject: &str, signal: SignalKind, lure_id: &str) -> EvidenceSnapshot {
        let now = Instant::now();
        let mut records = self.records.lock().await;
        let subject_records = records.entry(subject.to_owned()).or_default();
        subject_records.retain(|record| now.duration_since(record.observed_at) <= EVIDENCE_WINDOW);
        subject_records.push_back(EvidenceRecord {
            observed_at: now,
            signal,
            lure_id: lure_id.to_owned(),
        });
        let mut snapshot = EvidenceSnapshot {
            total: subject_records.len(),
            ..EvidenceSnapshot::default()
        };
        let mut distinct_lures = HashSet::new();
        for record in subject_records {
            distinct_lures.insert(record.lure_id.as_str());
            match record.signal {
                SignalKind::LureViewed => {}
                SignalKind::AuthAttempt => snapshot.auth_attempts += 1,
                SignalKind::CredentialUsed => snapshot.credential_uses += 1,
                SignalKind::ExploitProbe => snapshot.exploit_probes += 1,
            }
        }
        snapshot.distinct_lures = distinct_lures.len();
        snapshot
    }
}

fn decide(snapshot: &EvidenceSnapshot) -> PolicyDecision {
    if snapshot.credential_uses >= 3 && snapshot.distinct_lures >= 3 {
        return PolicyDecision {
            action: ResponseAction::HumanReview,
            ttl_seconds: Some(86_400),
            requires_human_review: true,
            reason: "repeated honeytoken use across independent lures",
        };
    }
    if snapshot.credential_uses >= 2 {
        return PolicyDecision {
            action: ResponseAction::TemporaryBlock,
            ttl_seconds: Some(86_400),
            requires_human_review: false,
            reason: "repeated exact honeytoken use",
        };
    }
    if snapshot.credential_uses == 1 {
        return PolicyDecision {
            action: ResponseAction::ManagedChallenge,
            ttl_seconds: Some(3_600),
            requires_human_review: false,
            reason: "first exact honeytoken use",
        };
    }
    if snapshot.exploit_probes >= 4 || snapshot.auth_attempts >= 20 {
        return PolicyDecision {
            action: ResponseAction::ManagedChallenge,
            ttl_seconds: Some(1_800),
            requires_human_review: false,
            reason: "sustained exploit or authentication probing",
        };
    }
    if snapshot.auth_attempts >= 8 {
        return PolicyDecision {
            action: ResponseAction::RateLimit,
            ttl_seconds: Some(900),
            requires_human_review: false,
            reason: "repeated authentication attempts",
        };
    }
    PolicyDecision {
        action: ResponseAction::Observe,
        ttl_seconds: None,
        requires_human_review: false,
        reason: "insufficient evidence for active friction",
    }
}

#[derive(Clone)]
struct AppState {
    settings: Settings,
    tokens: Arc<BTreeMap<String, String>>,
    ledger: Arc<EvidenceLedger>,
    concurrency: Arc<Semaphore>,
}

impl AppState {
    fn new(settings: Settings) -> Self {
        let tokens = LURE_IDS.iter().map(|lure_id| (
            (*lure_id).to_owned(),
            honeytoken(&settings.secrets.honeytoken_key, lure_id, &settings.lure_generation),
        )).collect();
        let max_concurrent_requests = settings.max_concurrent_requests;
        Self {
            settings,
            tokens: Arc::new(tokens),
            ledger: Arc::new(EvidenceLedger::default()),
            concurrency: Arc::new(Semaphore::new(max_concurrent_requests)),
        }
    }

    fn token(&self, lure_id: &str) -> &str {
        self.tokens.get(lure_id).map(String::as_str).expect("declared lure has token")
    }
}

#[derive(Serialize)]
struct UnsignedSecurityEvent {
    schema: &'static str,
    observed_at_unix_ms: u128,
    request_id: String,
    subject_pseudonym: String,
    user_agent_pseudonym: String,
    method: String,
    path: String,
    lure_id: String,
    signal: SignalKind,
    request_bytes: usize,
    evidence_count: usize,
    distinct_lures: usize,
    decision: PolicyDecision,
    cf_ray: Option<String>,
    cf_country: Option<String>,
    cf_asn: Option<String>,
}

#[derive(Serialize)]
struct SecurityEvent {
    #[serde(flatten)]
    unsigned: UnsignedSecurityEvent,
    event_signature: String,
}

fn build_router(settings: Settings) -> Router {
    let state = AppState::new(settings);
    let max_request_bytes = state.settings.max_request_bytes;
    Router::new()
        .route("/", get(index))
        .route("/admin/login", get(admin_login))
        .route("/.env", get(env_lure))
        .route("/.git/config", get(git_lure))
        .route("/backup/config.json", get(backup_lure))
        .route("/api/v1/auth", post(auth_lure))
        .route("/api/v1/backup", get(backup_api_lure).post(auth_lure))
        .route("/robots.txt", get(robots))
        .route("/healthz", get(health))
        .route("/readyz", get(health))
        .fallback(fallback)
        .layer(RequestBodyLimitLayer::new(max_request_bytes))
        .layer(CatchPanicLayer::new())
        .layer(middleware::from_fn_with_state(state.clone(), request_guard))
        .with_state(state)
}

async fn request_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let permit = match timeout(Duration::from_millis(100), state.concurrency.clone().acquire_owned()).await {
        Ok(Ok(permit)) => permit,
        _ => return plain_response(StatusCode::TOO_MANY_REQUESTS, "service busy"),
    };
    let response = timeout(Duration::from_secs(state.settings.request_timeout_seconds), next.run(request)).await;
    drop(permit);
    let mut response = match response {
        Ok(response) => response,
        Err(_) => plain_response(StatusCode::REQUEST_TIMEOUT, "request timed out"),
    };
    apply_security_headers(response.headers_mut());
    response
}

async fn index(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "admin-console", SignalKind::LureViewed, 0).await;
    render_login(&state.settings.public_origin)
}

async fn admin_login(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "admin-login", SignalKind::LureViewed, 0).await;
    render_login(&state.settings.public_origin)
}

fn render_login(public_origin: &str) -> Response {
    let origin = public_origin.to_owned();
    let html = render_to_string(move || view! {
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>"Control Plane"</title>
                <style>{PAGE_CSS}</style>
            </head>
            <body>
                <main>
                    <div class="panel">
                        <span class="status">"Operational"</span>
                        <h1>"Control Plane"</h1>
                        <p>"Restricted operator access for " <code>{origin}</code></p>
                        <form method="post" action="/api/v1/auth" autocomplete="off">
                            <label for="username">"Operator ID"</label>
                            <input id="username" name="username" type="text"/>
                            <label for="password">"Access token"</label>
                            <input id="password" name="password" type="password"/>
                            <button type="submit">"Authenticate"</button>
                        </form>
                        <p><small>"Authorized operators only. Events are audited."</small></p>
                    </div>
                </main>
            </body>
        </html>
    });
    Html(format!("<!doctype html>{html}")).into_response()
}

async fn env_lure(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "env-admin", SignalKind::LureViewed, 0).await;
    let document = format!(
        "CONTROL_PLANE_USER=svc-control\nCONTROL_PLANE_TOKEN={}\nBACKUP_API_TOKEN={}\nDATABASE_URL=postgresql://readonly:{}@db-internal.example.invalid:5432/control\n",
        state.token("env-admin"), state.token("api-backup"), state.token("backup-config"),
    );
    typed_response(StatusCode::OK, "text/plain; charset=utf-8", document)
}

async fn git_lure(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "git-mirror", SignalKind::LureViewed, 0).await;
    let document = format!(
        "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = https://svc-mirror:{}@git-mirror.example.invalid/control-plane.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        state.token("git-mirror"),
    );
    typed_response(StatusCode::OK, "text/plain; charset=utf-8", document)
}

async fn backup_lure(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "backup-config", SignalKind::LureViewed, 0).await;
    Json(json!({
        "endpoint": "https://backup.example.invalid/v1/snapshots",
        "serviceAccount": "svc-backup",
        "apiToken": state.token("backup-config"),
        "retentionDays": 30
    })).into_response()
}

async fn backup_api_lure(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "api-backup", SignalKind::LureViewed, 0).await;
    Json(json!({
        "version": "v1",
        "token": state.token("api-backup"),
        "upload": "https://backup.example.invalid/v1/upload"
    })).into_response()
}

async fn auth_lure(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, state.settings.max_request_bytes).await {
        Ok(body) => body,
        Err(_) => return plain_response(StatusCode::PAYLOAD_TOO_LARGE, "request too large"),
    };
    let matched_lure = detect_honeytoken(&state, &parts.headers, &body);
    let (lure_id, signal) = matched_lure
        .map(|lure_id| (lure_id, SignalKind::CredentialUsed))
        .unwrap_or_else(|| ("api-auth".to_owned(), SignalKind::AuthAttempt));
    record_signal(&state, &parts, &lure_id, signal, body.len()).await;
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid credentials"}))).into_response()
}

async fn robots(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    record_signal(&state, &parts, "admin-console", SignalKind::LureViewed, 0).await;
    typed_response(
        StatusCode::OK,
        "text/plain; charset=utf-8",
        "User-agent: *\nDisallow: /admin/\nDisallow: /backup/\nDisallow: /.env\nDisallow: /.git/\n",
    )
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

async fn fallback(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    if is_probe_path(parts.uri.path()) {
        record_signal(&state, &parts, "generic-exploit-probe", SignalKind::ExploitProbe, 0).await;
    }
    plain_response(StatusCode::NOT_FOUND, "not found")
}

async fn record_signal(state: &AppState, parts: &Parts, lure_id: &str, signal: SignalKind, request_bytes: usize) {
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let trusted_proxy = state.settings.trust_cloudflare_headers
        && state.settings.trusted_proxy_cidrs.iter().any(|cidr| cidr.contains(&peer));
    let client_ip = if trusted_proxy {
        parts.headers.get("cf-connecting-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .unwrap_or(peer)
    } else {
        peer
    };
    let subject = pseudonymize(
        &state.settings.secrets.pseudonym_key,
        b"ip",
        client_ip.to_string().as_bytes(),
        "ip",
    );
    let user_agent = parts.headers.get(header::USER_AGENT)
        .map(HeaderValue::as_bytes)
        .unwrap_or_default();
    let user_agent_pseudonym = pseudonymize(
        &state.settings.secrets.pseudonym_key,
        b"ua",
        user_agent,
        "ua",
    );
    let snapshot = state.ledger.record(&subject, signal, lure_id).await;
    let decision = decide(&snapshot);
    let unsigned = UnsignedSecurityEvent {
        schema: "ores.honeypot.event.v1",
        observed_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
        request_id: Uuid::new_v4().to_string(),
        subject_pseudonym: subject,
        user_agent_pseudonym,
        method: parts.method.as_str().to_owned(),
        path: parts.uri.path().to_owned(),
        lure_id: lure_id.to_owned(),
        signal,
        request_bytes,
        evidence_count: snapshot.total,
        distinct_lures: snapshot.distinct_lures,
        decision,
        cf_ray: trusted_proxy.then(|| sanitized_header(&parts.headers, "cf-ray", 64)).flatten(),
        cf_country: trusted_proxy.then(|| sanitized_header(&parts.headers, "cf-ipcountry", 2)).flatten(),
        cf_asn: trusted_proxy.then(|| sanitized_header(&parts.headers, "cf-client-asn", 16)).flatten(),
    };
    let canonical = serde_json::to_vec(&unsigned).expect("security event serializes");
    let event = SecurityEvent {
        event_signature: hmac_hex(&state.settings.secrets.event_key, b"event", &canonical),
        unsigned,
    };
    info!(target: "security_event", event = %serde_json::to_string(&event).expect("security event serializes"));
}

fn detect_honeytoken(state: &AppState, headers: &HeaderMap, body: &Bytes) -> Option<String> {
    state.tokens.iter().find_map(|(lure_id, token)| {
        let needle = token.as_bytes();
        let header_match = headers.values().any(|value| contains_bytes(value.as_bytes(), needle));
        let body_match = contains_bytes(body, needle);
        (header_match || body_match).then(|| lure_id.clone())
    })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|window| window == needle)
}

fn sanitized_header(headers: &HeaderMap, name: &str, max_len: usize) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?;
    if value.is_empty() || value.len() > max_len
        || !value.chars().all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')) {
        return None;
    }
    Some(value.to_owned())
}

fn is_probe_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["wp-login.php", "phpmyadmin", ".aws/credentials", ".ssh/", "actuator/env", "vendor/phpunit", "cgi-bin/"]
        .iter().any(|needle| lower.contains(needle))
}

fn apply_security_headers(headers: &mut HeaderMap) {
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store, max-age=0"),
        (HeaderName::from_static("x-content-type-options"), "nosniff"),
        (HeaderName::from_static("referrer-policy"), "no-referrer"),
        (HeaderName::from_static("content-security-policy"), "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"),
        (HeaderName::from_static("x-robots-tag"), "noindex, nofollow, noarchive"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
}

fn typed_response(status: StatusCode, content_type: &'static str, body: impl Into<Body>) -> Response {
    Response::builder().status(status).header(header::CONTENT_TYPE, content_type)
        .body(body.into()).expect("static response is valid")
}

fn plain_response(status: StatusCode, body: &'static str) -> Response {
    typed_response(status, "text/plain; charset=utf-8", body)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt().json().flatten_event(true)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let settings = Settings::from_env()?;
    let bind_addr = settings.bind_addr;
    let router = build_router(settings);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    info!(bind_addr = %bind_addr, "deception service listening");
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async { signal::ctrl_c().await.expect("failed to install Ctrl+C handler"); };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler").recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = terminate => {} }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TEST_KEY: &[u8] = b"a-test-key-that-is-deliberately-long-enough";

    #[test]
    fn token_is_stable_and_vendor_neutral() {
        let first = honeytoken(TEST_KEY, "env-admin", "2026-08");
        let second = honeytoken(TEST_KEY, "env-admin", "2026-08");
        assert_eq!(first, second);
        assert!(first.starts_with("ores_hp_v1_env_admin_2026_08_"));
        for forbidden_prefix in ["ghp_", "github_pat_", "sk_live_", "AKIA"] {
            assert!(!first.starts_with(forbidden_prefix));
        }
    }

    #[tokio::test]
    async fn exact_token_use_escalates_reversibly() {
        let ledger = EvidenceLedger::default();
        let first = ledger.record("subject", SignalKind::CredentialUsed, "env-admin").await;
        assert!(matches!(decide(&first).action, ResponseAction::ManagedChallenge));
        let second = ledger.record("subject", SignalKind::CredentialUsed, "backup-config").await;
        assert!(matches!(decide(&second).action, ResponseAction::TemporaryBlock));
        let third = ledger.record("subject", SignalKind::CredentialUsed, "git-mirror").await;
        let decision = decide(&third);
        assert!(matches!(decision.action, ResponseAction::HumanReview));
        assert!(decision.requires_human_review);
    }

    #[tokio::test]
    async fn env_lure_exposes_only_synthetic_values() {
        let response = build_router(Settings::for_test()).oneshot(
            Request::builder().uri("/.env").body(Body::empty()).expect("request")
        ).await.expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.expect("body").to_bytes();
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(text.contains("ores_hp_v1_env_admin_test_generation_"));
        assert!(text.contains("example.invalid"));
    }

    #[tokio::test]
    async fn exact_honeytoken_use_is_rejected() {
        let settings = Settings::for_test();
        let token = honeytoken(&settings.secrets.honeytoken_key, "api-auth", &settings.lure_generation);
        let response = build_router(settings).oneshot(
            Request::builder().method("POST").uri("/api/v1/auth")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("username=operator")).expect("request")
        ).await.expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
