use std::env;
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::contracts::{ContractVersion, OutcomeClass, PushJob, PushOutcome, TraceMetadata};
use crate::dispatch::ProviderRegistry;
use crate::provider::ProviderError;
use crate::redaction::truncate_utf8;
use crate::validation::validate_push_job;

pub const DEFAULT_JOB_STREAM: &str = "PUSH_JOBS_V1";
pub const DEFAULT_RESULT_STREAM: &str = "PUSH_RESULTS_V1";
pub const DEFAULT_DEAD_STREAM: &str = "PUSH_DEAD_V1";
pub const DEFAULT_JOB_SUBJECT: &str = "push.jobs.v1";
pub const DEFAULT_RESULT_SUBJECT: &str = "push.results.v1";
pub const DEFAULT_DEAD_SUBJECT: &str = "push.dead.v1";
pub const DEFAULT_CONSUMER: &str = "push-notification-server-v1";

const ENVELOPE_SCHEMA: &str = "push.job.envelope.v1";
const RESULT_SCHEMA: &str = "push.result.v1";
const DEAD_SCHEMA: &str = "push.dead.v1";
const MAX_SAFE_DETAIL_BYTES: usize = 512;
const DEFAULT_MAX_PAYLOAD_BYTES: usize = 512 * 1024;

pub type NatsRuntimeError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub url: String,
    pub job_stream: String,
    pub result_stream: String,
    pub dead_stream: String,
    pub job_subject: String,
    pub result_subject: String,
    pub dead_subject: String,
    pub consumer: String,
    pub ack_wait: Duration,
    pub nak_delay: Duration,
    pub max_deliver: i64,
    pub max_payload_bytes: usize,
    pub max_concurrency: usize,
    envelope_secret: Option<Arc<[u8]>>,
}

impl NatsConfig {
    pub fn from_env() -> Result<Option<Self>, NatsConfigError> {
        let Some(url) = non_empty_env("NATS_URL") else {
            return Ok(None);
        };
        let envelope_auth_enabled = environment_flag("ENABLE_NATS_ENVELOPE_AUTH");
        let envelope_secret = if envelope_auth_enabled {
            let value = non_empty_env("NATS_SHARED_SECRET")
                .ok_or(NatsConfigError::MissingEnvelopeSecret)?;
            validate_secret(&value)?;
            Some(Arc::from(value.as_bytes()))
        } else {
            None
        };

        Ok(Some(Self {
            url,
            job_stream: env_value("NATS_PUSH_JOB_STREAM", DEFAULT_JOB_STREAM),
            result_stream: env_value("NATS_PUSH_RESULT_STREAM", DEFAULT_RESULT_STREAM),
            dead_stream: env_value("NATS_PUSH_DEAD_STREAM", DEFAULT_DEAD_STREAM),
            job_subject: env_value("NATS_PUSH_JOB_SUBJECT", DEFAULT_JOB_SUBJECT),
            result_subject: env_value("NATS_PUSH_RESULT_SUBJECT", DEFAULT_RESULT_SUBJECT),
            dead_subject: env_value("NATS_PUSH_DEAD_SUBJECT", DEFAULT_DEAD_SUBJECT),
            consumer: env_value("NATS_PUSH_CONSUMER", DEFAULT_CONSUMER),
            ack_wait: Duration::from_secs(positive_u64("NATS_PUSH_ACK_WAIT_SECONDS", 120)?),
            nak_delay: Duration::from_secs(positive_u64("NATS_PUSH_NAK_DELAY_SECONDS", 15)?),
            max_deliver: positive_i64("NATS_PUSH_MAX_DELIVER", 5)?,
            max_payload_bytes: positive_usize(
                "NATS_PUSH_MAX_PAYLOAD_BYTES",
                DEFAULT_MAX_PAYLOAD_BYTES,
            )?,
            max_concurrency: positive_usize("NATS_PUSH_MAX_CONCURRENCY", 32)?,
            envelope_secret,
        }))
    }

    pub fn envelope_auth_required(&self) -> bool {
        self.envelope_secret.is_some()
    }

