use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    target_contracts: Vec<Value>,
    interface_modes: Vec<Value>,
    families: Vec<String>,
    family_counts: BTreeMap<String, usize>,
    target_kinds: Vec<String>,
    route_machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.decomposition-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /decomposition/catalog", "GET /fabrication/decomposition/catalog"],
        "targetCount": target_contracts.len(),
        "interfaceModeCount": interface_modes.len(),
        "families": families,
        "familyCounts": family_counts,
        "targetKinds": target_kinds,
        "routeMachineKinds": route_machine_kinds,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /controllers/plan", "POST /fabrication/controllers/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "hybridMakePlan.splitCombineDecisions",
            "decompositionPlan.targets",
            "decompositionPlan.routeContracts",
            "decompositionPlan.recompositionInterfaces",
            "decompositionPlan.releaseGates",
            "interfaceControlPlan.controls",
            "interfaceControlPlan.decisionLinks",
            "releasePackagePlan.packages"
        ],
        "learningSurfaces": [
            "decompositionPlan.learningObservations",
            "interfaceControlPlan.learningObservations",
            "mdp-request.artifacts.decompositionPlan",
            "mdp-request.artifacts.interfaceControlPlan",
            "learning.outcomes"
        ],
        "releasePolicy": [
            "decomposition catalog entries are draft split/combine and interface-control contracts, not certified assembly release",
            "machine-ready release remains blocked until child geometry, per-route machine code, datum transfer, interface metrology, recomposition, and operator or automation evidence are retained",
            "split/combine target kinds and interface-control signals are emitted as MDP/POMDP/neural observations so workers can compare single-piece, split-route, and recomposed outcomes"
        ],
        "targetContracts": target_contracts,
        "interfaceModes": interface_modes
    })
}
