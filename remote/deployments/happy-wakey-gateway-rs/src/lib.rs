use async_trait::async_trait;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, Response, StatusCode},
    response::{Html, IntoResponse},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Write,
    path::{Path as FilePath, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use utoipa::openapi::{
    security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Components, OpenApi,
};
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::Scalar;

const SERVICE_NAME: &str = "happy-wakey-gateway-rs";
const STORE_VERSION: u8 = 1;
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_SYNC_JOBS: usize = 200;
const MAX_STORED_JOBS: usize = 10_000;
const MAX_TITLE_CHARS: usize = 160;
const MAX_BODY_CHARS: usize = 1_000;
const MAX_ID_CHARS: usize = 128;
const MIN_ID_CHARS: usize = 16;
const DISPATCH_BATCH: usize = 50;
const CONTACT_EMAIL_SEND_SUBJECT: &str = "dd.remote.contact.email.send";

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub state_path: PathBuf,
    pub reminder_horizon: Duration,
    pub scheduler_interval: Duration,
    pub sms_connector_configured: bool,
    pub push_connector_configured: bool,
    pub geolocation_connector_configured: bool,
    pub mcp_broker_configured: bool,
    pub task_manager_connector_configured: bool,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            state_path: PathBuf::from(
                optional_env("HAPPY_WAKEY_STATE_PATH")
                    .unwrap_or_else(|| "/var/lib/happy-wakey/reminders.json".into()),
            ),
            reminder_horizon: Duration::from_secs(env_u64(
                "HAPPY_WAKEY_REMINDER_HORIZON_SECONDS",
                14 * 24 * 60 * 60,
                60 * 60,
                31 * 24 * 60 * 60,
            )?),
            scheduler_interval: Duration::from_secs(env_u64(
                "HAPPY_WAKEY_SCHEDULER_INTERVAL_SECONDS",
                15,
                1,
                300,
            )?),
            sms_connector_configured: env_flag("HAPPY_WAKEY_SMS_ENABLED"),
            push_connector_configured: env_flag("HAPPY_WAKEY_PUSH_ENABLED"),
            geolocation_connector_configured: optional_env("GEOLOCATION_BASE_URL").is_some(),
            mcp_broker_configured: optional_env("MCP_BROKER_BASE_URL").is_some(),
            task_manager_connector_configured: optional_env("TASK_MANAGER_BASE_URL").is_some(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub user_id: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyFailure {
    Unauthorized,
    Unavailable,
}

#[async_trait]
pub trait IdentityVerifier: Send + Sync {
    async fn verify(&self, access_token: &str) -> Result<Identity, VerifyFailure>;
}

pub struct SharedAuthVerifier {
    http: reqwest::Client,
    introspect_url: url::Url,
    service_secret: String,
}

impl SharedAuthVerifier {
    pub fn new(base_url: &str, service_secret: String) -> Result<Self, String> {
        if service_secret.trim().len() < 32 {
            return Err("SHARED_AUTH_INTROSPECT_SECRET must contain at least 32 characters".into());
        }
        let mut base = parse_service_url(base_url, "SHARED_AUTH_BASE_URL")?;
        ensure_directory_url(&mut base);
        let introspect_url = base
            .join("auth/introspect")
            .map_err(|_| "SHARED_AUTH_BASE_URL cannot form the introspection endpoint")?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(6))
            .user_agent("happy-wakey-gateway-rs/0.1")
            .build()
            .map_err(|_| "could not build shared-auth HTTP client")?;
        Ok(Self {
            http,
            introspect_url,
            service_secret,
        })
    }
}

#[derive(Deserialize)]
struct IntrospectionResponse {
    active: bool,
    sub: Option<String>,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
}

#[async_trait]
impl IdentityVerifier for SharedAuthVerifier {
    async fn verify(&self, access_token: &str) -> Result<Identity, VerifyFailure> {
        let response = self
            .http
            .post(self.introspect_url.clone())
            .bearer_auth(&self.service_secret)
            .json(&json!({ "token": access_token }))
            .send()
            .await
            .map_err(|_| VerifyFailure::Unavailable)?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(VerifyFailure::Unavailable);
        }
        if !response.status().is_success() {
            return Err(VerifyFailure::Unavailable);
        }
        let body: IntrospectionResponse = response
            .json()
            .await
            .map_err(|_| VerifyFailure::Unavailable)?;
        let user_id = body
            .sub
            .filter(|value| valid_identifier(value))
            .ok_or(VerifyFailure::Unauthorized)?;
        if !body.active {
            return Err(VerifyFailure::Unauthorized);
        }
        Ok(Identity {
            user_id,
            email: body.email.filter(|value| valid_email(value)),
            email_verified: body.email_verified,
        })
    }
}

