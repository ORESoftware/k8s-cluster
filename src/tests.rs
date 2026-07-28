//! HTTP + security integration tests for the customer server. Extracted
//! verbatim from main.rs; `use super::*` gives access to every crate-root
//! handler and helper under test.

use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::delete;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt; // for `oneshot`

const ORG_A: &str = "00000000-0000-4000-8000-000000000001";
const ORG_B: &str = "00000000-0000-4000-8000-000000000002";
const KEY_ID: &str = "0123456789abcdef";

#[derive(Clone, Debug)]
struct CapturedAuthRequest {
    method: Method,
    path_and_query: String,
    authorization: Option<String>,
    idempotency_key: Option<String>,
    body: serde_json::Value,
}

#[derive(Clone, Default)]
struct MockAuthState {
    requests: Arc<Mutex<Vec<CapturedAuthRequest>>>,
}

fn mock_key_meta() -> serde_json::Value {
    json!({
        "key_id": KEY_ID,
        "org_id": ORG_B,
        "name": "Production webhooks",
        "scopes": ["requests:write"],
        "env": "live",
        "last_used_ms": null,
        "revoked": false,
        "version": 1,
        "require_idempotency": true,
        // The upstream contract must never include this, but even a drifted
        // response cannot pass it through the BFF's typed sanitizer.
        "secret_hash": "sha256:must-not-leak"
    })
}

async fn mock_auth_request(
    State(state): State<MockAuthState>,
    request: axum::extract::Request,
) -> Response {
    let method = request.method().clone();
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/")
        .to_string();
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let idempotency_key = request
        .headers()
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .expect("read mock auth request body");
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("mock auth request JSON")
    };
    state.requests.lock().unwrap().push(CapturedAuthRequest {
        method: method.clone(),
        path_and_query: path_and_query.clone(),
        authorization,
        idempotency_key,
        body,
    });

    let path = parts.uri.path();
    let mut rotated_meta = mock_key_meta();
    rotated_meta["version"] = json!(2);
    let response = match (method, path) {
        (Method::GET, "/v1/keys") => json!({ "keys": [mock_key_meta()] }),
        (Method::POST, "/v1/keys") => json!({
            "api_key": format!("fdc_live_{KEY_ID}.{}", "a".repeat(64)),
            "key": mock_key_meta()
        }),
        (Method::POST, "/v1/keys/0123456789abcdef/rotate") => json!({
            "ok": true,
            "api_key": format!("fdc_live_{KEY_ID}.{}", "b".repeat(64)),
            "key": rotated_meta,
            "secret_once": true,
            "overlap_seconds": 60
        }),
        (Method::DELETE, "/v1/keys/0123456789abcdef") => {
            json!({ "revoked": true })
        }
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "unexpected_mock_auth_route" })),
            )
                .into_response()
        }
    };
    Json(response).into_response()
}

async fn spawn_mock_auth() -> (String, MockAuthState, tokio::task::JoinHandle<()>) {
    let state = MockAuthState::default();
    let app = Router::new()
        .fallback(mock_auth_request)
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), state, server)
}

/// Create a throwaway `static/` dir with the minimum files the static
/// handler serves (home page, 404 fallback, a hashed asset).
fn temp_dir(prefix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

fn temp_static_dir() -> PathBuf {
    let dir = temp_dir("fiducia-site-test");
    std::fs::create_dir_all(dir.join("_astro")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>Fiducia</title><h1>home</h1>",
    )
    .unwrap();
    std::fs::write(
        dir.join("404.html"),
        "<!doctype html><title>Not found</title>no quorum on this page",
    )
    .unwrap();
    std::fs::write(dir.join("_astro/app.css"), "body{color:rebeccapurple}").unwrap();
    dir
}

fn test_config() -> AppConfig {
    // No pool: authenticated route tests exercise dependency failures without
    // inventing customer data or requiring a live Postgres/node deployment.
    AppConfig {
        static_dir: temp_static_dir(),
        customer_app_host: "app.fiducia.cloud".to_string(),
        customer_app_origin: None,
        customer_site_mode: false,
        supabase_url: None,
        supabase_publishable_key: None,
        auth_url: None,
        pool: None,
        // Tests exercise the handlers as an authenticated customer with a
        // fixed org; production uses `Authenticator::from_env()` (fail-closed).
        authenticator: Authenticator::Static(std::sync::Arc::new(CustomerCtx {
            user_id: "00000000-0000-4000-8000-000000000002".to_string(),
            email: Some("test@fiducia.cloud".to_string()),
            orgs: vec!["00000000-0000-0000-0000-000000000001".to_string()],
            aal: "aal2".to_string(),
            credential_binding: "authorization\0verified-supabase-session".to_string(),
            cookie_authenticated: false,
        })),
        request_security: RequestSecurity::new(
            "https://app.fiducia.cloud",
            b"0123456789abcdef0123456789abcdef".to_vec(),
        )
        .unwrap(),
        cron_services: CronServices::disabled(),
    }
}

/// A no-DB config with a chosen authenticator (for auth-gate tests).
fn config_with_auth(authenticator: Authenticator) -> AppConfig {
    AppConfig {
        authenticator,
        ..test_config()
    }
}

fn multi_org_config() -> AppConfig {
    config_with_auth(Authenticator::Static(Arc::new(CustomerCtx {
        user_id: "00000000-0000-4000-8000-000000000099".to_string(),
        email: Some("multi-org@fiducia.cloud".to_string()),
        orgs: vec![ORG_A.to_string(), ORG_B.to_string()],
        aal: "aal2".to_string(),
        credential_binding: "authorization\0verified-supabase-session".to_string(),
        cookie_authenticated: false,
    })))
}

async fn bff_request(
    config: AppConfig,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
    org_id: Option<&str>,
    idempotency_key: Option<&str>,
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "app.fiducia.cloud")
        .header(header::AUTHORIZATION, "Bearer verified-supabase-session");
    if let Some(org_id) = org_id {
        builder = builder.header(CUSTOMER_ORG_HEADER, org_id);
    }
    if let Some(idempotency_key) = idempotency_key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    build_router(config)
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn response_json(response: Response) -> (StatusCode, HeaderMap, serde_json::Value) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "response was not JSON ({error}): {}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, headers, body)
}

