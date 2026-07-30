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
use tokio::sync::{Notify, oneshot};
use tokio::time::{sleep, timeout};

const AUTH_SECRET: &str = "contact-integration-secret-32-bytes";

struct TaggedContactProvider {
    kind: ContactProviderKind,
    code: &'static str,
}

#[async_trait]
impl ContactProvider for TaggedContactProvider {
    fn kind(&self) -> ContactProviderKind {
        self.kind
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(
        &self,
        job: &ContactJob,
    ) -> Result<ContactOutcome, ContactProviderError> {
        Ok(ContactOutcome::accepted(job, Some(self.code.to_owned())))
    }
}

struct BlockingContactProvider {
    started: Notify,
    release: Notify,
}

#[async_trait]
impl ContactProvider for BlockingContactProvider {
    fn kind(&self) -> ContactProviderKind {
        ContactProviderKind::Sendgrid
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(
        &self,
        job: &ContactJob,
    ) -> Result<ContactOutcome, ContactProviderError> {
        self.started.notify_waiters();
        self.release.notified().await;
        Ok(ContactOutcome::accepted(job, Some("drained".to_owned())))
    }
}

fn email_job(id: &str, address: &str) -> ContactJob {
    ContactJob {
        version: ContractVersion::V1,
        job_id: id.to_owned(),
        tenant_id: "tenant-integration".to_owned(),
        application_id: "app-integration".to_owned(),
        idempotency_key: format!("event-{id}"),
        provider: ContactProviderKind::Sendgrid,
        target: ContactTarget::Email {
            address: address.to_owned(),
            name: Some("Integration Recipient".to_owned()),
        },
        content: ContactContent::Email {
            subject: Some("Integration email".to_owned()),
            text: Some("Provider-neutral email delivery".to_owned()),
            html: None,
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: Some("support@example.invalid".to_owned()),
        },
        trace: TraceMetadata {
            correlation_id: Some(format!("correlation-{id}")),
            ..TraceMetadata::default()
        },
    }
}

fn sms_job(id: &str, e164: &str) -> ContactJob {
    ContactJob {
        version: ContractVersion::V1,
        job_id: id.to_owned(),
        tenant_id: "tenant-integration".to_owned(),
        application_id: "app-integration".to_owned(),
        idempotency_key: format!("event-{id}"),
        provider: ContactProviderKind::Twilio,
        target: ContactTarget::Sms {
            e164: e164.to_owned(),
        },
        content: ContactContent::Sms {
            body: "Provider-neutral SMS delivery".to_owned(),
        },
        trace: TraceMetadata {
            correlation_id: Some(format!("correlation-{id}")),
            ..TraceMetadata::default()
        },
    }
}

fn all_contact_registry() -> ContactProviderRegistry {
    ContactProviderRegistry::new()
        .with_provider(Arc::new(TaggedContactProvider {
            kind: ContactProviderKind::Sendgrid,
            code: "sendgrid-mock",
        }))
        .with_provider(Arc::new(TaggedContactProvider {
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
        .expect("bind contact test server");
    let address = listener.local_addr().expect("contact test address");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_socket_contact_routes_email_and_sms_without_recipient_leakage() {
    let email_address = "recipient@example.invalid";
    let phone_number = "+15550001111";
    let jobs = vec![
        email_job("job-email", email_address),
        sms_job("job-sms", phone_number),
    ];
    let (base_url, shutdown, server) = spawn_server(all_contact_registry()).await;
    let client = reqwest::Client::new();

    let readiness = client
        .get(format!("{base_url}/v1/contact/readyz"))
        .send()
        .await
        .expect("contact readiness response");
    assert_eq!(readiness.status(), StatusCode::OK);
    let readiness: Value = readiness.json().await.expect("readiness JSON");
    assert_eq!(readiness["authentication_configured"], true);
    assert_eq!(readiness["providers"]["sendgrid"]["configured"], true);
    assert_eq!(readiness["providers"]["twilio"]["configured"], true);

    let unauthorized = client
        .post(format!("{base_url}/v1/contact/jobs"))
        .json(&jobs[0])
        .send()
        .await
        .expect("unauthorized response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = client
        .post(format!("{base_url}/v1/contact/jobs/batch"))
        .bearer_auth(AUTH_SECRET)
        .json(&json!({"jobs": jobs}))
        .send()
        .await
        .expect("contact batch response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response_text = response.text().await.expect("contact batch body");
    assert!(!response_text.contains(email_address));
    assert!(!response_text.contains(phone_number));
    let response_json: Value = serde_json::from_str(&response_text).expect("contact batch JSON");
    assert_eq!(response_json["accepted"], 2);
    assert_eq!(response_json["rejected"], 0);
    assert_eq!(
        response_json["outcomes"][0]["provider_code"],
        "sendgrid-mock"
    );
    assert_eq!(
        response_json["outcomes"][1]["provider_code"],
        "twilio-mock"
    );

    let mut mismatched = email_job("job-mismatch", email_address);
    mismatched.provider = ContactProviderKind::Twilio;
    let rejected = client
        .post(format!("{base_url}/v1/contact/jobs"))
        .bearer_auth(AUTH_SECRET)
        .json(&mismatched)
        .send()
        .await
        .expect("mismatch response");
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let rejected_text = rejected.text().await.expect("mismatch body");
    assert!(!rejected_text.contains(email_address));

    let oversized = format!("{{\"padding\":\"{}\"}}", "x".repeat(769 * 1024));
    let oversized_response = client
        .post(format!("{base_url}/v1/contact/jobs"))
        .bearer_auth(AUTH_SECRET)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(oversized)
        .send()
        .await
        .expect("oversized response");
    assert_eq!(oversized_response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    shutdown.send(()).expect("signal contact shutdown");
    timeout(Duration::from_secs(5), server)
        .await
        .expect("contact server shutdown timeout")
        .expect("contact server task")
        .expect("contact server result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_drains_an_in_flight_contact_request() {
    let provider = Arc::new(BlockingContactProvider {
        started: Notify::new(),
        release: Notify::new(),
    });
    let registry = ContactProviderRegistry::new().with_provider(provider.clone());
    let (base_url, shutdown, server) = spawn_server(registry).await;
    let started = provider.started.notified();
    let client = reqwest::Client::new();
    let request_job = email_job("job-drain", "drain@example.invalid");

    let request = tokio::spawn(async move {
        client
            .post(format!("{base_url}/v1/contact/jobs"))
            .bearer_auth(AUTH_SECRET)
            .json(&request_job)
            .send()
            .await
    });

    timeout(Duration::from_secs(3), started)
        .await
        .expect("contact provider did not start");
    shutdown.send(()).expect("signal graceful shutdown");
    sleep(Duration::from_millis(100)).await;
    assert!(!request.is_finished(), "contact request ended before release");
    assert!(!server.is_finished(), "server did not drain contact request");

    provider.release.notify_waiters();
    let response = timeout(Duration::from_secs(5), request)
        .await
        .expect("contact request timeout")
        .expect("contact request task")
        .expect("contact request response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let outcome: ContactOutcome = response.json().await.expect("contact outcome JSON");
    assert_eq!(outcome.class, ContactOutcomeClass::Accepted);
    assert_eq!(outcome.provider_code.as_deref(), Some("drained"));

    timeout(Duration::from_secs(5), server)
        .await
        .expect("contact server drain timeout")
        .expect("contact server task")
        .expect("contact server result");
}