#[derive(Clone, Debug)]
pub struct EmailDelivery {
    pub idempotency_key: String,
    pub to: String,
    pub title: String,
    pub body: String,
}

#[async_trait]
pub trait ContactPublisher: Send + Sync {
    async fn publish_email(&self, delivery: &EmailDelivery) -> Result<(), ()>;
    fn ready(&self) -> bool;
}

pub struct NatsContactPublisher {
    client: async_nats::Client,
    message_secret: Option<String>,
}

impl NatsContactPublisher {
    pub async fn connect(nats_url: &str, message_secret: Option<String>) -> Result<Self, String> {
        let parsed = parse_service_url(nats_url, "NATS_URL")?;
        if !matches!(parsed.scheme(), "nats" | "tls") {
            return Err("NATS_URL must use nats:// or tls://".into());
        }
        let client = tokio::time::timeout(
            Duration::from_secs(10),
            async_nats::ConnectOptions::new().connect(nats_url),
        )
        .await
        .map_err(|_| "NATS connection timed out")?
        .map_err(|_| "NATS connection failed")?;
        Ok(Self {
            client,
            message_secret,
        })
    }
}

#[async_trait]
impl ContactPublisher for NatsContactPublisher {
    async fn publish_email(&self, delivery: &EmailDelivery) -> Result<(), ()> {
        let payload = serde_json::to_vec(&json!({
            "to": delivery.to,
            "subject": delivery.title,
            "html": format!(
                "<p>{}</p>",
                html_escape(&delivery.body).replace('\n', "<br>")
            ),
            "text": delivery.body,
            "auth": self.message_secret,
            "idempotency_key": delivery.idempotency_key,
            "source": SERVICE_NAME,
        }))
        .map_err(|_| ())?;
        let response = tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .request(CONTACT_EMAIL_SEND_SUBJECT, payload.into()),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        let result: ContactResult = serde_json::from_slice(&response.payload).map_err(|_| ())?;
        if result.ok && result.idempotency_key.as_deref() == Some(&delivery.idempotency_key) {
            Ok(())
        } else {
            Err(())
        }
    }

    fn ready(&self) -> bool {
        self.client.connection_state() == async_nats::connection::State::Connected
    }
}

