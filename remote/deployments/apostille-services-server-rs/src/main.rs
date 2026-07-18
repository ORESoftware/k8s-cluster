use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::signal;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::{error, info};
use uuid::Uuid;

const SERVICE_NAME: &str = "dd-apostille-services-server";
const SCHEMA_VERSION: &str = "apostille.services.v1";
const DEFAULT_PORT: u16 = 8122;
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_CASES: usize = 2_000;
const MAX_WEBHOOK_EVENTS_PER_CASE: usize = 128;
const MAX_SUBMISSIONS_PER_CASE: usize = 32;
const MAX_DOCUMENTS_PER_CASE: usize = 32;
const MAX_TEXT_LEN: usize = 32_000;
const MAX_SHORT_TEXT_LEN: usize = 512;
const MAX_TOKEN_LEN: usize = 160;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    store: Arc<RwLock<CaseStore>>,
    metrics: Arc<Metrics>,
}

struct Config {
    bind_addr: String,
    port: u16,
    server_auth_secret: Option<String>,
    webhook_secret: Option<String>,
    allow_unauthenticated: bool,
    allow_unauthenticated_webhooks: bool,
    allow_private_provider_urls: bool,
    default_target_language: String,
    provider_configs: BTreeMap<String, GovernmentProvider>,
    translation_provider: Option<TranslationProvider>,
}

#[derive(Clone)]
struct GovernmentProvider {
    slug: String,
    base_url: Url,
    submit_path: String,
    status_path: Option<String>,
    enabled: bool,
    services: BTreeSet<String>,
    auth: Option<ProviderAuth>,
}

#[derive(Clone)]
struct ProviderAuth {
    kind: String,
    header_name: String,
    token: String,
}

#[derive(Clone)]
struct TranslationProvider {
    base_url: Url,
    path: String,
    auth_header: Option<String>,
    token: Option<String>,
}

