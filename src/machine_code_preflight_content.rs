use serde_json::{json, Value};

use super::{unique_sorted, SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(program_contracts: Vec<Value>, controller_targets: Vec<Value>) -> Value {
    let generated_languages = unique_sorted(program_contracts.iter().flat_map(|contract| {
        contract
            .get("generatedLanguages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let machine_classes = unique_sorted(program_contracts.iter().flat_map(|contract| {
        contract
            .get("machineClasses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let output_formats = unique_sorted(controller_targets.iter().filter_map(|target| {
        target
            .get("outputFormat")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.machine-code-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /machine-code/preflight/catalog",
            "GET /fabrication/machine-code/preflight/catalog"
        ],
        "machineCodeCatalogRoutes": ["GET /machine-code/catalog", "GET /fabrication/machine-code/catalog"],
        "generationRoutes": ["POST /machine-code/generate", "POST /fabrication/machine-code/generate"],
        "resultRoutes": ["POST /machine-code/result", "POST /fabrication/machine-code/result"],
        "relatedRoutes": [
            "GET /fabrication/instructions/generation/preflight/catalog",
            "GET /fabrication/instructions/import/preflight/catalog",
            "GET /fabrication/controllers/preflight/catalog",
            "GET /fabrication/toolpaths/catalog",
            "GET /fabrication/simulation/preflight/catalog",
            "GET /fabrication/release/preflight/catalog",
            "GET /fabrication/learning/preflight/catalog"
        ],
        "generatedLanguageCount": generated_languages.len(),
        "generatedLanguages": generated_languages,
        "machineClassCount": machine_classes.len(),
        "machineClasses": machine_classes,
        "outputFormatCount": output_formats.len(),
        "outputFormats": output_formats,
        "preflightGroups": [
            {
                "group": "program-source-and-design-state",
                "evidence": [
                    "design package, imported instruction stream, or generated-program request is retained",
                    "units, coordinate system, part split/combine context, and source revision are declared",
                    "draft generatedPrograms entries remain traceable to designInputReview or instructionImportReview"
                ],
                "blocks": ["machine-code generation", "controller handoff", "releasePackagePlan.readyPackageCount"]
            },
            {
                "group": "controller-postprocessor-and-dialect-state",
                "evidence": [
                    "target controller, postprocessor, dialect family, output format, macro policy, and tool table are selected",
                    "controllerPlan.compatibilityTargets and controllerPlan.releaseGates have no unresolved blockers",
                    "postprocessed output checksum and source revision evidence are retained before release"
                ],
                "blocks": ["controllerPlan.releaseGates", "machineRelease.generatedProgramsBlocked"]
            },
            {
                "group": "machine-setup-toolpath-and-process-state",
                "evidence": [
                    "machine profile, workholding, tooling, material/feedstock, support media, offsets, calibration, and setup evidence are current",
                    "toolpathPlan.segments and executionPlan.programRuns identify setup changes and human checkpoints",
                    "printer thermal/extrusion state or CNC spindle/feed/coolant/support-process state is reviewable before execution"
                ],
                "blocks": ["toolpathPlan.releaseGates", "executionPlan.stopPoints", "operatorInterventionPlan.requiredOperatorActions"]
            },
            {
                "group": "validation-simulation-release-and-learning-state",
                "evidence": [
                    "validation findings, failure boundaries, dry-run or simulation results, and quality gates are retained",
                    "releasePackagePlan.requiredArtifacts includes generated code, controller checks, setup evidence, simulation, and signoff artifacts",
                    "DES, MDP/POMDP, reward, neural, and learning outcome records are linked without bypassing release gates"
                ],
                "blocks": ["machineReady release", "releasePackagePlan.releaseGates", "learning.promotion"]
            }
        ],
        "responseSurfaces": [
            "generatedPrograms",
            "generatedPrograms.instructions",
            "generatedPrograms.draft",
            "generatedPrograms.machineReady",
            "controllerPlan.compatibilityTargets",
            "controllerPlan.releaseGates",
            "postprocessPlan.controllerTargets",
            "toolpathPlan.segments",
            "simulation.programs",
            "validation.failureBoundaries",
            "executionPlan.programRuns",
            "operatorInterventionPlan.requiredOperatorActions",
            "machineRelease.generatedProgramsBlocked",
            "releasePackagePlan.requiredArtifacts",
            "learning.outcomeDraft"
        ],
        "artifactSurfaces": [
            "generated-machine-program",
            "controller-plan",
            "postprocess-plan",
            "toolpath-plan",
            "simulation-report",
            "quality-plan",
            "release-package-plan",
            "mdp-request.artifacts.generatedPrograms"
        ],
        "releasePolicy": [
            "machine-code preflight entries describe evidence required before generated or imported controller output can be trusted for release review; they do not certify machine execution",
            "generatedPrograms remain draft=true and machineReady=false until design provenance, controller/postprocessor compatibility, machine setup, validation, simulation or dry-run, quality, release package, and signoff evidence clear",
            "failed machine-code preflight checks feed DES, MDP/POMDP, reward, neural, and learning-outcome workers so future plans can regenerate code, choose alternate machines, split/combine parts, or add human checkpoints"
        ]
    })
}
