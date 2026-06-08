pub(super) fn machine_classes() -> Vec<&'static str> {
    vec![
        "fdm-printer",
        "pellet-fgf-printer",
        "robotic-additive-cell",
        "sheet-lamination-printer",
        "sla-msla-resin-printer",
        "material-jetting-printer",
        "directed-energy-deposition-cell",
        "continuous-fiber-composite-printer",
        "binder-jet-printer",
        "sls-mjf-powder-bed-printer",
        "metal-pbf-printer",
        "dmls-slm-lpbf-metal-powder-bed-printer",
        "vertical-mill",
        "five-axis-mill",
        "rotary-indexer-mill",
        "horizontal-mill",
        "cnc-router",
        "laser-sheet-cutter",
        "waterjet-sheet-cutter",
        "plasma-sheet-cutter",
        "wire-edm-sheet-cutter",
        "sinker-edm-cell",
        "precision-grinder",
        "cmm-inspection-cell",
        "thermal-postprocess-furnace",
        "surface-finishing-cell",
        "metal-joining-cell",
        "molding-casting-cell",
        "composite-layup-cell",
        "hot-wire-foam-cutter",
        "press-brake-forming-cell",
        "mill-turn-center",
        "swiss-turning-center",
        "lathe",
        "robotic-assembly-cell",
        "manual-or-special-process",
    ]
}

pub(super) fn generated_artifacts() -> Vec<&'static str> {
    vec![
        "design-summary",
        "parametric-design",
        "design-package",
        "design-export-bundle",
        "design-input-review",
        "generated-design-export",
        "process-plan",
        "production-plan",
        "machine-schedule",
        "machine-selection",
        "machine-profile-evidence",
        "process-graph",
        "manufacturing-handoff",
        "quality-plan",
        "tooling-plan",
        "machine-release",
        "execution-plan",
        "postprocess-plan",
        "intervention-map",
        "pomdp-belief-state",
        "release-probe-plan",
        "neural-training-corpus",
        "learning-policy-snapshot",
        "learning-outcome-memory",
        "learning-corpus",
        "mdp-request",
    ]
}

pub(super) fn learning_channels() -> Vec<&'static str> {
    vec![
        "mdp-states-actions-rewards",
        "pomdp-belief-state-and-probes",
        "release-probe-plan",
        "neural-policy-sketch",
        "neural-training-corpus",
        "fabrication-outcome-rewards",
        "compact-learning-outcomes",
        "des-engine-capability-catalog",
        "material-method-remediation-risks",
    ]
}

pub(super) fn strategy_quality_surfaces() -> Vec<&'static str> {
    vec![
        "policySummary.successRate",
        "policySummary.failureRate",
        "policySummary.learnedQuality",
        "learningOutcomeQuality.riskReviewRequired",
        "learningOutcomeQuality.releasePolicy",
    ]
}