#[derive(Default)]
struct CaseStore {
    cases: BTreeMap<String, ServiceCase>,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    cases_created_total: AtomicU64,
    submit_attempts_total: AtomicU64,
    submit_success_total: AtomicU64,
    translations_total: AtomicU64,
    translation_provider_errors_total: AtomicU64,
    government_webhooks_total: AtomicU64,
    auth_failures_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Jurisdiction {
    slug: String,
    display_name: String,
    region: String,
    aliases: Vec<String>,
    primary_languages: Vec<String>,
    services: Vec<String>,
    interop_level: String,
    notes: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGovernmentProvider {
    base_url: String,
    submit_path: Option<String>,
    status_path: Option<String>,
    enabled: Option<bool>,
    services: Option<Vec<String>>,
    auth_kind: Option<String>,
    auth_header: Option<String>,
    token_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaseRequest {
    request_id: Option<String>,
    service_type: String,
    jurisdiction: String,
    customer_reference: Option<String>,
    source_language: Option<String>,
    target_language: Option<String>,
    applicant: Applicant,
    documents: Vec<DocumentInput>,
    workflow: Option<WorkflowOptions>,
    consent: Option<ConsentAttestation>,
    metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Applicant {
    full_name: String,
    email: Option<String>,
    phone: Option<String>,
    nationality: Option<String>,
    date_of_birth: Option<String>,
    passport_country: Option<String>,
    address_country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentInput {
    document_id: Option<String>,
    kind: String,
    title: Option<String>,
    issuing_country: Option<String>,
    issuing_authority: Option<String>,
    source_language: Option<String>,
    text: Option<String>,
    file_url: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowOptions {
    auto_submit: Option<bool>,
    expedite: Option<bool>,
    notify_webhook_url: Option<String>,
    desired_outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsentAttestation {
    authorized_representative: Option<bool>,
    self_filed: Option<bool>,
    data_use_accepted: Option<bool>,
    government_terms_accepted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceCase {
    case_id: String,
    request_id: String,
    customer_reference: Option<String>,
    status: String,
    service_type: String,
    jurisdiction: Jurisdiction,
    applicant: Applicant,
    documents: Vec<DocumentRecord>,
    target_language: String,
    translations: Vec<DocumentTranslation>,
    government_submissions: Vec<GovernmentSubmission>,
    webhook_events: Vec<GovernmentWebhookEvent>,
    consent: Option<ConsentAttestation>,
    warnings: Vec<String>,
    metadata: Option<Value>,
    created_at_ms: u128,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentRecord {
    document_id: String,
    kind: String,
    title: Option<String>,
    issuing_country: Option<String>,
    issuing_authority: Option<String>,
    source_language: String,
    text: Option<String>,
    file_url: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentTranslation {
    document_id: String,
    field: String,
    source_language: String,
    target_language: String,
    status: String,
    provider: String,
    original_text: String,
    translated_text: Option<String>,
    translated_at_ms: u128,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GovernmentSubmission {
    submission_id: String,
    jurisdiction: String,
    service_type: String,
    provider: String,
    status: String,
    provider_reference: Option<String>,
    submitted_at_ms: u128,
    response_status: Option<u16>,
    response_summary: Option<Value>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GovernmentWebhookPayload {
    request_id: Option<String>,
    case_id: Option<String>,
    jurisdiction: Option<String>,
    provider_reference: Option<String>,
    event_type: Option<String>,
    status: Option<String>,
    payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GovernmentWebhookEvent {
    event_id: String,
    request_id: String,
    jurisdiction: Option<String>,
    provider_reference: Option<String>,
    event_type: String,
    status: String,
    received_at_ms: u128,
    payload_shape: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslateRequest {
    request_id: Option<String>,
    text: String,
    source_language: Option<String>,
    target_language: Option<String>,
    context: Option<String>,
}

enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init(SERVICE_NAME);
    let config = config_from_env().map_err(|message| {
        error!(error = %message, "apostille service configuration failed");
        std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
    })?;
    let bind: SocketAddr = format!("{}:{}", config.bind_addr, config.port).parse()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .user_agent(format!("{SERVICE_NAME}/0.1"))
        .build()?;
    let state = AppState {
        config: Arc::new(config),
        http,
        store: Arc::new(RwLock::new(CaseStore::default())),
        metrics: Arc::new(Metrics::default()),
    };

    let app = Router::new()
        .route("/", get(home))
        .route("/home", get(home))
        .route("/home/flow", get(home_flow_fragment))
        .route("/home/jurisdictions", get(home_jurisdictions_fragment))
        .route("/home/webhooks", get(home_webhooks_fragment))
        .route("/home/examples", get(home_examples_fragment))
        .route("/descriptor", get(descriptor))
        .route("/jurisdictions", get(jurisdictions))
        .route("/services", get(services))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/cases", get(list_cases).post(create_case))
        .route("/cases/:case_id", get(get_case))
        .route("/cases/:case_id/submit", post(submit_case))
        .route("/translate", post(translate))
        .route("/webhooks/government", post(government_webhook_default))
        .route(
            "/webhooks/government/:jurisdiction",
            post(government_webhook_for_jurisdiction),
        )
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_HTTP_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(
        addr = %bind,
        providers = state.config.provider_configs.len(),
        translation_provider = state.config.translation_provider.is_some(),
        "apostille services server listening"
    );
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("apostille services server shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => info!("received SIGINT, beginning graceful shutdown"),
        _ = terminate => info!("received SIGTERM, beginning graceful shutdown"),
    }
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn env_u16(key: &str, fallback: u16) -> u16 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn config_from_env() -> Result<Config, String> {
    let allow_private_provider_urls = env_bool("APOSTILLE_ALLOW_PRIVATE_PROVIDER_URLS", false);
    let provider_configs = parse_provider_configs(allow_private_provider_urls)?;
    let translation_provider = parse_translation_provider(allow_private_provider_urls)?;
    Ok(Config {
        bind_addr: env_value("HOST", &env_value("BIND_ADDR", "0.0.0.0")),
        port: env_u16("PORT", DEFAULT_PORT),
        server_auth_secret: optional_env("SERVER_AUTH_SECRET")
            .or_else(|| optional_env("APOSTILLE_SERVER_AUTH_SECRET")),
        webhook_secret: optional_env("APOSTILLE_WEBHOOK_SECRET")
            .or_else(|| optional_env("GOVERNMENT_WEBHOOK_SECRET")),
        allow_unauthenticated: env_bool("APOSTILLE_ALLOW_UNAUTHENTICATED", false),
        allow_unauthenticated_webhooks: env_bool("APOSTILLE_ALLOW_UNAUTHENTICATED_WEBHOOKS", false),
        allow_private_provider_urls,
        default_target_language: env_value("APOSTILLE_DEFAULT_TARGET_LANGUAGE", "en"),
        provider_configs,
        translation_provider,
    })
}

fn parse_provider_configs(
    allow_private_provider_urls: bool,
) -> Result<BTreeMap<String, GovernmentProvider>, String> {
    let mut providers = BTreeMap::new();
    if let Some(raw_json) = optional_env("APOSTILLE_PROVIDER_CONFIG_JSON") {
        let raw: BTreeMap<String, RawGovernmentProvider> = serde_json::from_str(&raw_json)
            .map_err(|error| {
                format!("APOSTILLE_PROVIDER_CONFIG_JSON must be a JSON object: {error}")
            })?;
        for (slug, provider) in raw {
            let provider = build_provider(&slug, provider, allow_private_provider_urls)?;
            providers.insert(provider.slug.clone(), provider);
        }
    }
    for jurisdiction in jurisdiction_catalog() {
        let env_key = env_slug(&jurisdiction.slug);
        let base_var = format!("APOSTILLE_PROVIDER_{env_key}_BASE_URL");
        if let Some(base_url) = optional_env(&base_var) {
            let raw = RawGovernmentProvider {
                base_url,
                submit_path: optional_env(&format!("APOSTILLE_PROVIDER_{env_key}_SUBMIT_PATH")),
                status_path: optional_env(&format!("APOSTILLE_PROVIDER_{env_key}_STATUS_PATH")),
                enabled: Some(env_bool(
                    &format!("APOSTILLE_PROVIDER_{env_key}_ENABLED"),
                    true,
                )),
                services: optional_env(&format!("APOSTILLE_PROVIDER_{env_key}_SERVICES")).map(
                    |value| {
                        value
                            .split(',')
                            .map(|item| item.trim().to_string())
                            .collect()
                    },
                ),
                auth_kind: optional_env(&format!("APOSTILLE_PROVIDER_{env_key}_AUTH_KIND")),
                auth_header: optional_env(&format!("APOSTILLE_PROVIDER_{env_key}_AUTH_HEADER")),
                token_env: Some(format!("APOSTILLE_PROVIDER_{env_key}_TOKEN")),
            };
            let provider = build_provider(&jurisdiction.slug, raw, allow_private_provider_urls)?;
            providers.insert(provider.slug.clone(), provider);
        }
    }
    Ok(providers)
}

fn build_provider(
    slug: &str,
    raw: RawGovernmentProvider,
    allow_private_provider_urls: bool,
) -> Result<GovernmentProvider, String> {
    let slug = canonical_slug(slug);
    let base_url = validate_outbound_url(&raw.base_url, allow_private_provider_urls)?;
    let submit_path = raw.submit_path.unwrap_or_else(|| "/submit".to_string());
    let token_env = raw
        .token_env
        .unwrap_or_else(|| format!("APOSTILLE_PROVIDER_{}_TOKEN", env_slug(&slug)));
    let token = optional_env(&token_env);
    let auth = token.map(|token| {
        let kind = raw.auth_kind.unwrap_or_else(|| "bearer".to_string());
        let header_name = raw.auth_header.unwrap_or_else(|| match kind.as_str() {
            "api-key" | "apikey" => "x-api-key".to_string(),
            "header" => "x-provider-auth".to_string(),
            _ => "authorization".to_string(),
        });
        ProviderAuth {
            kind,
            header_name,
            token,
        }
    });
    let services = raw
        .services
        .unwrap_or_else(|| {
            service_catalog()
                .into_iter()
                .map(|service| service.slug)
                .collect()
        })
        .into_iter()
        .map(|service| clean_service_slug(&service))
        .collect::<BTreeSet<_>>();
    Ok(GovernmentProvider {
        slug,
        base_url,
        submit_path,
        status_path: raw.status_path,
        enabled: raw.enabled.unwrap_or(true),
        services,
        auth,
    })
}

fn parse_translation_provider(
    allow_private_provider_urls: bool,
) -> Result<Option<TranslationProvider>, String> {
    let Some(base_url) = optional_env("APOSTILLE_TRANSLATION_BASE_URL") else {
        return Ok(None);
    };
    let base_url = validate_outbound_url(&base_url, allow_private_provider_urls)?;
    Ok(Some(TranslationProvider {
        base_url,
        path: env_value("APOSTILLE_TRANSLATION_PATH", "/translate"),
        auth_header: optional_env("APOSTILLE_TRANSLATION_AUTH_HEADER")
            .or_else(|| Some("authorization".to_string())),
        token: optional_env("APOSTILLE_TRANSLATION_AUTH_TOKEN"),
    }))
}

fn validate_outbound_url(raw: &str, allow_private: bool) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid provider url {raw}: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("provider url scheme must be http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("provider urls must not include credentials".to_string());
    }
    if !allow_private {
        if let Some(host) = url.host_str() {
            if blocked_host(host) {
                return Err(format!(
                    "provider url host {host} is private/local; set APOSTILLE_ALLOW_PRIVATE_PROVIDER_URLS=true only for approved private network integrations"
                ));
            }
        }
    }
    Ok(url)
}

fn blocked_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(addr)) => {
            addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_broadcast()
                || addr.is_documentation()
                || addr.is_unspecified()
        }
        Ok(IpAddr::V6(addr)) => {
            addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_multicast()
        }
        Err(_) => false,
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<&String>, fallback: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn canonical_slug(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn env_slug(value: &str) -> String {
    canonical_slug(value).replace('-', "_").to_ascii_uppercase()
}

fn clean_service_slug(value: &str) -> String {
    canonical_slug(value)
}

fn clean_short(value: Option<&String>) -> Option<String> {
    value
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(MAX_SHORT_TEXT_LEN)
                .collect()
        })
}

fn clean_required(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_SHORT_TEXT_LEN {
        return Err(format!(
            "{label} must be at most {MAX_SHORT_TEXT_LEN} bytes"
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

fn clean_text(value: Option<&String>, max_len: usize) -> Option<String> {
    value
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(max_len)
                .collect()
        })
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated {
        return Ok(());
    }
    let Some(secret) = state.config.server_auth_secret.as_ref() else {
        return Err(AuthFailure::MissingSecret);
    };
    let provided = headers
        .get("x-server-auth")
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if value == secret => Ok(()),
        _ => Err(AuthFailure::Unauthorized),
    }
}

fn require_webhook_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated_webhooks {
        return Ok(());
    }
    if let Some(secret) = state.config.webhook_secret.as_ref() {
        let provided = headers
            .get("x-apostille-webhook-secret")
            .or_else(|| headers.get("x-government-webhook-secret"))
            .or_else(|| headers.get("x-webhook-secret"))
            .and_then(|value| value.to_str().ok());
        return match provided {
            Some(value) if value == secret => Ok(()),
            _ => Err(AuthFailure::Unauthorized),
        };
    }
    require_auth(headers, state)
}

fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => "server auth secret is not configured",
        AuthFailure::Unauthorized => "unauthorized",
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": message })),
    )
        .into_response()
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "ok": false, "error": message.into() })),
    )
        .into_response()
}

fn jurisdiction_catalog() -> Vec<Jurisdiction> {
    vec![
        jurisdiction("peru", "Peru", "Latin America", &["pe"], &["es"], "Configured connector support for SUNARP, Migraciones, and ministry/notary workflows when approved credentials are supplied."),
        jurisdiction("colombia", "Colombia", "Latin America", &["co"], &["es"], "Configured connector support for Cancilleria, notary, registry, and migration workflows when approved credentials are supplied."),
        jurisdiction("usa", "United States", "North America", &["us", "united-states"], &["en"], "Configured connector support for state apostille offices, notary verification, USCIS-style case intake, and federal/state document workflows."),
        jurisdiction("mexico", "Mexico", "North America", &["mx"], &["es"], "Configured connector support for SRE/state apostille, notary, civil registry, and INM-style immigration workflows."),
        jurisdiction("europe", "Europe / EU", "Europe", &["eu", "european-union"], &["en", "fr", "es", "it", "hr"], "Regional adapter slot for EU-wide or multi-country providers; prefer country-specific connectors when available."),
        jurisdiction("spain", "Spain", "Europe", &["es"], &["es"], "Configured connector support for MAEC, civil registry, notary, and immigration appointment/status workflows."),
        jurisdiction("italy", "Italy", "Europe", &["it"], &["it"], "Configured connector support for Prefettura/Procura apostille, notary, consular, and immigration workflows."),
        jurisdiction("china", "China", "Asia", &["cn", "prc"], &["zh"], "Configured connector support for consular/legalization, notarial certificate, and visa/residency workflows."),
        jurisdiction("taiwan", "Taiwan", "Asia", &["tw", "roc"], &["zh"], "Configured connector support for BOCA/MOFA document authentication, notary, and residency workflows."),
        jurisdiction("thailand", "Thailand", "Asia", &["th"], &["th"], "Configured connector support for legalization, notarial services, and immigration/status workflows."),
        jurisdiction("cambodia", "Cambodia", "Asia", &["kh"], &["km"], "Configured connector support for ministry/legalization, notary, and immigration workflows."),
        jurisdiction("france", "France", "Europe", &["fr"], &["fr"], "Configured connector support for apostille/legalization, notarial, prefecture, and immigration workflows."),
        jurisdiction("croatia", "Croatia", "Europe", &["hr"], &["hr"], "Configured connector support for apostille, court/notary, civil registry, and residence workflows."),
        jurisdiction("laos", "Laos", "Asia", &["la", "lao"], &["lo"], "Configured connector support for legalization, notary, and immigration workflows."),
        jurisdiction("myanmar", "Myanmar / Burma", "Asia", &["mm", "burma"], &["my"], "Configured connector support for document legalization, notarial, and immigration workflows."),
        jurisdiction("vietnam", "Vietnam", "Asia", &["vn"], &["vi"], "Configured connector support for consular legalization, notary, civil registry, and immigration workflows."),
        jurisdiction("brazil", "Brazil", "Latin America", &["br", "brasil"], &["pt"], "Configured connector support for cartorio, e-notariado style providers, apostille, and immigration workflows."),
    ]
}

fn jurisdiction(
    slug: &str,
    display_name: &str,
    region: &str,
    aliases: &[&str],
    languages: &[&str],
    notes: &str,
) -> Jurisdiction {
    Jurisdiction {
        slug: slug.to_string(),
        display_name: display_name.to_string(),
        region: region.to_string(),
        aliases: aliases.iter().map(|value| value.to_string()).collect(),
        primary_languages: languages.iter().map(|value| value.to_string()).collect(),
        services: service_catalog()
            .into_iter()
            .map(|service| service.slug)
            .collect(),
        interop_level: "configurable-provider".to_string(),
        notes: notes.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceType {
    slug: String,
    display_name: String,
    notes: String,
}

fn service_catalog() -> Vec<ServiceType> {
    vec![
        ServiceType {
            slug: "apostille".to_string(),
            display_name: "Apostille and legalization".to_string(),
            notes: "Document authentication, legalization, consular routing, and Hague-apostille style workflows where supported.".to_string(),
        },
        ServiceType {
            slug: "notary".to_string(),
            display_name: "Notary services".to_string(),
            notes: "Notarial certificate intake, notary validation, remote notarization provider handoff, and registry evidence capture.".to_string(),
        },
        ServiceType {
            slug: "immigration".to_string(),
            display_name: "Immigration services".to_string(),
            notes: "Visa, residency, appointment, case-status, and supporting-document workflow orchestration through approved providers.".to_string(),
        },
    ]
}

fn canonical_jurisdiction(input: &str) -> Option<Jurisdiction> {
    let normalized = canonical_slug(input);
    jurisdiction_catalog().into_iter().find(|jurisdiction| {
        jurisdiction.slug == normalized
            || jurisdiction
                .aliases
                .iter()
                .any(|alias| canonical_slug(alias) == normalized)
    })
}

fn service_type(input: &str) -> Option<ServiceType> {
    let normalized = clean_service_slug(input);
    service_catalog()
        .into_iter()
        .find(|service| service.slug == normalized)
}

fn connector_status(state: &AppState) -> Vec<Value> {
    jurisdiction_catalog()
        .into_iter()
        .map(|jurisdiction| {
            let provider = state.config.provider_configs.get(&jurisdiction.slug);
            json!({
                "jurisdiction": jurisdiction.slug,
                "displayName": jurisdiction.display_name,
                "configured": provider.is_some(),
                "enabled": provider.map(|provider| provider.enabled).unwrap_or(false),
                "services": provider
                    .map(|provider| provider.services.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
                "statusPathConfigured": provider.and_then(|provider| provider.status_path.clone()).is_some(),
                "authConfigured": provider.and_then(|provider| provider.auth.as_ref()).is_some()
            })
        })
        .collect()
}

async fn home(State(state): State<AppState>) -> Html<String> {
    let connector_count = state.config.provider_configs.len();
    let translation = if state.config.translation_provider.is_some() {
        "configured"
    } else {
        "not configured"
    };
    Html(render_home(jurisdiction_catalog().len(), connector_count, translation).into_string())
}

/// The home page shell. maud compile-checks the structure and auto-escapes any
/// dynamic value, replacing the previous `HOME_HTML` string-replace template.
fn render_home(jurisdiction_count: usize, connector_count: usize, translation: &str) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Apostille, Notary, Immigration Services" }
                script src="https://unpkg.com/htmx.org@1.9.12" {}
                style { (PreEscaped(HOME_CSS)) }
            }
            body {
                header {
                    div class="wrap hero" {
                        span class="eyebrow" { "Rust Axum service" }
                        h1 { "Apostille, Notary, Immigration Services" }
                        p class="lead" {
                            "A normalized intake and orchestration server for document legalization, "
                            "notarial workflows, immigration support, English translation handoff, "
                            "government-provider submissions, and inbound status webhooks."
                        }
                        div class="stats" {
                            div class="stat" { strong { (jurisdiction_count) } span { "jurisdictions in the catalog" } }
                            div class="stat" { strong { (connector_count) } span { "provider connectors configured at boot" } }
                            div class="stat" { strong { (translation) } span { "translation provider" } }
                        }
                    }
                }
                nav {
                    div class="wrap tabs" {
                        button hx-get="/home/flow" hx-target="#home-panel" hx-swap="innerHTML" { "Workflow" }
                        button hx-get="/home/jurisdictions" hx-target="#home-panel" hx-swap="innerHTML" { "Jurisdictions" }
                        button hx-get="/home/webhooks" hx-target="#home-panel" hx-swap="innerHTML" { "Webhooks" }
                        button hx-get="/home/examples" hx-target="#home-panel" hx-swap="innerHTML" { "Payloads" }
                        button onclick="location.href='/docs/api'" { "API Docs" }
                    }
                }
                main class="wrap grid" {
                    section id="home-panel" hx-get="/home/flow" hx-trigger="load" hx-swap="innerHTML" {
                        p { "Loading workflow..." }
                    }
                    aside class="panel" {
                        h2 { "Runtime Contract" }
                        div class="aside-list" {
                            div {
                                strong { "Auth" }
                                p {
                                    "Operator routes accept " code { "X-Server-Auth" } " or legacy "
                                    code { "Auth" } ". Government callbacks use "
                                    code { "X-Apostille-Webhook-Secret" } "."
                                }
                            }
                            div {
                                strong { "Translation" }
                                p {
                                    "Every non-English document field is queued through the configured "
                                    "translation provider. Without a provider, the case is marked "
                                    "translation-pending."
                                }
                            }
                            div {
                                strong { "Interop" }
                                p {
                                    "Government API calls are made only for jurisdictions with explicit "
                                    "provider base URLs and credentials from environment or secret-backed "
                                    "JSON config."
                                }
                            }
                            div {
                                strong { "Storage" }
                                p {
                                    "This first slice keeps a bounded in-memory case ledger. Add a "
                                    "Postgres contract before storing legal case data durably."
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

const HOME_CSS: &str = r##":root { color-scheme: light; --bg:#f4f5f3; --ink:#17201a; --muted:#617067; --panel:#ffffff; --line:#d8ded7; --accent:#1f6f5b; --accent-2:#7b3f2a; --soft:#edf3ef; --code:#eef2f0; }
    * { box-sizing:border-box; }
    body { margin:0; background:var(--bg); color:var(--ink); font:15px/1.55 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    header { min-height:66vh; display:grid; align-items:end; background:linear-gradient(180deg, rgba(23,32,26,.18), rgba(244,245,243,.98)), url("https://images.unsplash.com/photo-1450101499163-c8848c66ca85?auto=format&fit=crop&w=1800&q=80") center/cover; }
    .wrap { width:min(1120px, calc(100% - 32px)); margin:0 auto; }
    .hero { padding:64px 0 36px; max-width:860px; }
    .eyebrow { display:inline-flex; background:rgba(255,255,255,.82); border:1px solid rgba(255,255,255,.95); color:#20302a; border-radius:6px; padding:5px 9px; font-size:12px; font-weight:700; text-transform:uppercase; letter-spacing:0; }
    h1 { margin:18px 0 12px; font-size:clamp(40px, 8vw, 82px); line-height:.95; letter-spacing:0; max-width:840px; }
    .lead { max-width:760px; color:#293730; font-size:18px; background:rgba(255,255,255,.72); border-left:4px solid var(--accent); padding:12px 14px; }
    .stats { display:flex; flex-wrap:wrap; gap:10px; margin-top:18px; }
    .stat { background:rgba(255,255,255,.84); border:1px solid rgba(255,255,255,.95); border-radius:6px; padding:8px 10px; min-width:160px; }
    .stat strong { display:block; font-size:22px; }
    nav { position:sticky; top:0; z-index:5; background:rgba(244,245,243,.94); backdrop-filter:blur(10px); border-bottom:1px solid var(--line); }
    .tabs { display:flex; gap:8px; overflow:auto; padding:10px 0; }
    button { border:1px solid var(--line); background:var(--panel); color:var(--ink); border-radius:6px; padding:8px 11px; font:inherit; font-weight:700; cursor:pointer; white-space:nowrap; }
    button:hover, button:focus { border-color:var(--accent); outline:none; }
    main { padding:28px 0 42px; }
    .grid { display:grid; grid-template-columns:1.1fr .9fr; gap:18px; align-items:start; }
    section, .panel { background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:18px; }
    h2 { margin:0 0 10px; font-size:24px; letter-spacing:0; }
    h3 { margin:16px 0 6px; font-size:17px; letter-spacing:0; }
    p { margin:0 0 10px; color:var(--muted); }
    code, pre { background:var(--code); border-radius:6px; }
    code { padding:2px 5px; overflow-wrap:anywhere; }
    pre { margin:10px 0 0; padding:12px; overflow:auto; border:1px solid var(--line); }
    table { width:100%; border-collapse:collapse; }
    th, td { text-align:left; border-bottom:1px solid var(--line); padding:9px 7px; vertical-align:top; }
    th { color:var(--muted); font-size:12px; text-transform:uppercase; letter-spacing:0; }
    .flow { display:grid; gap:10px; }
    .step { border-left:4px solid var(--accent); background:var(--soft); padding:10px 12px; border-radius:0 6px 6px 0; }
    .step strong { display:block; }
    .aside-list { display:grid; gap:10px; }
    .aside-list div { border:1px solid var(--line); border-radius:6px; padding:10px; }
    .muted { color:var(--muted); }
    @media (max-width:820px) {
      header { min-height:58vh; }
      .grid { grid-template-columns:1fr; }
      h1 { font-size:42px; }
      table, tbody, tr, td { display:block; width:100%; }
      thead { display:none; }
      tr { border-bottom:1px solid var(--line); padding:7px 0; }
      td { border-bottom:0; padding:4px 0; }
    }"##;

async fn home_flow_fragment() -> Html<String> {
    Html(
        html! {
            h2 { "How It Works" }
            div class="flow" {
                div class="step" { strong { "1. Intake" } span { "Clients post one normalized case to " code { "POST /cases" } " with service type, jurisdiction, applicant data, consent, and document metadata or text." } }
                div class="step" { strong { "2. Normalize" } span { "The server canonicalizes jurisdictions, service types, language codes, document ids, and public file URLs. It rejects unknown jurisdictions and unsafe outbound URLs." } }
                div class="step" { strong { "3. Translate To English" } span { "When source language differs from the configured target language, document titles and text are sent to " code { "APOSTILLE_TRANSLATION_BASE_URL" } ". If no translator is configured, the case remains translation-pending." } }
                div class="step" { strong { "4. Submit Through Approved Connectors" } span { code { "POST /cases/:case_id/submit" } ", or " code { "workflow.autoSubmit=true" } ", posts to the jurisdiction provider configured through " code { "APOSTILLE_PROVIDER_CONFIG_JSON" } " or per-country env vars." } }
                div class="step" { strong { "5. Receive Government Webhooks" } span { "Providers call " code { "POST /webhooks/government" } " or " code { "POST /webhooks/government/:jurisdiction" } ". The server appends status events and updates the matching case." } }
            }
        }
        .into_string(),
    )
}

async fn home_jurisdictions_fragment(State(state): State<AppState>) -> Html<String> {
    // maud auto-escapes every interpolated value, so the manual escape_html
    // calls (and their easy-to-forget failure mode) are gone.
    Html(
        html! {
            h2 { "Jurisdiction Coverage" }
            p { "The catalog covers the requested countries and regions. Each government API connector is opt-in, credential-backed, and configured by operators rather than hard-coded in source." }
            table {
                thead { tr { th { "Jurisdiction" } th { "Languages" } th { "Connector" } th { "Notes" } } }
                tbody {
                    @for jurisdiction in jurisdiction_catalog() {
                        @let configured = state
                            .config
                            .provider_configs
                            .get(&jurisdiction.slug)
                            .map(|provider| if provider.enabled { "configured" } else { "disabled" })
                            .unwrap_or("not configured");
                        tr {
                            td { strong { (jurisdiction.display_name) } div class="muted" { code { (jurisdiction.slug) } } }
                            td { (jurisdiction.primary_languages.join(", ")) }
                            td { (configured) }
                            td { (jurisdiction.notes) }
                        }
                    }
                }
            }
        }
        .into_string(),
    )
}

async fn home_webhooks_fragment() -> Html<String> {
    Html(
        html! {
            h2 { "Webhook Flow" }
            p { "Government agencies and approved intermediaries usually expose different callback formats. This service accepts one normalized envelope and stores only compact status metadata in the case ledger." }
            h3 { "Callback URL" }
            pre { code { "POST /webhooks/government\nPOST /webhooks/government/:jurisdiction" } }
            h3 { "Authentication" }
            p { "Set " code { "APOSTILLE_WEBHOOK_SECRET" } " and require providers to send one of these headers:" }
            pre { code { "X-Apostille-Webhook-Secret: ...\nX-Government-Webhook-Secret: ...\nX-Webhook-Secret: ..." } }
            h3 { "Envelope" }
            pre { code {
"{
  \"caseId\": \"case_...\",
  \"jurisdiction\": \"peru\",
  \"providerReference\": \"gov-reference-123\",
  \"eventType\": \"status.changed\",
  \"status\": \"approved\",
  \"payload\": {
    \"rawProviderStatus\": \"approved\",
    \"receivedAt\": \"2026-06-07T18:00:00Z\"
  }
}" } }
            p { "When " code { "caseId" } " is present, the case status becomes " code { "government_<status>" } ". Orphan callbacks are accepted and reported as unmatched so providers can retry with a corrected id." }
        }
        .into_string(),
    )
}

async fn home_examples_fragment() -> Html<String> {
    Html(
        html! {
            h2 { "Payload Examples" }
            h3 { "Create And Auto-Submit A Case" }
            pre { code {
r#"curl -sS http://127.0.0.1:8122/cases \
  -H 'content-type: application/json' \
  -H "X-Server-Auth: $SERVER_AUTH_SECRET" \
  -d '{
    "serviceType": "apostille",
    "jurisdiction": "peru",
    "sourceLanguage": "es",
    "targetLanguage": "en",
    "applicant": { "fullName": "Example Applicant", "email": "applicant@example.com" },
    "documents": [
      {
        "kind": "birth-certificate",
        "title": "Partida de nacimiento",
        "sourceLanguage": "es",
        "text": "Texto publico o extraido del documento..."
      }
    ],
    "workflow": { "autoSubmit": true, "expedite": false },
    "consent": {
      "authorizedRepresentative": true,
      "dataUseAccepted": true,
      "governmentTermsAccepted": true
    }
  }'"# } }
            h3 { "Provider Config Shape" }
            pre { code {
r#"{
  "peru": {
    "baseUrl": "https://approved-provider.example",
    "submitPath": "/apostille/submit",
    "statusPath": "/apostille/status",
    "services": ["apostille", "notary", "immigration"],
    "authKind": "bearer",
    "tokenEnv": "APOSTILLE_PROVIDER_PERU_TOKEN"
  }
}"# } }
        }
        .into_string(),
    )
}

#[cfg(test)]
mod maud_render_tests {
    use super::*;

    #[test]
    fn home_page_renders_htmx_shell_and_counts() {
        let html = render_home(7, 3, "configured").into_string();
        assert!(html.starts_with("<!DOCTYPE html>"));
        // HTMX wiring preserved verbatim from the original template.
        assert!(html.contains("src=\"https://unpkg.com/htmx.org@1.9.12\""));
        assert!(html.contains("hx-get=\"/home/flow\""));
        assert!(html.contains("hx-target=\"#home-panel\""));
        assert!(html.contains("hx-trigger=\"load\""));
        // Dynamic counts are interpolated into the stat tiles.
        assert!(html.contains("<strong>7</strong>"));
        assert!(html.contains("<strong>3</strong>"));
        assert!(html.contains("<strong>configured</strong>"));
        // CSS embedded verbatim via PreEscaped.
        assert!(html.contains("--accent:#1f6f5b"));
    }

    #[test]
    fn webhooks_fragment_auto_escapes_angle_brackets() {
        // The old hand-written template wrote `&lt;status&gt;` by hand; maud must
        // produce the same escaped output from the literal `government_<status>`.
        let html = tokio_test_block(home_webhooks_fragment());
        assert!(html.contains("government_&lt;status&gt;"));
        assert!(!html.contains("government_<status>"));
    }

    // Minimal executor so the async fragment can be rendered without pulling in
    // a full tokio runtime dependency for the test.
    fn tokio_test_block(fut: impl std::future::Future<Output = Html<String>>) -> String {
        use std::pin::pin;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn noop(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = pin!(fut);
        loop {
            if let Poll::Ready(Html(body)) = fut.as_mut().poll(&mut cx) {
                return body;
            }
        }
    }
}

async fn descriptor(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "description": "Apostille, notary, immigration, English-translation, government-provider submission, and status-webhook orchestration service.",
        "auth": {
            "operator": "X-Server-Auth or Auth",
            "governmentWebhook": "X-Apostille-Webhook-Secret, X-Government-Webhook-Secret, or X-Webhook-Secret",
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "allowUnauthenticatedWebhooks": state.config.allow_unauthenticated_webhooks
        },
        "translation": {
            "targetLanguage": state.config.default_target_language,
            "providerConfigured": state.config.translation_provider.is_some()
        },
        "connectorStatus": connector_status(&state),
        "endpoints": {
            "home": "GET /",
            "descriptor": "GET /descriptor",
            "jurisdictions": "GET /jurisdictions",
            "services": "GET /services",
            "schema": "GET /schema",
            "example": "GET /example",
            "cases": "GET /cases, POST /cases",
            "caseDetail": "GET /cases/:case_id",
            "submit": "POST /cases/:case_id/submit",
            "translate": "POST /translate",
            "governmentWebhook": "POST /webhooks/government, POST /webhooks/government/:jurisdiction",
            "docs": "GET /docs/api, GET /api/docs, GET /api/docs.json"
        }
    }))
}

async fn jurisdictions(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "jurisdictions": jurisdiction_catalog(),
        "connectorStatus": connector_status(&state)
    }))
}

async fn services() -> Json<Value> {
    Json(json!({ "ok": true, "services": service_catalog() }))
}

async fn schema() -> Json<Value> {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "caseRequest": {
            "serviceType": "apostille | notary | immigration",
            "jurisdiction": "peru | colombia | usa | mexico | europe | spain | italy | china | taiwan | thailand | cambodia | france | croatia | laos | myanmar | vietnam | brazil",
            "sourceLanguage": "optional BCP-47-ish language code; document-level sourceLanguage can override",
            "targetLanguage": "defaults to APOSTILLE_DEFAULT_TARGET_LANGUAGE, normally en",
            "applicant": "bounded applicant identity/contact object",
            "documents": "1-32 document metadata records with optional extracted text or public fileUrl",
            "workflow": "autoSubmit, expedite, notifyWebhookUrl, desiredOutcome",
            "consent": "authorizedRepresentative or selfFiled plus dataUseAccepted; governmentTermsAccepted needed before provider submit"
        },
        "governmentProviderConfig": {
            "envJson": "APOSTILLE_PROVIDER_CONFIG_JSON",
            "perJurisdictionEnv": "APOSTILLE_PROVIDER_<JURISDICTION>_BASE_URL plus optional *_SUBMIT_PATH, *_STATUS_PATH, *_TOKEN",
            "secretRule": "tokens come from env vars, not JSON literals committed to Git"
        },
        "webhook": {
            "paths": ["/webhooks/government", "/webhooks/government/:jurisdiction"],
            "headers": ["X-Apostille-Webhook-Secret", "X-Government-Webhook-Secret", "X-Webhook-Secret"],
            "payload": ["caseId", "jurisdiction", "providerReference", "eventType", "status", "payload"]
        }
    }))
}

async fn example() -> Json<Value> {
    Json(json!({
        "createCase": {
            "requestId": "demo-apostille-001",
            "serviceType": "apostille",
            "jurisdiction": "peru",
            "sourceLanguage": "es",
            "targetLanguage": "en",
            "customerReference": "customer-42",
            "applicant": {
                "fullName": "Example Applicant",
                "email": "applicant@example.com",
                "nationality": "PE"
            },
            "documents": [
                {
                    "documentId": "birth-cert-1",
                    "kind": "birth-certificate",
                    "title": "Partida de nacimiento",
                    "sourceLanguage": "es",
                    "text": "Texto extraido del documento..."
                }
            ],
            "workflow": {
                "autoSubmit": true,
                "expedite": false,
                "desiredOutcome": "apostille-for-us-use"
            },
            "consent": {
                "authorizedRepresentative": true,
                "dataUseAccepted": true,
                "governmentTermsAccepted": true
            }
        },
        "webhook": {
            "caseId": "case_...",
            "jurisdiction": "peru",
            "providerReference": "gov-123",
            "eventType": "status.changed",
            "status": "approved",
            "payload": { "rawProviderStatus": "approved" }
        }
    }))
}

async fn list_cases(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let cases = state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .cases
        .values()
        .cloned()
        .collect::<Vec<_>>();
    Json(json!({ "ok": true, "count": cases.len(), "cases": cases })).into_response()
}

async fn get_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let case = state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .cases
        .get(&case_id)
        .cloned();
    match case {
        Some(case) => Json(json!({ "ok": true, "case": case })).into_response(),
        None => json_error(StatusCode::NOT_FOUND, "case not found"),
    }
}

async fn create_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CaseRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    match build_service_case(&state, request).await {
        Ok((case, should_submit)) => {
            upsert_case(&state, case.clone());
            state
                .metrics
                .cases_created_total
                .fetch_add(1, Ordering::Relaxed);
            let case = if should_submit {
                let submission = submit_case_to_provider(&state, &case).await;
                append_submission(&state, &case.case_id, submission).unwrap_or(case)
            } else {
                case
            };
            Json(json!({ "ok": true, "case": case })).into_response()
        }
        Err(message) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(StatusCode::BAD_REQUEST, message)
        }
    }
}

async fn submit_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let Some(case) = state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .cases
        .get(&case_id)
        .cloned()
    else {
        return json_error(StatusCode::NOT_FOUND, "case not found");
    };
    if !submission_consent_ok(&case) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "case consent must include authorizedRepresentative or selfFiled, dataUseAccepted, and governmentTermsAccepted before submission",
        );
    }
    if case_has_pending_translations(&case) {
        return json_error(
            StatusCode::BAD_REQUEST,
            "case has pending translations; configure APOSTILLE_TRANSLATION_BASE_URL or submit only after translatedText is available",
        );
    }
    let submission = submit_case_to_provider(&state, &case).await;
    let case = append_submission(&state, &case_id, submission.clone()).unwrap_or(case);
    Json(json!({ "ok": true, "submission": submission, "case": case })).into_response()
}

async fn translate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TranslateRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let request_id = request_id(request.request_id.as_ref(), "translate");
    let source_language = request
        .source_language
        .unwrap_or_else(|| "auto".to_string())
        .chars()
        .take(32)
        .collect::<String>();
    let target_language = request
        .target_language
        .unwrap_or_else(|| state.config.default_target_language.clone())
        .chars()
        .take(32)
        .collect::<String>();
    let text = request.text.chars().take(MAX_TEXT_LEN).collect::<String>();
    let translation = translate_text(
        &state,
        &request_id,
        "ad-hoc",
        "text",
        &text,
        &source_language,
        &target_language,
        request.context.as_deref(),
    )
    .await;
    Json(json!({ "ok": true, "requestId": request_id, "translation": translation })).into_response()
}

async fn government_webhook_default(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GovernmentWebhookPayload>,
) -> Response {
    handle_government_webhook(state, headers, None, payload).await
}

async fn government_webhook_for_jurisdiction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(jurisdiction): Path<String>,
    Json(payload): Json<GovernmentWebhookPayload>,
) -> Response {
    handle_government_webhook(state, headers, Some(jurisdiction), payload).await
}

async fn handle_government_webhook(
    state: AppState,
    headers: HeaderMap,
    path_jurisdiction: Option<String>,
    payload: GovernmentWebhookPayload,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(failure) = require_webhook_auth(&headers, &state) {
        return auth_failure_response(&state, failure);
    }
    let jurisdiction = path_jurisdiction
        .or(payload.jurisdiction.clone())
        .map(|value| canonical_slug(&value));
    let request_id = request_id(payload.request_id.as_ref(), "government-webhook");
    let status = payload
        .status
        .clone()
        .unwrap_or_else(|| "received".to_string())
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect::<String>();
    let event = GovernmentWebhookEvent {
        event_id: format!("webhook_{}", Uuid::new_v4()),
        request_id,
        jurisdiction,
        provider_reference: payload.provider_reference.clone(),
        event_type: payload
            .event_type
            .unwrap_or_else(|| "government.status".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        status: status.clone(),
        received_at_ms: now_ms(),
        payload_shape: payload_shape(&payload.payload),
    };
    state
        .metrics
        .government_webhooks_total
        .fetch_add(1, Ordering::Relaxed);
    let matched_case = payload
        .case_id
        .as_ref()
        .and_then(|case_id| append_webhook_event(&state, case_id, event.clone(), &status));
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "ok": true,
            "matched": matched_case.is_some(),
            "event": event,
            "case": matched_case
        })),
    )
        .into_response()
}

