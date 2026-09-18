use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    alerts,
    notifications::{ContactPoint, ContactPointKind, NotificationPolicy},
};

const MAX_DISPATCH_RECORDS: usize = 512;
const MAX_DISPATCH_REASON_BYTES: usize = 512;
const MAX_ATTEMPTS_PER_DISPATCH: usize = 64;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DispatchAlertRequest {
    pub reason: Option<String>,
    pub force: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DispatchStatus {
    Queued,
    Skipped,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DeliveryAttemptStatus {
    Queued,
    Skipped,
    Blocked,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationDispatchRecord {
    pub dispatch_id: String,
    pub rule_id: String,
    pub title: String,
    pub status: DispatchStatus,
    pub evaluation_state: String,
    pub forced: bool,
    pub reason: Option<String>,
    pub delivery_mode: &'static str,
    pub matched_policy_count: usize,
    pub attempt_count: usize,
    pub queued_attempt_count: usize,
    pub labels: BTreeMap<String, String>,
    pub annotation_keys: Vec<String>,
    pub attempts: Vec<NotificationDeliveryAttempt>,
    pub warnings: Vec<String>,
    pub created_at_ms: u128,
    pub processed_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationDispatchSummary {
    dispatch_id: String,
    rule_id: String,
    status: DispatchStatus,
    evaluation_state: String,
    forced: bool,
    matched_policy_count: usize,
    attempt_count: usize,
    queued_attempt_count: usize,
    processed_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationDeliveryAttempt {
    attempt_id: String,
    contact_id: String,
    contact_name: String,
    kind: ContactPointKind,
    policy_ids: Vec<String>,
    status: DeliveryAttemptStatus,
    secret_ref_configured: bool,
    worker_action: &'static str,
    created_at_ms: u128,
}

impl NotificationDispatchRecord {
    pub(crate) fn summary(&self) -> NotificationDispatchSummary {
        NotificationDispatchSummary {
            dispatch_id: self.dispatch_id.clone(),
            rule_id: self.rule_id.clone(),
            status: self.status.clone(),
            evaluation_state: self.evaluation_state.clone(),
            forced: self.forced,
            matched_policy_count: self.matched_policy_count,
            attempt_count: self.attempt_count,
            queued_attempt_count: self.queued_attempt_count,
            processed_at_ms: self.processed_at_ms,
        }
    }
}

pub(crate) fn dispatch(
    request: DispatchAlertRequest,
    rule: alerts::AlertRule,
    evaluation: Value,
    policies: &[NotificationPolicy],
    contacts: &BTreeMap<String, ContactPoint>,
    now_ms: u128,
) -> Result<NotificationDispatchRecord, String> {
    let reason = normalize_reason(request.reason)?;
    let forced = request.force.unwrap_or(false);
    let evaluation_state = evaluation
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let should_dispatch = forced || evaluation_state == "alerting";
    let mut warnings = Vec::new();
    if !should_dispatch {
        warnings.push(format!(
            "alert evaluation state `{evaluation_state}` did not trigger notification dispatch"
        ));
    }

    let routing = route_contacts(&rule.labels, policies, contacts, &mut warnings);
    let mut attempts = Vec::new();
    if should_dispatch {
        for routed in routing.contacts {
            if attempts.len() >= MAX_ATTEMPTS_PER_DISPATCH {
                warnings.push(format!(
                    "delivery attempts truncated to max {MAX_ATTEMPTS_PER_DISPATCH}"
                ));
                break;
            }
            attempts.push(attempt_from_contact(
                &rule.rule_id,
                routed,
                contacts,
                now_ms,
                &mut warnings,
            ));
        }
    }
    let queued_attempt_count = attempts
        .iter()
        .filter(|attempt| attempt.status == DeliveryAttemptStatus::Queued)
        .count();
    let status = if !should_dispatch {
        DispatchStatus::Skipped
    } else if queued_attempt_count > 0 {
        DispatchStatus::Queued
    } else if attempts.is_empty() {
        DispatchStatus::Skipped
    } else {
        DispatchStatus::Blocked
    };
    if should_dispatch && routing.matched_policy_count == 0 {
        warnings.push("no notification policies matched alert labels".to_string());
    }

    Ok(NotificationDispatchRecord {
        dispatch_id: format!("{}-{}", rule.rule_id, now_ms),
        rule_id: rule.rule_id,
        title: rule.title,
        status,
        evaluation_state,
        forced,
        reason,
        delivery_mode: "bounded-in-memory-outbox",
        matched_policy_count: routing.matched_policy_count,
        attempt_count: attempts.len(),
        queued_attempt_count,
        labels: rule.labels,
        annotation_keys: rule.annotations.keys().cloned().collect(),
        attempts,
        warnings,
        created_at_ms: now_ms,
        processed_at_ms: now_ms,
    })
}

pub(crate) fn store_record(
    records: &mut BTreeMap<String, NotificationDispatchRecord>,
    record: NotificationDispatchRecord,
) {
    while records.len() >= MAX_DISPATCH_RECORDS {
        let Some(oldest_key) = records
            .iter()
            .min_by_key(|(_, record)| record.created_at_ms)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        records.remove(&oldest_key);
    }
    records.insert(record.dispatch_id.clone(), record);
}

pub(crate) fn catalog_payload(summaries: Vec<NotificationDispatchSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.notification-dispatches.v1",
        "dispatches": summaries,
        "limits": limits_payload()
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxDispatchRecords": MAX_DISPATCH_RECORDS,
        "maxDispatchReasonBytes": MAX_DISPATCH_REASON_BYTES,
        "maxAttemptsPerDispatch": MAX_ATTEMPTS_PER_DISPATCH,
        "deliveryMode": "bounded-in-memory-outbox"
    })
}

struct RoutingPlan {
    matched_policy_count: usize,
    contacts: Vec<RoutedContact>,
}

struct RoutedContact {
    contact_id: String,
    policy_ids: Vec<String>,
}

fn route_contacts(
    labels: &BTreeMap<String, String>,
    policies: &[NotificationPolicy],
    contacts: &BTreeMap<String, ContactPoint>,
    warnings: &mut Vec<String>,
) -> RoutingPlan {
    let mut ordered_policies = policies.to_vec();
    ordered_policies.sort_by(|left, right| {
        right
            .match_labels
            .len()
            .cmp(&left.match_labels.len())
            .then_with(|| left.policy_id.cmp(&right.policy_id))
    });

    let mut matched_policy_count = 0usize;
    let mut routed_contacts = BTreeMap::<String, BTreeSet<String>>::new();
    for policy in ordered_policies {
        if !policy_matches(&policy, labels) {
            continue;
        }
        matched_policy_count += 1;
        if policy.disabled {
            warnings.push(format!(
                "notification policy `{}` is disabled",
                policy.policy_id
            ));
        } else {
            for contact_id in &policy.contact_ids {
                if !contacts.contains_key(contact_id) {
                    warnings.push(format!(
                        "notification policy `{}` references missing contact `{contact_id}`",
                        policy.policy_id
                    ));
                    continue;
                }
                routed_contacts
                    .entry(contact_id.clone())
                    .or_default()
                    .insert(policy.policy_id.clone());
            }
        }
        if !policy.continue_matching {
            break;
        }
    }

    RoutingPlan {
        matched_policy_count,
        contacts: routed_contacts
            .into_iter()
            .map(|(contact_id, policy_ids)| RoutedContact {
                contact_id,
                policy_ids: policy_ids.into_iter().collect(),
            })
            .collect(),
    }
}

fn attempt_from_contact(
    rule_id: &str,
    routed: RoutedContact,
    contacts: &BTreeMap<String, ContactPoint>,
    now_ms: u128,
    warnings: &mut Vec<String>,
) -> NotificationDeliveryAttempt {
    let Some(contact) = contacts.get(&routed.contact_id) else {
        return NotificationDeliveryAttempt {
            attempt_id: format!("{rule_id}-missing-{}", routed.contact_id),
            contact_id: routed.contact_id,
            contact_name: "missing contact".to_string(),
            kind: ContactPointKind::Email,
            policy_ids: routed.policy_ids,
            status: DeliveryAttemptStatus::Blocked,
            secret_ref_configured: false,
            worker_action: "blocked-missing-contact",
            created_at_ms: now_ms,
        };
    };
    let (status, worker_action) = if contact.disabled {
        warnings.push(format!(
            "contact point `{}` is disabled",
            contact.contact_id
        ));
        (DeliveryAttemptStatus::Skipped, "skipped-disabled-contact")
    } else if requires_secret_ref(contact.kind) && contact.secret_ref.is_none() {
        warnings.push(format!(
            "contact point `{}` is missing secretRef",
            contact.contact_id
        ));
        (DeliveryAttemptStatus::Blocked, "blocked-missing-secret-ref")
    } else {
        (
            DeliveryAttemptStatus::Queued,
            "queued-for-secret-ref-worker",
        )
    };

    NotificationDeliveryAttempt {
        attempt_id: format!("{rule_id}-{}-{now_ms}", contact.contact_id),
        contact_id: contact.contact_id.clone(),
        contact_name: contact.name.clone(),
        kind: contact.kind,
        policy_ids: routed.policy_ids,
        status,
        secret_ref_configured: contact.secret_ref.is_some(),
        worker_action,
        created_at_ms: now_ms,
    }
}

fn policy_matches(policy: &NotificationPolicy, labels: &BTreeMap<String, String>) -> bool {
    policy
        .match_labels
        .iter()
        .all(|(key, value)| labels.get(key).map(String::as_str) == Some(value.as_str()))
}

fn requires_secret_ref(kind: ContactPointKind) -> bool {
    !matches!(kind, ContactPointKind::Email)
}

fn normalize_reason(reason: Option<String>) -> Result<Option<String>, String> {
    let Some(reason) = reason
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty())
    else {
        return Ok(None);
    };
    if reason.len() > MAX_DISPATCH_REASON_BYTES {
        return Err(format!(
            "dispatch reason exceeds max {MAX_DISPATCH_REASON_BYTES} bytes"
        ));
    }
    if looks_secret_bearing(&reason) {
        return Err("dispatch reason contains secret-looking text".to_string());
    }
    Ok(Some(reason))
}

fn looks_secret_bearing(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "authorization",
        "bearer",
        "api_key",
        "private_key",
        "access_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        alerts::{AlertCondition, AlertOperator, AlertReducer, AlertState, SaveAlertRuleRequest},
        notifications::{ContactPointKind, SaveContactPointRequest, SaveNotificationPolicyRequest},
        QueryDialect, QueryRequest,
    };

    fn rule() -> alerts::AlertRule {
        SaveAlertRuleRequest {
            rule_id: "high-cpu".to_string(),
            title: "High CPU".to_string(),
            query: QueryRequest {
                dialect: QueryDialect::Sql,
                query: "SELECT AVG(cpu) AS avgCpu FROM metrics".to_string(),
                dataset_id: Some("metrics".to_string()),
                limit: Some(10),
            },
            condition: AlertCondition {
                field: "avgCpu".to_string(),
                reducer: AlertReducer::Max,
                op: AlertOperator::Gt,
                threshold: 0.8,
            },
            for_seconds: None,
            labels: Some(BTreeMap::from([
                ("severity".to_string(), "critical".to_string()),
                ("service".to_string(), "api".to_string()),
            ])),
            annotations: Some(BTreeMap::from([(
                "summary".to_string(),
                "CPU is high".to_string(),
            )])),
            dashboard_id: None,
            panel_id: None,
            enabled: Some(true),
        }
        .into_rule(100)
        .expect("rule validates")
    }

    fn contact() -> ContactPoint {
        SaveContactPointRequest {
            contact_id: "prod-slack".to_string(),
            name: "Prod Slack".to_string(),
            kind: ContactPointKind::Slack,
            owner: Some("sre".to_string()),
            tags: Some(vec!["prod".to_string()]),
            secret_ref: Some("grafana.slack.prod".to_string()),
            settings: Some(BTreeMap::from([(
                "channel".to_string(),
                "#alerts".to_string(),
            )])),
            disabled: None,
        }
        .into_contact_point(100)
        .expect("contact validates")
    }

    fn policy() -> NotificationPolicy {
        SaveNotificationPolicyRequest {
            policy_id: "prod-critical".to_string(),
            name: "Prod critical".to_string(),
            match_labels: Some(BTreeMap::from([(
                "severity".to_string(),
                "critical".to_string(),
            )])),
            contact_ids: vec!["prod-slack".to_string()],
            group_by: Some(vec!["service".to_string()]),
            continue_matching: None,
            disabled: None,
        }
        .into_policy(100, &BTreeSet::from(["prod-slack".to_string()]))
        .expect("policy validates")
    }

    #[test]
    fn dispatch_queues_secret_ref_attempt_for_alerting_rule() {
        let contacts = BTreeMap::from([("prod-slack".to_string(), contact())]);
        let record = dispatch(
            DispatchAlertRequest {
                reason: Some("operator-confirmed alert".to_string()),
                force: None,
            },
            rule(),
            json!({ "state": "alerting" }),
            &[policy()],
            &contacts,
            200,
        )
        .expect("dispatch builds");

        assert_eq!(record.status, DispatchStatus::Queued);
        assert_eq!(record.matched_policy_count, 1);
        assert_eq!(record.queued_attempt_count, 1);
        assert_eq!(
            record.attempts[0].worker_action,
            "queued-for-secret-ref-worker"
        );
        assert_eq!(record.annotation_keys, vec!["summary"]);
    }

    #[test]
    fn dispatch_skips_non_alerting_rule_without_force() {
        let contacts = BTreeMap::from([("prod-slack".to_string(), contact())]);
        let record = dispatch(
            DispatchAlertRequest {
                reason: None,
                force: None,
            },
            rule(),
            json!({ "state": "normal" }),
            &[policy()],
            &contacts,
            200,
        )
        .expect("dispatch builds");

        assert_eq!(record.status, DispatchStatus::Skipped);
        assert_eq!(record.attempt_count, 0);
        assert!(record.warnings[0].contains("did not trigger"));
    }

    #[test]
    fn dispatch_reason_rejects_secret_like_text() {
        let error = dispatch(
            DispatchAlertRequest {
                reason: Some("token=abc".to_string()),
                force: Some(true),
            },
            rule(),
            json!({ "state": AlertState::Alerting }),
            &[],
            &BTreeMap::new(),
            200,
        )
        .expect_err("secret reason rejected");

        assert!(error.contains("secret-looking"));
    }

    #[test]
    fn dispatch_store_prunes_oldest_records() {
        let mut records = BTreeMap::new();
        for index in 0..=MAX_DISPATCH_RECORDS {
            let mut record = dispatch(
                DispatchAlertRequest {
                    reason: None,
                    force: None,
                },
                rule(),
                json!({ "state": "normal" }),
                &[],
                &BTreeMap::new(),
                1_000 + index as u128,
            )
            .expect("dispatch builds");
            record.dispatch_id = format!("dispatch-{index}");
            store_record(&mut records, record);
        }

        assert_eq!(records.len(), MAX_DISPATCH_RECORDS);
        assert!(!records.contains_key("dispatch-0"));
    }
}
