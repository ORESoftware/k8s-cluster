use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    action_contracts: Vec<Value>,
    action_types: Vec<String>,
    action_families: Vec<String>,
    automation_contracts: Vec<Value>,
    automation_types: Vec<String>,
    evidence_gates: Vec<Value>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.intervention-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /interventions/catalog", "GET /fabrication/interventions/catalog"],
        "actionCount": action_contracts.len(),
        "automationTypeCount": automation_contracts.len(),
        "evidenceGateCount": evidence_gates.len(),
        "actionTypes": action_types,
        "actionFamilies": action_families,
        "automationTypes": automation_types,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "boundarySummary.automationRequirements",
            "interventionMap.humanInterventionPoints",
            "interventionMap.automationPaths",
            "interventionMap.splitCombineDecisions",
            "interventionMap.programBoundaries",
            "executionPlan.stopPoints",
            "executionPlan.checkpoints",
            "operatorInterventionPlan.requiredOperatorActions",
            "operatorInterventionPlan.evidenceGates",
            "operatorInterventionPlan.automationCandidates",
            "operatorInterventionPlan.splitCombineReviews",
            "releaseProbePlan.probes",
            "pomdpBeliefState.hiddenStates"
        ],
        "learningSurfaces": [
            "interventionMap.learningObservations",
            "operatorInterventionPlan.learningObservations",
            "executionPlan.learningObservations",
            "learning.interventionSignals",
            "neuralTrainingCorpus.examples",
            "mdp-request.artifacts.operatorInterventionPlan"
        ],
        "releasePolicy": [
            "intervention catalog entries describe preflight evidence contracts, not controller-certified restart instructions",
            "machine-ready release remains blocked while required operator actions, unresolved execution stop points, split/combine reviews, or unverified automation candidates remain open",
            "human-intervention and automation observations are emitted for MDP/POMDP/neural workers so future planning can learn when to add automation, split jobs, or keep human checkpoints"
        ],
        "actionContracts": action_contracts,
        "automationContracts": automation_contracts,
        "evidenceGateContracts": evidence_gates
    })
}