#[derive(Deserialize)]
struct ContactResult {
    ok: bool,
    idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReminderStatus {
    Pending,
    Dispatching,
    Dispatched,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredReminder {
    job_id: String,
    idempotency_key: String,
    user_id: String,
    email: String,
    title: String,
    body: String,
    trigger_at: i64,
    next_attempt_at: i64,
    status: ReminderStatus,
    attempts: u16,
    updated_at: i64,
    delivered_at: Option<i64>,
}

impl StoredReminder {
    fn delivery(&self) -> EmailDelivery {
        EmailDelivery {
            idempotency_key: self.idempotency_key.clone(),
            to: self.email.clone(),
            title: self.title.clone(),
            body: self.body.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct StoreFile {
    version: u8,
    jobs: BTreeMap<String, StoredReminder>,
}

pub struct ReminderStore {
    path: PathBuf,
    data: Mutex<StoreFile>,
}

impl ReminderStore {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let mut data = match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 16 * 1024 * 1024 {
                    return Err("reminder store exceeds 16 MiB".into());
                }
                serde_json::from_slice::<StoreFile>(&bytes)
                    .map_err(|_| "reminder store is not valid JSON")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreFile::default(),
            Err(_) => return Err("reminder store could not be read".into()),
        };
        data.version = STORE_VERSION;
        let now = now_unix();
        for job in data.jobs.values_mut() {
            if job.status == ReminderStatus::Dispatching {
                job.status = ReminderStatus::Pending;
                job.next_attempt_at = now.saturating_add(5);
            }
        }
        persist_store_file(&path, &data)?;
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    async fn sync_user(
        &self,
        identity: &Identity,
        jobs: Vec<ValidatedReminder>,
    ) -> Result<SyncResult, String> {
        let email = identity
            .email
            .as_ref()
            .filter(|_| identity.email_verified)
            .ok_or_else(|| "verified_email_required".to_string())?;
        let now = now_unix();
        let mut data = self.data.lock().await;
        let mut candidate = data.clone();
        let incoming_ids: BTreeSet<String> = jobs.iter().map(|job| job.job_id.clone()).collect();
        let before = candidate.jobs.len();
        candidate.jobs.retain(|_, job| {
            job.user_id != identity.user_id
                || job.status == ReminderStatus::Dispatched
                || incoming_ids.contains(&job.job_id)
        });
        let canceled = before.saturating_sub(candidate.jobs.len());
        let mut accepted = 0usize;
        let mut unchanged = 0usize;

        for job in jobs {
            let key = store_key(&identity.user_id, &job.job_id);
            if candidate
                .jobs
                .get(&key)
                .is_some_and(|existing| existing.idempotency_key == job.idempotency_key)
            {
                unchanged += 1;
                continue;
            }
            candidate.jobs.insert(
                key,
                StoredReminder {
                    job_id: job.job_id,
                    idempotency_key: job.idempotency_key,
                    user_id: identity.user_id.clone(),
                    email: email.clone(),
                    title: job.title,
                    body: job.body,
                    trigger_at: job.trigger_at,
                    next_attempt_at: job.trigger_at,
                    status: ReminderStatus::Pending,
                    attempts: 0,
                    updated_at: now,
                    delivered_at: None,
                },
            );
            accepted += 1;
        }
        prune_store(&mut candidate, now);
        if candidate.jobs.len() > MAX_STORED_JOBS {
            return Err("reminder_capacity_reached".into());
        }
        self.persist_blocking(&candidate)?;
        *data = candidate;
        Ok(SyncResult {
            accepted,
            unchanged,
            canceled,
        })
    }

    async fn cancel_user_job(&self, user_id: &str, job_id: &str) -> Result<bool, String> {
        let mut data = self.data.lock().await;
        let mut candidate = data.clone();
        let removed = candidate.jobs.remove(&store_key(user_id, job_id)).is_some();
        if removed {
            self.persist_blocking(&candidate)?;
            *data = candidate;
        }
        Ok(removed)
    }

    async fn status_for(&self, user_id: &str) -> ReminderCounts {
        let data = self.data.lock().await;
        let mut counts = ReminderCounts::default();
        for job in data.jobs.values().filter(|job| job.user_id == user_id) {
            match job.status {
                ReminderStatus::Pending | ReminderStatus::Dispatching => counts.pending += 1,
                ReminderStatus::Dispatched => counts.dispatched += 1,
            }
        }
        counts
    }

    async fn claim_due(&self, now: i64) -> Result<Vec<StoredReminder>, String> {
        let mut data = self.data.lock().await;
        let mut candidate = data.clone();
        let mut claimed = Vec::new();
        for job in candidate.jobs.values_mut() {
            if claimed.len() >= DISPATCH_BATCH {
                break;
            }
            if job.status == ReminderStatus::Pending
                && job.trigger_at <= now
                && job.next_attempt_at <= now
            {
                job.status = ReminderStatus::Dispatching;
                job.attempts = job.attempts.saturating_add(1);
                job.updated_at = now;
                claimed.push(job.clone());
            }
        }
        if !claimed.is_empty() {
            self.persist_blocking(&candidate)?;
            *data = candidate;
        }
        Ok(claimed)
    }

    async fn complete_dispatch(
        &self,
        user_id: &str,
        job_id: &str,
        success: bool,
        now: i64,
    ) -> Result<(), String> {
        let mut data = self.data.lock().await;
        let mut candidate = data.clone();
        if let Some(job) = candidate.jobs.get_mut(&store_key(user_id, job_id)) {
            if success {
                job.status = ReminderStatus::Dispatched;
                job.delivered_at = Some(now);
            } else {
                job.status = ReminderStatus::Pending;
                let backoff = (u64::from(job.attempts).saturating_mul(30)).min(15 * 60);
                job.next_attempt_at = now.saturating_add(backoff as i64);
            }
            job.updated_at = now;
            self.persist_blocking(&candidate)?;
            *data = candidate;
        }
        Ok(())
    }

    fn persist_blocking(&self, data: &StoreFile) -> Result<(), String> {
        persist_store_file(&self.path, data)
    }
}

fn persist_store_file(path: &FilePath, data: &StoreFile) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "reminder store path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|_| "reminder store directory unavailable")?;
    let temporary = path.with_extension("json.tmp");
    let bytes =
        serde_json::to_vec_pretty(data).map_err(|_| "reminder store serialization failed")?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "reminder store temporary file unavailable")?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|_| "reminder store write failed")?;
    std::fs::rename(&temporary, path).map_err(|_| "reminder store atomic replace failed")?;
    Ok(())
}

#[derive(Default)]
pub struct Metrics {
    requests: AtomicU64,
    auth_failures: AtomicU64,
    reminder_syncs: AtomicU64,
    dispatches: AtomicU64,
    dispatch_failures: AtomicU64,
}

impl Metrics {
    fn render(&self) -> String {
        format!(
            "# HELP happy_wakey_gateway_requests_total HTTP requests handled.\n\
             # TYPE happy_wakey_gateway_requests_total counter\n\
             happy_wakey_gateway_requests_total {}\n\
             # HELP happy_wakey_gateway_auth_failures_total Authentication failures.\n\
             # TYPE happy_wakey_gateway_auth_failures_total counter\n\
             happy_wakey_gateway_auth_failures_total {}\n\
             # HELP happy_wakey_gateway_reminder_syncs_total Reminder reconciliation requests.\n\
             # TYPE happy_wakey_gateway_reminder_syncs_total counter\n\
             happy_wakey_gateway_reminder_syncs_total {}\n\
             # HELP happy_wakey_gateway_dispatches_total Reminder messages published.\n\
             # TYPE happy_wakey_gateway_dispatches_total counter\n\
             happy_wakey_gateway_dispatches_total {}\n\
             # HELP happy_wakey_gateway_dispatch_failures_total Reminder publish failures.\n\
             # TYPE happy_wakey_gateway_dispatch_failures_total counter\n\
             happy_wakey_gateway_dispatch_failures_total {}\n",
            self.requests.load(Ordering::Relaxed),
            self.auth_failures.load(Ordering::Relaxed),
            self.reminder_syncs.load(Ordering::Relaxed),
            self.dispatches.load(Ordering::Relaxed),
            self.dispatch_failures.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    verifier: Arc<dyn IdentityVerifier>,
    publisher: Arc<dyn ContactPublisher>,
    store: Arc<ReminderStore>,
    metrics: Arc<Metrics>,
    test_sequence: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        verifier: Arc<dyn IdentityVerifier>,
        publisher: Arc<dyn ContactPublisher>,
        store: Arc<ReminderStore>,
    ) -> Self {
        Self {
            config,
            verifier,
            publisher,
            store,
            metrics: Arc::new(Metrics::default()),
            test_sequence: Arc::new(AtomicU64::new(1)),
        }
    }
}

pub fn app(state: AppState) -> Router {
    let (router, openapi) = api_router().split_for_parts();
    router
        .layer(Extension(Arc::new(finalize_openapi(openapi))))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub fn openapi_document() -> OpenApi {
    let (_, openapi) = api_router().split_for_parts();
    finalize_openapi(openapi)
}

fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(metrics))
        .routes(routes!(api_docs))
        .routes(routes!(api_docs_html))
        .routes(routes!(docs_api_html))
        .routes(routes!(bootstrap))
        .routes(routes!(sync_reminders))
        .routes(routes!(reminder_status))
        .routes(routes!(cancel_reminder))
        .routes(routes!(test_reminder))
}

fn finalize_openapi(mut openapi: OpenApi) -> OpenApi {
    openapi.info.title = "Happy Wakey gateway API".to_string();
    openapi.info.version = env!("CARGO_PKG_VERSION").to_string();
    openapi.info.description = Some(
        "Shared-auth-backed capabilities and durable off-app calendar reminder reconciliation."
            .to_string(),
    );
    openapi
        .components
        .get_or_insert_with(Components::new)
        .add_security_scheme(
            "shared_auth_bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some(
                        "Short-lived access token issued by the shared-auth exchange.".to_string(),
                    ))
                    .build(),
            ),
        );
    openapi
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getHappyWakeyHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process is alive", body = Value))
)]
async fn healthz() -> Json<Value> {
    Json(json!({ "ok": true, "service": SERVICE_NAME }))
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getHappyWakeyReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Required dependencies are ready", body = Value),
        (status = 503, description = "A required dependency is unavailable", body = Value)
    )
)]
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let ready = state.publisher.ready();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "dependencies": {
                "shared_auth": "configured",
                "reminder_store": "ready",
                "contact_queue": if ready { "ready" } else { "unavailable" },
            }
        })),
    )
}

