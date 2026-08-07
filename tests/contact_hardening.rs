//! Real-socket hardening tests for the contact (SendGrid/Twilio) lanes:
//! authentication strictness, batch bounds, provider-error classification,
//! recipient redaction, and contract-validation edges beyond the happy paths
//! covered by `contact_process.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use push_notification_server::{
    ContactApiState, ContactContent, ContactJob, ContactOutcome, ContactOutcomeClass,
    ContactProvider, ContactProviderError, ContactProviderKind, ContactProviderRegistry,
    ContactTarget, ContractVersion, ProviderReadiness, SharedSecretAuthenticator, TraceMetadata,
    contact_router,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::timeout;

const AUTH_SECRET: &str = "contact-hardening-secret-32-bytes!!";

struct AcceptingProvider {
    kind: ContactProviderKind,
    code: &'static str,
}

#[async_trait]
impl ContactProvider for AcceptingProvider {
    fn kind(&self) -> ContactProviderKind {
        self.kind
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
        Ok(ContactOutcome::accepted(job, Some(self.code.to_owned())))
    }
}

struct FailingProvider {
    kind: ContactProviderKind,
    make_error: fn() -> ContactProviderError,
}

#[async_trait]
impl ContactProvider for FailingProvider {
    fn kind(&self) -> ContactProviderKind {
        self.kind
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, _job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
        Err((self.make_error)())
    }
}

fn email_job(id: &str, address: &str) -> ContactJob {
    ContactJob {
        version: ContractVersion::V1,
        job_id: id.to_owned(),
        tenant_id: "tenant-hardening".to_owned(),
        application_id: "app-hardening".to_owned(),
        idempotency_key: format!("event-{id}"),
        provider: ContactProviderKind::Sendgrid,
        target: ContactTarget::Email {
            address: address.to_owned(),
            name: Some("Hardening Recipient".to_owned()),
        },
        content: ContactContent::Email {
            subject: Some("Hardening email".to_owned()),
            text: Some("Contact lane hardening".to_owned()),
            html: None,
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        },
        trace: TraceMetadata::default(),
    }
}

fn sms_job(id: &str, e164: &str, body: &str) -> ContactJob {
    ContactJob {
        version: ContractVersion::V1,
        job_id: id.to_owned(),
        tenant_id: "tenant-hardening".to_owned(),
        application_id: "app-hardening".to_owned(),
        idempotency_key: format!("event-{id}"),
        provider: ContactProviderKind::Twilio,
        target: ContactTarget::Sms {
            e164: e164.to_owned(),
        },
        content: ContactContent::Sms {
            body: body.to_owned(),
        },
        trace: TraceMetadata::default(),
    }
}

fn sendgrid_only_registry() -> ContactProviderRegistry {
    ContactProviderRegistry::new().with_provider(Arc::new(AcceptingProvider {
        kind: ContactProviderKind::Sendgrid,
        code: "sendgrid-mock",
    }))
}

fn full_registry() -> ContactProviderRegistry {
    sendgrid_only_registry().with_provider(Arc::new(AcceptingProvider {
        kind: ContactProviderKind::Twilio,
        code: "twilio-mock",
    }))
}

