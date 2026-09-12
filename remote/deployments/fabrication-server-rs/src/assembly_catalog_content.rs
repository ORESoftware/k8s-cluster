use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    contracts: Vec<Value>,
    families: Vec<String>,
    worker_families: Vec<String>,
    response_surfaces: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.assembly-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /assembly/catalog", "GET /fabrication/assembly/catalog"],
        "contractCount": contracts.len(),
        "families": families,
        "workerFamilies": worker_families,
        "responseSurfaces": response_surfaces,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "relatedDiscoveryRoutes": [
            "GET /handoff/catalog",
            "GET /fabrication/handoff/catalog",
            "GET /decomposition/catalog",
            "GET /fabrication/decomposition/catalog",
            "GET /quality/catalog",
            "GET /fabrication/quality/catalog",
            "GET /release/catalog",
            "GET /fabrication/release/catalog"
        ],
        "artifactSurfaces": [
            "assembly-plan",
            "hybrid-make-plan",
            "interface-control-plan",
            "quality-plan",
            "assembly-kit-and-join-traveler",
            "assembly-recomposition-release",
            "release-package-plan",
            "mdp-request"
        ],
        "learningSurfaces": [
            "hybridMakePlan.learningObservations",
            "interfaceControlPlan.learningObservations",
            "qualityPlan.learningObservations",
            "releasePackagePlan.learningObservations",
            "learning.outcomes"
        ],
        "releasePolicy": [
            "assembly catalog entries describe worker-lane evidence contracts, not certified assembly or robot-cell release",
            "machine-ready release remains blocked until child route packages, interface controls, dry-fit or metrology, join recipe evidence, and operator or automation signoff are retained",
            "assembly, interface, quality, release, and outcome observations feed MDP/POMDP/neural workers so future plans can learn when to split, combine, recompose, or keep a part single-piece"
        ],
        "contracts": contracts
    })
}
