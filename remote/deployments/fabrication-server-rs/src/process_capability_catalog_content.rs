use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    capability_families: Vec<String>,
    machine_kinds: Vec<String>,
    capability_scopes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.process-capability-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /process-capabilities/catalog", "GET /fabrication/process-capabilities/catalog"],
        "capabilityFamilyCount": entries.len(),
        "capabilityFamilies": capability_families,
        "machineKinds": machine_kinds,
        "capabilityScopes": capability_scopes,
        "planningRoutes": [
            "POST /process-capabilities/plan",
            "POST /fabrication/process-capabilities/plan",
            "POST /design/import/review",
            "POST /fabrication/design/import/review",
            "POST /machine-code/generate",
            "POST /fabrication/machine-code/generate",
            "POST /decomposition/plan",
            "POST /fabrication/decomposition/plan",
            "POST /release/preview",
            "POST /fabrication/release/preview"
        ],
        "reviewRoutes": [
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate",
            "POST /simulation/result",
            "POST /fabrication/simulation/result",
            "POST /quality/result",
            "POST /fabrication/quality/result"
        ],
        "responseSurfaces": [
            "designInputReview.capabilityFindings",
            "slicerPlan.profileEvidence",
            "toolingPlan.requirements",
            "processRecipe.cutChart",
            "decompositionPlan.parts",
            "interfaceControlPlan.interfaces",
            "qualityPlan.measurementTargets",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "process-capability-catalog",
            "design-input-review",
            "tooling-plan",
            "decomposition-plan",
            "simulation-report",
            "mdp-request.artifacts.processCapabilityEvidence"
        ],
        "releasePolicy": [
            "process-capability catalog entries describe printability, tool-access, workholding, kerf, and split/combine evidence, not certified machine capability studies",
            "machine-ready release remains blocked when requested geometry exceeds reviewed process capability and no redesign, alternate route, split/combine plan, or human intervention evidence is present",
            "capability failures, alternate routes, split boundaries, and measured process outcomes are retained as MDP/POMDP/neural learning signals"
        ],
        "processCapabilityContracts": entries
    })
}
