//! Tenant-safe customer cron control plane.
//!
//! The browser authenticates only to this BFF. Every downstream request is
//! rebuilt from scratch with the canonical organization selected from the
//! verified customer session, the appropriate trusted-hop credential, optional
//! W3C trace context, and a stable idempotency key. Browser cookies and bearer
//! tokens are never forwarded to fiducia-node or the function runtime.

use super::*;
use reqwest::{Client, Url};
use serde_json::Value;
use std::time::Duration;

const CRON_UPSTREAM_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_UPSTREAM_BYTES: usize = 2 * 1024 * 1024;
const MAX_SCHEDULE_NAME_BYTES: usize = 128;
const MAX_FUNCTION_SOURCE_BYTES: usize = 256 * 1024;
const MAX_FUNCTION_SLUG_BYTES: usize = 80;
const MAX_CRON_EXPRESSION_BYTES: usize = 128;
const MAX_TARGET_BYTES: usize = 2_048;
const MAX_RUN_MS: u64 = 120_000;
const MAX_TRACESTATE_BYTES: usize = 512;

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn valid_traceparent(value: &HeaderValue) -> bool {
    let Ok(value) = value.to_str() else {
        return false;
    };
    let bytes = value.as_bytes();
    if bytes.len() != 55
        || &bytes[0..2] != b"00"
        || bytes[2] != b'-'
        || bytes[35] != b'-'
        || bytes[52] != b'-'
    {
        return false;
    }
    let lower_hex = |byte: &u8| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase();
    if !bytes[3..35].iter().all(lower_hex)
        || !bytes[36..52].iter().all(lower_hex)
        || !bytes[53..55].iter().all(lower_hex)
    {
        return false;
    }
    bytes[3..35].iter().any(|byte| *byte != b'0') && bytes[36..52].iter().any(|byte| *byte != b'0')
}

fn valid_tracestate(value: &HeaderValue) -> bool {
    value.to_str().is_ok_and(|value| {
        !value.is_empty()
            && value.len() <= MAX_TRACESTATE_BYTES
            && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    })
}

fn insert_valid_trace_context(incoming: &HeaderMap, outgoing: &mut HeaderMap) {
    let traceparent = HeaderName::from_static("traceparent");
    let tracestate = HeaderName::from_static("tracestate");
    let Some(value) = incoming
        .get(&traceparent)
        .filter(|value| valid_traceparent(value))
    else {
        return;
    };
    outgoing.insert(traceparent, value.clone());
    if let Some(value) = incoming
        .get(&tracestate)
        .filter(|value| valid_tracestate(value))
    {
        outgoing.insert(tracestate, value.clone());
    }
}

#[derive(Clone)]
pub(crate) struct CronServices {
    client: Client,
    node: Option<TrustedService>,
    functions: Option<TrustedService>,
}

#[derive(Clone)]
struct TrustedService {
    base: Url,
    auth_header: HeaderName,
    secret: HeaderValue,
}

#[derive(Clone, Copy)]
enum ServiceKind {
    Node,
    Functions,
}

