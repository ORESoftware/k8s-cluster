use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    inspection_contracts: Vec<Value>,
    measurement_contracts: Vec<Value>,
    families: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.quality-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /quality/catalog", "GET /fabrication/quality/catalog"],
        "inspectionContractCount": inspection_contracts.len(),
        "measurementContractCount": measurement_contracts.len(),
        "families": families,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "qualityPlan.status",
            "qualityPlan.inspectionPoints",
            "qualityPlan.measurementTargets",
            "qualityPlan.releaseGates",
            "validation.failureBoundaries",
            "machineRelease.blockers",
            "postprocessPlan.blockers",
            "releasePackagePlan.releaseGates",
            "interfaceControlPlan.controls"
        ],
        "artifactSurfaces": [
            "quality-plan",
            "mdp-request.artifacts.qualityPlan",
            "first-article-metrology-record",
            "final-fit-metrology-record",
            "surface-finish-inspection-record",
            "material-process-coupon-record"
        ],
        "learningSurfaces": [
            "qualityPlan.learningObservations",
            "quality-gate:*",
            "measurement-target:*",
            "quality-boundary:*",
            "assembly-quality-interfaces:*"
        ],
        "releasePolicy": [
            "quality catalog entries describe inspection and measurement evidence contracts, not certified acceptance results",
            "machine-ready release remains blocked while required quality inspection, postprocess, material traceability, interface fit, or metrology evidence is absent",
            "quality observations are retained for MDP/POMDP/neural workers so future planning can learn when to add inspection, split parts, adjust processes, or require human signoff"
        ],
        "inspectionContracts": inspection_contracts,
        "measurementContracts": measurement_contracts
    })
}