    fn authenticate(&self, supplied: Option<&str>) -> bool {
        match &self.envelope_secret {
            None => true,
            Some(expected) => supplied
                .map(|value| constant_time_eq(value.as_bytes(), expected))
                .unwrap_or(false),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NatsConfigError {
    #[error("ENABLE_NATS_ENVELOPE_AUTH is true but NATS_SHARED_SECRET is absent")]
    MissingEnvelopeSecret,

    #[error("NATS_SHARED_SECRET must contain 32 to 4096 non-control characters")]
    InvalidEnvelopeSecret,

    #[error("{0} must be a positive integer")]
    InvalidPositiveInteger(&'static str),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushJobEnvelopeV1 {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    pub job: PushJob,
}

impl PushJobEnvelopeV1 {
    pub fn new(job: PushJob) -> Self {
        Self {
            schema: ENVELOPE_SCHEMA.to_owned(),
            auth: None,
            job,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PushResultEventV1 {
    pub schema: &'static str,
    pub outcome: PushOutcome,
    pub trace: TraceMetadata,
    pub delivery_attempt: u64,
    pub emitted_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushDeadLetterEventV1 {
    pub schema: &'static str,
    pub reason_code: String,
    pub payload_sha256: String,
    pub payload_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PushOutcome>,
    pub delivery_attempt: u64,
    pub max_deliver: i64,
    pub emitted_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatsDisposition {
    Ack,
    Nak(Duration),
    DeadLetter,
}

pub async fn run_nats_consumer(
    config: NatsConfig,
    registry: ProviderRegistry,
) -> Result<(), NatsRuntimeError> {
    let client = async_nats::connect(config.url.clone()).await?;
    let context = jetstream::new(client);
    ensure_streams(&context, &config).await?;
    let semaphore = Arc::new(Semaphore::new(config.max_concurrency));
    let config = Arc::new(config);

    loop {
        let consumer = build_consumer(&context, &config).await?;
        let mut messages = consumer.messages().await?;
        while let Some(next) = messages.next().await {
            let message = match next {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!(%error, "JetStream message fetch failed; recreating consumer stream");
                    break;
                }
            };
            let permit = semaphore.clone().acquire_owned().await?;
            let context = context.clone();
            let config = config.clone();
            let registry = registry.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = handle_message(&context, &config, &registry, message).await {
                    tracing::error!(%error, "JetStream push message handling failed");
                }
            });
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn ensure_streams(
    context: &jetstream::Context,
    config: &NatsConfig,
) -> Result<(), NatsRuntimeError> {
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: config.job_stream.clone(),
            subjects: vec![config.job_subject.clone()],
            retention: jetstream::stream::RetentionPolicy::WorkQueue,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            max_message_size: i32::try_from(config.max_payload_bytes).unwrap_or(i32::MAX),
            ..Default::default()
        })
        .await?;
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: config.result_stream.clone(),
            subjects: vec![config.result_subject.clone()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            ..Default::default()
        })
        .await?;
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: config.dead_stream.clone(),
            subjects: vec![config.dead_subject.clone()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            ..Default::default()
        })
        .await?;
    Ok(())
}

async fn build_consumer(
    context: &jetstream::Context,
    config: &NatsConfig,
) -> Result<jetstream::consumer::PullConsumer, NatsRuntimeError> {
    let stream = context.get_stream(&config.job_stream).await?;
    Ok(stream
        .get_or_create_consumer::<jetstream::consumer::pull::Config>(
            &config.consumer,
            jetstream::consumer::pull::Config {
                durable_name: Some(config.consumer.clone()),
                filter_subject: config.job_subject.clone(),
                ack_wait: config.ack_wait,
                max_deliver: config.max_deliver,
                ..Default::default()
            },
        )
        .await?)
}

async fn handle_message(
    context: &jetstream::Context,
    config: &NatsConfig,
    registry: &ProviderRegistry,
    message: jetstream::Message,
) -> Result<(), NatsRuntimeError> {
    let payload = message.payload.as_ref();
    let delivery_attempt = message
        .info()
        .map(|info| info.delivered)
        .unwrap_or(1);

    if payload.len() > config.max_payload_bytes {
        let event = dead_letter_for_payload(
            payload,
            "payload_too_large",
            None,
            None,
            delivery_attempt,
            config.max_deliver,
        );
        publish_json(context, &config.dead_subject, &event).await?;
        message.ack_with(jetstream::AckKind::Term).await?;
        return Ok(());
    }

    let envelope = match serde_json::from_slice::<PushJobEnvelopeV1>(payload) {
        Ok(envelope) => envelope,
        Err(_) => {
            let event = dead_letter_for_payload(
                payload,
                "invalid_envelope_json",
                None,
                None,
                delivery_attempt,
                config.max_deliver,
            );
            publish_json(context, &config.dead_subject, &event).await?;
            message.ack_with(jetstream::AckKind::Term).await?;
            return Ok(());
        }
    };

    if envelope.schema != ENVELOPE_SCHEMA {
        let event = dead_letter_for_payload(
            payload,
            "unsupported_envelope_schema",
            Some(&envelope.job),
            None,
            delivery_attempt,
            config.max_deliver,
        );
        publish_json(context, &config.dead_subject, &event).await?;
        message.ack_with(jetstream::AckKind::Term).await?;
        return Ok(());
    }

    if !config.authenticate(envelope.auth.as_deref()) {
        let event = dead_letter_for_payload(
            payload,
            "envelope_authentication_failed",
            Some(&envelope.job),
            None,
            delivery_attempt,
            config.max_deliver,
        );
        publish_json(context, &config.dead_subject, &event).await?;
        message.ack_with(jetstream::AckKind::Term).await?;
        return Ok(());
    }

    let progress_every = (config.ack_wait / 3).max(Duration::from_secs(5));
    let outcome = run_with_ack_progress(
        &message,
        progress_every,
        process_job(registry, &envelope.job),
    )
    .await;
    let result = PushResultEventV1 {
        schema: RESULT_SCHEMA,
        outcome: outcome.clone(),
        trace: envelope.job.trace.clone(),
        delivery_attempt,
        emitted_at_ms: now_ms(),
    };

    if let Err(error) = publish_json(context, &config.result_subject, &result).await {
        let _ = message
            .ack_with(jetstream::AckKind::Nak(Some(config.nak_delay)))
            .await;
        return Err(error);
    }

    match disposition_for_outcome(
        outcome.class,
        delivery_attempt,
        config.max_deliver,
        config.nak_delay,
    ) {
        NatsDisposition::Ack => message.ack().await?,
        NatsDisposition::Nak(delay) => {
            message
                .ack_with(jetstream::AckKind::Nak(Some(delay)))
                .await?
        }
        NatsDisposition::DeadLetter => {
            let event = dead_letter_for_payload(
                payload,
                "retry_budget_exhausted",
                Some(&envelope.job),
                Some(outcome),
                delivery_attempt,
                config.max_deliver,
            );
            publish_json(context, &config.dead_subject, &event).await?;
            message.ack_with(jetstream::AckKind::Term).await?;
        }
    }
    Ok(())
}

async fn process_job(registry: &ProviderRegistry, job: &PushJob) -> PushOutcome {
    if let Err(errors) = validate_push_job(job) {
        return PushOutcome {
            version: ContractVersion::V1,
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            class: OutcomeClass::InvalidPayload,
            provider_code: Some("contract_validation_failed".to_owned()),
            retry_after_ms: None,
            safe_detail: Some(truncate_utf8(
                &errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
                MAX_SAFE_DETAIL_BYTES,
            )),
        };
    }

    match registry.dispatch(job).await {
        Ok(outcome) => outcome,
        Err(error) => outcome_from_provider_error(job, error),
    }
}

fn outcome_from_provider_error(job: &PushJob, error: ProviderError) -> PushOutcome {
    let (class, safe_detail, retry_after, provider_code) = match error {
        ProviderError::NotConfigured { safe_reason } => (
            OutcomeClass::PermanentProviderFailure,
            safe_reason,
            None,
            Some("provider_not_configured".to_owned()),
        ),
        ProviderError::Delivery {
            class,
            safe_detail,
            retry_after,
            provider_code,
        } => (class, safe_detail, retry_after, provider_code),
        ProviderError::Internal { safe_detail } => (
            OutcomeClass::InternalFailure,
            safe_detail,
            None,
            Some("internal_provider_error".to_owned()),
        ),
    };
    PushOutcome {
        version: ContractVersion::V1,
        job_id: job.job_id.clone(),
        provider: job.provider,
        target_fingerprint: job.target.fingerprint(),
        class,
        provider_code,
        retry_after_ms: retry_after.map(duration_to_millis),
        safe_detail: Some(truncate_utf8(&safe_detail, MAX_SAFE_DETAIL_BYTES)),
    }
}

pub fn disposition_for_outcome(
    class: OutcomeClass,
    delivery_attempt: u64,
    max_deliver: i64,
    nak_delay: Duration,
) -> NatsDisposition {
    if !class.is_retryable() {
        return NatsDisposition::Ack;
    }
    if max_deliver > 0 && delivery_attempt >= u64::try_from(max_deliver).unwrap_or(u64::MAX) {
        NatsDisposition::DeadLetter
    } else {
        NatsDisposition::Nak(nak_delay)
    }
}

async fn publish_json<T: Serialize>(
    context: &jetstream::Context,
    subject: &str,
    value: &T,
) -> Result<(), NatsRuntimeError> {
    let payload = serde_json::to_vec(value)?;
    context
        .publish(subject.to_owned(), payload.into())
        .await?
        .await?;
    Ok(())
}

fn dead_letter_for_payload(
    payload: &[u8],
    reason_code: &str,
    job: Option<&PushJob>,
    outcome: Option<PushOutcome>,
    delivery_attempt: u64,
    max_deliver: i64,
) -> PushDeadLetterEventV1 {
    PushDeadLetterEventV1 {
        schema: DEAD_SCHEMA,
        reason_code: reason_code.to_owned(),
        payload_sha256: hex::encode(Sha256::digest(payload)),
        payload_bytes: payload.len(),
        job_id: job.map(|job| job.job_id.clone()),
        tenant_id: job.map(|job| job.tenant_id.clone()),
        application_id: job.map(|job| job.application_id.clone()),
        outcome,
        delivery_attempt,
        max_deliver,
        emitted_at_ms: now_ms(),
    }
}

async fn run_with_ack_progress<F, T>(
    message: &jetstream::Message,
    interval: Duration,
    future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let mut future = std::pin::pin!(future);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        tokio::select! {
            biased;
            output = &mut future => return output,
            _ = ticker.tick() => {
                if let Err(error) = message.ack_with(jetstream::AckKind::Progress).await {
                    tracing::warn!(%error, "JetStream ack-progress heartbeat failed");
                }
            }
        }
    }
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn validate_secret(value: &str) -> Result<(), NatsConfigError> {
    if value.len() < 32 || value.len() > 4096 || value.chars().any(char::is_control) {
        Err(NatsConfigError::InvalidEnvelopeSecret)
    } else {
        Ok(())
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_value(name: &str, default: &str) -> String {
    non_empty_env(name).unwrap_or_else(|| default.to_owned())
}

fn environment_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn positive_u64(name: &'static str, default: u64) -> Result<u64, NatsConfigError> {
    match non_empty_env(name) {
        None => Ok(default),
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(NatsConfigError::InvalidPositiveInteger(name)),
    }
}

fn positive_i64(name: &'static str, default: i64) -> Result<i64, NatsConfigError> {
    match non_empty_env(name) {
        None => Ok(default),
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(NatsConfigError::InvalidPositiveInteger(name)),
    }
}

fn positive_usize(name: &'static str, default: usize) -> Result<usize, NatsConfigError> {
    match non_empty_env(name) {
        None => Ok(default),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(NatsConfigError::InvalidPositiveInteger(name)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use async_trait::async_trait;
    use serde_json::{Value, json};

    use super::*;
    use crate::contracts::{Notification, ProviderKind, PushOptions, PushTarget};
    use crate::dispatch::ProviderSlot;
    use crate::provider::{ProviderReadiness, PushProvider};

    struct AcceptingProvider;

    #[async_trait]
    impl PushProvider for AcceptingProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Fcm
        }

        fn readiness(&self) -> ProviderReadiness {
            ProviderReadiness::ready()
        }

        async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
            Ok(PushOutcome::accepted(job))
        }
    }

    fn job() -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ProviderKind::Fcm,
            target: PushTarget::Fcm {
                token: "fixture-token-123".to_owned(),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                data: BTreeMap::from([("count".to_owned(), json!(1))]),
                ..Notification::default()
            },
            options: PushOptions::default(),
            trace: TraceMetadata::default(),
        }
    }

    fn config_with_secret(secret: Option<String>) -> NatsConfig {
        NatsConfig {
            url: "nats://127.0.0.1:4222".to_owned(),
            job_stream: DEFAULT_JOB_STREAM.to_owned(),
            result_stream: DEFAULT_RESULT_STREAM.to_owned(),
            dead_stream: DEFAULT_DEAD_STREAM.to_owned(),
            job_subject: DEFAULT_JOB_SUBJECT.to_owned(),
            result_subject: DEFAULT_RESULT_SUBJECT.to_owned(),
            dead_subject: DEFAULT_DEAD_SUBJECT.to_owned(),
            consumer: DEFAULT_CONSUMER.to_owned(),
            ack_wait: Duration::from_secs(120),
            nak_delay: Duration::from_secs(15),
            max_deliver: 5,
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
            max_concurrency: 8,
            envelope_secret: secret.map(|value| Arc::from(value.into_bytes())),
        }
    }

    #[test]
    fn envelope_authentication_is_explicit_and_constant_time_compatible() {
        let secret = "s".repeat(32);
        let required = config_with_secret(Some(secret.clone()));
        assert!(required.authenticate(Some(&secret)));
        assert!(!required.authenticate(Some("wrong")));
        assert!(!required.authenticate(None));
        assert!(config_with_secret(None).authenticate(None));
    }

    #[test]
    fn maps_outcomes_to_ack_nak_or_dead_letter() {
        let delay = Duration::from_secs(15);
        assert_eq!(
            disposition_for_outcome(OutcomeClass::Accepted, 1, 5, delay),
            NatsDisposition::Ack
        );
        assert_eq!(
            disposition_for_outcome(OutcomeClass::InvalidToken, 1, 5, delay),
            NatsDisposition::Ack
        );
        assert_eq!(
            disposition_for_outcome(OutcomeClass::Throttled, 2, 5, delay),
            NatsDisposition::Nak(delay)
        );
        assert_eq!(
            disposition_for_outcome(OutcomeClass::InternalFailure, 5, 5, delay),
            NatsDisposition::DeadLetter
        );
    }

    #[tokio::test]
    async fn processing_and_result_events_never_echo_target_capabilities() {
        let registry = ProviderRegistry::new()
            .with_provider(ProviderSlot::Fcm, Arc::new(AcceptingProvider))
            .expect("registry");
        let job = job();
        let token = match &job.target {
            PushTarget::Fcm { token } => token.clone(),
            _ => unreachable!(),
        };
        let outcome = process_job(&registry, &job).await;
        let event = PushResultEventV1 {
            schema: RESULT_SCHEMA,
            outcome,
            trace: job.trace.clone(),
            delivery_attempt: 1,
            emitted_at_ms: 1,
        };
        let encoded = serde_json::to_string(&event).expect("result JSON");
        assert!(!encoded.contains(&token));
    }

    #[test]
    fn dead_letters_hash_payloads_without_copying_raw_capabilities() {
        let payload = serde_json::to_vec(&PushJobEnvelopeV1::new(job())).expect("envelope");
        let token = "fixture-token-123";
        let event = dead_letter_for_payload(
            &payload,
            "invalid_envelope_json",
            None,
            None,
            1,
            5,
        );
        let encoded: Value = serde_json::to_value(event).expect("dead event");
        assert_eq!(encoded["payload_bytes"], payload.len());
        assert!(!encoded.to_string().contains(token));
        assert_eq!(
            encoded["payload_sha256"].as_str().map(str::len),
            Some(64)
        );
    }
}