impl CronServices {
    pub(crate) fn from_env() -> Self {
        let client = Client::builder()
            .timeout(CRON_UPSTREAM_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("cron upstream client configuration is static");
        let node_url =
            optional_env("FIDUCIA_CRON_NODE_URL").or_else(|| optional_env("FIDUCIA_NODE_URL"));
        let node_secret = optional_env("FIDUCIA_INTERNAL_SECRET");
        let function_url = optional_env("FIDUCIA_LAMBDA_SERVICE_URL");
        let function_secret = optional_env("FIDUCIA_LAMBDA_SERVER_AUTH_SECRET");
        Self {
            client,
            node: trusted_service(
                node_url,
                node_secret,
                HeaderName::from_static("x-fiducia-internal-auth"),
            ),
            functions: trusted_service(
                function_url,
                function_secret,
                HeaderName::from_static("x-server-auth"),
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            client: Client::builder()
                .timeout(CRON_UPSTREAM_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            node: None,
            functions: None,
        }
    }
}

fn trusted_service(
    raw_url: Option<String>,
    raw_secret: Option<String>,
    auth_header: HeaderName,
) -> Option<TrustedService> {
    let raw_url = raw_url?;
    let raw_secret = raw_secret?;
    let mut base = Url::parse(raw_url.trim()).ok()?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        tracing::error!(service = %auth_header, "invalid cron upstream URL; surface disabled");
        return None;
    }
    base.set_query(None);
    base.set_fragment(None);
    let normalized = format!("{}/", base.path().trim_end_matches('/'));
    base.set_path(&normalized);
    let secret = HeaderValue::from_str(&raw_secret).ok()?;
    Some(TrustedService {
        base,
        auth_header,
        secret,
    })
}

pub(crate) fn cron_routes(router: Router<AppConfig>) -> Router<AppConfig> {
    router
        .route("/app/crons", get(customer_crons))
        .route("/app/fragments/crons", get(cron_fragment))
        .route(
            "/app/fragments/crons/:name/history",
            get(cron_history_fragment),
        )
        .route("/app/crons/schedules", post(create_schedule_form))
        .route(
            "/app/crons/schedules/:name/:action",
            post(schedule_action_form),
        )
        .route("/app/crons/functions", post(create_function_form))
        .route(
            "/app/crons/functions/:function_id/:action",
            post(function_action_form),
        )
        .route(
            "/api/customer/crons",
            get(list_schedules_api).post(create_schedule_api),
        )
        .route(
            "/api/customer/crons/:name",
            get(get_schedule_api)
                .put(update_schedule_api)
                .delete(delete_schedule_api),
        )
        .route("/api/customer/crons/:name/pause", post(pause_schedule_api))
        .route(
            "/api/customer/crons/:name/resume",
            post(resume_schedule_api),
        )
        .route(
            "/api/customer/crons/:name/trigger",
            post(trigger_schedule_api),
        )
        .route(
            "/api/customer/crons/:name/history",
            get(schedule_history_api),
        )
        .route(
            "/api/customer/cron-functions",
            get(list_functions_api).post(create_function_api),
        )
        .route(
            "/api/customer/cron-functions/:function_id",
            get(get_function_api)
                .put(update_function_api)
                .delete(delete_function_api),
        )
        .route(
            "/api/customer/cron-functions/:function_id/check",
            post(check_function_api),
        )
        .route(
            "/api/customer/cron-functions/:function_id/pause",
            post(pause_function_api),
        )
}

async fn customer_crons(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Crons,
        selection.org_id.as_deref(),
    )
    .await
}

async fn list_schedules_api(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    proxy_read(
        &config,
        &headers,
        selection.org_id.as_deref(),
        ServiceKind::Node,
        &["v1", "cron", "schedules"],
        &[("limit", "200".to_string())],
    )
    .await
}

async fn create_schedule_api(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    let customer = match authenticate_write(&config, &headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let org_id = match selected_customer_org(&customer, &headers) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    let name = match take_schedule_name(&mut body) {
        Ok(name) => name,
        Err(response) => return response,
    };
    let idempotency = match required_idempotency(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    proxy_json(
        &config,
        &headers,
        &org_id,
        ServiceKind::Node,
        reqwest::Method::PUT,
        &["v1", "cron", "schedules", &name],
        &[],
        Some(body),
        Some(idempotency),
    )
    .await
}

async fn get_schedule_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    if !valid_schedule_name(&name) {
        return bad_request("invalid_schedule_name");
    }
    proxy_read(
        &config,
        &headers,
        selection.org_id.as_deref(),
        ServiceKind::Node,
        &["v1", "cron", "schedules", &name],
        &[],
    )
    .await
}

async fn update_schedule_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !valid_schedule_name(&name) {
        return bad_request("invalid_schedule_name");
    }
    proxy_write_for_selected_org(
        &config,
        &headers,
        ServiceKind::Node,
        reqwest::Method::PUT,
        &["v1", "cron", "schedules", &name],
        &[],
        Some(body),
    )
    .await
}

async fn delete_schedule_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !valid_schedule_name(&name) {
        return bad_request("invalid_schedule_name");
    }
    proxy_write_for_selected_org(
        &config,
        &headers,
        ServiceKind::Node,
        reqwest::Method::DELETE,
        &["v1", "cron", "schedules", &name],
        &[],
        None,
    )
    .await
}

async fn pause_schedule_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    schedule_write_action(&config, &headers, &name, "pause", &[]).await
}

#[derive(Debug, Default, Deserialize)]
struct ResumeQuery {
    catch_up: Option<bool>,
}

async fn resume_schedule_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ResumeQuery>,
) -> Response {
    schedule_write_action(
        &config,
        &headers,
        &name,
        "resume",
        &[("catch_up", query.catch_up.unwrap_or(false).to_string())],
    )
    .await
}

#[derive(Debug, Default, Deserialize)]
struct TriggerQuery {
    fire_id_ms: Option<u64>,
}

async fn trigger_schedule_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(query): Query<TriggerQuery>,
) -> Response {
    let fire_id_ms = query.fire_id_ms.unwrap_or_else(now_ms);
    schedule_write_action(
        &config,
        &headers,
        &name,
        "trigger",
        &[("fire_id_ms", fire_id_ms.to_string())],
    )
    .await
}

async fn schedule_history_api(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    if !valid_schedule_name(&name) {
        return bad_request("invalid_schedule_name");
    }
    proxy_read(
        &config,
        &headers,
        selection.org_id.as_deref(),
        ServiceKind::Node,
        &["v1", "cron", "schedules", &name, "history"],
        &[("limit", "100".to_string())],
    )
    .await
}

async fn schedule_write_action(
    config: &AppConfig,
    headers: &HeaderMap,
    name: &str,
    action: &str,
    query: &[(&str, String)],
) -> Response {
    if !valid_schedule_name(name) {
        return bad_request("invalid_schedule_name");
    }
    proxy_write_for_selected_org(
        config,
        headers,
        ServiceKind::Node,
        reqwest::Method::POST,
        &["v1", "cron", "schedules", name, action],
        query,
        None,
    )
    .await
}

async fn list_functions_api(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    proxy_read(
        &config,
        &headers,
        selection.org_id.as_deref(),
        ServiceKind::Functions,
        &["v1", "functions"],
        &[],
    )
    .await
}

