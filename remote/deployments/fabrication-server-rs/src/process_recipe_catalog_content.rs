use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    recipe_families: Vec<String>,
    machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.process-recipe-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /process-recipes/catalog", "GET /fabrication/process-recipes/catalog"],
        "recipeFamilyCount": entries.len(),
        "recipeFamilies": recipe_families,
        "machineKinds": machine_kinds,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /toolpaths/plan", "POST /fabrication/toolpaths/plan"],
        "reviewRoutes": [
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate",
            "POST /machine-code/generate",
            "POST /fabrication/machine-code/generate",
            "POST /simulation/run",
            "POST /fabrication/simulation/run",
            "POST /postprocess/plan",
            "POST /fabrication/postprocess/plan"
        ],
        "responseSurfaces": [
            "materialPlan.routeRequirements",
            "toolingPlan.requirements",
            "controllerPlan.requiredControllerChecks",
            "simulation.riskProfile",
            "qualityPlan.measurementTargets",
            "postprocessPlan.requiredArtifacts",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "tooling-plan",
            "controller-plan",
            "simulation-report",
            "quality-plan",
            "postprocess-plan",
            "mdp-request.artifacts.processRecipes"
        ],
        "releasePolicy": [
            "process recipe catalog entries describe required parameter, cut-chart, slicer-profile, thermal, chemical, and inspection evidence, not certified production recipes",
            "machine-ready release remains blocked until recipe provenance, material/tool/machine compatibility, simulation, first-article or coupon evidence, and operator or automation signoff clear",
            "recipe selections, parameter revisions, feed/speed outcomes, thermal cycles, edge quality, first-layer behavior, and postprocess results are retained as MDP/POMDP/neural learning signals"
        ],
        "processRecipes": entries
    })
}