async fn post_json(config: AppConfig, uri: &str, body: &str) -> StatusCode {
    build_router(config)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::HOST, "app.fiducia.cloud")
                .header("content-type", "application/json")
                .header(IDEMPOTENCY_KEY_HEADER, "test-request-1")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

const CREATE_KEY_BODY: &str = r#"{"name":"k","environment":"live","scope":"requests:write"}"#;

#[test]
fn customer_origin_validation_accepts_one_exact_http_origin() {
    assert_eq!(
        parse_customer_app_origin("https://app.fiducia.cloud")
            .unwrap()
            .to_str()
            .unwrap(),
        "https://app.fiducia.cloud"
    );
    assert_eq!(
        parse_customer_app_origin("http://127.0.0.1:4173/")
            .unwrap()
            .to_str()
            .unwrap(),
        "http://127.0.0.1:4173"
    );
    for invalid in [
        "*",
        "app.fiducia.cloud",
        "https://app.fiducia.cloud/path",
        "https://app.fiducia.cloud?query=1",
        "https://user@app.fiducia.cloud",
        "https://app.fiducia.cloud,https://evil.example",
    ] {
        assert!(
            parse_customer_app_origin(invalid).is_err(),
            "accepted invalid origin {invalid}"
        );
    }
}

#[test]
fn org_selection_is_explicit_and_membership_checked() {
    let config = multi_org_config();
    let Authenticator::Static(ctx) = &config.authenticator else {
        panic!("multi-org test config must use static auth");
    };

    assert_eq!(
        selected_customer_org(ctx, &HeaderMap::new())
            .unwrap_err()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let mut selected = HeaderMap::new();
    selected.insert(CUSTOMER_ORG_HEADER, HeaderValue::from_static(ORG_B));
    assert_eq!(selected_customer_org(ctx, &selected).unwrap(), ORG_B);

    selected.insert(
        CUSTOMER_ORG_HEADER,
        HeaderValue::from_static("00000000-0000-4000-8000-000000000003"),
    );
    assert_eq!(
        selected_customer_org(ctx, &selected).unwrap_err().status(),
        StatusCode::FORBIDDEN
    );

    selected.insert(
        CUSTOMER_ORG_HEADER,
        HeaderValue::from_static("org with whitespace"),
    );
    assert_eq!(
        selected_customer_org(ctx, &selected).unwrap_err().status(),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn auth_key_sanitizer_checks_org_wire_shape_and_scopes() {
    let key: AuthKeyMeta = serde_json::from_value(mock_key_meta()).unwrap();
    let display = auth_key_to_display(&key, ORG_B).unwrap();
    assert_eq!(display["prefix"], format!("fdc_live_{KEY_ID}"));
    assert!(display.get("secret_hash").is_none());
    assert!(auth_key_to_display(&key, ORG_A).is_err());
    assert!(raw_api_key_matches(
        &format!("fdc_live_{KEY_ID}.{}", "a".repeat(64)),
        &key
    ));
    assert!(!raw_api_key_matches(
        &format!("fdc_live_{KEY_ID}.short"),
        &key
    ));
}

#[tokio::test]
async fn cors_allows_only_the_configured_customer_origin_and_headers() {
    let mut config = test_config();
    config.customer_app_origin =
        Some(parse_customer_app_origin("https://app.fiducia.cloud").unwrap());
    let preflight = |origin: &'static str| {
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/customer/api-keys")
            .header(header::HOST, "app.fiducia.cloud")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,content-type,idempotency-key,x-fiducia-csrf,x-fiducia-org-id",
            )
            .body(Body::empty())
            .unwrap()
    };

    let allowed = build_router(config.clone())
        .oneshot(preflight("https://app.fiducia.cloud"))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        allowed
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "https://app.fiducia.cloud"
    );
    let allowed_headers = allowed
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
        .unwrap()
        .to_str()
        .unwrap();
    for required in [
        "authorization",
        "content-type",
        IDEMPOTENCY_KEY_HEADER,
        CUSTOMER_CSRF_HEADER,
        CUSTOMER_ORG_HEADER,
    ] {
        assert!(allowed_headers.contains(required), "missing {required}");
    }

    let denied = build_router(config)
        .oneshot(preflight("https://evil.example"))
        .await
        .unwrap();
    assert_eq!(
        denied
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "https://app.fiducia.cloud",
        "the fixed allow-origin must never reflect a foreign Origin"
    );
}

