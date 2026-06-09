use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(contracts: Vec<Value>, families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.monitoring-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /monitoring/catalog", "GET /fabrication/monitoring/catalog"],
        "monitoringContractCount": contracts.len(),
        "families": families,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "resultRoutes": ["POST /monitoring/result", "POST /fabrication/monitoring/result"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "monitoringPlan",
            "monitoringPlan.monitorPoints",
            "monitoringPlan.monitorPoints.channels",
            "monitoringPlan.monitorPoints.expectedSignals",
            "monitoringPlan.monitorPoints.requiredEvidence",
            "monitoringPlan.alertRules",
            "monitoringPlan.recoveryActions",
            "monitoringPlan.releaseGates",
            "monitoringResult.channels",
            "monitoringResult.alerts",
            "monitoringResult.recoveryActions",
            "monitoringResult.operatorInterventions",
            "machineRelease.blockers",
            "operatorInterventionPlan.requiredOperatorActions",
            "validation.failureBoundaries",
            "releaseProbePlan.probes"
        ],
        "artifactSurfaces": [
            "monitoring-plan",
            "monitoring-result",
            "monitoring-alerts",
            "monitoring-recovery-actions",
            "monitoring-operator-interventions",
            "parametric-design.monitoringPlan",
            "mdp-request.artifacts.monitoringPlan",
            "mdp-request.artifacts.monitoringResult",
            "analysis-mdp-request.artifacts.monitoringPlan"
        ],
        "learningSurfaces": [
            "monitoringPlan.learningObservations",
            "monitoringResult.learning.observations",
            "learning.interventionSignals",
            "pomdpBeliefState.hiddenStates",
            "neuralTrainingCorpus.examples",
            "monitoring-route:*",
            "monitoring-alert:*",
            "monitoring-blockers:*",
            "monitoring-result:*",
            "monitoring-recovery:*",
            "verify-monitoring-plan-*",
            "clear-monitoring-blockers-*"
        ],
        "releasePolicy": [
            "monitoring catalog entries describe runtime evidence contracts, not certified safety systems or controller restart procedures",
            "machine-ready and unattended release remain blocked while monitor channels, alert rules, safe-stop behavior, recovery actions, or restart authority are unresolved",
            "monitoring and recovery observations are retained for MDP/POMDP/neural workers so future planning can learn when to add sensors, split jobs, require operators, or improve generated instructions"
        ],
        "monitoringContracts": contracts
    })
}