#[utoipa::path(
    get,
    path = "/metrics",
    operation_id = "getHappyWakeyMetrics",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Prometheus text exposition", body = String, content_type = "text/plain"))
)]
async fn metrics(State(state): State<AppState>) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; version=0.0.4")
        .body(Body::from(state.metrics.render()))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getHappyWakeyOpenApi",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Generated OpenAPI 3.1 contract"))
)]
async fn api_docs(Extension(openapi): Extension<Arc<OpenApi>>) -> Json<OpenApi> {
    Json((*openapi).clone())
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getHappyWakeyApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Interactive Scalar API reference", body = String, content_type = "text/html"))
)]
async fn api_docs_html(Extension(openapi): Extension<Arc<OpenApi>>) -> Html<String> {
    Html(Scalar::new((*openapi).clone()).to_html())
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getHappyWakeyApiReferenceAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the API reference", body = String, content_type = "text/html"))
)]
async fn docs_api_html(Extension(openapi): Extension<Arc<OpenApi>>) -> Html<String> {
    Html(Scalar::new((*openapi).clone()).to_html())
}

#[utoipa::path(
    get,
    path = "/v1/bootstrap",
    operation_id = "getHappyWakeyBootstrap",
    tag = "product",
    security(("shared_auth_bearer" = [])),
    responses(
        (status = 200, description = "Bounded product capabilities and reminder counts", body = Value),
        (status = 401, description = "Missing or invalid shared-auth bearer", body = Value),
        (status = 503, description = "Authentication dependency unavailable", body = Value)
    )
)]
async fn bootstrap(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let identity = match authorize(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    let counts = state.store.status_for(&identity.user_id).await;
    (
        StatusCode::OK,
        Json(json!({
            "service": SERVICE_NAME,
            "api_version": "v1",
            "user_id": identity.user_id,
            "capabilities": {
                "remote_reminders": true,
                "email": state.publisher.ready(),
                "sms": state.config.sms_connector_configured,
                "push": state.config.push_connector_configured,
                "geolocation": state.config.geolocation_connector_configured,
                "mcp": state.config.mcp_broker_configured,
                "task_managers": state.config.task_manager_connector_configured,
            },
            "reminders": counts,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ReminderSyncRequest {
    jobs: Vec<ReminderJobRequest>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ReminderJobRequest {
    job_id: String,
    idempotency_key: String,
    title: String,
    body: String,
    trigger_at: i64,
    channel: String,
}

#[derive(Clone, Debug)]
struct ValidatedReminder {
    job_id: String,
    idempotency_key: String,
    title: String,
    body: String,
    trigger_at: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct SyncResult {
    accepted: usize,
    unchanged: usize,
    canceled: usize,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
struct SyncResponse {
    ok: bool,
    result: SyncResult,
}

#[utoipa::path(
    put,
    path = "/v1/reminders/sync",
    operation_id = "syncHappyWakeyReminders",
    tag = "reminders",
    security(("shared_auth_bearer" = [])),
    request_body = ReminderSyncRequest,
    responses(
        (status = 200, description = "User reminder set reconciled", body = SyncResponse),
        (status = 400, description = "Invalid bounded reminder request", body = Value),
        (status = 401, description = "Missing or invalid shared-auth bearer", body = Value),
        (status = 403, description = "Verified account email required", body = Value),
        (status = 503, description = "A required dependency is unavailable", body = Value)
    )
)]
async fn sync_reminders(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ReminderSyncRequest>,
) -> impl IntoResponse {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let identity = match authorize(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    if !identity.email_verified
        || identity
            .email
            .as_deref()
            .is_none_or(|value| !valid_email(value))
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "verified_email_required",
            "A verified account email is required for cloud reminders.",
        );
    }
    let jobs = match validate_sync(request, state.config.reminder_horizon) {
        Ok(jobs) => jobs,
        Err(code) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                code,
                "The reminder request was invalid.",
            )
        }
    };
    match state.store.sync_user(&identity, jobs).await {
        Ok(result) => {
            state.metrics.reminder_syncs.fetch_add(1, Ordering::Relaxed);
            (StatusCode::OK, Json(SyncResponse { ok: true, result })).into_response()
        }
        Err(code) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            &code,
            "Reminder storage is unavailable.",
        ),
    }
}

#[utoipa::path(
    get,
    path = "/v1/reminders/status",
    operation_id = "getHappyWakeyReminderStatus",
    tag = "reminders",
    security(("shared_auth_bearer" = [])),
    responses(
        (status = 200, description = "User-scoped reminder counts", body = ReminderStatusResponse),
        (status = 401, description = "Missing or invalid shared-auth bearer", body = Value)
    )
)]
async fn reminder_status(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let identity = match authorize(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    let counts = state.store.status_for(&identity.user_id).await;
    (
        StatusCode::OK,
        Json(ReminderStatusResponse {
            ok: true,
            reminders: counts,
        }),
    )
        .into_response()
}

#[utoipa::path(
    delete,
    path = "/v1/reminders/jobs/{job_id}",
    operation_id = "cancelHappyWakeyReminder",
    tag = "reminders",
    security(("shared_auth_bearer" = [])),
    params(("job_id" = String, Path, description = "Deterministic reminder job identifier")),
    responses(
        (status = 200, description = "Reminder canceled or already absent", body = Value),
        (status = 400, description = "Invalid reminder identifier", body = Value),
        (status = 401, description = "Missing or invalid shared-auth bearer", body = Value)
    )
)]
async fn cancel_reminder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let identity = match authorize(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    if !valid_identifier(&job_id) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_job_id",
            "The reminder identifier was invalid.",
        );
    }
    match state
        .store
        .cancel_user_job(&identity.user_id, &job_id)
        .await
    {
        Ok(removed) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "removed": removed })),
        )
            .into_response(),
        Err(_) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "Reminder storage is unavailable.",
        ),
    }
}

