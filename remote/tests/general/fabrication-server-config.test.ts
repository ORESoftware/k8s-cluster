import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/fabrication-server-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust fabrication server exposes planning, analysis, nats, and learning hooks', async () => {
  const cargo = await readRepoFile('remote/deployments/fabrication-server-rs/Cargo.toml');
  const source = await readRepoFile('remote/deployments/fabrication-server-rs/src/main.rs');
  const readme = await readRepoFile('remote/deployments/fabrication-server-rs/readme.md');
  const subjectSchema = await readRepoFile(
    'remote/libs/nats/subject-defs/schema/fabrication.schema.json',
  );
  const docs = await readRepoFile(
    'remote/deployments/fabrication-server-rs/generated/api-docs.json',
  );

  assert.match(cargo, /name\s*=\s*"dd-fabrication-server"/);
  assert.match(cargo, /async-nats\s*=\s*"=0\.38\.0"/);
  assert.match(cargo, /dd-nats-subject-defs\s*=\s*\{\s*path/);
  assert.match(
    source,
    /use dd_nats_subject_defs::\{[\s\S]*?FABRICATION_REQUESTS_QUEUE_GROUP[\s\S]*?FABRICATION_REQUESTS_SUBJECT[\s\S]*?FABRICATION_RESULTS_SUBJECT[\s\S]*?MDP_OPTIMIZE_SUBJECT[\s\S]*?RUNTIME_EVENTS_SUBJECT[\s\S]*?\};/,
  );
  assert.match(source, /const SCHEMA_VERSION: &str = "fabrication\.plan\.v1"/);
  assert.match(source, /struct FabricationPlanRequest/);
  assert.match(source, /struct InstructionAnalysisRequest/);
  assert.match(source, /struct FabricationOutcomeRequest/);
  assert.match(source, /struct FabricationLearningResponse/);
  assert.match(source, /struct OutcomeRemediationPlan/);
  assert.match(source, /struct OutcomeRootCause/);
  assert.match(source, /struct OutcomeRemediationAction/);
  assert.match(source, /struct LearningOutcomeRequest/);
  assert.match(source, /struct LearningPolicySnapshot/);
  assert.match(source, /struct LearningRemediationRisk/);
  assert.match(source, /remediation_risks: Vec<LearningRemediationRisk>/);
  assert.match(source, /struct LearningPlan/);
  assert.match(source, /struct PomdpBeliefState/);
  assert.match(source, /struct PomdpHiddenStateBelief/);
  assert.match(source, /struct PomdpObservationLikelihood/);
  assert.match(source, /struct PomdpProbe/);
  assert.match(source, /struct StrategyCandidate/);
  assert.match(source, /struct InterventionLearningSignal/);
  assert.match(source, /struct ProcessGraph/);
  assert.match(source, /struct ProcessGraphDependency/);
  assert.match(source, /struct ProcessGraphGate/);
  assert.match(source, /struct BoundaryInterventionMap/);
  assert.match(source, /struct ProgramBoundaryTrace/);
  assert.match(source, /struct BoundaryHumanInterventionPoint/);
  assert.match(source, /struct BoundarySplitCombineDecision/);
  assert.match(source, /struct BoundaryAutomationPath/);
  assert.match(source, /struct MachineSelectionTrace/);
  assert.match(source, /struct MachineSelectionCandidate/);
  assert.match(source, /struct ManufacturingHandoff/);
  assert.match(source, /struct DesignPackage/);
  assert.match(source, /struct DesignPackagePart/);
  assert.match(source, /struct DesignAssemblyExport/);
  assert.match(source, /struct DesignExportTarget/);
  assert.match(source, /struct DesignExportBundle/);
  assert.match(source, /struct GeneratedDesignExport/);
  assert.match(source, /struct GeneratedAssemblyDesignExport/);
  assert.match(source, /struct DesignExportBundleSummary/);
  assert.match(source, /struct QualityPlan/);
  assert.match(source, /struct QualityInspectionPoint/);
  assert.match(source, /struct QualityMeasurementTarget/);
  assert.match(source, /struct ToolingPlan/);
  assert.match(source, /struct ToolingRequirement/);
  assert.match(source, /struct ProductionPlan/);
  assert.match(source, /struct ProductionBatch/);
  assert.match(source, /struct MachineSchedule/);
  assert.match(source, /struct MachineScheduleLane/);
  assert.match(source, /struct MachineScheduleOperation/);
  assert.match(source, /struct MachineScheduleHold/);
  assert.match(source, /struct MachineReleaseReport/);
  assert.match(source, /struct MachineReleaseBlocker/);
  assert.match(source, /struct MachineReleaseChecklistItem/);
  assert.match(source, /struct ExecutionReadinessPlan/);
  assert.match(source, /struct ExecutionProgramRun/);
  assert.match(source, /struct ExecutionCheckpoint/);
  assert.match(source, /struct ExecutionStopPoint/);
  assert.match(source, /struct PostprocessPlan/);
  assert.match(source, /struct PostprocessTarget/);
  assert.match(source, /struct PostprocessGate/);
  assert.match(source, /struct PostprocessBlocker/);
  assert.match(source, /struct AssemblyGraph/);
  assert.match(source, /struct AssemblyInterface/);
  assert.match(source, /struct AssemblySequenceStep/);
  assert.match(source, /struct NeuralPolicySketch/);
  assert.match(source, /struct NeuralTrainingCorpus/);
  assert.match(source, /struct NeuralTrainingExample/);
  assert.match(source, /struct NeuralInferenceCandidate/);
  assert.match(source, /fn neural_policy_sketch/);
  assert.match(source, /fn neural_training_corpus/);
  assert.match(source, /fn pomdp_belief_state/);
  assert.match(source, /fn strategy_candidates/);
  assert.match(source, /fn plan_fabrication\(request: FabricationPlanRequest\)/);
  assert.match(source, /fn plan_fabrication_with_policy/);
  assert.match(source, /fn apply_learning_policy_to_request/);
  assert.match(source, /fn learned_preferred_methods/);
  assert.match(source, /fn learned_preferred_assembly_strategy/);
  assert.match(source, /fn learned_remediation_risks/);
  assert.match(source, /learned-remediation-risk/);
  assert.match(source, /avoid-learned-risk/);
  assert.match(source, /preferred_assembly_strategy/);
  assert.match(source, /fn assembly_graph/);
  assert.match(source, /assembly_graph: AssemblyGraph/);
  assert.match(source, /fn process_graph/);
  assert.match(source, /process_graph: ProcessGraph/);
  assert.match(source, /fn intervention_map/);
  assert.match(source, /fn fallback_boundary_process_link/);
  assert.match(source, /intervention_map: BoundaryInterventionMap/);
  assert.match(source, /program_boundaries: Vec<ProgramBoundaryTrace>/);
  assert.match(source, /human_intervention_points: Vec<BoundaryHumanInterventionPoint>/);
  assert.match(source, /split_combine_decisions: Vec<BoundarySplitCombineDecision>/);
  assert.match(source, /automation_paths: Vec<BoundaryAutomationPath>/);
  assert.match(source, /fn machine_selection_trace/);
  assert.match(source, /machine_selection: Vec<MachineSelectionTrace>/);
  assert.match(source, /"machine-selection"/);
  assert.match(source, /"machineSelection": response\.machine_selection/);
  assert.match(source, /review-operation-gap/);
  assert.match(source, /viable-alternative/);
  assert.match(source, /fn manufacturing_handoff/);
  assert.match(source, /manufacturing_handoff: ManufacturingHandoff/);
  assert.match(source, /fn design_package/);
  assert.match(source, /design_package: DesignPackage/);
  assert.match(source, /fn design_export_bundle/);
  assert.match(source, /design_exports: DesignExportBundle/);
  assert.match(source, /fn design_export_content/);
  assert.match(source, /fn assembly_design_export_content/);
  assert.match(source, /fn quality_plan/);
  assert.match(source, /quality_plan: QualityPlan/);
  assert.match(source, /fn tooling_plan/);
  assert.match(source, /tooling_plan: ToolingPlan/);
  assert.match(source, /fn tooling_required_tools/);
  assert.match(source, /fn production_plan/);
  assert.match(source, /production_plan: ProductionPlan/);
  assert.match(source, /fn machine_schedule/);
  assert.match(source, /machine_schedule: MachineSchedule/);
  assert.match(source, /dependency_holds: Vec<MachineScheduleHold>/);
  assert.match(source, /fn machine_release_report/);
  assert.match(source, /machine_release: MachineReleaseReport/);
  assert.match(source, /fn execution_readiness_plan/);
  assert.match(source, /execution_plan: ExecutionReadinessPlan/);
  assert.match(source, /program_runs: Vec<ExecutionProgramRun>/);
  assert.match(source, /stop_points: Vec<ExecutionStopPoint>/);
  assert.match(source, /fn postprocess_plan/);
  assert.match(source, /postprocess_plan: PostprocessPlan/);
  assert.match(source, /controller_targets: Vec<PostprocessTarget>/);
  assert.match(source, /gates: Vec<PostprocessGate>/);
  assert.match(source, /required_artifacts: Vec<String>/);
  assert.match(source, /fn postprocessor_for/);
  assert.match(source, /"manufacturing-handoff"/);
  assert.match(source, /"design-package"/);
  assert.match(source, /"design-export-bundle"/);
  assert.match(source, /"generated-design-export"/);
  assert.match(source, /dd\.fabrication\.design-package\.v1/);
  assert.match(source, /dd\.fabrication\.design-export-bundle\.v1/);
  assert.match(source, /dd\.fabrication\.generated-design-export\.v1/);
  assert.match(source, /"quality-plan"/);
  assert.match(source, /dd\.fabrication\.quality-plan\.v1/);
  assert.match(source, /"tooling-plan"/);
  assert.match(source, /dd\.fabrication\.tooling-plan\.v1/);
  assert.match(source, /"production-plan"/);
  assert.match(source, /dd\.fabrication\.production-plan\.v1/);
  assert.match(source, /"machine-schedule"/);
  assert.match(source, /dd\.fabrication\.machine-schedule\.v1/);
  assert.match(source, /"machine-release"/);
  assert.match(source, /"analysis-machine-release"/);
  assert.match(source, /"execution-plan"/);
  assert.match(source, /"analysis-execution-plan"/);
  assert.match(source, /dd\.fabrication\.execution-plan\.v1/);
  assert.match(source, /"postprocess-plan"/);
  assert.match(source, /"analysis-postprocess-plan"/);
  assert.match(source, /dd\.fabrication\.postprocess-plan\.v1/);
  assert.match(source, /"pomdp-belief-state"/);
  assert.match(source, /dd\.fabrication\.pomdp-belief-state\.v1/);
  assert.match(source, /"neural-training-corpus"/);
  assert.match(source, /dd\.fabrication\.neural-training-corpus\.v1/);
  assert.match(source, /"intervention-map"/);
  assert.match(source, /"analysis-intervention-map"/);
  assert.match(source, /dd\.fabrication\.intervention-map\.v1/);
  assert.match(source, /"outcome-remediation-plan"/);
  assert.match(source, /dd\.fabrication\.outcome-remediation\.v1/);
  assert.match(source, /"productionPlan": response\.production_plan/);
  assert.match(source, /"machineSchedule": response\.machine_schedule/);
  assert.match(source, /"designPackage": response\.design_package/);
  assert.match(source, /"designExports": response\.design_exports/);
  assert.match(source, /"designInputReview": response\.design_input_review/);
  assert.match(source, /struct DesignInputFile/);
  assert.match(source, /struct DesignInputReview/);
  assert.match(source, /const DESIGN_FORMAT_SPECS/);
  assert.match(source, /ptc-creo-pro-engineer-native/);
  assert.match(source, /solidworks-native/);
  assert.match(source, /autodesk-fusion-native/);
  assert.match(source, /siemens-nx-native/);
  assert.match(source, /catia-native/);
  assert.match(source, /onshape-cloud-document/);
  assert.match(source, /freecad-native/);
  assert.match(source, /openscad-source/);
  assert.match(source, /blender-native/);
  assert.match(source, /zbrush-native/);
  assert.match(source, /design-input-review/);
  assert.match(source, /fn sanitize_design_source_uri/);
  assert.match(source, /fn token_parts_contain/);
  assert.match(source, /fn design_source_extension/);
  assert.match(source, /plan_reviews_professional_open_artistic_and_slicer_design_inputs/);
  assert.match(source, /design_input_review_hardens_ambiguous_extensions_and_redacts_uris/);
  assert.match(source, /"qualityPlan": response\.quality_plan/);
  assert.match(source, /"toolingPlan": response\.tooling_plan/);
  assert.match(source, /"interventionMap": response\.intervention_map/);
  assert.match(source, /"executionPlan": response\.execution_plan/);
  assert.match(source, /"postprocessPlan": response\.postprocess_plan/);
  assert.match(source, /"pomdpBeliefState": response\.learning\.pomdp_belief_state/);
  assert.match(source, /"neuralTrainingCorpus": response\.learning\.neural_training_corpus/);
  assert.match(source, /"machineRelease": response\.machine_release/);
  assert.match(source, /"manufacturingHandoff": response\.manufacturing_handoff/);
  assert.match(source, /assembly-interface/);
  assert.match(source, /gated-before-machine-release/);
  assert.match(source, /printed-pocket-turned-insert/);
  assert.match(source, /first-article-metrology-and-fit-check/);
  assert.match(source, /fn analyze_instruction_programs/);
  assert.match(source, /fn learn_from_outcome/);
  assert.match(source, /fn outcome_remediation_plan/);
  assert.match(source, /outcome_remediation: OutcomeRemediationPlan/);
  assert.match(source, /record_observations\.extend\(response\.outcome_remediation\.learning_signals\.clone\(\)\)/);
  assert.match(source, /fn learning_artifacts/);
  assert.match(source, /fn stored_learning_job/);
  assert.match(source, /fn fabrication_mdp_request/);
  assert.match(source, /async fn run_nats_loop/);
  assert.match(source, /queue_subscribe\(state\.request_subject\.clone\(\), state\.queue_group\.clone\(\)\)/);
  assert.match(source, /enum FabricationNatsRequest/);
  assert.match(source, /fn parse_fabrication_nats_request/);
  assert.match(source, /FabricationNatsRequest::InstructionAnalysis/);
  assert.match(source, /FabricationNatsRequest::FabricationOutcome/);
  assert.match(source, /FabricationNatsRequest::LearningOutcome/);
  assert.match(source, /fn parse_fabrication_outcome_nats_value/);
  assert.match(source, /fn parse_learning_outcome_nats_value/);
  assert.match(source, /async fn publish_analysis_outputs/);
  assert.match(source, /"fabrication\.instructions\.analysis\.result"/);
  assert.match(source, /async fn publish_learning_outcome_outputs/);
  assert.match(source, /"fabrication\.learning\.outcome\.result"/);
  assert.match(source, /"fabrication\.learning-outcome\.v1"/);
  assert.match(source, /"fabrication\.learning\.observe"/);
  assert.match(source, /"fabrication\.learning\.outcome"/);
  assert.match(source, /publish_learning_outcome_outputs\(\s*&task_state/);
  assert.match(source, /publish_learning_outcome_outputs\(&state/);
  assert.match(source, /FABRICATION_MDP_AUTOPUBLISH/);
  assert.match(source, /dd_fabrication_server_learning_requests_total/);
  assert.match(source, /dd_fabrication_server_learning_events_stored_total/);
  assert.match(source, /dd_fabrication_server_current_learning_outcomes/);
  assert.match(source, /dd_fabrication_server_nats_messages_total/);
  assert.match(source, /dd_fabrication_server_nats_results_published_total/);
  assert.match(source, /dd_fabrication_server_mdp_published_total/);
  assert.match(source, /dd_fabrication_server_jobs_stored_total/);
  assert.match(source, /dd_fabrication_server_artifacts_stored_total/);
  assert.match(source, /dd_fabrication_server_current_jobs/);
  assert.match(source, /struct FabricationJobRecord/);
  assert.match(source, /struct FabricationArtifact/);
  assert.match(source, /struct SimulationReport/);
  assert.match(source, /fn stored_plan_job/);
  assert.match(source, /fn stored_analysis_job/);
  assert.match(source, /fn simulate_instruction_programs/);
  assert.match(source, /fn simulated_arc_axis_ranges/);
  assert.match(source, /SIMULATED_MOTION_AXES/);
  assert.match(source, /fn simulated_axis_unit/);
  assert.match(source, /arc sweep/);
  assert.match(source, /fn has_arc_plane_evidence/);
  assert.match(source, /fn arc_center_offsets_match_plane/);
  assert.match(source, /arc-plane-not-verified/);
  assert.match(source, /arc-plane-boundary/);
  assert.match(source, /arc-center-offset-plane-mismatch/);
  assert.match(source, /arc-plane-offset-boundary/);
  assert.match(source, /arc-missing-center-or-radius/);
  assert.match(source, /arc-geometry-boundary/);
  assert.match(source, /number_after\(&stripped, 'K'\)/);
  assert.match(source, /mill_router_lathe_analysis_requires_arc_plane_evidence_before_arc/);
  assert.match(source, /mill_router_lathe_analysis_requires_arc_offsets_match_plane/);
  assert.match(source, /cnc_analysis_flags_arc_missing_center_or_radius_boundary/);
  assert.match(source, /simulated-axis-envelope-exceeded/);
  assert.match(source, /simulated-machine-envelope/);
  assert.match(source, /simulated-rapid-below-clearance/);
  assert.match(source, /simulated-rapid-clearance/);
  assert.match(source, /simulated-rotary-index-review/);
  assert.match(source, /rotary-index-boundary/);
  assert.match(source, /fn has_process_media_or_chip_evacuation/);
  assert.match(source, /fn has_process_media_or_chip_evacuation_stop/);
  assert.match(source, /fn feed_move_needs_chip_evacuation_review/);
  assert.match(source, /chip-evacuation-not-verified/);
  assert.match(source, /chip-evacuation-boundary/);
  assert.match(source, /chip-evacuation-stopped-before-cut/);
  assert.match(source, /chip-evacuation-stop-boundary/);
  assert.match(source, /fn stock_envelope_excesses/);
  assert.match(source, /id: "horizontal-mill-1"/);
  assert.match(source, /horizontal-slotted-feature/);
  assert.match(source, /draft horizontal milling program generated by dd-fabrication-server/);
  assert.match(source, /horizontal-subtractive-feature/);
  assert.match(source, /id: "sla-printer-1"/);
  assert.match(source, /id: "sls-printer-1"/);
  assert.match(source, /draft resin SLA\/MSLA job generated by dd-fabrication-server/);
  assert.match(source, /draft powder-bed additive job generated by dd-fabrication-server/);
  assert.match(source, /text-post-processing-boundary/);
  assert.match(source, /slicer-profile-missing/);
  assert.match(source, /slicer-profile-boundary/);
  assert.match(source, /slicer-orientation-support-review-missing/);
  assert.match(source, /slicer-orientation-support-boundary/);
  assert.match(source, /slicer-first-layer-evidence-missing/);
  assert.match(source, /slicer-first-layer-boundary/);
  assert.match(source, /has_slicer_mesh_source_context/);
  assert.match(source, /has_slicer_mesh_topology_evidence/);
  assert.match(source, /fn has_text_slicer_mesh_source_context/);
  assert.match(source, /fn has_text_slicer_mesh_topology_evidence/);
  assert.match(source, /slicer-mesh-topology-evidence-missing/);
  assert.match(source, /slicer-mesh-topology-boundary/);
  assert.match(source, /add-slicer-profile-record/);
  assert.match(source, /add-slicer-support-orientation-review/);
  assert.match(source, /add-slicer-first-layer-evidence/);
  assert.match(source, /add-slicer-mesh-topology-evidence/);
  assert.match(source, /fn has_text_resin_context/);
  assert.match(source, /fn has_text_resin_print_context/);
  assert.match(source, /fn has_text_resin_profile_evidence/);
  assert.match(source, /fn has_text_resin_postprocess_evidence/);
  assert.match(source, /fn has_text_powder_bed_context/);
  assert.match(source, /fn has_text_powder_bed_print_context/);
  assert.match(source, /fn has_text_powder_bed_profile_evidence/);
  assert.match(source, /fn has_text_powder_bed_handling_evidence/);
  assert.match(source, /fn has_text_subtractive_context/);
  assert.match(source, /fn has_text_subtractive_setup_evidence/);
  assert.match(source, /fn has_text_subtractive_process_evidence/);
  assert.match(source, /fn has_text_sheet_cutting_context/);
  assert.match(source, /fn has_text_sheet_cutting_recipe_evidence/);
  assert.match(source, /fn has_text_assembly_context/);
  assert.match(source, /fn has_text_assembly_fit_evidence/);
  assert.match(source, /text-resin-handling-boundary/);
  assert.match(source, /resin-handling-boundary/);
  assert.match(source, /resin-print-profile-evidence-missing/);
  assert.match(source, /resin-print-profile-boundary/);
  assert.match(source, /add-resin-print-profile-evidence/);
  assert.match(source, /fn has_text_resin_vat_capacity_context/);
  assert.match(source, /fn has_text_resin_vat_capacity_evidence/);
  assert.match(source, /has_resin_vat_capacity_context/);
  assert.match(source, /has_resin_vat_capacity_evidence/);
  assert.match(source, /resin-vat-capacity-evidence-missing/);
  assert.match(source, /resin-vat-capacity-boundary/);
  assert.match(source, /add-resin-vat-capacity-evidence/);
  assert.match(source, /text_resin_large_jobs_require_vat_capacity_evidence/);
  assert.match(source, /resin-postprocess-evidence-missing/);
  assert.match(source, /resin-postprocess-boundary/);
  assert.match(source, /add-resin-postprocess-evidence/);
  assert.match(source, /text-powder-handling-boundary/);
  assert.match(source, /powder-handling-boundary/);
  assert.match(source, /powder-bed-build-profile-evidence-missing/);
  assert.match(source, /powder-bed-build-profile-boundary/);
  assert.match(source, /add-powder-bed-build-profile-evidence/);
  assert.match(source, /powder-bed-handling-evidence-missing/);
  assert.match(source, /powder-bed-handling-boundary/);
  assert.match(source, /add-powder-bed-handling-evidence/);
  assert.match(source, /subtractive-text-setup-evidence-missing/);
  assert.match(source, /subtractive-text-setup-boundary/);
  assert.match(source, /add-subtractive-text-setup-evidence/);
  assert.match(source, /subtractive-text-process-evidence-missing/);
  assert.match(source, /subtractive-text-process-boundary/);
  assert.match(source, /add-subtractive-text-process-evidence/);
  assert.match(source, /sheet-cutting-recipe-evidence-missing/);
  assert.match(source, /sheet-cutting-recipe-boundary/);
  assert.match(source, /add-sheet-cutting-recipe-evidence/);
  assert.match(source, /assembly-fit-metrology-evidence-missing/);
  assert.match(source, /assembly-fit-metrology-boundary/);
  assert.match(source, /add-assembly-fit-metrology-evidence/);
  assert.match(source, /has_text_precision_requirement_context/);
  assert.match(source, /has_text_precision_inspection_evidence/);
  assert.match(source, /fn has_text_precision_requirement_context/);
  assert.match(source, /fn has_text_precision_inspection_evidence/);
  assert.match(source, /precision-inspection-evidence-missing/);
  assert.match(source, /precision-metrology-boundary/);
  assert.match(source, /add-precision-metrology-evidence/);
  assert.match(source, /has_text_unattended_run_context/);
  assert.match(source, /has_text_unattended_monitoring_evidence/);
  assert.match(source, /fn has_text_unattended_run_context/);
  assert.match(source, /fn has_text_unattended_monitoring_evidence/);
  assert.match(source, /unattended-monitoring-evidence-missing/);
  assert.match(source, /unattended-monitoring-boundary/);
  assert.match(source, /add-unattended-monitoring-evidence/);
  assert.match(source, /has_text_thermal_postprocess_context/);
  assert.match(source, /has_text_thermal_postprocess_evidence/);
  assert.match(source, /fn has_text_thermal_postprocess_context/);
  assert.match(source, /fn has_text_thermal_postprocess_evidence/);
  assert.match(source, /thermal-postprocess-evidence-missing/);
  assert.match(source, /thermal-postprocess-boundary/);
  assert.match(source, /add-thermal-postprocess-evidence/);
  assert.match(source, /has_text_surface_finishing_context/);
  assert.match(source, /has_text_surface_finishing_evidence/);
  assert.match(source, /fn has_text_surface_finishing_context/);
  assert.match(source, /fn has_text_surface_finishing_evidence/);
  assert.match(source, /surface-finishing-evidence-missing/);
  assert.match(source, /surface-finishing-boundary/);
  assert.match(source, /add-surface-finishing-evidence/);
  assert.match(
    source,
    /text_resin_and_powder_jobs_require_process_evidence_before_release/,
  );
  assert.match(
    source,
    /text_resin_print_jobs_require_profile_exposure_and_support_evidence/,
  );
  assert.match(
    source,
    /text_powder_bed_print_jobs_require_build_profile_and_powder_lot_evidence/,
  );
  assert.match(source, /text_subtractive_jobs_require_setup_and_process_evidence/);
  assert.match(
    source,
    /text_sheet_cutting_jobs_require_material_thickness_and_recipe_evidence/,
  );
  assert.match(source, /text_assembly_jobs_require_fit_metrology_and_join_evidence/);
  assert.match(
    source,
    /text_precision_jobs_require_metrology_and_surface_finish_evidence/,
  );
  assert.match(
    source,
    /text_unattended_jobs_require_monitoring_and_recovery_evidence/,
  );
  assert.match(
    source,
    /text_thermal_postprocess_jobs_require_profile_fixture_and_inspection_evidence/,
  );
  assert.match(
    source,
    /text_surface_finishing_jobs_require_chemistry_masking_and_inspection_evidence/,
  );
  assert.match(source, /machine\.axes must be at least 1/);
  assert.match(source, /machine_profile_validation_rejects_zero_axis_machine/);
  assert.match(source, /additive-material-change-boundary/);
  assert.match(source, /fn has_additive_material_resume_evidence/);
  assert.match(source, /additive_material_change_pending_resume/);
  assert.match(source, /additive-material-resume-not-verified/);
  assert.match(source, /additive-material-resume-boundary/);
  assert.match(source, /additive_analysis_requires_resume_evidence_after_material_change/);
  assert.match(source, /fn has_additive_tool_temperature_evidence/);
  assert.match(source, /additive_tool_selection_pending_temperature/);
  assert.match(source, /additive-tool-temperature-not-verified/);
  assert.match(source, /printer-tool-temperature-boundary/);
  assert.match(source, /additive_analysis_requires_selected_tool_temperature_after_tool_change/);
  assert.match(source, /fn has_additive_pause_command/);
  assert.match(source, /additive_pause_pending_resume/);
  assert.match(source, /additive-pause-resume-not-verified/);
  assert.match(source, /printer-pause-resume-boundary/);
  assert.match(source, /additive_analysis_requires_resume_state_after_pause/);
  assert.match(source, /fn add_additive_design_boundaries/);
  assert.match(source, /additive-support-orientation-boundary/);
  assert.match(source, /additive-support-boundary/);
  assert.match(source, /additive-thin-wall-boundary/);
  assert.match(source, /resin-drain-cupping-boundary/);
  assert.match(source, /missing-bed-temperature-wait/);
  assert.match(source, /printer-bed-adhesion-boundary/);
  assert.match(source, /part-cooling-before-first-layer/);
  assert.match(source, /printer-fan-timing-boundary/);
  assert.match(source, /first-layer-setup-risk/);
  assert.match(source, /printer-first-layer-boundary/);
  assert.match(source, /fn has_additive_z_offset_evidence/);
  assert.match(source, /additive_z_offset_evidence_observed/);
  assert.match(source, /additive-negative-z-extrusion-not-verified/);
  assert.match(source, /printer-negative-z-extrusion-boundary/);
  assert.match(source, /additive_analysis_requires_z_offset_evidence_before_negative_z_extrusion/);
  assert.match(source, /fn positioning_absolute_from_line/);
  assert.match(source, /reported_incremental_positioning_program_end_boundary/);
  assert.match(source, /incremental-positioning-not-reset-before-end/);
  assert.match(source, /incremental-positioning-boundary/);
  assert.match(source, /cnc_analysis_requires_absolute_positioning_before_program_end/);
  assert.match(source, /fn has_additive_relative_positioning_evidence/);
  assert.match(source, /additive_relative_positioning_verified/);
  assert.match(source, /additive-relative-positioning-extrusion-not-verified/);
  assert.match(source, /printer-relative-positioning-boundary/);
  assert.match(source, /additive_analysis_requires_positioning_reset_after_relative_mode/);
  assert.match(source, /fn has_controller_dependency_review_evidence/);
  assert.match(source, /fn has_controller_macro_or_subprogram_dependency/);
  assert.match(source, /controller_dependency_review_observed/);
  assert.match(source, /controller-dependency-not-verified/);
  assert.match(source, /controller-dependency-boundary/);
  assert.match(source, /cnc_analysis_flags_unverified_controller_macro_dependencies/);
  assert.match(source, /missing-tool-length-compensation/);
  assert.match(source, /tool-length-boundary/);
  assert.match(source, /mill_router_analysis_flags_rapid_tool_length_plunge_without_reference/);
  assert.match(source, /tool_length_compensation_active/);
  assert.match(source, /reported_tool_length_compensation_cancel_boundary/);
  assert.match(source, /tool-length-compensation-not-cancelled-before-tool-change/);
  assert.match(source, /tool-length-compensation-cancel-boundary/);
  assert.match(source, /mill_router_analysis_requires_tool_length_cancel_before_tool_change/);
  assert.match(source, /reported_tool_change_spindle_stop_boundary/);
  assert.match(source, /tool-change-before-spindle-stop/);
  assert.match(source, /tool-change-spindle-stop-boundary/);
  assert.match(source, /mill_router_analysis_requires_spindle_stop_before_tool_change/);
  assert.match(source, /fn has_cutter_compensation_evidence/);
  assert.match(source, /cutter-compensation-offset-not-verified/);
  assert.match(source, /cutter-compensation-boundary/);
  assert.match(source, /cutter_compensation_active/);
  assert.match(source, /reported_cutter_compensation_cancel_boundary/);
  assert.match(source, /cutter-compensation-not-cancelled/);
  assert.match(source, /cutter-compensation-cancel-boundary/);
  assert.match(source, /mill_router_analysis_requires_cutter_compensation_offset_evidence/);
  assert.match(source, /mill_router_analysis_requires_cutter_compensation_cancel_before_end/);
  assert.match(source, /canned-cycle-missing-plane-or-depth/);
  assert.match(source, /canned-cycle-boundary/);
  assert.match(source, /canned-cycle-unsafe-retract-plane/);
  assert.match(source, /canned-cycle-retract-plane-boundary/);
  assert.match(source, /mill_analysis_requires_positive_canned_cycle_retract_plane/);
  assert.match(source, /modal_canned_cycle_active/);
  assert.match(source, /reported_modal_canned_cycle_boundary/);
  assert.match(source, /motion-before-canned-cycle-cancel/);
  assert.match(source, /canned-cycle-cancel-boundary/);
  assert.match(source, /mill_analysis_flags_motion_before_canned_cycle_cancel/);
  assert.match(source, /tapping-cycle-boundary/);
  assert.match(source, /instruction-material-machine-incompatible/);
  assert.match(source, /material-machine-boundary/);
  assert.match(source, /plan_existing_instructions_inherit_material_machine_validation/);
  assert.match(
    source,
    /analyze_instruction_material_compatibility\(&existing_programs, &machines, &material\)/,
  );
  assert.match(
    source,
    /struct FabricationPlanResponse[\s\S]*boundary_summary: BoundarySummary[\s\S]*improvements: Vec<InstructionImprovement>[\s\S]*improved_programs: Vec<ImprovedInstructionProgram>/,
  );
  assert.match(source, /manufacturing_handoff: ManufacturingHandoff/);
  assert.match(source, /design_package: DesignPackage/);
  assert.match(source, /quality_plan: QualityPlan/);
  assert.match(source, /struct DesignPackage/);
  assert.match(source, /struct DesignPackagePart/);
  assert.match(source, /struct DesignAssemblyExport/);
  assert.match(source, /struct DesignExportTarget/);
  assert.match(source, /fn design_package/);
  assert.match(source, /export_targets: Vec<DesignExportTarget>/);
  assert.match(source, /coordinate_frame: Vec<String>/);
  assert.match(source, /assembly_exports: Vec<DesignAssemblyExport>/);
  assert.match(source, /struct ManufacturingHandoff/);
  assert.match(source, /struct ManufacturingHandoffPart/);
  assert.match(source, /struct ManufacturingHandoffGate/);
  assert.match(source, /fn manufacturing_handoff/);
  assert.match(source, /dd\.fabrication\.manufacturing-handoff\.v1/);
  assert.match(source, /struct QualityPlan/);
  assert.match(source, /struct QualityInspectionPoint/);
  assert.match(source, /struct QualityMeasurementTarget/);
  assert.match(source, /fn quality_plan/);
  assert.match(source, /inspection_points: Vec<QualityInspectionPoint>/);
  assert.match(source, /measurement_targets: Vec<QualityMeasurementTarget>/);
  assert.match(source, /learning_observations: Vec<String>/);
  assert.match(source, /struct BoundarySummary/);
  assert.match(source, /struct AutomationRequirement/);
  assert.match(source, /struct BoundaryResolutionPlan/);
  assert.match(source, /struct BoundaryResolutionStep/);
  assert.match(source, /fn boundary_summary/);
  assert.match(source, /fn automation_requirement_type/);
  assert.match(source, /fn resolution_phase_and_next_state/);
  assert.match(source, /fn boundary_resolution_plan/);
  assert.match(source, /machine_release_blocked/);
  assert.match(source, /failed-until-resolved/);
  assert.match(source, /human_intervention_required/);
  assert.match(source, /split_recommended/);
  assert.match(source, /combine_recommended/);
  assert.match(source, /automation_required/);
  assert.match(source, /automation_requirements: Vec<AutomationRequirement>/);
  assert.match(source, /resolution_plan: BoundaryResolutionPlan/);
  assert.match(source, /material-change-automation/);
  assert.match(source, /process-cell-automation/);
  assert.match(source, /regeneration_recommended/);
  assert.match(source, /"boundary-summary"/);
  assert.match(source, /"analysis-boundary-summary"/);
  assert.match(source, /"resolution-plan"/);
  assert.match(source, /"process-graph"/);
  assert.match(source, /"machine-selection"/);
  assert.match(source, /"manufacturing-handoff"/);
  assert.match(source, /"analysis-resolution-plan"/);
  assert.match(source, /fn boundary_learning_actions/);
  assert.match(source, /fn boundary_learning_observations/);
  assert.match(source, /fn intervention_learning_signals/);
  assert.match(source, /intervention_signals: Vec<InterventionLearningSignal>/);
  assert.match(source, /automation-requirement-vector/);
  assert.match(source, /resolution-step-policy-state/);
  assert.match(source, /boundary-split-job-or-part-machine-envelope/);
  assert.match(source, /boundary-kind:machine-envelope:/);
  assert.match(source, /"plan-improvements"/);
  assert.match(
    source,
    /improve_instruction_programs\(&generated_as_input, &validation, &improvements\)/,
  );
  assert.match(source, /lathe-css-without-spindle-limit/);
  assert.match(source, /lathe-threading-boundary/);
  assert.match(source, /fn has_lathe_threading_feed_mode_evidence/);
  assert.match(source, /lathe_threading_feed_mode_observed/);
  assert.match(source, /lathe-threading-feed-mode-not-verified/);
  assert.match(source, /lathe-threading-feed-mode-boundary/);
  assert.match(source, /lathe_analysis_requires_threading_feed_mode_evidence/);
  assert.match(source, /has_lathe_text_threading_context/);
  assert.match(source, /has_lathe_text_threading_sync_evidence/);
  assert.match(source, /lathe-text-threading-sync-evidence-missing/);
  assert.match(source, /lathe-text-threading-sync-boundary/);
  assert.match(source, /add-lathe-text-threading-sync-evidence/);
  assert.match(
    source,
    /text_lathe_threading_jobs_require_feed_per_rev_pitch_sync_evidence/,
  );
  assert.match(source, /lathe-part-off-boundary/);
  assert.match(source, /fn has_lathe_partoff_support_evidence/);
  assert.match(source, /fn has_lathe_partoff_command/);
  assert.match(source, /lathe_partoff_support_evidence_observed/);
  assert.match(source, /lathe-partoff-support-not-verified/);
  assert.match(source, /lathe-partoff-support-boundary/);
  assert.match(source, /lathe_analysis_requires_partoff_support_evidence/);
  assert.match(source, /has_lathe_text_partoff_context/);
  assert.match(source, /has_lathe_text_partoff_support_evidence/);
  assert.match(source, /lathe-text-partoff-support-evidence-missing/);
  assert.match(source, /lathe-text-partoff-support-boundary/);
  assert.match(source, /add-lathe-text-partoff-support-evidence/);
  assert.match(
    source,
    /text_lathe_partoff_jobs_require_catcher_or_stock_support_evidence/,
  );
  assert.match(source, /fn has_lathe_workholding_evidence/);
  assert.match(source, /lathe-workholding-not-verified/);
  assert.match(source, /lathe-workholding-boundary/);
  assert.match(source, /fn has_lathe_tool_nose_compensation_evidence/);
  assert.match(source, /reported_lathe_tool_change_spindle_stop_boundary/);
  assert.match(source, /lathe-tool-change-before-spindle-stop/);
  assert.match(source, /lathe-tool-change-spindle-stop-boundary/);
  assert.match(source, /lathe_analysis_requires_spindle_stop_before_tool_change/);
  assert.match(source, /lathe-tool-nose-compensation-not-verified/);
  assert.match(source, /lathe-tool-nose-compensation-boundary/);
  assert.match(source, /lathe_tool_nose_compensation_active/);
  assert.match(source, /reported_lathe_tool_nose_compensation_cancel_boundary/);
  assert.match(source, /lathe-tool-nose-compensation-not-cancelled/);
  assert.match(source, /lathe-tool-nose-compensation-cancel-boundary/);
  assert.match(source, /lathe_analysis_requires_tool_nose_compensation_evidence/);
  assert.match(source, /lathe_analysis_requires_tool_nose_compensation_cancel_before_end/);
  assert.match(source, /M82 ; absolute extrusion mode/);
  assert.match(source, /filament dry-storage evidence verified/);
  assert.match(source, /G92 E0 ; reset extruder before priming/);
  assert.match(source, /missing-extrusion-mode/);
  assert.match(source, /missing-extruder-reset-before-prime/);
  assert.match(source, /printer-extrusion-state-boundary/);
  assert.match(source, /fn has_additive_extrusion_reset_evidence/);
  assert.match(source, /additive_extrusion_mode_reset_pending/);
  assert.match(source, /additive-extrusion-mode-switch-reset-not-verified/);
  assert.match(source, /printer-extrusion-mode-switch-boundary/);
  assert.match(source, /additive_analysis_requires_extruder_reset_after_mode_switch/);
  assert.match(source, /fn has_additive_nozzle_wait_evidence/);
  assert.match(source, /additive_nozzle_wait_pending/);
  assert.match(source, /additive-nozzle-wait-not-verified/);
  assert.match(source, /printer-nozzle-wait-boundary/);
  assert.match(source, /additive_analysis_requires_nozzle_wait_after_async_heat_command/);
  assert.match(source, /nozzle_heat_active/);
  assert.match(source, /reported_nozzle_cooldown_boundary/);
  assert.match(source, /extrusion-after-nozzle-cooldown/);
  assert.match(source, /printer-nozzle-cooldown-boundary/);
  assert.match(source, /additive_analysis_flags_extrusion_after_nozzle_cooldown/);
  assert.match(source, /bed_heat_active/);
  assert.match(source, /bed_wait_active/);
  assert.match(source, /fn has_additive_bed_wait_evidence/);
  assert.match(source, /additive_bed_wait_pending/);
  assert.match(source, /additive-bed-wait-not-verified/);
  assert.match(source, /printer-bed-temperature-wait-boundary/);
  assert.match(source, /additive_analysis_requires_bed_wait_after_async_target_change/);
  assert.match(source, /bed_cooldown_observed/);
  assert.match(source, /reported_bed_cooldown_boundary/);
  assert.match(source, /extrusion-after-bed-cooldown/);
  assert.match(source, /printer-bed-cooldown-boundary/);
  assert.match(source, /additive_analysis_flags_extrusion_after_bed_cooldown/);
  assert.match(source, /fn has_printer_restart_position_evidence/);
  assert.match(source, /printer_position_reference_active/);
  assert.match(source, /printer_stepper_disable_observed/);
  assert.match(source, /reported_printer_stepper_disable_boundary/);
  assert.match(source, /motion-after-stepper-disable/);
  assert.match(source, /printer-stepper-idle-boundary/);
  assert.match(source, /additive_analysis_flags_motion_after_stepper_disable/);
  assert.match(source, /fn has_additive_material_conditioning_evidence/);
  assert.match(source, /missing-filament-conditioning-evidence/);
  assert.match(source, /printer-material-conditioning-boundary/);
  assert.match(source, /fn has_additive_extrusion_calibration_evidence/);
  assert.match(source, /additive_extrusion_calibration_observed/);
  assert.match(source, /reported_additive_extrusion_calibration_boundary/);
  assert.match(source, /additive-extrusion-calibration-missing/);
  assert.match(source, /printer-extrusion-calibration-boundary/);
  assert.match(source, /additive_analysis_requires_extrusion_calibration_before_first_extrusion/);
  assert.match(source, /ADDITIVE_MATERIAL_CAPACITY_EXTRUSION_MM_THRESHOLD/);
  assert.match(source, /fn has_additive_material_capacity_evidence/);
  assert.match(source, /fn additive_extrusion_uses_significant_material/);
  assert.match(source, /additive_material_capacity_observed/);
  assert.match(source, /reported_additive_material_capacity_boundary/);
  assert.match(source, /additive-material-capacity-evidence-missing/);
  assert.match(source, /printer-material-capacity-boundary/);
  assert.match(source, /additive_analysis_requires_material_capacity_for_large_extrusion/);
  assert.match(source, /fn has_additive_firmware_retraction_evidence/);
  assert.match(source, /additive_firmware_retraction_evidence_observed/);
  assert.match(source, /reported_additive_firmware_retraction_boundary/);
  assert.match(source, /additive-firmware-retraction-settings-missing/);
  assert.match(source, /printer-firmware-retraction-boundary/);
  assert.match(source, /additive_analysis_requires_firmware_retraction_evidence/);
  assert.match(source, /fn has_additive_high_speed_kinematic_evidence/);
  assert.match(source, /fn additive_feed_is_high_speed/);
  assert.match(source, /additive_high_speed_kinematic_evidence_observed/);
  assert.match(source, /reported_additive_high_speed_kinematic_boundary/);
  assert.match(source, /additive-high-speed-kinematics-missing/);
  assert.match(source, /printer-high-speed-kinematics-boundary/);
  assert.match(source, /additive_analysis_requires_high_speed_kinematic_evidence/);
  assert.match(source, /fn has_text_additive_high_speed_context/);
  assert.match(source, /has_slicer_high_speed_context/);
  assert.match(source, /has_slicer_high_speed_kinematic_evidence/);
  assert.match(source, /slicer-high-speed-kinematics-evidence-missing/);
  assert.match(source, /slicer-high-speed-kinematics-boundary/);
  assert.match(source, /add-slicer-high-speed-kinematic-evidence/);
  assert.match(source, /text_slicer_jobs_require_mesh_topology_and_scale_evidence/);
  assert.match(source, /text_slicer_high_speed_jobs_require_kinematic_evidence/);
  assert.match(source, /fn has_additive_warp_prone_material_context/);
  assert.match(source, /fn has_additive_chamber_thermal_evidence/);
  assert.match(source, /additive_warp_prone_material_observed/);
  assert.match(source, /additive_chamber_thermal_evidence_observed/);
  assert.match(source, /additive-chamber-thermal-evidence-missing/);
  assert.match(source, /printer-chamber-thermal-boundary/);
  assert.match(
    source,
    /additive_analysis_requires_chamber_thermal_evidence_for_warp_prone_filament/,
  );
  assert.match(source, /fn has_tool_change_automation_evidence/);
  assert.match(source, /tool-change-automation-not-verified/);
  assert.match(source, /tool-change-automation-boundary/);
  assert.match(source, /ATC\/magazine or operator-loaded/);
  assert.match(source, /fn has_mill_router_workholding_evidence/);
  assert.match(source, /mill-router-workholding-not-verified/);
  assert.match(source, /mill-router-workholding-boundary/);
  assert.match(source, /operator-verified spoilboard, vacuum\/hold-down/);
  assert.match(source, /line_has_mill_router_negative_z_rapid/);
  assert.match(source, /mill_router_analysis_requires_setup_evidence_before_rapid_plunge/);
  assert.match(source, /fn has_cutting_feed_rate_evidence/);
  assert.match(source, /cutting-feed-rate-not-verified/);
  assert.match(source, /cutting-feed-rate-boundary/);
  assert.match(source, /SUBTRACTIVE_TOOL_LIFE_LONG_CUT_MM_THRESHOLD/);
  assert.match(source, /fn has_subtractive_tool_life_evidence/);
  assert.match(source, /fn feed_move_is_long_subtractive_cut/);
  assert.match(source, /subtractive_tool_life_evidence_observed/);
  assert.match(source, /reported_subtractive_tool_life_boundary/);
  assert.match(source, /subtractive-tool-life-evidence-missing/);
  assert.match(source, /subtractive-tool-life-boundary/);
  assert.match(source, /subtractive_analysis_requires_tool_life_for_long_cuts/);
  assert.match(source, /fn has_work_offset_datum_evidence/);
  assert.match(source, /work-offset-datum-not-verified/);
  assert.match(source, /work-offset-datum-boundary/);
  assert.match(source, /fn has_probe_cycle_command/);
  assert.match(source, /fn has_probe_setup_evidence/);
  assert.match(source, /fn has_probe_recovery_evidence/);
  assert.match(source, /fn probe_cycle_has_safe_feed/);
  assert.match(source, /probe_setup_evidence_observed/);
  assert.match(source, /probe_recovery_evidence_observed/);
  assert.match(source, /reported_probing_cycle_safety_boundary/);
  assert.match(source, /probing-cycle-safety-evidence-missing/);
  assert.match(source, /probing-cycle-safety-boundary/);
  assert.match(source, /subtractive_analysis_requires_probe_cycle_setup_feed_and_recovery_evidence/);
  assert.match(source, /subtractive_spindle_speed_evidence_observed/);
  assert.match(source, /reported_spindle_speed_boundary/);
  assert.match(source, /spindle-speed-not-verified-before-start/);
  assert.match(source, /spindle-speed-boundary/);
  assert.match(source, /subtractive_analysis_requires_spindle_speed_before_start/);
  assert.match(source, /current_spindle_direction/);
  assert.match(source, /reported_spindle_direction_change_boundary/);
  assert.match(source, /spindle-direction-change-before-stop/);
  assert.match(source, /spindle-direction-boundary/);
  assert.match(source, /subtractive_analysis_requires_stop_before_spindle_direction_change/);
  assert.match(source, /reported_rapid_plunge_before_spindle_boundary/);
  assert.match(source, /rapid-plunge-before-spindle/);
  assert.match(source, /rapid-plunge-spindle-boundary/);
  assert.match(source, /mill_router_analysis_flags_rapid_plunge_before_spindle_start/);
  assert.match(source, /reported_rapid_plunge_after_process_stop_boundary/);
  assert.match(source, /rapid-plunge-after-process-stop/);
  assert.match(source, /rapid-plunge-process-stop-boundary/);
  assert.match(source, /mill_router_analysis_flags_rapid_plunge_after_process_stop/);
  assert.match(source, /cut-after-process-stop/);
  assert.match(source, /machine-process-stop-boundary/);
  assert.match(source, /subtractive_analysis_flags_feed_after_process_stop/);
  assert.match(source, /id: "laser-cutter-1"/);
  assert.match(source, /id: "waterjet-cutter-1"/);
  assert.match(source, /id: "plasma-cutter-1"/);
  assert.match(source, /draft laser sheet-cutting job generated by dd-fabrication-server/);
  assert.match(source, /draft waterjet sheet-cutting job generated by dd-fabrication-server/);
  assert.match(source, /draft plasma sheet-cutting job generated by dd-fabrication-server/);
  assert.match(source, /ABRASIVE_FLOW_TEST/);
  assert.match(source, /PLASMA_CUT/);
  assert.match(source, /text-sheet-cutting-boundary/);
  assert.match(source, /fn has_sheet_cutting_process_evidence/);
  assert.match(source, /SHEET_CUT_PART_RETENTION_LONG_MOVE_MM_THRESHOLD/);
  assert.match(source, /fn has_sheet_cutting_part_retention_evidence/);
  assert.match(source, /fn sheet_cut_feed_needs_part_retention_review/);
  assert.match(source, /sheet_cutting_part_retention_evidence_observed/);
  assert.match(source, /reported_sheet_cutting_part_retention_boundary/);
  assert.match(source, /fn has_sheet_cutting_support_media_evidence/);
  assert.match(source, /sheet_cutting_support_media_active/);
  assert.match(source, /fn has_plasma_work_clamp_evidence/);
  assert.match(source, /plasma_work_clamp_evidence_observed/);
  assert.match(source, /plasma-work-clamp-not-verified/);
  assert.match(source, /plasma-work-clamp-boundary/);
  assert.match(source, /fn has_waterjet_pressure_or_abrasive_evidence/);
  assert.match(source, /waterjet_pressure_or_abrasive_evidence_observed/);
  assert.match(source, /waterjet-pressure-abrasive-not-verified/);
  assert.match(source, /waterjet-pressure-abrasive-boundary/);
  assert.match(source, /sheet-cutting-process-not-verified/);
  assert.match(source, /sheet-cutting-process-boundary/);
  assert.match(source, /sheet-cutting-part-retention-evidence-missing/);
  assert.match(source, /sheet-cutting-part-retention-boundary/);
  assert.match(source, /sheet_cut_analysis_requires_part_retention_for_profile_release/);
  assert.match(source, /sheet-cutting-support-media-stopped-before-cut/);
  assert.match(source, /sheet-cutting-support-media-stop-boundary/);
  assert.match(source, /sheet_cut_analysis_requires_support_media_restart_after_stop/);
  assert.match(source, /sheet_cut_analysis_requires_waterjet_pressure_and_abrasive_evidence/);
  assert.match(source, /sheet_cut_analysis_requires_plasma_work_clamp_evidence/);
  assert.match(source, /kerf-controlled-sheet-profile/);
  assert.match(source, /"choose-sheet-cutting-process"\.to_string\(\)/);
  assert.match(source, /method_combination_preferences/);
  assert.match(source, /learned_parts_for_method_combination/);
  assert.match(source, /prefer-learned-method-combination/);
  assert.match(source, /dd\.fabrication\.neural-policy-sketch\.v1/);
  assert.match(source, /pomdp_belief_state: PomdpBeliefState/);
  assert.match(source, /neural_training_corpus: NeuralTrainingCorpus/);
  assert.match(source, /feature_vector: Vec<f64>/);
  assert.match(source, /inference_candidates: Vec<NeuralInferenceCandidate>/);
  assert.match(source, /strategy_candidates: Vec<StrategyCandidate>/);
  assert.match(source, /selected-hybrid-plan/);
  assert.match(source, /additive-consolidation-candidate/);
  assert.match(source, /machined-datum-finish-candidate/);
  assert.match(source, /split-for-inspection-candidate/);
  assert.match(source, /"strategyCandidates": response\.learning\.strategy_candidates/);
  assert.match(source, /"designPackage": response\.design_package/);
  assert.match(source, /"productionPlan": response\.production_plan/);
  assert.match(source, /"machineSchedule": response\.machine_schedule/);
  assert.match(source, /"qualityPlan": response\.quality_plan/);
  assert.match(source, /"toolingPlan": response\.tooling_plan/);
  assert.match(source, /"machineSelection": response\.machine_selection/);
  assert.match(source, /"manufacturingHandoff": response\.manufacturing_handoff/);
  assert.match(source, /"processGraph": response\.process_graph/);
  assert.match(source, /"interventionMap": response\.intervention_map/);
  assert.match(source, /"interventionSignals": response\.learning\.intervention_signals/);
  assert.match(source, /"postprocessPlan": response\.postprocess_plan/);
  assert.match(source, /"pomdpBeliefState": response\.learning\.pomdp_belief_state/);
  assert.match(source, /"neuralTrainingCorpus": response\.learning\.neural_training_corpus/);
  assert.match(source, /"automationRequirements": response\.boundary_summary\.automation_requirements/);
  assert.match(source, /"resolutionPlan": response\.resolution_plan/);
  assert.match(source, /"automation-required"\.to_string\(\)/);
  assert.match(source, /hidden_activations/);
  assert.match(source, /action_scores/);
  assert.match(source, /id: "cnc-router-1"/);
  assert.match(source, /preferred_method: Some\("routing"\.to_string\(\)\)/);
  assert.match(source, /"choose-routing-process"\.to_string\(\)/);
  assert.match(source, /draft router profile program generated by dd-fabrication-server/);
  assert.match(source, /lift over tab boundary/);
  assert.match(source, /"machine-envelope"/);
  assert.match(source, /MAX_MACHINE_PROFILE_EVIDENCE/);
  assert.match(source, /struct MachineProfileEvidence/);
  assert.match(source, /profile_evidence: Option<MachineProfileEvidence>/);
  assert.match(source, /validate_machine_profile_evidence/);
  assert.match(source, /machine_profile_evidence_boundaries/);
  assert.match(source, /machine_profile_blocker_count/);
  assert.match(source, /select_machine/);
  assert.match(source, /rejected-profile-blocker/);
  assert.match(source, /"machine-profile-blocker"\.to_string\(\)/);
  assert.match(source, /machine_profile_evidence_blockers_hold_plan_release/);
  assert.match(source, /machine_selection_prefers_profile_clear_machine/);
  assert.match(source, /machine_profile_evidence_blockers_hold_instruction_analysis_release/);
  assert.match(source, /async fn capabilities/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.capabilities\.v1"/);
  assert.match(source, /"defaultMachines": default_machines\(\)/);
  assert.match(source, /"acceptedInstructionKinds"/);
  assert.match(source, /"safetyBoundaryClasses"/);
  assert.match(source, /"machine-profile-evidence"/);
  assert.match(source, /"machine-profile-blocker"/);
  assert.match(source, /async fn request_schema/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.request-schema\.v1"/);
  assert.match(source, /"planRequest"/);
  assert.match(source, /"profileEvidence"/);
  assert.match(source, /"machineProfileEvidence"/);
  assert.match(source, /"instructionProgram"/);
  assert.match(source, /async fn examples/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.examples\.v1"/);
  assert.match(source, /"hybridPlan"/);
  assert.match(source, /"instructionAnalysis"/);
  assert.match(source, /async fn list_jobs/);
  assert.match(source, /async fn get_artifact/);
  assert.match(source, /async fn learning_observe_http/);
  assert.match(source, /async fn learning_policy_http/);
  assert.match(source, /\.route\("\/jobs", get\(list_jobs\)\)/);
  assert.match(source, /\.route\("\/capabilities", get\(capabilities\)\)/);
  assert.match(source, /\.route\("\/fabrication\/capabilities", get\(capabilities\)\)/);
  assert.match(source, /\.route\("\/schema", get\(request_schema\)\)/);
  assert.match(source, /\.route\("\/fabrication\/schema", get\(request_schema\)\)/);
  assert.match(source, /\.route\("\/examples", get\(examples\)\)/);
  assert.match(source, /\.route\("\/fabrication\/examples", get\(examples\)\)/);
  assert.match(source, /\.route\("\/jobs\/:job_id", get\(get_job\)\)/);
  assert.match(
    source,
    /\.route\("\/jobs\/:job_id\/artifacts\/:artifact_id", get\(get_artifact\)\)/,
  );
  assert.match(source, /\.route\("\/plan", post\(plan_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/plan", post\(plan_http\)\)/);
  assert.match(source, /\.route\("\/instructions\/analyze", post\(analyze_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/instructions\/analyze", post\(analyze_http\)\)/);
  assert.match(source, /\.route\("\/learning\/policy", get\(learning_policy_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/learning\/policy", get\(learning_policy_http\)\)/);
  assert.match(source, /\.route\("\/learning\/observe", post\(learning_observe_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/learning\/observe", post\(learning_observe_http\)\)/,
  );
  assert.match(source, /\.route\("\/learning\/outcomes", post\(learning_outcome_http\)\)/);

  assert.match(readme, /`GET \/jobs`/);
  assert.match(readme, /`GET \/capabilities`/);
  assert.match(readme, /`GET \/fabrication\/capabilities`/);
  assert.match(readme, /`GET \/schema`/);
  assert.match(readme, /`GET \/fabrication\/schema`/);
  assert.match(readme, /`GET \/examples`/);
  assert.match(readme, /`GET \/fabrication\/examples`/);
  assert.match(readme, /built-in `defaultMachines`/);
  assert.match(readme, /accepted instruction\s+kinds/);
  assert.match(readme, /safety boundary\s+classes/);
  assert.match(readme, /`profileEvidence`/);
  assert.match(readme, /`profileEvidence\.blockers` are promoted into validation findings/);
  assert.match(readme, /`machine-profile-blocker` failure boundaries/);
  assert.match(readme, /selection prefers compatible machines with no retained profile blockers/);
  assert.match(readme, /`rejected-profile-blocker` candidates/);
  assert.match(readme, /calibration, tools, fixtures,\s+materials, process support/);
  assert.match(readme, /ready-to-edit JSON examples/);
  assert.match(readme, /hybrid\s+printed\/milled\/turned plan/);
  assert.match(readme, /`GET \/jobs\/:job_id`/);
  assert.match(readme, /`GET \/jobs\/:job_id\/artifacts\/:artifact_id`/);
  assert.match(readme, /`GET \/learning\/policy`/);
  assert.match(readme, /`POST \/learning\/observe`/);
  assert.match(readme, /`POST \/fabrication\/learning\/observe`/);
  assert.match(readme, /Outcome Learning/);
  assert.match(readme, /reward-signal/);
  assert.match(readme, /outcome-remediation-plan/);
  assert.match(readme, /`outcomeRemediation` plan/);
  assert.match(readme, /`remediationRisks`/);
  assert.match(readme, /material-specific `remediationRisks`/);
  assert.match(readme, /learned-remediation-risk/);
  assert.match(readme, /avoid-learned-risk-milling-petg/);
  assert.match(readme, /review\/avoid policy actions/);
  assert.match(readme, /pomdp-observations/);
  assert.match(readme, /`pomdpBeliefState`/);
  assert.match(readme, /`pomdp-belief-state`/);
  assert.match(readme, /hidden-state probabilities/);
  assert.match(readme, /observation likelihoods/);
  assert.match(readme, /recommended probe actions/);
  assert.match(readme, /neural-example/);
  assert.match(readme, /neural-policy sketch/);
  assert.match(readme, /`neuralTrainingCorpus`/);
  assert.match(readme, /`neural-training-corpus`/);
  assert.match(readme, /feature vectors, labels, inference candidates/);
  assert.match(readme, /policy-memory examples/);
  assert.match(readme, /strategy inference candidates/);
  assert.match(readme, /scored `strategyCandidates`/);
  assert.match(readme, /typed `interventionSignals`/);
  assert.match(readme, /boundary-specific\s+policy actions/);
  assert.match(readme, /regeneration decisions can be learned/);
  assert.match(readme, /selected hybrid/);
  assert.match(readme, /additive consolidation/);
  assert.match(readme, /machined datum-finish/);
  assert.match(readme, /split-for-inspection/);
  assert.match(readme, /`neuralPolicy` sketch/);
  assert.match(readme, /`interventionSignals` expose/);
  assert.match(readme, /hidden activations/);
  assert.match(readme, /reuse strong learned method and assembly/);
  assert.match(readme, /structured `assemblyGraph`/);
  assert.match(readme, /hybrid\s+interface edges/);
  assert.match(readme, /without explicit\s+`preferredMethods`/);
  assert.match(readme, /learned hybrid join strategies/);
  assert.match(readme, /`POST \/plan`/);
  assert.match(readme, /`POST \/fabrication\/plan`/);
  assert.match(readme, /`POST \/instructions\/analyze`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/analyze`/);
  assert.match(readme, /rich fabrication outcome payloads/);
  assert.match(readme, /compact learning-outcome payloads/);
  assert.match(readme, /fabrication\.learning\.observe/);
  assert.match(readme, /fabrication\.learning\.outcome/);
  assert.match(readme, /fabrication\.learning\.outcome\.result/);
  assert.match(
    readme,
    /Plan,\s+instruction-analysis,\s+learning-observation,\s+and compact learning-outcome results/,
  );
  assert.match(readme, /learning-observation, or\s+learning-outcome request/);
  assert.match(readme, /bounded in-process job and artifact ledger/);
  assert.match(readme, /boundary summaries/);
  assert.match(readme, /resolution plans/);
  assert.match(readme, /machine-release reports/);
  assert.match(readme, /execution plans/);
  assert.match(readme, /postprocess plans/);
  assert.match(readme, /POMDP\s+belief states/);
  assert.match(readme, /neural training corpora/);
  assert.match(readme, /manufacturing\s+handoffs/);
  assert.match(readme, /design packages/);
  assert.match(readme, /quality plans/);
  assert.match(readme, /tooling plans/);
  assert.match(readme, /machine-selection traces/);
  assert.match(readme, /production plans/);
  assert.match(readme, /process\s+graphs/);
  assert.match(readme, /intervention maps/);
  assert.match(readme, /assembly graphs/);
  assert.match(readme, /structured `processGraph`/);
  assert.match(readme, /`boundarySummary` object/);
  assert.match(readme, /typed `automationRequirements`/);
  assert.match(readme, /`resolutionPlan`/);
  assert.match(readme, /`machineRelease` report/);
  assert.match(readme, /`executionPlan` preflight/);
  assert.match(readme, /program runs, checkpoints, stop points/);
  assert.match(readme, /`postprocessPlan` preflight/);
  assert.match(readme, /controller-specific targets/);
  assert.match(readme, /postprocessor selection/);
  assert.match(readme, /dry-run gates/);
  assert.match(readme, /operator signoff/);
  assert.match(readme, /`machineSelection` trace/);
  assert.match(readme, /`designPackage`/);
  assert.match(readme, /`designExports` bundle/);
  assert.match(readme, /`designInputReview`/);
  assert.match(readme, /generated design export payloads/);
  assert.match(readme, /3MF\/STL\/STEP\/DXF\/CAM setup\/nesting\/assembly payloads/);
  assert.match(readme, /neutral export targets/);
  assert.match(readme, /Creo\/Pro\/ENGINEER, SOLIDWORKS, Fusion/);
  assert.match(readme, /Siemens NX, CATIA, Onshape/);
  assert.match(readme, /FreeCAD, OpenSCAD, Blender, ZBrush/);
  assert.match(readme, /PrusaSlicer\/OrcaSlicer\/Cura\/Bambu Studio/);
  assert.match(readme, /coordinate frames/);
  assert.match(readme, /3MF, STL, STEP, DXF, CAM setup JSON/);
  assert.match(readme, /`productionPlan`/);
  assert.match(readme, /quantity-aware batches/);
  assert.match(readme, /`machineSchedule`/);
  assert.match(readme, /deterministic machine lanes/);
  assert.match(readme, /operator or automation assignments/);
  assert.match(readme, /`qualityPlan`/);
  assert.match(readme, /inspection points/);
  assert.match(readme, /measurement targets/);
  assert.match(readme, /learning observations/);
  assert.match(readme, /`toolingPlan`/);
  assert.match(readme, /setup traveler/);
  assert.match(readme, /required tools/);
  assert.match(readme, /workholding/);
  assert.match(readme, /consumables/);
  assert.match(readme, /automation dependencies/);
  assert.match(readme, /candidate scores, material\/process/);
  assert.match(readme, /operation gaps, and fallback warnings/);
  assert.match(readme, /`manufacturingHandoff` package/);
  assert.match(readme, /checklist status, release blockers/);
  assert.match(readme, /datum scheme, fixture\/setup plan/);
  assert.match(readme, /split-job-or-part/);
  assert.match(readme, /combine-or-assemble-parts/);
  assert.match(readme, /add-verified-automation/);
  assert.match(readme, /resolve-machine-failure-risk/);
  assert.match(readme, /`boundary-summary`/);
  assert.match(readme, /`analysis-boundary-summary`/);
  assert.match(readme, /`resolution-plan`/);
  assert.match(readme, /`analysis-resolution-plan`/);
  assert.match(readme, /`intervention-map`/);
  assert.match(readme, /`analysis-intervention-map`/);
  assert.match(readme, /`execution-plan`/);
  assert.match(readme, /`analysis-execution-plan`/);
  assert.match(readme, /`postprocess-plan`/);
  assert.match(readme, /`analysis-postprocess-plan`/);
  assert.match(readme, /`neural-training-corpus`/);
  assert.match(readme, /human intervention points/);
  assert.match(readme, /split\/combine decisions/);
  assert.match(readme, /automation paths/);
  assert.match(readme, /program boundary traces/);
  assert.match(readme, /`machine-selection`/);
  assert.match(readme, /`design-package`/);
  assert.match(readme, /`design-export-bundle`/);
  assert.match(readme, /`design-input-review`/);
  assert.match(readme, /`generated-design-export`/);
  assert.match(readme, /`production-plan`/);
  assert.match(readme, /`quality-plan`/);
  assert.match(readme, /`tooling-plan`/);
  assert.match(readme, /`machine-release`/);
  assert.match(readme, /`analysis-machine-release`/);
  assert.match(readme, /`manufacturing-handoff`/);
  assert.match(readme, /`assembly\.assemblyGraph`/);
  assert.match(readme, /retained `process-graph`/);
  assert.match(readme, /operation order, generated programs/);
  assert.match(readme, /join interfaces, dry-fit\/metrology gates/);
  assert.match(readme, /`process-graph`/);
  assert.match(
    readme,
    /`mdp-request` artifact includes\s+`strategyCandidates`, `interventionSignals`, `pomdpBeliefState`,\s+`neuralTrainingCorpus`/,
  );
  assert.match(
    readme,
    /`designPackage`, `designExports`, `designInputReview`, `productionPlan`,\s+`machineSchedule`, `machineSelection`, `manufacturingHandoff`, `qualityPlan`,\s+`toolingPlan`, `interventionMap`, `executionPlan`, `postprocessPlan`,\s+`automationRequirements`, `resolutionPlan`, and `machineRelease`/,
  );
  assert.match(readme, /execution stop points/);
  assert.match(readme, /unattended-run eligibility/);
  assert.match(readme, /postprocessor gates/);
  assert.match(readme, /`designExports` generated design export payloads/);
  assert.match(readme, /source previews, media types, blockers/);
  assert.match(readme, /design export state/);
  assert.match(readme, /CAD\/model\/slicer source assumptions/);
  assert.match(readme, /batch-planning\s+state/);
  assert.match(readme, /machine-schedule state/);
  assert.match(readme, /`machine-schedule`/);
  assert.match(readme, /operation windows/);
  assert.match(readme, /quality evidence targets/);
  assert.match(readme, /tooling\/setup\s+requirements/);
  assert.match(readme, /intervention\s+paths/);
  assert.match(readme, /machine-choice\s+alternatives/);
  assert.match(readme, /CAD\/CAM\s+handoff\s+assumptions/);
  assert.match(readme, /ordered release-blocking remediation steps/);
  assert.match(readme, /attempts machine-ready release/);
  assert.match(readme, /horizontal side-slot\/keyway milling/);
  assert.match(readme, /horizontal-milled side slots\/keyways/);
  assert.match(readme, /SLA\/MSLA resin print-wash-cure job sheets/);
  assert.match(readme, /SLS\/MJF-style powder-bed/);
  assert.match(readme, /laser, waterjet, plasma\/sheet cutters/);
  assert.match(readme, /waterjet cutter, plasma cutter/);
  assert.match(readme, /kerf\s+tests and fire\/fume gates/);
  assert.match(readme, /SLA resin printer, SLS powder-bed printer/);
  assert.match(readme, /laser cutter/);
  assert.match(readme, /sheet-cutting\s+kerf\/fire\/fume checks/);
  assert.match(readme, /method\s+combination preferences/);
  assert.match(readme, /additive-print\+milling/);
  assert.match(readme, /additive material\/color\/tool-change/);
  assert.match(readme, /slicer job\s+sheets/);
  assert.match(
    readme,
    /additive slicer profile\/support\/\s+orientation\/first-layer, mesh unit\/scale\/topology\/wall-thickness evidence,\s+high-speed kinematic evidence/,
  );
  assert.match(readme, /slicer profile\/support\/orientation\/first-layer evidence/);
  assert.match(readme, /mesh unit\/scale\/topology\/wall-thickness evidence/);
  assert.match(readme, /additive thin-wall geometry/);
  assert.match(
    readme,
    /resin\s+vat-capacity\/refill evidence, resin-handling\/postprocess evidence, powder-bed build profile\/powder lot\/nesting evidence, powder-handling\/cooldown-depowder evidence/,
  );
  assert.match(readme, /powder-bed build profile\/powder lot\/nesting evidence/);
  assert.match(readme, /resin exposure\/profile\/layer\/support evidence/);
  assert.match(readme, /resin\s+vat-capacity\/refill evidence/);
  assert.match(readme, /subtractive text setup\/process evidence/);
  assert.match(readme, /workholding\/datum\/tool-length/);
  assert.match(readme, /spindle\/feed\/coolant\/kerf\/pierce\/cut-chart/);
  assert.match(readme, /Submitted `existingInstructions` are analyzed beside generated drafts/);
  assert.match(readme, /resolved machine profile material lists/);
  assert.match(readme, /`improvements` and `improvedPrograms` review drafts/);
  assert.match(readme, /machine-ready release/);
  assert.match(readme, /nonzero axis counts/);
  assert.match(readme, /positioning-mode reset state/);
  assert.match(
    readme,
    /CNC program end while still in `G91` incremental positioning without `G90` reset/,
  );
  assert.match(readme, /additive relative-positioning extrusion state/);
  assert.match(readme, /printer\s+async-nozzle-wait state/);
  assert.match(readme, /async-bed-target re-wait state/);
  assert.match(readme, /nozzle-cooldown\/\s*reheat state/);
  assert.match(readme, /bed-cooldown\/\s*re-wait state/);
  assert.match(readme, /stepper-idle\/\s*re-home state/);
  assert.match(readme, /post-mode-switch extrusion reset state/);
  assert.match(readme, /negative-Z extrusion\/Z-offset probe state/);
  assert.match(readme, /material-capacity\/runout evidence/);
  assert.match(readme, /extrusion calibration\/flow\/pressure-advance evidence/);
  assert.match(readme, /firmware retraction\/recover settings evidence/);
  assert.match(readme, /high-speed input-shaper\/acceleration\/volumetric-flow evidence/);
  assert.match(
    readme,
    /missing slicer mesh unit\/scale\/watertight\/manifold\/normals\/wall-thickness evidence for STL\/3MF\/OBJ\/model inputs/,
  );
  assert.match(readme, /orientation\/first-layer, mesh unit\/scale\/topology\/wall-thickness evidence/);
  assert.match(
    readme,
    /slicer high-speed input-shaper\/acceleration\/volumetric-flow evidence/,
  );
  assert.match(readme, /chamber\/enclosure\/thermal-soak evidence for warp-prone filament/);
  assert.match(
    readme,
    /printer\s+async-nozzle-wait state, async-bed-target re-wait state, nozzle-cooldown\/\s+reheat state, bed-cooldown\/re-wait state, stepper-idle\/re-home state,\s+extrusion-mode\/reset state, post-mode-switch extrusion reset state,\s+negative-Z extrusion\/Z-offset probe state, filament lot\/dry-storage\s+conditioning evidence, material-capacity\/runout evidence,\s+extrusion calibration\/flow\/pressure-advance evidence,\s+firmware retraction\/recover settings evidence,\s+high-speed input-shaper\/acceleration\/volumetric-flow evidence,\s+chamber\/enclosure\/thermal-soak evidence for warp-prone filament,\s+bed-adhesion, first-layer, fan-timing/,
  );
  assert.match(
    readme,
    /after async `M104` nozzle targets without `M109` or verified hotend wait/,
  );
  assert.match(
    readme,
    /after async `M140` bed target changes without `M190` or verified bed wait/,
  );
  assert.match(readme, /after nozzle cooldown without reheat/);
  assert.match(readme, /after bed cooldown without re-wait/);
  assert.match(readme, /after\s+stepper idle without re-homing/);
  assert.match(
    readme,
    /missing filament lot\/dry-storage\/\s+dryer\/desiccant evidence before first\s+extrusion/,
  );
  assert.match(
    readme,
    /missing spool-weight\/remaining-filament\/runout-sensor evidence before long extrusion/,
  );
  assert.match(
    readme,
    /missing extrusion\s+calibration\/flow\/pressure-advance evidence before first extrusion/,
  );
  assert.match(
    readme,
    /missing\s+chamber\/enclosure\/thermal-soak evidence before first extrusion for ABS\/ASA\/PC\/nylon/,
  );
  assert.match(readme, /overhang, bridge, cantilever, thin-wall, snap-fit/);
  assert.match(readme, /resin\s+drain\/cupping geometry/);
  assert.match(readme, /missing bed-temperature waits or\s+re-waits/);
  assert.match(
    readme,
    /later `M82`\/`M83` extrusion-mode switches without renewed `G92 E`\s+reset evidence/,
  );
  assert.match(
    readme,
    /positive extrusion while `G91` relative axis positioning remains\s+active without `G90` or coordinate-state verification/,
  );
  assert.match(
    readme,
    /positive extrusion below\s+build-surface Z without measured Z-offset\/probe evidence/,
  );
  assert.match(
    readme,
    /missing `M82`\/`M83`\s+extrusion\s+mode and `G92 E0` reset state before\s+priming/,
  );
  assert.match(
    readme,
    /firmware `G10`\/`G11` retract\/unretract before `M207`\/`M208`\/`M209`/,
  );
  assert.match(readme, /first-layer adhesion setup/);
  assert.match(readme, /early\s+part-cooling fan timing/);
  assert.match(readme, /post-change extrusion without purge\/prime\/resume evidence/);
  assert.match(
    readme,
    /printer pauses before renewed position\/extrusion resume evidence/,
  );
  assert.match(
    readme,
    /selected-tool extrusion without `M104`\/`M109` or hotend temperature evidence/,
  );
  assert.match(
    readme,
    /high-speed FDM extrusion without input-shaper\/acceleration\/volumetric-flow evidence/,
  );
  assert.match(
    readme,
    /CNC subprogram calls, macro variables, conditionals, or jumps before controller dependency review evidence/,
  );
  assert.match(readme, /resin IPA\/wash\/cure\/drain\/PPE\/\s+waste controls/);
  assert.match(readme, /missing resin exposure\/profile\/layer\/support\/build-plate evidence/);
  assert.match(readme, /missing resin vat-volume\/level\/refill evidence for large resin jobs/);
  assert.match(readme, /missing resin postprocess evidence/);
  assert.match(
    readme,
    /powder\s+build profile\/powder lot\/nesting controls or missing powder-bed build\/profile evidence, cooldown\/depowder\/recovery controls/,
  );
  assert.match(readme, /missing powder-bed build\/profile evidence/);
  assert.match(readme, /missing powder-bed handling evidence/);
  assert.match(readme, /subtractive text setup\/process evidence/);
  assert.match(readme, /assembly fit\/metrology\/datum\/torque\/cure evidence/);
  assert.match(readme, /precision tolerance\/surface-finish metrology evidence/);
  assert.match(readme, /unattended\/batch monitoring and recovery evidence/);
  assert.match(readme, /thermal postprocess temperature\/fixture\/cooldown evidence/);
  assert.match(readme, /surface\/chemical finishing media\/masking\/PPE\/waste evidence/);
  assert.match(readme, /assembly\s+dry-fit\/metrology\/datum\/torque\/cure controls/);
  assert.match(readme, /missing assembly fit\/metrology evidence/);
  assert.match(readme, /missing precision tolerance\/surface-finish metrology evidence/);
  assert.match(readme, /missing unattended\/batch monitoring and recovery evidence/);
  assert.match(
    readme,
    /missing thermal postprocess temperature\/furnace\/atmosphere\/cooldown\/quench\/inspection evidence/,
  );
  assert.match(
    readme,
    /missing surface\/chemical finishing media\/masking\/PPE\/waste\/thickness\/inspection evidence/,
  );
  assert.match(
    readme,
    /sheet-cutting material\/thickness\/cut-chart\/recipe evidence, pierce\/kerf\/focus\/gas\/fume\/support, retained-tab\/microjoint\/part-release evidence, waterjet pressure\/abrasive-flow, and plasma work-clamp evidence/,
  );
  assert.match(
    readme,
    /missing sheet-cutting material\/thickness\/cut-chart recipe evidence/,
  );
  assert.match(readme, /CNC tool-change automation\/operator-load\/spindle-stop evidence/);
  assert.match(readme, /tool-length\/probe compensation\/cancel state/);
  assert.match(readme, /probing-cycle setup\/feed\/recovery state/);
  assert.match(readme, /cutter-compensation offset\/cancel state/);
  assert.match(readme, /lathe text part-off support evidence/);
  assert.match(readme, /lathe text threading feed-per-rev\/pitch-sync evidence/);
  assert.match(
    readme,
    /lathe text threading feed-per-rev\/pitch\/spindle-encoder evidence/,
  );
  assert.match(
    readme,
    /lathe text part-off catcher\/subspindle\/tailstock\/stock-support evidence/,
  );
  assert.match(
    readme,
    /mill\/router rapid\/feed negative-Z plunges after tool selection without\s+explicit `G43`\/probe\/tool-length state or later `M6` tool changes before `G49` cancellation/,
  );
  assert.match(
    readme,
    /`G41`\/`G42` cutter compensation without\s+`D` offset or tool radius\/diameter evidence or without `G40` cancellation before program end/,
  );
  assert.match(
    readme,
    /`M6` tool changes before ATC\/magazine\/\s+carousel\/operator-loaded evidence or while spindle\/process remains active without `M5`\/`M05` stop evidence/,
  );
  assert.match(readme, /mill\/router fixture\/hold-down evidence/);
  assert.match(readme, /cutting feed-rate\/cut-chart evidence/);
  assert.match(readme, /work-offset\/datum evidence/);
  assert.match(
    readme,
    /mill\/router\/lathe cutting feeds and mill\/router rapid negative-Z plunges before probed\s+datum\/touch-off\/edge-finder\/work-offset evidence/,
  );
  assert.match(
    readme,
    /mill\/router cutting feeds or rapid negative-Z plunges before\s+fixture\/vise\/clamp\/vacuum\/hold-down\/tab evidence/,
  );
  assert.match(
    readme,
    /cutting moves before positive\s+`F` feed-rate, chip-load, feeds-and-speeds, or cut-chart evidence/,
  );
  assert.match(
    readme,
    /`G31`\/`G38\.x` probing cycles before touch-probe calibration, skip\/contact input, safe-feed, and retract\/recovery evidence/,
  );
  assert.match(readme, /tool-life\/wear\/load-monitor evidence/);
  assert.match(
    readme,
    /long mill\/router\/lathe cutting feeds before tool-life, wear-inspection, fresh-edge, or load-monitor evidence/,
  );
  assert.match(readme, /spindle-speed\/direction\/start\/process-stop state/);
  assert.match(readme, /chip\/coolant\/dust-collection\s+state/);
  assert.match(readme, /lathe\s+chuck\/stick-out\/runout evidence/);
  assert.match(readme, /part-off catcher\/support evidence/);
  assert.match(readme, /tool\/turret-change stop state/);
  assert.match(readme, /tool-nose compensation evidence\/cancel state/);
  assert.match(
    readme,
    /missing\s+coolant, air blast,\s+dust\s+collection,\s+chip conveyor, or dry-machining approval\s+before cutting feed moves or after those systems are stopped/,
  );
  assert.match(
    readme,
    /sheet-cutter\s+feed\s+moves before\s+pierce\/kerf\/focus\/assist-gas\/fume\/support\s+evidence, outside-profile release cuts before retained-tab\/bridge\/microjoint\/catcher\/tip-up evidence, waterjet pump-pressure\/abrasive-flow evidence, plasma work-clamp\/ground-return evidence, or after assist-gas\/fume\/abrasive support media is stopped/,
  );
  assert.match(readme, /canned drilling\/tapping cycle setup\/cancel state/);
  assert.match(readme, /motion before `G80` cancellation/);
  assert.match(
    readme,
    /unsafe\s+canned\s+drilling\/peck\/tapping cycles with missing or nonpositive `R` retract planes or motion before `G80` cancellation/,
  );
  assert.match(
    readme,
    /mill\/router\/lathe `M3`\/`M4` spindle starts without positive `S` speed evidence or changes direction while active without `M5`\/`M05` stop evidence/,
  );
  assert.match(
    readme,
    /subtractive feed moves before spindle start or after\s+explicit `M5`\/`M05` process stop/,
  );
  assert.match(readme, /declared\s+material\/machine compatibility/);
  assert.match(readme, /declared material\s+incompatibility/);
  assert.match(readme, /lathe\s+chuck\/collet\/tailstock\/stick-out\/runout\s+evidence/);
  assert.match(
    readme,
    /part-off or cutoff operations without catcher\/subspindle\/tailstock\/stock-support evidence/,
  );
  assert.match(
    readme,
    /lathe `T` tool\/turret changes while spindle\/process remains active without `M5`\/`M05` stop evidence/,
  );
  assert.match(
    readme,
    /lathe `G41`\/`G42` tool-nose compensation without tool-nose radius\/geometry\/wear offset evidence or without `G40` cancellation before program end/,
  );
  assert.match(readme, /lathe\s+constant-surface-speed\s+without a spindle cap/);
  assert.match(
    readme,
    /threading cycles without feed-per-rev or pitch-synchronization evidence,\s+part-off/,
  );
  assert.match(
    readme,
    /mill\/router rapid negative-Z plunges before spindle\/process start or after explicit `M5`\/`M05` process stop without restart/,
  );
  assert.match(readme, /deep-cut,\s+arc-plane\/geometry/);
  assert.match(
    readme,
    /arc\s+moves before explicit `G17`\/`G18`\/`G19` plane evidence/,
  );
  assert.match(
    readme,
    /arc\s+moves before explicit `G17`\/`G18`\/`G19` plane evidence, with center offsets that do not match the selected plane, or without plane-matched `I`\/`J`, `I`\/`K`, or `J`\/`K` center offsets or `R` radius/,
  );
  assert.match(readme, /analysis-simulation-report/);
  assert.match(readme, /rotary\/index `A`\/`B`\/`C` axis words/);
  assert.match(readme, /`axisExtents` with\s+degree units/);
  assert.match(readme, /simulated-rotary-index-review/);
  assert.match(readme, /rotary-index-boundary/);
  assert.match(readme, /conservative `G2`\/`G3` arc sweeps/);
  assert.match(readme, /simulated-axis-envelope-exceeded/);
  assert.match(readme, /simulated-machine-envelope/);
  assert.match(readme, /simulated-rapid-below-clearance/);
  assert.match(readme, /simulated-rapid-clearance/);
  assert.match(readme, /GRBL-style router\s+profile programs with tab gates/);
  assert.match(readme, /CNC router/);
  assert.match(readme, /routed sheet\/profile parts/);
  assert.match(readme, /machine-envelope/);
  assert.match(readme, /dd\.remote\.fabrication\.requests/);
  assert.match(readme, /dd\.remote\.fabrication\.results/);
  assert.match(readme, /direct instruction-analysis payloads/);
  assert.match(readme, /fabrication\.instructions\.analyze/);
  assert.match(readme, /are published to the fabrication result subject/);
  assert.match(
    readme,
    /Compact learning outcomes fan\s+out `fabrication\.learning\.outcome\.result`/,
  );
  assert.match(readme, /FABRICATION_MDP_AUTOPUBLISH=true/);
  assert.match(readme, /default local port is `8113`/);

  assert.match(subjectSchema, /dd\.remote\.fabrication\.requests/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.results/);
  assert.match(subjectSchema, /"queueGroup": "dd-fabrication-server"/);

  assert.match(docs, /"path": "\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/plan"/);
  assert.match(docs, /"path": "\/capabilities"/);
  assert.match(docs, /"path": "\/fabrication\/capabilities"/);
  assert.match(docs, /"path": "\/schema"/);
  assert.match(docs, /"path": "\/fabrication\/schema"/);
  assert.match(docs, /"path": "\/examples"/);
  assert.match(docs, /"path": "\/fabrication\/examples"/);
  assert.match(docs, /"path": "\/instructions\/analyze"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/analyze"/);
  assert.match(docs, /"path": "\/jobs"/);
  assert.match(docs, /"path": "\/jobs\/:job_id"/);
  assert.match(docs, /"path": "\/jobs\/:job_id\/artifacts\/:artifact_id"/);
  assert.match(docs, /"path": "\/learning\/policy"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/policy"/);
  assert.match(docs, /"path": "\/learning\/observe"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/observe"/);
  assert.match(docs, /"path": "\/learning\/outcomes"/);
});

test('fabrication server is deployed through runtime manifests, gateway, and observability', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-fabrication-server.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-fabrication-server.service.yaml',
  );
  const networkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-fabrication-server.networkpolicy.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const availability = await readRepoFile('remote/argocd/dd-next-runtime/availability-pdbs.yaml');
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const remoteReadme = await readRepoFile('remote/readme.md');

  assert.match(deployment, /name:\s*dd-fabrication-server/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8113'/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /FABRICATION_REQUEST_SUBJECT[\s\S]*dd\.remote\.fabrication\.requests/);
  assert.match(deployment, /FABRICATION_QUEUE_GROUP[\s\S]*dd-fabrication-server/);
  assert.match(deployment, /FABRICATION_RESULT_SUBJECT[\s\S]*dd\.remote\.fabrication\.results/);
  assert.match(deployment, /FABRICATION_MDP_OPTIMIZE_SUBJECT[\s\S]*dd\.remote\.mdp\.optimize/);
  assert.match(deployment, /FABRICATION_MDP_AUTOPUBLISH[\s\S]*value:\s*'true'/);
  assert.match(deployment, /RUNTIME_CONFIG_APPLY_URL[\s\S]*dd-fabrication-server\.default\.svc\.cluster\.local:8113/);
  assert.match(deployment, /revisionHistoryLimit:\s*3/);
  assert.match(deployment, /topologySpreadConstraints:[\s\S]*topologyKey:\s*kubernetes\.io\/hostname/);
  assert.match(deployment, /podAntiAffinity:[\s\S]*preferredDuringSchedulingIgnoredDuringExecution/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-fabrication-server/);
  assert.match(service, /appProtocol:\s*http/);
  assert.match(service, /port:\s*8113/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(kustomization, /dd-fabrication-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-fabrication-server\.service\.yaml/);
  assert.match(kustomization, /dd-fabrication-server\.networkpolicy\.yaml/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /name:\s*dd-fabrication-server/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*Ingress[\s\S]*Egress/);
  assert.match(networkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(networkPolicy, /app:\s*dd-runtime-config/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*kube-system[\s\S]*port:\s*53/);
  assert.match(networkPolicy, /app:\s*dd-nats[\s\S]*port:\s*4222/);
  assert.match(networkPolicy, /app:\s*dd-runtime-config[\s\S]*port:\s*8110/);
  assert.match(networkPolicy, /cidr:\s*0\.0\.0\.0\/0[\s\S]*port:\s*443/);
  assert.match(availability, /name:\s*dd-fabrication-server[\s\S]*minAvailable:\s*1/);
  assert.match(gateway, /location = \/fabrication[\s\S]*return 302 \/fabrication\//);
  assert.match(gateway, /location = \/fabrication[\s\S]*add_header X-Request-ID \$request_id always/);
  assert.match(gateway, /location = \/fabrication[\s\S]*error_page 405 = @fabrication_method_not_allowed/);
  assert.match(gateway, /location = \/fabrication\/internal[\s\S]*add_header X-Request-ID \$request_id always[\s\S]*return 404 '\{"error":"not_found"/);
  assert.match(gateway, /location \^~ \/fabrication\/internal\/[\s\S]*add_header X-Request-ID \$request_id always[\s\S]*return 404 '\{"error":"not_found"/);
  assert.match(gateway, /location @auth_required[\s\S]*add_header X-Request-ID \$request_id always/);
  assert.match(
    gateway,
    /location \/fabrication\/[\s\S]*add_header X-Request-ID \$request_id always[\s\S]*dd-fabrication-server\.default\.svc\.cluster\.local:8113\//,
  );
  assert.match(gateway, /location \/fabrication\/[\s\S]*error_page 405 = @fabrication_method_not_allowed/);
  assert.match(gateway, /location \/fabrication\/[\s\S]*error_page 413 = @fabrication_payload_too_large/);
  assert.match(gateway, /location \/fabrication\/[\s\S]*error_page 429 = @fabrication_rate_limited/);
  assert.match(gateway, /location @fabrication_method_not_allowed[\s\S]*return 405 '\{"error":"method_not_allowed"/);
  assert.match(gateway, /location @fabrication_payload_too_large[\s\S]*return 413 '\{"error":"payload_too_large"/);
  assert.match(gateway, /location @fabrication_rate_limited[\s\S]*return 429 '\{"error":"rate_limited"/);
  assert.match(
    prometheus,
    /job_name:\s*dd-fabrication-server[\s\S]*dd-fabrication-server\.default\.svc\.cluster\.local:8113/,
  );
  assert.match(
    otel,
    /job_name:\s*dd-fabrication-server[\s\S]*dd-fabrication-server\.default\.svc\.cluster\.local:8113/,
  );
  assert.match(home, /dd-fabrication-server/);
  assert.match(home, /\/fabrication\/jobs/);
  assert.match(home, /POST \/fabrication\/plan/);
  assert.match(home, /label: FABRICATION_REQUESTS_SUBJECT/);
  assert.match(home, /label: FABRICATION_RESULTS_SUBJECT/);
  assert.match(runtimeReadme, /dd-fabrication-server/);
  assert.match(runtimeReadme, /\/fabrication\/capabilities/);
  assert.match(runtimeReadme, /\/fabrication\/schema/);
  assert.match(runtimeReadme, /\/fabrication\/examples/);
  assert.match(runtimeReadme, /\/fabrication\/jobs/);
  assert.match(runtimeReadme, /`POST \/fabrication\/plan`/);
  assert.match(runtimeReadme, /`POST \/fabrication\/instructions\/analyze`/);
  assert.match(runtimeReadme, /Gateway-generated `\/fabrication` redirects/);
  assert.match(runtimeReadme, /return `X-Request-ID`/);
  assert.match(runtimeReadme, /JSON `not_found` 404/);
  assert.match(runtimeReadme, /JSON `method_not_allowed` 405/);
  assert.match(runtimeReadme, /JSON `payload_too_large` 413/);
  assert.match(runtimeReadme, /JSON `rate_limited` 429/);
  assert.match(runtimeReadme, /explicit runtime hardening/);
  assert.match(runtimeReadme, /dedicated NetworkPolicy/);
  assert.match(remoteReadme, /fabrication-server-rs/);
});
