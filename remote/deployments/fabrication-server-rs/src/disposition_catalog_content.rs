use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(entries: Vec<Value>, disposition_families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.disposition-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /dispositions/catalog", "GET /fabrication/dispositions/catalog"],
        "dispositionFamilyCount": entries.len(),
        "dispositionFamilies": disposition_families,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /remediation/plan", "POST /fabrication/remediation/plan"],
        "reviewRoutes": [
            "POST /quality/result",
            "POST /fabrication/quality/result",
            "POST /failure-modes/result",
            "POST /fabrication/failure-modes/result",
            "POST /learning/observe",
            "POST /fabrication/learning/observe",
            "POST /release/result",
            "POST /fabrication/release/result"
        ],
        "responseSurfaces": [
            "qualityResult.measurements",
            "qualityResult.findings",
            "simulation.findings",
            "failureModeResult.failureEvents",
            "boundaryRemediationPlan.actions",
            "decompositionPlan.parts",
            "interfaceControlPlan.interfaces",
            "assemblyPlan.requiredEvidence",
            "releasePackagePlan.releaseGates",
            "learning.outcomes",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "quality-result",
            "failure-mode-result",
            "nonconformance-disposition-record",
            "boundary-remediation-plan",
            "release-readiness-result",
            "learning-outcomes"
        ],
        "releasePolicy": [
            "disposition catalog entries describe post-inspection, post-simulation, and post-failure decision evidence contracts, not certified quality acceptance",
            "machine-ready or customer release remains blocked while pass, rework, scrap, waiver, or split/combine redesign decisions lack retained evidence and human or automation authority",
            "disposition outcomes are retained as MDP/POMDP/neural learning signals so future planners can avoid failed routes, change fixtures, split parts, remake, or add inspection earlier"
        ],
        "dispositions": entries
    })
}