#[utoipa::path(
    post,
    path = "/v1/reminders/test",
    operation_id = "testHappyWakeyReminder",
    tag = "reminders",
    security(("shared_auth_bearer" = [])),
    responses(
        (status = 202, description = "Test email reminder queued", body = Value),
        (status = 401, description = "Missing or invalid shared-auth bearer", body = Value),
        (status = 403, description = "Verified account email required", body = Value)
    )
)]
async fn test_reminder(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let identity = match authorize(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    if !identity.email_verified
        || identity
            .email
            .as_deref()
            .is_none_or(|value| !valid_email(value))
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "verified_email_required",
            "A verified account email is required for cloud reminders.",
        );
    }
    let now = now_unix();
    let sequence = state.test_sequence.fetch_add(1, Ordering::Relaxed);
    let job_id = format!("cloud-test-{now}-{sequence:016x}");
    let reminder = ValidatedReminder {
        idempotency_key: job_id.clone(),
        job_id,
        title: "Happy Wakey cloud reminders are ready".into(),
        body: "This reminder was delivered while the desktop app can be closed.".into(),
        trigger_at: now,
    };
    match state.store.sync_user(&identity, vec![reminder]).await {
        Ok(result) => (
            StatusCode::ACCEPTED,
            Json(SyncResponse { ok: true, result }),
        )
            .into_response(),
        Err(_) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "Reminder storage is unavailable.",
        ),
    }
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Identity, axum::response::Response> {
    let token = bearer(headers).filter(|value| value.len() <= MAX_TOKEN_BYTES);
    let Some(token) = token else {
        state.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
        return Err(api_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication is required.",
        ));
    };
    match state.verifier.verify(token).await {
        Ok(identity) => Ok(identity),
        Err(VerifyFailure::Unauthorized) => {
            state.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
            Err(api_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Authentication is required.",
            ))
        }
        Err(VerifyFailure::Unavailable) => Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Authentication is temporarily unavailable.",
        )),
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn validate_sync(
    request: ReminderSyncRequest,
    horizon: Duration,
) -> Result<Vec<ValidatedReminder>, &'static str> {
    if request.jobs.len() > MAX_SYNC_JOBS {
        return Err("too_many_jobs");
    }
    let now = now_unix();
    let latest = now.saturating_add(horizon.as_secs() as i64);
    let mut ids = BTreeSet::new();
    let mut output = Vec::with_capacity(request.jobs.len());
    for job in request.jobs {
        let job_id = clean_identifier(&job.job_id).ok_or("invalid_job_id")?;
        let idempotency_key =
            clean_identifier(&job.idempotency_key).ok_or("invalid_idempotency_key")?;
        if !ids.insert(job_id.clone()) {
            return Err("duplicate_job_id");
        }
        if job.channel != "email" {
            return Err("unsupported_channel");
        }
        if job.trigger_at < now.saturating_sub(60) || job.trigger_at > latest {
            return Err("trigger_out_of_range");
        }
        let title = clean_text(&job.title, MAX_TITLE_CHARS).ok_or("invalid_title")?;
        let body = clean_text(&job.body, MAX_BODY_CHARS).ok_or("invalid_body")?;
        output.push(ValidatedReminder {
            job_id,
            idempotency_key,
            title,
            body,
            trigger_at: job.trigger_at,
        });
    }
    Ok(output)
}

