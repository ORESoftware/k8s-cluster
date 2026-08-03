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
use gha_capacity_broker::{
    decide_capacity, decision_variables, BillingUsageResponse, CapacityDecision, OrgPolicy,
    VariableMutation,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use utoipa::{OpenApi, ToSchema};

const SERVICE: &str = "gha-capacity-broker";
const GITHUB_API_VERSION: &str = "2026-03-10";
const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
const DEFAULT_PORT: u16 = 8117;

#[derive(Clone, Debug)]
struct GitHubAppCredentials {
    app_id: String,
    installation_id: u64,
    private_key_path: PathBuf,
}

impl GitHubAppCredentials {
    fn from_env(prefix: &str) -> Result<Self, String> {
        let app_id_key = format!("{prefix}_ID");
        let installation_id_key = format!("{prefix}_INSTALLATION_ID");
        let private_key_path_key = format!("{prefix}_PRIVATE_KEY_PATH");
        Ok(Self {
            app_id: required_env(&app_id_key)?,
            installation_id: required_positive_u64(&installation_id_key)?,
            private_key_path: PathBuf::from(required_env(&private_key_path_key)?),
        })
    }
}

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    github_api_base: String,
    mutation_app: GitHubAppCredentials,
    billing_app: GitHubAppCredentials,
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

        let mutation_app = GitHubAppCredentials::from_env("GITHUB_MUTATION_APP")?;
        let billing_app = GitHubAppCredentials::from_env("GITHUB_BILLING_APP")?;
        validate_app_separation(&mutation_app, &billing_app)?;

        Ok(Self {
            host: optional_env("HOST").unwrap_or_else(|| "0.0.0.0".to_string()),
            port: optional_env("PORT")
                .map(|value| value.parse::<u16>())
                .transpose()
                .map_err(|error| format!("invalid PORT: {error}"))?
                .unwrap_or(DEFAULT_PORT),
            github_api_base,
            mutation_app,
            billing_app,
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
    api_base: String,
    http: reqwest::Client,
    billing_auth: GitHubAppAuth,
    mutation_auth: GitHubAppAuth,
}

#[derive(Clone)]
struct GitHubAppAuth {
    credentials: GitHubAppCredentials,
    installation_token_cache: Arc<Mutex<Option<CachedInstallationToken>>>,
}

impl GitHubAppAuth {
    fn new(credentials: GitHubAppCredentials) -> Self {
        Self {
            credentials,
            installation_token_cache: Arc::new(Mutex::new(None)),
        }
    }

    async fn installation_token(
        &self,
        http: &reqwest::Client,
        api_base: &str,
    ) -> Result<String, ApiError> {
        let mut guard = self.installation_token_cache.lock().await;
        if let Some(cached) = guard.as_ref() {
            if Instant::now() < cached.refresh_at {
                return Ok(cached.token.clone());
            }
        }

        let private_key = tokio::fs::read(&self.credentials.private_key_path)
            .await
            .map_err(|error| {
                ApiError::internal(format!(
                    "failed to read GitHub App private key from {}: {error}",
                    self.credentials.private_key_path.display()
                ))
            })?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let claims = AppJwtClaims {
            iat: now.saturating_sub(60),
            exp: now.saturating_add(540),
            iss: &self.credentials.app_id,
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
            "{api_base}/app/installations/{}/access_tokens",
            self.credentials.installation_id
        );
        let response = http
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
                github_error_summary(&body)
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
    billing_app_configured: bool,
    mutation_app_configured: bool,
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

fn required_positive_u64(key: &str) -> Result<u64, String> {
    required_env(key)?
        .parse::<u64>()
        .map_err(|error| format!("invalid {key}: {error}"))
        .and_then(|value| {
            if value == 0 {
                Err(format!("{key} must be positive"))
            } else {
                Ok(value)
            }
        })
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

fn validate_app_separation(
    mutation: &GitHubAppCredentials,
    billing: &GitHubAppCredentials,
) -> Result<(), String> {
    if mutation.private_key_path == billing.private_key_path {
        return Err(
            "billing and mutation GitHub Apps must use distinct private-key files".to_string(),
        );
    }
    if mutation.app_id == billing.app_id && mutation.installation_id == billing.installation_id {
        return Err(
            "billing and mutation GitHub Apps must use distinct App installations".to_string(),
        );
    }
    Ok(())
}

fn billing_usage_url(api_base: &str, org: &str) -> String {
    format!("{api_base}/organizations/{org}/settings/billing/usage/summary")
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
    fn new(config: &Config) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(RedirectPolicy::limited(3))
            .user_agent("oresoftware-gha-capacity-broker/0.1")
            .build()
            .map_err(|error| format!("failed to build GitHub HTTP client: {error}"))?;
        Ok(Self {
            api_base: config.github_api_base.clone(),
            http,
            billing_auth: GitHubAppAuth::new(config.billing_app.clone()),
            mutation_auth: GitHubAppAuth::new(config.mutation_app.clone()),
        })
    }

    async fn billing_usage(&self, org: &str) -> Result<BillingUsageResponse, ApiError> {
        validate_org(org).map_err(ApiError::not_found)?;
        let token = self
            .billing_auth
            .installation_token(&self.http, &self.api_base)
            .await?;
        let url = billing_usage_url(&self.api_base, org);
        let now = time::OffsetDateTime::now_utc();
        let period = [
            ("year", now.year().to_string()),
            ("month", (now.month() as u8).to_string()),
            ("product", "Actions".to_string()),
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
                github_error_summary(&body)
            )));
        }
        serde_json::from_str(&body).map_err(|error| {
            ApiError::bad_gateway(format!("invalid GitHub billing response: {error}"))
        })
    }

    async fn upsert_variable(&self, org: &str, value: &VariableMutation) -> Result<(), ApiError> {
        let token = self
            .mutation_auth
            .installation_token(&self.http, &self.api_base)
            .await?;
        let patch_url = format!(
            "{}/orgs/{org}/actions/variables/{}",
            self.api_base, value.name
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
                github_error_summary(&response_body)
            )));
        }

        let create_url = format!("{}/orgs/{org}/actions/variables", self.api_base);
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
            github_error_summary(&response_body)
        )))
    }
}