fn payload_shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let keys = map.keys().take(64).cloned().collect::<Vec<_>>();
            json!({ "type": "object", "keys": keys, "keyCount": map.len() })
        }
        Value::Array(values) => json!({ "type": "array", "length": values.len() }),
        Value::String(text) => json!({ "type": "string", "length": text.len() }),
        Value::Number(_) => json!({ "type": "number" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Null => json!({ "type": "null" }),
    }
}

async fn build_service_case(
    state: &AppState,
    request: CaseRequest,
) -> Result<(ServiceCase, bool), String> {
    let service = service_type(&request.service_type).ok_or_else(|| {
        format!(
            "unsupported serviceType {}; supported values are apostille, notary, immigration",
            request.service_type
        )
    })?;
    let jurisdiction = canonical_jurisdiction(&request.jurisdiction).ok_or_else(|| {
        format!(
            "unsupported jurisdiction {}; call GET /jurisdictions for the catalog",
            request.jurisdiction
        )
    })?;
    if request.documents.is_empty() {
        return Err("documents must include at least one item".to_string());
    }
    if request.documents.len() > MAX_DOCUMENTS_PER_CASE {
        return Err(format!(
            "documents length must be at most {MAX_DOCUMENTS_PER_CASE}"
        ));
    }
    if let Some(url) = request
        .workflow
        .as_ref()
        .and_then(|workflow| workflow.notify_webhook_url.as_ref())
    {
        validate_outbound_url(url, state.config.allow_private_provider_urls)?;
    }
    let request_id = request_id(request.request_id.as_ref(), "case");
    let case_id = format!("case_{}", Uuid::new_v4());
    let target_language = request
        .target_language
        .clone()
        .unwrap_or_else(|| state.config.default_target_language.clone())
        .chars()
        .take(32)
        .collect::<String>();
    let default_source_language = request
        .source_language
        .clone()
        .or_else(|| jurisdiction.primary_languages.first().cloned())
        .unwrap_or_else(|| "auto".to_string());
    let mut warnings = Vec::new();
    let mut documents = Vec::new();
    let mut translations = Vec::new();
    for (index, doc) in request.documents.into_iter().enumerate() {
        let document_id = doc
            .document_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("doc_{}_{}", index + 1, Uuid::new_v4()));
        if let Some(file_url) = doc.file_url.as_ref() {
            validate_outbound_url(file_url, state.config.allow_private_provider_urls)?;
        }
        let source_language = doc
            .source_language
            .clone()
            .or_else(|| request.source_language.clone())
            .unwrap_or_else(|| default_source_language.clone())
            .chars()
            .take(32)
            .collect::<String>();
        let title = clean_text(doc.title.as_ref(), MAX_SHORT_TEXT_LEN);
        let text = clean_text(doc.text.as_ref(), MAX_TEXT_LEN);
        if let Some(title) = title.as_ref() {
            if needs_translation(&source_language, &target_language) {
                translations.push(
                    translate_text(
                        state,
                        &case_id,
                        &document_id,
                        "title",
                        title,
                        &source_language,
                        &target_language,
                        Some(&service.slug),
                    )
                    .await,
                );
            }
        }
        if let Some(text) = text.as_ref() {
            if needs_translation(&source_language, &target_language) {
                translations.push(
                    translate_text(
                        state,
                        &case_id,
                        &document_id,
                        "text",
                        text,
                        &source_language,
                        &target_language,
                        Some(&service.slug),
                    )
                    .await,
                );
            }
        }
        documents.push(DocumentRecord {
            document_id,
            kind: clean_required(&doc.kind, "document.kind")?,
            title,
            issuing_country: clean_short(doc.issuing_country.as_ref()),
            issuing_authority: clean_short(doc.issuing_authority.as_ref()),
            source_language,
            text,
            file_url: doc.file_url,
            metadata: doc.metadata,
        });
    }
    let translation_pending = translations.iter().any(|translation| {
        translation.status != "translated" && translation.status != "not_required"
    });
    if translation_pending {
        warnings.push(
            "one or more fields still need an English translation provider result".to_string(),
        );
    }
    let auto_submit_requested = request
        .workflow
        .as_ref()
        .and_then(|workflow| workflow.auto_submit)
        .unwrap_or(false);
    let status = if translation_pending {
        "translation_pending"
    } else {
        "ready_for_submission"
    }
    .to_string();
    let mut case = ServiceCase {
        case_id,
        request_id,
        customer_reference: clean_short(request.customer_reference.as_ref()),
        status,
        service_type: service.slug,
        jurisdiction,
        applicant: sanitize_applicant(request.applicant)?,
        documents,
        target_language,
        translations,
        government_submissions: Vec::new(),
        webhook_events: Vec::new(),
        consent: request.consent,
        warnings,
        metadata: request.metadata,
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
    };
    let can_submit = submission_consent_ok(&case) && !translation_pending;
    if auto_submit_requested && !can_submit {
        case.warnings.push(
            "autoSubmit was requested but submission is blocked until consent and translations are complete".to_string(),
        );
    }
    Ok((case, auto_submit_requested && can_submit))
}