#[derive(Default, Serialize, utoipa::ToSchema)]
struct ReminderCounts {
    pending: usize,
    dispatched: usize,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ReminderStatusResponse {
    ok: bool,
    reminders: ReminderCounts,
}

pub async fn dispatch_due(state: &AppState, now: i64) -> Result<usize, String> {
    let jobs = state.store.claim_due(now).await?;
    let mut sent = 0usize;
    for job in jobs {
        let success = state.publisher.publish_email(&job.delivery()).await.is_ok();
        if success {
            sent += 1;
            state.metrics.dispatches.fetch_add(1, Ordering::Relaxed);
        } else {
            state
                .metrics
                .dispatch_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        state
            .store
            .complete_dispatch(&job.user_id, &job.job_id, success, now)
            .await?;
    }
    Ok(sent)
}

pub async fn run_scheduler(state: AppState) {
    loop {
        if let Err(error) = dispatch_due(&state, now_unix()).await {
            tracing::error!(error.code = %error, "reminder scheduler pass failed");
        }
        tokio::time::sleep(state.config.scheduler_interval).await;
    }
}

fn api_error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message,
            }
        })),
    )
        .into_response()
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_flag(name: &str) -> bool {
    optional_env(name).is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"))
}

fn env_u64(name: &str, default: u64, min: u64, max: u64) -> Result<u64, String> {
    let value = match optional_env(name) {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| format!("{name} must be an integer"))?,
        None => default,
    };
    if !(min..=max).contains(&value) {
        return Err(format!("{name} must be between {min} and {max}"));
    }
    Ok(value)
}

