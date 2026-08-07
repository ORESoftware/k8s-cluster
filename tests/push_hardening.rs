//! Real-socket hardening tests for the push lanes: authentication strictness,
//! batch bounds, token and Web Push contract validation, provider-error
//! classification, and capability redaction beyond the happy paths covered by
//! `http_process.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use push_notification_server::{
    ApiState, ContractVersion, Notification, OutcomeClass, ProviderError, ProviderKind,
    ProviderReadiness, ProviderRegistry, ProviderSlot, PushJob, PushOptions, PushOutcome,
    PushProvider, PushTarget, SharedSecretAuthenticator, TraceMetadata, router,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::timeout;

const AUTH_SECRET: &str = "push-hardening-secret-32-bytes!!!!";

struct AcceptingProvider {
    kind: ProviderKind,
    code: &'static str,
}

#[async_trait]
impl PushProvider for AcceptingProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        let mut outcome = PushOutcome::accepted(job);
        outcome.provider_code = Some(self.code.to_owned());
        Ok(outcome)
    }
}

struct FailingProvider {
    kind: ProviderKind,
    make_error: fn() -> ProviderError,
}

#[async_trait]
impl PushProvider for FailingProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, _job: &PushJob) -> Result<PushOutcome, ProviderError> {
        Err((self.make_error)())
    }
}

fn job(id: &str, target: PushTarget) -> PushJob {
    PushJob {
        version: ContractVersion::V1,
        job_id: id.to_owned(),
        tenant_id: "tenant-hardening".to_owned(),
        application_id: "app-hardening".to_owned(),
        idempotency_key: format!("event-{id}"),
        provider: target.provider(),
        target,
        notification: Notification {
            title: Some("Hardening test".to_owned()),
            body: Some("Push lane hardening".to_owned()),
            image_url: None,
            data: BTreeMap::new(),
        },
        options: PushOptions::default(),
        trace: TraceMetadata::default(),
    }
}

fn fcm_job(id: &str, token: &str) -> PushJob {
    job(
        id,
        PushTarget::Fcm {
            token: token.to_owned(),
        },
    )
}

fn web_push_job(id: &str, endpoint: &str, p256dh: &str, auth: &str) -> PushJob {
    job(
        id,
        PushTarget::WebPush {
            endpoint: endpoint.to_owned(),
            p256dh: p256dh.to_owned(),
            auth: auth.to_owned(),
        },
    )
}

fn fcm_only_registry() -> ProviderRegistry {
    ProviderRegistry::new()
        .with_provider(
            ProviderSlot::Fcm,
            Arc::new(AcceptingProvider {
                kind: ProviderKind::Fcm,
                code: "fcm-mock",
            }),
        )
        .expect("FCM provider")
}

fn fcm_and_web_push_registry() -> ProviderRegistry {
    fcm_only_registry()
        .with_provider(
            ProviderSlot::WebPush,
            Arc::new(AcceptingProvider {
                kind: ProviderKind::WebPush,
                code: "web-push-mock",
            }),
        )
        .expect("Web Push provider")
}

