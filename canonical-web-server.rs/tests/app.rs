use async_trait::async_trait;
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    routing::post,
    Json, Router,
};
use canonical_web_server::{
    auth::{
        AuthProvider, AuthProviderError, AuthTokens, SessionService, SupabaseAuth, SupabaseUser,
    },
    build_app,
    config::Config,
    db::{entity::web_session, migration::Migrator},
    error::AppError,
    AppState,
};
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use sea_orm_migration::MigratorTrait;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tower::ServiceExt;
use uuid::Uuid;

const USER_ID: Uuid = Uuid::from_u128(0x8c1f269a_2d15_4b56_8e38_e68db51f979f);
const OTHER_USER_ID: Uuid = Uuid::from_u128(0x2f9a41c3_7be0_4d12_9c55_10afdd42be71);

#[derive(Clone)]
struct FakeAuth;

#[async_trait]
impl AuthProvider for FakeAuth {
    async fn password_sign_in(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthTokens, AuthProviderError> {
        match (email, password) {
            ("user@example.com", "secret") => Ok(tokens()),
            ("other@example.com", "secret") => Ok(other_tokens()),
            _ => Err(AuthProviderError::InvalidCredentials),
        }
    }

    async fn refresh(&self, refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
        match refresh_token {
            "refresh-token" => Ok(tokens()),
            "refresh-token-b" => Ok(other_tokens()),
            _ => Err(AuthProviderError::InvalidCredentials),
        }
    }

    async fn user_for_token(&self, access_token: &str) -> Result<SupabaseUser, AuthProviderError> {
        if access_token == "valid-token" {
            Ok(user())
        } else {
            Err(AuthProviderError::InvalidCredentials)
        }
    }

    async fn sign_out(&self, _access_token: &str) -> Result<(), AuthProviderError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum RefreshFailure {
    InvalidCredentials,
    Unavailable,
}

#[derive(Clone)]
struct RefreshFailureAuth {
    failure: RefreshFailure,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AuthProvider for RefreshFailureAuth {
    async fn password_sign_in(
        &self,
        _email: &str,
        _password: &str,
    ) -> Result<AuthTokens, AuthProviderError> {
        unreachable!("the session fixture creates tokens directly")
    }

    async fn refresh(&self, _refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.failure {
            RefreshFailure::InvalidCredentials => Err(AuthProviderError::InvalidCredentials),
            RefreshFailure::Unavailable => Err(AuthProviderError::Unavailable),
        }
    }

    async fn user_for_token(&self, _access_token: &str) -> Result<SupabaseUser, AuthProviderError> {
        unreachable!("the refresh fixture authenticates only session cookies")
    }

    async fn sign_out(&self, _access_token: &str) -> Result<(), AuthProviderError> {
        Ok(())
    }
}

fn user() -> SupabaseUser {
    SupabaseUser {
        id: USER_ID,
        email: Some("user@example.com".into()),
    }
}

fn tokens() -> AuthTokens {
    AuthTokens {
        access_token: "access-token".into(),
        refresh_token: "refresh-token".into(),
        expires_at: Utc::now() + ChronoDuration::hours(1),
        user: user(),
    }
}

fn other_user() -> SupabaseUser {
    SupabaseUser {
        id: OTHER_USER_ID,
        email: Some("other@example.com".into()),
    }
}

fn other_tokens() -> AuthTokens {
    AuthTokens {
        access_token: "access-token-b".into(),
        refresh_token: "refresh-token-b".into(),
        expires_at: Utc::now() + ChronoDuration::hours(1),
        user: other_user(),
    }
}

async fn expired_session(
    failure: RefreshFailure,
) -> (
    DatabaseConnection,
    SessionService,
    String,
    String,
    Arc<AtomicUsize>,
) {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = Arc::new(RefreshFailureAuth {
        failure,
        calls: calls.clone(),
    });
    let sessions =
        SessionService::new(db.clone(), auth, &[7; 32], Duration::from_secs(3600)).unwrap();
    let mut expired_tokens = tokens();
    expired_tokens.expires_at = Utc::now() - ChronoDuration::minutes(1);
    let created = sessions.create(expired_tokens).await.unwrap();
    let session_hash = created.context.session_hash.unwrap();
    (db, sessions, created.raw_id, session_hash, calls)
}

async fn app() -> Router {
    build_app(state().await)
}

async fn state() -> AppState {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    let config = Config {
        port: 8081,
        static_dir: PathBuf::from("test-fixtures/no-marketing-site"),
        app_asset_dir: PathBuf::from("client/dist"),
        database_url: "[test connection is injected]".into(),
        database_max_connections: 1,
        auto_migrate: false,
        app_base_url: "http://localhost:8081".into(),
        allowed_origins: HashSet::from(["http://localhost:8081".into()]),
        session_cookie: "canonical_session".into(),
        cookie_secure: false,
        session_encryption_key: vec![7; 32],
        session_ttl: Duration::from_secs(30 * 24 * 60 * 60),
        supabase_url: "http://localhost:9999".into(),
        supabase_publishable_key: "test-publishable-key".into(),
    };
    AppState::new(config, db, Arc::new(FakeAuth)).unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn health_and_versioned_info_are_available() {
    let app = app().await;
    let health = app
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let info = app
        .oneshot(Request::get("/api/v1/info").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(info.status(), StatusCode::OK);
    let body = body_json(info).await;
    assert_eq!(body["service"], "canonical-web-server");
    assert_eq!(body["stack"][0], "supabase");
    assert_eq!(body["stack"][4], "htmx");
}

#[tokio::test]
async fn api_not_found_is_json_and_does_not_fall_into_marketing_site() {
    let response = app()
        .await
        .oneshot(Request::get("/api/v1/missing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"]["code"], "not_found");
}

#[tokio::test]
async fn bearer_auth_exposes_only_the_verified_user() {
    let response = app()
        .await
        .oneshot(
            Request::get("/api/v1/me")
                .header(header::AUTHORIZATION, "Bearer valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["userId"], USER_ID.to_string());
    assert_eq!(body["email"], "user@example.com");
}

#[tokio::test]
async fn password_login_creates_an_opaque_session_for_maud_pages() {
    let app = app().await;
    let login_page = app
        .clone()
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(login_page.status(), StatusCode::OK);
    let csrf_cookie = cookie_value(&login_page, "canonical_login_csrf").unwrap();
    let page = body_text(login_page).await;
    assert!(page.contains("hx-post=\"/auth/login\""));
    assert!(page.contains("/app-assets/app.js"));

    let login = app
        .clone()
        .oneshot(
            Request::post("/auth/login")
                .header(header::ORIGIN, "http://localhost:8081")
                .header(
                    header::COOKIE,
                    format!("canonical_login_csrf={csrf_cookie}"),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "email=user%40example.com&password=secret&csrf={csrf_cookie}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    let session_cookie = cookie_value(&login, "canonical_session").unwrap();
    assert_ne!(session_cookie, "access-token");
    assert_ne!(session_cookie, "refresh-token");

    let dashboard = app
        .oneshot(
            Request::get("/app")
                .header(
                    header::COOKIE,
                    format!("canonical_session={session_cookie}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dashboard.status(), StatusCode::OK);
    let page = body_text(dashboard).await;
    assert!(page.contains("data-sync-root=\"draft_note\""));
    assert!(page.contains("hx-ext=\"ws\""));
    assert!(page.contains("ws-connect=\"/ws\""));
    assert!(page.contains(&USER_ID.to_string()));
}

#[tokio::test]
async fn invalid_htmx_login_returns_a_targeted_fragment_without_changing_the_page() {
    let app = app().await;
    let login_page = app
        .clone()
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf_cookie = cookie_value(&login_page, "canonical_login_csrf").unwrap();

    let htmx_response = app
        .clone()
        .oneshot(
            Request::post("/auth/login")
                .header(header::ORIGIN, "http://localhost:8081")
                .header("hx-request", "true")
                .header(
                    header::COOKIE,
                    format!("canonical_login_csrf={csrf_cookie}"),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "email=user%40example.com&password=wrong&csrf={csrf_cookie}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(htmx_response.status(), StatusCode::OK);
    assert_eq!(
        htmx_response.headers().get("hx-retarget").unwrap(),
        "#login-result"
    );
    let fragment = body_text(htmx_response).await;
    assert!(fragment.contains("role=\"alert\""));
    assert!(fragment.contains("Email or password was not accepted."));
    assert!(!fragment.contains("<!DOCTYPE html>"));

    let full_page_response = app
        .oneshot(
            Request::post("/auth/login")
                .header(header::ORIGIN, "http://localhost:8081")
                .header(
                    header::COOKIE,
                    format!("canonical_login_csrf={csrf_cookie}"),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "email=user%40example.com&password=wrong&csrf={csrf_cookie}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(full_page_response.status(), StatusCode::UNAUTHORIZED);
    assert!(body_text(full_page_response)
        .await
        .contains("<!DOCTYPE html>"));
}

#[tokio::test]
async fn app_csp_does_not_leak_onto_the_marketing_fallback() {
    let app = app().await;
    let app_page = app
        .clone()
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(app_page
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY));
    assert!(app_page
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("ws://localhost:8081"));

    let marketing_fallback = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(!marketing_fallback
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY));
    assert_eq!(
        marketing_fallback
            .headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .unwrap(),
        "nosniff"
    );
    assert_eq!(
        marketing_fallback
            .headers()
            .get(header::REFERRER_POLICY)
            .unwrap(),
        "strict-origin-when-cross-origin"
    );
}

#[tokio::test]
async fn invalid_sync_cursor_has_a_stable_machine_readable_error() {
    let response = app()
        .await
        .oneshot(
            Request::get("/api/v1/sync/changes?cursor=not-a-cursor")
                .header(header::AUTHORIZATION, "Bearer valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "invalid_sync_cursor"
    );
}

#[derive(Clone)]
struct TokenCapture {
    sender: tokio::sync::mpsc::UnboundedSender<HeaderMap>,
}

async fn capture_password_token_request(
    State(capture): State<TokenCapture>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    capture.sender.send(headers).unwrap();
    Json(serde_json::json!({
        "access_token": "captured-access-token",
        "refresh_token": "captured-refresh-token",
        "expires_in": 3600,
        "user": {
            "id": USER_ID,
            "email": "user@example.com"
        }
    }))
}

#[tokio::test]
async fn supabase_password_token_request_uses_apikey_without_bearer_authorization() {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/auth/v1/token", post(capture_password_token_request))
                .with_state(TokenCapture { sender }),
        )
        .await
        .unwrap();
    });

    let auth =
        SupabaseAuth::new(format!("http://{address}"), "test-publishable-key".into()).unwrap();
    let tokens = auth
        .password_sign_in("user@example.com", "secret")
        .await
        .unwrap();
    assert_eq!(tokens.access_token, "captured-access-token");

    let headers = receiver.recv().await.unwrap();
    assert_eq!(headers.get("apikey").unwrap(), "test-publishable-key");
    assert!(!headers.contains_key(header::AUTHORIZATION));
    server.abort();
}

#[tokio::test]
async fn rejected_refresh_token_revokes_the_local_session_once() {
    let (db, sessions, raw_id, session_hash, calls) =
        expired_session(RefreshFailure::InvalidCredentials).await;

    assert!(matches!(
        sessions.authenticate(&raw_id).await,
        Err(AppError::Unauthorized)
    ));
    let model = web_session::Entity::find_by_id(&session_hash)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(model.revoked_at.is_some());

    assert!(matches!(
        sessions.authenticate(&raw_id).await,
        Err(AppError::Unauthorized)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transient_refresh_failure_keeps_the_local_session_retryable() {
    let (db, sessions, raw_id, session_hash, calls) =
        expired_session(RefreshFailure::Unavailable).await;

    assert!(matches!(
        sessions.authenticate(&raw_id).await,
        Err(AppError::AuthUpstream)
    ));
    let model = web_session::Entity::find_by_id(&session_hash)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(model.revoked_at.is_none());

    assert!(matches!(
        sessions.authenticate(&raw_id).await,
        Err(AppError::AuthUpstream)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn sync_mutations_are_idempotent_and_conflicts_return_the_server_snapshot() {
    let app = app().await;
    let client_id = Uuid::new_v4();
    let mutation_id = Uuid::new_v4();
    let record_id = Uuid::new_v4();
    let create = serde_json::json!({
        "protocolVersion": 1,
        "clientId": client_id,
        "operations": [{
            "mutationId": mutation_id,
            "key": { "kind": "draft_note", "id": record_id },
            "action": "put",
            "baseVersion": null,
            "schemaVersion": 1,
            "value": { "title": "Offline first", "body": "queued locally" }
        }]
    });

    let first = push(&app, &create).await;
    assert_eq!(first["results"][0]["status"], "applied");
    assert_eq!(first["results"][0]["record"]["version"], "1");
    let replay = push(&app, &create).await;
    assert_eq!(replay, first);

    let reused = serde_json::json!({
        "protocolVersion": 1,
        "clientId": client_id,
        "operations": [{
            "mutationId": mutation_id,
            "key": { "kind": "draft_note", "id": record_id },
            "action": "put",
            "baseVersion": "1",
            "schemaVersion": 1,
            "value": { "title": "Different", "body": "payload" }
        }]
    });
    assert_eq!(
        push(&app, &reused).await["results"][0]["status"],
        "idempotency_key_reused"
    );

    let stale = serde_json::json!({
        "protocolVersion": 1,
        "clientId": client_id,
        "operations": [{
            "mutationId": Uuid::new_v4(),
            "key": { "kind": "draft_note", "id": record_id },
            "action": "put",
            "baseVersion": "0",
            "schemaVersion": 1,
            "value": { "title": "Stale", "body": "base" }
        }]
    });
    let conflict = push(&app, &stale).await;
    assert_eq!(conflict["results"][0]["status"], "conflict");
    assert_eq!(conflict["results"][0]["record"]["version"], "1");

    let pull = app
        .oneshot(
            Request::get("/api/v1/sync/changes")
                .header(header::AUTHORIZATION, "Bearer valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pull.status(), StatusCode::OK);
    let pull = body_json(pull).await;
    assert_eq!(pull["changes"].as_array().unwrap().len(), 1);
    assert_eq!(pull["changes"][0]["key"]["id"], record_id.to_string());
    assert_eq!(pull["caughtUp"], true);
    assert!(pull["nextCursor"].as_str().unwrap().len() > 20);
}

#[tokio::test]
async fn invalid_mutations_are_receipted_and_base_versions_are_canonical() {
    let app = app().await;
    let client_id = Uuid::new_v4();
    let mutation_id = Uuid::new_v4();
    let record_id = Uuid::new_v4();
    let invalid = serde_json::json!({
        "protocolVersion": 1,
        "clientId": client_id,
        "operations": [{
            "mutationId": mutation_id,
            "key": { "kind": "draft_note", "id": record_id },
            "action": "put",
            "baseVersion": null,
            "schemaVersion": 1,
            "value": { "title": "x".repeat(201), "body": "too long" }
        }]
    });
    let first = push(&app, &invalid).await;
    assert_eq!(first["results"][0]["status"], "invalid");
    assert_eq!(push(&app, &invalid).await, first);

    let reused = serde_json::json!({
        "protocolVersion": 1,
        "clientId": client_id,
        "operations": [{
            "mutationId": mutation_id,
            "key": { "kind": "draft_note", "id": record_id },
            "action": "put",
            "baseVersion": null,
            "schemaVersion": 1,
            "value": { "title": "fixed", "body": "different payload" }
        }]
    });
    assert_eq!(
        push(&app, &reused).await["results"][0]["status"],
        "idempotency_key_reused"
    );

    for invalid_version in ["-1", "01", "9223372036854775808"] {
        let invalid_base = serde_json::json!({
            "protocolVersion": 1,
            "clientId": client_id,
            "operations": [{
                "mutationId": Uuid::new_v4(),
                "key": { "kind": "draft_note", "id": Uuid::new_v4() },
                "action": "put",
                "baseVersion": invalid_version,
                "schemaVersion": 1,
                "value": { "title": "bad base", "body": "body" }
            }]
        });
        assert_eq!(
            push(&app, &invalid_base).await["results"][0]["status"],
            "invalid"
        );
    }
}

#[tokio::test]
async fn worst_case_valid_drafts_fit_pushes_and_pull_pages_stay_under_the_byte_budget() {
    let app = app().await;
    let client_id = Uuid::new_v4();
    let escaped_body = "\0".repeat(100_000);
    for index in 0..3 {
        let create = serde_json::json!({
            "protocolVersion": 1,
            "clientId": client_id,
            "operations": [{
                "mutationId": Uuid::new_v4(),
                "key": { "kind": "draft_note", "id": Uuid::new_v4() },
                "action": "put",
                "baseVersion": null,
                "schemaVersion": 1,
                "value": { "title": format!("Escaped draft {index}"), "body": escaped_body }
            }]
        });
        assert_eq!(push(&app, &create).await["results"][0]["status"], "applied");
    }

    let response = app
        .oneshot(
            Request::get("/api/v1/sync/changes?limit=500")
                .header(header::AUTHORIZATION, "Bearer valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let encoded = body_text(response).await;
    assert!(encoded.len() < 1024 * 1024);
    let pull: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(pull["changes"].as_array().unwrap().len(), 1);
    assert_eq!(pull["caughtUp"], false);
}

#[tokio::test]
async fn pull_queries_never_materialize_more_than_sixteen_candidate_rows() {
    let app = app().await;
    let client_id = Uuid::new_v4();
    for index in 0..17 {
        let create = serde_json::json!({
            "protocolVersion": 1,
            "clientId": client_id,
            "operations": [{
                "mutationId": Uuid::new_v4(),
                "key": { "kind": "draft_note", "id": Uuid::new_v4() },
                "action": "put",
                "baseVersion": null,
                "schemaVersion": 1,
                "value": { "title": format!("Draft {index}"), "body": "small" }
            }]
        });
        assert_eq!(push(&app, &create).await["results"][0]["status"], "applied");
    }

    let response = app
        .oneshot(
            Request::get("/api/v1/sync/changes?limit=500")
                .header(header::AUTHORIZATION, "Bearer valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let pull = body_json(response).await;
    assert_eq!(pull["changes"].as_array().unwrap().len(), 16);
    assert_eq!(pull["caughtUp"], false);
}

#[tokio::test]
async fn websocket_authentication_happens_before_upgrade() {
    let response = app()
        .await
        .oneshot(
            Request::get("/ws")
                .header(header::CONNECTION, "upgrade")
                .header(header::UPGRADE, "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authenticated_websocket_receives_typed_invalidation_hints() {
    let state = state().await;
    let hub = state.hub.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_app(state)).await.unwrap();
    });

    let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer valid-token".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let hello = next_text(&mut socket).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["protocolVersion"], 1);

    // Owner-scoped hints for another account must never reach this socket.
    hub.invalidate(Uuid::new_v4(), 41);
    hub.invalidate(USER_ID, 42);
    let invalidation = next_text(&mut socket).await;
    assert_eq!(invalidation["type"], "sync.invalidated");
    assert_eq!(invalidation["latestHint"], "42");

    socket.close(None).await.unwrap();
    server.abort();
}

async fn push(app: &Router, payload: &serde_json::Value) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/sync/mutations")
                .header(header::AUTHORIZATION, "Bearer valid-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

fn cookie_value(response: &axum::response::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix(&format!("{name}="))
                .and_then(|value| value.split(';').next())
                .map(str::to_owned)
        })
}

async fn next_text<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> serde_json::Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Ping(payload) => socket.send(Message::Pong(payload)).await.unwrap(),
            _ => {}
        }
    }
}

async fn sign_in(app: &Router, email: &str, password: &str) -> String {
    let login_page = app
        .clone()
        .oneshot(Request::get("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csrf = cookie_value(&login_page, "canonical_login_csrf").unwrap();
    let login = app
        .clone()
        .oneshot(
            Request::post("/auth/login")
                .header(header::ORIGIN, "http://localhost:8081")
                .header(header::COOKIE, format!("canonical_login_csrf={csrf}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "email={}&password={password}&csrf={csrf}",
                    email.replace('@', "%40")
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    cookie_value(&login, "canonical_session").unwrap()
}

async fn authed_get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header(header::COOKIE, format!("canonical_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_text(response).await)
}

async fn authed_post(
    app: &Router,
    path: &str,
    session: &str,
    body: String,
    htmx: bool,
) -> axum::response::Response {
    let mut request = Request::post(path)
        .header(header::ORIGIN, "http://localhost:8081")
        .header(header::COOKIE, format!("canonical_session={session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if htmx {
        request = request.header("hx-request", "true");
    }
    app.clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

fn extract_after<'a>(page: &'a str, marker: &str, terminator: char) -> &'a str {
    let start = page.find(marker).expect("marker present in page") + marker.len();
    let rest = &page[start..];
    let end = rest.find(terminator).expect("terminator present");
    &rest[..end]
}

fn page_csrf(page: &str) -> String {
    extract_after(page, "name=\"csrf\" value=\"", '"').to_string()
}

fn first_engagement_id(page: &str) -> String {
    extract_after(page, "href=\"/app/engagements/", '"').to_string()
}

#[tokio::test]
async fn engagement_pages_require_a_session() {
    let app = app().await;
    let response = app
        .oneshot(
            Request::get("/app/engagements")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/login");
}

#[tokio::test]
async fn engagement_lifecycle_create_list_detail_status_note() {
    let app = app().await;
    let session = sign_in(&app, "user@example.com", "secret").await;

    let (status, page) = authed_get(&app, "/app/engagements", &session).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("No engagements yet"));
    let csrf = page_csrf(&page);

    let create = authed_post(
        &app,
        "/app/engagements",
        &session,
        format!("csrf={csrf}&company=Acme%20Corp&framework=soc2&target_report_date=2026-12-31"),
        false,
    )
    .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);
    assert_eq!(create.headers()[header::LOCATION], "/app/engagements");

    let (_, page) = authed_get(&app, "/app/engagements", &session).await;
    assert!(page.contains("Acme Corp"));
    assert!(page.contains("SOC 2"));
    assert!(page.contains("report due 2026-12-31"));
    let id = first_engagement_id(&page);

    let (status, detail) = authed_get(&app, &format!("/app/engagements/{id}"), &session).await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail.contains("Status: Scoping"));
    assert!(detail.contains("No notes yet."));

    let update = authed_post(
        &app,
        &format!("/app/engagements/{id}/status"),
        &session,
        format!("csrf={csrf}&status=in_audit"),
        true,
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let fragment = body_text(update).await;
    assert!(fragment.contains("Status: In audit"));

    let note = authed_post(
        &app,
        &format!("/app/engagements/{id}/notes"),
        &session,
        format!("csrf={csrf}&body=Kickoff%20call%20scheduled"),
        false,
    )
    .await;
    assert_eq!(note.status(), StatusCode::SEE_OTHER);

    let (_, detail) = authed_get(&app, &format!("/app/engagements/{id}"), &session).await;
    assert!(detail.contains("Kickoff call scheduled"));
    assert!(detail.contains("Status: In audit"));
}

#[tokio::test]
async fn invalid_engagement_input_is_rejected_with_a_targeted_fragment() {
    let app = app().await;
    let session = sign_in(&app, "user@example.com", "secret").await;
    let (_, page) = authed_get(&app, "/app/engagements", &session).await;
    let csrf = page_csrf(&page);

    let bad_framework = authed_post(
        &app,
        "/app/engagements",
        &session,
        format!("csrf={csrf}&company=Acme&framework=warp9"),
        true,
    )
    .await;
    assert_eq!(bad_framework.status(), StatusCode::OK);
    assert_eq!(
        bad_framework.headers()["hx-retarget"],
        "#engagement-form-error"
    );
    assert!(body_text(bad_framework)
        .await
        .contains("supported compliance framework"));

    let oversize_company = "a".repeat(201);
    let too_long = authed_post(
        &app,
        "/app/engagements",
        &session,
        format!("csrf={csrf}&company={oversize_company}&framework=soc2"),
        true,
    )
    .await;
    assert_eq!(too_long.status(), StatusCode::OK);
    assert_eq!(too_long.headers()["hx-retarget"], "#engagement-form-error");

    let (_, page) = authed_get(&app, "/app/engagements", &session).await;
    assert!(page.contains("No engagements yet"));
}

#[tokio::test]
async fn engagements_are_owner_scoped() {
    let app = app().await;
    let session_a = sign_in(&app, "user@example.com", "secret").await;
    let (_, page) = authed_get(&app, "/app/engagements", &session_a).await;
    let csrf_a = page_csrf(&page);
    let create = authed_post(
        &app,
        "/app/engagements",
        &session_a,
        format!("csrf={csrf_a}&company=Secret%20Client&framework=hipaa"),
        false,
    )
    .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);
    let (_, page) = authed_get(&app, "/app/engagements", &session_a).await;
    let id = first_engagement_id(&page);

    let session_b = sign_in(&app, "other@example.com", "secret").await;
    let (status, page_b) = authed_get(&app, "/app/engagements", &session_b).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page_b.contains("No engagements yet"));
    assert!(!page_b.contains("Secret Client"));

    let (status, _) = authed_get(&app, &format!("/app/engagements/{id}"), &session_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let csrf_b = page_csrf(&page_b);
    let foreign_update = authed_post(
        &app,
        &format!("/app/engagements/{id}/status"),
        &session_b,
        format!("csrf={csrf_b}&status=complete"),
        true,
    )
    .await;
    assert_eq!(foreign_update.status(), StatusCode::NOT_FOUND);

    let (_, detail_a) = authed_get(&app, &format!("/app/engagements/{id}"), &session_a).await;
    assert!(detail_a.contains("Status: Scoping"));
}
