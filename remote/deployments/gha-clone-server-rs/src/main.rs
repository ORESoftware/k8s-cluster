use std::{
    collections::BTreeMap,
    env,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use gha_clone_server::{
    decide_capacity, decision_variables, BillingUsageResponse, CapacityDecision, OrgPolicy,
    VariableMutation,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use utoipa::{OpenApi, ToSchema};

const SERVICE: &str = "gha-clone-server";
const GITHUB_API_VERSION: &str = "2026-03-10";
const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
const DEFAULT_PORT: u16 = 8117;

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    github_api_base: String,
    github_app_id: String,
    github_app_installation_id: u64,
    github_app_private_key_path: PathBuf,
    operator_secret: String,
    mutation_enabled: bool,
    reconcile_interval: Duration,
    organization: String,
    policy: OrgPolicy,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let github_api_base = optional_env("GITHUB_API_BASE_URL")
            .unwrap_or_else(|| DEFAULT_GITHUB_API_BASE.to_string());
        if github_api_base != DEFAULT_GITHUB_API_BASE {
            return Err("GITHUB_API_BASE_URL must remain https://api.github.com".to_string());
        }

        let organization = required_env("GHA_ORGANIZATION")?;
        validate_org(&organization)?;
        let policy_raw = required_env("GHA_ORG_POLICY_JSON")?;
        let policy: OrgPolicy = serde_json::from_str(&policy_raw)
            .map_err(|error| format!("invalid GHA_ORG_POLICY_JSON: {error}"))?;
        policy
            .validate()
            .map_err(|error| format!("invalid policy for {organization}: {error}"))?;

        let operator_secret = required_env("SERVER_AUTH_SECRET")?;
        if operator_secret.len() < 32 {
            return Err("SERVER_AUTH_SECRET must contain at least 32 characters".to_string());
        }

        Ok(Self {
            host: optional_env("HOST").unwrap_or_else(|| "0.0.0.0".to_string()),
            port: optional_env("PORT")
                .map(|value| value.parse::<u16>())
                .transpose()
                .map_err(|error| format!("invalid PORT: {error}"))?
                .unwrap_or(DEFAULT_PORT),
            github_api_base,
            github_app_id: required_env("GITHUB_APP_ID")?,
            github_app_installation_id: required_env("GITHUB_APP_INSTALLATION_ID")?
                .parse::<u64>()
                .map_err(|error| format!("invalid GITHUB_APP_INSTALLATION_ID: {error}"))
                .and_then(|value| {
                    if value == 0 {
                        Err("GITHUB_APP_INSTALLATION_ID must be positive".to_string())
                    } else {
                        Ok(value)
                    }
                })?,
            github_app_private_key_path: PathBuf::from(required_env(
                "GITHUB_APP_PRIVATE_KEY_PATH",
            )?),
            operator_secret,
            mutation_enabled: env_bool("GHA_MUTATION_ENABLED", false),
            reconcile_interval: Duration::from_secs(env_u64("GHA_RECONCILE_INTERVAL_SECONDS", 900)),
            organization,
            policy,
        })
    }
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    billing_reads_total: AtomicU64,
    billing_read_failures_total: AtomicU64,
    reconcile_total: AtomicU64,
    reconcile_failures_total: AtomicU64,
    variable_mutations_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    config: Config,
    github: GitHubClient,
    metrics: Arc<Metrics>,
}

#[derive(Clone)]
struct GitHubClient {
    config: Config,
    http: reqwest::Client,
    token: Arc<Mutex<Option<CachedInstallationToken>>>,
}

struct CachedInstallationToken {
    token: String,
    refresh_at: Instant,
}

