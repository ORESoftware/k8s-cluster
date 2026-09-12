use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    gate_contracts: Vec<Value>,
    gate_types: Vec<String>,
    release_blocking_gate_count: usize,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.release-gate-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /release/gates/catalog",
            "GET /fabrication/release/gates/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/release/catalog",
            "GET /fabrication/release/preflight/catalog",
            "POST /fabrication/release/preview",
            "POST /fabrication/release/result",
            "GET /fabrication/jobs/:job_id/release-bundle"
        ],
        "gateCount": gate_contracts.len(),
        "releaseBlockingGateCount": release_blocking_gate_count,
        "gateTypes": gate_types,
        "machineReadyRule": "machineReady remains false until every release-blocking gate has retained evidence, cleared blockers, and operator or automation signoff when required",
        "evidenceSurfaceFamilies": [
            "source-provenance",
            "design-export-review",
            "machine-envelope",
            "controller-postprocess-compatibility",
            "setup-quality-monitoring-evidence",
            "split-combine-interface-release",
            "simulation-or-dry-run-evidence",
            "learning-disposition"
        ],
        "responseSurfaces": [
            "releasePackagePlan.releaseGates",
            "releasePackagePlan.requiredArtifacts",
            "machineRelease.status",
            "machineRelease.blockers",
            "machineRelease.checklist",
            "controllerPlan.releaseGates",
            "postprocessPlan.blockers",
            "simulation.riskProfile",
            "decompositionPlan.releaseGates",
            "interfaceControlPlan.releaseGates",
            "priorityDispositions"
        ],
        "learningSurfaces": [
            "releasePackagePlan.learningObservations",
            "releaseProbePlan.probes",
            "releaseReadinessResult.learning.outcomeDraft",
            "pomdpBeliefState.hiddenStates",
            "neuralTrainingCorpus.examples",
            "mdp-request.artifacts.releasePackagePlan"
        ],
        "releasePolicy": [
            "release gate catalog entries expose machine-ready blockers for UIs, workers, and reviewers; they do not certify equipment safety",
            "generated designs, machine code, slicer jobs, imported CNC/controller streams, text job sheets, split routes, and recomposed assemblies remain advisory until retained gate evidence clears",
            "gate outcomes and priority dispositions are learning signals for MDP/POMDP/DES/neural workers, but learned policy never bypasses release gates"
        ],
        "gateContracts": gate_contracts
    })
}
