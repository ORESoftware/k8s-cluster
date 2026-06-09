use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    safety_families: Vec<String>,
    machine_kinds: Vec<String>,
    hazards: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.safety-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /safety/catalog", "GET /fabrication/safety/catalog"],
        "safetyFamilyCount": entries.len(),
        "safetyFamilies": safety_families,
        "machineKinds": machine_kinds,
        "hazards": hazards,
        "planningRoutes": [
            "POST /safety/plan",
            "POST /fabrication/safety/plan",
            "POST /monitoring/plan",
            "POST /fabrication/monitoring/plan",
            "POST /execution/plan",
            "POST /fabrication/execution/plan",
            "POST /release/preview",
            "POST /fabrication/release/preview"
        ],
        "reviewRoutes": [
            "POST /monitoring/result",
            "POST /fabrication/monitoring/result",
            "POST /execution/result",
            "POST /fabrication/execution/result",
            "POST /release/result",
            "POST /fabrication/release/result"
        ],
        "responseSurfaces": [
            "executionPlan.stopPoints",
            "executionPlan.operatorActions",
            "interventionMap.requiredInterventions",
            "monitoringPlan.monitorPoints",
            "monitoringPlan.alertRules",
            "monitoringPlan.recoveryActions",
            "releasePackagePlan.requiredArtifacts",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "monitoring-plan",
            "execution-plan",
            "operator-intervention-plan",
            "release-package-plan",
            "machine-release",
            "mdp-request.artifacts.safetyEvidence"
        ],
        "releasePolicy": [
            "safety catalog entries describe guarding, interlock, extraction, emergency-stop, lockout, and human-intervention evidence, not certified machine-safety approvals",
            "machine-ready release remains blocked until machine guarding, process support, operator intervention, emergency response, monitoring, alerting, and release signoff evidence clear",
            "interlock states, operator stops, extraction failures, E-stop events, recovery actions, and unattended-release outcomes are retained as MDP/POMDP/neural learning signals"
        ],
        "safetyContracts": entries
    })
}