#[derive(Serialize)]
struct AppJwtClaims<'a> {
    iat: u64,
    exp: u64,
    iss: &'a str,
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "operator authentication required".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": self.message,
            })),
        )
            .into_response()
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    mutation_enabled: bool,
    organization: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct CapabilityResponse {
    service: &'static str,
    github_actions_protocol: &'static str,
    control_plane_clone: bool,
    arbitrary_command_execution: bool,
    supported_modes: Vec<&'static str>,
    variable_names: Vec<&'static str>,
    notes: Vec<&'static str>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct OrganizationDecisionResponse {
    organization: String,
    decision: CapacityDecision,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct ReconcileResponse {
    organization: String,
    dry_run: bool,
    decision: CapacityDecision,
    variables: BTreeMap<String, VariableMutation>,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        healthz,
        readyz,
        capabilities,
        organization_decision,
        reconcile_organization,
        metrics
    ),
    components(schemas(
        HealthResponse,
        CapabilityResponse,
        OrganizationDecisionResponse,
        ReconcileResponse,
        CapacityDecision,
        OrgPolicy,
        BillingUsageResponse,
        VariableMutation
    )),
    tags(
        (name = "capacity", description = "GitHub Actions usage and execution routing"),
        (name = "operations", description = "Service health and metrics")
    )
)]
struct ApiDoc;

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_env(key: &str) -> Result<String, String> {
    optional_env(key).ok_or_else(|| format!("{key} is required"))
}

fn env_bool(key: &str, fallback: bool) -> bool {
    optional_env(key)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    optional_env(key)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn validate_org(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 100 {
        return Err("organization name must be between 1 and 100 characters".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err(format!("invalid GitHub organization name: {value}"));
    }
    Ok(())
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get("x-server-auth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|candidate| {
            let presented = Sha256::digest(candidate.as_bytes());
            let expected = Sha256::digest(expected.as_bytes());
            presented[..].ct_eq(&expected[..]).into()
        })
}

impl GitHubClient {
    fn new(config: Config) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(RedirectPolicy::limited(3))
            .user_agent("oresoftware-gha-clone-server/0.1")
            .build()
            .map_err(|error| format!("failed to build GitHub HTTP client: {error}"))?;
        Ok(Self {
            config,
            http,
            token: Arc::new(Mutex::new(None)),
        })
    }

    async fn installation_token(&self) -> Result<String, ApiError> {
        let mut guard = self.token.lock().await;
        if let Some(cached) = guard.as_ref() {
            if Instant::now() < cached.refresh_at {
                return Ok(cached.token.clone());
            }
        }

        let private_key = tokio::fs::read(&self.config.github_app_private_key_path)
            .await
            .map_err(|error| {
                ApiError::internal(format!(
                    "failed to read GitHub App private key from {}: {error}",
                    self.config.github_app_private_key_path.display()
                ))
            })?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = AppJwtClaims {
            iat: now.saturating_sub(60),
            exp: now.saturating_add(540),
            iss: &self.config.github_app_id,
        };
        let jwt = encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(&private_key).map_err(|error| {
                ApiError::internal(format!("invalid GitHub App RSA private key: {error}"))
            })?,
        )
        .map_err(|error| ApiError::internal(format!("failed to sign GitHub App JWT: {error}")))?;

        let url = format!(
            "{}/app/installations/{}/access_tokens",
            self.config.github_api_base, self.config.github_app_installation_id
        );
        let response = self
            .http
            .post(url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .bearer_auth(jwt)
            .json(&json!({}))
            .send()
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("GitHub token request failed: {error}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            ApiError::bad_gateway(format!("failed to read GitHub token response: {error}"))
        })?;
        if !status.is_success() {
            return Err(ApiError::bad_gateway(format!(
                "GitHub installation token request returned {status}: {}",
                redact_github_body(&body)
            )));
        }
        let token: InstallationTokenResponse = serde_json::from_str(&body).map_err(|error| {
            ApiError::bad_gateway(format!(
                "invalid GitHub installation token response: {error}"
            ))
        })?;
        *guard = Some(CachedInstallationToken {
            token: token.token.clone(),
            refresh_at: Instant::now() + Duration::from_secs(50 * 60),
        });
        Ok(token.token)
    }

    async fn billing_usage(&self, org: &str) -> Result<BillingUsageResponse, ApiError> {
        validate_org(org).map_err(ApiError::not_found)?;
        let token = self.installation_token().await?;
        let url = format!(
            "{}/organizations/{org}/settings/billing/usage",
            self.config.github_api_base
        );
        let now = time::OffsetDateTime::now_utc();
        let period = [
            ("year", now.year().to_string()),
            ("month", (now.month() as u8).to_string()),
        ];
        let response = self
            .http
            .get(url)
            .query(&period)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("GitHub billing request failed: {error}"))
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            ApiError::bad_gateway(format!("failed to read GitHub billing response: {error}"))
        })?;
        if !status.is_success() {
            return Err(ApiError::bad_gateway(format!(
                "GitHub billing request returned {status}: {}",
                redact_github_body(&body)
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            ApiError::bad_gateway(format!("invalid GitHub billing response: {error}"))
        })
    }

    async fn upsert_variable(&self, org: &str, value: &VariableMutation) -> Result<(), ApiError> {
        let token = self.installation_token().await?;
        let patch_url = format!(
            "{}/orgs/{org}/actions/variables/{}",
            self.config.github_api_base, value.name
        );
        let body = json!({
            "name": &value.name,
            "value": &value.value,
            "visibility": &value.visibility,
            "selected_repository_ids": &value.selected_repository_ids,
        });
        let patch = self
            .http
            .patch(patch_url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("GitHub variable update failed: {error}"))
            })?;
        if patch.status().is_success() {
            return Ok(());
        }
        if patch.status() != reqwest::StatusCode::NOT_FOUND {
            let status = patch.status();
            let response_body = patch.text().await.unwrap_or_default();
            return Err(ApiError::bad_gateway(format!(
                "GitHub variable update returned {status}: {}",
                redact_github_body(&response_body)
            )));
        }

        let create_url = format!(
            "{}/orgs/{org}/actions/variables",
            self.config.github_api_base
        );
        let create = self
            .http
            .post(create_url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("GitHub variable create failed: {error}"))
            })?;
        if create.status().is_success() {
            return Ok(());
        }
        let status = create.status();
        let response_body = create.text().await.unwrap_or_default();
        Err(ApiError::bad_gateway(format!(
            "GitHub variable create returned {status}: {}",
            redact_github_body(&response_body)
        )))
    }
}