fn sanitize_applicant(applicant: Applicant) -> Result<Applicant, String> {
    Ok(Applicant {
        full_name: clean_required(&applicant.full_name, "applicant.fullName")?,
        email: clean_short(applicant.email.as_ref()),
        phone: clean_short(applicant.phone.as_ref()),
        nationality: clean_short(applicant.nationality.as_ref()),
        date_of_birth: clean_short(applicant.date_of_birth.as_ref()),
        passport_country: clean_short(applicant.passport_country.as_ref()),
        address_country: clean_short(applicant.address_country.as_ref()),
    })
}

fn needs_translation(source_language: &str, target_language: &str) -> bool {
    let source = source_language
        .split(['-', '_'])
        .next()
        .unwrap_or(source_language)
        .to_ascii_lowercase();
    let target = target_language
        .split(['-', '_'])
        .next()
        .unwrap_or(target_language)
        .to_ascii_lowercase();
    source != target && source != "en"
}

async fn translate_text(
    state: &AppState,
    request_id: &str,
    document_id: &str,
    field: &str,
    text: &str,
    source_language: &str,
    target_language: &str,
    context: Option<&str>,
) -> DocumentTranslation {
    state
        .metrics
        .translations_total
        .fetch_add(1, Ordering::Relaxed);
    if !needs_translation(source_language, target_language) {
        return DocumentTranslation {
            document_id: document_id.to_string(),
            field: field.to_string(),
            source_language: source_language.to_string(),
            target_language: target_language.to_string(),
            status: "not_required".to_string(),
            provider: "local".to_string(),
            original_text: text.to_string(),
            translated_text: Some(text.to_string()),
            translated_at_ms: now_ms(),
            error: None,
        };
    }
    let Some(provider) = state.config.translation_provider.as_ref() else {
        return DocumentTranslation {
            document_id: document_id.to_string(),
            field: field.to_string(),
            source_language: source_language.to_string(),
            target_language: target_language.to_string(),
            status: "provider_not_configured".to_string(),
            provider: "none".to_string(),
            original_text: text.to_string(),
            translated_text: None,
            translated_at_ms: now_ms(),
            error: Some("APOSTILLE_TRANSLATION_BASE_URL is not configured".to_string()),
        };
    };
    let endpoint = provider_endpoint(&provider.base_url, &provider.path);
    let body = json!({
        "requestId": request_id,
        "documentId": document_id,
        "field": field,
        "text": text,
        "sourceLanguage": source_language,
        "targetLanguage": target_language,
        "context": context
    });
    let mut builder = state.http.post(endpoint).json(&body);
    if let (Some(header), Some(token)) = (provider.auth_header.as_ref(), provider.token.as_ref()) {
        if header == "authorization" {
            builder = builder.bearer_auth(token);
        } else {
            builder = builder.header(header.as_str(), token);
        }
    }
    match builder.send().await {
        Ok(response) => {
            let status = response.status();
            match response.json::<Value>().await {
                Ok(value) if status.is_success() => {
                    let translated = value
                        .get("translatedText")
                        .or_else(|| value.get("translation"))
                        .or_else(|| value.get("text"))
                        .and_then(Value::as_str)
                        .map(|value| value.chars().take(MAX_TEXT_LEN).collect::<String>());
                    DocumentTranslation {
                        document_id: document_id.to_string(),
                        field: field.to_string(),
                        source_language: source_language.to_string(),
                        target_language: target_language.to_string(),
                        status: if translated.is_some() {
                            "translated".to_string()
                        } else {
                            "provider_response_missing_translation".to_string()
                        },
                        provider: "configured-http".to_string(),
                        original_text: text.to_string(),
                        translated_text: translated,
                        translated_at_ms: now_ms(),
                        error: None,
                    }
                }
                Ok(value) => {
                    state
                        .metrics
                        .translation_provider_errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    DocumentTranslation {
                        document_id: document_id.to_string(),
                        field: field.to_string(),
                        source_language: source_language.to_string(),
                        target_language: target_language.to_string(),
                        status: "provider_error".to_string(),
                        provider: "configured-http".to_string(),
                        original_text: text.to_string(),
                        translated_text: None,
                        translated_at_ms: now_ms(),
                        error: Some(format!(
                            "translation provider returned HTTP {} with shape {}",
                            status.as_u16(),
                            payload_shape(&value)
                        )),
                    }
                }
                Err(error) => {
                    state
                        .metrics
                        .translation_provider_errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    DocumentTranslation {
                        document_id: document_id.to_string(),
                        field: field.to_string(),
                        source_language: source_language.to_string(),
                        target_language: target_language.to_string(),
                        status: "provider_error".to_string(),
                        provider: "configured-http".to_string(),
                        original_text: text.to_string(),
                        translated_text: None,
                        translated_at_ms: now_ms(),
                        error: Some(format!("translation provider JSON error: {error}")),
                    }
                }
            }
        }
        Err(error) => {
            state
                .metrics
                .translation_provider_errors_total
                .fetch_add(1, Ordering::Relaxed);
            DocumentTranslation {
                document_id: document_id.to_string(),
                field: field.to_string(),
                source_language: source_language.to_string(),
                target_language: target_language.to_string(),
                status: "provider_error".to_string(),
                provider: "configured-http".to_string(),
                original_text: text.to_string(),
                translated_text: None,
                translated_at_ms: now_ms(),
                error: Some(format!("translation provider request failed: {error}")),
            }
        }
    }
}

