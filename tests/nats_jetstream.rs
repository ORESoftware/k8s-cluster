use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream;
use async_trait::async_trait;
use futures_util::StreamExt;
use push_notification_server::{
    ContractVersion, NatsConfig, Notification, OutcomeClass, ProviderError, ProviderKind,
    ProviderReadiness, ProviderRegistry, ProviderSlot, PushJob, PushJobEnvelopeV1, PushOptions,
    PushOutcome, PushProvider, PushTarget, TraceMetadata, run_nats_consumer,
};
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};

struct ScriptedProvider {
    retry_calls: AtomicUsize,
    dead_calls: AtomicUsize,
}

impl ScriptedProvider {
    fn transient_failure() -> ProviderError {
        ProviderError::delivery(
            OutcomeClass::TransientProviderFailure,
            "mock upstream temporarily unavailable",
            Some(Duration::from_millis(50)),
            Some("mock_unavailable".to_owned()),
        )
    }
}

#[async_trait]
impl PushProvider for ScriptedProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Fcm
    }

    fn readiness(&self) -> ProviderReadiness {
        ProviderReadiness::ready()
    }

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        match job.job_id.as_str() {
            "job-retry" => {
                let attempt = self.retry_calls.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err(Self::transient_failure())
                } else {
                    Ok(PushOutcome::accepted(job))
                }
            }
            "job-dead" => {
                self.dead_calls.fetch_add(1, Ordering::SeqCst);
                Err(Self::transient_failure())
            }
            _ => Ok(PushOutcome::accepted(job)),
        }
    }
}