#[tokio::test]
async fn customer_key_bff_is_org_scoped_sanitized_and_forwards_idempotency() {
    let (auth_url, state, server) = spawn_mock_auth().await;
    let mut config = multi_org_config();
    config.auth_url = Some(auth_url);

    let context = bff_request(
        config.clone(),
        Method::GET,
        "/api/customer/context",
        None,
        None,
        None,
    )
    .await;
    let (status, headers, body) = response_json(context).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(body["user"]["orgs"].as_array().unwrap().len(), 2);

    let missing_org = bff_request(
        config.clone(),
        Method::GET,
        "/api/customer/api-keys",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(missing_org.status(), StatusCode::BAD_REQUEST);

    let listed = bff_request(
        config.clone(),
        Method::GET,
        "/api/customer/api-keys",
        None,
        Some(ORG_B),
        None,
    )
    .await;
    let (status, headers, body) = response_json(listed).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(body["api_keys"][0]["prefix"], format!("fdc_live_{KEY_ID}"));
    assert!(body.to_string().find("secret_hash").is_none());

    let create_body = json!({
        "name": "Production webhooks",
        "environment": "live",
        "scope": "requests:write",
        "require_idempotency": true
    });
    let missing_idempotency = bff_request(
        config.clone(),
        Method::POST,
        "/api/customer/api-keys",
        Some(create_body.clone()),
        Some(ORG_B),
        None,
    )
    .await;
    let (status, _, body) = response_json(missing_idempotency).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "idempotency_key_required");

    let created = bff_request(
        config.clone(),
        Method::POST,
        "/api/customer/api-keys",
        Some(create_body),
        Some(ORG_B),
        Some("customer-create-key-1"),
    )
    .await;
    let (status, headers, body) = response_json(created).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(headers.get(header::PRAGMA).unwrap(), "no-cache");
    assert!(body["secret"].as_str().unwrap().starts_with("fdc_live_"));
    assert!(body.to_string().find("secret_hash").is_none());

    let rotated = bff_request(
        config.clone(),
        Method::POST,
        "/api/customer/api-keys/rotate",
        Some(json!({ "prefix": format!("fdc_live_{KEY_ID}") })),
        Some(ORG_B),
        Some("customer-rotate-key-1"),
    )
    .await;
    let (status, headers, body) = response_json(rotated).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(body["api_key"]["version"], 2);
    assert_eq!(body["overlap_seconds"], 60);

    let revoked = bff_request(
        config.clone(),
        Method::POST,
        "/api/customer/api-keys/revoke",
        Some(json!({ "prefix": format!("fdc_live_{KEY_ID}") })),
        Some(ORG_B),
        Some("customer-revoke-key-1"),
    )
    .await;
    let (status, headers, body) = response_json(revoked).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(body["status"], "revoked");

    let catchup = bff_request(
        config,
        Method::GET,
        "/api/customer/sync/api_keys?since=99",
        None,
        Some(ORG_B),
        None,
    )
    .await;
    let (status, headers, body) = response_json(catchup).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(body["snapshot"], true);
    assert_eq!(body["requested_since"], 99);
    assert!(body.to_string().find("secret_hash").is_none());

    let requests = state.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(
        |request| request.authorization.as_deref() == Some("Bearer verified-supabase-session")
    ));
    let create = requests
        .iter()
        .find(|request| request.method == Method::POST && request.path_and_query == "/v1/keys")
        .unwrap();
    assert_eq!(
        create.idempotency_key.as_deref(),
        Some("customer-create-key-1")
    );
    assert_eq!(create.body["org_id"], ORG_B);
    let rotate = requests
        .iter()
        .find(|request| request.path_and_query.contains("/rotate"))
        .unwrap();
    assert_eq!(
        rotate.idempotency_key.as_deref(),
        Some("customer-rotate-key-1")
    );
    assert!(rotate.path_and_query.ends_with(&format!("org_id={ORG_B}")));
    let revoke = requests
        .iter()
        .find(|request| request.method == Method::DELETE)
        .unwrap();
    assert_eq!(
        revoke.idempotency_key.as_deref(),
        Some("customer-revoke-key-1")
    );
    assert!(revoke.path_and_query.ends_with(&format!("org_id={ORG_B}")));
    assert!(requests
        .iter()
        .filter(|request| request.method == Method::GET)
        .all(|request| request.path_and_query.ends_with(&format!("org_id={ORG_B}"))));
    server.abort();
}

#[tokio::test]
async fn server_managed_customer_cookie_is_forwarded_to_the_key_authority() {
    let (auth_url, state, server) = spawn_mock_auth().await;
    let mut config = multi_org_config();
    config.auth_url = Some(auth_url);
    let response = build_router(config)
        .oneshot(
            Request::builder()
                .uri("/api/customer/api-keys")
                .header(header::HOST, "app.fiducia.cloud")
                .header(
                    header::COOKIE,
                    format!("{CUSTOMER_SESSION_COOKIE}=cookie-supabase-session"),
                )
                .header(CUSTOMER_ORG_HEADER, ORG_B)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer cookie-supabase-session")
    );
    server.abort();
}

