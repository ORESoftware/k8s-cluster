use serde::Serialize;
use serde_json::{json, Value};

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
            "serverAuthSecretConfigured": auth_configured,
            "allowUnauthenticated": allow_unauthenticated,
            "localBypassEnv": "DATA_VIZ_ALLOW_UNAUTHENTICATED"
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
            evidence: "require_operator_auth checks X-Server-Auth, Auth, and bearer authorization.",
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
            severity: "medium",
            risk: "A single operator secret is coarse compared with enterprise RBAC.",
            planned_mitigation: "Integrate gateway identity, roles, audit logs, and per-resource policies.",
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