fn redact_github_body(body: &str) -> String {
    let compact = body.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(500).collect()
}

async fn decision_for_org(state: &AppState, org: &str) -> Result<CapacityDecision, ApiError> {
    validate_org(org).map_err(ApiError::not_found)?;
    if !state.config.organization.eq_ignore_ascii_case(org) {
        return Err(ApiError::not_found(format!(
            "this broker instance is scoped to {}, not {org}",
            state.config.organization
        )));
    }
    state
        .metrics
        .billing_reads_total
        .fetch_add(1, Ordering::Relaxed);
    let usage = match state.github.billing_usage(&state.config.organization).await {
        Ok(usage) => Some(usage.actions_minutes()),
        Err(error) => {
            state
                .metrics
                .billing_read_failures_total
                .fetch_add(1, Ordering::Relaxed);
            warn!(organization = org, error = %error.message, "billing usage unavailable; applying fail-closed policy");
            None
        }
    };
    Ok(decide_capacity(&state.config.policy, usage))
}

#[utoipa::path(
    get,
    path = "/healthz",
    responses((status = 200, body = HealthResponse)),
    tag = "operations"
)]
async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(HealthResponse {
        ok: true,
        service: SERVICE,
        mutation_enabled: state.config.mutation_enabled,
        organization: state.config.organization.clone(),
    })
}

