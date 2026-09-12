use serde_json::{json, Value};

use super::{
    unique_sorted, FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT,
    FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT, FABRICATION_RESULTS_SUBJECT,
    MDP_OPTIMIZE_SUBJECT, SCHEMA_VERSION, SERVICE_NAME,
};

pub(super) fn lanes() -> Vec<Value> {
    vec![
        json!({
            "lane": "source-design-conversion",
            "workerFamilies": ["native-cad-translator", "cloud-cad-exporter", "open-scripted-cad-evaluator", "lightweight-cad-pmi-inspector", "cad-kernel-inspector", "sheet-profile-cad-inspector", "slicer-profile-reviewer"],
            "sourceSurfaces": ["designInputReview.inputs", "designInputReview.conversionPlan"],
            "artifactSurfaces": ["design-input-review", "parametric-design.designInputReview", "mdp-request.artifacts.designInputReview"],
            "natsSubjects": {
                "requests": FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT,
                "results": FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT,
                "queueGroup": "dd-fabrication-design-conversion"
            },
            "requiredEvidence": ["source identity without secret URI parts", "translator or exporter version", "units/scale/topology/PMI review", "neutral export or manual review result"],
            "blocks": ["machine-ready release", "generated design export certification"]
        }),
        json!({
            "lane": "generated-design-and-cam-export",
            "workerFamilies": ["design-agent", "slicer", "mesh-review", "cam", "sheet-cam", "cam-setup-agent", "nesting-agent", "assembly-planner"],
            "sourceSurfaces": ["designPackage", "designExports", "processGraph", "manufacturingHandoff.parts"],
            "artifactSurfaces": ["design-package", "design-export-bundle", "generated-design-export", "generated-assembly-design-export", "manufacturing-handoff"],
            "requiredEvidence": ["generated export media type and source preview", "program/process-node links", "export blockers reviewed", "CAD/CAM/slicer regeneration result attached"],
            "blocks": ["machine-code release", "slicer release", "assembly/recomposition release"]
        }),
        json!({
            "lane": "machine-program-controller-release",
            "workerFamilies": ["postprocessor", "controller-reviewer", "dry-run-simulator", "operator-review"],
            "sourceSurfaces": ["generatedPrograms", "controllerPlan", "postprocessPlan", "releasePackagePlan.packages"],
            "artifactSurfaces": ["program-*", "controller-plan", "postprocess-plan", "release-package-plan"],
            "requiredEvidence": ["postprocessor identity and output format", "controller dialect checks", "dry-run or simulation evidence", "operator or automation signoff"],
            "blocks": ["controller transfer", "machine start", "unattended repeat run"]
        }),
        json!({
            "lane": "setup-quality-monitoring-release",
            "workerFamilies": ["fixture-planner", "tooling-reviewer", "quality-inspector", "monitoring-operator", "safe-stop-reviewer"],
            "sourceSurfaces": ["toolingPlan", "fixturePlan", "qualityPlan", "monitoringPlan", "machineRelease"],
            "artifactSurfaces": ["tooling-plan", "fixture-plan", "quality-plan", "monitoring-plan", "machine-release"],
            "requiredEvidence": ["tool/workholding/fixture proof", "inspection targets and records", "monitor channels and recovery actions", "machine-release blockers cleared"],
            "blocks": ["machine-ready release", "unattended release", "restart after stop"]
        }),
        json!({
            "lane": "hybrid-split-combine-assembly",
            "workerFamilies": ["decomposition-planner", "interface-control-reviewer", "assembly-planner", "robotic-cell-reviewer", "operator-review"],
            "sourceSurfaces": ["hybridMakePlan", "decompositionPlan", "interfaceControlPlan", "manufacturingHandoff.parts", "releasePackagePlan.packages"],
            "artifactSurfaces": ["hybrid-make-plan", "decomposition-plan", "interface-control-plan", "manufacturing-handoff", "release-package-plan"],
            "requiredEvidence": ["split target and recomposition route", "interface acceptance criteria", "datum transfer and mating-surface evidence", "assembly/recomposition release package"],
            "blocks": ["combine/recomposition release", "assembly handoff", "single-piece fallback release"]
        }),
        json!({
            "lane": "learning-policy-and-outcome-feedback",
            "workerFamilies": ["des-scheduler", "mdp-optimizer", "pomdp-probe-planner", "neural-policy-trainer", "outcome-learning-worker"],
            "sourceSurfaces": ["learning", "learningPolicySnapshot", "learningOutcomes", "learningCorpus", "pomdpBeliefState", "releaseProbePlan", "neuralTrainingCorpus", "mdp-request", "learning.outcomes"],
            "artifactSurfaces": ["learning-plan", "learning-policy-snapshot", "learning-outcome-memory", "learning-corpus", "pomdp-belief-state", "release-probe-plan", "neural-training-corpus", "mdp-request", "reward-signal", "mdp-experience", "neural-example"],
            "natsSubjects": {
                "mdpOptimize": MDP_OPTIMIZE_SUBJECT,
                "fabricationResults": FABRICATION_RESULTS_SUBJECT
            },
            "requiredEvidence": ["policy preview retained as advisory evidence", "probe requirements promoted into machineRelease", "outcome rewards and remediation risks recorded", "validation/simulation/operator gates still authoritative"],
            "blocks": ["learned preference promotion", "unreviewed retry after failed outcome"]
        }),
    ]
}

