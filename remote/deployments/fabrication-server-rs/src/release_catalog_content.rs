use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    gate_contracts: Vec<Value>,
    gate_types: Vec<String>,
    package_kinds: Vec<Value>,
    package_kind_names: Vec<String>,
    required_artifacts: Vec<&'static str>,
    blocker_sources: Vec<Value>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.release-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /release/catalog", "GET /fabrication/release/catalog"],
        "packageKindCount": package_kinds.len(),
        "gateCount": gate_contracts.len(),
        "packageKinds": package_kind_names,
        "gateTypes": gate_types,
        "releaseStates": ["release-blocked", "release-review-ready", "machine-ready"],
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "machineRelease.status",
            "machineRelease.blockers",
            "machineRelease.checklist",
            "releasePackagePlan.packages",
            "releasePackagePlan.releaseGates",
            "releasePackagePlan.requiredArtifacts",
            "releasePackagePlan.learningObservations",
            "controllerPlan.releaseGates",
            "postprocessPlan.blockers",
            "simulation.riskProfile",
            "decompositionPlan.releaseGates",
            "interfaceControlPlan.releaseGates"
        ],
        "learningSurfaces": [
            "releasePackagePlan.learningObservations",
            "releaseProbePlan.probes",
            "pomdpBeliefState.hiddenStates",
            "neuralTrainingCorpus.examples",
            "mdp-request.artifacts.releasePackagePlan"
        ],
        "requiredArtifacts": required_artifacts,
        "blockerSources": blocker_sources,
        "releasePolicy": [
            "release catalog entries describe machine-ready evidence contracts, not certified equipment safety",
            "machine-ready release remains blocked until validation findings, failure boundaries, release probes, controller/postprocessor checks, simulation or dry-run evidence, split/combine interface gates, and operator or automation signoff clear",
            "release-package observations are emitted for MDP/POMDP/neural workers so future planning can learn which evidence cleared or blocked printed, milled, turned, sheet-cut, EDM, and recomposed routes"
        ],
        "packageKindContracts": package_kinds,
        "gateContracts": gate_contracts
    })
}
