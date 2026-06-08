use serde::{Deserialize, Serialize};

use crate::{config::SCHEMA_VERSION, standards::all_standard_ids};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRequest {
    pub request_id: Option<String>,
    pub schema_version: Option<String>,
    pub standard_ids: Option<Vec<String>>,
    pub target: AuditTarget,
    #[serde(default)]
    pub evidence: Vec<EvidenceItem>,
    pub options: Option<AuditOptions>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTarget {
    pub kind: AuditTargetKind,
    pub name: Option<String>,
    pub uri: Option<String>,
    pub repo_url: Option<String>,
    pub git_ref: Option<String>,
    pub inline_text: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AuditTargetKind {
    Artifact,
    Codebase,
    Network,
    System,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceItem {
    pub control_id: String,
    pub standard_id: Option<String>,
    pub description: String,
    pub artifact_ref: Option<String>,
    pub confidence: Option<f64>,
    pub status: Option<EvidenceStatus>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceStatus {
    Observed,
    Gap,
    NotApplicable,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditOptions {
    pub include_all_controls: Option<bool>,
    pub fail_on_unknown_standards: Option<bool>,
    pub fetch_external_artifact: Option<bool>,
    pub clone_repo: Option<bool>,
    pub max_findings: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub ok: bool,
    pub request_id: String,
    pub schema_version: String,
    pub target: AuditTarget,
    pub standards: Vec<String>,
    pub score: f64,
    pub status: ComplianceStatus,
    pub summary: String,
    pub standard_results: Vec<StandardResult>,
    pub findings: Vec<Finding>,
    pub collected_artifacts: Vec<CollectedArtifact>,
    pub generated_at_ms: u128,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ComplianceStatus {
    EvidenceCompliant,
    PartiallyCompliant,
    NonCompliant,
    NeedsReview,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StandardResult {
    pub standard_id: String,
    pub standard_name: String,
    pub category: String,
    pub jurisdiction: String,
    pub score: f64,
    pub status: ComplianceStatus,
    pub controls_total: usize,
    pub controls_satisfied: usize,
    pub controls_needs_evidence: usize,
    pub controls_failed: usize,
    pub control_results: Vec<ControlResult>,
    pub regulatory_notice: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlResult {
    pub control_id: String,
    pub name: String,
    pub family: String,
    pub status: ControlStatus,
    pub evidence_count: usize,
    pub evidence_refs: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ControlStatus {
    Satisfied,
    NeedsEvidence,
    Failed,
    NotApplicable,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: FindingSeverity,
    pub standard_id: Option<String>,
    pub control_id: Option<String>,
    pub message: String,
    pub recommendation: String,
    pub evidence_required: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub enum FindingSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedArtifact {
    pub kind: String,
    pub source: String,
    pub bytes: usize,
    pub scanned_files: usize,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub request_id: String,
    pub status: JobStatus,
    pub created_at_ms: u128,
    pub started_at_ms: Option<u128>,
    pub finished_at_ms: Option<u128>,
    pub result: Option<AuditReport>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

pub fn example_request() -> AuditRequest {
    AuditRequest {
        request_id: Some("example-saas-startup-readiness".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        standard_ids: Some(vec![
            "soc-2".to_string(),
            "iso-27001".to_string(),
            "gdpr".to_string(),
            "pci-dss".to_string(),
            "hipaa".to_string(),
            "ccpa".to_string(),
            "fedramp".to_string(),
            "cmmc".to_string(),
        ]),
        target: AuditTarget {
            kind: AuditTargetKind::Codebase,
            name: Some("example-b2b-saas".to_string()),
            uri: None,
            repo_url: None,
            git_ref: None,
            inline_text: Some(
                "Policies: access review, MFA, encryption at rest and in transit, logging, incident response, vendor due diligence, retention and deletion, privacy notice, secure SDLC, vulnerability scanning."
                    .to_string(),
            ),
            tags: vec!["startup".to_string(), "saas".to_string()],
        },
        evidence: vec![
            EvidenceItem {
                control_id: "access_control".to_string(),
                standard_id: None,
                description: "Quarterly access reviews and MFA are enforced for production."
                    .to_string(),
                artifact_ref: Some("iam/access-review-q2".to_string()),
                confidence: Some(0.9),
                status: Some(EvidenceStatus::Observed),
            },
            EvidenceItem {
                control_id: "payment_security".to_string(),
                standard_id: Some("pci-dss".to_string()),
                description: "Card data is tokenized by the payment processor; PAN is not stored."
                    .to_string(),
                artifact_ref: Some("payments/tokenization-design".to_string()),
                confidence: Some(0.8),
                status: Some(EvidenceStatus::Observed),
            },
        ],
        options: Some(AuditOptions {
            include_all_controls: Some(false),
            fail_on_unknown_standards: Some(true),
            fetch_external_artifact: Some(false),
            clone_repo: Some(false),
            max_findings: Some(200),
        }),
    }
}

pub fn schema_example() -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": SCHEMA_VERSION,
        "targetKinds": ["artifact", "codebase", "network", "system"],
        "standardIds": all_standard_ids(),
        "request": {
            "requestId": "string optional",
            "schemaVersion": SCHEMA_VERSION,
            "standardIds": ["soc-2", "iso-27001", "gdpr"],
            "target": {
                "kind": "codebase",
                "name": "string optional",
                "uri": "https://example.com/artifact.json optional",
                "repoUrl": "https://github.com/org/repo.git optional",
                "gitRef": "branch-or-tag optional",
                "inlineText": "operator-supplied evidence or source text optional",
                "tags": ["saas", "prod"]
            },
            "evidence": [{
                "controlId": "access_control",
                "standardId": "soc-2 optional",
                "description": "MFA is enforced for production access.",
                "artifactRef": "iam/report-q2",
                "confidence": 0.9,
                "status": "observed"
            }],
            "options": {
                "includeAllControls": false,
                "failOnUnknownStandards": true,
                "fetchExternalArtifact": false,
                "cloneRepo": false,
                "maxFindings": 200
            }
        }
    })
}