fn provider_endpoint(base_url: &Url, path: &str) -> Url {
    let mut base = base_url.clone();
    let mut normalized_path = base.path().trim_end_matches('/').to_string();
    normalized_path.push('/');
    normalized_path.push_str(path.trim_start_matches('/'));
    base.set_path(&normalized_path);
    base
}

fn submission_consent_ok(case: &ServiceCase) -> bool {
    let Some(consent) = case.consent.as_ref() else {
        return false;
    };
    let authorized =
        consent.authorized_representative.unwrap_or(false) || consent.self_filed.unwrap_or(false);
    authorized
        && consent.data_use_accepted.unwrap_or(false)
        && consent.government_terms_accepted.unwrap_or(false)
}

fn case_has_pending_translations(case: &ServiceCase) -> bool {
    case.translations.iter().any(|translation| {
        translation.status != "translated" && translation.status != "not_required"
    })
}

async fn submit_case_to_provider(state: &AppState, case: &ServiceCase) -> GovernmentSubmission {
    state
        .metrics
        .submit_attempts_total
        .fetch_add(1, Ordering::Relaxed);
    let submission_id = format!("submission_{}", Uuid::new_v4());
    let Some(provider) = state.config.provider_configs.get(&case.jurisdiction.slug) else {
        return GovernmentSubmission {
            submission_id,
            jurisdiction: case.jurisdiction.slug.clone(),
            service_type: case.service_type.clone(),
            provider: "none".to_string(),
            status: "connector_not_configured".to_string(),
            provider_reference: None,
            submitted_at_ms: now_ms(),
            response_status: None,
            response_summary: None,
            error: Some(format!(
                "no approved provider connector configured for {}",
                case.jurisdiction.slug
            )),
        };
    };
    if !provider.enabled {
        return GovernmentSubmission {
            submission_id,
            jurisdiction: case.jurisdiction.slug.clone(),
            service_type: case.service_type.clone(),
            provider: provider.slug.clone(),
            status: "connector_disabled".to_string(),
            provider_reference: None,
            submitted_at_ms: now_ms(),
            response_status: None,
            response_summary: None,
            error: Some("provider connector is configured but disabled".to_string()),
        };
    }
    if !provider.services.contains(&case.service_type) {
        return GovernmentSubmission {
            submission_id,
            jurisdiction: case.jurisdiction.slug.clone(),
            service_type: case.service_type.clone(),
            provider: provider.slug.clone(),
            status: "service_not_supported_by_connector".to_string(),
            provider_reference: None,
            submitted_at_ms: now_ms(),
            response_status: None,
            response_summary: None,
            error: Some(format!(
                "provider connector does not advertise service {}",
                case.service_type
            )),
        };
    }
    let endpoint = provider_endpoint(&provider.base_url, &provider.submit_path);
    let mut request = state.http.post(endpoint).json(&json!({
        "schemaVersion": SCHEMA_VERSION,
        "submissionId": submission_id,
        "case": case,
    }));
    if let Some(auth) = provider.auth.as_ref() {
        request = match auth.kind.as_str() {
            "bearer" => request.bearer_auth(&auth.token),
            "api-key" | "apikey" | "header" => {
                request.header(auth.header_name.as_str(), &auth.token)
            }
            _ => request.header(auth.header_name.as_str(), &auth.token),
        };
    }
    match request.send().await {
        Ok(response) => {
            let status_code = response.status();
            match response.json::<Value>().await {
                Ok(value) => {
                    let provider_reference = provider_reference(&value);
                    let status = if status_code.is_success() {
                        state
                            .metrics
                            .submit_success_total
                            .fetch_add(1, Ordering::Relaxed);
                        "submitted"
                    } else {
                        "provider_error"
                    };
                    GovernmentSubmission {
                        submission_id,
                        jurisdiction: case.jurisdiction.slug.clone(),
                        service_type: case.service_type.clone(),
                        provider: provider.slug.clone(),
                        status: status.to_string(),
                        provider_reference,
                        submitted_at_ms: now_ms(),
                        response_status: Some(status_code.as_u16()),
                        response_summary: Some(payload_shape(&value)),
                        error: if status_code.is_success() {
                            None
                        } else {
                            Some(format!("provider returned HTTP {}", status_code.as_u16()))
                        },
                    }
                }
                Err(error) => GovernmentSubmission {
                    submission_id,
                    jurisdiction: case.jurisdiction.slug.clone(),
                    service_type: case.service_type.clone(),
                    provider: provider.slug.clone(),
                    status: "provider_error".to_string(),
                    provider_reference: None,
                    submitted_at_ms: now_ms(),
                    response_status: Some(status_code.as_u16()),
                    response_summary: None,
                    error: Some(format!("provider JSON response error: {error}")),
                },
            }
        }
        Err(error) => GovernmentSubmission {
            submission_id,
            jurisdiction: case.jurisdiction.slug.clone(),
            service_type: case.service_type.clone(),
            provider: provider.slug.clone(),
            status: "provider_request_failed".to_string(),
            provider_reference: None,
            submitted_at_ms: now_ms(),
            response_status: None,
            response_summary: None,
            error: Some(format!("provider request failed: {error}")),
        },
    }
}

