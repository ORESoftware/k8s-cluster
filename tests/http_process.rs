use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use push_notification_server::{
    ApiState, ContractVersion, Notification, OutcomeClass, ProviderEnvironment, ProviderError,
    ProviderKind, ProviderReadiness, ProviderRegistry, ProviderSlot, PushJob, PushOptions,
    PushOutcome, PushProvider, PushTarget, SharedSecretAuthenticator, TraceMetadata, router,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tokio::time::{sleep, timeout};

const AUTH_SECRET: &str = "0123456789abcdef0123456789abcdef";

struct TaggedProvider {
    kind: ProviderKind,
    code: &'static str,
}

#[async_trait]
impl PushProvider for TaggedProvider {
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

struct BlockingProvider {
    started: Notify,
    release: Notify,
}

#[async_trait]
impl PushProvider for BlockingProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Fcm
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        self.started.notify_waiters();
        self.release.notified().await;
        Ok(PushOutcome::accepted(job))
    }
}

fn job(id: &str, target: PushTarget) -> PushJob {
    PushJob {
        version: ContractVersion::V1,
        job_id: id.to_owned(),
        tenant_id: "tenant-integration".to_owned(),
        application_id: "app-integration".to_owned(),
        idempotency_key: format!("event-{id}"),
        provider: target.provider(),
        target,
        notification: Notification {
            title: Some("Integration test".to_owned()),
            body: Some("Provider-neutral delivery".to_owned()),
            image_url: None,
            data: BTreeMap::from([("source".to_owned(), json!("http-e2e"))]),
        },
        options: PushOptions::default(),
        trace: TraceMetadata {
            correlation_id: Some(format!("correlation-{id}")),
            ..TraceMetadata::default()
        },
    }
}

fn provider_jobs() -> Vec<PushJob> {
    vec![
        job(
            "job-fcm",
            PushTarget::Fcm {
                token: "fcm:integration_token_123456".to_owned(),
            },
        ),
        job(
            "job-apns-production",
            PushTarget::Apns {
                token: "01".repeat(32),
                environment: ProviderEnvironment::Production,
            },
        ),
        job(
            "job-apns-sandbox",
            PushTarget::Apns {
                token: "02".repeat(32),
                environment: ProviderEnvironment::Sandbox,
            },
        ),
        job(
            "job-expo",
            PushTarget::Expo {
                token: "ExpoPushToken[integration_token_123456]".to_owned(),
            },
        ),
        job(
            "job-web-push",
            PushTarget::WebPush {
                endpoint: "https://push.services.mozilla.com/wpush/v2/capability-secret".to_owned(),
                p256dh: "integration-p256dh-material".to_owned(),
                auth: "integration-auth-material".to_owned(),
            },
        ),
    ]
}

