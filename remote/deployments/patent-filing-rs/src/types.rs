use serde::{Deserialize, Serialize};

use crate::{
    claims::ClaimAudit, deadlines::DeadlineReport, fees::FeeEstimate, SCHEMA_VERSION,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatentIntakeRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) schema_version: Option<String>,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) inventor_names: Vec<String>,
    pub(crate) applicant: Option<String>,
    #[serde(default)]
    pub(crate) invention_summary: String,
    #[serde(default)]
    pub(crate) technical_field: String,
    #[serde(default)]
    pub(crate) problem: String,
    #[serde(default)]
    pub(crate) solution: String,
    #[serde(default)]
    pub(crate) novelty_claims: Vec<String>,
    #[serde(default)]
    pub(crate) embodiments: Vec<String>,
    #[serde(default)]
    pub(crate) alternatives: Vec<String>,
    #[serde(default)]
    pub(crate) advantages: Vec<String>,
    pub(crate) public_disclosure_date: Option<String>,
    pub(crate) provisional_filing_date: Option<String>,
    pub(crate) foreign_priority_date: Option<String>,
    pub(crate) target_filing: Option<String>,
    pub(crate) entity_status: Option<String>,
    pub(crate) desired_claim_count: Option<usize>,
    pub(crate) attorney_review: Option<bool>,
    #[serde(default)]
    pub(crate) known_prior_art: Vec<KnownPriorArt>,
    #[serde(default)]
    pub(crate) attachments: Vec<AttachmentEvidence>,
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnownPriorArt {
    pub(crate) title: String,
    pub(crate) url: Option<String>,
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentEvidence {
    pub(crate) name: String,
    pub(crate) kind: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatentMatterPackage {
    pub(crate) ok: bool,
    pub(crate) matter_id: String,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) generated_at_ms: u128,
    pub(crate) filing_track: String,
    pub(crate) title: String,
    pub(crate) applicant: Option<String>,
    pub(crate) inventor_names: Vec<String>,
    pub(crate) readiness: ReadinessReview,
    pub(crate) draft: ProvisionalDraft,
    pub(crate) search_plan: SearchPlan,
    pub(crate) claim_audit: ClaimAudit,
    pub(crate) fee_estimate: FeeEstimate,
    pub(crate) deadlines: DeadlineReport,
    pub(crate) filing_checklist: Vec<ChecklistItem>,
    pub(crate) attorney_handoff: AttorneyHandoff,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PackageReviewRequest {
    pub(crate) matter_id: Option<String>,
    pub(crate) package: Option<PatentMatterPackageInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatentMatterPackageInput {
    pub(crate) readiness_score: Option<u8>,
    pub(crate) blocker_count: Option<usize>,
    pub(crate) section_count: Option<usize>,
    pub(crate) checklist_open_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PackageReviewResponse {
    pub(crate) ok: bool,
    pub(crate) status: String,
    pub(crate) release_gate: String,
    pub(crate) findings: Vec<FilingFinding>,
    pub(crate) next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadinessReview {
    pub(crate) score: u8,
    pub(crate) status: String,
    pub(crate) blockers: Vec<FilingFinding>,
    pub(crate) warnings: Vec<FilingFinding>,
    pub(crate) strengths: Vec<String>,
    pub(crate) next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilingFinding {
    pub(crate) code: String,
    pub(crate) severity: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProvisionalDraft {
    pub(crate) title: String,
    pub(crate) abstract_draft: String,
    pub(crate) sections: Vec<DraftSection>,
    pub(crate) claim_seeds: Vec<String>,
    pub(crate) drawing_plan: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftSection {
    pub(crate) heading: String,
    pub(crate) body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchPlan {
    pub(crate) queries: Vec<SearchQuery>,
    pub(crate) classification_hints: Vec<String>,
    pub(crate) sources: Vec<SearchSource>,
    pub(crate) review_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchQuery {
    pub(crate) label: String,
    pub(crate) query: String,
    pub(crate) intent: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchSource {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) use_case: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChecklistItem {
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) owner: String,
    pub(crate) notes: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttorneyHandoff {
    pub(crate) summary: String,
    pub(crate) questions: Vec<String>,
    pub(crate) package_manifest: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UiPackageForm {
    pub(crate) title: String,
    pub(crate) inventor_names: String,
    pub(crate) applicant: Option<String>,
    pub(crate) technical_field: Option<String>,
    pub(crate) invention_summary: String,
    pub(crate) problem: String,
    pub(crate) solution: String,
    pub(crate) novelty_claims: String,
    pub(crate) embodiments: Option<String>,
    pub(crate) alternatives: Option<String>,
    pub(crate) advantages: Option<String>,
    pub(crate) known_prior_art: Option<String>,
    pub(crate) attachments: Option<String>,
    pub(crate) public_disclosure_date: Option<String>,
    pub(crate) provisional_filing_date: Option<String>,
    pub(crate) foreign_priority_date: Option<String>,
    pub(crate) target_filing: Option<String>,
    pub(crate) entity_status: Option<String>,
    pub(crate) attorney_review: Option<String>,
}

pub(crate) fn finding(code: &str, severity: &str, message: &str) -> FilingFinding {
    FilingFinding {
        code: code.to_string(),
        severity: severity.to_string(),
        message: message.to_string(),
    }
}

pub(crate) fn example_request() -> PatentIntakeRequest {
    PatentIntakeRequest {
        request_id: Some("example-patent-package".to_string()),
        schema_version: Some(SCHEMA_VERSION.to_string()),
        title: "Adaptive thermal sensor array".to_string(),
        inventor_names: vec!["Avery Chen".to_string(), "Morgan Patel".to_string()],
        applicant: Some("Example Robotics LLC".to_string()),
        invention_summary: "A distributed sensor array combines low-cost temperature probes, edge calibration, and a controller that changes sampling frequency based on local thermal gradients. Each node reports confidence and drift estimates so the controller can prioritize high-risk zones without flooding the network.".to_string(),
        technical_field: "distributed sensing and thermal control".to_string(),
        problem: "Existing thermal monitoring systems either sample too slowly to catch fast changes or sample every node constantly, which wastes network capacity and power in dense installations.".to_string(),
        solution: "The array estimates local gradients at each node, assigns an adaptive sampling budget, and routes high-confidence alerts through a compact priority protocol while slower regions remain in a low-power cadence.".to_string(),
        novelty_claims: vec![
            "node-level drift confidence changes sampling rates".to_string(),
            "gradient-triggered priority routing reduces bandwidth".to_string(),
            "controller fuses confidence scores with thermal risk zones".to_string(),
        ],
        embodiments: vec![
            "warehouse battery pack monitoring".to_string(),
            "server rack airflow diagnostics".to_string(),
        ],
        alternatives: vec![
            "wireless mesh nodes".to_string(),
            "wired industrial bus nodes".to_string(),
        ],
        advantages: vec![
            "lower power usage".to_string(),
            "reduced telemetry volume".to_string(),
            "faster high-risk thermal alerts".to_string(),
        ],
        public_disclosure_date: None,
        provisional_filing_date: None,
        foreign_priority_date: None,
        target_filing: Some("provisional".to_string()),
        entity_status: Some("micro".to_string()),
        desired_claim_count: Some(8),
        attorney_review: Some(true),
        known_prior_art: vec![KnownPriorArt {
            title: "Static threshold thermal monitoring systems".to_string(),
            url: None,
            notes: None,
        }],
        attachments: vec![
            AttachmentEvidence {
                name: "System block diagram".to_string(),
                kind: Some("figure".to_string()),
                url: None,
                notes: None,
            },
            AttachmentEvidence {
                name: "Sampling-state flow chart".to_string(),
                kind: Some("figure".to_string()),
                url: None,
                notes: None,
            },
        ],
        notes: None,
    }
}