#[utoipa::path(
    get,
    path = "/readyz",
    responses((status = 200, body = HealthResponse), (status = 503, body = HealthResponse)),
    tag = "operations"
)]
async fn readyz(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let key_exists = tokio::fs::metadata(&state.config.github_app_private_key_path)
        .await
        .is_ok();
    let ok = key_exists && !state.config.operator_secret.is_empty();
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(HealthResponse {
            ok,
            service: SERVICE,
            mutation_enabled: state.config.mutation_enabled,
            organization: state.config.organization.clone(),
        }),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/capabilities",
    responses((status = 200, body = CapabilityResponse)),
    tag = "capacity"
)]
async fn capabilities(State(state): State<AppState>) -> Json<CapabilityResponse> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(CapabilityResponse {
        service: SERVICE,
        github_actions_protocol: "delegated-to-official-arc",
        control_plane_clone: false,
        arbitrary_command_execution: false,
        supported_modes: vec!["hosted", "self-hosted", "build-server", "hold"],
        variable_names: vec!["CI_EXECUTION_MODE", "CI_LINUX_RUNS_ON_JSON"],
        notes: vec![
            "ARC preserves normal GitHub Actions workflow and action compatibility on Linux",
            "build-server mode is advisory and accepts only separately reviewed profiles",
            "macOS, Windows, Android/KVM, and public-fork workloads require separate lanes",
        ],
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/organizations/{org}/decision",
    params(("org" = String, Path, description = "GitHub organization")),
    responses(
        (status = 200, body = OrganizationDecisionResponse),
        (status = 404, description = "No policy configured")
    ),
    tag = "capacity"
)]
async fn organization_decision(
    State(state): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Result<Json<OrganizationDecisionResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if !authorized(&headers, &state.config.operator_secret) {
        return Err(ApiError::unauthorized());
    }
    let decision = decision_for_org(&state, &org).await?;
    Ok(Json(OrganizationDecisionResponse {
        organization: org,
        decision,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/organizations/{org}/reconcile",
    params(("org" = String, Path, description = "GitHub organization")),
    responses(
        (status = 200, body = ReconcileResponse),
        (status = 401, description = "Operator auth required"),
        (status = 404, description = "No policy configured"),
        (status = 502, description = "GitHub API failure")
    ),
    tag = "capacity"
)]
async fn reconcile_organization(
    State(state): State<AppState>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ReconcileResponse>, ApiError> {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if !authorized(&headers, &state.config.operator_secret) {
        return Err(ApiError::unauthorized());
    }
    let decision = decision_for_org(&state, &org).await?;
    let variables =
        decision_variables(&state.config.policy, &decision).map_err(ApiError::internal)?;
    state
        .metrics
        .reconcile_total
        .fetch_add(1, Ordering::Relaxed);
    if state.config.mutation_enabled {
        for value in variables.values() {
            if let Err(error) = state.github.upsert_variable(&org, value).await {
                state
                    .metrics
                    .reconcile_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
            state
                .metrics
                .variable_mutations_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(Json(ReconcileResponse {
        organization: org,
        dry_run: !state.config.mutation_enabled,
        decision,
        variables,
    }))
}

#[utoipa::path(
    get,
    path = "/metrics",
    responses((status = 200, description = "Prometheus metrics")),
    tag = "operations"
)]
async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = format!(
        concat!(
            "# HELP gha_clone_server_build_info Build metadata.\n",
            "# TYPE gha_clone_server_build_info gauge\n",
            "gha_clone_server_build_info{{service=\"gha-clone-server\"}} 1\n",
            "# HELP gha_clone_server_http_requests_total HTTP requests.\n",
            "# TYPE gha_clone_server_http_requests_total counter\n",
            "gha_clone_server_http_requests_total {}\n",
            "# HELP gha_clone_server_billing_reads_total Billing reads attempted.\n",
            "# TYPE gha_clone_server_billing_reads_total counter\n",
            "gha_clone_server_billing_reads_total {}\n",
            "# HELP gha_clone_server_billing_read_failures_total Billing read failures.\n",
            "# TYPE gha_clone_server_billing_read_failures_total counter\n",
            "gha_clone_server_billing_read_failures_total {}\n",
            "# HELP gha_clone_server_reconcile_total Reconciliations attempted.\n",
            "# TYPE gha_clone_server_reconcile_total counter\n",
            "gha_clone_server_reconcile_total {}\n",
            "# HELP gha_clone_server_reconcile_failures_total Reconciliation failures.\n",
            "# TYPE gha_clone_server_reconcile_failures_total counter\n",
            "gha_clone_server_reconcile_failures_total {}\n",
            "# HELP gha_clone_server_variable_mutations_total GitHub Actions variables mutated.\n",
            "# TYPE gha_clone_server_variable_mutations_total counter\n",
            "gha_clone_server_variable_mutations_total {}\n"
        ),
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.billing_reads_total.load(Ordering::Relaxed),
        state
            .metrics
            .billing_read_failures_total
            .load(Ordering::Relaxed),
        state.metrics.reconcile_total.load(Ordering::Relaxed),
        state
            .metrics
            .reconcile_failures_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .variable_mutations_total
            .load(Ordering::Relaxed),
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

async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

async fn docs_html() -> Html<&'static str> {
    Html(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>gha-clone-server API</title></head><body><h1>gha-clone-server API</h1><p>This service routes CI capacity; official ARC executes GitHub Actions jobs.</p><p><a href="/api/docs.json">OpenAPI 3.1 JSON</a></p></body></html>"#,
    )
}

async fn reconcile_all(state: AppState) {
    if !state.config.mutation_enabled {
        return;
    }
    let org = state.config.organization.clone();
    match decision_for_org(&state, &org).await {
        Ok(decision) => {
            info!(organization = org, mode = ?decision.mode, "capacity decision");
            match decision_variables(&state.config.policy, &decision) {
                Ok(variables) => {
                    for value in variables.values() {
                        if let Err(error) = state.github.upsert_variable(&org, value).await {
                            state
                                .metrics
                                .reconcile_failures_total
                                .fetch_add(1, Ordering::Relaxed);
                            error!(organization = org, error = %error.message, "variable reconciliation failed");
                            break;
                        }
                        state
                            .metrics
                            .variable_mutations_total
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(message) => {
                    state
                        .metrics
                        .reconcile_failures_total
                        .fetch_add(1, Ordering::Relaxed);
                    error!(
                        organization = org,
                        error = message,
                        "variable reconciliation blocked"
                    );
                }
            }
        }
        Err(error) => {
            state
                .metrics
                .reconcile_failures_total
                .fetch_add(1, Ordering::Relaxed);
            error!(organization = org, error = %error.message, "capacity reconciliation failed");
        }
    }
    state
        .metrics
        .reconcile_total
        .fetch_add(1, Ordering::Relaxed);
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/api/v1/capabilities", get(capabilities))
        .route(
            "/api/v1/organizations/:org/decision",
            get(organization_decision),
        )
        .route(
            "/api/v1/organizations/:org/reconcile",
            post(reconcile_organization),
        )
        .route("/api/docs.json", get(openapi_json))
        .route("/api/docs", get(docs_html))
        .route("/docs/api", get(docs_html))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gha_clone_server=info,info".into()),
        )
        .json()
        .init();

    let config = Config::from_env()?;
    let github = GitHubClient::new(config.clone())?;
    let state = AppState {
        config: config.clone(),
        github,
        metrics: Arc::new(Metrics::default()),
    };

    if config.mutation_enabled {
        let worker_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(worker_state.config.reconcile_interval);
            loop {
                interval.tick().await;
                reconcile_all(worker_state.clone()).await;
            }
        });
    }

    let address: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|error| format!("invalid listen address: {error}"))?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("failed to bind {address}: {error}"))?;
    info!(
        %address,
        organization = config.organization,
        mutation_enabled = config.mutation_enabled,
        "gha-clone-server listening"
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|error| format!("server failed: {error}"))
}