pub(super) fn discovery_contracts() -> Vec<&'static str> {
    vec![
        "GET /capabilities",
        "GET /fabrication/capabilities",
        "GET /objective/coverage",
        "GET /fabrication/objective/coverage",
        "GET /machines/catalog",
        "GET /fabrication/machines/catalog",
        "POST /machines/select",
        "POST /fabrication/machines/select",
        "GET /printers/catalog",
        "GET /fabrication/printers/catalog",
        "GET /subtractive/catalog",
        "GET /fabrication/subtractive/catalog",
        "GET /cnc/catalog",
        "GET /fabrication/cnc/catalog",
        "GET /cells/catalog",
        "GET /fabrication/cells/catalog",
        "GET /hybrid/catalog",
        "GET /fabrication/hybrid/catalog",
        "GET /methods/catalog",
        "GET /fabrication/methods/catalog",
        "GET /controllers/catalog",
        "GET /fabrication/controllers/catalog",
        "POST /controllers/result",
        "POST /fabrication/controllers/result",
        "GET /materials/catalog",
        "GET /fabrication/materials/catalog",
        "POST /materials/plan",
        "POST /fabrication/materials/plan",
        "POST /materials/result",
        "POST /fabrication/materials/result",
        "GET /design/formats",
        "GET /fabrication/design/formats",
        "GET /slicers/catalog",
        "GET /fabrication/slicers/catalog",
        "POST /slicers/result",
        "POST /fabrication/slicers/result",
        "GET /mesh-repair/catalog",
        "GET /fabrication/mesh-repair/catalog",
        "POST /mesh-repair/result",
        "POST /fabrication/mesh-repair/result",
        "GET /design/import/catalog",
        "GET /fabrication/design/import/catalog",
        "GET /subjects/catalog",
        "GET /fabrication/subjects/catalog",
        "GET /workers/catalog",
        "GET /fabrication/workers/catalog",
        "GET /results/catalog",
        "GET /fabrication/results/catalog",
        "GET /design/generation/catalog",
        "GET /fabrication/design/generation/catalog",
        "POST /design/synthesis/result",
        "POST /fabrication/design/synthesis/result",
        "POST /design/generate",
        "POST /fabrication/design/generate",
        "POST /design/import/review",
        "POST /fabrication/design/import/review",
        "POST /design/import/result",
        "POST /fabrication/design/import/result",
        "POST /design/convert/plan",
        "POST /fabrication/design/convert/plan",
        "POST /design/convert/result",
        "POST /fabrication/design/convert/result",
        "GET /handoff/catalog",
        "GET /fabrication/handoff/catalog",
        "GET /instructions/languages",
        "GET /fabrication/instructions/languages",
        "GET /instructions/generation/catalog",
        "GET /fabrication/instructions/generation/catalog",
        "POST /instructions/generate",
        "POST /fabrication/instructions/generate",
        "POST /instructions/generation/result",
        "POST /fabrication/instructions/generation/result",
        "POST /instructions/review/result",
        "POST /fabrication/instructions/review/result",
        "POST /instructions/validation/result",
        "POST /fabrication/instructions/validation/result",
        "GET /machine-code/catalog",
        "GET /fabrication/machine-code/catalog",
        "POST /machine-code/generate",
        "POST /fabrication/machine-code/generate",
        "POST /machine-code/result",
        "POST /fabrication/machine-code/result",
        "GET /toolpaths/catalog",
        "GET /fabrication/toolpaths/catalog",
        "POST /toolpaths/plan",
        "POST /fabrication/toolpaths/plan",
        "POST /toolpaths/result",
        "POST /fabrication/toolpaths/result",
        "GET /improvements/catalog",
        "GET /fabrication/improvements/catalog",
        "GET /boundaries/catalog",
        "GET /fabrication/boundaries/catalog",
        "GET /remediation/catalog",
        "GET /fabrication/remediation/catalog",
        "POST /remediation/plan",
        "POST /fabrication/remediation/plan",
        "GET /decomposition/catalog",
        "GET /fabrication/decomposition/catalog",
        "POST /decomposition/plan",
        "POST /fabrication/decomposition/plan",
        "POST /decomposition/result",
        "POST /fabrication/decomposition/result",
        "GET /assembly/catalog",
        "GET /fabrication/assembly/catalog",
        "POST /assembly/plan",
        "POST /fabrication/assembly/plan",
        "POST /assembly/result",
        "POST /fabrication/assembly/result",
        "GET /release/catalog",
        "GET /fabrication/release/catalog",
        "POST /release/result",
        "POST /fabrication/release/result",
        "GET /workflow/catalog",
        "GET /fabrication/workflow/catalog",
        "POST /workflow/plan",
        "POST /fabrication/workflow/plan",
        "GET /strategy/catalog",
        "GET /fabrication/strategy/catalog",
        "POST /strategy/recommend",
        "POST /fabrication/strategy/recommend",
        "POST /strategy/result",
        "POST /fabrication/strategy/result",
        "GET /schedule/catalog",
        "GET /fabrication/schedule/catalog",
        "POST /schedule/result",
        "POST /fabrication/schedule/result",
        "GET /simulation/catalog",
        "GET /fabrication/simulation/catalog",
        "POST /simulation/run",
        "POST /fabrication/simulation/run",
        "POST /simulation/result",
        "POST /fabrication/simulation/result",
        "GET /quality/catalog",
        "GET /fabrication/quality/catalog",
        "POST /quality/plan",
        "POST /fabrication/quality/plan",
        "POST /quality/result",
        "POST /fabrication/quality/result",
        "GET /calibration/catalog",
        "GET /fabrication/calibration/catalog",
        "POST /calibration/plan",
        "POST /fabrication/calibration/plan",
        "POST /calibration/result",
        "POST /fabrication/calibration/result",
        "GET /interventions/catalog",
        "GET /fabrication/interventions/catalog",
        "POST /interventions/result",
        "POST /fabrication/interventions/result",
        "GET /setup/catalog",
        "GET /fabrication/setup/catalog",
        "GET /tooling/catalog",
        "GET /fabrication/tooling/catalog",
        "GET /workholding/catalog",
        "GET /fabrication/workholding/catalog",
        "GET /nesting/catalog",
        "GET /fabrication/nesting/catalog",
        "POST /nesting/result",
        "POST /fabrication/nesting/result",
        "GET /support-strategies/catalog",
        "GET /fabrication/support-strategies/catalog",
        "POST /support-strategies/result",
        "POST /fabrication/support-strategies/result",
        "GET /process-recipes/catalog",
        "GET /fabrication/process-recipes/catalog",
        "POST /process-recipes/result",
        "POST /fabrication/process-recipes/result",
        "GET /kinematics/catalog",
        "GET /fabrication/kinematics/catalog",
        "POST /kinematics/result",
        "POST /fabrication/kinematics/result",
        "GET /tolerances/catalog",
        "GET /fabrication/tolerances/catalog",
        "GET /process-capabilities/catalog",
        "GET /fabrication/process-capabilities/catalog",
        "GET /manufacturability/catalog",
        "GET /fabrication/manufacturability/catalog",
        "POST /manufacturability/result",
        "POST /fabrication/manufacturability/result",
        "GET /failure-modes/catalog",
        "GET /fabrication/failure-modes/catalog",
        "POST /failure-modes/result",
        "POST /fabrication/failure-modes/result",
        "GET /safety/catalog",
        "GET /fabrication/safety/catalog",
        "GET /environment/catalog",
        "GET /fabrication/environment/catalog",
        "GET /provenance/catalog",
        "GET /fabrication/provenance/catalog",
        "GET /as-built/catalog",
        "GET /fabrication/as-built/catalog",
        "POST /as-built/result",
        "POST /fabrication/as-built/result",
        "POST /setup/plan",
        "POST /fabrication/setup/plan",
        "POST /setup/result",
        "POST /fabrication/setup/result",
        "GET /monitoring/catalog",
        "GET /fabrication/monitoring/catalog",
        "POST /monitoring/plan",
        "POST /fabrication/monitoring/plan",
        "POST /monitoring/result",
        "POST /fabrication/monitoring/result",
        "GET /postprocess/catalog",
        "GET /fabrication/postprocess/catalog",
        "POST /postprocess/plan",
        "POST /fabrication/postprocess/plan",
        "POST /postprocess/result",
        "POST /fabrication/postprocess/result",
        "GET /evidence/catalog",
        "GET /fabrication/evidence/catalog",
        "GET /artifacts/catalog",
        "GET /fabrication/artifacts/catalog",
        "GET /learning/capabilities",
        "GET /fabrication/learning/capabilities",
        "GET /learning/engines/catalog",
        "GET /fabrication/learning/engines/catalog",
        "GET /learning/models/catalog",
        "GET /fabrication/learning/models/catalog",
        "GET /learning/optimizers/catalog",
        "GET /fabrication/learning/optimizers/catalog",
        "POST /learning/models/result",
        "POST /fabrication/learning/models/result",
        "POST /learning/optimizers/result",
        "POST /fabrication/learning/optimizers/result",
        "GET /schema",
        "GET /fabrication/schema",
        "GET /examples",
        "GET /fabrication/examples",
    ]
}