#[tokio::test]
async fn unauthenticated_customer_mutations_fail_closed() {
    // No auth backend configured → every /api/customer mutation is denied (503),
    // closing the pre-fix hole where anyone could mint a live API key.
    let deny = || config_with_auth(Authenticator::Deny);
    assert_eq!(
        post_json(deny(), "/api/customer/api-keys", CREATE_KEY_BODY).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        post_json(
            deny(),
            "/api/customer/api-keys/rotate",
            r#"{"prefix":"fid_live_x"}"#
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // The catch-up endpoint is GET-only; mutation attempts never reach auth.
    assert_eq!(
        post_json(deny(), "/api/customer/sync/api_keys", "{}").await,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

#[tokio::test]
async fn authenticated_customer_without_database_fails_closed() {
    let status = post_json(test_config(), "/api/customer/api-keys", CREATE_KEY_BODY).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// Send a GET through the router and return (status, content-type, body).
async fn send(uri: &str) -> (StatusCode, String, String) {
    send_with_host(uri, None).await
}

async fn send_with_host(uri: &str, host: Option<&str>) -> (StatusCode, String, String) {
    let app = build_router(test_config());
    let default_host =
        if uri.starts_with("/app") || uri.starts_with("/login") || uri.starts_with("/api/customer")
        {
            "app.fiducia.cloud"
        } else {
            "www.fiducia.cloud"
        };
    let builder = Request::builder()
        .uri(uri)
        .header(header::HOST, host.unwrap_or(default_host));
    let resp = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, ct, String::from_utf8_lossy(&bytes).into_owned())
}

async fn spawn_mock(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), task)
}

/// The billing webhook route is mounted OUTSIDE `/api/customer`, so it is not
/// blocked by the session/CSRF/AAL gates — but it must fail closed on its own:
/// an unknown provider is a 404 before any state is touched, and a known
/// provider still never reaches the ledger without a verifiable signature.
#[tokio::test]
async fn billing_webhook_fails_closed() {
    // Unknown provider: rejected at the path, no config/env/DB touched.
    let resp = build_router(test_config())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/billing/webhooks/venmo")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unknown provider must 404"
    );

    // Known provider, but no valid signature: whether the signing secret is
    // configured in this environment or not, the outcome is a rejection
    // (503 provider_not_configured, or 400 signature_verification_failed) —
    // never a 2xx that would let an unverified body be recorded.
    let resp = build_router(test_config())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/billing/webhooks/stripe")
                .body(Body::from(r#"{"id":"evt_1","type":"invoice.paid"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE | StatusCode::BAD_REQUEST
        ),
        "a stripe webhook without a valid signature must be rejected, got {}",
        resp.status(),
    );
}

#[tokio::test]
async fn customer_login_is_server_mediated_and_issues_only_customer_cookie() {
    const MOCK_SUPABASE_TOKEN_PATH: &str = "/auth/v1/token";
    const MOCK_AUTH_ME_PATH: &str = "/v1/me";
    let supabase = Router::new()
        .route(
            MOCK_SUPABASE_TOKEN_PATH,
            axum::routing::post(|| async { Json(json!({ "access_token": "customer.jwt" })) }),
        )
        // The login now runs a fail-closed MFA factor lookup before issuing a
        // session; this account has no enrolled factor, so login finalizes.
        .route(
            "/auth/v1/user",
            get(|| async { Json(json!({ "factors": [] })) }),
        );
    let auth = Router::new().route(
        MOCK_AUTH_ME_PATH,
        get(|| async {
            Json(json!({
                "user": {
                    "user_id": "00000000-0000-4000-8000-000000000002",
                    "email": "customer@example.com",
                    "orgs": ["00000000-0000-4000-8000-000000000001"],
                    "roles": []
                }
            }))
        }),
    );
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let (auth_url, auth_task) = spawn_mock(auth).await;
    let mut config = test_config();
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("public-publishable-key".to_string());
    config.authenticator = Authenticator::AuthService(auth_url);
    let app = build_router(config);
    let login_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .header(header::HOST, "app.fiducia.cloud")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_page.status(), StatusCode::OK);
    let login_cookie = login_page
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|value| {
            let value = value.to_str().ok()?;
            value
                .starts_with(&format!("{CUSTOMER_LOGIN_CSRF_COOKIE}="))
                .then(|| value.split(';').next().unwrap().to_string())
        })
        .unwrap();
    let login_html = String::from_utf8(
        axum::body::to_bytes(login_page.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let csrf_field = &login_html[login_html.find("name=\"csrf_token\"").unwrap()..];
    let csrf_value = &csrf_field[csrf_field.find("value=\"").unwrap() + 7..];
    let csrf_value = &csrf_value[..csrf_value.find('"').unwrap()];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, login_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf_value}&email=customer%40example.com&password=correct"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/app");
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect::<Vec<_>>();
    assert!(cookies
        .iter()
        .any(|cookie| cookie.starts_with(&format!("{CUSTOMER_SESSION_COOKIE}=customer.jwt"))));
    assert!(cookies.iter().all(|cookie| cookie.contains("HttpOnly")));
    assert!(cookies
        .iter()
        .any(|cookie| cookie.starts_with(&format!("{CUSTOMER_LOGIN_CSRF_COOKIE}=;"))));
    assert!(cookies
        .iter()
        .all(|cookie| !cookie.contains("fiducia_admin_session")));
    supabase_task.abort();
    auth_task.abort();
}

// Regression: a password login by an account with a verified TOTP factor must
// be forced into aal2 step-up (parked on the MFA challenge with the pending
// cookie), NEVER handed a session cookie. Without the factor lookup in
// `customer_login_submit`, the always-rendered password form silently
// bypasses the second factor.
#[tokio::test]
async fn customer_password_login_with_verified_totp_factor_forces_step_up() {
    let supabase = Router::new()
        .route(
            "/auth/v1/token",
            axum::routing::post(|| async { Json(json!({ "access_token": "customer.jwt" })) }),
        )
        .route(
            "/auth/v1/user",
            get(|| async {
                Json(json!({
                    "factors": [
                        { "id": "factor-1", "factor_type": "totp", "status": "verified" }
                    ]
                }))
            }),
        )
        .route(
            "/auth/v1/factors/:id/challenge",
            axum::routing::post(|| async { Json(json!({ "id": "challenge-1" })) }),
        );
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let mut config = test_config();
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("public-publishable-key".to_string());
    // The step-up path never reaches /v1/me, so a dead authenticator is fine.
    config.authenticator = Authenticator::AuthService("http://127.0.0.1:1".to_string());
    let app = build_router(config);

    let login_page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .header(header::HOST, "app.fiducia.cloud")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let login_cookie = login_page
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|value| {
            let value = value.to_str().ok()?;
            value
                .starts_with(&format!("{CUSTOMER_LOGIN_CSRF_COOKIE}="))
                .then(|| value.split(';').next().unwrap().to_string())
        })
        .unwrap();
    let login_html = String::from_utf8(
        axum::body::to_bytes(login_page.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let csrf_field = &login_html[login_html.find("name=\"csrf_token\"").unwrap()..];
    let csrf_value = &csrf_field[csrf_field.find("value=\"").unwrap() + 7..];
    let csrf_value = &csrf_value[..csrf_value.find('"').unwrap()];

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, login_cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf_value}&email=customer%40example.com&password=correct"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    // Step-up page, not a redirect into the app.
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect::<Vec<_>>();
    // The interim aal1 token rides the pending cookie...
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with(&format!("{CUSTOMER_MFA_PENDING_COOKIE}="))),
        "expected the MFA pending cookie to be set, got {cookies:?}"
    );
    // ...and NO app session cookie is issued before the second factor.
    assert!(
            cookies
                .iter()
                .all(|cookie| !cookie.starts_with(&format!("{CUSTOMER_SESSION_COOKIE}="))),
            "a verified-TOTP account must not receive a session cookie from the password form: {cookies:?}"
        );
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(
        body.contains("/login/mfa"),
        "step-up challenge form should target /login/mfa"
    );
    supabase_task.abort();
}

