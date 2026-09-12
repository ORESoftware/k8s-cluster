use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_identifier, now_ms};

const MAX_CONTACT_POINTS: usize = 128;
const MAX_NOTIFICATION_POLICIES: usize = 256;
const MAX_MATCHERS: usize = 32;
const MAX_CONTACT_IDS: usize = 16;
const MAX_TAGS: usize = 24;
const MAX_SETTINGS: usize = 32;
const MAX_SETTING_VALUE_BYTES: usize = 256;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveContactPointRequest {
    pub contact_id: String,
    pub name: String,
    pub kind: ContactPointKind,
    pub owner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub secret_ref: Option<String>,
    pub settings: Option<BTreeMap<String, String>>,
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContactPointKind {
    Email,
    Slack,
    Webhook,
    PagerDuty,
    Teams,
    Opsgenie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContactPoint {
    pub contact_id: String,
    pub name: String,
    pub kind: ContactPointKind,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub secret_ref: Option<String>,
    pub settings: BTreeMap<String, String>,
    pub disabled: bool,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContactPointSummary {
    contact_id: String,
    name: String,
    kind: ContactPointKind,
    owner: Option<String>,
    tag_count: usize,
    secret_ref_configured: bool,
    disabled: bool,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveContactPointResponse {
    ok: bool,
    contact_point: ContactPoint,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveNotificationPolicyRequest {
    pub policy_id: String,
    pub name: String,
    pub match_labels: Option<BTreeMap<String, String>>,
    pub contact_ids: Vec<String>,
    pub group_by: Option<Vec<String>>,
    pub continue_matching: Option<bool>,
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationPolicy {
    pub policy_id: String,
    pub name: String,
    pub match_labels: BTreeMap<String, String>,
    pub contact_ids: Vec<String>,
    pub group_by: Vec<String>,
    pub continue_matching: bool,
    pub disabled: bool,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationPolicySummary {
    policy_id: String,
    name: String,
    matcher_count: usize,
    contact_count: usize,
    disabled: bool,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveNotificationPolicyResponse {
    ok: bool,
    policy: NotificationPolicy,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct NotificationPreviewInput {
    pub rule_id: String,
    pub title: String,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationPreviewResponse {
    ok: bool,
    schema_version: &'static str,
    previewed_at_ms: u128,
    rule_id: String,
    title: String,
    matched_policy_count: usize,
    delivery_count: usize,
    matched_policies: Vec<MatchedNotificationPolicy>,
    deliveries: Vec<NotificationDeliveryPlan>,
    labels: BTreeMap<String, String>,
    annotation_keys: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MatchedNotificationPolicy {
    policy_id: String,
    name: String,
    match_labels: BTreeMap<String, String>,
    contact_ids: Vec<String>,
    disabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationDeliveryPlan {
    contact_id: String,
    name: String,
    kind: ContactPointKind,
    secret_ref_configured: bool,
    disabled: bool,
    delivery_mode: &'static str,
}

impl SaveContactPointRequest {
    pub(crate) fn into_contact_point(self, now_ms: u128) -> Result<ContactPoint, String> {
        let contact_id = clean_identifier(&self.contact_id).ok_or_else(|| {
            "contactId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let name = bounded_label("contact point name", &self.name, 160)?;
        let owner = self
            .owner
            .as_deref()
            .map(|owner| bounded_label("contact point owner", owner, 120))
            .transpose()?;
        let tags = normalize_tags(self.tags.unwrap_or_default())?;
        let secret_ref = self
            .secret_ref
            .as_deref()
            .map(|value| {
                clean_identifier(value)
                    .ok_or_else(|| "secretRef must be a safe identifier".to_string())
            })
            .transpose()?;
        if self.kind.requires_secret_ref() && secret_ref.is_none() {
            return Err(format!(
                "{} contact points require secretRef instead of raw credentials",
                self.kind.label()
            ));
        }
        let settings = normalize_settings(self.settings.unwrap_or_default())?;
        Ok(ContactPoint {
            contact_id,
            name,
            kind: self.kind,
            owner,
            tags,
            secret_ref,
            settings,
            disabled: self.disabled.unwrap_or(false),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl ContactPoint {
    pub(crate) fn summary(&self) -> ContactPointSummary {
        ContactPointSummary {
            contact_id: self.contact_id.clone(),
            name: self.name.clone(),
            kind: self.kind,
            owner: self.owner.clone(),
            tag_count: self.tags.len(),
            secret_ref_configured: self.secret_ref.is_some(),
            disabled: self.disabled,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

impl SaveNotificationPolicyRequest {
    pub(crate) fn into_policy(
        self,
        now_ms: u128,
        contact_ids: &BTreeSet<String>,
    ) -> Result<NotificationPolicy, String> {
        let policy_id = clean_identifier(&self.policy_id).ok_or_else(|| {
            "policyId must contain letters, numbers, dash, underscore, dot, or colon".to_string()
        })?;
        let name = bounded_label("notification policy name", &self.name, 160)?;
        let match_labels = normalize_matchers(self.match_labels.unwrap_or_default())?;
        if self.contact_ids.is_empty() {
            return Err("notification policy requires at least one contactId".to_string());
        }
        if self.contact_ids.len() > MAX_CONTACT_IDS {
            return Err(format!(
                "notification policy contactIds exceeds max {MAX_CONTACT_IDS}"
            ));
        }
        let mut clean_contacts = Vec::new();
        let mut seen_contacts = BTreeSet::new();
        for contact_id in self.contact_ids {
            let contact_id = clean_identifier(&contact_id)
                .ok_or_else(|| "notification policy contactId is invalid".to_string())?;
            if !contact_ids.contains(&contact_id) {
                return Err(format!("contact point `{contact_id}` not found"));
            }
            if seen_contacts.insert(contact_id.clone()) {
                clean_contacts.push(contact_id);
            }
        }
        let group_by = normalize_group_by(self.group_by.unwrap_or_default())?;
        Ok(NotificationPolicy {
            policy_id,
            name,
            match_labels,
            contact_ids: clean_contacts,
            group_by,
            continue_matching: self.continue_matching.unwrap_or(false),
            disabled: self.disabled.unwrap_or(false),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }
}

impl NotificationPolicy {
    pub(crate) fn summary(&self) -> NotificationPolicySummary {
        NotificationPolicySummary {
            policy_id: self.policy_id.clone(),
            name: self.name.clone(),
            matcher_count: self.match_labels.len(),
            contact_count: self.contact_ids.len(),
            disabled: self.disabled,
            updated_at_ms: self.updated_at_ms,
        }
    }

    fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.match_labels
            .iter()
            .all(|(key, value)| labels.get(key).map(String::as_str) == Some(value.as_str()))
    }
}

impl ContactPointKind {
    fn requires_secret_ref(self) -> bool {
        !matches!(self, Self::Email)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Slack => "slack",
            Self::Webhook => "webhook",
            Self::PagerDuty => "pager-duty",
            Self::Teams => "teams",
            Self::Opsgenie => "opsgenie",
        }
    }
}

pub(crate) fn save_contact_response(
    contact_point: ContactPoint,
    warnings: Vec<String>,
) -> SaveContactPointResponse {
    SaveContactPointResponse {
        ok: true,
        contact_point,
        warnings,
    }
}

pub(crate) fn save_policy_response(
    policy: NotificationPolicy,
    warnings: Vec<String>,
) -> SaveNotificationPolicyResponse {
    SaveNotificationPolicyResponse {
        ok: true,
        policy,
        warnings,
    }
}

pub(crate) fn contact_catalog_payload(contacts: Vec<ContactPointSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.alert-contact-points.v1",
        "contactPoints": contacts,
        "limits": limits_payload()
    })
}

pub(crate) fn policy_catalog_payload(policies: Vec<NotificationPolicySummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.alert-notification-policies.v1",
        "policies": policies,
        "limits": limits_payload()
    })
}

pub(crate) fn preview(
    input: NotificationPreviewInput,
    policies: &[NotificationPolicy],
    contacts: &BTreeMap<String, ContactPoint>,
) -> NotificationPreviewResponse {
    let mut warnings = Vec::new();
    let mut matched_policies = Vec::new();
    let mut deliveries = Vec::new();
    let mut delivered_contact_ids = BTreeSet::new();
    let mut ordered_policies = policies.to_vec();
    ordered_policies.sort_by(|left, right| {
        right
            .match_labels
            .len()
            .cmp(&left.match_labels.len())
            .then_with(|| left.policy_id.cmp(&right.policy_id))
    });

    for policy in ordered_policies {
        if !policy.matches(&input.labels) {
            continue;
        }
        matched_policies.push(MatchedNotificationPolicy {
            policy_id: policy.policy_id.clone(),
            name: policy.name.clone(),
            match_labels: policy.match_labels.clone(),
            contact_ids: policy.contact_ids.clone(),
            disabled: policy.disabled,
        });
        if policy.disabled {
            warnings.push(format!(
                "notification policy `{}` is disabled",
                policy.policy_id
            ));
        } else {
            for contact_id in &policy.contact_ids {
                if !delivered_contact_ids.insert(contact_id.clone()) {
                    continue;
                }
                match contacts.get(contact_id) {
                    Some(contact) => deliveries.push(NotificationDeliveryPlan {
                        contact_id: contact.contact_id.clone(),
                        name: contact.name.clone(),
                        kind: contact.kind,
                        secret_ref_configured: contact.secret_ref.is_some(),
                        disabled: contact.disabled,
                        delivery_mode: "dry-run-blueprint",
                    }),
                    None => warnings.push(format!(
                        "notification policy `{}` references missing contact `{contact_id}`",
                        policy.policy_id
                    )),
                }
            }
        }
        if !policy.continue_matching {
            break;
        }
    }
    if matched_policies.is_empty() {
        warnings.push("no notification policies matched alert labels".to_string());
    }
    let annotation_keys = input.annotations.keys().cloned().collect::<Vec<_>>();
    NotificationPreviewResponse {
        ok: true,
        schema_version: "data-viz.alert-notification-preview.v1",
        previewed_at_ms: now_ms(),
        rule_id: input.rule_id,
        title: input.title,
        matched_policy_count: matched_policies.len(),
        delivery_count: deliveries.len(),
        matched_policies,
        deliveries,
        labels: input.labels,
        annotation_keys,
        warnings,
    }
}

pub(crate) fn max_contact_points() -> usize {
    MAX_CONTACT_POINTS
}

pub(crate) fn max_notification_policies() -> usize {
    MAX_NOTIFICATION_POLICIES
}

fn normalize_matchers(
    matchers: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if matchers.len() > MAX_MATCHERS {
        return Err(format!("notification matchers exceeds max {MAX_MATCHERS}"));
    }
    let mut normalized = BTreeMap::new();
    for (key, value) in matchers {
        let key = clean_identifier(&key)
            .ok_or_else(|| "notification matcher key is invalid".to_string())?;
        let value = bounded_label("notification matcher value", &value, 160)?;
        normalized.insert(key, value);
    }
    Ok(normalized)
}

fn normalize_group_by(values: Vec<String>) -> Result<Vec<String>, String> {
    if values.len() > MAX_MATCHERS {
        return Err(format!(
            "notification policy groupBy exceeds max {MAX_MATCHERS}"
        ));
    }
    let mut group_by = values
        .into_iter()
        .filter_map(|value| clean_identifier(&value))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    group_by.sort();
    Ok(group_by)
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_TAGS {
        return Err(format!("contact point tags exceeds max {MAX_TAGS}"));
    }
    let mut tags = tags
        .into_iter()
        .filter_map(|tag| clean_identifier(&tag.to_ascii_lowercase()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    tags.sort();
    Ok(tags)
}

fn normalize_settings(
    settings: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if settings.len() > MAX_SETTINGS {
        return Err(format!("contact point settings exceeds max {MAX_SETTINGS}"));
    }
    let mut normalized = BTreeMap::new();
    for (key, value) in settings {
        let key = clean_identifier(&key).ok_or_else(|| "contact setting key is invalid")?;
        let key_lower = key.to_ascii_lowercase();
        if key_lower.contains("token")
            || key_lower.contains("secret")
            || key_lower.contains("password")
            || key_lower == "url"
            || key_lower.ends_with("_url")
            || key_lower.ends_with("url")
        {
            return Err(format!(
                "contact setting `{key}` looks secret-bearing; use secretRef instead"
            ));
        }
        let value = value.trim().to_string();
        if value.len() > MAX_SETTING_VALUE_BYTES {
            return Err(format!(
                "contact setting `{key}` exceeds max {MAX_SETTING_VALUE_BYTES} bytes"
            ));
        }
        normalized.insert(key, value);
    }
    Ok(normalized)
}

fn bounded_label(label: &str, value: &str, max_len: usize) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > max_len {
        Err(format!("{label} must be 1-{max_len} characters"))
    } else {
        Ok(value)
    }
}

fn limits_payload() -> Value {
    json!({
        "maxContactPoints": MAX_CONTACT_POINTS,
        "maxNotificationPolicies": MAX_NOTIFICATION_POLICIES,
        "maxMatchers": MAX_MATCHERS,
        "maxContactIds": MAX_CONTACT_IDS,
        "maxTags": MAX_TAGS,
        "maxSettings": MAX_SETTINGS,
        "maxSettingValueBytes": MAX_SETTING_VALUE_BYTES
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_point_requires_secret_ref_for_webhook_like_channels() {
        let error = SaveContactPointRequest {
            contact_id: "prod-slack".to_string(),
            name: "Prod Slack".to_string(),
            kind: ContactPointKind::Slack,
            owner: None,
            tags: None,
            secret_ref: None,
            settings: None,
            disabled: None,
        }
        .into_contact_point(100)
        .expect_err("secret ref required");

        assert!(error.contains("secretRef"));
    }

    #[test]
    fn notification_preview_matches_policy_and_redacts_delivery() {
        let contact = SaveContactPointRequest {
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
        .expect("contact validates");
        let policy = SaveNotificationPolicyRequest {
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
        .expect("policy validates");
        let contacts = BTreeMap::from([("prod-slack".to_string(), contact)]);
        let response = preview(
            NotificationPreviewInput {
                rule_id: "latency".to_string(),
                title: "High latency".to_string(),
                labels: BTreeMap::from([
                    ("severity".to_string(), "critical".to_string()),
                    ("service".to_string(), "api".to_string()),
                ]),
                annotations: BTreeMap::from([("summary".to_string(), "Too slow".to_string())]),
            },
            &[policy],
            &contacts,
        );

        assert_eq!(response.matched_policy_count, 1);
        assert_eq!(response.delivery_count, 1);
        assert!(response.deliveries[0].secret_ref_configured);
        assert_eq!(response.deliveries[0].delivery_mode, "dry-run-blueprint");
    }
}
