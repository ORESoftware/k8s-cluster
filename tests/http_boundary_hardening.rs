use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use push_notification_server::{
    ApiState, AuthConfigError, ContactApiState, ContactBatchRequest, ContactContent, ContactJob,
    ContactProviderKind, ContactProviderRegistry, ContactTarget, ContractVersion,
    DenyAllAuthenticator, Notification, ProviderKind, ProviderRegistry, PushJob, PushOptions,
    PushPriority, PushTarget, SharedSecretAuthenticator, TraceMetadata, contact_router, router,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn push_job() -> PushJob {
    PushJob {
        version: ContractVersion::V1,
        job_id: "boundary-push-1".to_owned(),
        tenant_id: "tenant-1".to_owned(),
        application_id: "app-1".to_owned(),
        idempotency_key: "event-push-1".to_owned(),
        provider: ProviderKind::Fcm,
        target: PushTarget::Fcm {
            token: "device-capability-that-must-not-be-echoed".to_owned(),
        },
        notification: Notification {
            title: Some("Boundary test".to_owned()),
            body: Some("body".to_owned()),
            image_url: None,
            data: BTreeMap::new(),
        },
        options: PushOptions {
            priority: PushPriority::Normal,
            ttl_seconds: None,
            collapse_key: None,
            dry_run: false,
        },
        trace: TraceMetadata::default(),
    }
}

fn contact_job() -> ContactJob {
    ContactJob {
        version: ContractVersion::V1,
        job_id: "boundary-contact-1".to_owned(),
        tenant_id: "tenant-1".to_owned(),
        application_id: "app-1".to_owned(),
        idempotency_key: "event-contact-1".to_owned(),
        provider: ContactProviderKind::Sendgrid,
        target: ContactTarget::Email {
            address: "person@example.com".to_owned(),
            name: None,
        },
        content: ContactContent::Email {
            subject: Some("Boundary test".to_owned()),
            text: Some("body".to_owned()),
            html: None,
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        },
        trace: TraceMetadata::default(),
    }
}

fn push_app(authenticated: bool) -> Router {
    let authenticator: Arc<dyn push_notification_server::RequestAuthenticator> = if authenticated {
        Arc::new(SharedSecretAuthenticator::new(SECRET).expect("valid secret"))
    } else {
        Arc::new(DenyAllAuthenticator)
    };
    router(ApiState::new(ProviderRegistry::new(), authenticator))
}

fn contact_app(authenticated: bool) -> Router {
    let authenticator: Arc<dyn push_notification_server::RequestAuthenticator> = if authenticated {
        Arc::new(SharedSecretAuthenticator::new(SECRET).expect("valid secret"))
    } else {
        Arc::new(DenyAllAuthenticator)
    };
    contact_router(ContactApiState::new(
        ContactProviderRegistry::new(),
        authenticator,
    ))
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 16 * 1024)
        .await
        .expect("bounded response body");
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[test]
fn shared_secret_configuration_enforces_exact_bounds_and_control_rejection() {
    assert_eq!(
        SharedSecretAuthenticator::new("x".repeat(31))
            .err()
            .map(|error| error.to_string()),
        Some(AuthConfigError::InvalidSharedSecret.to_string())
    );
    assert!(SharedSecretAuthenticator::new("x".repeat(32)).is_ok());
    assert!(SharedSecretAuthenticator::new("x".repeat(4096)).is_ok());
    assert!(SharedSecretAuthenticator::new("x".repeat(4097)).is_err());
    assert!(
        SharedSecretAuthenticator::new(format!("{}\n{}", "x".repeat(16), "x".repeat(16)))
            .is_err()
    );
}

#[tokio::test]
async fn readiness_fails_closed_without_authentication_or_providers() {
    let push = push_app(false)
        .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(push.status(), StatusCode::SERVICE_UNAVAILABLE);
    let push_body = response_json(push).await;
    assert_eq!(push_body["ok"], false);
    assert_eq!(push_body["authentication"]["configured"], false);
    assert_eq!(push_body["authentication"]["mode"], "disabled");

    let contact = contact_app(false)
        .oneshot(
            Request::get("/v1/contact/readyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(contact.status(), StatusCode::SERVICE_UNAVAILABLE);
    let contact_body = response_json(contact).await;
    assert_eq!(contact_body["ok"], false);
    assert_eq!(contact_body["authentication_configured"], false);
    assert_eq!(contact_body["authentication_mode"], "disabled");
}

#[tokio::test]
async fn malformed_bearer_variants_are_rejected_without_echoing_secrets() {
    let payload = serde_json::to_vec(&push_job()).unwrap();
    for authorization in [
        None,
        Some(format!("bearer {SECRET}")),
        Some("Bearer definitely-wrong-secret-value".to_owned()),
        Some(format!("Bearer  {SECRET}")),
        Some(format!("Bearer {SECRET} trailing")),
    ] {
        let mut request = Request::post("/v1/push/jobs")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.clone()))
            .unwrap();
        if let Some(value) = authorization {
            request
                .headers_mut()
                .insert(header::AUTHORIZATION, value.parse().unwrap());
        }
        let response = push_app(true).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        let rendered = body.to_string();
        assert_eq!(body["error"]["code"], "unauthorized");
        assert_eq!(
            body["error"]["safe_detail"],
            "request authentication failed"
        );
        assert!(!rendered.contains(SECRET));
        assert!(!rendered.contains("device-capability-that-must-not-be-echoed"));
        assert!(rendered.len() < 2048);
    }
}

#[tokio::test]
async fn authentication_precedes_batch_shape_disclosure() {
    let unauthenticated_push = push_app(false)
        .oneshot(
            Request::post("/v1/push/jobs/batch")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"jobs":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated_push.status(), StatusCode::UNAUTHORIZED);

    let authenticated_push = push_app(true)
        .oneshot(
            Request::post("/v1/push/jobs/batch")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}"))
                .body(Body::from(r#"{"jobs":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authenticated_push.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(authenticated_push).await["error"]["code"],
        "invalid_batch_size"
    );

    let unauthenticated_contact = contact_app(false)
        .oneshot(
            Request::post("/v1/contact/jobs/batch")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"jobs":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated_contact.status(), StatusCode::UNAUTHORIZED);

    let request = ContactBatchRequest { jobs: Vec::new() };
    let authenticated_contact = contact_app(true)
        .oneshot(
            Request::post("/v1/contact/jobs/batch")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}"))
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authenticated_contact.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(authenticated_contact).await["error"]["code"],
        "invalid_batch_size"
    );
}

#[tokio::test]
async fn body_limits_reject_oversized_requests_before_dispatch() {
    let push = push_app(true)
        .oneshot(
            Request::post("/v1/push/jobs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}"))
                .body(Body::from(vec![b'x'; 512 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(push.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let contact = contact_app(true)
        .oneshot(
            Request::post("/v1/contact/jobs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {SECRET}"))
                .body(Body::from(vec![b'x'; 768 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(contact.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn contact_unauthorized_response_never_echoes_recipient_data() {
    let recipient = "private-recipient@example.com";
    let mut job = contact_job();
    job.target = ContactTarget::Email {
        address: recipient.to_owned(),
        name: Some("Private Recipient".to_owned()),
    };
    let response = contact_app(true)
        .oneshot(
            Request::post("/v1/contact/jobs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer wrong")
                .body(Body::from(serde_json::to_vec(&job).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let rendered = response_json(response).await.to_string();
    assert!(!rendered.contains(recipient));
    assert!(!rendered.contains("Private Recipient"));
    assert!(!rendered.contains(SECRET));
}

#[tokio::test]
async fn empty_batch_payload_serializes_to_the_documented_shape() {
    let request = ContactBatchRequest { jobs: Vec::new() };
    assert_eq!(serde_json::to_value(request).unwrap(), json!({"jobs": []}));
}
