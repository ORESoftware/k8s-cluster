use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    nesting_families: Vec<String>,
    machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.nesting-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /nesting/catalog", "GET /fabrication/nesting/catalog"],
        "nestingFamilyCount": entries.len(),
        "nestingFamilies": nesting_families,
        "machineKinds": machine_kinds,
        "planningRoutes": [
            "POST /plan",
            "POST /fabrication/plan",
            "POST /design/convert/plan",
            "POST /fabrication/design/convert/plan",
            "POST /toolpaths/plan",
            "POST /fabrication/toolpaths/plan",
            "POST /setup/plan",
            "POST /fabrication/setup/plan",
            "POST /execution/plan",
            "POST /fabrication/execution/plan"
        ],
        "reviewRoutes": [
            "POST /simulation/result",
            "POST /fabrication/simulation/result",
            "POST /workholding/result",
            "POST /fabrication/workholding/result",
            "POST /nesting/result",
            "POST /fabrication/nesting/result",
            "POST /support-strategies/result",
            "POST /fabrication/support-strategies/result",
            "POST /release/result",
            "POST /fabrication/release/result"
        ],
        "responseSurfaces": [
            "designExports.partExports",
            "designExports.partExports.content.nesting",
            "slicerPlan.profileEvidence",
            "toolingPlan.requirements.consumables",
            "fixturePlan.setups.workholding",
            "supportStrategyPlan",
            "postprocessPlan.steps",
            "executionPlan.stopPoints",
            "qualityPlan.measurementTargets",
            "releasePackagePlan.packages",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "dd-sheet-nesting-json",
            "dd-plate-layout-json",
            "powder-bed-build-map",
            "flat-blank-layout",
            "hybrid-kit-layout",
            "release-package",
            "mdp-request.artifacts.nestingCatalog"
        ],
        "learningSurfaces": [
            "nesting:additive-plate",
            "nesting:powder-bed",
            "nesting:sheet-cut",
            "nesting:sheet-forming",
            "nesting:hybrid-kit",
            "learning.outcomes"
        ],
        "releasePolicy": [
            "nesting catalog entries describe build-plate, powder-bed, sheet, flat-blank, and hybrid kit layout evidence contracts, not certified CAM or slicer nests",
            "machine-ready release remains blocked while layout envelope, material, support, tab/drop, thermal, traceability, fixture, postprocess, or operator/automation evidence is unresolved",
            "nesting observations are retained for MDP/POMDP/neural workers so future plans can adjust orientation, split jobs, change batch layout, add retention, or require human intervention earlier"
        ],
        "nestingContracts": entries
    })
}