fn provider_reference(value: &Value) -> Option<String> {
    for key in [
        "providerReference",
        "reference",
        "submissionId",
        "caseReference",
        "id",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return Some(text.chars().take(MAX_TOKEN_LEN).collect());
        }
    }
    None
}

fn upsert_case(state: &AppState, case: ServiceCase) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.cases.insert(case.case_id.clone(), case);
    if store.cases.len() > MAX_CASES {
        if let Some(oldest) = store
            .cases
            .iter()
            .min_by_key(|(_, case)| case.created_at_ms)
            .map(|(case_id, _)| case_id.clone())
        {
            store.cases.remove(&oldest);
        }
    }
}

fn append_submission(
    state: &AppState,
    case_id: &str,
    submission: GovernmentSubmission,
) -> Option<ServiceCase> {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    let case = store.cases.get_mut(case_id)?;
    case.government_submissions.push(submission.clone());
    if case.government_submissions.len() > MAX_SUBMISSIONS_PER_CASE {
        let overflow = case.government_submissions.len() - MAX_SUBMISSIONS_PER_CASE;
        case.government_submissions.drain(0..overflow);
    }
    case.status = match submission.status.as_str() {
        "submitted" => "submitted".to_string(),
        "connector_not_configured" => "blocked_connector_not_configured".to_string(),
        other => other.to_string(),
    };
    case.updated_at_ms = now_ms();
    Some(case.clone())
}

