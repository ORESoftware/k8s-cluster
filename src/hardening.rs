use serde::Serialize;
use serde_json::{json, Value};

use crate::rbac;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitDescriptor {
    pub id: &'static str,
    pub value: usize,
    pub rationale: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlDescriptor {
    pub id: &'static str,
    pub status: &'static str,
    pub description: &'static str,
    pub evidence: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidualRisk {
    pub id: &'static str,
    pub severity: &'static str,
    pub risk: &'static str,
    pub planned_mitigation: &'static str,
}

pub fn hardening_payload(
    max_datasets: usize,
    max_records: usize,
    max_columns: usize,
    max_query_rows: usize,
    max_body_bytes: usize,
    auth_configured: bool,
    allow_unauthenticated: bool,
) -> Value {
    json!({
        "ok": true,
        "schemaVersion": "data-viz.hardening.v1",
        "auth": {
            "operatorHeaders": ["X-Server-Auth", "Auth", "Authorization: Bearer ..."],
            "roleHeaders": ["X-Data-Viz-Role", "X-DD-Role"],
            "serverAuthSecretConfigured": auth_configured,
            "allowUnauthenticated": allow_unauthenticated,
            "localBypassEnv": "DATA_VIZ_ALLOW_UNAUTHENTICATED"
        },
        "rbac": {
            "schemaVersion": "data-viz.rbac.v1",
            "roles": rbac::policy_catalog()
        },
        "limits": limit_catalog(max_datasets, max_records, max_columns, max_query_rows, max_body_bytes),
        "controls": control_catalog(),
        "residualRisks": residual_risks(),
        "auditSummary": [
            "Operator endpoints require shared-secret auth unless the explicit local development bypass is enabled.",
            "Dataset and query sizes are bounded before entering the columnar execution path.",
            "The current service stores only in-memory user-supplied data; no raw cloud credentials or connector secrets are persisted.",
            "Generated presentation layers are inert JSON/XML/Markdown blueprints and do not execute external API calls."
        ]
    })
}

pub fn limit_catalog(
    max_datasets: usize,
    max_records: usize,
    max_columns: usize,
    max_query_rows: usize,
    max_body_bytes: usize,
) -> Vec<LimitDescriptor> {
    vec![
        LimitDescriptor {
            id: "http-body-bytes",
            value: max_body_bytes,
            rationale: "bound JSON parsing memory and keep operator mistakes contained",
        },
        LimitDescriptor {
            id: "datasets",
            value: max_datasets,
            rationale: "prevent unbounded in-process dataset accumulation",
        },
        LimitDescriptor {
            id: "records-per-dataset",
            value: max_records,
            rationale:
                "keep the first in-memory engine slice predictable until durable spill lands",
        },
        LimitDescriptor {
            id: "columns-per-dataset",
            value: max_columns,
            rationale: "prevent pathological wide records from dominating metadata and profiles",
        },
        LimitDescriptor {
            id: "query-result-rows",
            value: max_query_rows,
            rationale: "avoid returning massive API payloads from exploratory queries",
        },
    ]
}

pub fn control_catalog() -> Vec<ControlDescriptor> {
    vec![
        ControlDescriptor {
            id: "operator-auth",
            status: "implemented",
            description: "Mutating and data-bearing endpoints require an operator secret.",
            evidence: "authorize checks X-Server-Auth, Auth, and bearer authorization before role checks.",
        },
        ControlDescriptor {
            id: "role-based-access-control",
            status: "implemented",
            description: "Protected routes enforce role permissions after operator auth succeeds.",
            evidence: "authorize checks X-Data-Viz-Role / X-DD-Role against data-viz.rbac.v1 policy.",
        },
        ControlDescriptor {
            id: "identifier-validation",
            status: "implemented",
            description: "Dataset and field identifiers are restricted to safe ASCII tokens.",
            evidence: "clean_identifier and clean_field gate dataset IDs and projected fields.",
        },
        ControlDescriptor {
            id: "parser-backed-sql",
            status: "implemented",
            description: "SQL SELECT requests compile through sqlparser before becoming a LogicalPlan.",
            evidence: "src/sql_frontend.rs accepts one SELECT AST and rejects joins, CTEs, set operations, unsupported predicates, and unsupported aggregates fail-closed.",
        },
        ControlDescriptor {
            id: "bounded-associative-selection",
            status: "implemented",
            description: "Cross-dataset associative selection requests bound dataset count, selection count, and values returned per field.",
            evidence: "src/associative.rs enforces max dataset, selection, and field-value limits before computing Qlik-style state.",
        },
        ControlDescriptor {
            id: "bounded-alert-rules",
            status: "implemented",
            description: "Grafana-style alert rules are validated, capped, compiled through the query planner, and evaluated through bounded query results.",
            evidence: "src/alerts.rs validates rule metadata and /alerts/rules/:rule_id/evaluate reuses the existing LogicalPlan executor.",
        },
        ControlDescriptor {
            id: "secret-ref-alert-notifications",
            status: "implemented",
            description: "Grafana-style contact points and notification policies are capped, validate label matchers, reject secret-looking settings, and return dry-run delivery blueprints instead of sending outbound messages.",
            evidence: "src/notifications.rs requires secretRef for webhook-like channels and /alerts/rules/:rule_id/notification-preview computes delivery plans without network side effects.",
        },
        ControlDescriptor {
            id: "bounded-semantic-models",
            status: "implemented",
            description: "LookML-like semantic models are capped, parsed as a strict subset, and validated against ingested dataset fields before compilation.",
            evidence: "src/semantic.rs validates dimensions, measures, tags, and generated SQL targets before storing model definitions.",
        },
        ControlDescriptor {
            id: "bounded-dax-expressions",
            status: "implemented",
            description: "Power BI-style DAX expressions are capped by byte size, token count, nesting depth, and function arity, reject secret-looking text, and validate all field references against an ingested dataset.",
            evidence: "src/dax.rs parses a strict DAX subset and /expressions/dax/compile returns AST, dependencies, and SQL preview without evaluating user formulas.",
        },
        ControlDescriptor {
            id: "bounded-etl-plans",
            status: "implemented",
            description: "Domo Magic ETL/Power Query-style plans are capped by step count, output field count, aggregation count, and formula length, and formulas are treated as metadata instead of executed code.",
            evidence: "src/etl.rs validates requested fields against dataset shapes, returns lineage and pushdown hints, and does not inspect raw row values.",
        },
        ControlDescriptor {
            id: "secret-ref-connections",
            status: "implemented",
            description: "Warehouse and BI connection definitions are capped, require secretRef for network and cloud engines, reject secret-looking inline settings, and expose only dry-run test plans.",
            evidence: "src/connections.rs validates endpoints and settings; /connections/:connection_id/test-plan returns planner checks without opening sockets or cloud APIs.",
        },
        ControlDescriptor {
            id: "bounded-self-service-questions",
            status: "implemented",
            description: "Metabase/Superset-style saved questions and chart bindings are capped by field, filter, aggregation, tag, and encoding counts, then validated against ingested dataset fields before storage.",
            evidence: "src/self_service.rs compiles question-builder payloads into bounded SQL request metadata and rejects missing fields or invalid chart encodings.",
        },
        ControlDescriptor {
            id: "bounded-publishing-approvals",
            status: "implemented",
            description: "Publishing approval requests are capped, target saved dashboards/questions/charts must exist, notes/comments are bounded and reject secret-looking text, and reviews are RBAC-gated.",
            evidence: "src/publishing.rs validates request/review metadata; /publishing/requests/:request_id/review applies approve/reject decisions through PublishingReview permission.",
        },
        ControlDescriptor {
            id: "bounded-question-nl",
            status: "implemented",
            description: "Natural-language question proposals are bounded by prompt bytes and suggestion count, reject secret-looking prompt text, and map only to existing dataset fields.",
            evidence: "src/question_nl.rs produces deterministic QuestionBuilder proposals without model calls or query execution.",
        },
        ControlDescriptor {
            id: "bounded-sql-lab-history",
            status: "implemented",
            description: "Superset-style SQL Lab history is capped, accepts only single SELECT statements, rejects comments, mutating SQL, and secret-looking tokens, and keeps external connection entries dry-run only.",
            evidence: "src/sql_lab.rs validates stored SQL history; /sql-lab/history lists summaries without raw query text while detail reads remain role-gated.",
        },
        ControlDescriptor {
            id: "bounded-query-cache",
            status: "implemented",
            description: "Query result snapshots are capped by entry count, cached row count, and TTL, and cache summaries omit raw query text.",
            evidence: "src/query_cache.rs bounds in-memory snapshots; /query-cache lists redacted summaries and /query-cache/:cache_id is RBAC-gated.",
        },
        ControlDescriptor {
            id: "bounded-infra-diagrams",
            status: "implemented",
            description: "Terraform HCL, Terraform plan JSON, AWS, and GCP diagram requests are bounded by file bytes, import JSON bytes, resource count, node count, and edge count, and raw attributes are not echoed across the expanded renderer bundle.",
            evidence: "src/infra_diagrams.rs extracts topology references into neutral graph nodes, edges, and diagram-as-code, web graph, graph analytics, whiteboard, spatial, and Kroki renderer blueprints while enforcing source-size limits before returning derived diagrams.",
        },
        ControlDescriptor {
            id: "route-derived-docs",
            status: "implemented",
            description: "API documentation is served by the same route catalog used by the router.",
            evidence: "/docs/api, /api/docs, and /api/docs.json.",
        },
        ControlDescriptor {
            id: "explicit-telemetry",
            status: "implemented",
            description: "The service exposes Prometheus counters and dd.log.v1 startup logs without runtime monkey patching.",
            evidence: "/metrics and log_event.",
        },
        ControlDescriptor {
            id: "connector-secret-posture",
            status: "planned",
            description: "Future connector credentials must be references to environment or workload identities, never request payload secrets.",
            evidence: "connector catalog names auth modes but does not accept secret values.",
        },
        ControlDescriptor {
            id: "rbac",
            status: "planned",
            description: "Superset/enterprise parity requires role-aware dataset, chart, workbook, and export permissions.",
            evidence: "hardening surface documents the target until identity integration lands.",
        },
    ]
}

pub fn residual_risks() -> Vec<ResidualRisk> {
    vec![
        ResidualRisk {
            id: "in-memory-only",
            severity: "medium",
            risk: "Datasets and evolution runs disappear on restart and can consume process memory up to configured bounds.",
            planned_mitigation: "Add durable Arrow/Parquet spill and dataset TTL enforcement.",
        },
        ResidualRisk {
            id: "non-sql-parser-subsets",
            severity: "medium",
            risk: "Non-SQL dialect support is intentionally subset-based and should not be advertised as full GraphQL/PromQL/Flux/Cypher compatibility yet.",
            planned_mitigation: "Add async-graphql-parser, PromQL/LogQL parsers, and dialect-specific parser crates with fail-closed AST validation.",
        },
        ResidualRisk {
            id: "shared-secret-auth",
            severity: "low",
            risk: "Role checks are implemented, but identity still enters through a shared operator secret rather than gateway-backed user identity.",
            planned_mitigation: "Integrate gateway identity, audit logs, and per-resource policies.",
        },
        ResidualRisk {
            id: "presentation-blueprints",
            severity: "low",
            risk: "Presentation export returns package layers rather than writing final .pptx files or calling Google APIs.",
            planned_mitigation: "Move artifact generation into an authenticated worker that zips OpenXML and calls Google Slides with scoped OAuth.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardening_payload_reports_auth_and_limits() {
        let payload = hardening_payload(64, 50_000, 192, 5_000, 4_194_304, true, false);

        assert_eq!(payload["ok"], true);
        assert_eq!(payload["auth"]["serverAuthSecretConfigured"], true);
        assert!(payload["limits"].as_array().unwrap().len() >= 5);
        assert!(payload["residualRisks"].as_array().unwrap().len() >= 3);
    }
}