fn github_error_summary(body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "GitHub API returned a non-success response".to_string());
    message
        .chars()
        .filter(|ch| !ch.is_control())
        .take(200)
        .collect()
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
        Ok(usage) => {
            let gross_minutes = usage.actions_gross_minutes();
            let billable_minutes = usage.actions_billable_minutes();
            info!(
                organization = org,
                gross_actions_minutes = gross_minutes,
                billable_actions_minutes = billable_minutes,
                "current-month Actions billing summary"
            );
            Some(gross_minutes)
        }
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
        billing_app_configured: true,
        mutation_app_configured: true,
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
    let mutation_key_exists = tokio::fs::metadata(&state.config.mutation_app.private_key_path)
        .await
        .is_ok();
    let billing_key_exists = tokio::fs::metadata(&state.config.billing_app.private_key_path)
        .await
        .is_ok();
    let ok = mutation_key_exists && billing_key_exists && !state.config.operator_secret.is_empty();
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
            billing_app_configured: billing_key_exists,
            mutation_app_configured: mutation_key_exists,
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
            "billing reads and organization-variable mutation use separate least-privilege GitHub Apps",
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
            "# HELP gha_capacity_broker_build_info Build metadata.\n",
            "# TYPE gha_capacity_broker_build_info gauge\n",
            "gha_capacity_broker_build_info{{service=\"gha-capacity-broker\"}} 1\n",
            "# HELP gha_capacity_broker_http_requests_total HTTP requests.\n",
            "# TYPE gha_capacity_broker_http_requests_total counter\n",
            "gha_capacity_broker_http_requests_total {}\n",
            "# HELP gha_capacity_broker_billing_reads_total Billing reads attempted.\n",
            "# TYPE gha_capacity_broker_billing_reads_total counter\n",
            "gha_capacity_broker_billing_reads_total {}\n",
            "# HELP gha_capacity_broker_billing_read_failures_total Billing read failures.\n",
            "# TYPE gha_capacity_broker_billing_read_failures_total counter\n",
            "gha_capacity_broker_billing_read_failures_total {}\n",
            "# HELP gha_capacity_broker_reconcile_total Reconciliations attempted.\n",
            "# TYPE gha_capacity_broker_reconcile_total counter\n",
            "gha_capacity_broker_reconcile_total {}\n",
            "# HELP gha_capacity_broker_reconcile_failures_total Reconciliation failures.\n",
            "# TYPE gha_capacity_broker_reconcile_failures_total counter\n",
            "gha_capacity_broker_reconcile_failures_total {}\n",
            "# HELP gha_capacity_broker_variable_mutations_total GitHub Actions variables mutated.\n",
            "# TYPE gha_capacity_broker_variable_mutations_total counter\n",
            "gha_capacity_broker_variable_mutations_total {}\n"
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
        r#"<!doctype html><html><head><meta charset="utf-8"><title>gha-capacity-broker API</title></head><body><h1>gha-capacity-broker API</h1><p>This service routes CI capacity; official ARC executes GitHub Actions jobs.</p><p><a href="/api/docs.json">OpenAPI 3.1 JSON</a></p></body></html>"#,
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
                .unwrap_or_else(|_| "gha_capacity_broker=info,info".into()),
        )
        .json()
        .init();

    let config = Config::from_env()?;
    let github = GitHubClient::new(&config)?;
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
        "gha-capacity-broker listening"
    );
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|error| format!("server failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(app_id: &str, installation_id: u64, path: &str) -> GitHubAppCredentials {
        GitHubAppCredentials {
            app_id: app_id.to_string(),
            installation_id,
            private_key_path: PathBuf::from(path),
        }
    }

    #[test]
    fn billing_url_uses_current_summary_endpoint() {
        assert_eq!(
            billing_usage_url("https://api.github.com", "sonus-auris"),
            "https://api.github.com/organizations/sonus-auris/settings/billing/usage/summary"
        );
    }

    #[test]
    fn billing_and_mutation_apps_must_be_distinct() {
        let mutation = app("mutation-app", 10, "/var/run/gha-mutation-app/key.pem");
        let billing = app("billing-app", 20, "/var/run/gha-billing-app/key.pem");
        assert!(validate_app_separation(&mutation, &billing).is_ok());

        let shared_path = app("billing-app", 20, "/var/run/gha-mutation-app/key.pem");
        assert!(validate_app_separation(&mutation, &shared_path).is_err());

        let shared_identity = app("mutation-app", 10, "/var/run/other/key.pem");
        assert!(validate_app_separation(&mutation, &shared_identity).is_err());
    }

    #[test]
    fn github_error_summary_omits_non_message_fields() {
        let body = r#"{"message":"Bad credentials","token":"do-not-echo"}"#;
        let summary = github_error_summary(body);
        assert_eq!(summary, "Bad credentials");
        assert!(!summary.contains("do-not-echo"));
    }

    #[test]
    fn github_error_summary_is_bounded_and_control_free() {
        let message = format!("line-one\n{}", "x".repeat(300));
        let body = serde_json::to_string(&json!({"message": message})).expect("JSON");
        let summary = github_error_summary(&body);
        assert!(summary.len() <= 200);
        assert!(!summary.contains('\n'));
    }
}