// H13 regression: C1 protects the password handler, but a pre-existing or
// future aal1 session must also be stopped at the authenticated router
// boundary when Supabase reports a verified factor for that account.
#[tokio::test]
async fn aal1_session_with_a_verified_factor_is_rejected_on_customer_routes() {
    let auth = Router::new().route(
        "/v1/me",
        get(|| async {
            Json(json!({
                "user": {
                    "user_id": "00000000-0000-4000-8000-000000000002",
                    "email": "customer@example.com",
                    "orgs": [ORG_A],
                    "roles": [],
                    "aal": "aal1"
                }
            }))
        }),
    );
    let supabase = Router::new().route(
        "/auth/v1/user",
        get(|| async {
            Json(json!({
                "factors": [
                    { "id": "factor-1", "factor_type": "totp", "status": "verified" }
                ]
            }))
        }),
    );
    let (auth_url, auth_task) = spawn_mock(auth).await;
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let mut config = test_config();
    config.authenticator = Authenticator::AuthService(auth_url);
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("publishable-test-key".to_string());

    let response = build_router(config)
        .oneshot(
            Request::builder()
                .uri("/api/customer/context")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::AUTHORIZATION, "Bearer aal1-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("mfa_step_up_required"));
    auth_task.abort();
    supabase_task.abort();
}

#[tokio::test]
async fn mfa_activation_rotates_the_app_cookie_to_the_stepped_up_session() {
    let supabase = Router::new()
        .route(
            "/auth/v1/user",
            get(|| async {
                Json(json!({
                    "factors": [
                        { "id": "factor-1", "factor_type": "totp", "status": "unverified" }
                    ]
                }))
            }),
        )
        .route(
            "/auth/v1/factors/factor-1/challenge",
            post(|| async { Json(json!({ "id": "challenge-1" })) }),
        )
        .route(
            "/auth/v1/factors/factor-1/verify",
            post(|| async { Json(json!({ "access_token": "fresh-aal2-session" })) }),
        );
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let customer = CustomerCtx {
        user_id: "00000000-0000-4000-8000-000000000002".to_string(),
        email: Some("customer@example.com".to_string()),
        orgs: vec![ORG_A.to_string()],
        aal: "aal1".to_string(),
        credential_binding: "authorization\0old-aal1-session".to_string(),
        cookie_authenticated: false,
    };
    let mut config = test_config();
    let csrf = customer_csrf_token(&config, &customer);
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("publishable-test-key".to_string());
    config.authenticator = Authenticator::Static(Arc::new(customer));

    let response = build_router(config)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/app/security/mfa/activate")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::AUTHORIZATION, "Bearer old-aal1-session")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf}&factor_id=factor-1&code=123456"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("fiducia_customer_session=fresh-aal2-session;")),
        "activation must replace the aal1 app cookie with Supabase's aal2 token"
    );
    supabase_task.abort();
}

// H14 regression: a single-factor context must never remove an MFA factor,
// even if a future router refactor accidentally bypasses the global gate.
#[tokio::test]
async fn mfa_disable_requires_aal2_at_the_handler_boundary() {
    let supabase = Router::new().route(
        "/auth/v1/user",
        get(|| async { Json(json!({ "factors": [] })) }),
    );
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let mut config = test_config();
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("publishable-test-key".to_string());
    config.authenticator = Authenticator::Static(Arc::new(CustomerCtx {
        user_id: "00000000-0000-4000-8000-000000000002".to_string(),
        email: Some("customer@example.com".to_string()),
        orgs: vec![ORG_A.to_string()],
        aal: "aal1".to_string(),
        credential_binding: "authorization\0aal1-session".to_string(),
        cookie_authenticated: false,
    }));

    let response = build_router(config)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/app/security/mfa/disable")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::AUTHORIZATION, "Bearer aal1-session")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(
                    "csrf_token=unused&factor_id=factor-1&code=123456",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("mfa_step_up_required"));
    supabase_task.abort();
}

#[tokio::test]
async fn mfa_disable_never_unenrolls_without_a_fresh_totp_verification() {
    let delete_calls = Arc::new(AtomicUsize::new(0));
    let deleted = delete_calls.clone();
    // Deliberately omit the challenge/verify routes. A password attacker who
    // has an old aal2 session but no current TOTP code must not reach DELETE.
    let supabase = Router::new().route(
        "/auth/v1/factors/factor-1",
        delete(move || {
            let deleted = deleted.clone();
            async move {
                deleted.fetch_add(1, Ordering::Relaxed);
                StatusCode::OK
            }
        }),
    );
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let customer = CustomerCtx {
        user_id: "00000000-0000-4000-8000-000000000002".to_string(),
        email: Some("customer@example.com".to_string()),
        orgs: vec![ORG_A.to_string()],
        aal: "aal2".to_string(),
        credential_binding: "authorization\0old-aal2-session".to_string(),
        cookie_authenticated: false,
    };
    let mut config = test_config();
    let csrf = customer_csrf_token(&config, &customer);
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("publishable-test-key".to_string());
    config.authenticator = Authenticator::Static(Arc::new(customer));

    let response = build_router(config)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/app/security/mfa/disable")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::AUTHORIZATION, "Bearer old-aal2-session")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf}&factor_id=factor-1&code=123456"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);
    assert_eq!(
        delete_calls.load(Ordering::Relaxed),
        0,
        "unenrollment requires a current TOTP challenge and verification"
    );
    supabase_task.abort();
}

