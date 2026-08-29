use axum::{
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use prometheus::{Encoder, TextEncoder};

use crate::agents::{
    agents_tasks_body, agents_threads_body, AGENTS_TASKS_CSS, AGENTS_TASKS_JS, AGENTS_THREADS_CSS,
    AGENTS_THREADS_JS,
};
use crate::container_pool::{
    CONTAINER_POOL_CONFIG_BODY, CONTAINER_POOL_CONFIG_CSS, CONTAINER_POOL_CONFIG_JS,
};
use crate::home::home_document;
use crate::jello::{jello_document, jello_sample_markup, JelloSampleQuery};
use crate::lambda::{LAMBDA_FUNCTIONS_BODY, LAMBDA_FUNCTIONS_CSS, LAMBDA_FUNCTIONS_JS};
use crate::labs::{
    PRESENCE_TEST_BODY, PRESENCE_TEST_CSS, PRESENCE_TEST_JS, WSS_TEST_BODY, WSS_TEST_CSS,
    WSS_TEST_JS,
};
use crate::metrics::{record_request, STARTED_AT, UPTIME_SECONDS};
use crate::shared::{
    html_asset, inline_ui_document, text_asset, ui_document, SHARED_HEADER_CSS, SHARED_HEADER_JS,
};
use crate::state::{AppState, HealthResponse};

fn redirect_home() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/home"));
    response
}

pub(crate) async fn root() -> impl IntoResponse {
    record_request("GET", "/", StatusCode::FOUND);
    redirect_home()
}

pub(crate) async fn home(State(state): State<AppState>) -> impl IntoResponse {
    record_request("GET", "/home", StatusCode::OK);
    home_document(&state)
}

pub(crate) async fn jello_page() -> impl IntoResponse {
    record_request("GET", "/jello", StatusCode::OK);
    jello_document()
}

pub(crate) async fn jello_sample(Query(query): Query<JelloSampleQuery>) -> impl IntoResponse {
    record_request("GET", "/jello/sample", StatusCode::OK);
    Html(jello_sample_markup(query.product.as_deref()).into_string())
}

pub(crate) async fn api_docs_html() -> Html<&'static str> {
    record_request("GET", "/docs/api", StatusCode::OK);
    Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl IntoResponse {
    record_request("GET", "/api/docs.json", StatusCode::OK);
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

pub(crate) async fn api_docs_index_html() -> Html<&'static str> {
    record_request("GET", "/api-docs", StatusCode::OK);
    Html(include_str!("../../generated-api-docs-index.html"))
}

pub(crate) async fn api_docs_index_json() -> impl IntoResponse {
    record_request("GET", "/api-docs.json", StatusCode::OK);
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        include_str!("../../generated-api-docs-index.json"),
    )
}

pub(crate) async fn factmachine_markets_html() -> Html<&'static str> {
    record_request("GET", "/factmachine-markets", StatusCode::OK);
    Html(include_str!("../generated/factmachine-markets.html"))
}

pub(crate) async fn agents_tasks_page() -> impl IntoResponse {
    record_request("GET", "/agents/tasks", StatusCode::OK);
    ui_document(
        "dd agents tasks",
        "tasks",
        "#101417",
        "/assets/web-home/agents-tasks.css",
        "/assets/web-home/agents-tasks.js",
        agents_tasks_body(),
    )
}

pub(crate) async fn agents_threads_page() -> impl IntoResponse {
    record_request("GET", "/agents/threads", StatusCode::OK);
    ui_document(
        "dd agent threads",
        "threads",
        "#101417",
        "/assets/web-home/agents-threads.css",
        "/assets/web-home/agents-threads.js",
        agents_threads_body(),
    )
}

pub(crate) async fn agents_tasks_css() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-tasks.css",
        "text/css; charset=utf-8",
        AGENTS_TASKS_CSS,
    )
}

pub(crate) async fn agents_tasks_js() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-tasks.js",
        "text/javascript; charset=utf-8",
        AGENTS_TASKS_JS,
    )
}

pub(crate) async fn agents_tasks_html_fragment() -> impl IntoResponse {
    html_asset("/assets/web-home/agents-tasks.html", agents_tasks_body())
}

pub(crate) async fn agents_threads_css() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-threads.css",
        "text/css; charset=utf-8",
        AGENTS_THREADS_CSS,
    )
}

pub(crate) async fn agents_threads_js() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-threads.js",
        "text/javascript; charset=utf-8",
        AGENTS_THREADS_JS,
    )
}

pub(crate) async fn agents_threads_html_fragment() -> impl IntoResponse {
    html_asset(
        "/assets/web-home/agents-threads.html",
        agents_threads_body(),
    )
}

pub(crate) async fn shared_header_css() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/shared-header.css",
        "text/css; charset=utf-8",
        SHARED_HEADER_CSS,
    )
}

pub(crate) async fn shared_header_js() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/shared-header.js",
        "text/javascript; charset=utf-8",
        SHARED_HEADER_JS,
    )
}

pub(crate) async fn lambda_functions_page() -> impl IntoResponse {
    record_request("GET", "/lambdas/functions", StatusCode::OK);
    inline_ui_document(
        "dd lambda functions",
        "lambdas",
        LAMBDA_FUNCTIONS_CSS,
        LAMBDA_FUNCTIONS_BODY,
        LAMBDA_FUNCTIONS_JS,
    )
}

pub(crate) async fn presence_test_page() -> impl IntoResponse {
    record_request("GET", "/presence-test", StatusCode::OK);
    inline_ui_document(
        "presence test",
        "presence",
        PRESENCE_TEST_CSS,
        PRESENCE_TEST_BODY,
        PRESENCE_TEST_JS,
    )
}

pub(crate) async fn wss_test_page() -> impl IntoResponse {
    record_request("GET", "/wss-test", StatusCode::OK);
    inline_ui_document(
        "wss test lab",
        "wss",
        WSS_TEST_CSS,
        WSS_TEST_BODY,
        WSS_TEST_JS,
    )
}

const SHARED_SERVICE_WORKER_JS: &str = include_str!("../../../libs/browser/service-worker.js");

pub(crate) async fn service_worker_js() -> impl IntoResponse {
    record_request("GET", "/service-worker.js", StatusCode::OK);
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (
                header::HeaderName::from_static("service-worker-allowed"),
                "/",
            ),
        ],
        SHARED_SERVICE_WORKER_JS,
    )
}

pub(crate) async fn favicon() -> impl IntoResponse {
    record_request("GET", "/favicon.ico", StatusCode::NO_CONTENT);
    StatusCode::NO_CONTENT
}

pub(crate) async fn healthz() -> impl IntoResponse {
    record_request("GET", "/healthz", StatusCode::OK);
    Json(HealthResponse {
        ok: true,
        service: "dd-remote-web-home".to_string(),
        mode: "public-web".to_string(),
    })
}

pub(crate) async fn metrics() -> impl IntoResponse {
    record_request("GET", "/metrics", StatusCode::OK);
    UPTIME_SECONDS.set(STARTED_AT.elapsed().as_secs() as i64);

    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("failed to encode prometheus metrics");

    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        buffer,
    )
}

pub(crate) async fn container_pool_config_page() -> impl IntoResponse {
    record_request("GET", "/container-pool/config", StatusCode::OK);
    inline_ui_document(
        "dd container pool config",
        "container-pool-config",
        CONTAINER_POOL_CONFIG_CSS,
        CONTAINER_POOL_CONFIG_BODY,
        CONTAINER_POOL_CONFIG_JS,
    )
}