async fn spawn_server(
    registry: ProviderRegistry,
) -> (
    String,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), std::io::Error>>,
) {
    let authenticator = SharedSecretAuthenticator::new(AUTH_SECRET).expect("authenticator");
    let app = router(ApiState::new(registry, Arc::new(authenticator)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind push hardening server");
    let address = listener.local_addr().expect("push hardening address");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    (format!("http://{address}"), shutdown_tx, server)
}

async fn stop_server(
    shutdown: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<Result<(), std::io::Error>>,
) {
    shutdown.send(()).expect("signal shutdown");
    timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_is_public_while_push_routes_fail_closed() {
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();

    let health = client
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .expect("health response");
    assert_eq!(health.status(), StatusCode::OK);

    let push_job = fcm_job("job-auth", "fcm:hardening_token_123456");
    let url = format!("{base_url}/v1/push/jobs");

    let wrong_secret = client
        .post(&url)
        .bearer_auth("push-hardening-wrong-secret-32-b!!")
        .json(&push_job)
        .send()
        .await
        .expect("wrong-secret response");
    assert_eq!(wrong_secret.status(), StatusCode::UNAUTHORIZED);

    let basic_scheme = client
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
        .json(&push_job)
        .send()
        .await
        .expect("basic-scheme response");
    assert_eq!(basic_scheme.status(), StatusCode::UNAUTHORIZED);

    let lowercase_scheme = client
        .post(&url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("bearer {AUTH_SECRET}"),
        )
        .json(&push_job)
        .send()
        .await
        .expect("lowercase-scheme response");
    assert_eq!(lowercase_scheme.status(), StatusCode::UNAUTHORIZED);

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readyz_reports_unready_without_configured_providers() {
    let (base_url, shutdown, server) = spawn_server(ProviderRegistry::new()).await;
    let client = reqwest::Client::new();

    let readiness = client
        .get(format!("{base_url}/readyz"))
        .send()
        .await
        .expect("readiness response");
    assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
    let readiness: Value = readiness.json().await.expect("readiness JSON");
    assert_eq!(readiness["ok"], false);
    assert_eq!(readiness["authentication"]["configured"], true);

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_size_bounds_are_enforced() {
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/push/jobs/batch");

    let empty = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&json!({"jobs": []}))
        .send()
        .await
        .expect("empty batch response");
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);
    let empty: Value = empty.json().await.expect("empty batch JSON");
    assert_eq!(empty["error"]["code"], "invalid_batch_size");

    let oversized_jobs: Vec<PushJob> = (0..101)
        .map(|index| fcm_job(&format!("job-{index}"), "fcm:hardening_token_123456"))
        .collect();
    let oversized = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&json!({"jobs": oversized_jobs}))
        .send()
        .await
        .expect("oversized batch response");
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
    let oversized: Value = oversized.json().await.expect("oversized batch JSON");
    assert_eq!(oversized["error"]["code"], "invalid_batch_size");

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_batch_returns_multi_status_and_redacts_tokens() {
    let valid_token = "fcm:valid_hardening_token_123456";
    let short_token = "tiny";
    let jobs = vec![
        fcm_job("job-valid", valid_token),
        fcm_job("job-short-token", short_token),
        // APNs has no configured provider in this registry.
        job(
            "job-orphan",
            PushTarget::Apns {
                token: "03".repeat(32),
                environment: push_notification_server::ProviderEnvironment::Production,
            },
        ),
    ];
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{base_url}/v1/push/jobs/batch"))
        .bearer_auth(AUTH_SECRET)
        .json(&json!({"jobs": jobs}))
        .send()
        .await
        .expect("mixed batch response");
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let body = response.text().await.expect("mixed batch body");
    assert!(!body.contains(valid_token));
    assert!(!body.contains(&"03".repeat(32)));
    let body: Value = serde_json::from_str(&body).expect("mixed batch JSON");
    assert_eq!(body["accepted"], 1);
    assert_eq!(body["rejected"], 2);
    assert_eq!(body["outcomes"][0]["class"], "accepted");
    assert_eq!(body["outcomes"][0]["provider_code"], "fcm-mock");
    assert_eq!(body["outcomes"][1]["class"], "invalid_payload");
    assert_eq!(
        body["outcomes"][2]["provider_code"],
        "provider_not_configured"
    );
    for outcome in body["outcomes"].as_array().expect("outcomes array") {
        assert!(outcome["target_fingerprint"].is_string());
    }

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_shape_is_strictly_validated_and_never_echoed() {
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/push/jobs");

    let long_token = "x".repeat(4_097);
    let cases = [
        ("too-short", "seven77".to_owned()),
        ("too-long", long_token),
        ("path-traversal", "token/../../3/device/other".to_owned()),
        ("whitespace", "token with spaces".to_owned()),
        ("control", "token\r\ninjected".to_owned()),
        ("query-significant", "token?priority=high&x=1".to_owned()),
    ];
    for (label, token) in cases {
        let response = client
            .post(&url)
            .bearer_auth(AUTH_SECRET)
            .json(&fcm_job(&format!("job-{label}"), &token))
            .send()
            .await
            .expect("invalid token response");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "expected rejection for {label}"
        );
        let body = response.text().await.expect("invalid token body");
        assert!(
            !body.contains(token.trim()),
            "token for {label} must not be echoed"
        );
        assert!(body.contains("target.fcm.token"));
    }

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_push_contract_rejects_unsafe_endpoints_and_key_material() {
    let (base_url, shutdown, server) = spawn_server(fcm_and_web_push_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/push/jobs");
    let good_p256dh = "hardening-p256dh-material";
    let good_auth = "hardening-auth-material";

    let endpoint_cases = [
        ("plain-http", "http://push.services.mozilla.com/wpush/v2/x"),
        (
            "non-443-port",
            "https://push.services.mozilla.com:8443/wpush/v2/x",
        ),
        (
            "embedded-credentials",
            "https://user:pass@push.services.mozilla.com/wpush/v2/x",
        ),
        ("not-a-url", "not a url at all"),
    ];
    for (label, endpoint) in endpoint_cases {
        let response = client
            .post(&url)
            .bearer_auth(AUTH_SECRET)
            .json(&web_push_job(
                &format!("job-{label}"),
                endpoint,
                good_p256dh,
                good_auth,
            ))
            .send()
            .await
            .expect("unsafe endpoint response");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "expected rejection for {label}"
        );
    }

    let short_key = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&web_push_job(
            "job-short-key",
            "https://push.services.mozilla.com/wpush/v2/x",
            "short",
            good_auth,
        ))
        .send()
        .await
        .expect("short key response");
    assert_eq!(short_key.status(), StatusCode::BAD_REQUEST);

    // An explicit :443 port and a well-formed subscription still pass the
    // contract and reach the provider.
    let accepted = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&web_push_job(
            "job-explicit-port",
            "https://push.services.mozilla.com:443/wpush/v2/capability",
            good_p256dh,
            good_auth,
        ))
        .send()
        .await
        .expect("explicit port response");
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let accepted_body = accepted.text().await.expect("accepted body");
    assert!(!accepted_body.contains("capability"));
    assert!(!accepted_body.contains(good_p256dh));
    assert!(!accepted_body.contains(good_auth));

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_push_bodies_are_rejected() {
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();

    let oversized = format!("{{\"padding\":\"{}\"}}", "x".repeat(513 * 1024));
    let response = client
        .post(format!("{base_url}/v1/push/jobs"))
        .bearer_auth(AUTH_SECRET)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(oversized)
        .send()
        .await
        .expect("oversized response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_and_option_bounds_are_enforced() {
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/push/jobs");

    let mut empty_notification = fcm_job("job-empty", "fcm:hardening_token_123456");
    empty_notification.notification = Notification::default();
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&empty_notification)
        .send()
        .await
        .expect("empty notification response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let mut oversized_title = fcm_job("job-title", "fcm:hardening_token_123456");
    oversized_title.notification.title = Some("t".repeat(257));
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&oversized_title)
        .send()
        .await
        .expect("oversized title response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let mut oversized_ttl = fcm_job("job-ttl", "fcm:hardening_token_123456");
    oversized_ttl.options.ttl_seconds = Some(28 * 24 * 60 * 60 + 1);
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&oversized_ttl)
        .send()
        .await
        .expect("oversized ttl response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    stop_server(shutdown, server).await;
}

type ProviderErrorCase = (&'static str, fn() -> ProviderError, StatusCode);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_error_classes_map_to_http_statuses() {
    let cases: [ProviderErrorCase; 5] = [
        (
            "throttled",
            || ProviderError::Delivery {
                class: OutcomeClass::Throttled,
                safe_detail: "provider throttled the request".to_owned(),
                retry_after: Some(Duration::from_secs(15)),
                provider_code: Some("rate_limited".to_owned()),
            },
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            "invalid-token",
            || ProviderError::Delivery {
                class: OutcomeClass::InvalidToken,
                safe_detail: "provider rejected the token".to_owned(),
                retry_after: None,
                provider_code: Some("unregistered".to_owned()),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "transient",
            || ProviderError::Delivery {
                class: OutcomeClass::TransientProviderFailure,
                safe_detail: "provider unavailable".to_owned(),
                retry_after: None,
                provider_code: Some("upstream_unavailable".to_owned()),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "permanent",
            || ProviderError::Delivery {
                class: OutcomeClass::PermanentProviderFailure,
                safe_detail: "provider rejected the request".to_owned(),
                retry_after: None,
                provider_code: Some("permanent_rejection".to_owned()),
            },
            StatusCode::BAD_GATEWAY,
        ),
        (
            "internal",
            || ProviderError::Internal {
                safe_detail: "adapter failure".to_owned(),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ];

    for (label, make_error, expected_status) in cases {
        let registry = ProviderRegistry::new()
            .with_provider(
                ProviderSlot::Fcm,
                Arc::new(FailingProvider {
                    kind: ProviderKind::Fcm,
                    make_error,
                }),
            )
            .expect("failing FCM provider");
        let (base_url, shutdown, server) = spawn_server(registry).await;
        let client = reqwest::Client::new();

        let response = client
            .post(format!("{base_url}/v1/push/jobs"))
            .bearer_auth(AUTH_SECRET)
            .json(&fcm_job(
                &format!("job-{label}"),
                "fcm:hardening_token_123456",
            ))
            .send()
            .await
            .expect("provider error response");
        assert_eq!(response.status(), expected_status, "case {label}");
        let outcome: Value = response.json().await.expect("provider error JSON");
        if label == "throttled" {
            assert_eq!(outcome["retry_after_ms"], 15_000);
        }
        if label == "internal" {
            assert_eq!(outcome["provider_code"], "internal_provider_error");
        }

        stop_server(shutdown, server).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_bodies_are_rejected_with_client_errors() {
    let (base_url, shutdown, server) = spawn_server(fcm_only_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/push/jobs");

    let broken_syntax = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{not json")
        .send()
        .await
        .expect("broken syntax response");
    assert_eq!(broken_syntax.status(), StatusCode::BAD_REQUEST);

    let wrong_shape = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{\"unexpected\":true}")
        .send()
        .await
        .expect("wrong shape response");
    assert_eq!(wrong_shape.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let missing_content_type = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .body(
            serde_json::to_vec(&fcm_job("job-ct", "fcm:hardening_token_123456")).expect("job JSON"),
        )
        .send()
        .await
        .expect("missing content type response");
    assert_eq!(
        missing_content_type.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    stop_server(shutdown, server).await;
}
