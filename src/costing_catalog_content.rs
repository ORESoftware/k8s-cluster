use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(entries: Vec<Value>, cost_families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.costing-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /costing/catalog", "GET /fabrication/costing/catalog"],
        "costFamilyCount": entries.len(),
        "costFamilies": cost_families,
        "planningRoutes": [
            "POST /plan",
            "POST /fabrication/plan",
            "POST /schedule/result",
            "POST /fabrication/schedule/result",
            "POST /learning/observe",
            "POST /fabrication/learning/observe"
        ],
        "responseSurfaces": [
            "machineSchedule.lanes",
            "materialPlan.quantity",
            "qualityPlan.releaseGates",
            "boundaryRemediationPlan.actions",
            "decompositionPlan.parts",
            "assemblyPlan.requiredEvidence",
            "releasePackagePlan.requiredArtifacts",
            "learning.outcomes",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "quote-assumption-record",
            "machine-time-estimate",
            "material-yield-estimate",
            "quality-risk-estimate",
            "split-combine-route-comparison",
            "controller-review-effort-record",
            "mdp-request.artifacts.costing"
        ],
        "releasePolicy": [
            "costing catalog entries describe estimation evidence contracts, not binding quotes, certified cost accounting, or shop-floor release authorization",
            "machine-ready and customer release remain blocked when route economics omit setup, material yield, scrap, quality, review, human intervention, or split/combine evidence",
            "cost, yield, scrap, cycle-time, and rework outcomes are retained as MDP/POMDP/neural learning signals so future planners can choose cheaper, safer, or more reliable routes"
        ],
        "costContracts": entries
    })
}