#[tokio::test]
async fn mfa_disable_uses_the_freshly_verified_session_for_unenrollment() {
    let delete_authorization = Arc::new(Mutex::new(None));
    let captured_authorization = delete_authorization.clone();
    let supabase = Router::new()
        .route(
            "/auth/v1/factors/factor-1/challenge",
            post(|| async { Json(json!({ "id": "challenge-1" })) }),
        )
        .route(
            "/auth/v1/factors/factor-1/verify",
            post(|| async { Json(json!({ "access_token": "fresh-aal2-session" })) }),
        )
        .route(
            "/auth/v1/factors/factor-1",
            delete(move |headers: HeaderMap| {
                let captured_authorization = captured_authorization.clone();
                async move {
                    *captured_authorization.lock().unwrap() = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    StatusCode::OK
                }
            }),
        );
    let (supabase_url, supabase_task) = spawn_mock(supabase).await;
    let customer = CustomerCtx {
        user_id: "00000000-0000-4000-8000-000000000002".to_string(),
        email: Some("customer@example.com".to_string()),
        orgs: vec![ORG_A.to_string()],
        aal: "aal2".to_string(),
        credential_binding: "authorization\0old-aal2-session".to_string(),
        cookie_authenticated: false,
    };
    let mut config = test_config();
    let csrf = customer_csrf_token(&config, &customer);
    config.supabase_url = Some(supabase_url);
    config.supabase_publishable_key = Some("publishable-test-key".to_string());
    config.authenticator = Authenticator::Static(Arc::new(customer));

    let response = build_router(config)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/app/security/mfa/disable")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::AUTHORIZATION, "Bearer old-aal2-session")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "csrf_token={csrf}&factor_id=factor-1&code=123456"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        delete_authorization.lock().unwrap().as_deref(),
        Some("Bearer fresh-aal2-session")
    );
    supabase_task.abort();
}

#[tokio::test]
async fn customer_pages_redirect_missing_sessions_to_customer_login() {
    for uri in ["/", "/app"] {
        let mut config = test_config();
        config.authenticator = Authenticator::AuthService("http://127.0.0.1:1".to_string());
        let response = build_router(config)
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::HOST, "app.fiducia.cloud")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{uri}");
        assert_eq!(
            response.headers().get("location").unwrap(),
            "/login",
            "{uri}"
        );
    }
}

