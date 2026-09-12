use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    tool_families: Vec<String>,
    machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.tooling-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /tooling/catalog", "GET /fabrication/tooling/catalog"],
        "toolFamilyCount": entries.len(),
        "toolFamilies": tool_families,
        "machineKinds": machine_kinds,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /setup/plan", "POST /fabrication/setup/plan"],
        "reviewRoutes": [
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate",
            "POST /machine-code/generate",
            "POST /fabrication/machine-code/generate",
            "POST /toolpaths/plan",
            "POST /fabrication/toolpaths/plan",
            "POST /simulation/run",
            "POST /fabrication/simulation/run",
            "POST /tooling/result",
            "POST /fabrication/tooling/result"
        ],
        "responseSurfaces": [
            "toolingPlan.requirements.requiredTools",
            "toolingPlan.requirements.consumables",
            "toolingPlan.requirements.setupChecks",
            "toolingPlan.releaseGates",
            "fixturePlan.setups.requiredEvidence",
            "controllerPlan.requiredControllerChecks",
            "calibrationPlan.offsetEvidence",
            "qualityPlan.measurementTargets",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "tooling-plan",
            "fixture-plan",
            "calibration-plan",
            "quality-plan",
            "controller-plan",
            "machine-release",
            "mdp-request.artifacts.toolingPlan"
        ],
        "releasePolicy": [
            "tooling catalog entries describe required tool, consumable, offset, holder, probe, and support evidence, not certified tooling setup sheets",
            "machine-ready release remains blocked until tool identity, geometry, offsets, wear/tool-life, process support, calibration, and operator or automation signoff evidence clear",
            "tool selection, tool-life, offset, feed/speed, support-media, and inspection outcomes are retained as MDP/POMDP/neural learning signals for future planning and instruction repair"
        ],
        "toolingFamilies": entries
    })
}