pub(super) fn planning_contracts() -> Vec<&'static str> {
    vec![
        "POST /plan",
        "POST /fabrication/plan",
        "POST /release/preview",
        "POST /fabrication/release/preview",
        "POST /strategy/recommend",
        "POST /fabrication/strategy/recommend",
        "POST /strategy/result",
        "POST /fabrication/strategy/result",
    ]
}

pub(super) fn instruction_generation_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/generate",
        "POST /fabrication/instructions/generate",
        "POST /instructions/generation/result",
        "POST /fabrication/instructions/generation/result",
    ]
}

pub(super) fn toolpath_contracts() -> Vec<&'static str> {
    vec![
        "GET /machine-code/catalog",
        "GET /fabrication/machine-code/catalog",
        "POST /machine-code/generate",
        "POST /fabrication/machine-code/generate",
        "POST /machine-code/result",
        "POST /fabrication/machine-code/result",
        "POST /toolpaths/plan",
        "POST /fabrication/toolpaths/plan",
        "POST /toolpaths/result",
        "POST /fabrication/toolpaths/result",
    ]
}

pub(super) fn material_contracts() -> Vec<&'static str> {
    vec![
        "POST /materials/plan",
        "POST /fabrication/materials/plan",
        "POST /materials/result",
        "POST /fabrication/materials/result",
    ]
}

pub(super) fn schedule_contracts() -> Vec<&'static str> {
    vec![
        "GET /schedule/catalog",
        "GET /fabrication/schedule/catalog",
        "POST /schedule/result",
        "POST /fabrication/schedule/result",
    ]
}

pub(super) fn decomposition_contracts() -> Vec<&'static str> {
    vec![
        "GET /decomposition/catalog",
        "GET /fabrication/decomposition/catalog",
        "POST /decomposition/plan",
        "POST /fabrication/decomposition/plan",
        "POST /decomposition/result",
        "POST /fabrication/decomposition/result",
    ]
}

pub(super) fn instruction_analysis_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/analyze",
        "POST /fabrication/instructions/analyze",
    ]
}

pub(super) fn instruction_validation_catalog_contracts() -> Vec<&'static str> {
    vec![
        "GET /instructions/validation/catalog",
        "GET /fabrication/instructions/validation/catalog",
    ]
}