fn fcm_job(job_id: &str, token: &str) -> PushJob {
    PushJob {
        version: ContractVersion::V1,
        job_id: job_id.to_owned(),
        tenant_id: "tenant-jetstream".to_owned(),
        application_id: "app-jetstream".to_owned(),
        idempotency_key: format!("event-{job_id}"),
        provider: ProviderKind::Fcm,
        target: PushTarget::Fcm {
            token: token.to_owned(),
        },
        notification: Notification {
            title: Some("JetStream integration".to_owned()),
            body: Some("Delivery fixture".to_owned()),
            image_url: None,
            data: BTreeMap::from([("source".to_owned(), json!("nats-e2e"))]),
        },
        options: PushOptions::default(),
        trace: TraceMetadata {
            correlation_id: Some(format!("correlation-{job_id}")),
            ..TraceMetadata::default()
        },
    }
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after Unix epoch")
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

async fn wait_for_stream(context: &jetstream::Context, name: &str) {
    timeout(Duration::from_secs(10), async {
        loop {
            if context.get_stream(name).await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("JetStream stream was not created");
}

async fn publish_envelope(context: &jetstream::Context, subject: &str, job: PushJob) {
    let payload = serde_json::to_vec(&PushJobEnvelopeV1::new(job)).expect("serialize envelope");
    context
        .publish(subject.to_owned(), payload.into())
        .await
        .expect("publish envelope")
        .await
        .expect("JetStream publish acknowledgement");
}

async fn publish_raw(context: &jetstream::Context, subject: &str, payload: &[u8]) {
    context
        .publish(subject.to_owned(), payload.to_vec().into())
        .await
        .expect("publish raw message")
        .await
        .expect("JetStream publish acknowledgement");
}

async fn next_json(subscriber: &mut async_nats::Subscriber) -> Value {
    let message = timeout(Duration::from_secs(12), subscriber.next())
        .await
        .expect("NATS message timeout")
        .expect("NATS subscriber ended");
    serde_json::from_slice(&message.payload).expect("JSON event")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live NATS server with JetStream enabled"]
async fn live_jetstream_retries_redelivers_and_dead_letters_without_leaking_targets() {
    let mut config = NatsConfig::from_env()
        .expect("NATS configuration")
        .expect("NATS_URL must be configured for this ignored test");
    let suffix = unique_suffix();
    config.job_stream = format!("PUSH_JOBS_{suffix}").replace('-', "_");
    config.result_stream = format!("PUSH_RESULTS_{suffix}").replace('-', "_");
    config.dead_stream = format!("PUSH_DEAD_{suffix}").replace('-', "_");
    config.job_subject = format!("push.jobs.{suffix}");
    config.result_subject = format!("push.results.{suffix}");
    config.dead_subject = format!("push.dead.{suffix}");
    config.consumer = format!("push-consumer-{suffix}");
    config.ack_wait = Duration::from_secs(5);
    config.nak_delay = Duration::from_millis(100);
    config.max_deliver = 3;
    config.max_concurrency = 4;

    let provider = Arc::new(ScriptedProvider {
        retry_calls: AtomicUsize::new(0),
        dead_calls: AtomicUsize::new(0),
    });
    let registry = ProviderRegistry::new()
        .with_provider(ProviderSlot::Fcm, provider.clone())
        .expect("scripted provider");

    let client = async_nats::connect(config.url.clone())
        .await
        .expect("connect test client");
    let mut results = client
        .subscribe(config.result_subject.clone())
        .await
        .expect("subscribe results");
    let mut dead_letters = client
        .subscribe(config.dead_subject.clone())
        .await
        .expect("subscribe dead letters");
    client.flush().await.expect("flush subscriptions");

    let consumer_config = config.clone();
    let consumer = tokio::spawn(async move { run_nats_consumer(consumer_config, registry).await });
    let context = jetstream::new(client.clone());
    wait_for_stream(&context, &config.job_stream).await;

    let retry_token = "fcm:retry_capability_123456";
    publish_envelope(
        &context,
        &config.job_subject,
        fcm_job("job-retry", retry_token),
    )
    .await;
    let retry_first = next_json(&mut results).await;
    let retry_second = next_json(&mut results).await;
    assert_eq!(
        retry_first["outcome"]["class"],
        "transient_provider_failure"
    );
    assert_eq!(retry_first["delivery_attempt"], 1);
    assert_eq!(retry_second["outcome"]["class"], "accepted");
    assert_eq!(retry_second["delivery_attempt"], 2);
    assert_eq!(provider.retry_calls.load(Ordering::SeqCst), 2);
    assert!(!retry_first.to_string().contains(retry_token));
    assert!(!retry_second.to_string().contains(retry_token));

    let dead_token = "fcm:dead_capability_123456";
    publish_envelope(
        &context,
        &config.job_subject,
        fcm_job("job-dead", dead_token),
    )
    .await;
    for expected_attempt in 1..=3 {
        let event = next_json(&mut results).await;
        assert_eq!(event["outcome"]["job_id"], "job-dead");
        assert_eq!(event["outcome"]["class"], "transient_provider_failure");
        assert_eq!(event["delivery_attempt"], expected_attempt);
        assert!(!event.to_string().contains(dead_token));
    }
    let exhausted = next_json(&mut dead_letters).await;
    assert_eq!(exhausted["reason_code"], "retry_budget_exhausted");
    assert_eq!(exhausted["job_id"], "job-dead");
    assert_eq!(exhausted["delivery_attempt"], 3);
    assert_eq!(exhausted["max_deliver"], 3);
    assert_eq!(
        exhausted["payload_sha256"].as_str().map(str::len),
        Some(64)
    );
    assert!(!exhausted.to_string().contains(dead_token));
    assert_eq!(provider.dead_calls.load(Ordering::SeqCst), 3);

    let poison_payload = br#"{\"schema\":\"not-a-valid-envelope\",\"token\":\"poison-secret\"}"#;
    publish_raw(&context, &config.job_subject, poison_payload).await;
    let poison = next_json(&mut dead_letters).await;
    assert_eq!(poison["reason_code"], "invalid_envelope_json");
    assert_eq!(poison["payload_bytes"], poison_payload.len());
    assert_eq!(poison["payload_sha256"].as_str().map(str::len), Some(64));
    assert!(!poison.to_string().contains("poison-secret"));

    consumer.abort();
}