async fn send_json(
    method: &str,
    uri: &str,
    payload: serde_json::Value,
) -> (StatusCode, String, String) {
    let app = build_router(test_config());
    let resp = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::CONTENT_TYPE, "application/json")
                .header(IDEMPOTENCY_KEY_HEADER, "test-request-1")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, ct, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn healthz_and_api_health_report_ok() {
    for uri in ["/healthz", "/api/health"] {
        let (status, _ct, body) = send(uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok", "{uri}");
        assert_eq!(v["service"], "fiducia-backend", "{uri}");
    }
}

#[tokio::test]
async fn api_info_describes_the_website_tier() {
    let (status, ct, body) = send("/api/info").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["service"], "fiducia-backend");
    assert_eq!(v["domain"], "fiducia.cloud");
    assert_eq!(v["role"], "website");
    assert_eq!(v["customer_portal"]["host"], "app.fiducia.cloud");
    assert_eq!(v["customer_portal"]["path"], "/app");
    assert_eq!(v["customer_portal"]["rendering"], "maud+htmx");
    assert_eq!(v["customer_portal"]["streams"]["websocket"], "/app/ws");
    assert_eq!(v["customer_portal"]["streams"]["sse"], "/app/events");
    assert_eq!(v["customer_portal"]["supabase_login"], false);
    assert_eq!(v["components"]["data_plane"], "fiducia-node");
    assert_eq!(v["components"]["control_plane"], "fiducia-brain");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn customer_api_keys_require_database() {
    let (status, _, _) = send("/api/customer/api-keys").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _, _) = send_json(
        "POST",
        "/api/customer/api-keys",
        json!({
            "name": "Production webhooks",
            "environment": "live",
            "scope": "requests:write",
            "require_idempotency": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn customer_api_key_creation_validates_input() {
    let (status, _ct, body) = send_json(
        "POST",
        "/api/customer/api-keys",
        json!({
            "name": "",
            "environment": "live",
            "scope": "requests:write",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"], "name_required");

    let (status, _ct, body) = send_json(
        "POST",
        "/api/customer/api-keys",
        json!({
            "name": "bad scope",
            "environment": "live",
            "scope": "root",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"], "invalid_scope");
}

#[tokio::test]
async fn customer_preferences_require_database_and_validate_before_io() {
    let (status, _, _) = send("/api/customer/preferences").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _, _) = send_json(
        "PUT",
        "/api/customer/preferences",
        json!({
            "region": "iad1",
            "timezone": "utc",
            "density": "compact",
            "notify_lock_contention": false,
            "notify_key_rotation": true,
            "notify_mfa": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _ct, body) = send_json(
        "PUT",
        "/api/customer/preferences",
        json!({
            "region": "moon",
            "timezone": "utc",
            "density": "compact",
            "notify_lock_contention": false,
            "notify_key_rotation": true,
            "notify_mfa": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let rejected: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rejected["error"], "invalid_region");
}

#[tokio::test]
async fn customer_security_sessions_require_database() {
    let (status, _, _) = send("/api/customer/security/sessions").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _, _) = send_json(
        "POST",
        "/api/customer/security/sessions/revoke",
        json!({ "device": "Safari on iPhone" }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn customer_activity_requires_database() {
    // Auth is static in this unit test, but the activity route must still
    // fail closed rather than inventing an empty tenant audit feed.
    let (status, _, _) = send("/api/customer/activity?limit=0").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn docs_api_and_alias_serve_html() {
    for uri in ["/docs/api", "/api/docs"] {
        let (status, ct, body) = send(uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(ct.contains("text/html"), "{uri} ct={ct}");
        assert!(body.contains("fiducia-customer.rs API docs"), "{uri}");
    }
}

#[tokio::test]
async fn api_docs_json_is_machine_readable() {
    let (status, ct, body) = send("/api/docs.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("application/json"), "ct={ct}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["service"], "fiducia-customer.rs");
    let routes = v["routes"].as_array().unwrap();
    assert_eq!(v["routeCount"].as_u64().unwrap() as usize, routes.len());
    assert!(
        routes.len() >= 30,
        "generated inventory is unexpectedly incomplete"
    );
    for path in [
        "/api/customer/api-keys",
        "/api/customer/api-keys/rotate",
        "/api/customer/api-keys/revoke",
        "/api/customer/preferences",
        "/api/customer/security/sessions",
        "/api/customer/sync/:table",
        "/app/ws",
        "/app/events",
    ] {
        assert!(
            routes.iter().any(|route| route["path"] == path),
            "generated API inventory is missing {path}"
        );
    }
    for removed in ["/app/kv", "/app/locks", "/app/requests", "/app/services"] {
        assert!(
            routes.iter().all(|route| route["path"] != removed),
            "generated API inventory retained removed route {removed}"
        );
    }
    let standard = v["standardDocsRoutes"].as_array().unwrap();
    for r in ["/docs/api", "/api/docs", "/api/docs.json"] {
        assert!(
            standard.iter().any(|x| x == r),
            "missing {r} in standardDocsRoutes"
        );
    }
}

#[tokio::test]
async fn diagram_route_serves_html() {
    let (status, ct, body) = send("/docs/diagram").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/html"), "ct={ct}");
    assert!(body.contains("AI-agent fleets"));
    assert!(body.contains("durable brain-Raft: target HA step"));
}

#[tokio::test]
async fn root_serves_the_static_index() {
    let (status, ct, body) = send("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/html"), "ct={ct}");
    assert!(body.contains("home"));
}

#[tokio::test]
async fn app_host_root_serves_the_customer_portal() {
    let (status, ct, body) = send_with_host("/", Some("app.fiducia.cloud")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/html"), "ct={ct}");
    assert!(body.contains("Fiducia Customer Portal"));
    assert!(body.contains("/assets/htmx.min.js"));
    assert!(!body.contains("/_customer/"));
}

#[tokio::test]
async fn app_route_serves_the_customer_portal() {
    let (status, ct, body) = send("/app").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/html"), "ct={ct}");
    assert!(body.contains("Account posture, API access"));
    assert!(body.contains("Supabase Auth"));
    assert!(body.contains("verified"));
    assert!(body.contains("test@fiducia.cloud"));
    assert!(body.contains("API Keys"));
    assert!(body.contains("Supabase-managed"));
    assert!(body.contains("Preferences"));
    assert!(body.contains("operator infrastructure controls live only in the admin app"));
    assert!(!body.contains("Config KV"));
    assert!(!body.contains("Service Discovery"));
}

#[tokio::test]
async fn customer_account_routes_render_customer_controls() {
    let cases = [
        ("/app/auth", "Verified by fiducia-auth"),
        ("/app/signup", "Organization membership"),
        ("/app/api-keys", "Create API key"),
        ("/app/security", "provider managed"),
        ("/app/activity", "Organization activity"),
        ("/app/settings", "Loading persisted preferences"),
        ("/app/preferences", "Loading persisted preferences"),
    ];

    for (uri, needle) in cases {
        let (status, ct, body) = send(uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(ct.contains("text/html"), "{uri} ct={ct}");
        assert!(body.contains(needle), "{uri} missing {needle}");
    }
}

#[tokio::test]
async fn operator_pages_are_absent_from_the_customer_app() {
    for uri in [
        "/app/locks",
        "/app/requests",
        "/app/kv",
        "/app/services",
        "/app/fragments/locks",
    ] {
        let (status, _, body) = send(uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(body.contains("no quorum on this page"), "{uri}");
        assert!(!body.contains("operator"), "{uri}");
    }
}

#[tokio::test]
async fn customer_sse_stream_is_available() {
    let app = build_router(test_config());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/events")
                .header(header::HOST, "app.fiducia.cloud")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text/event-stream"), "ct={ct}");
}

#[tokio::test]
async fn customer_sync_is_read_only() {
    let resp = build_router(test_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/customer/sync/api_keys")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn customer_websocket_route_requires_upgrade() {
    let app = build_router(test_config());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/ws")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::ORIGIN, "https://app.fiducia.cloud")
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn static_asset_served_with_correct_mime() {
    let (status, ct, body) = send("/_astro/app.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/css"), "ct={ct}");
    assert!(body.contains("rebeccapurple"));
}

#[tokio::test]
async fn customer_asset_served_with_correct_mime() {
    let (status, ct, body) = send("/assets/htmx.min.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        ct.contains("text/javascript") || ct.contains("application/javascript"),
        "ct={ct}"
    );
    assert!(body.contains("htmx"));
}

#[tokio::test]
async fn unknown_path_falls_back_to_the_404_page() {
    // SPA-style fallback: the styled 404 page is served (ServeFile returns 200).
    let (status, _ct, body) = send("/does/not/exist").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("no quorum on this page"));
}

#[test]
fn release_cookie_policy_ignores_the_insecure_escape_hatch() {
    assert_eq!(cookie_secure_suffix_for(true, true), "; Secure");
    assert_eq!(cookie_secure_suffix_for(true, false), "; Secure");
    assert_eq!(cookie_secure_suffix_for(false, false), "; Secure");
    assert_eq!(cookie_secure_suffix_for(false, true), "");
}

#[tokio::test]
async fn cookie_mutations_require_exact_origin_and_bound_csrf() {
    let customer = CustomerCtx {
        user_id: "00000000-0000-4000-8000-000000000002".to_string(),
        email: Some("cookie@fiducia.cloud".to_string()),
        orgs: vec![ORG_A.to_string()],
        aal: "aal2".to_string(),
        credential_binding: "cookie\0customer.jwt".to_string(),
        cookie_authenticated: true,
    };
    let mut config = test_config();
    let csrf = customer_csrf_token(&config, &customer);
    config.authenticator = Authenticator::Static(Arc::new(customer));
    let app = build_router(config);
    let request = |origin: &'static str, csrf: Option<&str>| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/customer/api-keys")
            .header(header::HOST, "app.fiducia.cloud")
            .header(header::ORIGIN, origin)
            .header("sec-fetch-site", "same-origin")
            .header(
                header::COOKIE,
                format!("{CUSTOMER_SESSION_COOKIE}=customer.jwt"),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .header(IDEMPOTENCY_KEY_HEADER, "cookie-create-1");
        if let Some(csrf) = csrf {
            builder = builder.header(CUSTOMER_CSRF_HEADER, csrf);
        }
        builder
            .body(Body::from(
                json!({ "name": "", "environment": "live", "scope": "requests:write" }).to_string(),
            ))
            .unwrap()
    };

    let missing = app
        .clone()
        .oneshot(request("https://app.fiducia.cloud", None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let sibling = app
        .clone()
        .oneshot(request("https://admin.fiducia.cloud", Some(&csrf)))
        .await
        .unwrap();
    assert_eq!(sibling.status(), StatusCode::FORBIDDEN);

    let accepted = app
        .oneshot(request("https://app.fiducia.cloud", Some(&csrf)))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn portal_served_at_root_on_app_host_is_hardened_like_app() {
    // The authenticated dashboard is reachable at both `/app` and `/` (on the
    // customer app host). It carries the user's email, org ids, and CSRF token,
    // so the root path must be just as no-store / strict-CSP as `/app`.
    let response = build_router(test_config())
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::HOST, "app.fiducia.cloud")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_marker = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store",
        "root-served portal must not be cacheable"
    );
    assert_eq!(response.headers().get(header::PRAGMA).unwrap(), "no-cache");
    assert!(
        body_marker.contains("form-action 'self'"),
        "root-served portal must carry the strict portal CSP, got: {body_marker}"
    );
    // `same-origin`, not `no-referrer`: no-referrer nulls the Origin header
    // browsers attach to mutations, which would break the same-origin gate
    // for every real browser (see the security_headers comment).
    assert_eq!(
        response.headers().get(header::REFERRER_POLICY).unwrap(),
        "same-origin"
    );
}

#[tokio::test]
async fn customer_dynamic_responses_are_never_cacheable() {
    for uri in ["/login", "/app"] {
        let response = build_router(test_config())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::HOST, "app.fiducia.cloud")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert_eq!(response.headers().get(header::PRAGMA).unwrap(), "no-cache");
        assert!(response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("form-action 'self'"));
    }
}

#[tokio::test]
async fn multi_org_portal_propagates_only_validated_selection() {
    let response = build_router(multi_org_config())
        .oneshot(
            Request::builder()
                .uri(format!("/app/api-keys?org_id={ORG_B}"))
                .header(header::HOST, "app.fiducia.cloud")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(&format!("?org_id={ORG_B}")));
    assert!(body.contains(&format!("name=\"org_id\" value=\"{ORG_B}\"")));
    assert!(body.contains("name=\"csrf_token\""));
    assert!(body.contains("name=\"idempotency_key\""));

    let foreign = build_router(multi_org_config())
        .oneshot(
            Request::builder()
                .uri("/app/api-keys?org_id=00000000-0000-4000-8000-000000000003")
                .header(header::HOST, "app.fiducia.cloud")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
}

#[test]
fn api_key_table_exposes_csrf_protected_rotation_and_revocation() {
    let key: AuthKeyMeta = serde_json::from_value(mock_key_meta()).unwrap();
    let display = auth_key_to_display(&key, ORG_B).unwrap();
    let body = api_keys_table_markup(&[display], None, "csrf-test", ORG_B).into_string();
    assert!(body.contains("/app/api-keys/rotate"));
    assert!(body.contains("/app/api-keys/revoke"));
    assert!(body.contains("name=\"csrf_token\" value=\"csrf-test\""));
    assert!(body.contains("name=\"idempotency_key\""));
    assert!(body.contains(&format!("name=\"org_id\" value=\"{ORG_B}\"")));
}

#[tokio::test]
async fn user_controlled_customer_fields_are_bounded_before_io() {
    let (status, _, body) = send_json(
        "POST",
        "/api/customer/api-keys",
        json!({
            "name": "x".repeat(MAX_API_KEY_NAME_CHARS + 1),
            "environment": "live",
            "scope": "requests:write",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["error"],
        "name_too_long"
    );

    let (status, _, body) = send_json(
        "PUT",
        "/api/customer/preferences",
        json!({
            "region": "iad1",
            "timezone": "x".repeat(MAX_TIMEZONE_CHARS + 1),
            "density": "compact",
            "notify_lock_contention": false,
            "notify_key_rotation": true,
            "notify_mfa": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["error"],
        "invalid_timezone"
    );

    let (status, _, body) = send_json(
        "POST",
        "/api/customer/security/sessions/revoke",
        json!({ "device": "x".repeat(MAX_SESSION_DEVICE_CHARS + 1) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["error"],
        "invalid_device"
    );
}

#[tokio::test]
async fn websocket_rejects_same_site_sibling_origin() {
    let response = build_router(test_config())
        .oneshot(
            Request::builder()
                .uri("/app/ws")
                .header(header::HOST, "app.fiducia.cloud")
                .header(header::ORIGIN, "https://admin.fiducia.cloud")
                .header("sec-fetch-site", "same-site")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
