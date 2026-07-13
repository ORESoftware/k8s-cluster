use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    contracts: Vec<Value>,
    families: Vec<String>,
    contract_types: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.setup-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /setup/catalog", "GET /fabrication/setup/catalog"],
        "setupContractCount": contracts.len(),
        "families": families,
        "contractTypes": contract_types,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "toolingPlan",
            "toolingPlan.requirements",
            "toolingPlan.requirements.requiredTools",
            "toolingPlan.requirements.workholding",
            "toolingPlan.requirements.consumables",
            "toolingPlan.requirements.setupChecks",
            "toolingPlan.requirements.automationDependencies",
            "toolingPlan.releaseGates",
            "fixturePlan",
            "fixturePlan.setups",
            "fixturePlan.setups.datumScheme",
            "fixturePlan.setups.requiredEvidence",
            "fixturePlan.setups.clearanceChecks",
            "fixturePlan.setups.automationCandidate",
            "fixturePlan.datumTransfers",
            "monitoringPlan",
            "monitoringPlan.monitorPoints",
            "monitoringPlan.alertRules",
            "monitoringPlan.recoveryActions",
            "monitoringPlan.releaseGates",
            "machineRelease.blockers",
            "releasePackagePlan.requiredArtifacts"
        ],
        "artifactSurfaces": [
            "tooling-plan",
            "fixture-plan",
            "monitoring-plan",
            "parametric-design.toolingPlan",
            "parametric-design.fixturePlan",
            "parametric-design.monitoringPlan",
            "mdp-request.artifacts.toolingPlan",
            "mdp-request.artifacts.fixturePlan",
            "mdp-request.artifacts.monitoringPlan"
        ],
        "learningSurfaces": [
            "toolingPlan.learningObservations",
            "fixturePlan.learningObservations",
            "monitoringPlan.learningObservations",
            "releaseProbePlan.probes",
            "neuralTrainingCorpus.examples",
            "learning.interventionSignals"
        ],
        "releasePolicy": [
            "setup catalog entries describe tooling, fixture, datum, workholding, monitoring, recovery, and operator or automation evidence contracts, not certified fixture designs or safety procedures",
            "machine-ready release remains blocked while required tools, workholding, setup checks, fixture evidence, datum transfer, monitoring channels, alert rules, recovery actions, or signoff gates are unresolved",
            "setup, fixture, and monitoring observations are retained for MDP/POMDP/neural workers so future planning can learn when to change workholding, split setups, add automation, or require human intervention"
        ],
        "schemas": [
            "dd.fabrication.tooling-plan.v1",
            "dd.fabrication.fixture-plan.v1",
            "dd.fabrication.monitoring-plan.v1"
        ],
        "setupContracts": contracts
    })
}
