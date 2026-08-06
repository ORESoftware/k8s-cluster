//! Router-level password-boundary contracts for `/auth/login`.
//!
//! These tests intentionally cross the full Axum + Postgres boundary. CI
//! supplies `AUTH_TEST_DATABASE_URL`; local runs skip cleanly when it is absent.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use shared_auth_server::config::{
    AppConfig, DbConfig, MagicLinkConfig, SessionConfig, SigningConfig, TwilioVerifyConfig,
};
use tower::ServiceExt;
use uuid::Uuid;

const MAX_PASSWORD_BYTES: usize = 1024;

fn router_signing_pem() -> String {
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    p256::SecretKey::from_slice(&[11u8; 32])
        .expect("fixed E2E signing scalar must be valid")
        .to_pkcs8_pem(LineEnding::LF)
        .expect("fixed E2E signing key must encode")
        .to_string()
}

async fn router_with_db(url: String) -> axum::Router {
    let config = AppConfig {
        bind_addr: "127.0.0.1:0".parse().expect("test bind address"),
        projects: vec![],
        signing: SigningConfig {
            ec_private_pem: router_signing_pem(),
            key_id: "shared-auth-login-bound-e2e".into(),
            issuer: "https://auth.oresoftware.dev".into(),
            audience: "oresoftware".into(),
            ttl_secs: 3600,
        },
        db: Some(DbConfig {
            url,
            max_connections: 2,
        }),
        redis: None,
        sessions: SessionConfig {
            refresh_ttl_secs: 3600,
            allow_registration: true,
        },
        magic_links: MagicLinkConfig {
            sendgrid_api_key: None,
            otp_pepper: None,
            from_email: None,
            from_name: "OreSoftware".into(),
            link_base_url: None,
            ttl_secs: 900,
            allow_signup: false,
        },
        twilio_verify: TwilioVerifyConfig {
            account_sid: None,
            auth_token: None,
            service_sid: None,
        },
        webhook_secret: None,
        introspect_secret: None,
        cors_allow_origins: vec![],
    };
    let state = shared_auth_server::state::AppState::build(config)
        .await
        .expect("router state must build against disposable Postgres");
    shared_auth_server::http::router(state)
}

async fn post_json(
    app: axum::Router,
    path: &'static str,
    value: serde_json::Value,
) -> axum::response::Response {
    app.oneshot(
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .expect("valid E2E request"),
    )
    .await
    .expect("router must answer E2E request")
}

#[tokio::test]
async fn login_accepts_the_exact_registration_maximum_at_the_http_boundary() {
    let Some(url) = std::env::var("AUTH_TEST_DATABASE_URL").ok() else {
        eprintln!("AUTH_TEST_DATABASE_URL unset; skipping login boundary E2E test");
        return;
    };
    let app = router_with_db(url).await;

    let response = post_json(
        app,
        "/auth/login",
        serde_json::json!({
            "email": format!("unknown-{}@example.invalid", Uuid::new_v4()),
            "password": "a".repeat(MAX_PASSWORD_BYTES),
        }),
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "1024 bytes must reach ordinary credential verification rather than the length guard",
    );
}

#[tokio::test]
async fn overlong_login_rejection_is_identical_for_existing_and_unknown_accounts() {
    let Some(url) = std::env::var("AUTH_TEST_DATABASE_URL").ok() else {
        eprintln!("AUTH_TEST_DATABASE_URL unset; skipping login enumeration E2E test");
        return;
    };
    let app = router_with_db(url).await;
    let existing_email = format!("existing-{}@example.invalid", Uuid::new_v4());

    let registration = post_json(
        app.clone(),
        "/auth/register",
        serde_json::json!({
            "email": existing_email,
            "password": "correct horse battery staple",
            "display_name": "Boundary E2E User",
        }),
    )
    .await;
    assert_eq!(registration.status(), StatusCode::CREATED);

    let overlong = "z".repeat(MAX_PASSWORD_BYTES + 1);
    let existing = post_json(
        app.clone(),
        "/auth/login",
        serde_json::json!({
            "email": existing_email,
            "password": overlong,
        }),
    )
    .await;
    let unknown = post_json(
        app,
        "/auth/login",
        serde_json::json!({
            "email": format!("unknown-{}@example.invalid", Uuid::new_v4()),
            "password": "z".repeat(MAX_PASSWORD_BYTES + 1),
        }),
    )
    .await;

    assert_eq!(existing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    let existing_body = to_bytes(existing.into_body(), 16 * 1024)
        .await
        .expect("existing-account error body");
    let unknown_body = to_bytes(unknown.into_body(), 16 * 1024)
        .await
        .expect("unknown-account error body");
    assert_eq!(
        existing_body, unknown_body,
        "the pre-hash guard must not reveal whether an account exists",
    );
}