fn append_webhook_event(
    state: &AppState,
    case_id: &str,
    event: GovernmentWebhookEvent,
    status: &str,
) -> Option<ServiceCase> {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    let case = store.cases.get_mut(case_id)?;
    case.webhook_events.push(event);
    if case.webhook_events.len() > MAX_WEBHOOK_EVENTS_PER_CASE {
        let overflow = case.webhook_events.len() - MAX_WEBHOOK_EVENTS_PER_CASE;
        case.webhook_events.drain(0..overflow);
    }
    case.status = format!("government_{}", canonical_slug(status));
    case.updated_at_ms = now_ms();
    Some(case.clone())
}

async fn healthz() -> Json<Value> {
    Json(json!({ "ok": true, "service": SERVICE_NAME }))
}

async fn readyz(State(state): State<AppState>) -> Json<Value> {
    let case_count = state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .cases
        .len();
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "caseCount": case_count,
        "jurisdictionCount": jurisdiction_catalog().len(),
        "providerConnectorCount": state.config.provider_configs.len(),
        "translationProviderConfigured": state.config.translation_provider.is_some(),
        "allowPrivateProviderUrls": state.config.allow_private_provider_urls
    }))
}

async fn metrics(State(state): State<AppState>) -> String {
    let m = &state.metrics;
    format!(
        "\
# TYPE apostille_http_requests_total counter
apostille_http_requests_total {}
# TYPE apostille_cases_created_total counter
apostille_cases_created_total {}
# TYPE apostille_submit_attempts_total counter
apostille_submit_attempts_total {}
# TYPE apostille_submit_success_total counter
apostille_submit_success_total {}
# TYPE apostille_translations_total counter
apostille_translations_total {}
# TYPE apostille_translation_provider_errors_total counter
apostille_translation_provider_errors_total {}
# TYPE apostille_government_webhooks_total counter
apostille_government_webhooks_total {}
# TYPE apostille_auth_failures_total counter
apostille_auth_failures_total {}
# TYPE apostille_errors_total counter
apostille_errors_total {}
",
        m.http_requests_total.load(Ordering::Relaxed),
        m.cases_created_total.load(Ordering::Relaxed),
        m.submit_attempts_total.load(Ordering::Relaxed),
        m.submit_success_total.load(Ordering::Relaxed),
        m.translations_total.load(Ordering::Relaxed),
        m.translation_provider_errors_total.load(Ordering::Relaxed),
        m.government_webhooks_total.load(Ordering::Relaxed),
        m.auth_failures_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
    )
}

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}
