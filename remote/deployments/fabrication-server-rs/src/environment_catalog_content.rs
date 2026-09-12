use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    environment_families: Vec<String>,
    machine_kinds: Vec<String>,
    condition_scopes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.environment-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /environment/catalog", "GET /fabrication/environment/catalog"],
        "environmentFamilyCount": entries.len(),
        "environmentFamilies": environment_families,
        "machineKinds": machine_kinds,
        "conditionScopes": condition_scopes,
        "planningRoutes": [
            "POST /environment/plan",
            "POST /fabrication/environment/plan",
            "POST /materials/plan",
            "POST /fabrication/materials/plan",
            "POST /monitoring/plan",
            "POST /fabrication/monitoring/plan",
            "POST /quality/plan",
            "POST /fabrication/quality/plan"
        ],
        "reviewRoutes": [
            "POST /materials/result",
            "POST /fabrication/materials/result",
            "POST /monitoring/result",
            "POST /fabrication/monitoring/result",
            "POST /quality/result",
            "POST /fabrication/quality/result"
        ],
        "responseSurfaces": [
            "materialPlan.routeRequirements",
            "processRecipe.materialConditioning",
            "processRecipe.coolant",
            "monitoringPlan.monitorPoints",
            "monitoringPlan.alertRules",
            "qualityPlan.measurementTargets",
            "calibrationPlan.requiredEvidence",
            "releasePackagePlan.requiredArtifacts",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "material-plan",
            "monitoring-plan",
            "quality-plan",
            "calibration-plan",
            "environment-evidence",
            "mdp-request.artifacts.environmentEvidence"
        ],
        "releasePolicy": [
            "environment catalog entries describe humidity, thermal, coolant, chip, utility, extraction, vibration, and metrology-environment evidence, not certified facility qualifications",
            "machine-ready release remains blocked until material conditioning, ambient/process utilities, extraction, thermal stability, monitoring, inspection environment, and signoff evidence clear",
            "humidity, drying, coolant, extraction, utility, vibration, temperature, and metrology outcomes are retained as MDP/POMDP/neural learning signals"
        ],
        "environmentContracts": entries
    })
}
