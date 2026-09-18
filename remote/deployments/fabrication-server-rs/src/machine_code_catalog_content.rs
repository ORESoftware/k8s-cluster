use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{unique_sorted, SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    program_contracts: Vec<Value>,
    controller_targets: Vec<Value>,
    dialect_counts: BTreeMap<String, usize>,
    target_selection_matrix: Vec<Value>,
) -> Value {
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
    let dialect_families = unique_sorted(controller_targets.iter().filter_map(|target| {
        target
            .get("dialectFamily")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let postprocessors = unique_sorted(controller_targets.iter().filter_map(|target| {
        target
            .get("postprocessor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let generated_language_count = generated_languages.len();
    let machine_class_count = machine_classes.len();
    let output_format_count = output_formats.len();
    let dialect_family_count = dialect_families.len();
    let postprocessor_count = postprocessors.len();
    let known_postprocessor_count = controller_targets
        .iter()
        .filter(|target| {
            target
                .get("postprocessorKnown")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.machine-code-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /machine-code/catalog", "GET /fabrication/machine-code/catalog"],
        "generationRoutes": ["POST /machine-code/generate", "POST /fabrication/machine-code/generate"],
        "resultRoutes": ["POST /machine-code/result", "POST /fabrication/machine-code/result"],
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "relatedRoutes": [
            "GET /instructions/generation/catalog",
            "GET /fabrication/instructions/generation/catalog",
            "GET /controllers/catalog",
            "GET /fabrication/controllers/catalog",
            "POST /toolpaths/plan",
            "POST /fabrication/toolpaths/plan",
            "GET /simulation/catalog",
            "GET /fabrication/simulation/catalog",
            "GET /setup/catalog",
            "GET /fabrication/setup/catalog",
            "GET /release/catalog",
            "GET /fabrication/release/catalog"
        ],
        "programContractCount": program_contracts.len(),
        "controllerTargetCount": controller_targets.len(),
        "knownPostprocessorCount": known_postprocessor_count,
        "generatedLanguages": generated_languages,
        "generatedLanguageCount": generated_language_count,
        "machineClasses": machine_classes,
        "machineClassCount": machine_class_count,
        "outputFormats": output_formats,
        "outputFormatCount": output_format_count,
        "dialectFamilies": dialect_families,
        "dialectFamilyCount": dialect_family_count,
        "postprocessors": postprocessors,
        "postprocessorCount": postprocessor_count,
        "dialectCounts": dialect_counts,
        "responseSurfaces": [
            "generatedPrograms",
            "generatedPrograms.instructions",
            "generatedPrograms.language",
            "generatedPrograms.machineKind",
            "generatedPrograms.draft",
            "generatedPrograms.machineReady",
            "controllerPlan.compatibilityTargets",
            "controllerPlan.dialectSummaries",
            "controllerPlan.releaseGates",
            "postprocessPlan.controllerTargets",
            "toolpathPlan.segments",
            "simulation.programs",
            "executionPlan.programRuns",
            "machineRelease.generatedProgramsBlocked",
            "releasePackagePlan.requiredArtifacts",
            "learning.neuralTrainingCorpus"
        ],
        "artifactSurfaces": [
            "generated-machine-program",
            "program-*",
            "controller-plan",
            "postprocess-plan",
            "toolpath-plan",
            "simulation-report",
            "execution-plan",
            "release-package-plan",
            "mdp-request.artifacts.generatedPrograms"
        ],
        "releaseEvidence": [
            "controller dialect and postprocessor selection",
            "machine profile, material, workholding, setup, and calibration evidence",
            "simulation, dry-run, or equivalent controller verification",
            "postprocessed output checksum and source revision",
            "operator or automation signoff before machine-ready release"
        ],
        "learningSurfaces": [
            "program-generation:*",
            "controller-release:*",
            "simulation-risk:*",
            "release-probe:*",
            "neuralTrainingCorpus.examples",
            "learning.outcomes"
        ],
        "machineCodePolicy": [
            "machine-code catalog entries are discovery contracts for draft printer, mill, router, sheet-cutting, mill-turn, lathe, and special-process programs",
            "generated controller or printer programs remain draft=true and machineReady=false until validation, simulation or dry-run evidence, controller/postprocessor compatibility, setup, quality, release package, and signoff gates clear",
            "machine-code generation observations feed MDP/POMDP/neural workers so future plans can regenerate programs, choose alternate machines, split parts, combine assemblies, or add human checkpoints"
        ],
        "targetSelectionMatrix": target_selection_matrix,
        "programContracts": program_contracts,
        "controllerTargets": controller_targets
    })
}