async fn spawn_server(
    registry: ContactProviderRegistry,
) -> (
    String,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), std::io::Error>>,
) {
    let authenticator = SharedSecretAuthenticator::new(AUTH_SECRET).expect("authenticator");
    let app = contact_router(ContactApiState::new(registry, Arc::new(authenticator)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind contact hardening server");
    let address = listener.local_addr().expect("contact hardening address");
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
async fn wrong_or_malformed_bearer_credentials_are_rejected() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let job = email_job("job-auth", "auth@example.invalid");
    let url = format!("{base_url}/v1/contact/jobs");

    let wrong_secret = client
        .post(&url)
        .bearer_auth("contact-hardening-wrong-secret-32b!!")
        .json(&job)
        .send()
        .await
        .expect("wrong-secret response");
    assert_eq!(wrong_secret.status(), StatusCode::UNAUTHORIZED);

    let basic_scheme = client
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
        .json(&job)
        .send()
        .await
        .expect("basic-scheme response");
    assert_eq!(basic_scheme.status(), StatusCode::UNAUTHORIZED);

    // The Bearer scheme prefix is matched exactly; a lowercase scheme must not
    // authenticate by accident.
    let lowercase_scheme = client
        .post(&url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("bearer {AUTH_SECRET}"),
        )
        .json(&job)
        .send()
        .await
        .expect("lowercase-scheme response");
    assert_eq!(lowercase_scheme.status(), StatusCode::UNAUTHORIZED);

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readyz_reports_unready_without_configured_providers() {
    let (base_url, shutdown, server) = spawn_server(ContactProviderRegistry::new()).await;
    let client = reqwest::Client::new();

    let readiness = client
        .get(format!("{base_url}/v1/contact/readyz"))
        .send()
        .await
        .expect("readiness response");
    assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
    let readiness: Value = readiness.json().await.expect("readiness JSON");
    assert_eq!(readiness["ok"], false);
    assert_eq!(readiness["authentication_configured"], true);
    assert_eq!(readiness["providers"]["sendgrid"]["configured"], false);
    assert_eq!(readiness["providers"]["twilio"]["configured"], false);

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_size_bounds_are_enforced() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/contact/jobs/batch");

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

    let oversized_jobs: Vec<ContactJob> = (0..101)
        .map(|index| sms_job(&format!("job-{index}"), "+15550002222", "batch bound"))
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
async fn mixed_batch_returns_multi_status_and_redacts_recipients() {
    let valid_address = "valid@example.invalid";
    let invalid_address = "not an address";
    let orphan_phone = "+15550003333";
    let jobs = vec![
        email_job("job-valid", valid_address),
        email_job("job-invalid", invalid_address),
        // Twilio is not configured in this registry, so this dispatch fails
        // with provider_not_configured rather than validation errors.
        sms_job("job-orphan", orphan_phone, "no provider"),
    ];
    let (base_url, shutdown, server) = spawn_server(sendgrid_only_registry()).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{base_url}/v1/contact/jobs/batch"))
        .bearer_auth(AUTH_SECRET)
        .json(&json!({"jobs": jobs}))
        .send()
        .await
        .expect("mixed batch response");
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let body = response.text().await.expect("mixed batch body");
    assert!(!body.contains(valid_address));
    assert!(!body.contains(invalid_address));
    assert!(!body.contains(orphan_phone));
    let body: Value = serde_json::from_str(&body).expect("mixed batch JSON");
    assert_eq!(body["accepted"], 1);
    assert_eq!(body["rejected"], 2);
    assert_eq!(body["outcomes"][0]["class"], "accepted");
    assert_eq!(body["outcomes"][1]["class"], "invalid_payload");
    assert_eq!(
        body["outcomes"][2]["provider_code"],
        "provider_not_configured"
    );

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subject_and_display_name_header_injection_is_rejected() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/contact/jobs");

    let mut injected_subject = email_job("job-subject", "victim@example.invalid");
    if let ContactContent::Email { subject, .. } = &mut injected_subject.content {
        *subject = Some("Hello\r\nBcc: attacker@example.invalid".to_owned());
    }
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&injected_subject)
        .send()
        .await
        .expect("subject injection response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("subject injection body");
    assert!(!body.contains("attacker@example.invalid"));
    assert!(!body.contains("victim@example.invalid"));
    assert!(body.contains("content.subject contains invalid characters"));

    let mut injected_name = email_job("job-name", "victim@example.invalid");
    injected_name.target = ContactTarget::Email {
        address: "victim@example.invalid".to_owned(),
        name: Some("Evil\r\nX-Injected: header".to_owned()),
    };
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&injected_name)
        .send()
        .await
        .expect("display-name injection response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("display-name injection body");
    assert!(!body.contains("victim@example.invalid"));
    assert!(body.contains("target.email.name contains invalid characters"));

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sms_limit_counts_characters_not_bytes() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/contact/jobs");

    // 1,600 three-byte characters exceed 1,600 bytes but not 1,600 characters.
    let at_limit = sms_job("job-at-limit", "+15550004444", &"€".repeat(1_600));
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&at_limit)
        .send()
        .await
        .expect("at-limit response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let over_limit = sms_job("job-over-limit", "+15550004444", &"€".repeat(1_601));
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&over_limit)
        .send()
        .await
        .expect("over-limit response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.expect("over-limit body");
    assert!(!body.contains("+15550004444"));

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e164_shape_is_strictly_validated() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/contact/jobs");

    for (label, number) in [
        ("zero-after-plus", "+015551234567"),
        ("missing-plus", "15551234567"),
        ("too-short", "+1555123"),
        ("too-long", "+1234567890123456"),
        ("alphabetic", "+1555ABCD567"),
    ] {
        let response = client
            .post(&url)
            .bearer_auth(AUTH_SECRET)
            .json(&sms_job(&format!("job-{label}"), number, "shape check"))
            .send()
            .await
            .expect("invalid E.164 response");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "expected rejection for {label}"
        );
    }

    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&sms_job(
            "job-max-length",
            "+123456789012345",
            "shape check",
        ))
        .send()
        .await
        .expect("valid E.164 response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_error_classes_map_to_http_statuses() {
    type ContactErrorCase = (&'static str, fn() -> ContactProviderError, StatusCode);
    let cases: [ContactErrorCase; 5] = [
        (
            "throttled",
            || ContactProviderError::Delivery {
                class: ContactOutcomeClass::Throttled,
                safe_detail: "provider throttled the request".to_owned(),
                retry_after: Some(Duration::from_secs(30)),
                provider_code: Some("rate_limited".to_owned()),
            },
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            "invalid-target",
            || ContactProviderError::Delivery {
                class: ContactOutcomeClass::InvalidTarget,
                safe_detail: "provider rejected the recipient".to_owned(),
                retry_after: None,
                provider_code: Some("invalid_recipient".to_owned()),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "transient",
            || ContactProviderError::Delivery {
                class: ContactOutcomeClass::TransientProviderFailure,
                safe_detail: "provider unavailable".to_owned(),
                retry_after: None,
                provider_code: Some("upstream_unavailable".to_owned()),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "permanent",
            || ContactProviderError::Delivery {
                class: ContactOutcomeClass::PermanentProviderFailure,
                safe_detail: "provider rejected the request".to_owned(),
                retry_after: None,
                provider_code: Some("permanent_rejection".to_owned()),
            },
            StatusCode::BAD_GATEWAY,
        ),
        (
            "internal",
            || ContactProviderError::Internal {
                safe_detail: "adapter failure".to_owned(),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ];

    for (label, make_error, expected_status) in cases {
        let registry = ContactProviderRegistry::new().with_provider(Arc::new(FailingProvider {
            kind: ContactProviderKind::Sendgrid,
            make_error,
        }));
        let (base_url, shutdown, server) = spawn_server(registry).await;
        let client = reqwest::Client::new();

        let response = client
            .post(format!("{base_url}/v1/contact/jobs"))
            .bearer_auth(AUTH_SECRET)
            .json(&email_job(&format!("job-{label}"), "case@example.invalid"))
            .send()
            .await
            .expect("provider error response");
        assert_eq!(response.status(), expected_status, "case {label}");
        let outcome: Value = response.json().await.expect("provider error JSON");
        if label == "throttled" {
            assert_eq!(outcome["retry_after_ms"], 30_000);
        }
        if label == "internal" {
            assert_eq!(outcome["provider_code"], "internal_provider_error");
        }

        stop_server(shutdown, server).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_failure_details_are_truncated_and_do_not_leak_recipients() {
    let recipient = "leak-probe@example.invalid";
    fn leaking_error() -> ContactProviderError {
        ContactProviderError::Delivery {
            class: ContactOutcomeClass::PermanentProviderFailure,
            safe_detail: format!("{}leak-probe@example.invalid", "x".repeat(600)),
            retry_after: None,
            provider_code: Some("verbose_upstream".to_owned()),
        }
    }
    let registry = ContactProviderRegistry::new().with_provider(Arc::new(FailingProvider {
        kind: ContactProviderKind::Sendgrid,
        make_error: leaking_error,
    }));
    let (base_url, shutdown, server) = spawn_server(registry).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{base_url}/v1/contact/jobs"))
        .bearer_auth(AUTH_SECRET)
        .json(&email_job("job-leak", recipient))
        .send()
        .await
        .expect("leak response");
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.expect("leak body");
    assert!(!body.contains(recipient));
    let outcome: Value = serde_json::from_str(&body).expect("leak JSON");
    let detail = outcome["safe_detail"].as_str().expect("safe detail");
    assert!(
        detail.len() <= 520,
        "safe detail must stay bounded, got {} bytes",
        detail.len()
    );

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_bodies_are_rejected_with_client_errors() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/contact/jobs");

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
        .body(serde_json::to_vec(&email_job("job-ct", "ct@example.invalid")).expect("job JSON"))
        .send()
        .await
        .expect("missing content type response");
    assert_eq!(
        missing_content_type.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    stop_server(shutdown, server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn template_mode_round_trips_and_rejects_bad_template_ids() {
    let (base_url, shutdown, server) = spawn_server(full_registry()).await;
    let client = reqwest::Client::new();
    let url = format!("{base_url}/v1/contact/jobs");

    let mut template = email_job("job-template", "template@example.invalid");
    template.content = ContactContent::Email {
        subject: None,
        text: None,
        html: None,
        template_id: Some("d-0123456789abcdef".to_owned()),
        dynamic_template_data: BTreeMap::from([("name".to_owned(), json!("Recipient"))]),
        reply_to: None,
    };
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&template)
        .send()
        .await
        .expect("template response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let mut bad_template = email_job("job-bad-template", "template@example.invalid");
    bad_template.content = ContactContent::Email {
        subject: None,
        text: None,
        html: None,
        template_id: Some("x-not-a-template".to_owned()),
        dynamic_template_data: BTreeMap::new(),
        reply_to: None,
    };
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&bad_template)
        .send()
        .await
        .expect("bad template response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let mut ambiguous = email_job("job-ambiguous", "template@example.invalid");
    ambiguous.content = ContactContent::Email {
        subject: Some("explicit subject".to_owned()),
        text: None,
        html: None,
        template_id: Some("d-0123456789abcdef".to_owned()),
        dynamic_template_data: BTreeMap::new(),
        reply_to: None,
    };
    let response = client
        .post(&url)
        .bearer_auth(AUTH_SECRET)
        .json(&ambiguous)
        .send()
        .await
        .expect("ambiguous mode response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    stop_server(shutdown, server).await;
}
