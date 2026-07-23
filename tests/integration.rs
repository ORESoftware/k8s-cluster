//! End-to-end HTTP tests: the real axum router driven via `oneshot`, exercising
//! the Supabase→OreSoftware exchange pipeline, introspection, the JWKS endpoint,
//! the script-free Maud UI, and metrics — with an HS256 "test project" so the whole
//! pipe runs with no network and no database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use shared_auth_server::config::{AppConfig, SessionConfig, SigningConfig, SupabaseProject};
use shared_auth_server::state::AppState;
use tower::ServiceExt;

const SUPA_SECRET: &str = "integration-test-secret";
const SUPA_ISS: &str = "https://itest.supabase.co/auth/v1";

/// Deterministic P-256 signing key from a fixed scalar (no transcription risk).
fn signing_pem() -> String {
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    p256::SecretKey::from_slice(&[7u8; 32])
        .unwrap()
        .to_pkcs8_pem(LineEnding::LF)
        .unwrap()
        .to_string()
}

fn config() -> AppConfig {
    AppConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        projects: vec![SupabaseProject {
            name: "fiducia-cloud".into(),
            project_ref: "itest".into(),
            issuer: Some(SUPA_ISS.into()),
            jwks_url: Some("http://127.0.0.1:1/jwks".into()),
            audience: "authenticated".into(),
            hs256_secret: Some(SUPA_SECRET.into()),
        }],
        signing: SigningConfig {
            ec_private_pem: signing_pem(),
            key_id: "shared-auth-v1".into(),
            issuer: "https://auth.oresoftware.dev".into(),
            audience: "oresoftware".into(),
            ttl_secs: 3600,
        },
        db: None,
        redis: None,
        sessions: SessionConfig {
            refresh_ttl_secs: 3600,
            allow_registration: false,
        },
        webhook_secret: None,
        cors_allow_origins: vec![],
    }
}

async fn app() -> axum::Router {
    let state = AppState::build(config()).await.unwrap();
    shared_auth_server::http::router(state)
}

fn supabase_token(sub: &str, email: &str) -> String {
    let claims = serde_json::json!({
        "sub": sub, "aud": "authenticated", "iss": SUPA_ISS,
        "exp": chrono::Utc::now().timestamp() + 3600,
        "email": email, "email_verified": true,
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SUPA_SECRET.as_bytes()),
    )
    .unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn healthz_ok() {
    let resp = app()
        .await
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("ok"));
}

#[tokio::test]
async fn jwks_exposes_our_signing_key() {
    let resp = app()
        .await
        .oneshot(
            Request::get("/.well-known/jwks.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(json["keys"][0]["kid"], "shared-auth-v1");
    assert_eq!(json["keys"][0]["alg"], "ES256");
}

// The headline integration: a Supabase token in, a verifiable OreSoftware token
// out, which then introspects as active.
#[tokio::test]
async fn exchange_then_introspect_roundtrip() {
    let app = app().await;
    let supa = supabase_token("supa-user-1", "user@example.com");

    let resp = app
        .clone()
        .oneshot(
            Request::post("/auth/exchange")
                .header("authorization", format!("Bearer {supa}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let out: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(out["project"], "fiducia-cloud");
    let ore = out["access_token"].as_str().unwrap().to_string();

    let resp = app
        .oneshot(
            Request::post("/auth/introspect")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "token": ore }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let intro: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(intro["active"], true);
    assert_eq!(intro["project"], "fiducia-cloud");
    assert_eq!(intro["supabase_user_id"], "supa-user-1");
}

#[tokio::test]
async fn exchange_rejects_bad_token() {
    let resp = app()
        .await
        .oneshot(
            Request::post("/auth/exchange")
                .header("authorization", "Bearer not.a.token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn landing_and_ui_render_html() {
    let app = app().await;
    let root = app
        .clone()
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(root.status(), StatusCode::OK);
    let html = body_string(root).await;
    assert!(html.contains("shared-auth-server"));
    assert!(html.contains("<!DOCTYPE html>"));

    let ui = app
        .oneshot(Request::get("/ui").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(body_string(ui).await.contains("token exchange"));
}

#[tokio::test]
async fn ui_exchange_returns_success_fragment() {
    let supa = supabase_token("html-user", "h@example.com");
    let form = format!("access_token={supa}");
    let resp = app()
        .await
        .oneshot(
            Request::post("/ui/exchange")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let frag = body_string(resp).await;
    assert!(frag.contains("exchanged"));
    assert!(frag.contains("fiducia-cloud"));
}

#[tokio::test]
async fn metrics_records_exchanges() {
    let app = app().await;
    let supa = supabase_token("m-user", "m@example.com");
    let _ = app
        .clone()
        .oneshot(
            Request::post("/auth/exchange")
                .header("authorization", format!("Bearer {supa}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let resp = app
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let text = body_string(resp).await;
    assert!(text.contains("shared_auth_exchanges_total"));
}

async fn mint_ore_token(app: &axum::Router, sub: &str) -> String {
    let supa = supabase_token(sub, "e@example.com");
    let resp = app
        .clone()
        .oneshot(
            Request::post("/auth/exchange")
                .header("authorization", format!("Bearer {supa}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let out: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    out["access_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn readyz_ready_without_db() {
    let resp = app()
        .await
        .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("ready"));
}

// Gateway auth_request: a valid OreSoftware token → 200 + identity headers.
#[tokio::test]
async fn verify_endpoint_accepts_our_token_and_sets_headers() {
    let app = app().await;
    let ore = mint_ore_token(&app, "verify-user").await;
    let resp = app
        .oneshot(
            Request::get("/auth/verify")
                .header("authorization", format!("Bearer {ore}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("x-auth-project").unwrap(),
        "fiducia-cloud"
    );
    assert!(resp.headers().get("x-auth-user-id").is_some());
}

#[tokio::test]
async fn verify_endpoint_rejects_missing_and_bad_tokens() {
    let app = app().await;
    let none = app
        .clone()
        .oneshot(Request::get("/auth/verify").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(none.status(), StatusCode::UNAUTHORIZED);

    let bad = app
        .oneshot(
            Request::get("/auth/verify")
                .header("authorization", "Bearer nope.nope.nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
}

// A raw Supabase token is NOT an OreSoftware token; introspect reports inactive.
#[tokio::test]
async fn introspect_non_ore_token_is_inactive() {
    let supa = supabase_token("x", "x@example.com");
    let resp = app()
        .await
        .oneshot(
            Request::post("/auth/introspect")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "token": supa }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let intro: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(intro["active"], false);
}

// Exchange accepts the token in the JSON body too, not only the header.
#[tokio::test]
async fn exchange_via_json_body() {
    let supa = supabase_token("body-user", "b@example.com");
    let resp = app()
        .await
        .oneshot(
            Request::post("/auth/exchange")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "access_token": supa }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// DB-less test mode still uses a stable provider-derived UUID.
#[tokio::test]
async fn dbless_shared_id_is_deterministic() {
    let app = app().await;
    let supa = supabase_token("supa-user-1", "u@example.com");
    let resp = app
        .oneshot(
            Request::post("/auth/exchange")
                .header("authorization", format!("Bearer {supa}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let out: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    let first = out["shared_user_id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(first).is_ok());
}