fn all_provider_registry() -> ProviderRegistry {
    let registry = ProviderRegistry::new()
        .with_provider(
            ProviderSlot::Fcm,
            Arc::new(TaggedProvider {
                kind: ProviderKind::Fcm,
                code: "fcm-mock",
            }),
        )
        .expect("FCM provider");
    let registry = registry
        .with_provider(
            ProviderSlot::ApnsProduction,
            Arc::new(TaggedProvider {
                kind: ProviderKind::Apns,
                code: "apns-production-mock",
            }),
        )
        .expect("APNs production provider");
    let registry = registry
        .with_provider(
            ProviderSlot::ApnsSandbox,
            Arc::new(TaggedProvider {
                kind: ProviderKind::Apns,
                code: "apns-sandbox-mock",
            }),
        )
        .expect("APNs sandbox provider");
    let registry = registry
        .with_provider(
            ProviderSlot::Expo,
            Arc::new(TaggedProvider {
                kind: ProviderKind::Expo,
                code: "expo-mock",
            }),
        )
        .expect("Expo provider");
    registry
        .with_provider(
            ProviderSlot::WebPush,
            Arc::new(TaggedProvider {
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
        .expect("bind HTTP test server");
    let address = listener.local_addr().expect("HTTP test address");
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
async fn real_socket_http_routes_all_providers_and_never_echoes_capabilities() {
    let jobs = provider_jobs();
    let secrets = jobs
        .iter()
        .flat_map(|job| match &job.target {
            PushTarget::Fcm { token }
            | PushTarget::Apns { token, .. }
            | PushTarget::Expo { token } => vec![token.as_str()],
            PushTarget::WebPush {
                endpoint,
                p256dh,
                auth,
            } => vec![endpoint.as_str(), p256dh.as_str(), auth.as_str()],
        })
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let (base_url, shutdown, server) = spawn_server(all_provider_registry()).await;
    let client = reqwest::Client::new();

    let health = client
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .expect("health response");
    assert_eq!(health.status(), StatusCode::OK);

    let readiness = client
        .get(format!("{base_url}/readyz"))
        .send()
        .await
        .expect("readiness response");
    assert_eq!(readiness.status(), StatusCode::OK);
    let readiness: Value = readiness.json().await.expect("readiness JSON");
    assert_eq!(readiness["authentication"]["configured"], true);
    assert_eq!(readiness["providers"]["fcm"]["configured"], true);
    assert_eq!(
        readiness["providers"]["apns_production"]["configured"],
        true
    );
    assert_eq!(readiness["providers"]["apns_sandbox"]["configured"], true);
    assert_eq!(readiness["providers"]["expo"]["configured"], true);
    assert_eq!(readiness["providers"]["web_push"]["configured"], true);

    let unauthorized = client
        .post(format!("{base_url}/v1/push/jobs"))
        .json(&jobs[0])
        .send()
        .await
        .expect("unauthorized response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = client
        .post(format!("{base_url}/v1/push/jobs/batch"))
        .bearer_auth(AUTH_SECRET)
        .json(&json!({"jobs": jobs}))
        .send()
        .await
        .expect("batch response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response_text = response.text().await.expect("batch response body");
    for secret in &secrets {
        assert!(
            !response_text.contains(secret),
            "response leaked capability material: {secret}"
        );
    }
    let response_json: Value = serde_json::from_str(&response_text).expect("batch JSON");
    assert_eq!(response_json["accepted"], 5);
    assert_eq!(response_json["rejected"], 0);
    let codes = response_json["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|outcome| outcome["provider_code"].as_str().expect("provider code"))
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        [
            "fcm-mock",
            "apns-production-mock",
            "apns-sandbox-mock",
            "expo-mock",
            "web-push-mock",
        ]
    );

    let oversized = format!("{{\"padding\":\"{}\"}}", "x".repeat(513 * 1024));
    let oversized_response = client
        .post(format!("{base_url}/v1/push/jobs"))
        .bearer_auth(AUTH_SECRET)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(oversized)
        .send()
        .await
        .expect("oversized response");
    assert_eq!(oversized_response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    shutdown.send(()).expect("signal HTTP shutdown");
    timeout(Duration::from_secs(5), server)
        .await
        .expect("HTTP server shutdown timeout")
        .expect("HTTP server task")
        .expect("HTTP server result");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_drains_an_in_flight_push_request() {
    let provider = Arc::new(BlockingProvider {
        started: Notify::new(),
        release: Notify::new(),
    });
    let registry = ProviderRegistry::new()
        .with_provider(ProviderSlot::Fcm, provider.clone())
        .expect("blocking provider");
    let (base_url, shutdown, server) = spawn_server(registry).await;
    let started = provider.started.notified();
    let client = reqwest::Client::new();
    let request_job = job(
        "job-drain",
        PushTarget::Fcm {
            token: "fcm:drain_token_123456".to_owned(),
        },
    );

    let request = tokio::spawn(async move {
        client
            .post(format!("{base_url}/v1/push/jobs"))
            .bearer_auth(AUTH_SECRET)
            .json(&request_job)
            .send()
            .await
    });

    timeout(Duration::from_secs(3), started)
        .await
        .expect("provider did not start");
    shutdown.send(()).expect("signal graceful shutdown");
    sleep(Duration::from_millis(100)).await;
    assert!(
        !request.is_finished(),
        "in-flight request ended before release"
    );
    assert!(
        !server.is_finished(),
        "server did not drain in-flight request"
    );

    provider.release.notify_waiters();
    let response = timeout(Duration::from_secs(5), request)
        .await
        .expect("request timeout")
        .expect("request task")
        .expect("request response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let outcome: PushOutcome = response.json().await.expect("outcome JSON");
    assert_eq!(outcome.class, OutcomeClass::Accepted);

    timeout(Duration::from_secs(5), server)
        .await
        .expect("server drain timeout")
        .expect("server task")
        .expect("server result");
}