pub(super) fn response() -> Value {
    let lanes = lanes();
    let worker_families = unique_sorted(lanes.iter().flat_map(|lane| {
        lane.get("workerFamilies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let source_surfaces = unique_sorted(lanes.iter().flat_map(|lane| {
        lane.get("sourceSurfaces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
    }));

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.handoff-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /handoff/catalog", "GET /fabrication/handoff/catalog"],
        "handoffLaneCount": lanes.len(),
        "workerFamilies": worker_families,
        "sourceSurfaces": source_surfaces,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "instructionAnalysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "jobInspectionRoutes": [
            "GET /jobs",
            "GET /fabrication/jobs",
            "GET /jobs/:job_id",
            "GET /fabrication/jobs/:job_id",
            "GET /jobs/:job_id/artifacts/:artifact_id",
            "GET /fabrication/jobs/:job_id/artifacts/:artifact_id"
        ],
        "discoveryRoutes": [
            "GET /design/formats",
            "GET /fabrication/design/formats",
            "GET /formats/catalog",
            "GET /fabrication/formats/catalog",
            "GET /design/import/catalog",
            "GET /fabrication/design/import/catalog",
            "GET /design/generation/catalog",
            "GET /fabrication/design/generation/catalog",
            "GET /instructions/generation/catalog",
            "GET /fabrication/instructions/generation/catalog",
            "GET /release/catalog",
            "GET /fabrication/release/catalog"
        ],
        "artifactSurfaces": [
            "design-package",
            "design-export-bundle",
            "generated-design-export",
            "manufacturing-handoff",
            "program-*",
            "controller-plan",
            "postprocess-plan",
            "release-package-plan",
            "tooling-plan",
            "fixture-plan",
            "quality-plan",
            "monitoring-plan",
            "interface-control-plan",
            "decomposition-plan",
            "mdp-request"
        ],
        "learningSurfaces": [
            "hybridMakePlan.learningObservations",
            "interfaceControlPlan.learningObservations",
            "decompositionPlan.learningObservations",
            "releasePackagePlan.learningObservations",
            "monitoringPlan.learningObservations",
            "neuralTrainingCorpus.examples",
            "learning.outcomes"
        ],
        "releasePolicy": [
            "handoff catalog lanes describe downstream worker contracts, not certified CAD, CAM, controller, fixture, inspection, or safety-system output",
            "machine-ready release remains blocked while conversion, export, controller, setup, monitoring, split/combine, release-package, or learned-remediation evidence is unresolved",
            "handoff lanes preserve response and artifact surfaces so MDP/POMDP/neural workers can learn which design, machine-code, setup, monitoring, or assembly evidence cleared or blocked prior work"
        ],
        "handoffLanes": lanes
    })
}