pub(super) fn instruction_validation_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/validate",
        "POST /fabrication/instructions/validate",
    ]
}

pub(super) fn instruction_validation_result_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/validation/result",
        "POST /fabrication/instructions/validation/result",
    ]
}

pub(super) fn instruction_improvement_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/improve",
        "POST /fabrication/instructions/improve",
    ]
}

pub(super) fn instruction_boundary_review_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/boundaries/review",
        "POST /fabrication/instructions/boundaries/review",
    ]
}

pub(super) fn boundary_remediation_plan_contracts() -> Vec<&'static str> {
    vec![
        "POST /remediation/plan",
        "POST /fabrication/remediation/plan",
    ]
}

pub(super) fn boundary_remediation_result_contracts() -> Vec<&'static str> {
    vec![
        "POST /remediation/result",
        "POST /fabrication/remediation/result",
    ]
}

pub(super) fn instruction_review_result_contracts() -> Vec<&'static str> {
    vec![
        "POST /instructions/review/result",
        "POST /fabrication/instructions/review/result",
    ]
}

pub(super) fn instruction_simulation_contracts() -> Vec<&'static str> {
    vec![
        "POST /simulation/run",
        "POST /fabrication/simulation/run",
        "POST /simulation/result",
        "POST /fabrication/simulation/result",
    ]
}

pub(super) fn quality_contracts() -> Vec<&'static str> {
    vec![
        "POST /quality/plan",
        "POST /fabrication/quality/plan",
        "POST /quality/result",
        "POST /fabrication/quality/result",
    ]
}

pub(super) fn calibration_contracts() -> Vec<&'static str> {
    vec![
        "POST /calibration/plan",
        "POST /fabrication/calibration/plan",
        "POST /calibration/result",
        "POST /fabrication/calibration/result",
    ]
}

pub(super) fn setup_contracts() -> Vec<&'static str> {
    vec![
        "POST /setup/plan",
        "POST /fabrication/setup/plan",
        "POST /setup/result",
        "POST /fabrication/setup/result",
    ]
}

pub(super) fn monitoring_contracts() -> Vec<&'static str> {
    vec![
        "GET /monitoring/catalog",
        "GET /fabrication/monitoring/catalog",
        "POST /monitoring/plan",
        "POST /fabrication/monitoring/plan",
        "POST /monitoring/result",
        "POST /fabrication/monitoring/result",
    ]
}

pub(super) fn postprocess_contracts() -> Vec<&'static str> {
    vec![
        "GET /postprocess/catalog",
        "GET /fabrication/postprocess/catalog",
        "POST /postprocess/plan",
        "POST /fabrication/postprocess/plan",
        "POST /postprocess/result",
        "POST /fabrication/postprocess/result",
    ]
}

pub(super) fn assembly_planning_contracts() -> Vec<&'static str> {
    vec![
        "POST /assembly/plan",
        "POST /fabrication/assembly/plan",
        "POST /assembly/result",
        "POST /fabrication/assembly/result",
    ]
}

pub(super) fn release_readiness_result_contracts() -> Vec<&'static str> {
    vec!["POST /release/result", "POST /fabrication/release/result"]
}

pub(super) fn execution_contracts() -> Vec<&'static str> {
    vec![
        "POST /execution/plan",
        "POST /fabrication/execution/plan",
        "POST /execution/result",
        "POST /fabrication/execution/result",
    ]
}

pub(super) fn learning_contracts() -> Vec<&'static str> {
    vec![
        "POST /learning/observe",
        "POST /fabrication/learning/observe",
        "POST /learning/outcomes",
        "POST /fabrication/learning/outcomes",
        "GET /learning/outcomes",
        "GET /fabrication/learning/outcomes",
        "GET /learning/capabilities",
        "GET /fabrication/learning/capabilities",
        "GET /learning/rewards/catalog",
        "GET /fabrication/learning/rewards/catalog",
        "GET /learning/corpus",
        "GET /fabrication/learning/corpus",
        "GET /learning/policy",
        "GET /fabrication/learning/policy",
    ]
}

pub(super) fn inspection_contracts() -> Vec<&'static str> {
    vec![
        "GET /jobs",
        "GET /fabrication/jobs",
        "GET /jobs/:job_id",
        "GET /fabrication/jobs/:job_id",
        "GET /jobs/:job_id/artifacts/:artifact_id",
        "GET /fabrication/jobs/:job_id/artifacts/:artifact_id",
    ]
}

pub(super) fn notes() -> Vec<&'static str> {
    vec![
        "Capabilities describe draft planning and validation support, not controller-certified machine release.",
        "Clients may submit their own machine profiles; defaultMachines are the built-in fallback fleet used when no fleet is supplied.",
        "Generated programs and improved programs remain machineReady=false until downstream CAD/CAM, slicer, simulation, workholding, and operator review are complete.",
    ]
}
