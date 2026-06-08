use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response() -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.execution-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /execution/preflight/catalog",
            "GET /fabrication/execution/preflight/catalog"
        ],
        "executionPlanRoutes": ["POST /execution/plan", "POST /fabrication/execution/plan"],
        "executionResultRoutes": ["POST /execution/result", "POST /fabrication/execution/result"],
        "relatedRoutes": [
            "GET /fabrication/machine-code/preflight/catalog",
            "GET /fabrication/release/preflight/catalog",
            "GET /fabrication/monitoring/catalog",
            "GET /fabrication/interventions/catalog",
            "GET /fabrication/setup/catalog",
            "GET /fabrication/learning/preflight/catalog",
            "POST /fabrication/release/preview",
            "POST /fabrication/learning/outcomes"
        ],
        "preflightGroups": [
            {
                "group": "program-run-and-machine-state",
                "requiredEvidence": [
                    "generated or imported machine program, controller dialect, postprocessor checksum, tool/nozzle/spindle state, and machine profile are retained",
                    "executionPlan.programRuns map every generated printer, mill, router, lathe, sheet-cut, EDM, assembly, and special-process program to a reviewed machine and setup",
                    "machineSchedule dependency holds, setup order, and process handoff boundaries are visible before any run can start"
                ],
                "blocks": ["executionPlan.programRuns", "machineSchedule.dependencyHolds", "controllerPlan.releaseGates"]
            },
            {
                "group": "stop-point-human-intervention-and-automation-state",
                "requiredEvidence": [
                    "executionPlan.stopPoints and checkpoints identify manual setup, inspection, material change, part combine/separate, or recovery boundaries",
                    "operatorInterventionPlan.requiredOperatorActions, evidenceGates, automationCandidates, and splitCombineReviews have owners and retained acceptance evidence",
                    "interventionMap human and automation paths describe when the job must pause rather than pretending unattended completion is possible"
                ],
                "blocks": ["executionPlan.stopPoints", "operatorInterventionPlan.requiredOperatorActions", "interventionMap.humanInterventionPoints"]
            },
            {
                "group": "monitoring-recovery-and-release-state",
                "requiredEvidence": [
                    "monitoringPlan monitor points, alert rules, recovery actions, and unattended-run release gates cover thermal, motion, extrusion, spindle, coolant, fixturing, and process-specific failure modes",
                    "simulation, validation, quality, release package, and machineRelease blockers have been reviewed before execution is declared machine-ready",
                    "execution result, monitoring result, and learning outcome routes are linked so failed runs feed MDP/POMDP/neural policy updates"
                ],
                "blocks": ["monitoringPlan.releaseGates", "machineRelease.blockers", "releasePackagePlan.releaseGates"]
            }
        ],
        "responseSurfaces": [
            "executionPlan.programRuns",
            "executionPlan.checkpoints",
            "executionPlan.stopPoints",
            "executionPlan.canStart",
            "executionPlan.canRunUnattended",
            "operatorInterventionPlan.requiredOperatorActions",
            "operatorInterventionPlan.evidenceGates",
            "operatorInterventionPlan.automationCandidates",
            "operatorInterventionPlan.splitCombineReviews",
            "interventionMap.humanInterventionPoints",
            "interventionMap.automationPaths",
            "machineSchedule.dependencyHolds",
            "monitoringPlan.monitorPoints",
            "monitoringPlan.alertRules",
            "monitoringPlan.recoveryActions",
            "machineRelease.blockers",
            "releasePackagePlan.releaseGates",
            "learning.interventionSignals"
        ],
        "artifactSurfaces": [
            "execution-plan",
            "operator-intervention-plan",
            "machine-schedule",
            "monitoring-plan",
            "machine-release",
            "release-package-plan",
            "simulation-report",
            "mdp-request.artifacts.executionPlan",
            "mdp-request.artifacts.operatorInterventionPlan",
            "mdp-request.artifacts.monitoringPlan"
        ],
        "learningSurfaces": [
            "executionPlan.learningObservations",
            "operatorInterventionPlan.learningObservations",
            "interventionMap.learningObservations",
            "machineSchedule.learningObservations",
            "monitoringPlan.learningObservations",
            "executionResult.learningOutcomeDraft",
            "learning.interventionSignals",
            "neuralTrainingCorpus.examples"
        ],
        "executionPolicy": [
            "execution preflight catalog entries describe conservative run-readiness evidence, not certified shop-floor authorization or controller safety",
            "machine-ready execution remains blocked while stop points, required operator actions, dependency holds, monitoring recovery gaps, split/combine evidence, release blockers, or learning handoff links are missing",
            "failed execution preflight gates should feed DES, MDP/POMDP, and neural workers so future plans can reroute, split jobs, regenerate instructions, or ask for human intervention earlier"
        ]
    })
}
