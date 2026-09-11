use std::{collections::HashMap, env, fmt::Write as _, time::Duration};

use async_nats::{
    jetstream::{
        self,
        kv::{CreateErrorKind, Store, UpdateErrorKind},
    },
    Client as NatsClient,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const RUN_EVENT_SCHEMA: &str = "dd.browser-automation.run.v1";
pub const PERIOD_STATE_SCHEMA: &str = "dd.browser-automation.period.v1";

#[derive(Clone)]
pub struct MeteringConfig {
    pub enabled: bool,
    pub require_context: bool,
    pub usage_subject: String,
    pub kv_bucket: String,
    pub period_seconds: u64,
    pub max_run_ids_per_period: usize,
    pub contact_email_subject: String,
    pub contact_nats_secret: Option<String>,
    policies: PolicySet,
}

impl MeteringConfig {
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            require_context: false,
            usage_subject: "dd.remote.browser_jobs.usage".to_string(),
            kv_bucket: "DD_BROWSER_USAGE".to_string(),
            period_seconds: 2_592_000,
            max_run_ids_per_period: 10_000,
            contact_email_subject: "dd.remote.contact.email.send".to_string(),
            contact_nats_secret: None,
            policies: PolicySet::default(),
        }
    }

    pub fn from_env() -> Self {
        let policies = env::var("BROWSER_JOB_METERING_POLICIES_JSON")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| match PolicySet::from_json(&value) {
                Ok(policies) => policies,
                Err(error) => {
                    tracing::error!(
                        "browser-job metering policies rejected; notices disabled: {error}"
                    );
                    PolicySet::default()
                }
            })
            .unwrap_or_default();

        Self {
            enabled: env_bool("BROWSER_JOB_METERING_ENABLED", false),
            require_context: env_bool("BROWSER_JOB_REQUIRE_METERING_CONTEXT", false),
            usage_subject: env_value(
                "BROWSER_JOB_METERING_USAGE_SUBJECT",
                "dd.remote.browser_jobs.usage",
            ),
            kv_bucket: env_value("BROWSER_JOB_METERING_KV_BUCKET", "DD_BROWSER_USAGE"),
            period_seconds: env_u64("BROWSER_JOB_METERING_PERIOD_SECONDS", 2_592_000)
                .clamp(3_600, 31_536_000),
            max_run_ids_per_period: env_usize(
                "BROWSER_JOB_METERING_MAX_RUN_IDS_PER_PERIOD",
                10_000,
            )
            .clamp(1, 100_000),
            contact_email_subject: env_value(
                "BROWSER_JOB_CONTACT_EMAIL_SUBJECT",
                "dd.remote.contact.email.send",
            ),
            contact_nats_secret: env_string("BROWSER_JOB_CONTACT_NATS_SECRET"),
            policies,
        }
    }

    pub fn policy_for(&self, context: &UsageContext) -> Option<&UpgradePolicy> {
        self.policies.get(&context.tenant_id, &context.product_id)
    }

    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_value(name: &str, fallback: &str) -> String {
    env_string(name).unwrap_or_else(|| fallback.to_string())
}

