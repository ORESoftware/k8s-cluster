use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    risk_contracts: Vec<Value>,
    trace_contracts: Vec<Value>,
    dry_run_contracts: Vec<Value>,
    risk_types: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.simulation-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /simulation/catalog", "GET /fabrication/simulation/catalog"],
        "riskContractCount": risk_contracts.len(),
        "traceContractCount": trace_contracts.len(),
        "dryRunContractCount": dry_run_contracts.len(),
        "riskTypes": risk_types,
        "riskStatuses": [
            "simulation-risk-low",
            "simulation-risk-review-required",
            "simulation-risk-blocked"
        ],
        "simulationInputs": [
            "generatedPrograms",
            "existingInstructions",
            "machineProfiles",
            "machineProfileEvidence",
            "postprocessor/controller evidence"
        ],
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "responseSurfaces": [
            "simulation.programs",
            "simulation.programs.axisExtents",
            "simulation.programs.safeClearanceObserved",
            "simulation.programs.spindleOrHeatupObserved",
            "simulation.riskProfile",
            "simulation.riskProfile.programRisks",
            "simulation.riskProfile.learningObservations",
            "simulation.findings",
            "simulation.failureBoundaries",
            "validation.failureBoundaries",
            "machineRelease.blockers",
            "executionPlan.stopPoints",
            "releaseProbePlan.probes"
        ],
        "artifactSurfaces": [
            "simulation-report",
            "analysis-simulation-report",
            "mdp-request.artifacts.simulation",
            "release-package-plan.requiredArtifacts",
            "rotary-clearance-simulation-report",
            "robot-path-or-fixture-simulation-report"
        ],
        "learningSurfaces": [
            "simulation.riskProfile.learningObservations",
            "simulation.riskProfile.programRisks.learningObservations",
            "learning.interventionSignals",
            "neuralTrainingCorpus.examples",
            "mdp-request.artifacts.releaseProbePlan"
        ],
        "releasePolicy": [
            "simulation catalog entries describe dry-run and risk evidence contracts, not certified machine safety",
            "machine-ready release remains blocked while simulation risk is blocked, envelope or clearance boundaries remain open, process-start proof is missing, or required dry-run artifacts are absent",
            "simulation-risk observations are emitted for MDP/POMDP/neural workers so future planning can learn when to reroute, split parts, add clearance, or require operator review"
        ],
        "riskContracts": risk_contracts,
        "traceContracts": trace_contracts,
        "dryRunContracts": dry_run_contracts
    })
}
