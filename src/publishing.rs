use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::clean_identifier;

const MAX_PUBLISH_REQUESTS: usize = 256;
const MAX_TAGS: usize = 24;
const MAX_NOTE_BYTES: usize = 1_024;
const MAX_LABEL_BYTES: usize = 160;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePublishRequest {
    pub request_id: Option<String>,
    pub target_kind: PublishTargetKind,
    pub target_id: String,
    pub requester: Option<String>,
    pub collection: Option<String>,
    pub notes: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewPublishRequest {
    pub decision: PublishDecision,
    pub reviewer: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishTargetMetadata {
    pub title: String,
    pub dataset_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PublishTargetKind {
    Dashboard,
    Question,
    Chart,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PublishDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PublishStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishRequestRecord {
    pub request_id: String,
    pub target_kind: PublishTargetKind,
    pub target_id: String,
    pub target_title: String,
    pub dataset_id: Option<String>,
    pub requester: Option<String>,
    pub collection: Option<String>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    pub status: PublishStatus,
    pub reviewer: Option<String>,
    pub review_comment: Option<String>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub reviewed_at_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishRequestSummary {
    request_id: String,
    target_kind: PublishTargetKind,
    target_id: String,
    target_title: String,
    dataset_id: Option<String>,
    requester: Option<String>,
    collection: Option<String>,
    tag_count: usize,
    status: PublishStatus,
    reviewer: Option<String>,
    updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavePublishResponse {
    ok: bool,
    request: PublishRequestRecord,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewPublishResponse {
    ok: bool,
    request: PublishRequestRecord,
}

impl SavePublishRequest {
    pub(crate) fn into_record(
        self,
        now_ms: u128,
        metadata: PublishTargetMetadata,
    ) -> Result<PublishRequestRecord, String> {
        let target_id = clean_identifier(&self.target_id)
            .ok_or_else(|| "targetId must be a safe identifier".to_string())?;
        let request_id = self
            .request_id
            .as_deref()
            .map(|value| {
                clean_identifier(value)
                    .ok_or_else(|| "requestId must be a safe identifier".to_string())
            })
            .transpose()?
            .unwrap_or_else(|| {
                format!(
                    "publish:{}:{}:{now_ms}",
                    self.target_kind.label(),
                    target_id
                )
            });
        let requester = self
            .requester
            .as_deref()
            .map(|value| bounded_label("requester", value))
            .transpose()?;
        let collection = self
            .collection
            .as_deref()
            .map(|value| bounded_label("collection", value))
            .transpose()?;
        let notes = self
            .notes
            .as_deref()
            .map(|value| bounded_note("publish notes", value))
            .transpose()?;
        let tags = normalize_tags(self.tags.unwrap_or_default())?;
        Ok(PublishRequestRecord {
            request_id,
            target_kind: self.target_kind,
            target_id,
            target_title: bounded_label("target title", &metadata.title)?,
            dataset_id: metadata.dataset_id,
            requester,
            collection,
            notes,
            tags,
            status: PublishStatus::Pending,
            reviewer: None,
            review_comment: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            reviewed_at_ms: None,
        })
    }
}

impl ReviewPublishRequest {
    pub(crate) fn apply_to(
        self,
        record: &mut PublishRequestRecord,
        now_ms: u128,
    ) -> Result<(), String> {
        let reviewer = bounded_label("reviewer", &self.reviewer)?;
        let comment = self
            .comment
            .as_deref()
            .map(|value| bounded_note("review comment", value))
            .transpose()?;
        record.status = match self.decision {
            PublishDecision::Approve => PublishStatus::Approved,
            PublishDecision::Reject => PublishStatus::Rejected,
        };
        record.reviewer = Some(reviewer);
        record.review_comment = comment;
        record.reviewed_at_ms = Some(now_ms);
        record.updated_at_ms = now_ms;
        Ok(())
    }
}

impl PublishRequestRecord {
    pub(crate) fn summary(&self) -> PublishRequestSummary {
        PublishRequestSummary {
            request_id: self.request_id.clone(),
            target_kind: self.target_kind,
            target_id: self.target_id.clone(),
            target_title: self.target_title.clone(),
            dataset_id: self.dataset_id.clone(),
            requester: self.requester.clone(),
            collection: self.collection.clone(),
            tag_count: self.tags.len(),
            status: self.status,
            reviewer: self.reviewer.clone(),
            updated_at_ms: self.updated_at_ms,
        }
    }
}

impl PublishTargetKind {
    fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Question => "question",
            Self::Chart => "chart",
        }
    }
}

pub(crate) fn save_response(
    request: PublishRequestRecord,
    warnings: Vec<String>,
) -> SavePublishResponse {
    SavePublishResponse {
        ok: true,
        request,
        warnings,
    }
}

pub(crate) fn review_response(request: PublishRequestRecord) -> ReviewPublishResponse {
    ReviewPublishResponse { ok: true, request }
}

pub(crate) fn catalog_payload(requests: Vec<PublishRequestSummary>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.publishing.v1",
        "requests": requests,
        "limits": limits_payload()
    })
}

pub(crate) fn max_publish_requests() -> usize {
    MAX_PUBLISH_REQUESTS
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxRequests": MAX_PUBLISH_REQUESTS,
        "maxTags": MAX_TAGS,
        "maxNoteBytes": MAX_NOTE_BYTES,
        "targetKinds": ["dashboard", "question", "chart"],
        "statuses": ["pending", "approved", "rejected"]
    })
}

fn bounded_label(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > MAX_LABEL_BYTES {
        Err(format!("{label} must be 1-{MAX_LABEL_BYTES} characters"))
    } else if looks_secret_bearing(&value) {
        Err(format!("{label} looks secret-bearing"))
    } else {
        Ok(value)
    }
}

fn bounded_note(label: &str, value: &str) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > MAX_NOTE_BYTES {
        Err(format!("{label} must be 1-{MAX_NOTE_BYTES} characters"))
    } else if looks_secret_bearing(&value) {
        Err(format!("{label} looks secret-bearing"))
    } else {
        Ok(value)
    }
}

fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > MAX_TAGS {
        return Err(format!("publish tags exceeds max {MAX_TAGS}"));
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

fn looks_secret_bearing(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "private_key",
        "credential",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> PublishTargetMetadata {
        PublishTargetMetadata {
            title: "Revenue by region".to_string(),
            dataset_id: Some("sales-lab".to_string()),
        }
    }

    #[test]
    fn publish_request_validates_and_summarizes_target() {
        let record = SavePublishRequest {
            request_id: Some("publish-1".to_string()),
            target_kind: PublishTargetKind::Chart,
            target_id: "revenue-by-region:chart".to_string(),
            requester: Some("analytics".to_string()),
            collection: Some("executive".to_string()),
            notes: Some("Ready for review".to_string()),
            tags: Some(vec!["Sales".to_string(), "sales".to_string()]),
        }
        .into_record(100, metadata())
        .expect("request validates");

        assert_eq!(record.status, PublishStatus::Pending);
        assert_eq!(record.tags, vec!["sales"]);
        assert_eq!(record.summary().target_title, "Revenue by region");
    }

    #[test]
    fn publish_review_marks_approval() {
        let mut record = SavePublishRequest {
            request_id: Some("publish-1".to_string()),
            target_kind: PublishTargetKind::Dashboard,
            target_id: "exec-sales".to_string(),
            requester: None,
            collection: None,
            notes: None,
            tags: None,
        }
        .into_record(100, metadata())
        .expect("request validates");

        ReviewPublishRequest {
            decision: PublishDecision::Approve,
            reviewer: "lead".to_string(),
            comment: Some("approved".to_string()),
        }
        .apply_to(&mut record, 200)
        .expect("review applies");

        assert_eq!(record.status, PublishStatus::Approved);
        assert_eq!(record.reviewed_at_ms, Some(200));
    }

    #[test]
    fn publish_request_rejects_secret_like_notes() {
        let error = SavePublishRequest {
            request_id: None,
            target_kind: PublishTargetKind::Question,
            target_id: "question-1".to_string(),
            requester: None,
            collection: None,
            notes: Some("token should not be here".to_string()),
            tags: None,
        }
        .into_record(100, metadata())
        .expect_err("secret-like notes rejected");

        assert!(error.contains("secret-bearing"));
    }
}
