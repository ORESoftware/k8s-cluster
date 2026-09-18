use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    failure_families: Vec<String>,
    machine_kinds: Vec<String>,
    failure_modes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.failure-mode-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /failure-modes/catalog", "GET /fabrication/failure-modes/catalog"],
        "failureFamilyCount": entries.len(),
        "failureFamilies": failure_families,
        "machineKinds": machine_kinds,
        "failureModes": failure_modes,
        "planningRoutes": [
            "POST /failure-modes/plan",
            "POST /fabrication/failure-modes/plan",
            "POST /instructions/analyze",
            "POST /fabrication/instructions/analyze",
            "POST /instructions/boundaries/review",
            "POST /fabrication/instructions/boundaries/review",
            "POST /decomposition/plan",
            "POST /fabrication/decomposition/plan",
            "POST /strategy/recommend",
            "POST /fabrication/strategy/recommend"
        ],
        "reviewRoutes": [
            "POST /failure-modes/result",
            "POST /fabrication/failure-modes/result",
            "POST /simulation/result",
            "POST /fabrication/simulation/result",
            "POST /execution/result",
            "POST /fabrication/execution/result",
            "POST /learning/outcomes",
            "POST /fabrication/learning/outcomes"
        ],
        "responseSurfaces": [
            "boundarySummary.boundaries",
            "interventionMap.requiredInterventions",
            "simulation.riskProfile.programRisks",
            "decompositionPlan.parts",
            "executionPlan.stopPoints",
            "learning.outcomes",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "failure-mode-catalog",
            "boundary-summary",
            "intervention-map",
            "simulation-report",
            "execution-plan",
            "mdp-request.artifacts.failureModes"
        ],
        "releasePolicy": [
            "failure-mode catalog entries describe process-failure signatures and evidence gates, not certified machine diagnostics",
            "machine-ready release remains blocked while likely failure modes require unresolved human intervention, redesign, support restart, tool/process state recovery, or split/combine planning",
            "failure signatures, remediation choices, split/combine outcomes, and operator interventions are retained as MDP/POMDP/neural learning signals"
        ],
        "failureModeContracts": entries
    })
}
