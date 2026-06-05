use std::collections::VecDeque;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub struct ExpectedRequest {
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    present_headers: Vec<String>,
    json_body: Option<Value>,
    response_status: StatusCode,
    response_json: Value,
}

#[derive(Debug)]
struct MockState {
    expected: Mutex<VecDeque<ExpectedRequest>>,
    errors: Mutex<Vec<String>>,
}

pub struct ProviderMock {
    base_url: String,
    state: Arc<MockState>,
    handle: JoinHandle<()>,
}

impl ExpectedRequest {
    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn post(path: &str) -> Self {
        Self::new(Method::POST, path)
    }

    fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_string(),
            query: Vec::new(),
            headers: Vec::new(),
            present_headers: Vec::new(),
            json_body: None,
            response_status: StatusCode::OK,
            response_json: serde_json::json!({}),
        }
    }

    pub fn query(mut self, key: &str, value: impl Into<String>) -> Self {
        self.query.push((key.to_string(), value.into()));
        self
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn header_present(mut self, name: &str) -> Self {
        self.present_headers.push(name.to_string());
        self
    }

    pub fn json_body(mut self, body: Value) -> Self {
        self.json_body = Some(body);
        self
    }

    pub fn respond_json(mut self, body: Value) -> Self {
        self.response_json = body;
        self
    }

    pub fn respond_status_json(mut self, status: StatusCode, body: Value) -> Self {
        self.response_status = status;
        self.response_json = body;
        self
    }
}

impl ProviderMock {
    pub async fn start(expected: Vec<ExpectedRequest>) -> Self {
        let state = Arc::new(MockState {
            expected: Mutex::new(expected.into()),
            errors: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/{*path}", any(handle))
            .fallback(any(handle))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        let handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!(error = %e, "provider mock server stopped");
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            state,
            handle,
        }
    }

    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    pub async fn assert_finished(&self) {
        let errors = self.state.errors.lock().await.clone();
        let remaining = self.state.expected.lock().await.len();
        assert!(
            errors.is_empty() && remaining == 0,
            "provider mock errors={errors:?} remaining={remaining}"
        );
    }
}

impl Drop for ProviderMock {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn handle(
    State(state): State<Arc<MockState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let expected = {
        let mut queue = state.expected.lock().await;
        queue.pop_front()
    };
    let Some(expected) = expected else {
        return record_error(&state, format!("unexpected request {method} {uri}")).await;
    };

    let mut errors = Vec::new();
    if method != expected.method {
        errors.push(format!("method expected {} got {method}", expected.method));
    }
    if uri.path() != expected.path {
        errors.push(format!(
            "path expected {} got {}",
            expected.path,
            uri.path()
        ));
    }

    let actual_query = parse_query(uri.query().unwrap_or(""));
    for (key, value) in &expected.query {
        if !actual_query
            .iter()
            .any(|(actual_key, actual_value)| actual_key == key && actual_value == value)
        {
            errors.push(format!(
                "missing query {key}={value}; actual={actual_query:?}"
            ));
        }
    }

    for (name, value) in &expected.headers {
        match headers.get(name).and_then(|v| v.to_str().ok()) {
            Some(actual) if actual == value => {}
            Some(actual) => errors.push(format!("header {name} expected {value:?} got {actual:?}")),
            None => errors.push(format!("missing header {name}")),
        }
    }

    for name in &expected.present_headers {
        if !headers.contains_key(name) {
            errors.push(format!("missing header {name}"));
        }
    }

    if let Some(expected_json) = &expected.json_body {
        match serde_json::from_slice::<Value>(&body) {
            Ok(actual) if actual == *expected_json => {}
            Ok(actual) => errors.push(format!("json body expected {expected_json} got {actual}")),
            Err(e) => errors.push(format!("json body decode failed: {e}")),
        }
    }

    if !errors.is_empty() {
        return record_error(&state, errors.join("; ")).await;
    }

    (expected.response_status, Json(expected.response_json)).into_response()
}

async fn record_error(state: &MockState, error: String) -> Response {
    state.errors.lock().await.push(error.clone());
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": error })),
    )
        .into_response()
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}