fn parse_service_url(raw: &str, name: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(raw).map_err(|_| format!("{name} must be an absolute URL"))?;
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(format!(
            "{name} must not contain credentials, query, or fragment"
        ));
    }
    Ok(parsed)
}

fn ensure_directory_url(url: &mut url::Url) {
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
}

fn valid_identifier(value: &str) -> bool {
    clean_identifier(value).as_deref() == Some(value)
}

fn clean_identifier(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !(MIN_ID_CHARS..=MAX_ID_CHARS).contains(&trimmed.len())
        || !trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn clean_text(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > max_chars {
        return None;
    }
    if trimmed
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn valid_email(value: &str) -> bool {
    let value = value.trim();
    value.len() <= 254
        && !value.contains(char::is_whitespace)
        && value
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
}

fn store_key(user_id: &str, job_id: &str) -> String {
    format!("{user_id}:{job_id}")
}

fn prune_store(data: &mut StoreFile, now: i64) {
    let cutoff = now.saturating_sub(7 * 24 * 60 * 60);
    data.jobs.retain(|_, job| {
        job.status != ReminderStatus::Dispatched || job.delivered_at.is_none_or(|at| at >= cutoff)
    });
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn validate_state_parent(path: &FilePath) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "HAPPY_WAKEY_STATE_PATH has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|_| "HAPPY_WAKEY_STATE_PATH parent is unavailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

    struct TestVerifier;

    #[async_trait]
    impl IdentityVerifier for TestVerifier {
        async fn verify(&self, token: &str) -> Result<Identity, VerifyFailure> {
            match token {
                "valid-user-a-token" => Ok(Identity {
                    user_id: "user-a-000000000".into(),
                    email: Some("a@example.test".into()),
                    email_verified: true,
                }),
                "valid-user-b-token" => Ok(Identity {
                    user_id: "user-b-000000000".into(),
                    email: Some("b@example.test".into()),
                    email_verified: true,
                }),
                "unverified-user-token" => Ok(Identity {
                    user_id: "user-c-000000000".into(),
                    email: Some("c@example.test".into()),
                    email_verified: false,
                }),
                "auth-service-down" => Err(VerifyFailure::Unavailable),
                _ => Err(VerifyFailure::Unauthorized),
            }
        }
    }

    #[derive(Default)]
    struct RecordingPublisher {
        sent: StdMutex<Vec<EmailDelivery>>,
        fail: bool,
    }

    #[async_trait]
    impl ContactPublisher for RecordingPublisher {
        async fn publish_email(&self, delivery: &EmailDelivery) -> Result<(), ()> {
            if self.fail {
                return Err(());
            }
            self.sent.lock().unwrap().push(delivery.clone());
            Ok(())
        }

        fn ready(&self) -> bool {
            !self.fail
        }
    }

    fn test_state(publisher: Arc<RecordingPublisher>) -> (tempfile::TempDir, AppState) {
        let directory = tempfile::tempdir().unwrap();
        let config = AppConfig {
            state_path: directory.path().join("reminders.json"),
            reminder_horizon: Duration::from_secs(14 * 24 * 60 * 60),
            scheduler_interval: Duration::from_secs(1),
            sms_connector_configured: false,
            push_connector_configured: false,
            geolocation_connector_configured: false,
            mcp_broker_configured: false,
            task_manager_connector_configured: false,
        };
        let store = Arc::new(ReminderStore::open(config.state_path.clone()).unwrap());
        (
            directory,
            AppState::new(config, Arc::new(TestVerifier), publisher, store),
        )
    }

    fn auth_request(
        method: &str,
        uri: &str,
        token: &str,
        body: Value,
    ) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn one_job(now: i64) -> Value {
        json!({
            "jobs": [{
                "job_id": "job-0000000000000001",
                "idempotency_key": "idem-000000000000001",
                "title": "Planning",
                "body": "Starts soon",
                "trigger_at": now,
                "channel": "email"
            }]
        })
    }

    #[tokio::test]
    async fn missing_and_invalid_auth_fail_uniformly() {
        let publisher = Arc::new(RecordingPublisher::default());
        let (_directory, state) = test_state(publisher);
        let router = app(state);

        for request in [
            axum::http::Request::builder()
                .uri("/v1/bootstrap")
                .body(Body::empty())
                .unwrap(),
            auth_request("GET", "/v1/bootstrap", "wrong-token-value", json!({})),
        ] {
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "unauthorized");
            assert!(body.get("sub").is_none());
        }
    }

    #[tokio::test]
    async fn bootstrap_is_bounded_and_does_not_leak_internal_addresses() {
        let publisher = Arc::new(RecordingPublisher::default());
        let (_directory, state) = test_state(publisher);
        let response = app(state)
            .oneshot(auth_request(
                "GET",
                "/v1/bootstrap",
                "valid-user-a-token",
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["capabilities"]["email"], true);
        let serialized = body.to_string();
        assert!(!serialized.contains("nats://"));
        assert!(!serialized.contains("svc.cluster.local"));
        assert!(!serialized.contains("secret"));
    }

    #[tokio::test]
    async fn reminder_sync_is_idempotent_and_user_scoped() {
        let publisher = Arc::new(RecordingPublisher::default());
        let (_directory, state) = test_state(publisher);
        let router = app(state.clone());
        let now = now_unix();

        let first = router
            .clone()
            .oneshot(auth_request(
                "PUT",
                "/v1/reminders/sync",
                "valid-user-a-token",
                one_job(now),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(response_json(first).await["result"]["accepted"], 1);

        let replay = router
            .clone()
            .oneshot(auth_request(
                "PUT",
                "/v1/reminders/sync",
                "valid-user-a-token",
                one_job(now),
            ))
            .await
            .unwrap();
        assert_eq!(response_json(replay).await["result"]["unchanged"], 1);

        let other = router
            .oneshot(auth_request(
                "GET",
                "/v1/reminders/status",
                "valid-user-b-token",
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response_json(other).await["reminders"]["pending"], 0);
    }

    #[tokio::test]
    async fn due_job_publishes_once_and_survives_reconciliation() {
        let publisher = Arc::new(RecordingPublisher::default());
        let (_directory, state) = test_state(publisher.clone());
        let identity = TestVerifier.verify("valid-user-a-token").await.unwrap();
        let now = now_unix();
        let jobs = validate_sync(
            serde_json::from_value(one_job(now)).unwrap(),
            state.config.reminder_horizon,
        )
        .unwrap();
        state.store.sync_user(&identity, jobs).await.unwrap();

        assert_eq!(dispatch_due(&state, now).await.unwrap(), 1);
        assert_eq!(dispatch_due(&state, now + 1).await.unwrap(), 0);
        assert_eq!(publisher.sent.lock().unwrap().len(), 1);

        let jobs = validate_sync(
            serde_json::from_value(one_job(now)).unwrap(),
            state.config.reminder_horizon,
        )
        .unwrap();
        let replay = state.store.sync_user(&identity, jobs).await.unwrap();
        assert_eq!(replay.unchanged, 1);
        assert_eq!(dispatch_due(&state, now + 2).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn unverified_email_and_unsupported_channel_fail_closed() {
        let publisher = Arc::new(RecordingPublisher::default());
        let (_directory, state) = test_state(publisher);
        let router = app(state);
        let now = now_unix();

        let unverified = router
            .clone()
            .oneshot(auth_request(
                "PUT",
                "/v1/reminders/sync",
                "unverified-user-token",
                one_job(now),
            ))
            .await
            .unwrap();
        assert_eq!(unverified.status(), StatusCode::FORBIDDEN);

        let mut sms = one_job(now);
        sms["jobs"][0]["channel"] = json!("sms");
        let unsupported = router
            .oneshot(auth_request(
                "PUT",
                "/v1/reminders/sync",
                "valid-user-a-token",
                sms,
            ))
            .await
            .unwrap();
        assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(unsupported).await["error"]["code"],
            "unsupported_channel"
        );
    }

    #[test]
    fn generated_openapi_describes_auth_and_typed_reminder_sync() {
        let contract = serde_json::to_value(openapi_document()).unwrap();
        assert_eq!(contract["openapi"], "3.1.0");
        assert_eq!(contract["info"]["title"], "Happy Wakey gateway API");

        let sync = &contract["paths"]["/v1/reminders/sync"]["put"];
        assert_eq!(sync["security"][0]["shared_auth_bearer"], json!([]));
        assert_eq!(
            sync["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/ReminderSyncRequest"
        );
        assert_eq!(
            sync["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/SyncResponse"
        );
        assert!(contract["components"]["securitySchemes"]["shared_auth_bearer"].is_object());
    }

    #[tokio::test]
    async fn failed_store_write_does_not_commit_memory_state() {
        let directory = tempfile::tempdir().unwrap();
        let store = ReminderStore {
            path: directory.path().to_path_buf(),
            data: Mutex::new(StoreFile::default()),
        };
        let identity = TestVerifier.verify("valid-user-a-token").await.unwrap();
        let jobs = validate_sync(
            serde_json::from_value(one_job(now_unix())).unwrap(),
            Duration::from_secs(14 * 24 * 60 * 60),
        )
        .unwrap();

        assert!(store.sync_user(&identity, jobs).await.is_err());
        assert!(store.data.lock().await.jobs.is_empty());
    }

    #[test]
    fn contact_subject_matches_generated_contract() {
        assert_eq!(CONTACT_EMAIL_SEND_SUBJECT, "dd.remote.contact.email.send");
    }
}