async fn create_function_api(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let body = match validated_function_body(body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    proxy_write_for_selected_org(
        &config,
        &headers,
        ServiceKind::Functions,
        reqwest::Method::POST,
        &["v1", "functions"],
        &[],
        Some(body),
    )
    .await
}

async fn get_function_api(
    State(config): State<AppConfig>,
    Path(function_id): Path<String>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    if !valid_function_id(&function_id) {
        return bad_request("invalid_function_id");
    }
    proxy_read(
        &config,
        &headers,
        selection.org_id.as_deref(),
        ServiceKind::Functions,
        &["v1", "functions", &function_id],
        &[],
    )
    .await
}

async fn update_function_api(
    State(config): State<AppConfig>,
    Path(function_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !valid_function_id(&function_id) {
        return bad_request("invalid_function_id");
    }
    let body = match validated_function_body(body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    proxy_write_for_selected_org(
        &config,
        &headers,
        ServiceKind::Functions,
        reqwest::Method::PUT,
        &["v1", "functions", &function_id],
        &[],
        Some(body),
    )
    .await
}

async fn delete_function_api(
    State(config): State<AppConfig>,
    Path(function_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    function_write_action(
        &config,
        &headers,
        &function_id,
        reqwest::Method::DELETE,
        None,
    )
    .await
}

async fn check_function_api(
    State(config): State<AppConfig>,
    Path(function_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    function_write_action(
        &config,
        &headers,
        &function_id,
        reqwest::Method::POST,
        Some("check"),
    )
    .await
}

async fn pause_function_api(
    State(config): State<AppConfig>,
    Path(function_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    function_write_action(
        &config,
        &headers,
        &function_id,
        reqwest::Method::POST,
        Some("pause"),
    )
    .await
}

async fn function_write_action(
    config: &AppConfig,
    headers: &HeaderMap,
    function_id: &str,
    method: reqwest::Method,
    action: Option<&str>,
) -> Response {
    if !valid_function_id(function_id) {
        return bad_request("invalid_function_id");
    }
    let mut path = vec!["v1", "functions", function_id];
    if let Some(action) = action {
        path.push(action);
    }
    proxy_write_for_selected_org(
        config,
        headers,
        ServiceKind::Functions,
        method,
        &path,
        &[],
        None,
    )
    .await
}

async fn proxy_read(
    config: &AppConfig,
    headers: &HeaderMap,
    explicit_org: Option<&str>,
    service: ServiceKind,
    path: &[&str],
    query: &[(&str, String)],
) -> Response {
    let customer = match config.authenticator.authenticate(headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let org_id = match selected_customer_org_from(&customer, headers, explicit_org) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    proxy_json(
        config,
        headers,
        &org_id,
        service,
        reqwest::Method::GET,
        path,
        query,
        None,
        None,
    )
    .await
}

async fn proxy_write_for_selected_org(
    config: &AppConfig,
    headers: &HeaderMap,
    service: ServiceKind,
    method: reqwest::Method,
    path: &[&str],
    query: &[(&str, String)],
    body: Option<Value>,
) -> Response {
    let customer = match authenticate_write(config, headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let org_id = match selected_customer_org(&customer, headers) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    let idempotency = match required_idempotency(headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    proxy_json(
        config,
        headers,
        &org_id,
        service,
        method,
        path,
        query,
        body,
        Some(idempotency),
    )
    .await
}

async fn authenticate_write(
    config: &AppConfig,
    headers: &HeaderMap,
) -> Result<CustomerCtx, Response> {
    let customer = config.authenticator.authenticate(headers).await?;
    if let Err(error) = require_api_write_security(headers, config, &customer) {
        return Err(request_security_error(error));
    }
    Ok(customer)
}

fn required_idempotency(headers: &HeaderMap) -> Result<HeaderValue, Response> {
    require_idempotency_key(headers).cloned()
}

#[allow(clippy::too_many_arguments)]
async fn proxy_json(
    config: &AppConfig,
    incoming: &HeaderMap,
    org_id: &str,
    service_kind: ServiceKind,
    method: reqwest::Method,
    path: &[&str],
    query: &[(&str, String)],
    body: Option<Value>,
    idempotency: Option<HeaderValue>,
) -> Response {
    let service = match service_kind {
        ServiceKind::Node => config.cron_services.node.as_ref(),
        ServiceKind::Functions => config.cron_services.functions.as_ref(),
    };
    let Some(service) = service else {
        return dependency_error(
            match service_kind {
                ServiceKind::Node => "fiducia-node",
                ServiceKind::Functions => "fiducia-lambda-service",
            },
            "cron_service_not_configured",
            "required cron upstream URL or secret is unset/invalid",
        );
    };
    let url = match service_url(service, path, query) {
        Ok(url) => url,
        Err(code) => return bad_gateway(code),
    };
    let headers = match outbound_headers(service, org_id, incoming, idempotency.as_ref()) {
        Ok(headers) => headers,
        Err(code) => return bad_gateway(code),
    };
    let mut request = config
        .cron_services
        .client
        .request(method, url)
        .headers(headers);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                dependency = match service_kind {
                    ServiceKind::Node => "fiducia-node",
                    ServiceKind::Functions => "fiducia-lambda-service",
                },
                error.class = if error.is_timeout() {
                    "timeout"
                } else {
                    "transport"
                },
                "cron upstream request failed"
            );
            return bad_gateway("cron_upstream_unavailable");
        }
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if response
        .content_length()
        .is_some_and(|length| length > MAX_UPSTREAM_BYTES as u64)
    {
        return bad_gateway("cron_upstream_response_too_large");
    }
    let bytes = match response.bytes().await {
        Ok(bytes) if bytes.len() <= MAX_UPSTREAM_BYTES => bytes,
        Ok(_) => return bad_gateway("cron_upstream_response_too_large"),
        Err(_) => return bad_gateway("cron_upstream_response_invalid"),
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return bad_gateway("cron_upstream_response_invalid"),
    };
    no_store_json(status, sanitize_upstream_json(status, value))
}

fn service_url(
    service: &TrustedService,
    path: &[&str],
    query: &[(&str, String)],
) -> Result<Url, &'static str> {
    let mut url = service.base.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "cron_upstream_url_invalid")?;
        segments.pop_if_empty();
        for segment in path {
            segments.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

fn outbound_headers(
    service: &TrustedService,
    org_id: &str,
    incoming: &HeaderMap,
    idempotency: Option<&HeaderValue>,
) -> Result<HeaderMap, &'static str> {
    let mut headers = HeaderMap::new();
    headers.insert(service.auth_header.clone(), service.secret.clone());
    headers.insert(
        HeaderName::from_static(CUSTOMER_ORG_HEADER),
        HeaderValue::from_str(org_id).map_err(|_| "invalid_org_selection")?,
    );
    if let Some(idempotency) = idempotency {
        headers.insert(
            HeaderName::from_static(IDEMPOTENCY_KEY_HEADER),
            idempotency.clone(),
        );
    }
    insert_valid_trace_context(incoming, &mut headers);
    Ok(headers)
}

fn sanitize_upstream_json(status: StatusCode, value: Value) -> Value {
    if status.is_success() {
        return value;
    }
    let code = value
        .get("error")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
        .unwrap_or("cron_upstream_rejected_request");
    json!({ "ok": false, "error": code })
}

fn take_schedule_name(body: &mut Value) -> Result<String, Response> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| bad_request("invalid_schedule"))?;
    let name = object
        .remove("name")
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| bad_request("schedule_name_required"))?;
    if !valid_schedule_name(&name) {
        return Err(bad_request("invalid_schedule_name"));
    }
    Ok(name)
}

fn validated_function_body(mut body: Value) -> Result<Value, Response> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| bad_request("invalid_function"))?;
    if let Some(runtime) = object.get("runtime").and_then(Value::as_str) {
        if runtime != "nodejs" {
            return Err(bad_request("unsupported_function_runtime"));
        }
    }
    object.insert("runtime".to_string(), Value::String("nodejs".to_string()));
    if object
        .get("entryCommand")
        .or_else(|| object.get("entry_command"))
        .is_some()
        || object.get("environment").is_some()
        || object.get("container").is_some()
    {
        return Err(bad_request("unsupported_function_configuration"));
    }
    let slug = object
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_slug(slug) {
        return Err(bad_request("invalid_function_slug"));
    }
    let source = object
        .get("functionBody")
        .or_else(|| object.get("function_body"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if source.is_empty() || source.len() > MAX_FUNCTION_SOURCE_BYTES {
        return Err(bad_request("invalid_function_source"));
    }
    let max_run_ms = object
        .get("maxRunMs")
        .or_else(|| object.get("max_run_ms"))
        .and_then(Value::as_u64)
        .unwrap_or(30_000);
    if max_run_ms == 0 || max_run_ms > MAX_RUN_MS {
        return Err(bad_request("invalid_function_timeout"));
    }
    Ok(body)
}

fn valid_schedule_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SCHEDULE_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_function_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FUNCTION_SLUG_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn bad_request(code: &'static str) -> Response {
    no_store_json(
        StatusCode::BAD_REQUEST,
        json!({ "ok": false, "error": code }),
    )
}

fn bad_gateway(code: &'static str) -> Response {
    no_store_json(
        StatusCode::BAD_GATEWAY,
        json!({ "ok": false, "error": code }),
    )
}

#[derive(Debug, Deserialize)]
struct CreateScheduleForm {
    csrf_token: String,
    org_id: String,
    idempotency_key: String,
    name: String,
    cron: String,
    target_kind: String,
    target_value: String,
    max_retries: Option<u32>,
}

async fn create_schedule_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<CreateScheduleForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let org_id = match selected_customer_org_from(&customer, &headers, Some(form.org_id.trim())) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    if !valid_schedule_name(form.name.trim())
        || form.cron.is_empty()
        || form.cron.len() > MAX_CRON_EXPRESSION_BYTES
        || form.target_value.is_empty()
        || form.target_value.len() > MAX_TARGET_BYTES
    {
        return bad_request("invalid_schedule");
    }
    let target = match form.target_kind.as_str() {
        "function" if valid_function_id(form.target_value.trim()) => {
            json!({ "kind": "function", "function_id": form.target_value.trim() })
        }
        "webhook" => json!({ "kind": "webhook", "url": form.target_value.trim() }),
        _ => return bad_request("invalid_schedule_target"),
    };
    let idempotency = match HeaderValue::from_str(form.idempotency_key.trim()) {
        Ok(value) => value,
        Err(_) => return bad_request("invalid_idempotency_key"),
    };
    let response = proxy_json(
        &config,
        &headers,
        &org_id,
        ServiceKind::Node,
        reqwest::Method::PUT,
        &["v1", "cron", "schedules", form.name.trim()],
        &[],
        Some(json!({
            "cron": form.cron.trim(),
            "target": target,
            "delivery": "at_least_once",
            "max_retries": form.max_retries.unwrap_or(3).min(20),
            "enabled": true
        })),
        Some(idempotency),
    )
    .await;
    if response.status().is_success() {
        cron_fragment_for(&config, &headers, &customer, &org_id).await
    } else {
        response
    }
}

#[derive(Debug, Deserialize)]
struct CronActionForm {
    csrf_token: String,
    org_id: String,
    idempotency_key: String,
}

async fn schedule_action_form(
    State(config): State<AppConfig>,
    Path((name, action)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<CronActionForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let org_id = match selected_customer_org_from(&customer, &headers, Some(form.org_id.trim())) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    if !valid_schedule_name(&name)
        || !matches!(action.as_str(), "pause" | "resume" | "trigger" | "delete")
    {
        return bad_request("invalid_schedule_action");
    }
    let idempotency = match HeaderValue::from_str(form.idempotency_key.trim()) {
        Ok(value) => value,
        Err(_) => return bad_request("invalid_idempotency_key"),
    };
    let (method, path, query) = if action == "delete" {
        (
            reqwest::Method::DELETE,
            vec!["v1", "cron", "schedules", name.as_str()],
            Vec::new(),
        )
    } else if action == "trigger" {
        (
            reqwest::Method::POST,
            vec!["v1", "cron", "schedules", name.as_str(), "trigger"],
            vec![("fire_id_ms", now_ms().to_string())],
        )
    } else {
        (
            reqwest::Method::POST,
            vec!["v1", "cron", "schedules", name.as_str(), action.as_str()],
            Vec::new(),
        )
    };
    let response = proxy_json(
        &config,
        &headers,
        &org_id,
        ServiceKind::Node,
        method,
        &path,
        &query,
        None,
        Some(idempotency),
    )
    .await;
    if response.status().is_success() {
        cron_fragment_for(&config, &headers, &customer, &org_id).await
    } else {
        response
    }
}

#[derive(Debug, Deserialize)]
struct CreateFunctionForm {
    csrf_token: String,
    org_id: String,
    idempotency_key: String,
    slug: String,
    display_name: String,
    function_body: String,
    max_run_ms: Option<u64>,
}

async fn create_function_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<CreateFunctionForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let org_id = match selected_customer_org_from(&customer, &headers, Some(form.org_id.trim())) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    let body = match validated_function_body(json!({
        "slug": form.slug.trim(),
        "displayName": form.display_name.trim(),
        "runtime": "nodejs",
        "functionBody": form.function_body,
        "maxRunMs": form.max_run_ms.unwrap_or(30_000),
        "labels": ["cron"]
    })) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let idempotency = match HeaderValue::from_str(form.idempotency_key.trim()) {
        Ok(value) => value,
        Err(_) => return bad_request("invalid_idempotency_key"),
    };
    let response = proxy_json(
        &config,
        &headers,
        &org_id,
        ServiceKind::Functions,
        reqwest::Method::POST,
        &["v1", "functions"],
        &[],
        Some(body),
        Some(idempotency),
    )
    .await;
    if response.status().is_success() {
        cron_fragment_for(&config, &headers, &customer, &org_id).await
    } else {
        response
    }
}

async fn function_action_form(
    State(config): State<AppConfig>,
    Path((function_id, action)): Path<(String, String)>,
    headers: HeaderMap,
    Form(form): Form<CronActionForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let org_id = match selected_customer_org_from(&customer, &headers, Some(form.org_id.trim())) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    if !valid_function_id(&function_id) || !matches!(action.as_str(), "check" | "pause" | "delete")
    {
        return bad_request("invalid_function_action");
    }
    let idempotency = match HeaderValue::from_str(form.idempotency_key.trim()) {
        Ok(value) => value,
        Err(_) => return bad_request("invalid_idempotency_key"),
    };
    let mut path = vec!["v1", "functions", function_id.as_str()];
    let method = if action == "delete" {
        reqwest::Method::DELETE
    } else {
        path.push(action.as_str());
        reqwest::Method::POST
    };
    let response = proxy_json(
        &config,
        &headers,
        &org_id,
        ServiceKind::Functions,
        method,
        &path,
        &[],
        None,
        Some(idempotency),
    )
    .await;
    if response.status().is_success() {
        cron_fragment_for(&config, &headers, &customer, &org_id).await
    } else {
        response
    }
}

async fn cron_fragment(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let org_id = match selected_customer_org_from(&customer, &headers, selection.org_id.as_deref())
    {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    cron_fragment_for(&config, &headers, &customer, &org_id).await
}

async fn cron_fragment_for(
    config: &AppConfig,
    headers: &HeaderMap,
    customer: &CustomerCtx,
    org_id: &str,
) -> Response {
    let schedules = fetch_value(
        config,
        headers,
        org_id,
        ServiceKind::Node,
        &["v1", "cron", "schedules"],
        &[("limit", "200".to_string())],
    )
    .await;
    let functions = fetch_value(
        config,
        headers,
        org_id,
        ServiceKind::Functions,
        &["v1", "functions"],
        &[],
    )
    .await;
    let dependency_error = schedules
        .as_ref()
        .err()
        .copied()
        .or_else(|| functions.as_ref().err().copied());
    cron_inventory_markup(
        org_id,
        &customer_csrf_token(config, customer),
        schedules.as_ref().ok(),
        functions.as_ref().ok(),
        dependency_error,
    )
    .into_response()
}

async fn fetch_value(
    config: &AppConfig,
    incoming: &HeaderMap,
    org_id: &str,
    kind: ServiceKind,
    path: &[&str],
    query: &[(&str, String)],
) -> Result<Value, &'static str> {
    let service = match kind {
        ServiceKind::Node => config.cron_services.node.as_ref(),
        ServiceKind::Functions => config.cron_services.functions.as_ref(),
    }
    .ok_or("cron_service_not_configured")?;
    let url = service_url(service, path, query)?;
    let headers = outbound_headers(service, org_id, incoming, None)?;
    let response = config
        .cron_services
        .client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|_| "cron_upstream_unavailable")?;
    if !response.status().is_success() {
        return Err("cron_upstream_rejected_request");
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_UPSTREAM_BYTES as u64)
    {
        return Err("cron_upstream_response_too_large");
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "cron_upstream_response_invalid")?;
    if bytes.len() > MAX_UPSTREAM_BYTES {
        return Err("cron_upstream_response_too_large");
    }
    serde_json::from_slice(&bytes).map_err(|_| "cron_upstream_response_invalid")
}

async fn cron_history_fragment(
    State(config): State<AppConfig>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    let org_id = match selected_customer_org_from(&customer, &headers, selection.org_id.as_deref())
    {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    if !valid_schedule_name(&name) {
        return bad_request("invalid_schedule_name");
    }
    match fetch_value(
        &config,
        &headers,
        &org_id,
        ServiceKind::Node,
        &["v1", "cron", "schedules", &name, "history"],
        &[("limit", "100".to_string())],
    )
    .await
    {
        Ok(value) => cron_history_markup(&value).into_response(),
        Err(code) => cron_error_markup(code).into_response(),
    }
}

pub(crate) fn cron_markup(org_id: &str, csrf_token: &str) -> Markup {
    let fragment = format!("/app/fragments/crons?org_id={}", encode_query_value(org_id));
    html! {
        div class="panel-grid" {
            section class="panel" aria-labelledby="create-cron-heading" {
                div class="panel__header" { h2 id="create-cron-heading" { "Create schedule" } span { "tenant scoped" } }
                form class="form-grid" method="post" action="/app/crons/schedules"
                    hx-post="/app/crons/schedules" hx-target="#cron-inventory" hx-swap="innerHTML" {
                    input type="hidden" name="csrf_token" value=(csrf_token);
                    input type="hidden" name="org_id" value=(org_id);
                    input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
                    label { span { "Name" } input name="name" maxlength=(MAX_SCHEDULE_NAME_BYTES) placeholder="daily-rollup" required; }
                    label { span { "Cron (UTC)" } input name="cron" maxlength=(MAX_CRON_EXPRESSION_BYTES) value="0 2 * * *" required; }
                    label { span { "Target kind" } select name="target_kind" { option value="function" { "Custom function" } option value="webhook" { "Webhook" } } }
                    label { span { "Function UUID or webhook URL" } input name="target_value" maxlength=(MAX_TARGET_BYTES) required; }
                    label { span { "Retries" } input type="number" name="max_retries" min="0" max="20" value="3"; }
                    button type="submit" { "Create schedule" }
                }
            }
            section class="panel" aria-labelledby="create-function-heading" {
                div class="panel__header" { h2 id="create-function-heading" { "Create custom function" } span { "managed Node.js" } }
                form class="form-grid" method="post" action="/app/crons/functions"
                    hx-post="/app/crons/functions" hx-target="#cron-inventory" hx-swap="innerHTML" {
                    input type="hidden" name="csrf_token" value=(csrf_token);
                    input type="hidden" name="org_id" value=(org_id);
                    input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
                    label { span { "Slug" } input name="slug" maxlength=(MAX_FUNCTION_SLUG_BYTES) placeholder="daily-rollup" required; }
                    label { span { "Display name" } input name="display_name" maxlength="120" placeholder="Daily rollup" required; }
                    label { span { "Timeout (ms)" } input type="number" name="max_run_ms" min="1" max=(MAX_RUN_MS) value="30000"; }
                    label { span { "Function body" } textarea name="function_body" maxlength=(MAX_FUNCTION_SOURCE_BYTES) rows="12" required { "return { ok: true, request };" } }
                    p class="muted" { "Code is stored outside Raft, remains draft until its sandbox check succeeds, and cannot select shell/container/browser runtimes." }
                    button type="submit" { "Create draft" }
                }
            }
        }
        section id="cron-inventory" hx-get=(fragment) hx-trigger="load, cron-refresh from:body" hx-swap="innerHTML" {
            p class="muted" { "Loading schedules and functions…" }
        }
    }
}

fn cron_inventory_markup(
    org_id: &str,
    csrf_token: &str,
    schedules: Option<&Value>,
    functions: Option<&Value>,
    error: Option<&'static str>,
) -> Markup {
    let schedule_rows = schedules
        .and_then(|value| value.get("schedules"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let function_rows = functions.and_then(function_rows).unwrap_or_default();
    html! {
        @if let Some(error) = error { (cron_error_markup(error)) }
        section class="panel" aria-labelledby="cron-schedules-heading" {
            div class="panel__header" { h2 id="cron-schedules-heading" { "Schedules" } span { (schedule_rows.len()) " total" } }
            div class="table-wrap" { table {
                thead { tr { th { "Name" } th { "Schedule" } th { "Target" } th { "State" } th { "Actions" } } }
                tbody {
                    @if schedule_rows.is_empty() { tr { td colspan="5" class="muted" { "No schedules yet." } } }
                    @for row in schedule_rows {
                        @let name = string_field(row, "name");
                        @let enabled = row.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                        tr {
                            td { code { (name) } }
                            td { (schedule_label(row)) }
                            td { (target_label(row.get("target"))) }
                            td { @if enabled { "enabled" } @else { "paused" } }
                            td class="action-row" {
                                (cron_action_form(org_id, csrf_token, &name, if enabled { "pause" } else { "resume" }, if enabled { "Pause" } else { "Resume" }))
                                (cron_action_form(org_id, csrf_token, &name, "trigger", "Run now"))
                                (cron_action_form(org_id, csrf_token, &name, "delete", "Delete"))
                                button type="button"
                                    hx-get=(format!("/app/fragments/crons/{}/history?org_id={}", encode_query_value(&name), encode_query_value(org_id)))
                                    hx-target=(format!("#cron-history-{}", safe_dom_id(&name))) hx-swap="innerHTML" { "Trail" }
                            }
                        }
                        tr { td colspan="5" { div id=(format!("cron-history-{}", safe_dom_id(&name))) {} } }
                    }
                }
            } }
        }
        section class="panel" aria-labelledby="cron-functions-heading" {
            div class="panel__header" { h2 id="cron-functions-heading" { "Custom functions" } span { (function_rows.len()) " total" } }
            div class="table-wrap" { table {
                thead { tr { th { "Name" } th { "UUID" } th { "Runtime" } th { "State" } th { "Actions" } } }
                tbody {
                    @if function_rows.is_empty() { tr { td colspan="5" class="muted" { "No custom functions yet." } } }
                    @for row in function_rows {
                        @let id = function_id(row);
                        tr {
                            td { (string_field_fallback(row, &["displayName", "display_name", "slug"])) }
                            td { code { (id) } }
                            td { (string_field_fallback(row, &["runtime"])) }
                            td { (string_field_fallback(row, &["status", "state"])) }
                            td class="action-row" {
                                @if valid_function_id(&id) {
                                    (function_action_button(org_id, csrf_token, &id, "check", "Check & activate"))
                                    (function_action_button(org_id, csrf_token, &id, "pause", "Pause"))
                                    (function_action_button(org_id, csrf_token, &id, "delete", "Delete"))
                                }
                            }
                        }
                    }
                }
            } }
        }
    }
}

fn cron_action_form(org_id: &str, csrf: &str, name: &str, action: &str, label: &str) -> Markup {
    html! {
        form method="post" action=(format!("/app/crons/schedules/{}/{}", encode_query_value(name), action))
            hx-post=(format!("/app/crons/schedules/{}/{}", encode_query_value(name), action)) hx-target="#cron-inventory" hx-swap="innerHTML" {
            input type="hidden" name="csrf_token" value=(csrf);
            input type="hidden" name="org_id" value=(org_id);
            input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
            button type="submit" { (label) }
        }
    }
}

fn function_action_button(org_id: &str, csrf: &str, id: &str, action: &str, label: &str) -> Markup {
    html! {
        form method="post" action=(format!("/app/crons/functions/{}/{}", id, action))
            hx-post=(format!("/app/crons/functions/{}/{}", id, action)) hx-target="#cron-inventory" hx-swap="innerHTML" {
            input type="hidden" name="csrf_token" value=(csrf);
            input type="hidden" name="org_id" value=(org_id);
            input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
            button type="submit" { (label) }
        }
    }
}

fn cron_history_markup(value: &Value) -> Markup {
    let rows = value
        .get("history")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    html! {
        div class="table-wrap" { table {
            thead { tr { th { "Fire" } th { "Status" } th { "Trigger" } th { "Attempts" } th { "Duration" } th { "HTTP" } th { "Error class" } th { "Trace" } } }
            tbody {
                @if rows.is_empty() { tr { td colspan="8" class="muted" { "No runs yet." } } }
                @for run in rows {
                    tr {
                        td { code { (display_field(run, "fire_id")) } }
                        td { (display_field(run, "status")) }
                        td { (display_field(run, "trigger")) }
                        td { (display_field(run, "attempts")) }
                        td { (display_field(run, "duration_ms")) " ms" }
                        td { (display_field(run, "http_status")) }
                        td { (display_field(run, "error_class")) }
                        td { code { (display_field(run, "trace_id")) } }
                    }
                }
            }
        } }
    }
}

fn cron_error_markup(code: &str) -> Markup {
    html! { section class="panel" role="alert" { p class="muted" { "Cron service unavailable: " code { (code) } } } }
}

fn function_rows(value: &Value) -> Option<&[Value]> {
    value
        .get("functions")
        .or_else(|| value.get("definitions"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn function_id(value: &Value) -> String {
    string_field_fallback(value, &["functionId", "function_id", "id"])
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn string_field_fallback(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

fn display_field(value: &Value, key: &str) -> String {
    match value.get(key) {
        None | Some(Value::Null) => "—".to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(_) => "[structured]".to_string(),
    }
}

fn schedule_label(value: &Value) -> String {
    value
        .get("cron")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("one_shot_at_ms")
                .map(|value| format!("one-shot {value}"))
        })
        .unwrap_or_else(|| "—".to_string())
}

fn target_label(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "—".to_string();
    };
    match value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "function" => format!("function:{}", display_field(value, "function_id")),
        "webhook" => "webhook:[configured]".to_string(),
        "grpc" => "grpc:[configured]".to_string(),
        "queue" => format!("queue:{}", display_field(value, "name")),
        other => other.to_string(),
    }
}

fn safe_dom_id(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                byte as char
            } else {
                '-'
            }
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> TrustedService {
        TrustedService {
            base: Url::parse("https://node.example/internal/").unwrap(),
            auth_header: HeaderName::from_static("x-fiducia-internal-auth"),
            secret: HeaderValue::from_static("internal-secret"),
        }
    }

    #[test]
    fn outbound_headers_never_forward_browser_credentials() {
        let mut incoming = HeaderMap::new();
        incoming.insert(header::COOKIE, HeaderValue::from_static("session=secret"));
        incoming.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer browser"),
        );
        incoming.insert(
            "traceparent",
            HeaderValue::from_static("00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"),
        );
        let headers = outbound_headers(
            &service(),
            "acme",
            &incoming,
            Some(&HeaderValue::from_static("idem-1")),
        )
        .unwrap();
        assert!(headers.get(header::COOKIE).is_none());
        assert!(headers.get(header::AUTHORIZATION).is_none());
        assert_eq!(headers.get(CUSTOMER_ORG_HEADER).unwrap(), "acme");
        assert_eq!(
            headers.get("x-fiducia-internal-auth").unwrap(),
            "internal-secret"
        );
        assert_eq!(headers.get(IDEMPOTENCY_KEY_HEADER).unwrap(), "idem-1");
        assert!(headers.get("traceparent").is_some());
    }

    #[test]
    fn outbound_headers_drop_invalid_browser_trace_context() {
        let mut incoming = HeaderMap::new();
        incoming.insert(
            "traceparent",
            HeaderValue::from_static("00-00000000000000000000000000000000-0123456789abcdef-01"),
        );
        incoming.insert("tracestate", HeaderValue::from_static("vendor=value"));
        let headers = outbound_headers(&service(), "acme", &incoming, None).unwrap();
        assert!(headers.get("traceparent").is_none());
        assert!(headers.get("tracestate").is_none());

        incoming.insert(
            "traceparent",
            HeaderValue::from_static("00-0123456789abcdef0123456789abcdef-0123456789abcdef-01"),
        );
        incoming.insert(
            "tracestate",
            HeaderValue::from_str(&"x".repeat(MAX_TRACESTATE_BYTES + 1)).unwrap(),
        );
        let headers = outbound_headers(&service(), "acme", &incoming, None).unwrap();
        assert!(headers.get("traceparent").is_some());
        assert!(headers.get("tracestate").is_none());
    }

    #[test]
    fn url_builder_encodes_path_segments() {
        let url = service_url(
            &service(),
            &["v1", "cron", "schedules", "daily rollup"],
            &[("limit", "50".to_string())],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://node.example/internal/v1/cron/schedules/daily%20rollup?limit=50"
        );
    }

    #[test]
    fn customer_function_policy_rejects_shell_runtime_and_entry_commands() {
        assert!(validated_function_body(json!({
            "slug": "unsafe",
            "runtime": "bash",
            "functionBody": "echo unsafe"
        }))
        .is_err());
        assert!(validated_function_body(json!({
            "slug": "unsafe",
            "runtime": "nodejs",
            "functionBody": "return true;",
            "entryCommand": "sh -c id"
        }))
        .is_err());
    }

    #[test]
    fn upstream_errors_are_reduced_to_safe_codes() {
        assert_eq!(
            sanitize_upstream_json(StatusCode::BAD_REQUEST, json!({"error":"invalid_schedule"})),
            json!({"ok":false,"error":"invalid_schedule"})
        );
        assert_eq!(
            sanitize_upstream_json(
                StatusCode::BAD_GATEWAY,
                json!({"error":"secret value with spaces"})
            ),
            json!({"ok":false,"error":"cron_upstream_rejected_request"})
        );
    }
}
