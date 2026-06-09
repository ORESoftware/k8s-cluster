use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    review_families: Vec<String>,
    machine_kinds: Vec<String>,
    check_scopes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.manufacturability-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /manufacturability/catalog", "GET /fabrication/manufacturability/catalog"],
        "reviewFamilyCount": entries.len(),
        "reviewFamilies": review_families,
        "machineKinds": machine_kinds,
        "checkScopes": check_scopes,
        "planningRoutes": [
            "POST /design/import/review",
            "POST /fabrication/design/import/review",
            "POST /design/generate",
            "POST /fabrication/design/generate",
            "POST /decomposition/plan",
            "POST /fabrication/decomposition/plan",
            "POST /machine-code/generate",
            "POST /fabrication/machine-code/generate"
        ],
        "reviewRoutes": [
            "POST /design/import/result",
            "POST /fabrication/design/import/result",
            "POST /manufacturability/result",
            "POST /fabrication/manufacturability/result",
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate",
            "POST /release/preview",
            "POST /fabrication/release/preview"
        ],
        "responseSurfaces": [
            "designInputReview.manufacturabilityEvidence",
            "designInputReview.conversionPlan",
            "processCapabilityContracts",
            "decompositionPlan.parts",
            "interfaceControlPlan.interfaces",
            "assemblyPlan.requiredEvidence",
            "qualityPlan.measurementTargets",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "manufacturability-catalog",
            "design-input-review",
            "process-capability-catalog",
            "decomposition-plan",
            "interface-control-plan",
            "mdp-request.artifacts.manufacturabilityEvidence"
        ],
        "releasePolicy": [
            "manufacturability catalog entries describe DFM, DfAM, tool-access, flat-pattern, and hybrid interface review evidence, not certified design approval",
            "machine-ready release remains blocked when CAD, mesh, sheet, or assembly geometry needs redesign, repair, alternate routing, split/combine planning, or human intervention evidence",
            "manufacturability failures, redesign actions, split/combine decisions, and successful route outcomes are retained as MDP/POMDP/neural learning signals"
        ],
        "manufacturabilityContracts": entries
    })
}
