use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(entries: Vec<Value>, utility_families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.utilities-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /utilities/catalog", "GET /fabrication/utilities/catalog"],
        "utilityFamilyCount": entries.len(),
        "utilityFamilies": utility_families,
        "planningRoutes": [
            "POST /plan",
            "POST /fabrication/plan",
            "POST /monitoring/result",
            "POST /fabrication/monitoring/result",
            "POST /failure-modes/result",
            "POST /fabrication/failure-modes/result",
            "POST /learning/observe",
            "POST /fabrication/learning/observe"
        ],
        "responseSurfaces": [
            "validation.failureBoundaries",
            "supportStrategyPlan.requirements",
            "monitoringPlan.alerts",
            "fixturePlan.setups",
            "toolingPlan.requirements",
            "executionPlan.stopPoints",
            "operatorInterventionPlan.requiredOperatorActions",
            "scheduleResult.holds",
            "learning.outcomes",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "utility-readiness-record",
            "facility-power-recovery-record",
            "coolant-chip-dust-state-record",
            "sheet-cut-support-media-record",
            "additive-thermal-material-state-record",
            "hybrid-cell-service-state-record",
            "mdp-request.artifacts.utilities"
        ],
        "releasePolicy": [
            "utilities catalog entries describe process-support and facility-readiness evidence contracts, not certified machine safety approval or facility compliance",
            "machine-ready release remains blocked while power, network, thermal, material-supply, coolant, chip, dust, gas, pump, abrasive, fume, vacuum, fixture, robot, or recovery utilities lack retained evidence",
            "utility outages, restarts, operator recovery, and environmental excursions are retained as MDP/POMDP/neural learning signals so future planners can resequence, add checkpoints, split work, or avoid brittle unattended routes"
        ],
        "utilityContracts": entries
    })
}