fn env_u64(name: &str, fallback: u64) -> u64 {
    env_string(name)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn env_usize(name: &str, fallback: usize) -> usize {
    env_string(name)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(fallback)
}

fn env_bool(name: &str, fallback: bool) -> bool {
    env_string(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageContext {
    pub tenant_id: String,
    pub actor_id: String,
    pub automation_id: String,
    pub product_id: String,
}

impl UsageContext {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("tenantId", self.tenant_id.as_str()),
            ("actorId", self.actor_id.as_str()),
            ("automationId", self.automation_id.as_str()),
            ("productId", self.product_id.as_str()),
        ] {
            validate_opaque_id(name, value)?;
        }
        Ok(())
    }
}

fn validate_opaque_id(name: &str, value: &str) -> Result<(), String> {
    if !(1..=128).contains(&value.len()) {
        return Err(format!("{name} must be 1..=128 bytes"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!(
            "{name} must be an opaque identifier using only ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NoticeMode {
    Enabled,
    DryRun,
    Suppressed,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradePolicy {
    pub tenant_id: String,
    pub product_id: String,
    pub plan: String,
    pub quota_runs: u64,
    pub notice_at_runs: u64,
    pub notice_mode: NoticeMode,
    pub recipient_email: Option<String>,
}

impl UpgradePolicy {
    fn validate(&self) -> Result<(), String> {
        validate_opaque_id("tenantId", &self.tenant_id)?;
        validate_opaque_id("productId", &self.product_id)?;
        validate_opaque_id("plan", &self.plan)?;
        if self.quota_runs == 0 || self.quota_runs > 1_000_000_000 {
            return Err("quotaRuns must be in 1..=1000000000".to_string());
        }
        if self.notice_at_runs == 0 || self.notice_at_runs > self.quota_runs {
            return Err("noticeAtRuns must be in 1..=quotaRuns".to_string());
        }
        if self.notice_mode == NoticeMode::Enabled
            && self
                .recipient_email
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            return Err("recipientEmail is required when noticeMode is enabled".to_string());
        }
        Ok(())
    }
}

#[derive(Default, Clone)]
struct PolicySet {
    by_tenant_product: HashMap<String, UpgradePolicy>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDocument {
    policies: Vec<UpgradePolicy>,
}

impl PolicySet {
    fn from_json(source: &str) -> Result<Self, String> {
        let document: PolicyDocument =
            serde_json::from_str(source).map_err(|error| format!("invalid JSON: {error}"))?;
        let mut by_tenant_product = HashMap::new();
        for policy in document.policies {
            policy.validate()?;
            let key = policy_key(&policy.tenant_id, &policy.product_id);
            if by_tenant_product.insert(key, policy).is_some() {
                return Err("duplicate tenantId/productId policy".to_string());
            }
        }
        Ok(Self { by_tenant_product })
    }

    fn get(&self, tenant_id: &str, product_id: &str) -> Option<&UpgradePolicy> {
        self.by_tenant_product
            .get(&policy_key(tenant_id, product_id))
    }

    fn len(&self) -> usize {
        self.by_tenant_product.len()
    }
}

fn policy_key(tenant_id: &str, product_id: &str) -> String {
    format!("{tenant_id}\u{1f}{product_id}")
}

#[derive(Clone, Debug)]
pub struct RunMetering {
    pub context: UsageContext,
    pub run_id: String,
    pub started_at_ms: u64,
    pub steps_requested: u64,
    pub timeout_budget_ms: u64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Accepted,
    Succeeded,
    Failed,
    Completed,
    TimedOut,
    Rejected,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundedUsageV1 {
    steps_requested: u64,
    timeout_budget_ms: u64,
    duration_ms: Option<u64>,
    screenshots_emitted: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunEventV1 {
    schema: &'static str,
    event_id: String,
    event_type: &'static str,
    tenant_id: String,
    actor_id: String,
    automation_id: String,
    product_id: String,
    run_id: String,
    started_at_ms: u64,
    finished_at_ms: Option<u64>,
    outcome: RunOutcome,
    usage: BoundedUsageV1,
}

impl RunEventV1 {
    fn accepted(run: &RunMetering) -> Self {
        Self {
            schema: RUN_EVENT_SCHEMA,
            event_id: format!("{}:accepted", run.run_id),
            event_type: "accepted",
            tenant_id: run.context.tenant_id.clone(),
            actor_id: run.context.actor_id.clone(),
            automation_id: run.context.automation_id.clone(),
            product_id: run.context.product_id.clone(),
            run_id: run.run_id.clone(),
            started_at_ms: run.started_at_ms,
            finished_at_ms: None,
            outcome: RunOutcome::Accepted,
            usage: BoundedUsageV1 {
                steps_requested: run.steps_requested,
                timeout_budget_ms: run.timeout_budget_ms,
                duration_ms: None,
                screenshots_emitted: None,
            },
        }
    }

    fn completed(
        run: &RunMetering,
        finished_at_ms: u64,
        outcome: RunOutcome,
        screenshots_emitted: Option<u64>,
    ) -> Self {
        Self {
            schema: RUN_EVENT_SCHEMA,
            event_id: format!("{}:completed", run.run_id),
            event_type: "completed",
            tenant_id: run.context.tenant_id.clone(),
            actor_id: run.context.actor_id.clone(),
            automation_id: run.context.automation_id.clone(),
            product_id: run.context.product_id.clone(),
            run_id: run.run_id.clone(),
            started_at_ms: run.started_at_ms,
            finished_at_ms: Some(finished_at_ms),
            outcome,
            usage: BoundedUsageV1 {
                steps_requested: run.steps_requested,
                timeout_budget_ms: run.timeout_budget_ms,
                duration_ms: Some(finished_at_ms.saturating_sub(run.started_at_ms)),
                screenshots_emitted: screenshots_emitted.map(|count| count.min(1_000_000)),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PeriodStateV1 {
    schema: String,
    tenant_id: String,
    product_id: String,
    period: u64,
    run_ids: Vec<String>,
    claimed_thresholds: Vec<u64>,
}

impl PeriodStateV1 {
    fn new(context: &UsageContext, period: u64) -> Self {
        Self {
            schema: PERIOD_STATE_SCHEMA.to_string(),
            tenant_id: context.tenant_id.clone(),
            product_id: context.product_id.clone(),
            period,
            run_ids: Vec::new(),
            claimed_thresholds: Vec::new(),
        }
    }

    fn apply_run(
        &mut self,
        run_id: &str,
        claim_threshold: Option<u64>,
        max_run_ids: usize,
    ) -> Result<ApplyResult, String> {
        if self.run_ids.iter().any(|existing| existing == run_id) {
            return Ok(ApplyResult {
                replayed: true,
                total_runs: self.run_ids.len() as u64,
                notice_claimed: false,
            });
        }
        if self.run_ids.len() >= max_run_ids {
            return Err("period run-id capacity reached".to_string());
        }

        self.run_ids.push(run_id.to_string());
        let total_runs = self.run_ids.len() as u64;
        let notice_claimed = claim_threshold
            .filter(|threshold| total_runs >= *threshold)
            .filter(|threshold| !self.claimed_thresholds.contains(threshold))
            .map(|threshold| {
                self.claimed_thresholds.push(threshold);
                true
            })
            .unwrap_or(false);

        Ok(ApplyResult {
            replayed: false,
            total_runs,
            notice_claimed,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ApplyResult {
    replayed: bool,
    total_runs: u64,
    notice_claimed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeDisposition {
    NotDue,
    Suppressed,
    DryRun,
    Published,
    PublishFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcceptanceResult {
    pub replayed: bool,
    pub total_runs: u64,
    pub event_published: bool,
    pub notice: NoticeDisposition,
}

pub async fn initialize_store(
    client: &NatsClient,
    config: &MeteringConfig,
) -> Result<Store, String> {
    let context = jetstream::new(client.clone());
    if let Ok(store) = context.get_key_value(config.kv_bucket.clone()).await {
        return Ok(store);
    }

    context
        .create_key_value(jetstream::kv::Config {
            bucket: config.kv_bucket.clone(),
            description: "Versioned, content-free browser automation usage records".to_string(),
            max_value_size: 1_048_576,
            history: 1,
            max_age: Duration::from_secs(config.period_seconds.saturating_mul(3)),
            max_bytes: 268_435_456,
            storage: jetstream::stream::StorageType::File,
            ..Default::default()
        })
        .await
        .map_err(|error| format!("JetStream KV unavailable: {error}"))
}

pub async fn record_acceptance(
    store: &Store,
    client: &NatsClient,
    config: &MeteringConfig,
    run: &RunMetering,
) -> Result<AcceptanceResult, String> {
    run.context.validate()?;
    validate_opaque_id("runId", &run.run_id)?;

    let event = RunEventV1::accepted(run);
    let event_created =
        create_event(store, &event, &format!("run.v1.{}.accepted", run.run_id)).await?;
    let policy = config.policy_for(&run.context);
    let claim_threshold = policy.and_then(|policy| {
        (policy.notice_mode != NoticeMode::Suppressed).then_some(policy.notice_at_runs)
    });
    let period = run.started_at_ms / 1_000 / config.period_seconds;
    let state_key = format!(
        "period.v1.{}.{}.{}",
        run.context.tenant_id, run.context.product_id, period
    );
    let applied = apply_period_state(
        store,
        &state_key,
        &run.context,
        period,
        &run.run_id,
        claim_threshold,
        config.max_run_ids_per_period,
    )
    .await?;

    let event_published = if event_created {
        publish_event(client, &config.usage_subject, &event).await
    } else {
        false
    };

    let notice = if applied.notice_claimed {
        match policy.map(|policy| policy.notice_mode) {
            Some(NoticeMode::DryRun) => NoticeDisposition::DryRun,
            Some(NoticeMode::Enabled) => {
                if let Some(policy) = policy {
                    if publish_upgrade_notice(client, config, policy, period, applied.total_runs)
                        .await
                    {
                        NoticeDisposition::Published
                    } else {
                        NoticeDisposition::PublishFailed
                    }
                } else {
                    NoticeDisposition::PublishFailed
                }
            }
            _ => NoticeDisposition::Suppressed,
        }
    } else if policy
        .map(|policy| {
            policy.notice_mode == NoticeMode::Suppressed
                && applied.total_runs >= policy.notice_at_runs
        })
        .unwrap_or(false)
    {
        NoticeDisposition::Suppressed
    } else {
        NoticeDisposition::NotDue
    };

    Ok(AcceptanceResult {
        replayed: applied.replayed,
        total_runs: applied.total_runs,
        event_published,
        notice,
    })
}

pub async fn record_completion(
    store: &Store,
    client: &NatsClient,
    config: &MeteringConfig,
    run: &RunMetering,
    finished_at_ms: u64,
    outcome: RunOutcome,
    screenshots_emitted: Option<u64>,
) -> Result<bool, String> {
    let event = RunEventV1::completed(run, finished_at_ms, outcome, screenshots_emitted);
    let created = create_event(store, &event, &format!("run.v1.{}.completed", run.run_id)).await?;
    if created {
        Ok(publish_event(client, &config.usage_subject, &event).await)
    } else {
        Ok(false)
    }
}

async fn create_event(store: &Store, event: &RunEventV1, key: &str) -> Result<bool, String> {
    let bytes = serde_json::to_vec(event)
        .map_err(|error| format!("event serialization failed: {error}"))?;
    match store.create(key, bytes.into()).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == CreateErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(format!("event storage failed: {error}")),
    }
}

async fn apply_period_state(
    store: &Store,
    key: &str,
    context: &UsageContext,
    period: u64,
    run_id: &str,
    claim_threshold: Option<u64>,
    max_run_ids: usize,
) -> Result<ApplyResult, String> {
    for _ in 0..32 {
        let entry = store
            .entry(key)
            .await
            .map_err(|error| format!("period state read failed: {error}"))?;
        match entry {
            Some(entry) => {
                let mut state: PeriodStateV1 = serde_json::from_slice(&entry.value)
                    .map_err(|error| format!("period state schema mismatch: {error}"))?;
                if state.schema != PERIOD_STATE_SCHEMA
                    || state.tenant_id != context.tenant_id
                    || state.product_id != context.product_id
                    || state.period != period
                {
                    return Err("period state identity mismatch".to_string());
                }
                let result = state.apply_run(run_id, claim_threshold, max_run_ids)?;
                if result.replayed {
                    return Ok(result);
                }
                let bytes = serde_json::to_vec(&state)
                    .map_err(|error| format!("period state serialization failed: {error}"))?;
                match store.update(key, bytes.into(), entry.revision).await {
                    Ok(_) => return Ok(result),
                    Err(error) if error.kind() == UpdateErrorKind::WrongLastRevision => continue,
                    Err(error) => return Err(format!("period state update failed: {error}")),
                }
            }
            None => {
                let mut state = PeriodStateV1::new(context, period);
                let result = state.apply_run(run_id, claim_threshold, max_run_ids)?;
                let bytes = serde_json::to_vec(&state)
                    .map_err(|error| format!("period state serialization failed: {error}"))?;
                match store.create(key, bytes.into()).await {
                    Ok(_) => return Ok(result),
                    Err(error) if error.kind() == CreateErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(format!("period state create failed: {error}")),
                }
            }
        }
    }
    Err("period state contention exceeded retry budget".to_string())
}

async fn publish_event(client: &NatsClient, subject: &str, event: &RunEventV1) -> bool {
    let Ok(bytes) = serde_json::to_vec(event) else {
        return false;
    };
    client
        .publish(subject.to_string(), bytes.into())
        .await
        .is_ok()
}

async fn publish_upgrade_notice(
    client: &NatsClient,
    config: &MeteringConfig,
    policy: &UpgradePolicy,
    period: u64,
    total_runs: u64,
) -> bool {
    let Some(payload) = upgrade_notice_payload(config, policy, period, total_runs) else {
        return false;
    };
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        return false;
    };
    if client
        .publish(config.contact_email_subject.clone(), bytes.into())
        .await
        .is_err()
    {
        return false;
    }
    client.flush().await.is_ok()
}

fn upgrade_notice_payload(
    config: &MeteringConfig,
    policy: &UpgradePolicy,
    period: u64,
    total_runs: u64,
) -> Option<serde_json::Value> {
    let recipient = policy.recipient_email.as_deref()?;
    let notice_id = notice_id(
        &policy.tenant_id,
        &policy.product_id,
        period,
        policy.notice_at_runs,
    );
    let subject = "Automation usage notice";
    let text = format!(
        "Your organization has used {total_runs} of {} configured automation runs in the current billing period. Review your plan or upgrade options before the quota is reached. This notice contains no browser content.",
        policy.quota_runs
    );
    let html = format!(
        "<p>Your organization has used <strong>{total_runs}</strong> of <strong>{}</strong> configured automation runs in the current billing period.</p><p>Review your plan or upgrade options before the quota is reached.</p><p>This notice contains no browser content.</p>",
        policy.quota_runs
    );
    Some(json!({
        "to": recipient,
        "subject": subject,
        "html": html,
        "text": text,
        "auth": config.contact_nats_secret,
        "idempotency_key": notice_id,
    }))
}

fn notice_id(tenant_id: &str, product_id: &str, period: u64, threshold: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(b"dd-browser-upgrade-notice-v1\0");
    digest.update(tenant_id.as_bytes());
    digest.update(b"\0");
    digest.update(product_id.as_bytes());
    digest.update(b"\0");
    digest.update(period.to_be_bytes());
    digest.update(threshold.to_be_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    format!("upgrade_{}", &hex[..48])
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn usage_context() -> UsageContext {
        UsageContext {
            tenant_id: "tenant_01".to_string(),
            actor_id: "actor_01".to_string(),
            automation_id: "automation_01".to_string(),
            product_id: "browser_runner".to_string(),
        }
    }

    fn run() -> RunMetering {
        RunMetering {
            context: usage_context(),
            run_id: "run_01".to_string(),
            started_at_ms: 1_700_000_000_000,
            steps_requested: 3,
            timeout_budget_ms: 60_000,
        }
    }

    #[test]
    fn opaque_ids_reject_email_urls_and_control_characters() {
        let mut context = usage_context();
        assert!(context.validate().is_ok());
        for invalid in ["person@example.com", "https://example.com", "actor\nadmin"] {
            context.actor_id = invalid.to_string();
            assert!(context.validate().is_err());
        }
    }

    #[test]
    fn policy_document_supports_free_paid_suppressed_and_dry_run_plans() {
        let source = r#"{
            "policies": [
                {"tenantId":"a","productId":"browser","plan":"free","quotaRuns":100,"noticeAtRuns":80,"noticeMode":"enabled","recipientEmail":"ops@example.com"},
                {"tenantId":"b","productId":"browser","plan":"paid","quotaRuns":10000,"noticeAtRuns":9000,"noticeMode":"dryRun","recipientEmail":null},
                {"tenantId":"c","productId":"browser","plan":"custom","quotaRuns":500,"noticeAtRuns":400,"noticeMode":"suppressed","recipientEmail":null}
            ]
        }"#;
        let policies = PolicySet::from_json(source).expect("valid policies");
        assert_eq!(policies.len(), 3);
        assert_eq!(policies.get("a", "browser").unwrap().plan, "free");
        assert_eq!(
            policies.get("b", "browser").unwrap().notice_mode,
            NoticeMode::DryRun
        );
        assert_eq!(
            policies.get("c", "browser").unwrap().notice_mode,
            NoticeMode::Suppressed
        );
    }

    #[test]
    fn policy_document_rejects_duplicate_or_unsafe_policies() {
        let duplicate = r#"{"policies":[
            {"tenantId":"a","productId":"browser","plan":"free","quotaRuns":10,"noticeAtRuns":8,"noticeMode":"dryRun","recipientEmail":null},
            {"tenantId":"a","productId":"browser","plan":"paid","quotaRuns":20,"noticeAtRuns":18,"noticeMode":"suppressed","recipientEmail":null}
        ]}"#;
        assert!(PolicySet::from_json(duplicate).is_err());
        let unsafe_id = r#"{"policies":[
            {"tenantId":"person@example.com","productId":"browser","plan":"free","quotaRuns":10,"noticeAtRuns":8,"noticeMode":"dryRun","recipientEmail":null}
        ]}"#;
        assert!(PolicySet::from_json(unsafe_id).is_err());
    }

    #[test]
    fn period_state_is_replay_safe_and_claims_threshold_once() {
        let context = usage_context();
        let mut state = PeriodStateV1::new(&context, 42);
        assert_eq!(
            state.apply_run("run_1", Some(2), 10).unwrap(),
            ApplyResult {
                replayed: false,
                total_runs: 1,
                notice_claimed: false,
            }
        );
        assert_eq!(
            state.apply_run("run_2", Some(2), 10).unwrap(),
            ApplyResult {
                replayed: false,
                total_runs: 2,
                notice_claimed: true,
            }
        );
        assert_eq!(
            state.apply_run("run_2", Some(2), 10).unwrap(),
            ApplyResult {
                replayed: true,
                total_runs: 2,
                notice_claimed: false,
            }
        );
        assert!(
            !state
                .apply_run("run_3", Some(2), 10)
                .unwrap()
                .notice_claimed
        );
    }

    #[test]
    fn period_state_storage_is_bounded() {
        let context = usage_context();
        let mut state = PeriodStateV1::new(&context, 42);
        state.apply_run("run_1", None, 1).unwrap();
        assert!(state.apply_run("run_2", None, 1).is_err());
    }

    #[test]
    fn suppressed_threshold_does_not_claim_and_can_be_enabled_later() {
        let context = usage_context();
        let mut state = PeriodStateV1::new(&context, 42);
        let suppressed = state.apply_run("run_1", None, 10).unwrap();
        assert!(!suppressed.notice_claimed);
        assert!(state.claimed_thresholds.is_empty());

        let enabled = state.apply_run("run_2", Some(1), 10).unwrap();
        assert!(enabled.notice_claimed);
        assert_eq!(state.claimed_thresholds, vec![1]);
    }

    #[test]
    fn run_events_are_versioned_and_content_free() {
        let event =
            RunEventV1::completed(&run(), 1_700_000_001_500, RunOutcome::Succeeded, Some(2));
        let value = serde_json::to_value(event).expect("serializable");
        assert_eq!(value["schema"], RUN_EVENT_SCHEMA);
        assert_eq!(value["tenantId"], "tenant_01");
        assert_eq!(value["actorId"], "actor_01");
        assert_eq!(value["automationId"], "automation_01");
        assert_eq!(value["outcome"], "succeeded");
        assert_eq!(value["usage"]["durationMs"], 1_500);
        let serialized = value.to_string().to_ascii_lowercase();
        for forbidden in [
            "url",
            "cookie",
            "credential",
            "password",
            "header",
            "scraped",
            "extracted",
            "screenshotdata",
            "recipient",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "found forbidden field {forbidden}"
            );
        }
    }

    #[test]
    fn notice_ids_are_deterministic_scoped_and_mail_safe() {
        let first = notice_id("tenant_a", "browser", 42, 80);
        assert_eq!(first, notice_id("tenant_a", "browser", 42, 80));
        assert_ne!(first, notice_id("tenant_a", "browser", 43, 80));
        assert_ne!(first, notice_id("tenant_a", "browser", 42, 90));
        assert!((16..=128).contains(&first.len()));
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')));
    }

    #[test]
    fn upgrade_mail_uses_the_protected_generic_payload_only() {
        let config = MeteringConfig {
            contact_nats_secret: Some("mail_lane_secret".to_string()),
            ..MeteringConfig::disabled()
        };
        let policy = UpgradePolicy {
            tenant_id: "tenant_01".to_string(),
            product_id: "browser_runner".to_string(),
            plan: "free".to_string(),
            quota_runs: 100,
            notice_at_runs: 80,
            notice_mode: NoticeMode::Enabled,
            recipient_email: Some("owner@example.com".to_string()),
        };
        let payload = upgrade_notice_payload(&config, &policy, 42, 80).unwrap();
        assert_eq!(payload["to"], "owner@example.com");
        assert_eq!(payload["auth"], "mail_lane_secret");
        assert_eq!(payload["subject"], "Automation usage notice");
        assert_eq!(payload["idempotency_key"].as_str().unwrap().len(), 56);

        let body = format!("{} {}", payload["text"], payload["html"]);
        assert!(!body.contains("tenant_01"));
        assert!(!body.contains("browser_runner"));
        assert!(!body.contains("free"));
        assert!(!body.contains("http"));
        assert!(!body.contains("cookie"));
    }

    #[tokio::test]
    #[ignore = "requires BROWSER_JOB_METERING_E2E_NATS_URL pointing to a JetStream-enabled NATS server"]
    async fn jetstream_e2e_is_replay_safe_and_publishes_one_notice() {
        let nats_url = env::var("BROWSER_JOB_METERING_E2E_NATS_URL")
            .expect("set BROWSER_JOB_METERING_E2E_NATS_URL for this ignored test");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let bucket = format!("DD_BROWSER_USAGE_TEST_{unique}");
        let usage_subject = format!("dd.test.browser_usage.{unique}");
        let mail_subject = format!("dd.test.browser_mail.{unique}");

        let client = async_nats::connect(nats_url)
            .await
            .expect("NATS connection");
        let mut usage_messages = client
            .subscribe(usage_subject.clone())
            .await
            .expect("usage subscription");
        let mut mail_messages = client
            .subscribe(mail_subject.clone())
            .await
            .expect("mail subscription");

        let mut config = MeteringConfig::disabled();
        config.enabled = true;
        config.kv_bucket = bucket.clone();
        config.usage_subject = usage_subject;
        config.contact_email_subject = mail_subject;
        config.period_seconds = 3_600;
        config.policies = PolicySet::from_json(
            r#"{"policies":[{"tenantId":"tenant_01","productId":"browser_runner","plan":"free","quotaRuns":10,"noticeAtRuns":1,"noticeMode":"enabled","recipientEmail":"owner@example.com"}]}"#,
        )
        .expect("test policy");

        let store = initialize_store(&client, &config)
            .await
            .expect("test KV bucket");
        let first = record_acceptance(&store, &client, &config, &run())
            .await
            .expect("first metering write");
        assert!(!first.replayed);
        assert_eq!(first.total_runs, 1);
        assert_eq!(first.notice, NoticeDisposition::Published);

        let usage = tokio::time::timeout(Duration::from_secs(2), usage_messages.next())
            .await
            .expect("usage event timeout")
            .expect("usage event");
        let usage_json: serde_json::Value =
            serde_json::from_slice(&usage.payload).expect("usage JSON");
        assert_eq!(usage_json["schema"], RUN_EVENT_SCHEMA);
        assert!(usage_json.get("url").is_none());
        assert!(usage_json.get("recipientEmail").is_none());

        let mail = tokio::time::timeout(Duration::from_secs(2), mail_messages.next())
            .await
            .expect("mail event timeout")
            .expect("mail event");
        let mail_json: serde_json::Value =
            serde_json::from_slice(&mail.payload).expect("mail JSON");
        assert_eq!(mail_json["to"], "owner@example.com");

        let replay = record_acceptance(&store, &client, &config, &run())
            .await
            .expect("replayed metering write");
        assert!(replay.replayed);
        assert_eq!(replay.total_runs, 1);
        assert_eq!(replay.notice, NoticeDisposition::NotDue);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), mail_messages.next())
                .await
                .is_err()
        );

        assert!(record_completion(
            &store,
            &client,
            &config,
            &run(),
            1_700_000_001_000,
            RunOutcome::Succeeded,
            Some(1),
        )
        .await
        .expect("completion write"));
        assert!(!record_completion(
            &store,
            &client,
            &config,
            &run(),
            1_700_000_001_000,
            RunOutcome::Succeeded,
            Some(1),
        )
        .await
        .expect("duplicate completion write"));

        jetstream::new(client)
            .delete_key_value(bucket)
            .await
            .expect("test bucket cleanup");
    }
}
