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

function resultReviewFunctionBodies(source: string): Array<{ name: string; body: string }> {
  const resultFunctions = Array.from(
    source.matchAll(/\nfn\s+([a-z0-9_]+_result_review_response)\s*\(/g),
  );

  return resultFunctions.map((match) => {
    const start = match.index ?? 0;
    const nextFunction = source.indexOf('\nfn ', start + 1);
    return {
      name: match[1],
      body: source.slice(start, nextFunction === -1 ? source.length : nextFunction),
    };
  });
}

function assertResultReviewLearningOutcomeDraftCoverage(source: string): void {
  const resultFunctions = resultReviewFunctionBodies(source);
  assert.ok(
    resultFunctions.length >= 40,
    `expected broad result-review endpoint coverage, found ${resultFunctions.length}`,
  );

  const missingDrafts = resultFunctions
    .filter(({ body }) => !/learning-outcome-draft/.test(body))
    .map(({ name }) => name);
  assert.deepEqual(missingDrafts, []);
}

function assertLearningOutcomeDraftSubmitCoverage(source: string, readme: string): void {
  assert.match(source, /#\[serde\(alias = "sourceRequestId"\)\]\s+request_id: Option<String>/);
  assert.match(source, /#\[serde\(alias = "sourceJobId"\)\]\s+job_id: Option<String>/);
  assert.match(source, /#\[serde\(alias = "rewardHint"\)\]\s+reward: Option<f64>/);
  assert.match(source, /#\[serde\(flatten\)\]\s+extra: BTreeMap<String, Value>/);
  assert.match(source, /fn outcome_draft_hint_observations/);
  assert.match(source, /fn outcome_draft_manufacturing_methods/);
  assert.match(source, /fn outcome_draft_assembly_strategy/);
  assert.match(source, /fn learning_outcome_record_accepts_result_outcome_draft_payloads/);
  assert.match(source, /fn outcome_drafts_teach_future_hybrid_split_combine_plans/);
  assert.match(readme, /They also accept the `learning\.outcomeDraft` payloads emitted by/);
  assert.match(readme, /`sourceRequestId`, `sourceJobId`, and `rewardHint`/);
  assert.match(readme, /`manufacturingMethodHints` can seed learned method preferences/);
  assert.match(readme, /`joinKindHints` and\s+`splitCombineHints` to seed learned assembly strategies/);
}

function fabricationRootRoutes(source: string): Set<string> {
  const rootMatch = source.match(/async fn root\(\)[\s\S]*?let routes = vec!\[([\s\S]*?)\];/);
  assert.ok(rootMatch, 'expected root route inventory in dd-fabrication-server');

  return new Set(
    Array.from(rootMatch[1].matchAll(/"([^"]+)"/g), (match) => match[1]).filter((route) =>
      route.startsWith('GET /') || route.startsWith('POST /'),
    ),
  );
}

function registeredFabricationRoutes(source: string): Set<string> {
  return new Set(
    Array.from(
      source.matchAll(/\.route\(\s*"([^"]+)"\s*,\s*(get|post)\(/g),
      (match) => `${match[2].toUpperCase()} ${match[1]}`,
    ),
  );
}

function assertRootRouteInventoryCoversRegisteredRoutes(source: string): void {
  const rootRoutes = fabricationRootRoutes(source);
  const registeredRoutes = registeredFabricationRoutes(source);
  assert.ok(rootRoutes.size >= 300, `expected broad root route inventory, found ${rootRoutes.size}`);
  assert.ok(
    registeredRoutes.size >= 300,
    `expected broad Axum route registration inventory, found ${registeredRoutes.size}`,
  );

  const missingRoutes = Array.from(registeredRoutes)
    .filter((route) => !rootRoutes.has(route))
    .sort();

  assert.deepEqual(missingRoutes, []);
}

function dashboardFabricationPathLiterals(grafanaDashboards: string): Set<string> {
  const decodedPaths = Array.from(
    grafanaDashboards.matchAll(/\\\/fabrication(?:\\\/[A-Za-z0-9._:-]+)+/g),
    (match) => match[0].replaceAll('\\/', '/'),
  );
  const plainPaths = Array.from(
    grafanaDashboards.matchAll(/\/fabrication(?:\/[A-Za-z0-9._:-]+)+/g),
    (match) => match[0],
  );

  return new Set([...decodedPaths, ...plainPaths]);
}

function assertGrafanaCoversFabricationRootRoutes(source: string, grafanaDashboards: string): void {
  const rootPaths = new Set(
    Array.from(fabricationRootRoutes(source), (route) => route.replace(/^(GET|POST) /, '')).filter(
      (path) => path.startsWith('/fabrication/'),
    ),
  );
  const dashboardPaths = dashboardFabricationPathLiterals(grafanaDashboards);
  const patternCoveredPaths = new Set([
    '/fabrication/jobs/:job_id',
    '/fabrication/jobs/:job_id/artifacts/:artifact_id',
    '/fabrication/jobs/:job_id/release-bundle',
  ]);
  const missingPaths = Array.from(rootPaths)
    .filter((path) => !dashboardPaths.has(path) && !patternCoveredPaths.has(path))
    .sort();

  assert.deepEqual(missingPaths, []);
  assert.match(grafanaDashboards, /job detail/);
  assert.match(grafanaDashboards, /artifact detail fetch/);
  assert.match(grafanaDashboards, /\/release-bundle/);
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

  assertResultReviewLearningOutcomeDraftCoverage(source);
  assertLearningOutcomeDraftSubmitCoverage(source, readme);
  assertRootRouteInventoryCoversRegisteredRoutes(source);

  assert.match(cargo, /name\s*=\s*"dd-fabrication-server"/);
  assert.match(cargo, /async-nats\s*=\s*"=0\.38\.0"/);
  assert.match(cargo, /des_engine\s*=\s*\{\s*path\s*=\s*"[^"]*discrete-event-system\.rs"/);
  assert.match(cargo, /dd-nats-subject-defs\s*=\s*\{\s*path/);
  assert.match(
    source,
    /use dd_nats_subject_defs::\{[\s\S]*?FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP[\s\S]*?FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT[\s\S]*?FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT[\s\S]*?FABRICATION_REQUESTS_QUEUE_GROUP[\s\S]*?FABRICATION_REQUESTS_SUBJECT[\s\S]*?FABRICATION_RESULTS_SUBJECT[\s\S]*?MDP_OPTIMIZE_SUBJECT[\s\S]*?RUNTIME_EVENTS_SUBJECT[\s\S]*?\};/,
  );
  assert.match(source, /const SCHEMA_VERSION: &str = "fabrication\.plan\.v1"/);
  assert.match(source, /struct FabricationPlanRequest/);
  assert.match(source, /struct InstructionAnalysisRequest/);
  assert.match(source, /learning: Option<LearningHints>/);
  assert.match(source, /struct InstructionAnalysisResponse[\s\S]*instruction_intent_map: InstructionIntentMap[\s\S]*learning: LearningPlan/);
  assert.match(source, /struct InstructionIntentMap/);
  assert.match(source, /review_priorities: Vec<InstructionReviewPriority>/);
  assert.match(source, /struct InstructionReviewPriority/);
  assert.match(source, /struct ProgramInstructionIntent/);
  assert.match(source, /fn instruction_intent_map/);
  assert.match(source, /fn instruction_review_priorities/);
  assert.match(source, /machine-failure-boundary-first/);
  assert.match(source, /human-intervention-required/);
  assert.match(source, /split-combine-or-interface-review/);
  assert.match(source, /non-gcode-job-sheet-evidence/);
  assert.match(source, /learning-feedback-after-disposition/);
  assert.match(source, /dd\.fabrication\.instruction-intent-map\.v1/);
  assert.match(source, /instruction-intent:/);
  assert.match(source, /analysis-instruction-intent-map/);
  assert.match(source, /"instructionIntentMap": &response\.instruction_intent_map/);
  assert.match(source, /struct ImprovedInstructionProgram[\s\S]*patch_manifest: InstructionPatchManifest/);
  assert.match(source, /struct InstructionPatchManifest/);
  assert.match(source, /struct InstructionPatchOperation/);
  assert.match(source, /fn instruction_patch_manifest/);
  assert.match(source, /review_summary: Value/);
  assert.match(source, /fn instruction_patch_review_summary/);
  assert.match(source, /fn instruction_patch_review_category/);
  assert.match(source, /"reviewSummary"/);
  assert.match(source, /blocked-pending-human-review/);
  assert.match(source, /fn instruction_patch_learning_actions/);
  assert.match(source, /fn instruction_patch_learning_observations/);
  assert.match(source, /dd\.fabrication\.instruction-patch-manifest\.v1/);
  assert.match(source, /fn instruction_improvement_review_response/);
  assert.match(source, /async fn instruction_improve_http/);
  assert.match(source, /dd\.fabrication\.instruction-improvement-review\.v1/);
  assert.match(source, /instructionImprovementRoutes/);
  assert.match(source, /instruction_improvement_review_endpoint_returns_patch_manifest_contract/);
  assert.match(source, /instruction-patch:/);
  assert.match(source, /apply-instruction-patch-/);
  assert.match(source, /"patchManifest": program\.patch_manifest/);
  assert.match(source, /struct FabricationOutcomeRequest/);
  assert.match(source, /struct FabricationLearningResponse/);
  assert.match(source, /struct OutcomeRemediationPlan/);
  assert.match(source, /struct OutcomeRootCause/);
  assert.match(source, /struct OutcomeRemediationAction/);
  assert.match(source, /struct LearningOutcomeRequest/);
  assert.match(source, /struct LearningPolicySnapshot/);
  assert.match(source, /struct LearningRemediationRisk/);
  assert.match(source, /remediation_risks: Vec<LearningRemediationRisk>/);
  assert.match(source, /machine_kind_preferences: Vec<LearningPreference>/);
  assert.match(source, /operation_sequence_preferences: Vec<LearningPreference>/);
  assert.match(source, /struct LearningPlan/);
  assert.match(source, /struct LearningEngineMetadata/);
  assert.match(source, /struct LearningMdpEnginePolicy/);
  assert.match(
    source,
    /use des_engine::\{[\s\S]*solve_mdp[\s\S]*solve_pomdp_underlying[\s\S]*MdpSpec[\s\S]*PomdpSpec[\s\S]*MDP_SCHEMA[\s\S]*POMDP_SCHEMA[\s\S]*NeuralNetworkLike[\s\S]*ActivationName[\s\S]*DenseLayerConfig[\s\S]*FeedForwardNetwork[\s\S]*analyze_model_spec[\s\S]*StudioModelSpec[\s\S]*STUDIO_GRAPH_SCHEMA[\s\S]*sdk as des_sdk[\s\S]*\};/,
  );
  assert.match(source, /engine: LearningEngineMetadata/);
  assert.match(source, /engine_policy: LearningMdpEnginePolicy/);
  assert.match(source, /engine: learning_engine_metadata\(\)/);
  assert.match(source, /let engine_policy = learning_mdp_engine_policy\(&mdp_states, &actions\)/);
  assert.match(source, /fn learning_mdp_engine_policy/);
  assert.match(source, /fn des_learning_mdp_spec/);
  assert.match(source, /fn des_learning_mdp_solution/);
  assert.match(source, /solve_pomdp_underlying/);
  assert.match(source, /PomdpSpec/);
  assert.match(source, /fn des_learning_pomdp_spec/);
  assert.match(source, /fn des_learning_pomdp_solution/);
  assert.match(source, /"learningEngine": &response\.learning\.engine/);
  assert.match(source, /"desMdpSpec": des_mdp_spec/);
  assert.match(source, /"desMdpSolution": des_mdp_solution/);
  assert.match(source, /"desPomdpSpec": des_pomdp_spec/);
  assert.match(source, /"desPomdpSolution": des_pomdp_solution/);
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
  assert.match(source, /struct BoundarySplitCombineInterfacePlan/);
  assert.match(source, /struct BoundaryAutomationPath/);
  assert.match(source, /struct MachineSelectionTrace/);
  assert.match(source, /struct MachineSelectionCandidate/);
  assert.match(source, /struct ManufacturingHandoff/);
  assert.match(source, /struct MaterialPlan/);
  assert.match(source, /struct MaterialRouteRequirement/);
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
  assert.match(source, /struct FixturePlan/);
  assert.match(source, /struct FixtureSetupPlan/);
  assert.match(source, /struct FixtureDatumTransfer/);
  assert.match(source, /struct MonitoringPlan/);
  assert.match(source, /struct MonitoringPoint/);
  assert.match(source, /struct MonitoringAlertRule/);
  assert.match(source, /struct ProductionPlan/);
  assert.match(source, /struct ProductionBatch/);
  assert.match(source, /struct MachineSchedule/);
  assert.match(source, /struct MachineScheduleLane/);
  assert.match(source, /struct MachineScheduleOperation/);
  assert.match(source, /struct MachineScheduleHold/);
  assert.match(source, /struct FabricationDesScheduleModel/);
  assert.match(source, /struct FabricationDesScheduleLaneModel/);
  assert.match(source, /des_schedule_model: FabricationDesScheduleModel/);
  assert.match(source, /fn fabrication_des_schedule_model/);
  assert.match(source, /struct FabricationDesInstructionModel/);
  assert.match(source, /struct FabricationDesInstructionProgramModel/);
  assert.match(source, /des_instruction_model: FabricationDesInstructionModel/);
  assert.match(source, /fn fabrication_des_instruction_model/);
  assert.match(source, /StudioBlockKind::Queue/);
  assert.match(source, /analyze_model_spec\(&model_spec\)/);
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
  assert.match(source, /struct SimulationRiskProfile/);
  assert.match(source, /struct SimulationProgramRisk/);
  assert.match(source, /risk_profile: SimulationRiskProfile/);
  assert.match(source, /fn simulation_risk_profile/);
  assert.match(source, /struct AssemblyGraph/);
  assert.match(source, /struct AssemblyInterface/);
  assert.match(source, /struct AssemblySequenceStep/);
  assert.match(source, /struct HybridMakePlan/);
  assert.match(source, /struct HybridPartRoute/);
  assert.match(source, /struct HybridJoinOperation/);
  assert.match(source, /struct HybridSplitCombineDecision/);
  assert.match(source, /hybrid_make_plan: HybridMakePlan/);
  assert.match(source, /fn hybrid_make_plan/);
  assert.match(source, /struct NeuralPolicySketch/);
  assert.match(source, /struct NeuralEngineInference/);
  assert.match(source, /engine_inference: NeuralEngineInference/);
  assert.match(source, /struct NeuralTrainingCorpus/);
  assert.match(source, /struct NeuralTrainingExample/);
  assert.match(source, /struct NeuralInferenceCandidate/);
  assert.match(source, /fn fabrication_neural_engine_network/);
  assert.match(source, /fn neural_engine_inference/);
  assert.match(source, /FeedForwardNetwork::new/);
  assert.match(source, /ActivationName::Sigmoid/);
  assert.match(source, /fn neural_policy_sketch/);
  assert.match(source, /\.predict\(&feature_vector\)/);
  assert.match(source, /fn neural_training_corpus/);
  assert.match(source, /fn boundary_training_feature_vector/);
  assert.match(source, /fn boundary_training_labels/);
  assert.match(source, /fn boundary_training_reward_hint/);
  assert.match(source, /source: "validation-boundary"/);
  assert.match(source, /resolution-action:/);
  assert.match(source, /fn instruction_patch_training_feature_vector/);
  assert.match(source, /fn instruction_patch_training_labels/);
  assert.match(source, /source: "instruction-patch"/);
  assert.match(source, /instruction-patch-action:/);
  assert.match(source, /patch-action:/);
  assert.match(source, /boundary_learning_examples: Vec<String>/);
  assert.match(source, /fn observation_has_boundary_learning_signal/);
  assert.match(source, /fn boundary_learning_example/);
  assert.match(source, /boundary-memory/);
  assert.match(source, /learned-boundary-memory/);
  assert.match(source, /fn pomdp_belief_state/);
  assert.match(source, /fn strategy_candidates/);
  assert.match(source, /fn learning_engine_metadata/);
  assert.match(source, /fn des_learning_mdp_spec/);
  assert.match(source, /fn des_learning_mdp_solution/);
  assert.match(source, /fn des_learning_pomdp_spec/);
  assert.match(source, /fn des_learning_pomdp_solution/);
  assert.match(source, /solve_mdp\(spec, MdpMethod::ValueIteration\)/);
  assert.match(source, /solve_pomdp_underlying\(spec\)/);
  assert.match(source, /fn plan_fabrication\(request: FabricationPlanRequest\)/);
  assert.match(source, /fn plan_fabrication_with_policy/);
  assert.match(source, /fn apply_learning_policy_to_request/);
  assert.match(source, /fn learned_preferred_methods/);
  assert.match(source, /fn learned_preferred_assembly_strategy/);
  assert.match(source, /fn learned_remediation_risks/);
  assert.match(source, /fn learned_boundary_memory/);
  assert.match(source, /learned-remediation-risk/);
  assert.match(source, /avoid-learned-risk/);
  assert.match(source, /fn learned_remediation_risk_observations/);
  assert.match(source, /learned_remediation_risk_count/);
  assert.match(
    source,
    /learned-remediation-risk:review-prior-failure-outcome-before-release/,
  );
  assert.match(source, /preferred_assembly_strategy/);
  assert.match(source, /fn assembly_graph/);
  assert.match(source, /assembly_graph: AssemblyGraph/);
  assert.match(source, /fn process_graph/);
  assert.match(source, /process_graph: ProcessGraph/);
  assert.match(source, /fn intervention_map/);
  assert.match(source, /fn fallback_boundary_process_link/);
  assert.match(source, /fn boundary_split_combine_interface_plan/);
  assert.match(source, /intervention_map: BoundaryInterventionMap/);
  assert.match(source, /program_boundaries: Vec<ProgramBoundaryTrace>/);
  assert.match(source, /human_intervention_points: Vec<BoundaryHumanInterventionPoint>/);
  assert.match(source, /split_combine_decisions: Vec<BoundarySplitCombineDecision>/);
  assert.match(source, /interface_plan: BoundarySplitCombineInterfacePlan/);
  assert.match(source, /split-combine-interface/);
  assert.match(source, /boundary-decomposition-fit-and-release-check/);
  assert.match(source, /automation_paths: Vec<BoundaryAutomationPath>/);
  assert.match(source, /fn machine_selection_trace/);
  assert.match(source, /machine_selection: Vec<MachineSelectionTrace>/);
  assert.match(source, /"machine-selection"/);
  assert.match(source, /"machineSelection": response\.machine_selection/);
  assert.match(source, /review-operation-gap/);
  assert.match(source, /viable-alternative/);
  assert.match(source, /fn manufacturing_handoff/);
  assert.match(source, /manufacturing_handoff: ManufacturingHandoff/);
  assert.match(source, /fn material_plan/);
  assert.match(source, /fn material_feedstock_kind/);
  assert.match(source, /material_plan: MaterialPlan/);
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
  assert.match(source, /fn fixture_plan/);
  assert.match(source, /fn fixture_plan_learning_actions/);
  assert.match(source, /fixture_plan: FixturePlan/);
  assert.match(source, /fn monitoring_plan/);
  assert.match(source, /fn monitoring_plan_learning_actions/);
  assert.match(source, /monitoring_plan: MonitoringPlan/);
  assert.match(source, /fn decomposition_plan\(/);
  assert.match(source, /fn decomposition_plan_learning_actions/);
  assert.match(source, /decomposition_plan: DecompositionPlan/);
  assert.match(source, /fn production_plan/);
  assert.match(source, /production_plan: ProductionPlan/);
  assert.match(source, /fn machine_schedule/);
  assert.match(source, /machine_schedule: MachineSchedule/);
  assert.match(source, /dependency_holds: Vec<MachineScheduleHold>/);
  assert.match(source, /fn machine_release_report/);
  assert.match(source, /release_probe_plan: Option<&ReleaseProbePlan>/);
  assert.match(source, /source: "release-probe"\.to_string\(\)/);
  assert.match(source, /item: "release-probes"\.to_string\(\)/);
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
  assert.match(source, /"material-plan"/);
  assert.match(source, /dd\.fabrication\.material-plan\.v1/);
  assert.match(source, /material-route:/);
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
  assert.match(source, /"fixture-plan"/);
  assert.match(source, /dd\.fabrication\.fixture-plan\.v1/);
  assert.match(source, /fixture-setup:/);
  assert.match(source, /fixture-datum-transfer:/);
  assert.match(source, /"monitoring-plan"/);
  assert.match(source, /dd\.fabrication\.monitoring-plan\.v1/);
  assert.match(source, /monitoring-route:/);
  assert.match(source, /monitoring-alert:/);
  assert.match(source, /"decomposition-plan"/);
  assert.match(source, /dd\.fabrication\.decomposition-plan\.v1/);
  assert.match(source, /decomposition-target:/);
  assert.match(source, /decomposition-route:/);
  assert.match(source, /decomposition-release-gate:/);
  assert.match(source, /"release-package-plan"/);
  assert.match(source, /dd\.fabrication\.release-package-plan\.v1/);
  assert.match(source, /struct ReleasePackagePlan/);
  assert.match(source, /struct ReleasePackage/);
  assert.match(source, /struct ReleasePackageGate/);
  assert.match(source, /fn release_package_plan\(/);
  assert.match(source, /instruction_program_ids: Vec<String>/);
  assert.match(source, /"imported-instruction-release"/);
  assert.match(source, /"instruction-programs"\.to_string\(\)/);
  assert.match(source, /fn release_package_plan_learning_actions/);
  assert.match(source, /release-package:/);
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
  assert.match(source, /"controller-plan"/);
  assert.match(source, /dd\.fabrication\.controller-plan\.v1/);
  assert.match(source, /struct ControllerPlan/);
  assert.match(source, /struct ControllerCompatibilityTarget/);
  assert.match(source, /struct ControllerDialectSummary/);
  assert.match(source, /struct ControllerReleaseGate/);
  assert.match(source, /fn controller_plan\(/);
  assert.match(source, /fn controller_plan_learning_actions/);
  assert.match(source, /"analysis-learning-plan"/);
  assert.match(source, /"analysis-pomdp-belief-state"/);
  assert.match(source, /"analysis-release-probe-plan"/);
  assert.match(source, /"analysis-neural-training-corpus"/);
  assert.match(source, /"analysis-mdp-request"/);
  assert.match(source, /"pomdp-belief-state"/);
  assert.match(source, /dd\.fabrication\.pomdp-belief-state\.v1/);
  assert.match(source, /struct ReleaseProbePlan/);
  assert.match(source, /struct ReleaseProbe/);
  assert.match(source, /fn release_probe_plan/);
  assert.match(source, /fn release_probe_is_blocker/);
  assert.match(source, /"release-probe-plan"/);
  assert.match(source, /dd\.fabrication\.release-probe-plan\.v1/);
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
  assert.match(source, /struct DesignInputConversionStep/);
  assert.match(source, /conversion_plan: Vec<DesignInputConversionStep>/);
  assert.match(source, /fn design_input_conversion_step/);
  assert.match(source, /fn design_input_conversion_required_evidence/);
  assert.match(source, /FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT/);
  assert.match(source, /FABRICATION_DESIGN_SYNTHESIS_REQUESTS_SUBJECT/);
  assert.match(source, /FABRICATION_DESIGN_SYNTHESIS_RESULTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_GENERATION_REQUESTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_GENERATION_RESULTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_GENERATION_REQUESTS_QUEUE_GROUP/);
  assert.match(source, /FABRICATION_INSTRUCTION_REVIEW_REQUESTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_REVIEW_RESULTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_REVIEW_REQUESTS_QUEUE_GROUP/);
  assert.match(source, /FABRICATION_INSTRUCTION_SIMULATION_REQUESTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_SIMULATION_RESULTS_SUBJECT/);
  assert.match(source, /FABRICATION_INSTRUCTION_SIMULATION_REQUESTS_QUEUE_GROUP/);
  assert.match(source, /FABRICATION_RELEASE_READINESS_REQUESTS_SUBJECT/);
  assert.match(source, /FABRICATION_RELEASE_READINESS_RESULTS_SUBJECT/);
  assert.match(source, /FABRICATION_RELEASE_READINESS_REQUESTS_QUEUE_GROUP/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.design\.conversion\.requests/);
  assert.match(subjectSchema, /dd-fabrication-design-converters/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.design\.conversion\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.design\.synthesis\.requests/);
  assert.match(subjectSchema, /dd-fabrication-design-synthesizers/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.design\.synthesis\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.instructions\.generation\.requests/);
  assert.match(subjectSchema, /dd-fabrication-instruction-generators/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.instructions\.generation\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.instructions\.review\.requests/);
  assert.match(subjectSchema, /dd-fabrication-instruction-reviewers/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.instructions\.review\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.instructions\.simulation\.requests/);
  assert.match(subjectSchema, /dd-fabrication-instruction-simulators/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.instructions\.simulation\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.release\.readiness\.requests/);
  assert.match(subjectSchema, /dd-fabrication-release-gates/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.release\.readiness\.results/);
  assert.match(source, /professional-cad-converter/);
  assert.match(source, /lightweight-cad-pmi-inspector/);
  assert.match(source, /cad-kernel-inspector/);
  assert.match(source, /sheet-profile-cad-inspector/);
  assert.match(source, /color-mesh-package-inspector/);
  assert.match(source, /slicer-profile-reviewer/);
  assert.match(source, /"slicer": "Lychee Slicer"/);
  assert.match(source, /"slicer": "Chitubox"/);
  assert.match(source, /lychee-slicer-project/);
  assert.match(source, /chitubox-project/);
  assert.match(source, /resin-exposure:\*/);
  assert.match(source, /fn slicer_profile_catalog_response/);
  assert.match(source, /fn slicer_profile_catalog_entries/);
  assert.match(source, /dd\.fabrication\.slicer-profile-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/slicers\/catalog"/);
  assert.match(source, /"slicerProfileCatalog"/);
  assert.match(source, /slicer_profile_catalog_endpoint_exposes_profile_evidence_and_release_policy/);
  assert.match(source, /struct SlicerProfileResultReviewRequest/);
  assert.match(source, /struct SlicerProfileResultCheck/);
  assert.match(source, /struct SlicerProfileResultPreparation/);
  assert.match(source, /async fn slicer_profile_result_http/);
  assert.match(source, /fn slicer_profile_result_review_response/);
  assert.match(source, /fn store_slicer_profile_result_response/);
  assert.match(source, /dd\.fabrication\.slicer-profile-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/slicers\/result"/);
  assert.match(source, /slicer-profile-result-print-preparation-release-blocked/);
  assert.match(source, /dd\.fabrication\.slicer-profile-learning-outcome-draft\.v1/);
  assert.match(source, /"sourceKind": "slicer-profile-result"/);
  assert.match(source, /"preparationHints": print_preparation/);
  assert.match(source, /"machineCodeCheckHints": machine_code_checks/);
  assert.match(source, /"printPreparationBlockerCount": preparation_blocker_count/);
  assert.match(
    source,
    /slicer_profile_result_endpoint_reviews_print_prep_artifacts_and_learning/,
  );
  assert.match(source, /fn mesh_repair_catalog_response/);
  assert.match(source, /fn mesh_repair_catalog_entries/);
  assert.match(source, /dd\.fabrication\.mesh-repair-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/mesh-repair\/catalog"/);
  assert.match(source, /"meshRepairCatalog"/);
  assert.match(source, /mesh_repair_catalog_endpoint_exposes_repair_evidence_and_learning_policy/);
  assert.match(source, /struct MeshRepairResultReviewRequest/);
  assert.match(source, /async fn mesh_repair_result_http/);
  assert.match(source, /fn mesh_repair_result_review_response/);
  assert.match(source, /fn store_mesh_repair_result_response/);
  assert.match(source, /dd\.fabrication\.mesh-repair-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.mesh-repair-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/mesh-repair\/result"/);
  assert.match(source, /mesh-repair-result-topology-release-blocked/);
  assert.match(source, /"topologyHints"/);
  assert.match(source, /"dimensionalHints"/);
  assert.match(source, /"orientationHints"/);
  assert.match(source, /mesh-repair-dimensional-reviews/);
  assert.match(source, /mesh-repair-learning-observations/);
  assert.match(
    source,
    /mesh_repair_result_endpoint_reviews_topology_drift_split_and_learning/,
  );
  assert.match(source, /\.route\("\/mesh-repair\/result", post\(mesh_repair_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/mesh-repair\/result",\s*post\(mesh_repair_result_http\),?\s*\)/,
  );
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
  assert.match(source, /jt-lightweight-cad/);
  assert.match(source, /neutral-lightweight-cad/);
  assert.match(source, /supported-lightweight-cad-pmi-review-required/);
  assert.match(source, /verify-jt-units-assembly-pmi-brep-or-tessellation/);
  assert.match(source, /regenerate-step-3mf-stl-or-cam-setup-from-jt/);
  assert.match(source, /parasolid-kernel/);
  assert.match(source, /acis-kernel/);
  assert.match(source, /neutral-kernel-cad/);
  assert.match(source, /supported-cad-kernel-review-required/);
  assert.match(source, /verify-kernel-version-units-solids-and-body-count/);
  assert.match(source, /regenerate-step-3mf-stl-or-cam-setup-from-kernel/);
  assert.match(source, /dxf-profile/);
  assert.match(source, /dwg-profile/);
  assert.match(source, /neutral-2d-cad/);
  assert.match(source, /supported-2d-profile-review-required/);
  assert.match(source, /verify-units-layers-closed-contours-kerf-and-revision/);
  assert.match(source, /regenerate-reviewed-dxf-svg-or-cam-setup-before-cutting/);
  assert.match(source, /ply-color-scan-mesh/);
  assert.match(source, /vrml-wrl-color-mesh/);
  assert.match(source, /gltf-glb-color-mesh/);
  assert.match(source, /amf-additive-package/);
  assert.match(source, /neutral-color-mesh/);
  assert.match(source, /supported-color-mesh-review-required/);
  assert.match(source, /verify-color-material-texture-scale-and-manifoldness/);
  assert.match(source, /regenerate-color-aware-3mf-or-reviewed-mesh-before-slicing/);
  assert.match(source, /design-input-review/);
  assert.match(source, /fn sanitize_design_source_uri/);
  assert.match(source, /fn token_parts_contain/);
  assert.match(source, /fn design_source_extension/);
  assert.match(source, /plan_reviews_professional_open_artistic_and_slicer_design_inputs/);
  assert.match(source, /design_input_review_hardens_ambiguous_extensions_and_redacts_uris/);
  assert.match(source, /"qualityPlan": response\.quality_plan/);
  assert.match(source, /"toolingPlan": response\.tooling_plan/);
  assert.match(source, /"fixturePlan": response\.fixture_plan/);
  assert.match(source, /"monitoringPlan": response\.monitoring_plan/);
  assert.match(source, /"interfaceControlPlan": response\.interface_control_plan/);
  assert.match(source, /"decompositionPlan": response\.decomposition_plan/);
  assert.match(source, /"releasePackagePlan": response\.release_package_plan/);
  assert.match(source, /"interventionMap": response\.intervention_map/);
  assert.match(source, /"executionPlan": response\.execution_plan/);
  assert.match(source, /"postprocessPlan": response\.postprocess_plan/);
  assert.match(source, /"controllerPlan": response\.controller_plan/);
  assert.match(source, /"pomdpBeliefState": response\.learning\.pomdp_belief_state/);
  assert.match(source, /"releaseProbePlan": response\.learning\.release_probe_plan/);
  assert.match(source, /"neuralTrainingCorpus": response\.learning\.neural_training_corpus/);
  assert.match(source, /"machineRelease": response\.machine_release/);
  assert.match(source, /"manufacturingHandoff": response\.manufacturing_handoff/);
  assert.match(source, /"materialPlan": response\.material_plan/);
  assert.match(source, /assembly-interface/);
  assert.match(source, /gated-before-machine-release/);
  assert.match(source, /printed-pocket-turned-insert/);
  assert.match(source, /first-article-metrology-and-fit-check/);
  assert.match(source, /fn analyze_instruction_programs/);
  assert.match(source, /fn analysis_part_plans/);
  assert.match(source, /fn analysis_process_steps/);
  assert.match(source, /fn learn_from_outcome/);
  assert.match(source, /fn enrich_outcome_with_stored_job_context/);
  assert.match(source, /fn enrich_outcome_from_store/);
  assert.match(source, /fn plan_outcome_operation_sequence/);
  assert.match(source, /let request = enrich_outcome_from_store\(&state, request\)/);
  assert.match(source, /let request = enrich_outcome_from_store\(&task_state, request\)/);
  assert.match(source, /source-plan-method/);
  assert.match(source, /fabrication_outcome_enriches_learning_from_source_plan_job/);
  assert.match(source, /fn outcome_remediation_plan/);
  assert.match(source, /outcome_remediation: OutcomeRemediationPlan/);
  assert.match(source, /record_observations\.extend\(response\.outcome_remediation\.learning_signals\.clone\(\)\)/);
  assert.match(source, /fn learning_artifacts/);
  assert.match(source, /fn stored_learning_job/);
  assert.match(source, /fn fabrication_mdp_request/);
  assert.match(source, /fn instruction_analysis_mdp_request/);
  assert.match(source, /fabrication\.mdp\.instruction-analysis-policy/);
  assert.match(source, /"learningEngine": &response\.learning\.engine/);
  assert.match(source, /"desMdpSpec": des_mdp_spec/);
  assert.match(source, /"desMdpSolution": des_mdp_solution/);
  assert.match(source, /"desPomdpSpec": des_pomdp_spec/);
  assert.match(source, /"desPomdpSolution": des_pomdp_solution/);
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
  assert.match(source, /dd_fabrication_server_costing_result_reviews_total/);
  assert.match(source, /dd_fabrication_server_nats_messages_total/);
  assert.match(source, /dd_fabrication_server_nats_results_published_total/);
  assert.match(source, /dd_fabrication_server_mdp_published_total/);
  assert.match(source, /dd_fabrication_server_operator_actions_total/);
  assert.match(source, /dd_fabrication_server_fixture_release_blockers_total/);
  assert.match(source, /dd_fabrication_server_split_combine_reviews_total/);
  assert.match(source, /dd_fabrication_server_jobs_stored_total/);
  assert.match(source, /dd_fabrication_server_artifacts_stored_total/);
  assert.match(source, /dd_fabrication_server_artifact_requests_total/);
  assert.match(source, /dd_fabrication_server_current_jobs/);
  assert.match(source, /dd_fabrication_server_current_artifacts/);
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
  assert.match(source, /arc-plane-not-reset-before-end/);
  assert.match(source, /arc-plane-boundary/);
  assert.match(source, /arc-center-offset-plane-mismatch/);
  assert.match(source, /arc-plane-offset-boundary/);
  assert.match(source, /arc-missing-center-or-radius/);
  assert.match(source, /arc-geometry-boundary/);
  assert.match(source, /number_after\(&stripped, 'K'\)/);
  assert.match(source, /mill_router_lathe_analysis_requires_arc_plane_evidence_before_arc/);
  assert.match(source, /mill_router_analysis_requires_arc_plane_reset_before_end/);
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
  assert.match(source, /id: "five-axis-mill-1"/);
  assert.match(source, /id: "rotary-indexer-mill-1"/);
  assert.match(source, /id: "mill-turn-center-1"/);
  assert.match(source, /id: "swiss-turning-center-1"/);
  assert.match(source, /fn wants_rotary_index_milling/);
  assert.match(source, /fn is_rotary_index_mill_kind/);
  assert.match(source, /five-axis-sculpted-feature/);
  assert.match(source, /draft five-axis milling program generated by dd-fabrication-server/);
  assert.match(source, /five-axis-sculpted-body/);
  assert.match(source, /G43\.4H8/);
  assert.match(source, /five-axis-mill-postprocessor/);
  assert.match(source, /five-axis-controller-gcode/);
  assert.match(source, /five_axis_mill_plan_generates_tcp_reviewed_program_and_artifacts/);
  assert.match(source, /"five-axis-mill"/);
  assert.match(source, /"five-axis-milling"/);
  assert.match(source, /indexed-rotary-feature/);
  assert.match(source, /draft rotary-indexed milling program generated by dd-fabrication-server/);
  assert.match(source, /indexed-rotary-subtractive-feature/);
  assert.match(source, /"indexed-mill-gcode"/);
  assert.match(source, /rotary-index-mill-postprocessor/);
  assert.match(source, /indexed-mill-controller-gcode/);
  assert.match(source, /rotary-index-fixture-setup-sheet/);
  assert.match(source, /rotary-clearance-simulation-report/);
  assert.match(source, /reprobe-datum-record/);
  assert.match(source, /rotary_index_mill_plan_generates_indexed_gcode_and_artifacts/);
  assert.match(source, /"rotary-indexer-mill"/);
  assert.match(source, /"indexed-rotary-milling"/);
  assert.match(source, /horizontal-slotted-feature/);
  assert.match(source, /draft horizontal milling program generated by dd-fabrication-server/);
  assert.match(source, /horizontal-subtractive-feature/);
  assert.match(source, /fn wants_mill_turning/);
  assert.match(source, /fn is_mill_turn_kind/);
  assert.match(source, /fn wants_swiss_turning/);
  assert.match(source, /fn is_swiss_turning_kind/);
  assert.match(source, /needs_mill_turn_part/);
  assert.match(source, /needs_swiss_turn_part/);
  assert.match(source, /wants_mill_turn_center/);
  assert.match(source, /wants_swiss_turning_center/);
  assert.match(source, /is_mill_turn_kind\(&machine\.kind\)/);
  assert.match(source, /is_swiss_turning_kind\(&machine\.kind\)/);
  assert.match(source, /draft mill-turn program generated by dd-fabrication-server/);
  assert.match(source, /mill-turn-live-tooling-boundary/);
  assert.match(source, /mill-turn-spindle-transfer-boundary/);
  assert.match(source, /mill-turn-gcode-postprocessor/);
  assert.match(source, /mill-turn-controller-gcode/);
  assert.match(source, /mill_turn_plan_generates_live_tool_and_transfer_program/);
  assert.match(source, /"mill-turn-gcode"/);
  assert.match(source, /"mill-turn-job"/);
  assert.match(source, /"mill-turn-center"/);
  assert.match(source, /draft swiss turning program generated by dd-fabrication-server/);
  assert.match(source, /swiss-guide-bushing-boundary/);
  assert.match(source, /swiss-live-tool-boundary/);
  assert.match(source, /swiss-pickoff-cutoff-boundary/);
  assert.match(source, /swiss-turning-gcode-postprocessor/);
  assert.match(source, /swiss-turning-controller-gcode/);
  assert.match(source, /swiss_turning_plan_generates_guide_bushing_pickoff_program/);
  assert.match(source, /fn has_text_swiss_turning_context/);
  assert.match(source, /fn has_text_swiss_guide_bushing_evidence/);
  assert.match(source, /fn has_text_swiss_pickoff_cutoff_evidence/);
  assert.match(source, /has_swiss_text_context/);
  assert.match(source, /has_swiss_guide_bushing_evidence/);
  assert.match(source, /has_swiss_pickoff_cutoff_evidence/);
  assert.match(source, /swiss-guide-bushing-evidence-missing/);
  assert.match(source, /swiss-pickoff-cutoff-evidence-missing/);
  assert.match(source, /add-swiss-guide-bushing-evidence/);
  assert.match(source, /add-swiss-pickoff-cutoff-evidence/);
  assert.match(source, /text_swiss_turning_jobs_require_guide_bushing_and_pickoff_cutoff_evidence/);
  assert.match(source, /"swiss-turning-gcode"/);
  assert.match(source, /"swiss-turning-job"/);
  assert.match(source, /"swiss-turning-center"/);
  assert.match(source, /swiss-guide-bushing-and-bar-feed-record/);
  assert.match(source, /swiss-gang-tool-and-live-tool-clearance-record/);
  assert.match(source, /swiss-pickoff-cutoff-and-ejection-record/);
  assert.match(source, /swiss-first-article-runout-and-remnant-record/);
  assert.match(source, /draft turning program generated by dd-fabrication-server/);
  assert.match(source, /chuck grip, collet pressure, stick-out/);
  assert.match(source, /G95 ; feed per revolution threading mode verified/);
  assert.match(source, /lathe-threading-boundary/);
  assert.match(source, /lathe-part-off-boundary/);
  assert.match(source, /generated_lathe_jobs_require_threading_sync_and_partoff_support_evidence/);
  assert.match(source, /part-off cutoff with part catcher verified/);
  assert.match(source, /turning-controller-gcode/);
  assert.match(source, /lathe-workholding-setup-sheet/);
  assert.match(source, /lathe-spindle-speed-limit-record/);
  assert.match(source, /threading-pitch-sync-record/);
  assert.match(source, /partoff-catcher-support-record/);
  assert.match(source, /lathe_plan_generates_threading_partoff_program_and_artifacts/);
  assert.match(source, /"lathe-job"/);
  assert.match(source, /"turning-job"/);
  assert.match(source, /id: "sla-printer-1"/);
  assert.match(source, /id: "pellet-fgf-printer-1"/);
  assert.match(source, /id: "robotic-additive-cell-1"/);
  assert.match(source, /id: "sheet-lamination-printer-1"/);
  assert.match(source, /id: "paste-extrusion-printer-1"/);
  assert.match(source, /id: "bound-metal-fff-printer-1"/);
  assert.match(source, /id: "multi-material-fdm-printer-1"/);
  assert.match(source, /id: "material-jetting-printer-1"/);
  assert.match(source, /id: "composite-fiber-printer-1"/);
  assert.match(source, /id: "sls-printer-1"/);
  assert.match(source, /id: "directed-energy-deposition-cell-1"/);
  assert.match(source, /id: "metal-pbf-printer-1"/);
  assert.match(source, /id: "binder-jet-printer-1"/);
  assert.match(source, /id: "robotic-assembly-cell-1"/);
  assert.match(source, /draft resin SLA\/MSLA job generated by dd-fabrication-server/);
  assert.match(source, /draft pellet FGF \/ large-format additive job generated by dd-fabrication-server/);
  assert.match(source, /draft robotic \/ gantry additive job generated by dd-fabrication-server/);
  assert.match(source, /draft sheet-lamination additive job generated by dd-fabrication-server/);
  assert.match(source, /draft paste\/clay extrusion job generated by dd-fabrication-server/);
  assert.match(source, /draft bound-metal filament FFF job generated by dd-fabrication-server/);
  assert.match(source, /draft multi-material FDM\/toolchanger job generated by dd-fabrication-server/);
  assert.match(source, /draft material jetting\/PolyJet job generated by dd-fabrication-server/);
  assert.match(source, /draft continuous-fiber composite job generated by dd-fabrication-server/);
  assert.match(source, /draft powder-bed additive job generated by dd-fabrication-server/);
  assert.match(source, /draft directed-energy deposition\/WAAM job generated by dd-fabrication-server/);
  assert.match(source, /draft metal powder-bed fusion job generated by dd-fabrication-server/);
  assert.match(source, /draft binder-jet additive job generated by dd-fabrication-server/);
  assert.match(source, /draft robotic assembly\/joining job generated by dd-fabrication-server/);
  assert.match(source, /fn wants_pellet_fgf_printing/);
  assert.match(source, /fn wants_robotic_additive_printing/);
  assert.match(source, /fn wants_sheet_lamination_printing/);
  assert.match(source, /fn is_sheet_lamination_printer_kind/);
  assert.match(source, /fn wants_paste_extrusion_printing/);
  assert.match(source, /fn wants_bound_metal_filament_printing/);
  assert.match(source, /fn wants_multi_material_fdm_printing/);
  assert.match(source, /wants_pellet_fgf_printer/);
  assert.match(source, /wants_robotic_additive_printer/);
  assert.match(source, /wants_sheet_lamination_part/);
  assert.match(source, /wants_sheet_lamination_printer/);
  assert.match(source, /wants_paste_extrusion_printer/);
  assert.match(source, /wants_bound_metal_fff_printer/);
  assert.match(source, /wants_multi_material_fdm_printer/);
  assert.match(source, /is_pellet_fgf_printer_kind\(&machine\.kind\)/);
  assert.match(source, /is_robotic_additive_printer_kind\(&machine\.kind\)/);
  assert.match(source, /is_sheet_lamination_printer_kind\(&machine\.kind\)/);
  assert.match(source, /is_paste_extrusion_printer_kind\(&machine\.kind\)/);
  assert.match(source, /is_bound_metal_filament_printer_kind\(&machine\.kind\)/);
  assert.match(source, /is_multi_material_fdm_printer_kind\(&machine\.kind\)/);
  assert.match(source, /DRY_PELLETS/);
  assert.match(source, /PRINT_BEAD_PATH/);
  assert.match(source, /LOAD_ROBOT_PATH/);
  assert.match(source, /DRY_RUN_ROBOT/);
  assert.match(source, /PURGE_ROBOTIC_EXTRUDER/);
  assert.match(source, /DEPOSIT_ROBOTIC_BEAD_PATH/);
  assert.match(source, /LOAD_SHEET_STACK/);
  assert.match(source, /REGISTER_LAYER_STACK/);
  assert.match(source, /CUT_OR_TRIM_LAYERS/);
  assert.match(source, /BOND_OR_CONSOLIDATE_LAYERS/);
  assert.match(source, /CONDITION_PASTE/);
  assert.match(source, /PRINT_PASTE_PATH/);
  assert.match(source, /LOAD_BOUND_METAL_FILAMENT/);
  assert.match(source, /SINTER_PART/);
  assert.match(source, /KIT_PARTS/);
  assert.match(source, /VERIFY_DATUMS/);
  assert.match(source, /PICK_PLACE/);
  assert.match(source, /INSPECT_JOIN/);
  assert.match(source, /kit_parts/);
  assert.match(source, /verify_datums/);
  assert.match(source, /pick_place/);
  assert.match(source, /part_revisions/);
  assert.match(source, /join_graph/);
  assert.match(source, /locating_pins/);
  assert.match(source, /press_fit_force_n/);
  assert.match(source, /pull_or_torque_test/);
  assert.match(source, /MATERIAL_MAP/);
  assert.match(source, /TOOLCHANGE_SEQUENCE/);
  assert.match(source, /PURGE_TOWER/);
  assert.match(source, /pellet-fgf-job-packager/);
  assert.match(source, /pellet-fgf-job-package/);
  assert.match(source, /robotic-additive-job-packager/);
  assert.match(source, /robotic-additive-job-package/);
  assert.match(source, /robotic-additive-controller-dialect/);
  assert.match(source, /robotic-additive-feedstock/);
  assert.match(source, /sheet-lamination-job-packager/);
  assert.match(source, /sheet-lamination-job-package/);
  assert.match(source, /paste-extrusion-job-packager/);
  assert.match(source, /paste-extrusion-job-package/);
  assert.match(source, /bound-metal-fff-job-packager/);
  assert.match(source, /bound-metal-fff-job-package/);
  assert.match(source, /multi-material-fdm-job-packager/);
  assert.match(source, /multi-material-fdm-job-package/);
  assert.match(source, /grinding-job-packager/);
  assert.match(source, /grinding-job-package/);
  assert.match(source, /inspection-report-packager/);
  assert.match(source, /inspection-report-package/);
  assert.match(source, /thermal-postprocess-job-packager/);
  assert.match(source, /thermal-postprocess-job-package/);
  assert.match(source, /surface-finishing-job-packager/);
  assert.match(source, /surface-finishing-job-package/);
  assert.match(source, /metal-joining-job-packager/);
  assert.match(source, /metal-joining-job-package/);
  assert.match(source, /molding-casting-job-packager/);
  assert.match(source, /molding-casting-job-package/);
  assert.match(source, /pcb-fabrication-job-packager/);
  assert.match(source, /pcb-fabrication-job-package/);
  assert.match(source, /pcb-assembly-job-packager/);
  assert.match(source, /pcb-assembly-job-package/);
  assert.match(source, /fixture-tooling-job-packager/);
  assert.match(source, /fixture-tooling-job-package/);
  assert.match(source, /adaptive-compensation-job-packager/);
  assert.match(source, /adaptive-compensation-job-package/);
  assert.match(source, /insert-installation-job-packager/);
  assert.match(source, /insert-installation-job-package/);
  assert.match(source, /adhesive-bonding-job-packager/);
  assert.match(source, /adhesive-bonding-job-package/);
  assert.match(source, /plastic-joining-job-packager/);
  assert.match(source, /plastic-joining-job-package/);
  assert.match(source, /fn wants_plastic_joining/);
  assert.match(source, /fn is_plastic_joining_kind/);
  assert.match(source, /id: "plastic-joining-cell-1"/);
  assert.match(source, /kind: "plastic-joining-cell"/);
  assert.match(source, /draft plastic joining \/ ultrasonic welding \/ heat staking job/);
  assert.match(source, /VERIFY_PLASTIC_JOIN_SETUP/);
  assert.match(source, /RUN_PLASTIC_JOIN/);
  assert.match(source, /VERIFY_PLASTIC_JOIN_RELEASE/);
  assert.match(source, /plastic-joining-setup-evidence-missing/);
  assert.match(source, /plastic-joining-release-evidence-missing/);
  assert.match(source, /add-plastic-joining-setup-evidence/);
  assert.match(source, /add-plastic-joining-release-evidence/);
  assert.match(source, /default_special_process_fleet_generates_plastic_joining_job/);
  assert.match(source, /text_plastic_joining_jobs_require_setup_and_release_evidence/);
  assert.match(source, /generated_plastic_joining_jobs_require_setup_and_release_evidence/);
  assert.match(source, /fastener-installation-job-packager/);
  assert.match(source, /fastener-installation-job-package/);
  assert.match(source, /rivet-installation-job-packager/);
  assert.match(source, /rivet-installation-job-package/);
  assert.match(source, /seal-installation-job-packager/);
  assert.match(source, /seal-installation-job-package/);
  assert.match(source, /bearing-installation-job-packager/);
  assert.match(source, /bearing-installation-job-package/);
  assert.match(source, /dynamic-balancing-job-packager/);
  assert.match(source, /dynamic-balancing-job-package/);
  assert.match(source, /part-marking-job-packager/);
  assert.match(source, /part-marking-job-package/);
  assert.match(source, /packaging-labeling-job-packager/);
  assert.match(source, /packaging-labeling-job-package/);
  assert.match(source, /composite-layup-job-packager/);
  assert.match(source, /composite-layup-job-package/);
  assert.match(source, /hot-wire-foam-job-packager/);
  assert.match(source, /hot-wire-foam-job-package/);
  assert.match(source, /press-brake-job-packager/);
  assert.match(source, /press-brake-job-package/);
  assert.match(source, /gear-cutting-job-packager/);
  assert.match(source, /gear-cutting-job-package/);
  assert.match(source, /paste-rheology-and-nozzle-record/);
  assert.match(source, /drying-shrinkage-and-green-part-support-record/);
  assert.match(source, /bound-metal-filament-profile-record/);
  assert.match(source, /debind-sinter-furnace-cycle-record/);
  assert.match(source, /sintered-density-and-shrinkage-inspection-record/);
  assert.match(source, /material-slot-map-and-filament-lot-record/);
  assert.match(source, /purge-tower-wipe-and-resume-record/);
  assert.match(source, /robot-frame-tcp-and-collision-record/);
  assert.match(source, /robotic-extruder-feedstock-and-purge-record/);
  assert.match(source, /robotic-bead-coupon-and-flow-record/);
  assert.match(source, /robotic-cell-interlock-and-release-record/);
  assert.match(source, /sheet-lamination-stock-and-stack-record/);
  assert.match(source, /sheet-lamination-registration-and-trim-record/);
  assert.match(source, /sheet-lamination-bond-and-consolidation-record/);
  assert.match(source, /sheet-lamination-delamination-and-dimensional-record/);
  assert.match(source, /wheel-dress-and-balance-record/);
  assert.match(source, /grinding-coolant-and-workholding-record/);
  assert.match(source, /surface-finish-and-final-metrology-record/);
  assert.match(source, /inspection-calibration-record/);
  assert.match(source, /datum-alignment-and-uncertainty-record/);
  assert.match(source, /first-article-measured-values-report/);
  assert.match(source, /nonconformance-disposition-record/);
  assert.match(source, /thermal-profile-and-furnace-log/);
  assert.match(source, /fixture-setter-and-atmosphere-record/);
  assert.match(source, /cooldown-quench-and-ppe-record/);
  assert.match(source, /distortion-hardness-and-release-inspection-record/);
  assert.match(source, /surface-media-chemistry-and-sds-record/);
  assert.match(source, /masking-plugging-and-protected-feature-record/);
  assert.match(source, /ventilation-ppe-and-waste-record/);
  assert.match(source, /finish-thickness-adhesion-and-inspection-record/);
  assert.match(source, /welding-procedure-and-qualification-record/);
  assert.match(source, /joint-fitup-fixture-and-clamp-record/);
  assert.match(source, /filler-flux-gas-and-fume-control-record/);
  assert.match(source, /heat-input-interpass-and-distortion-record/);
  assert.match(source, /weld-inspection-nde-and-repair-record/);
  assert.match(source, /mold-master-tooling-and-release-record/);
  assert.match(source, /mix-ratio-pot-life-and-batch-record/);
  assert.match(source, /degas-vacuum-pressure-and-cure-record/);
  assert.match(source, /demold-shrinkage-void-and-dimensional-record/);
  assert.match(source, /pcb-board-data-bom-and-centroid-record/);
  assert.match(source, /stencil-paste-feeder-and-nozzle-record/);
  assert.match(source, /reflow-profile-and-first-article-record/);
  assert.match(source, /aoi-xray-test-and-rework-record/);
  assert.match(source, /composite-tooling-release-and-ply-kit-record/);
  assert.match(source, /fiber-resin-prepreg-core-lot-record/);
  assert.match(source, /vacuum-bag-leak-debulk-and-cure-record/);
  assert.match(source, /demold-trim-coupon-void-and-dimensional-record/);
  assert.match(source, /foam-blank-density-and-template-record/);
  assert.match(source, /wire-temperature-tension-and-kerf-record/);
  assert.match(source, /fume-fire-watch-and-ppe-record/);
  assert.match(source, /foam-core-surface-taper-and-dimensional-record/);
  assert.match(source, /flat-pattern-and-bend-allowance-record/);
  assert.match(source, /press-brake-tooling-and-tonnage-record/);
  assert.match(source, /backgauge-bend-sequence-and-angle-inspection-record/);
  assert.match(source, /formed-part-dimensional-release-record/);
  assert.match(source, /gear-drawing-and-blank-datum-record/);
  assert.match(source, /gear-cutter-arbor-and-indexing-record/);
  assert.match(source, /gear-deburr-and-burr-control-record/);
  assert.match(source, /gear-inspection-over-pins-span-profile-record/);
  assert.match(source, /wants_material_jetting_printer/);
  assert.match(source, /is_material_jetting_printer_kind\(&machine\.kind\)/);
  assert.match(source, /material-jetting-job-packager/);
  assert.match(source, /material-jetting-job-package/);
  assert.match(source, /wants_ded_cell/);
  assert.match(source, /is_directed_energy_deposition_kind\(&machine\.kind\)/);
  assert.match(source, /directed-energy-deposition-job-packager/);
  assert.match(source, /directed-energy-deposition-job-package/);
  assert.match(source, /wants_composite_fiber_printer/);
  assert.match(source, /is_composite_fiber_printer_kind\(&machine\.kind\)/);
  assert.match(source, /composite-fiber-job-packager/);
  assert.match(source, /composite-fiber-job-package/);
  assert.match(source, /INERT_GAS_PURGE/);
  assert.match(source, /RECOATER_CLEARANCE_CHECK/);
  assert.match(source, /STRESS_RELIEF/);
  assert.match(source, /START_DEPOSITION/);
  assert.match(source, /MONITOR_MELT_POOL/);
  assert.match(source, /interpass-boundary/);
  assert.match(source, /BINDER_JET_PRINT/);
  assert.match(source, /SINTER_OR_INFILTRATE/);
  assert.match(source, /metal-pbf-job-packager/);
  assert.match(source, /metal-pbf-job-package/);
  assert.match(source, /binder-jet-job-packager/);
  assert.match(source, /binder-jet-job-package/);
  assert.match(source, /assembly-cell-job-packager/);
  assert.match(source, /assembly-cell-job-package/);
  assert.match(source, /assembly-kit-and-join-traveler/);
  assert.match(source, /robot-path-or-fixture-simulation-report/);
  assert.match(source, /final-fit-metrology-record/);
  assert.match(source, /default_additive_fleet_generates_pellet_fgf_printer_job/);
  assert.match(source, /default_additive_fleet_generates_robotic_additive_job/);
  assert.match(source, /default_additive_fleet_generates_sheet_lamination_job/);
  assert.match(source, /default_additive_fleet_generates_paste_extrusion_printer_job/);
  assert.match(source, /default_additive_fleet_generates_bound_metal_fff_printer_job/);
  assert.match(source, /default_additive_fleet_generates_material_jetting_printer_job/);
  assert.match(source, /default_additive_fleet_generates_multi_material_fdm_printer_job/);
  assert.match(source, /default_additive_fleet_generates_directed_energy_deposition_job/);
  assert.match(source, /default_additive_fleet_generates_composite_fiber_printer_job/);
  assert.match(source, /default_additive_fleet_generates_metal_pbf_printer_job/);
  assert.match(source, /default_additive_fleet_generates_binder_jet_printer_job/);
  assert.match(source, /default_fleet_generates_robotic_assembly_cell_job/);
  assert.match(source, /"sls-printer",\s+"metal-pbf-printer"/);
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
  assert.match(source, /fn has_text_mill_turn_context/);
  assert.match(source, /fn has_text_mill_turn_live_tooling_evidence/);
  assert.match(source, /fn has_text_mill_turn_transfer_context/);
  assert.match(source, /fn has_text_mill_turn_spindle_transfer_evidence/);
  assert.match(source, /mill-turn-live-tooling-evidence-missing/);
  assert.match(source, /add-mill-turn-live-tooling-evidence/);
  assert.match(source, /mill-turn-spindle-transfer-evidence-missing/);
  assert.match(source, /add-mill-turn-spindle-transfer-evidence/);
  assert.match(source, /text_mill_turn_jobs_require_live_tooling_and_spindle_transfer_evidence/);
  assert.match(source, /fn has_text_sheet_cutting_context/);
  assert.match(source, /fn has_text_sheet_cutting_recipe_evidence/);
  assert.match(source, /fn has_text_assembly_context/);
  assert.match(source, /fn has_text_assembly_fit_evidence/);
  assert.match(source, /fn has_text_assembly_cell_context/);
  assert.match(source, /fn has_text_assembly_cell_automation_evidence/);
  assert.match(source, /fn has_text_assembly_join_process_evidence/);
  assert.match(source, /fn has_text_part_separation_context/);
  assert.match(source, /fn has_text_part_separation_evidence/);
  assert.match(source, /fn has_text_part_separation_release_evidence/);
  assert.match(source, /text-resin-handling-boundary/);
  assert.match(source, /resin-handling-boundary/);
  assert.match(source, /resin-print-profile-evidence-missing/);
  assert.match(source, /resin-print-profile-boundary/);
  assert.match(source, /add-resin-print-profile-evidence/);
  assert.match(source, /fn has_text_resin_layer_manifest_context/);
  assert.match(source, /fn has_text_resin_layer_manifest_image_evidence/);
  assert.match(source, /fn has_text_resin_layer_manifest_motion_evidence/);
  assert.match(source, /has_resin_layer_manifest_context/);
  assert.match(source, /has_resin_layer_manifest_image_evidence/);
  assert.match(source, /has_resin_layer_manifest_motion_evidence/);
  assert.match(source, /resin-layer-manifest-evidence-missing/);
  assert.match(source, /resin-layer-manifest-boundary/);
  assert.match(source, /add-resin-layer-manifest-evidence/);
  assert.match(source, /"layer_manifest"/);
  assert.match(source, /"image_stack"/);
  assert.match(source, /"peel_lift"/);
  assert.match(source, /generated_resin_jobs_require_layer_image_and_peel_evidence/);
  assert.match(source, /resin_layer_manifests_require_image_and_peel_evidence/);
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
  assert.match(source, /fn has_text_pellet_fgf_context/);
  assert.match(source, /has_pellet_fgf_text_context/);
  assert.match(source, /pellet-fgf-material-evidence-missing/);
  assert.match(source, /pellet-fgf-material-boundary/);
  assert.match(source, /add-pellet-fgf-material-evidence/);
  assert.match(source, /pellet-fgf-bead-thermal-evidence-missing/);
  assert.match(source, /pellet-fgf-bead-thermal-boundary/);
  assert.match(source, /add-pellet-fgf-bead-thermal-evidence/);
  assert.match(source, /text_pellet_fgf_jobs_require_material_and_bead_thermal_evidence/);
  assert.match(source, /"dry_pellets"/);
  assert.match(source, /"purge_extruder"/);
  assert.match(source, /"print_bead_path"/);
  assert.match(source, /"dew_point_c"/);
  assert.match(source, /"bead_width_mm"/);
  assert.match(source, /"melt_temp_c"/);
  assert.match(source, /"trim_allowance_mm"/);
  assert.match(
    source,
    /generated_pellet_fgf_jobs_require_material_bead_and_thermal_evidence/,
  );
  assert.match(source, /fn has_text_robotic_additive_context/);
  assert.match(source, /has_robotic_additive_text_context/);
  assert.match(source, /has_robotic_additive_path_evidence/);
  assert.match(source, /has_robotic_additive_extrusion_evidence/);
  assert.match(source, /robotic-additive-path-evidence-missing/);
  assert.match(source, /robotic-additive-path-boundary/);
  assert.match(source, /add-robotic-additive-path-evidence/);
  assert.match(source, /robotic-additive-extrusion-evidence-missing/);
  assert.match(source, /robotic-additive-extrusion-boundary/);
  assert.match(source, /add-robotic-additive-extrusion-evidence/);
  assert.match(source, /text_robotic_additive_jobs_require_path_and_extrusion_evidence/);
  assert.match(source, /"load_robot_path"/);
  assert.match(source, /"dry_run_robot"/);
  assert.match(source, /"purge_robotic_extruder"/);
  assert.match(source, /"deposit_robotic_bead_path"/);
  assert.match(source, /"reach_collision_sim"/);
  assert.match(source, /"external_axis"/);
  assert.match(source, /"collision_watch"/);
  assert.match(
    source,
    /generated_robotic_additive_jobs_require_path_cell_and_extrusion_evidence/,
  );
  assert.match(source, /"robotic-additive-job"/);
  assert.match(source, /"robotic-pellet-job"/);
  assert.match(source, /"robotic-extrusion-job"/);
  assert.match(source, /"robotic-additive-cell"/);
  assert.match(source, /robotic-additive-job-sheet/);
  assert.match(source, /robot\/cell dry run/);
  assert.match(source, /fn has_text_sheet_lamination_context/);
  assert.match(source, /has_sheet_lamination_text_context/);
  assert.match(source, /has_sheet_lamination_stock_evidence/);
  assert.match(source, /has_sheet_lamination_bond_evidence/);
  assert.match(source, /sheet-lamination-stock-evidence-missing/);
  assert.match(source, /sheet-lamination-stock-boundary/);
  assert.match(source, /add-sheet-lamination-stock-evidence/);
  assert.match(source, /sheet-lamination-bond-evidence-missing/);
  assert.match(source, /sheet-lamination-bond-boundary/);
  assert.match(source, /add-sheet-lamination-bond-evidence/);
  assert.match(source, /text_sheet_lamination_jobs_require_stock_and_bond_evidence/);
  assert.match(source, /"load_sheet_stack"/);
  assert.match(source, /"register_layer_stack"/);
  assert.match(source, /"cut_or_trim_layers"/);
  assert.match(source, /"bond_or_consolidate_layers"/);
  assert.match(source, /"inspect_lamination"/);
  assert.match(source, /"sheet_lot"/);
  assert.match(source, /"stack_order"/);
  assert.match(source, /"amplitude_force_speed"/);
  assert.match(source, /"peel_or_lap_shear"/);
  assert.match(
    source,
    /generated_sheet_lamination_jobs_require_stock_registration_and_bond_evidence/,
  );
  assert.match(source, /"sheet-lamination-job"/);
  assert.match(source, /"laminated-object-job"/);
  assert.match(source, /"ultrasonic-additive-job"/);
  assert.match(source, /"sheet-lamination-printer"/);
  assert.match(source, /sheet-lamination-job-sheet/);
  assert.match(source, /sheet-lamination-registration-and-bond-calibration/);
  assert.match(source, /sheet-lamination-trim-bond-and-delamination-release/);
  assert.match(source, /fn has_text_paste_extrusion_context/);
  assert.match(source, /has_paste_extrusion_text_context/);
  assert.match(source, /paste-extrusion-rheology-evidence-missing/);
  assert.match(source, /paste-extrusion-rheology-boundary/);
  assert.match(source, /add-paste-extrusion-rheology-evidence/);
  assert.match(source, /paste-extrusion-drying-evidence-missing/);
  assert.match(source, /paste-extrusion-drying-boundary/);
  assert.match(source, /add-paste-extrusion-drying-evidence/);
  assert.match(source, /text_paste_extrusion_jobs_require_rheology_and_drying_evidence/);
  assert.match(source, /"condition_paste"/);
  assert.match(source, /"purge_syringe_or_auger"/);
  assert.match(source, /"print_paste_path"/);
  assert.match(source, /"dry_green_part"/);
  assert.match(source, /"water_content_pct"/);
  assert.match(source, /"pressure_or_ram_speed"/);
  assert.match(source, /"shrinkage_allowance_pct"/);
  assert.match(
    source,
    /generated_paste_extrusion_jobs_require_rheology_pressure_and_drying_evidence/,
  );
  assert.match(source, /fn has_text_bound_metal_fff_context/);
  assert.match(source, /has_bound_metal_fff_text_context/);
  assert.match(source, /bound-metal-fff-profile-evidence-missing/);
  assert.match(source, /bound-metal-fff-profile-boundary/);
  assert.match(source, /add-bound-metal-fff-profile-evidence/);
  assert.match(source, /bound-metal-fff-debind-sinter-evidence-missing/);
  assert.match(source, /bound-metal-fff-debind-sinter-boundary/);
  assert.match(source, /add-bound-metal-fff-debind-sinter-evidence/);
  assert.match(source, /text_bound_metal_fff_jobs_require_profile_and_debind_sinter_evidence/);
  assert.match(source, /"load_bound_metal_filament"/);
  assert.match(source, /"slice_bound_metal_fff"/);
  assert.match(source, /"print_green_part"/);
  assert.match(source, /"debind_green_part"/);
  assert.match(source, /"sinter_part"/);
  assert.match(source, /"shrinkage_scale_xyz"/);
  assert.match(source, /"green_part_fixture"/);
  assert.match(source, /"solvent_or_catalytic_or_thermal"/);
  assert.match(source, /"setter_support"/);
  assert.match(
    source,
    /generated_bound_metal_fff_jobs_require_profile_debind_and_sinter_evidence/,
  );
  assert.match(source, /fn has_text_multi_material_fdm_context/);
  assert.match(source, /has_multi_material_fdm_text_context/);
  assert.match(source, /multi-material-fdm-map-evidence-missing/);
  assert.match(source, /multi-material-fdm-material-map-boundary/);
  assert.match(source, /add-multi-material-fdm-map-evidence/);
  assert.match(source, /multi-material-fdm-purge-resume-evidence-missing/);
  assert.match(source, /multi-material-fdm-purge-resume-boundary/);
  assert.match(source, /add-multi-material-fdm-purge-resume-evidence/);
  assert.match(source, /text_multi_material_fdm_jobs_require_material_map_and_purge_resume_evidence/);
  assert.match(source, /fn has_text_material_jetting_context/);
  assert.match(source, /has_material_jetting_text_context/);
  assert.match(source, /material-jetting-material-evidence-missing/);
  assert.match(source, /material-jetting-material-boundary/);
  assert.match(source, /add-material-jetting-material-evidence/);
  assert.match(source, /material-jetting-support-uv-inspection-evidence-missing/);
  assert.match(source, /material-jetting-support-uv-inspection-boundary/);
  assert.match(source, /add-material-jetting-support-uv-inspection-evidence/);
  assert.match(source, /"pack_tray"/);
  assert.match(source, /"jet_materials"/);
  assert.match(source, /"uv_cure_inline"/);
  assert.match(source, /"remove_support"/);
  assert.match(source, /generated_material_jetting_jobs_require_material_support_and_uv_evidence/);
  assert.match(source, /text_material_jetting_jobs_require_material_support_and_uv_inspection_evidence/);
  assert.match(source, /fn has_text_ded_context/);
  assert.match(source, /has_ded_text_context/);
  assert.match(source, /ded-feedstock-path-evidence-missing/);
  assert.match(source, /ded-feedstock-path-boundary/);
  assert.match(source, /add-ded-feedstock-path-evidence/);
  assert.match(source, /ded-energy-thermal-inspection-evidence-missing/);
  assert.match(source, /ded-energy-thermal-inspection-boundary/);
  assert.match(source, /add-ded-energy-thermal-inspection-evidence/);
  assert.match(source, /"prep_substrate"/);
  assert.match(source, /"plan_beads"/);
  assert.match(source, /"start_deposition"/);
  assert.match(source, /"monitor_melt_pool"/);
  assert.match(source, /"inspect_deposit"/);
  assert.match(source, /generated_ded_jobs_require_feedstock_energy_thermal_and_inspection_evidence/);
  assert.match(source, /text_ded_jobs_require_feedstock_energy_thermal_and_inspection_evidence/);
  assert.match(source, /fn has_text_composite_fiber_context/);
  assert.match(source, /has_composite_fiber_text_context/);
  assert.match(source, /composite-fiber-layup-evidence-missing/);
  assert.match(source, /composite-fiber-layup-boundary/);
  assert.match(source, /add-composite-fiber-layup-evidence/);
  assert.match(source, /composite-fiber-process-inspection-evidence-missing/);
  assert.match(source, /composite-fiber-process-inspection-boundary/);
  assert.match(source, /add-composite-fiber-process-inspection-evidence/);
  assert.match(source, /text_composite_fiber_jobs_require_layup_process_and_inspection_evidence/);
  assert.match(source, /"fiber_layup"/);
  assert.match(source, /"fiber_cut_anchor"/);
  assert.match(source, /"print_composite"/);
  assert.match(source, /"fiber_orientation"/);
  assert.match(source, /"cutter_calibration"/);
  assert.match(source, /"fiber_continuity"/);
  assert.match(
    source,
    /generated_composite_fiber_jobs_require_layup_process_and_inspection_evidence/,
  );
  assert.match(source, /"prepare_layup_tool"/);
  assert.match(source, /"layup_plies"/);
  assert.match(source, /"vacuum_bag_and_leak_test"/);
  assert.match(source, /"cure_laminate"/);
  assert.match(source, /"demold_trim_inspect"/);
  assert.match(source, /"mold_or_mandrel"/);
  assert.match(source, /"release_system"/);
  assert.match(source, /"bag_stack"/);
  assert.match(source, /"cure_profile"/);
  assert.match(
    source,
    /generated_composite_layup_jobs_require_tooling_and_bag_cure_evidence/,
  );
  assert.match(source, /"foam_blank_setup"/);
  assert.match(source, /"wire_heat_tension_check"/);
  assert.match(source, /"kerf_coupon"/);
  assert.match(source, /"hot_wire_cut"/);
  assert.match(source, /"current_or_temp"/);
  assert.match(source, /"synchronized_axes"/);
  assert.match(
    source,
    /generated_hot_wire_foam_jobs_require_setup_and_process_evidence/,
  );
  assert.match(source, /fn has_text_sheet_forming_setup_evidence/);
  assert.match(source, /fn has_text_sheet_forming_inspection_evidence/);
  assert.match(source, /"load_flat_blank"/);
  assert.match(source, /"set_brake_tooling"/);
  assert.match(source, /"run_bend_sequence"/);
  assert.match(source, /"inspect_formed_part"/);
  assert.match(source, /"springback_compensation"/);
  assert.match(source, /"pass_fail"/);
  assert.match(
    source,
    /generated_sheet_forming_jobs_require_setup_bend_and_inspection_evidence/,
  );
  assert.match(source, /"load_gear_blank"/);
  assert.match(source, /"set_gear_tool"/);
  assert.match(source, /"cut_gear_teeth"/);
  assert.match(source, /"deburr_profile"/);
  assert.match(source, /"inspect_gear"/);
  assert.match(source, /"module_or_dp"/);
  assert.match(source, /"index_ratio"/);
  assert.match(source, /"tooth_thickness"/);
  assert.match(
    source,
    /generated_gear_cutting_jobs_require_setup_indexing_and_inspection_evidence/,
  );
  assert.match(source, /text-powder-handling-boundary/);
  assert.match(source, /powder-handling-boundary/);
  assert.match(source, /powder-bed-build-profile-evidence-missing/);
  assert.match(source, /powder-bed-build-profile-boundary/);
  assert.match(source, /add-powder-bed-build-profile-evidence/);
  assert.match(source, /fn has_text_powder_bed_recoater_thermal_context/);
  assert.match(source, /fn has_text_powder_bed_recoater_clearance_evidence/);
  assert.match(source, /fn has_text_powder_bed_thermal_pack_evidence/);
  assert.match(source, /has_powder_bed_recoater_thermal_context/);
  assert.match(source, /has_powder_bed_recoater_clearance_evidence/);
  assert.match(source, /has_powder_bed_thermal_pack_evidence/);
  assert.match(source, /powder-bed-recoater-thermal-evidence-missing/);
  assert.match(source, /powder-bed-recoater-thermal-boundary/);
  assert.match(source, /add-powder-bed-recoater-thermal-evidence/);
  assert.match(source, /powder_bed_recoater_and_dense_pack_jobs_require_clearance_and_thermal_evidence/);
  assert.match(source, /powder-bed-handling-evidence-missing/);
  assert.match(source, /powder-bed-handling-boundary/);
  assert.match(source, /add-powder-bed-handling-evidence/);
  assert.match(source, /"nest_parts"/);
  assert.match(source, /"print_powder_bed"/);
  assert.match(source, /"powder_recovery"/);
  assert.match(source, /generated_powder_bed_jobs_require_profile_recoater_and_handling_evidence/);
  assert.match(source, /"print_metal_pbf"/);
  assert.match(source, /"inert_gas_purge"/);
  assert.match(source, /"recoater_clearance_check"/);
  assert.match(source, /"stress_relief"/);
  assert.match(source, /"plate_removal"/);
  assert.match(source, /generated_metal_pbf_jobs_require_profile_recoater_and_handling_evidence/);
  assert.match(source, /fn has_text_binder_jet_context/);
  assert.match(source, /has_binder_jet_text_context/);
  assert.match(source, /binder-jet-process-evidence-missing/);
  assert.match(source, /binder-jet-process-boundary/);
  assert.match(source, /add-binder-jet-process-evidence/);
  assert.match(source, /binder-jet-postprocess-shrinkage-evidence-missing/);
  assert.match(source, /binder-jet-postprocess-shrinkage-boundary/);
  assert.match(source, /add-binder-jet-postprocess-shrinkage-evidence/);
  assert.match(source, /"binder_jet_print"/);
  assert.match(source, /"cure_green_part"/);
  assert.match(source, /"sinter_or_infiltrate"/);
  assert.match(source, /generated_binder_jet_jobs_require_process_postprocess_and_shrinkage_evidence/);
  assert.match(source, /text_binder_jet_jobs_require_process_postprocess_and_shrinkage_evidence/);
  assert.match(source, /subtractive-text-setup-evidence-missing/);
  assert.match(source, /subtractive-text-setup-boundary/);
  assert.match(source, /add-subtractive-text-setup-evidence/);
  assert.match(source, /subtractive-text-process-evidence-missing/);
  assert.match(source, /subtractive-text-process-boundary/);
  assert.match(source, /add-subtractive-text-process-evidence/);
  assert.match(source, /sheet-cutting-recipe-evidence-missing/);
  assert.match(source, /sheet-cutting-recipe-boundary/);
  assert.match(source, /add-sheet-cutting-recipe-evidence/);
  assert.match(source, /fn has_text_sheet_cutting_setup_evidence/);
  assert.match(source, /fn has_text_sheet_cutting_cut_path_evidence/);
  assert.match(source, /fn has_text_sheet_cutting_release_evidence/);
  assert.match(source, /has_sheet_cutting_setup_evidence/);
  assert.match(source, /has_sheet_cutting_cut_path_evidence/);
  assert.match(source, /has_sheet_cutting_release_evidence/);
  assert.match(source, /sheet-cutting-setup-evidence-missing/);
  assert.match(source, /sheet-cutting-setup-boundary/);
  assert.match(source, /add-sheet-cutting-setup-evidence/);
  assert.match(source, /sheet-cutting-cut-path-evidence-missing/);
  assert.match(source, /sheet-cutting-cut-path-boundary/);
  assert.match(source, /add-sheet-cutting-cut-path-evidence/);
  assert.match(source, /sheet-cutting-release-evidence-missing/);
  assert.match(source, /sheet-cutting-release-boundary/);
  assert.match(source, /add-sheet-cutting-release-evidence/);
  assert.match(source, /generated_sheet_cutting_jobs_require_setup_cut_path_and_release_evidence/);
  assert.match(source, /fn has_text_wire_edm_context/);
  assert.match(source, /fn has_text_wire_edm_setup_evidence/);
  assert.match(source, /fn has_text_wire_edm_process_evidence/);
  assert.match(source, /fn has_text_wire_edm_cut_command/);
  assert.match(source, /has_wire_edm_text_context/);
  assert.match(source, /has_wire_edm_setup_evidence/);
  assert.match(source, /has_wire_edm_process_evidence/);
  assert.match(source, /reported_wire_edm_cut_setup_boundary/);
  assert.match(source, /wire-edm-text-evidence-missing/);
  assert.match(source, /wire-edm-text-boundary/);
  assert.match(source, /add-wire-edm-text-evidence/);
  assert.match(source, /text_wire_edm_jobs_require_threading_flushing_and_slug_evidence/);
  assert.match(source, /wire-edm-cut-before-threading-setup/);
  assert.match(source, /wire-edm-cut-setup-boundary/);
  assert.match(source, /wire_edm_jobs_require_threading_setup_before_profile_cut/);
  assert.match(source, /generated_wire_edm_jobs_require_threading_flushing_and_slug_evidence/);
  assert.match(source, /fn has_text_sinker_edm_context/);
  assert.match(source, /fn has_text_sinker_edm_electrode_evidence/);
  assert.match(source, /fn has_text_sinker_edm_dielectric_evidence/);
  assert.match(source, /fn has_text_sinker_edm_burn_control_evidence/);
  assert.match(source, /has_sinker_edm_text_context/);
  assert.match(source, /has_sinker_edm_electrode_evidence/);
  assert.match(source, /has_sinker_edm_dielectric_evidence/);
  assert.match(source, /has_sinker_edm_burn_control_evidence/);
  assert.match(source, /sinker-edm-text-evidence-missing/);
  assert.match(source, /sinker-edm-text-boundary/);
  assert.match(source, /add-sinker-edm-text-evidence/);
  assert.match(source, /text_sinker_edm_jobs_require_electrode_dielectric_and_burn_evidence/);
  assert.match(source, /generated_sinker_edm_jobs_require_electrode_dielectric_and_burn_evidence/);
  assert.match(source, /fn has_text_grinding_context/);
  assert.match(source, /fn has_text_grinding_wheel_setup_evidence/);
  assert.match(source, /fn has_text_grinding_sparkout_inspection_evidence/);
  assert.match(source, /fn has_text_inspection_context/);
  assert.match(source, /fn has_text_inspection_calibration_evidence/);
  assert.match(source, /fn has_text_inspection_disposition_evidence/);
  assert.match(source, /has_text_grinding_context/);
  assert.match(source, /has_text_grinding_wheel_setup_evidence/);
  assert.match(source, /has_text_grinding_sparkout_inspection_evidence/);
  assert.match(source, /assembly-fit-metrology-evidence-missing/);
  assert.match(source, /assembly-fit-metrology-boundary/);
  assert.match(source, /add-assembly-fit-metrology-evidence/);
  assert.match(source, /assembly-cell-automation-evidence-missing/);
  assert.match(source, /assembly-cell-automation-boundary/);
  assert.match(source, /add-assembly-cell-automation-evidence/);
  assert.match(source, /assembly-cell-join-process-evidence-missing/);
  assert.match(source, /assembly-cell-join-process-boundary/);
  assert.match(source, /add-assembly-cell-join-process-evidence/);
  assert.match(source, /text-part-separation-boundary/);
  assert.match(source, /part-separation-evidence-missing/);
  assert.match(source, /part-separation-evidence-boundary/);
  assert.match(source, /add-part-separation-evidence/);
  assert.match(source, /part-separation-release-evidence-missing/);
  assert.match(source, /part-separation-release-boundary/);
  assert.match(source, /add-part-separation-release-evidence/);
  assert.match(source, /load_separation_fixture/);
  assert.match(source, /cut_path/);
  assert.match(source, /release_retained_tabs/);
  assert.match(source, /deburr_edges/);
  assert.match(source, /trace_parts/);
  assert.match(source, /inspect_separation/);
  assert.match(source, /has_text_precision_requirement_context/);
  assert.match(source, /has_text_precision_inspection_evidence/);
  assert.match(source, /fn has_text_precision_requirement_context/);
  assert.match(source, /fn has_text_precision_inspection_evidence/);
  assert.match(source, /precision-inspection-evidence-missing/);
  assert.match(source, /precision-metrology-boundary/);
  assert.match(source, /add-precision-metrology-evidence/);
  assert.match(source, /grinding-wheel-setup-evidence-missing/);
  assert.match(source, /grinding-wheel-setup-boundary/);
  assert.match(source, /add-grinding-wheel-setup-evidence/);
  assert.match(source, /grinding-sparkout-inspection-evidence-missing/);
  assert.match(source, /grinding-sparkout-inspection-boundary/);
  assert.match(source, /add-grinding-sparkout-inspection-evidence/);
  assert.match(source, /inspection-calibration-evidence-missing/);
  assert.match(source, /inspection-calibration-boundary/);
  assert.match(source, /add-inspection-calibration-evidence/);
  assert.match(source, /inspection-disposition-evidence-missing/);
  assert.match(source, /inspection-disposition-boundary/);
  assert.match(source, /add-inspection-disposition-evidence/);
  assert.match(source, /has_text_unattended_run_context/);
  assert.match(source, /has_text_unattended_monitoring_evidence/);
  assert.match(source, /has_text_unattended_recovery_evidence/);
  assert.match(source, /fn has_text_unattended_run_context/);
  assert.match(source, /fn has_text_unattended_monitoring_evidence/);
  assert.match(source, /fn has_text_unattended_recovery_evidence/);
  assert.match(source, /unattended-monitoring-evidence-missing/);
  assert.match(source, /unattended-monitoring-boundary/);
  assert.match(source, /add-unattended-monitoring-evidence/);
  assert.match(source, /unattended-recovery-evidence-missing/);
  assert.match(source, /unattended-recovery-boundary/);
  assert.match(source, /add-unattended-recovery-evidence/);
  assert.match(source, /has_text_thermal_postprocess_context/);
  assert.match(source, /has_text_thermal_postprocess_evidence/);
  assert.match(source, /fn has_text_thermal_postprocess_context/);
  assert.match(source, /fn has_text_thermal_postprocess_evidence/);
  assert.match(source, /load_thermal_batch/);
  assert.match(source, /run_thermal_profile/);
  assert.match(source, /control_cooldown/);
  assert.match(source, /inspect_thermal_release/);
  assert.match(source, /ramp_c_per_min/);
  assert.match(source, /safe_handling_temp_c/);
  assert.match(source, /hardness_or_cure/);
  assert.match(source, /thermal-postprocess-evidence-missing/);
  assert.match(source, /thermal-postprocess-boundary/);
  assert.match(source, /add-thermal-postprocess-evidence/);
  assert.match(source, /fn wants_surface_finishing/);
  assert.match(source, /fn is_surface_finishing_kind/);
  assert.match(source, /id: "surface-finishing-cell-1"/);
  assert.match(source, /kind: "surface-finishing-cell"/);
  assert.match(source, /draft surface finishing job/);
  assert.match(source, /MASK_FEATURES/);
  assert.match(source, /RUN_SURFACE_FINISH/);
  assert.match(source, /INSPECT_SURFACE_FINISH/);
  assert.match(source, /mask_features/);
  assert.match(source, /run_surface_finish/);
  assert.match(source, /inspect_surface_finish/);
  assert.match(source, /protected_surfaces/);
  assert.match(source, /media_or_chemistry/);
  assert.match(source, /agitation_or_blast_pressure/);
  assert.match(source, /thickness_um/);
  assert.match(source, /roughness_or_color/);
  assert.match(source, /surface-finishing-setup-boundary/);
  assert.match(source, /surface-finishing-release-boundary/);
  assert.match(source, /surface-finishing-job-sheet/);
  assert.match(source, /surface-finishing-release/);
  assert.match(source, /plating-job/);
  assert.match(source, /anodizing-job/);
  assert.match(source, /media-blasting-job/);
  assert.match(source, /powder-coating-job/);
  assert.match(source, /default_special_process_fleet_generates_surface_finishing_job/);
  assert.match(source, /fn wants_metal_joining/);
  assert.match(source, /fn is_metal_joining_kind/);
  assert.match(source, /id: "metal-joining-cell-1"/);
  assert.match(source, /kind: "metal-joining-cell"/);
  assert.match(source, /draft metal joining job/);
  assert.match(source, /PREP_JOINTS/);
  assert.match(source, /SET_JOINING_PROCESS/);
  assert.match(source, /RUN_METAL_JOIN/);
  assert.match(source, /INSPECT_JOIN/);
  assert.match(source, /prep_joints/);
  assert.match(source, /set_joining_process/);
  assert.match(source, /run_metal_join/);
  assert.match(source, /inspect_join/);
  assert.match(source, /joint_design/);
  assert.match(source, /edge_prep/);
  assert.match(source, /fitup_gap_mm/);
  assert.match(source, /filler_or_solder/);
  assert.match(source, /shielding_or_flux/);
  assert.match(source, /heat_input/);
  assert.match(source, /interpass_temp_c/);
  assert.match(source, /fillet_or_penetration/);
  assert.match(source, /nde_or_leak_test/);
  assert.match(source, /metal-joining-procedure-boundary/);
  assert.match(source, /metal-joining-inspection-boundary/);
  assert.match(source, /metal-joining-job-sheet/);
  assert.match(source, /metal-joining-release/);
  assert.match(source, /welding-job/);
  assert.match(source, /brazing-job/);
  assert.match(source, /soldering-job/);
  assert.match(source, /default_special_process_fleet_generates_metal_joining_job/);
  assert.match(source, /fn wants_molding_casting/);
  assert.match(source, /fn is_molding_casting_kind/);
  assert.match(source, /id: "molding-casting-cell-1"/);
  assert.match(source, /kind: "molding-casting-cell"/);
  assert.match(source, /draft molding\/casting job/);
  assert.match(source, /PREPARE_MOLD/);
  assert.match(source, /MIX_CASTING_MATERIAL/);
  assert.match(source, /DEGAS_AND_CAST/);
  assert.match(source, /DEMOLD_AND_INSPECT/);
  assert.match(source, /prepare_mold/);
  assert.match(source, /mix_casting_material/);
  assert.match(source, /degas_and_cast/);
  assert.match(source, /demold_and_inspect/);
  assert.match(source, /tool_revision/);
  assert.match(source, /mix_ratio/);
  assert.match(source, /pot_life_min/);
  assert.match(source, /fill_strategy/);
  assert.match(source, /demold_method/);
  assert.match(source, /mold-tooling-boundary/);
  assert.match(source, /mold-cure-demold-boundary/);
  assert.match(source, /molding-casting-job-sheet/);
  assert.match(source, /molding-casting-release/);
  assert.match(source, /molding-casting-job/);
  assert.match(source, /mold-casting-job/);
  assert.match(source, /casting-job/);
  assert.match(source, /molding-job/);
  assert.match(source, /urethane-casting-job/);
  assert.match(source, /silicone-molding-job/);
  assert.match(source, /vacuum-casting-job/);
  assert.match(source, /injection-molding-job/);
  assert.match(source, /default_special_process_fleet_generates_molding_casting_job/);
  assert.match(source, /fn wants_pcb_assembly/);
  assert.match(source, /fn is_pcb_assembly_kind/);
  assert.match(source, /id: "pcb-assembly-cell-1"/);
  assert.match(source, /kind: "pcb-assembly-cell"/);
  assert.match(source, /draft PCB\/SMT assembly job/);
  assert.match(source, /LOAD_BOARD_DATA/);
  assert.match(source, /PREPARE_STENCIL_AND_PASTE/);
  assert.match(source, /SETUP_PICK_PLACE/);
  assert.match(source, /RUN_REFLOW/);
  assert.match(source, /INSPECT_AND_TEST/);
  assert.match(source, /load_board_data/);
  assert.match(source, /prepare_stencil_and_paste/);
  assert.match(source, /setup_pick_place/);
  assert.match(source, /run_reflow/);
  assert.match(source, /inspect_and_test/);
  assert.match(source, /pcb-assembly-setup-boundary/);
  assert.match(source, /pcb-assembly-reflow-inspection-boundary/);
  assert.match(source, /pcb-assembly-release/);
  assert.match(source, /pcb-assembly-job/);
  assert.match(source, /electronics-assembly-job/);
  assert.match(source, /smt-assembly-job/);
  assert.match(source, /default_special_process_fleet_generates_pcb_assembly_job/);
  assert.match(source, /fn wants_composite_layup/);
  assert.match(source, /fn is_composite_layup_kind/);
  assert.match(source, /id: "composite-layup-cell-1"/);
  assert.match(source, /kind: "composite-layup-cell"/);
  assert.match(source, /draft composite layup \/ vacuum-bag \/ autoclave job/);
  assert.match(source, /PREPARE_LAYUP_TOOL/);
  assert.match(source, /LAYUP_PLIES/);
  assert.match(source, /VACUUM_BAG_AND_LEAK_TEST/);
  assert.match(source, /CURE_LAMINATE/);
  assert.match(source, /DEMOLD_TRIM_INSPECT/);
  assert.match(source, /composite-layup-tooling-boundary/);
  assert.match(source, /composite-layup-bag-cure-boundary/);
  assert.match(source, /composite-layup-job-sheet/);
  assert.match(source, /composite-layup-release/);
  assert.match(source, /composite-layup-job/);
  assert.match(source, /wet-layup-job/);
  assert.match(source, /prepreg-layup-job/);
  assert.match(source, /vacuum-bag-job/);
  assert.match(source, /autoclave-cure-job/);
  assert.match(source, /resin-infusion-job/);
  assert.match(source, /default_special_process_fleet_generates_composite_layup_job/);
  assert.match(source, /fn wants_hot_wire_foam_cutting/);
  assert.match(source, /fn is_hot_wire_foam_cutter_kind/);
  assert.match(source, /id: "hot-wire-foam-cutter-1"/);
  assert.match(source, /kind: "hot-wire-foam-cutter"/);
  assert.match(source, /draft hot-wire foam cutting job/);
  assert.match(source, /FOAM_BLANK_SETUP/);
  assert.match(source, /WIRE_HEAT_TENSION_CHECK/);
  assert.match(source, /KERF_COUPON/);
  assert.match(source, /HOT_WIRE_CUT/);
  assert.match(source, /hot-wire-foam-setup-boundary/);
  assert.match(source, /hot-wire-foam-process-boundary/);
  assert.match(source, /hot-wire-foam-job-sheet/);
  assert.match(source, /hot-wire-foam-cutting-release/);
  assert.match(source, /hot-wire-foam-job/);
  assert.match(source, /hot-wire-job/);
  assert.match(source, /foam-cutting-job/);
  assert.match(source, /foam-core-job/);
  assert.match(source, /wing-core-job/);
  assert.match(source, /hot-wire-foam-controller-dialect/);
  assert.match(source, /foam-blank-or-core-stock/);
  assert.match(source, /default_special_process_fleet_generates_hot_wire_foam_job/);
  assert.match(source, /fn wants_sheet_forming/);
  assert.match(source, /fn is_sheet_forming_kind/);
  assert.match(source, /id: "press-brake-1"/);
  assert.match(source, /kind: "press-brake-forming-cell"/);
  assert.match(source, /draft press brake sheet-forming job/);
  assert.match(source, /SET_BRAKE_TOOLING/);
  assert.match(source, /RUN_BEND_SEQUENCE/);
  assert.match(source, /INSPECT_FORMED_PART/);
  assert.match(source, /press-brake-setup-boundary/);
  assert.match(source, /press-brake-release-boundary/);
  assert.match(source, /sheet-forming-job-sheet/);
  assert.match(source, /press-brake-sheet-forming/);
  assert.match(source, /press-brake-job/);
  assert.match(source, /sheet-forming-job/);
  assert.match(source, /bend-job/);
  assert.match(source, /gear-cutting-job/);
  assert.match(source, /gear-hobbing-job/);
  assert.match(source, /spline-broaching-job/);
assert.match(source, /default_special_process_fleet_generates_press_brake_job/);
assert.match(source, /has_text_sheet_forming_context/);
assert.match(source, /fn has_text_sheet_forming_context/);
assert.match(source, /has_text_sheet_forming_setup_evidence/);
assert.match(source, /has_text_sheet_forming_inspection_evidence/);
assert.match(source, /fn has_text_sheet_forming_setup_evidence/);
assert.match(source, /fn has_text_sheet_forming_inspection_evidence/);
assert.match(source, /sheet-forming-evidence-missing/);
  assert.match(source, /sheet-forming-boundary/);
  assert.match(source, /add-sheet-forming-evidence/);
  assert.match(source, /fn wants_gear_cutting/);
  assert.match(source, /fn is_gear_cutting_kind/);
  assert.match(source, /id: "gear-cutting-cell-1"/);
  assert.match(source, /kind: "gear-cutting-cell"/);
  assert.match(source, /draft gear\/spline cutting job/);
  assert.match(source, /LOAD_GEAR_BLANK/);
  assert.match(source, /SET_GEAR_TOOL/);
  assert.match(source, /CUT_GEAR_TEETH/);
  assert.match(source, /DEBURR_PROFILE/);
  assert.match(source, /gear-cutting-setup-boundary/);
  assert.match(source, /gear-indexing-boundary/);
  assert.match(source, /gear-inspection-boundary/);
  assert.match(source, /gear-cutting-job-sheet/);
  assert.match(source, /gear-spline-cutting/);
  assert.match(source, /gear-cutting-job/);
  assert.match(source, /gear-hobbing-job/);
  assert.match(source, /spline-broaching-job/);
  assert.match(source, /gear-cutting-controller-dialect/);
  assert.match(source, /round-blank-or-gear-stock/);
  assert.match(source, /default_special_process_fleet_generates_gear_cutting_job/);
  assert.match(source, /has_text_gear_cutting_context/);
  assert.match(source, /has_text_gear_cutting_evidence/);
  assert.match(source, /fn has_text_gear_cutting_context/);
  assert.match(source, /fn has_text_gear_cutting_evidence/);
  assert.match(source, /gear-cutting-evidence-missing/);
  assert.match(source, /gear-cutting-boundary/);
  assert.match(source, /add-gear-cutting-evidence/);
  assert.match(source, /text_gear_cutting_jobs_require_tooling_indexing_and_inspection_evidence/);
  assert.match(source, /has_text_surface_finishing_context/);
  assert.match(source, /has_text_surface_finishing_evidence/);
  assert.match(source, /fn has_text_surface_finishing_context/);
  assert.match(source, /fn has_text_surface_finishing_evidence/);
  assert.match(source, /surface-finishing-evidence-missing/);
  assert.match(source, /surface-finishing-boundary/);
  assert.match(source, /add-surface-finishing-evidence/);
  assert.match(source, /has_text_metal_joining_context/);
  assert.match(source, /has_text_metal_joining_procedure_evidence/);
  assert.match(source, /has_text_metal_joining_inspection_evidence/);
  assert.match(source, /fn has_text_metal_joining_context/);
  assert.match(source, /fn has_text_metal_joining_procedure_evidence/);
  assert.match(source, /fn has_text_metal_joining_inspection_evidence/);
  assert.match(source, /metal-joining-procedure-evidence-missing/);
  assert.match(source, /metal-joining-inspection-evidence-missing/);
  assert.match(source, /add-metal-joining-procedure-evidence/);
  assert.match(source, /add-metal-joining-inspection-evidence/);
  assert.match(source, /text_metal_joining_jobs_require_procedure_and_inspection_evidence/);
  assert.match(source, /has_text_molding_casting_context/);
  assert.match(source, /has_text_molding_casting_tooling_evidence/);
  assert.match(source, /has_text_molding_casting_process_evidence/);
  assert.match(source, /fn has_text_molding_casting_context/);
  assert.match(source, /fn has_text_molding_casting_tooling_evidence/);
  assert.match(source, /fn has_text_molding_casting_process_evidence/);
  assert.match(source, /molding-casting-tooling-evidence-missing/);
  assert.match(source, /molding-casting-process-evidence-missing/);
  assert.match(source, /molding-casting-tooling-boundary/);
  assert.match(source, /molding-casting-process-boundary/);
  assert.match(source, /add-molding-casting-tooling-evidence/);
  assert.match(source, /add-molding-casting-process-evidence/);
  assert.match(source, /text_molding_casting_jobs_require_tooling_and_process_evidence/);
  assert.match(source, /has_text_pcb_assembly_context/);
  assert.match(source, /has_text_pcb_assembly_setup_evidence/);
  assert.match(source, /has_text_pcb_assembly_reflow_inspection_evidence/);
  assert.match(source, /fn has_text_pcb_assembly_context/);
  assert.match(source, /fn has_text_pcb_assembly_setup_evidence/);
  assert.match(source, /fn has_text_pcb_assembly_reflow_inspection_evidence/);
  assert.match(source, /pcb-assembly-setup-evidence-missing/);
  assert.match(source, /pcb-assembly-reflow-inspection-evidence-missing/);
  assert.match(source, /add-pcb-assembly-setup-evidence/);
  assert.match(source, /add-pcb-assembly-reflow-inspection-evidence/);
  assert.match(source, /text_pcb_assembly_jobs_require_setup_reflow_and_test_evidence/);
  assert.match(source, /generated_pcb_assembly_jobs_require_setup_reflow_and_test_evidence/);
  assert.match(source, /has_text_composite_layup_context/);
  assert.match(source, /has_text_composite_layup_tooling_evidence/);
  assert.match(source, /has_text_composite_layup_bag_cure_evidence/);
  assert.match(source, /fn has_text_composite_layup_context/);
  assert.match(source, /fn has_text_composite_layup_tooling_evidence/);
  assert.match(source, /fn has_text_composite_layup_bag_cure_evidence/);
  assert.match(source, /composite-layup-tooling-evidence-missing/);
  assert.match(source, /composite-layup-bag-cure-evidence-missing/);
  assert.match(source, /add-composite-layup-tooling-evidence/);
  assert.match(source, /add-composite-layup-bag-cure-evidence/);
  assert.match(source, /text_composite_layup_jobs_require_tooling_and_bag_cure_evidence/);
  assert.match(source, /has_text_hot_wire_foam_context/);
  assert.match(source, /has_text_hot_wire_foam_setup_evidence/);
  assert.match(source, /has_text_hot_wire_foam_process_evidence/);
  assert.match(source, /fn has_text_hot_wire_foam_context/);
  assert.match(source, /fn has_text_hot_wire_foam_setup_evidence/);
  assert.match(source, /fn has_text_hot_wire_foam_process_evidence/);
  assert.match(source, /hot-wire-foam-setup-evidence-missing/);
  assert.match(source, /hot-wire-foam-process-evidence-missing/);
  assert.match(source, /add-hot-wire-foam-setup-evidence/);
  assert.match(source, /add-hot-wire-foam-process-evidence/);
  assert.match(source, /text_hot_wire_foam_jobs_require_setup_and_process_evidence/);
  assert.match(source, /has_text_indexed_setup_context/);
  assert.match(source, /has_text_indexed_setup_evidence/);
  assert.match(source, /fn has_text_indexed_setup_context/);
  assert.match(source, /fn has_text_indexed_setup_evidence/);
  assert.match(source, /indexed-setup-evidence-missing/);
  assert.match(source, /indexed-setup-boundary/);
  assert.match(source, /add-indexed-setup-evidence/);
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
  assert.match(source, /text_assembly_cell_jobs_require_robot_and_join_recipe_evidence/);
  assert.match(
    source,
    /text_part_separation_jobs_require_fixture_cut_path_and_inspection_evidence/,
  );
  assert.match(
    source,
    /structured_part_separation_checklists_require_fixture_trace_and_inspection_evidence/,
  );
  assert.match(
    source,
    /generated_part_separation_jobs_require_fixture_cut_path_release_and_trace_evidence/,
  );
  assert.match(
    source,
    /text_precision_jobs_require_metrology_and_surface_finish_evidence/,
  );
  assert.match(
    source,
    /text_grinding_jobs_require_wheel_setup_and_sparkout_inspection_evidence/,
  );
  assert.match(
    source,
    /generated_grinding_jobs_require_wheel_setup_and_sparkout_inspection_evidence/,
  );
  assert.match(
    source,
    /text_inspection_jobs_require_calibration_and_disposition_evidence/,
  );
  assert.match(
    source,
    /generated_inspection_jobs_require_calibration_and_disposition_evidence/,
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
    /generated_thermal_postprocess_jobs_require_profile_fixture_cooldown_and_release_evidence/,
  );
  assert.match(
    source,
    /generated_surface_finishing_jobs_require_masking_process_and_release_evidence/,
  );
  assert.match(
    source,
    /generated_metal_joining_jobs_require_procedure_process_and_inspection_evidence/,
  );
  assert.match(
    source,
    /generated_molding_casting_jobs_require_tooling_mix_cast_and_demold_evidence/,
  );
  assert.match(
    source,
    /generated_assembly_cell_jobs_require_robot_path_join_and_inspection_evidence/,
  );
  assert.match(
    source,
    /text_surface_finishing_jobs_require_chemistry_masking_and_inspection_evidence/,
  );
  assert.match(
    source,
    /text_sheet_forming_jobs_require_tooling_backgauge_and_inspection_evidence/,
  );
  assert.match(
    source,
    /text_gear_cutting_jobs_require_tooling_indexing_and_inspection_evidence/,
  );
  assert.match(
    source,
    /text_indexed_setup_jobs_require_clamp_clearance_and_datum_evidence/,
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
  assert.match(source, /fn has_additive_bed_leveling_disable/);
  assert.match(source, /additive_z_offset_evidence_observed/);
  assert.match(source, /additive_bed_leveling_disabled/);
  assert.match(source, /additive-negative-z-extrusion-not-verified/);
  assert.match(source, /printer-negative-z-extrusion-boundary/);
  assert.match(source, /additive-bed-leveling-disabled-before-extrusion/);
  assert.match(source, /printer-bed-leveling-boundary/);
  assert.match(source, /additive_analysis_requires_z_offset_evidence_before_negative_z_extrusion/);
  assert.match(source, /additive_analysis_requires_bed_leveling_restore_after_disable/);
  assert.match(source, /fn positioning_absolute_from_line/);
  assert.match(source, /fn units_mode_from_line/);
  assert.match(source, /fn has_units_mode_change_evidence/);
  assert.match(source, /reported_units_mode_change_boundary/);
  assert.match(source, /units-mode-change-after-motion/);
  assert.match(source, /units-mode-change-boundary/);
  assert.match(source, /cnc_analysis_requires_units_mode_review_after_motion/);
  assert.match(source, /reported_incremental_positioning_program_end_boundary/);
  assert.match(source, /incremental-positioning-not-reset-before-end/);
  assert.match(source, /incremental-positioning-boundary/);
  assert.match(source, /cnc_analysis_requires_absolute_positioning_before_program_end/);
  assert.match(source, /fn has_coordinate_transform_start/);
  assert.match(source, /fn has_coordinate_transform_cancel/);
  assert.match(source, /fn has_coordinate_transform_evidence/);
  assert.match(source, /coordinate_transform_active/);
  assert.match(source, /coordinate-transform-not-verified/);
  assert.match(source, /coordinate-transform-not-cancelled-before-end/);
  assert.match(source, /coordinate-transform-boundary/);
  assert.match(source, /cnc_analysis_requires_coordinate_transform_review_and_cancel/);
  assert.match(source, /fn has_work_coordinate_offset_start/);
  assert.match(source, /fn has_work_coordinate_offset_cancel/);
  assert.match(source, /fn has_work_coordinate_offset_evidence/);
  assert.match(source, /work_coordinate_offset_active/);
  assert.match(source, /work-coordinate-offset-not-verified/);
  assert.match(source, /work-coordinate-offset-not-cancelled-before-end/);
  assert.match(source, /work-coordinate-offset-boundary/);
  assert.match(source, /cnc_analysis_requires_work_coordinate_offset_review_and_cancel/);
  assert.match(source, /fn has_dwell_command/);
  assert.match(source, /fn has_dwell_duration_or_review/);
  assert.match(source, /reported_dwell_duration_boundary/);
  assert.match(source, /dwell-duration-missing/);
  assert.match(source, /dwell-duration-boundary/);
  assert.match(source, /cnc_analysis_requires_dwell_duration_evidence/);
  assert.match(source, /fn has_inverse_time_feed_start/);
  assert.match(source, /fn has_inverse_time_feed_cancel/);
  assert.match(source, /fn has_inverse_time_feed_evidence/);
  assert.match(source, /inverse_time_feed_active/);
  assert.match(source, /inverse-time-feed-not-verified/);
  assert.match(source, /inverse-time-feed-not-cancelled-before-end/);
  assert.match(source, /inverse-time-feed-boundary/);
  assert.match(source, /cnc_analysis_requires_inverse_time_feed_review_and_cancel/);
  assert.match(source, /fn has_tool_center_point_start/);
  assert.match(source, /fn has_tool_center_point_evidence/);
  assert.match(source, /tool_center_point_active/);
  assert.match(source, /tool-center-point-not-verified/);
  assert.match(source, /tool-center-point-not-cancelled-before-end/);
  assert.match(source, /tool-center-point-boundary/);
  assert.match(source, /cnc_analysis_requires_tool_center_point_review_and_cancel/);
  assert.match(source, /fn has_additive_relative_positioning_evidence/);
  assert.match(source, /additive_relative_positioning_verified/);
  assert.match(source, /additive-relative-positioning-extrusion-not-verified/);
  assert.match(source, /printer-relative-positioning-boundary/);
  assert.match(source, /additive_analysis_requires_positioning_reset_after_relative_mode/);
  assert.match(source, /fn has_additive_inch_units_evidence/);
  assert.match(source, /additive_inch_units_verified/);
  assert.match(source, /additive-inch-units-not-verified/);
  assert.match(source, /printer-inch-units-boundary/);
  assert.match(source, /additive_analysis_requires_unit_conversion_evidence_for_inch_mode/);
  assert.match(source, /fn has_additive_coordinate_offset_start/);
  assert.match(source, /fn has_additive_coordinate_offset_evidence/);
  assert.match(source, /additive_coordinate_offset_verified/);
  assert.match(source, /additive-coordinate-offset-not-verified/);
  assert.match(source, /printer-coordinate-offset-boundary/);
  assert.match(source, /additive_analysis_requires_coordinate_offset_evidence_before_extrusion/);
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
    /struct FabricationPlanResponse[\s\S]*generated_programs: Vec<GeneratedProgram>[\s\S]*instruction_programs: Vec<AnalyzedProgram>[\s\S]*simulation: SimulationReport[\s\S]*instruction_intent_map: InstructionIntentMap[\s\S]*improvements: Vec<InstructionImprovement>[\s\S]*improved_programs: Vec<ImprovedInstructionProgram>/,
  );
  assert.match(source, /"instruction-programs"/);
  assert.match(source, /"instructionPrograms": response\.instruction_programs/);
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
  assert.match(source, /"hybrid-make-plan"/);
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
  assert.match(source, /fn has_additive_midprint_homing_evidence/);
  assert.match(source, /additive_midprint_homing_pending_resume/);
  assert.match(source, /reported_additive_midprint_homing_boundary/);
  assert.match(source, /additive-midprint-homing-resume-not-verified/);
  assert.match(source, /printer-midprint-homing-boundary/);
  assert.match(source, /additive_analysis_requires_resume_position_after_midprint_homing/);
  assert.match(source, /fn has_additive_material_conditioning_evidence/);
  assert.match(source, /missing-filament-conditioning-evidence/);
  assert.match(source, /printer-material-conditioning-boundary/);
  assert.match(source, /fn has_additive_extrusion_calibration_evidence/);
  assert.match(source, /additive_extrusion_calibration_observed/);
  assert.match(source, /reported_additive_extrusion_calibration_boundary/);
  assert.match(source, /additive-extrusion-calibration-missing/);
  assert.match(source, /printer-extrusion-calibration-boundary/);
  assert.match(source, /additive_analysis_requires_extrusion_calibration_before_first_extrusion/);
  assert.match(source, /fn has_additive_volumetric_extrusion_start/);
  assert.match(source, /fn has_additive_volumetric_extrusion_evidence/);
  assert.match(source, /additive_volumetric_extrusion_verified/);
  assert.match(source, /additive-volumetric-extrusion-not-verified/);
  assert.match(source, /printer-volumetric-extrusion-boundary/);
  assert.match(source, /additive_analysis_requires_volumetric_extrusion_evidence/);
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
  assert.match(source, /fn has_additive_arc_support_evidence/);
  assert.match(source, /additive_arc_support_evidence_observed/);
  assert.match(source, /reported_additive_arc_support_boundary/);
  assert.match(source, /additive-arc-support-not-verified/);
  assert.match(source, /printer-arc-support-boundary/);
  assert.match(source, /additive_analysis_requires_arc_support_evidence/);
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
  assert.match(
    source,
    /generated_mill_router_jobs_require_tool_length_workholding_and_atc_evidence/,
  );
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
  assert.match(source, /fn has_controller_work_offset_write/);
  assert.match(source, /fn has_work_offset_write_evidence/);
  assert.match(source, /reported_work_offset_write_boundary/);
  assert.match(source, /work-offset-write-not-verified/);
  assert.match(source, /work-offset-write-boundary/);
  assert.match(source, /subtractive_analysis_requires_work_offset_write_review/);
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
  assert.match(source, /fn has_shutdown_required_process_media/);
  assert.match(source, /reported_process_end_boundary/);
  assert.match(source, /process-not-stopped-before-program-end/);
  assert.match(source, /machine-process-end-boundary/);
  assert.match(source, /shutdown_required_process_media_active/);
  assert.match(source, /reported_process_media_end_boundary/);
  assert.match(source, /process-media-not-stopped-before-program-end/);
  assert.match(source, /process-media-end-boundary/);
  assert.match(source, /subtractive_analysis_requires_process_shutdown_before_program_end/);
  assert.match(source, /id: "laser-cutter-1"/);
  assert.match(source, /id: "waterjet-cutter-1"/);
  assert.match(source, /id: "plasma-cutter-1"/);
  assert.match(source, /id: "wire-edm-1"/);
  assert.match(source, /id: "sinker-edm-1"/);
  assert.match(source, /id: "precision-grinder-1"/);
  assert.match(source, /id: "cmm-inspection-cell-1"/);
  assert.match(source, /id: "thermal-postprocess-furnace-1"/);
  assert.match(source, /draft laser sheet-cutting job generated by dd-fabrication-server/);
  assert.match(source, /draft waterjet sheet-cutting job generated by dd-fabrication-server/);
  assert.match(source, /draft plasma sheet-cutting job generated by dd-fabrication-server/);
  assert.match(source, /draft wire EDM profile job generated by dd-fabrication-server/);
  assert.match(source, /draft sinker EDM cavity job generated by dd-fabrication-server/);
  assert.match(source, /draft precision grinding job generated by dd-fabrication-server/);
  assert.match(source, /draft CMM\/vision inspection job generated by dd-fabrication-server/);
  assert.match(source, /draft thermal postprocess job generated by dd-fabrication-server/);
  assert.match(source, /ABRASIVE_FLOW_TEST/);
  assert.match(source, /PLASMA_CUT/);
  assert.match(source, /WIRE_THREAD_CHECK/);
  assert.match(source, /SKIM_PASS/);
  assert.match(source, /ELECTRODE_VERIFY/);
  assert.match(source, /electrode_verify/);
  assert.match(source, /graphite_or_copper/);
  assert.match(source, /wear_allowance/);
  assert.match(source, /DIELECTRIC_FLUSH_TEST/);
  assert.match(source, /dielectric_flush_test/);
  assert.match(source, /ROUGH_BURN/);
  assert.match(source, /rough_burn/);
  assert.match(source, /DEPTH_CHECK/);
  assert.match(source, /depth_check/);
  assert.match(source, /ORBIT_FINISH/);
  assert.match(source, /orbit_finish/);
  assert.match(source, /DRESS_WHEEL/);
  assert.match(source, /dress_wheel/);
  assert.match(source, /SETUP_WORKHOLDING/);
  assert.match(source, /magnetic_chuck_or_centers/);
  assert.match(source, /GRIND_PASS/);
  assert.match(source, /SPARK_OUT/);
  assert.match(source, /spark_out/);
  assert.match(source, /no_new_sparks/);
  assert.match(source, /INSPECT_GRIND/);
  assert.match(source, /inspect_grind/);
  assert.match(source, /surface_finish_ra/);
  assert.match(source, /CALIBRATE_PROBE/);
  assert.match(source, /calibrate_probe/);
  assert.match(source, /ALIGN_DATUMS/);
  assert.match(source, /align_datums/);
  assert.match(source, /coordinate_system/);
  assert.match(source, /MEASURE_FEATURE/);
  assert.match(source, /REPORT_INSPECTION/);
  assert.match(source, /report_inspection/);
  assert.match(source, /measured_values/);
  assert.match(source, /pass_fail/);
  assert.match(source, /LOAD_THERMAL_BATCH/);
  assert.match(source, /RUN_THERMAL_PROFILE/);
  assert.match(source, /CONTROL_COOLDOWN/);
  assert.match(source, /INSPECT_THERMAL_RELEASE/);
  assert.match(source, /draft surface finishing job generated by dd-fabrication-server/);
  assert.match(source, /MASK_FEATURES/);
  assert.match(source, /RUN_SURFACE_FINISH/);
  assert.match(source, /INSPECT_SURFACE_FINISH/);
  assert.match(source, /surface-finishing-setup-boundary/);
  assert.match(source, /surface-finishing-release-boundary/);
  assert.match(source, /draft metal joining job generated by dd-fabrication-server/);
  assert.match(source, /PREP_JOINTS/);
  assert.match(source, /SET_JOINING_PROCESS/);
  assert.match(source, /RUN_METAL_JOIN/);
  assert.match(source, /INSPECT_JOIN/);
  assert.match(source, /metal-joining-procedure-boundary/);
  assert.match(source, /metal-joining-inspection-boundary/);
  assert.match(source, /draft molding\/casting job generated by dd-fabrication-server/);
  assert.match(source, /PREPARE_MOLD/);
  assert.match(source, /MIX_CASTING_MATERIAL/);
  assert.match(source, /DEGAS_AND_CAST/);
  assert.match(source, /DEMOLD_AND_INSPECT/);
  assert.match(source, /mold-tooling-boundary/);
  assert.match(source, /mold-cure-demold-boundary/);
  assert.match(source, /draft composite layup \/ vacuum-bag \/ autoclave job generated by dd-fabrication-server/);
  assert.match(source, /PREPARE_LAYUP_TOOL/);
  assert.match(source, /LAYUP_PLIES/);
  assert.match(source, /VACUUM_BAG_AND_LEAK_TEST/);
  assert.match(source, /CURE_LAMINATE/);
  assert.match(source, /DEMOLD_TRIM_INSPECT/);
  assert.match(source, /composite-layup-tooling-boundary/);
  assert.match(source, /composite-layup-bag-cure-boundary/);
  assert.match(source, /draft press brake sheet-forming job generated by dd-fabrication-server/);
  assert.match(source, /SET_BRAKE_TOOLING/);
  assert.match(source, /RUN_BEND_SEQUENCE/);
  assert.match(source, /INSPECT_FORMED_PART/);
  assert.match(source, /press-brake-setup-boundary/);
  assert.match(source, /press-brake-release-boundary/);
  assert.match(source, /wire-edm-profile-postprocessor/);
  assert.match(source, /sinker-edm-cavity-postprocessor/);
  assert.match(source, /grinding-job-packager/);
  assert.match(source, /inspection-report-packager/);
  assert.match(source, /thermal-postprocess-job-packager/);
  assert.match(source, /surface-finishing-job-packager/);
  assert.match(source, /metal-joining-job-packager/);
  assert.match(source, /molding-casting-job-packager/);
  assert.match(source, /composite-layup-job-packager/);
  assert.match(source, /press-brake-job-packager/);
  assert.match(source, /gear-cutting-job-packager/);
  assert.match(source, /default_sheet_cut_fleet_generates_wire_edm_job_for_conductive_profile/);
  assert.match(source, /default_special_process_fleet_generates_sinker_edm_cavity_job/);
  assert.match(source, /default_special_process_fleet_generates_precision_grinding_job/);
  assert.match(source, /default_special_process_fleet_generates_cmm_inspection_job/);
  assert.match(source, /default_special_process_fleet_generates_thermal_postprocess_job/);
  assert.match(source, /default_special_process_fleet_generates_surface_finishing_job/);
  assert.match(source, /default_special_process_fleet_generates_metal_joining_job/);
  assert.match(source, /default_special_process_fleet_generates_composite_layup_job/);
  assert.match(source, /default_special_process_fleet_generates_press_brake_job/);
  assert.match(source, /default_special_process_fleet_generates_gear_cutting_job/);
  assert.match(source, /"precision-grinder"/);
  assert.match(source, /"cmm-inspection-cell"/);
  assert.match(source, /"thermal-postprocess-furnace"/);
  assert.match(source, /"surface-finishing-cell"/);
  assert.match(source, /"metal-joining-cell"/);
  assert.match(source, /"molding-casting-cell"/);
  assert.match(source, /"composite-layup-cell"/);
  assert.match(source, /"press-brake-forming-cell"/);
  assert.match(source, /"gear-cutting-cell"/);
  assert.match(source, /"wire-edm",\s+"sinker-edm"/);
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
  assert.match(source, /wire-edm-profile-postprocessor/);
  assert.match(source, /wire-edm-job/);
  assert.match(source, /default_sheet_cut_fleet_generates_wire_edm_job_for_conductive_profile/);
  assert.match(source, /sinker-edm-cavity-postprocessor/);
  assert.match(source, /sinker-edm-job/);
  assert.match(source, /default_special_process_fleet_generates_sinker_edm_cavity_job/);
  assert.match(source, /precision-grinder/);
  assert.match(source, /grinding-job/);
  assert.match(source, /surface-grinder-job/);
  assert.match(source, /cylindrical-grinder-job/);
  assert.match(source, /default_special_process_fleet_generates_precision_grinding_job/);
  assert.match(source, /cmm-inspection-cell/);
  assert.match(source, /cmm-inspection-job/);
  assert.match(source, /vision-inspection-job/);
  assert.match(source, /metrology-job/);
  assert.match(source, /thermal-postprocess-job/);
  assert.match(source, /furnace-job/);
  assert.match(source, /heat-treatment-job/);
  assert.match(source, /surface-finishing-cell/);
  assert.match(source, /surface-finishing-job/);
  assert.match(source, /coating-job/);
  assert.match(source, /plating-job/);
  assert.match(source, /anodizing-job/);
  assert.match(source, /media-blasting-job/);
  assert.match(source, /powder-coating-job/);
  assert.match(source, /deburr-polish-job/);
  assert.match(source, /metal-joining-cell/);
  assert.match(source, /metal-joining-job/);
  assert.match(source, /welding-job/);
  assert.match(source, /brazing-job/);
  assert.match(source, /soldering-job/);
  assert.match(source, /molding-casting-cell/);
  assert.match(source, /molding-casting-job/);
  assert.match(source, /mold-casting-job/);
  assert.match(source, /urethane-casting-job/);
  assert.match(source, /silicone-molding-job/);
  assert.match(source, /vacuum-casting-job/);
  assert.match(source, /composite-layup-cell/);
  assert.match(source, /composite-layup-job/);
  assert.match(source, /wet-layup-job/);
  assert.match(source, /prepreg-layup-job/);
  assert.match(source, /vacuum-bag-job/);
  assert.match(source, /autoclave-cure-job/);
  assert.match(source, /resin-infusion-job/);
  assert.match(source, /press-brake-job/);
  assert.match(source, /sheet-forming-job/);
  assert.match(source, /bend-job/);
  assert.match(source, /default_special_process_fleet_generates_cmm_inspection_job/);
  assert.match(source, /default_special_process_fleet_generates_thermal_postprocess_job/);
  assert.match(source, /default_special_process_fleet_generates_surface_finishing_job/);
  assert.match(source, /default_special_process_fleet_generates_press_brake_job/);
  assert.match(source, /default_special_process_fleet_generates_gear_cutting_job/);
  assert.match(source, /wire-edm-text-evidence-missing/);
  assert.match(source, /wire-edm-text-boundary/);
  assert.match(source, /add-wire-edm-text-evidence/);
  assert.match(source, /sinker-edm-text-evidence-missing/);
  assert.match(source, /sinker-edm-text-boundary/);
  assert.match(source, /add-sinker-edm-text-evidence/);
  assert.match(source, /sheet-forming-evidence-missing/);
  assert.match(source, /sheet-forming-boundary/);
  assert.match(source, /add-sheet-forming-evidence/);
  assert.match(source, /gear-cutting-evidence-missing/);
  assert.match(source, /gear-cutting-boundary/);
  assert.match(source, /add-gear-cutting-evidence/);
  assert.match(source, /metal-joining-procedure-evidence-missing/);
  assert.match(source, /metal-joining-inspection-evidence-missing/);
  assert.match(source, /molding-casting-tooling-evidence-missing/);
  assert.match(source, /molding-casting-process-evidence-missing/);
  assert.match(source, /composite-layup-tooling-evidence-missing/);
  assert.match(source, /composite-layup-bag-cure-evidence-missing/);
  assert.match(source, /grinding-wheel-setup-evidence-missing/);
  assert.match(source, /grinding-sparkout-inspection-evidence-missing/);
  assert.match(source, /kerf-controlled-sheet-profile/);
  assert.match(source, /"choose-sheet-cutting-process"\.to_string\(\)/);
  assert.match(source, /method_combination_preferences/);
  assert.match(source, /machine_kind_preferences/);
  assert.match(source, /fn learned_preferred_machine_kinds/);
  assert.match(source, /learned-machine-kind-preference/);
  assert.match(source, /prefer-learned-machine-kind/);
  assert.match(source, /learned_machine_kind_preferences_steer_future_open_machine_selection/);
  assert.match(source, /operation_sequence_preferences/);
  assert.match(source, /fn learned_preferred_operation_sequence/);
  assert.match(source, /learned-operation-sequence-preference/);
  assert.match(source, /prefer-learned-operation-sequence/);
  assert.match(source, /learned_parts_for_operation_sequence/);
  assert.match(source, /learned_operation_sequence_preferences_order_future_hybrid_parts/);
  assert.match(source, /learned_parts_for_method_combination/);
  assert.match(source, /prefer-learned-method-combination/);
  assert.match(source, /Some\("plastic-joining"\.to_string\(\)\)/);
  assert.match(source, /learned_plastic_joining_combinations_decompose_future_open_requests/);
  assert.match(source, /prefer-learned-method-combination-additive-print-plastic-joining/);
  assert.match(source, /dd\.fabrication\.neural-policy-sketch\.v1/);
  assert.match(source, /dd\.fabrication\.neural-engine-inference\.v1/);
  assert.match(source, /des_engine::des::general::neural_network::FeedForwardNetwork/);
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
  assert.match(source, /"desScheduleModel": response\.des_schedule_model/);
  assert.match(source, /"qualityPlan": response\.quality_plan/);
  assert.match(source, /"toolingPlan": response\.tooling_plan/);
  assert.match(source, /"fixturePlan"\.to_string\(\),\s*json!\(&response\.fixture_plan\)/);
  assert.match(source, /"monitoringPlan"\.to_string\(\),\s*json!\(&response\.monitoring_plan\)/);
  assert.match(source, /"interfaceControlPlan"\.to_string\(\),\s*json!\(&response\.interface_control_plan\)/);
  assert.match(source, /"decompositionPlan"\.to_string\(\),\s*json!\(&response\.decomposition_plan\)/);
  assert.match(source, /"releasePackagePlan"\.to_string\(\),\s*json!\(&response\.release_package_plan\)/);
  assert.match(source, /struct InterfaceControlPlan/);
  assert.match(source, /struct InterfaceControlRecord/);
  assert.match(source, /struct InterfaceDecisionLink/);
  assert.match(source, /interface_control_plan: InterfaceControlPlan/);
  assert.match(source, /fn interface_control_plan\(/);
  assert.match(source, /fn interface_control_plan_learning_actions/);
  assert.match(source, /"interface-control-plan"/);
  assert.match(source, /dd\.fabrication\.interface-control-plan\.v1/);
  assert.match(source, /interface-control:/);
  assert.match(source, /interface-decision:/);
  assert.match(source, /struct DecompositionPlan/);
  assert.match(source, /struct DecompositionTarget/);
  assert.match(source, /struct DecompositionRouteContract/);
  assert.match(source, /struct DecompositionInterface/);
  assert.match(source, /struct DecompositionReleaseGate/);
  assert.match(source, /route_contracts: Vec<DecompositionRouteContract>/);
  assert.match(source, /"machineSelection": response\.machine_selection/);
  assert.match(source, /"manufacturingHandoff": response\.manufacturing_handoff/);
  assert.match(source, /"materialPlan": response\.material_plan/);
  assert.match(source, /"processGraph": response\.process_graph/);
  assert.match(source, /"hybridMakePlan": response\.hybrid_make_plan/);
  assert.match(source, /"interventionMap": response\.intervention_map/);
  assert.match(source, /"interventionSignals": response\.learning\.intervention_signals/);
  assert.match(source, /"postprocessPlan": response\.postprocess_plan/);
  assert.match(source, /"controllerPlan": response\.controller_plan/);
  assert.match(source, /"controllerPlan"\.to_string\(\),\s*json!\(&response\.controller_plan\)/);
  assert.match(source, /"releasePackagePlan": response\.release_package_plan/);
  assert.match(source, /controller-target:/);
  assert.match(source, /controller-gate:/);
  assert.match(source, /"simulation": response\.simulation/);
  assert.match(source, /"pomdpBeliefState": response\.learning\.pomdp_belief_state/);
  assert.match(source, /"releaseProbePlan": response\.learning\.release_probe_plan/);
  assert.match(source, /"neuralTrainingCorpus": response\.learning\.neural_training_corpus/);
  assert.match(source, /"automationRequirements": response\.boundary_summary\.automation_requirements/);
  assert.match(source, /"resolutionPlan": response\.resolution_plan/);
  assert.match(source, /"automation-required"\.to_string\(\)/);
  assert.match(source, /hidden_activations/);
  assert.match(source, /engine_inference/);
  assert.match(source, /top_signal/);
  assert.match(source, /dd\.fabrication\.des-schedule-model\.v1/);
  assert.match(source, /des-schedule-model/);
  assert.match(source, /dd\.fabrication\.hybrid-make-plan\.v1/);
  assert.match(source, /split_combine_decisions/);
  assert.match(source, /hybrid-boundary:/);
  assert.match(source, /hybrid-strategy-candidate/);
  assert.match(source, /dd\.fabrication\.simulation-risk-profile\.v1/);
  assert.match(source, /simulation-risk:/);
  assert.match(source, /machine-envelope-exceeded/);
  assert.match(source, /dd\.fabrication\.des-instruction-model\.v1/);
  assert.match(source, /analysis-des-instruction-model/);
  assert.match(source, /"desInstructionModel": &response\.des_instruction_model/);
  assert.match(source, /analysis-instruction-intent-map/);
  assert.match(source, /"instructionIntentMap": &response\.instruction_intent_map/);
  assert.match(source, /release_handoff_routes/);
  assert.match(source, /review_priorities/);
  assert.match(source, /response_surfaces/);
  assert.match(source, /release_policy/);
  assert.match(source, /machine_failure_watchpoints/);
  assert.match(source, /human_intervention_watchpoints/);
  assert.match(source, /split_combine_hints/);
  assert.match(source, /failure_boundary_count/);
  assert.match(source, /action_scores/);
  assert.match(source, /id: "cnc-router-1"/);
  assert.match(source, /preferred_method: Some\("routing"\.to_string\(\)\)/);
  assert.match(source, /"choose-routing-process"\.to_string\(\)/);
  assert.match(source, /draft router profile program generated by dd-fabrication-server/);
  assert.match(source, /lift over tab boundary/);
  assert.match(source, /"machine-envelope"/);
  assert.match(source, /const MAX_MACHINES: usize = 96;/);
  assert.match(source, /"machineFleetLimits"/);
  assert.match(source, /"maxMachines": MAX_MACHINES/);
  assert.match(source, /"defaultMachineCount": default_machines\(\)\.len\(\)/);
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
  assert.match(source, /"GET \/readyz"/);
  assert.match(
    source,
    /async fn root\(\)[\s\S]*let routes = vec!\[[\s\S]*"GET \/"[\s\S]*"POST \/fabrication\/design\/generate"[\s\S]*"GET \/fabrication\/handoff\/catalog"[\s\S]*"POST \/fabrication\/remediation\/result"[\s\S]*"POST \/fabrication\/execution\/plan"[\s\S]*"POST \/fabrication\/assembly\/result"[\s\S]*\];[\s\S]*Json\(json!\(/,
  );
  assert.match(source, /async fn landing_page\(\) -> axum::response::Html<&'static str>/);
  assert.match(source, /"landingPage": \{/);
  assert.match(source, /"Human fabrication overview"/);
  assert.match(source, /"startHere": \{/);
  assert.match(source, /"humanOverview": "\/fabrication\/landing"/);
  assert.match(source, /"workflowOverview": "\/fabrication\/how-it-works"/);
  assert.match(source, /"apiDocs": "\/api\/docs"/);
  assert.match(source, /"GET \/landing"/);
  assert.match(source, /"GET \/fabrication\/landing"/);
  assert.match(source, /fn how_it_works_response\(\) -> Value/);
  assert.match(source, /async fn how_it_works_http\(\) -> impl IntoResponse/);
  assert.match(source, /dd\.fabrication\.how-it-works\.v1/);
  assert.match(source, /"GET \/how-it-works"/);
  assert.match(source, /"GET \/fabrication\/how-it-works"/);
  assert.match(source, /"releaseGateMatrix": \[/);
  assert.match(source, /"gateId": "source-provenance"/);
  assert.match(source, /"gateId": "machine-envelope"/);
  assert.match(source, /"gateId": "process-readiness"/);
  assert.match(source, /"gateId": "simulation-evidence"/);
  assert.match(source, /"gateId": "human-or-automation-handoff"/);
  assert.match(source, /"gateId": "learning-disposition"/);
  assert.match(source, /"priorityDispositionContract": \{/);
  assert.match(source, /"responseSurface": "priorityDispositions"/);
  assert.match(source, /"<family>:<priority>:<disposition>"/);
  assert.match(source, /POST \/fabrication\/machine-code\/generate/);
  assert.match(source, /remote\/submodules\/discrete-event-system\.rs des_engine/);
  assert.match(
    source,
    /how_it_works_endpoint_exposes_intake_generation_validation_release_and_learning_flow/,
  );
  assert.match(source, /DD Fabrication Server/);
  assert.match(source, /How It Works/);
  assert.match(source, /submitted fabrication goal into evidence-backed choices/);
  assert.match(source, /decomposes or combines parts when a single process is risky/);
  assert.match(source, /records outcomes so later jobs can learn from the route/);
  assert.match(source, /Design And Toolchain Intake/);
  assert.match(source, /PTC Creo \/ Pro\/ENGINEER/);
  assert.match(source, /SOLIDWORKS/);
  assert.match(source, /Autodesk Fusion/);
  assert.match(source, /Siemens NX/);
  assert.match(source, /CATIA/);
  assert.match(source, /Onshape/);
  assert.match(source, /FreeCAD/);
  assert.match(source, /OpenSCAD/);
  assert.match(source, /Blender/);
  assert.match(source, /ZBrush/);
  assert.match(source, /PrusaSlicer/);
  assert.match(source, /OrcaSlicer/);
  assert.match(source, /Cura/);
  assert.match(source, /Bambu Studio/);
  assert.match(source, /Release Gates/);
  assert.match(source, /Priority Dispositions/);
  assert.match(source, /pending-blocker-resolution/);
  assert.match(
    source,
    /Generated designs, toolpaths, slicer plans, G-code, controller programs, and job-sheet interpretations stay advisory/,
  );
  assert.match(source, /Source provenance/);
  assert.match(source, /Machine envelope/);
  assert.match(source, /Process readiness/);
  assert.match(source, /Simulation evidence/);
  assert.match(source, /Human or automation handoff/);
  assert.match(source, /Learning disposition/);
  assert.match(source, /\/fabrication\/intake\/catalog/);
  assert.match(source, /\/fabrication\/templates\/catalog/);
  assert.match(source, /intake guide/);
  assert.match(source, /request templates/);
  assert.match(source, /This service produces planning and evidence packets/);
  assert.match(source, /async fn capabilities/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.capabilities\.v1"/);
  assert.match(source, /"GET \/cells\/catalog"/);
  assert.match(source, /"GET \/fabrication\/cells\/catalog"/);
  assert.match(source, /"GET \/fabrication\/printers\/catalog"/);
  assert.match(source, /"GET \/fabrication\/subtractive\/catalog"/);
  assert.match(source, /"GET \/fabrication\/cnc\/catalog"/);
  assert.match(source, /"GET \/fabrication\/hybrid\/catalog"/);
  assert.match(source, /"GET \/fabrication\/methods\/catalog"/);
  assert.match(source, /"GET \/fabrication\/workers\/catalog"/);
  assert.match(source, /"GET \/fabrication\/results\/catalog"/);
  assert.match(source, /"GET \/fabrication\/machine-code\/catalog"/);
  assert.match(source, /"GET \/fabrication\/learning\/rewards\/catalog"/);
  assert.match(source, /"GET \/fabrication\/learning\/corpus"/);
  assert.match(source, /"strategyQualitySurfaces"/);
  assert.match(source, /"policySummary\.learnedQuality"/);
  assert.match(source, /"learningOutcomeQuality\.riskReviewRequired"/);
  assert.match(source, /fn objective_coverage_matrix\(\) -> Vec<Value>/);
  assert.match(source, /fn objective_coverage_response\(\) -> Value/);
  assert.match(source, /async fn objective_coverage_http\(\) -> impl IntoResponse/);
  assert.match(source, /dd\.fabrication\.objective-coverage\.v1/);
  assert.match(source, /"GET \/objective\/coverage"/);
  assert.match(source, /"GET \/fabrication\/objective\/coverage"/);
  assert.match(source, /"objectiveCoverageMatrix": objective_coverage_matrix\(\)/);
  assert.match(source, /"objectiveCoverageMatrix": matrix/);
  assert.match(source, /"requirement": "3d-printing-and-hybrid-intake"/);
  assert.match(source, /"requirement": "machine-code-and-instruction-generation"/);
  assert.match(source, /"requirement": "existing-instruction-validation-and-improvement"/);
  assert.match(source, /"requirement": "machine-failure-and-human-intervention-boundaries"/);
  assert.match(source, /"requirement": "split-combine-and-multi-process-learning"/);
  assert.match(source, /"requirement": "mdp-pomdp-des-neural-learning"/);
  assert.match(source, /"evidenceSurfaces": \["mdpRequest", "desMdpSolution", "desPomdpSolution", "neuralPolicy", "neuralTrainingCorpus", "learningOutcomeMemory"\]/);
  assert.match(source, /objective_coverage_endpoint_exposes_goal_matrix/);
  assert.match(source, /"learning-policy-snapshot"/);
  assert.match(source, /"learning-outcome-memory"/);
  assert.match(source, /"learning-corpus"/);
  assert.match(source, /"decomposition": \[/);
  assert.match(
    source,
    /"decompositionResult": \["POST \/decomposition\/result", "POST \/fabrication\/decomposition\/result"\]/,
  );
  assert.match(source, /async fn design_formats/);
  assert.match(source, /fn design_format_catalog_response/);
  assert.match(source, /dd\.fabrication\.design-format-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/design\/formats"/);
  assert.match(source, /async fn design_import_catalog_http/);
  assert.match(source, /fn design_import_catalog_response/);
  assert.match(source, /fn design_import_catalog_contracts/);
  assert.match(source, /fn design_import_translator_readiness_checklist/);
  assert.match(source, /dd\.fabrication\.design-import-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/formats\/catalog"/);
  assert.match(source, /"GET \/fabrication\/design\/import\/catalog"/);
  assert.match(source, /"translatorReadinessChecklist"/);
  assert.match(source, /native-cad-translator-provenance/);
  assert.match(source, /neutral-kernel-and-pmi-preservation/);
  assert.match(source, /mesh-slicer-profile-readiness/);
  assert.match(source, /sheet-profile-and-cam-handoff/);
  assert.match(source, /async fn design_preflight_catalog_http/);
  assert.match(source, /fn design_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.design-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/design\/preflight\/catalog"/);
  assert.match(source, /source-identity-and-provenance-state/);
  assert.match(source, /geometry-units-and-feature-state/);
  assert.match(source, /conversion-simulation-and-learning-state/);
  assert.match(source, /struct DesignImportReviewRequest/);
  assert.match(source, /async fn design_import_review_http/);
  assert.match(source, /fn design_import_review_response/);
  assert.match(source, /dd\.fabrication\.design-import-review\.v1/);
  assert.match(source, /"POST \/fabrication\/design\/import\/review"/);
  assert.match(source, /struct DesignImportResultReviewRequest/);
  assert.match(source, /struct DesignImportResultCheck/);
  assert.match(source, /struct DesignImportResultBoundary/);
  assert.match(source, /struct DesignImportResultArtifact/);
  assert.match(source, /async fn design_import_result_http/);
  assert.match(source, /fn design_import_result_review_response/);
  assert.match(source, /fn store_design_import_result_response/);
  assert.match(source, /dd\.fabrication\.design-import-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.design-import-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/design\/import\/result"/);
  assert.match(source, /"recommendedActionHints"/);
  assert.match(source, /design-import-result-checks-release-blocked/);
  assert.match(source, /fn design_import_priority_dispositions/);
  assert.match(source, /"design-import-priority"/);
  assert.match(source, /"design-import-priority-dispositions"/);
  assert.match(source, /"sourceContextRetained": source_context_retained/);
  assert.match(source, /async fn design_conversion_plan_http/);
  assert.match(source, /fn design_conversion_plan_response/);
  assert.match(source, /dd\.fabrication\.design-conversion-plan\.v1/);
  assert.match(source, /"POST \/fabrication\/design\/convert\/plan"/);
  assert.match(source, /"workerDispatch"/);
  assert.match(source, /struct DesignConversionResultReviewRequest/);
  assert.match(source, /async fn design_conversion_result_http/);
  assert.match(source, /fn design_conversion_result_review_response/);
  assert.match(source, /dd\.fabrication\.design-conversion-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/design\/convert\/result"/);
  assert.match(source, /"design-conversion-result"/);
  assert.match(source, /dd\.fabrication\.design-conversion-learning-outcome-draft\.v1/);
  assert.match(source, /"sourceKind": "design-conversion-result"/);
  assert.match(source, /"neutralExportFormats": neutral_exports/);
  assert.match(source, /"blockerHints": blockers/);
  assert.match(source, /"missingReleaseEvidence": missing_release_evidence/);
  assert.match(source, /fn design_conversion_priority_dispositions/);
  assert.match(source, /"design-conversion-priority"/);
  assert.match(source, /"sourceContextRetained": source_context_retained/);
  assert.match(source, /"design-conversion-priority-dispositions"/);
  assert.match(source, /struct DesignSynthesisResultReviewRequest/);
  assert.match(source, /async fn design_synthesis_result_http/);
  assert.match(source, /fn design_synthesis_priority_dispositions/);
  assert.match(source, /fn design_synthesis_result_review_response/);
  assert.match(source, /dd\.fabrication\.design-synthesis-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/design\/synthesis\/result"/);
  assert.match(source, /"design-synthesis-result"/);
  assert.match(source, /"design-synthesis-priority-dispositions"/);
  assert.match(source, /design-synthesis-priority:candidate-selection-readiness:closed/);
  assert.match(source, /dd\.fabrication\.design-synthesis-learning-outcome-draft\.v1/);
  assert.match(source, /"sourceKind": "design-synthesis-result"/);
  assert.match(source, /"candidateIds": candidates/);
  assert.match(source, /"manufacturingMethodHints": candidates/);
  assert.match(source, /"manufacturabilityEvidenceHints": manufacturability_evidence/);
  assert.match(source, /"professional-cad-converter"/);
  assert.match(source, /"lightweight-cad-pmi-inspector"/);
  assert.match(source, /"native-cad-translator-result-required"/);
  assert.match(source, /design_import_catalog_endpoint_exposes_native_cad_worker_lanes/);
  assert.match(source, /design_import_review_endpoint_reuses_cad_validation_and_redaction/);
  assert.match(source, /design_import_result_endpoint_reviews_boundaries_artifacts_and_learning/);
  assert.match(source, /async fn design_generation_catalog_http/);
  assert.match(source, /fn design_generation_catalog_response/);
  assert.match(source, /fn design_generation_catalog_export_contracts/);
  assert.match(source, /fn design_generation_catalog_handoff_contracts/);
  assert.match(source, /dd\.fabrication\.design-generation-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/design\/generation\/catalog"/);
  assert.match(source, /async fn design_generate_http/);
  assert.match(source, /fn design_generation_response/);
  assert.match(source, /dd\.fabrication\.design-generation\.v1/);
  assert.match(source, /"POST \/fabrication\/design\/generate"/);
  assert.match(source, /fabrication\.design\.generated/);
  assert.match(source, /"designPackage\.parts\.primitive"/);
  assert.match(source, /"manufacturingHandoff\.parts"/);
  assert.match(
    source,
    /design_generation_catalog_endpoint_exposes_package_export_and_handoff_contract/,
  );
  assert.match(source, /design_generation_endpoint_returns_design_package_and_exports/);
  assert.match(source, /async fn handoff_catalog_http/);
  assert.match(source, /fn handoff_catalog_response/);
  assert.match(source, /fn handoff_catalog_lanes/);
  assert.match(source, /dd\.fabrication\.handoff-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/handoff\/catalog"/);
  assert.match(source, /"source-design-conversion"/);
  assert.match(source, /"generated-design-and-cam-export"/);
  assert.match(source, /"machine-program-controller-release"/);
  assert.match(source, /"hybrid-split-combine-assembly"/);
  assert.match(source, /handoff_catalog_endpoint_exposes_worker_lane_contracts/);
  assert.match(source, /struct HandoffResultReviewRequest/);
  assert.match(source, /async fn handoff_result_http/);
  assert.match(source, /fn handoff_result_review_response/);
  assert.match(source, /fn store_handoff_result_response/);
  assert.match(source, /dd\.fabrication\.handoff-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.handoff-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/handoff\/result"/);
  assert.match(source, /handoff-result-segment-release-blocked/);
  assert.match(source, /"segmentHints"/);
  assert.match(source, /"datumHints"/);
  assert.match(source, /"transportHints"/);
  assert.match(source, /handoff-datum-transfers/);
  assert.match(source, /handoff-transport-holds/);
  assert.match(source, /handoff-learning-observations/);
  assert.match(source, /handoff_result_endpoint_reviews_datum_transport_and_learning/);
  assert.match(source, /async fn machine_catalog/);
  assert.match(source, /fn machine_catalog_response/);
  assert.match(source, /fn machine_catalog_instruction_languages/);
  assert.match(source, /languages\.insert\("ctb-resin-job"\.to_string\(\)\)/);
  assert.match(source, /languages\.insert\("photon-resin-job"\.to_string\(\)\)/);
  assert.match(source, /languages\.insert\("lychee-resin-job"\.to_string\(\)\)/);
  assert.match(source, /languages\.insert\("chitubox-resin-job"\.to_string\(\)\)/);
  assert.match(source, /fn machine_catalog_release_gates/);
  assert.match(source, /dd\.fabrication\.machine-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/machines\/catalog"/);
  assert.match(source, /selectionEvidenceMatrix/);
  assert.match(source, /fdm-and-slicer-printers/);
  assert.match(source, /vertical-horizontal-and-indexed-mills/);
  assert.match(source, /routers-and-sheet-cutters/);
  assert.match(source, /lathes-mill-turn-and-swiss/);
  assert.match(source, /horizontal\/rotary access unresolved/);
  assert.match(source, /threading\/feed sync missing/);
  assert.match(source, /machine_catalog_endpoint_exposes_default_fleet_and_release_contract/);
  assert.match(source, /async fn printer_catalog_http/);
  assert.match(source, /fn printer_catalog_response/);
  assert.match(source, /dd\.fabrication\.printer-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/printers\/catalog"/);
  assert.match(source, /printer_catalog_endpoint_exposes_additive_fleet_and_release_contract/);
  assert.match(source, /async fn printer_preflight_catalog_http/);
  assert.match(source, /fn printer_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.printer-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/printers\/preflight\/catalog"/);
  assert.match(source, /"thermal-and-motion-state"/);
  assert.match(source, /"extrusion-material-and-resume-state"/);
  assert.match(source, /"support-orientation-and-first-article-state"/);
  assert.match(source, /printer_preflight_catalog_endpoint_exposes_thermal_extrusion_and_first_layer_gates/);
  assert.match(source, /async fn subtractive_catalog_http/);
  assert.match(source, /fn subtractive_catalog_response/);
  assert.match(source, /dd\.fabrication\.subtractive-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/subtractive\/catalog"/);
  assert.match(source, /async fn subtractive_preflight_catalog_http/);
  assert.match(source, /fn subtractive_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.subtractive-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/subtractive\/preflight\/catalog"/);
  assert.match(source, /stock-workholding-and-datum-state/);
  assert.match(source, /tool-process-and-media-state/);
  assert.match(source, /controller-geometry-and-simulation-state/);
  assert.match(source, /async fn turning_preflight_catalog_http/);
  assert.match(source, /fn turning_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.turning-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/turning\/preflight\/catalog"/);
  assert.match(source, /chuck-collet-bar-stock-and-support-state/);
  assert.match(source, /turning-tooling-offset-and-threading-state/);
  assert.match(source, /mill-turn-live-tool-and-transfer-state/);
  assert.match(source, /turning_preflight_catalog_endpoint_exposes_lathe_mill_turn_release_gates/);
  assert.match(source, /async fn cleanliness_preflight_catalog_http/);
  assert.match(source, /fn cleanliness_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.cleanliness-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/cleanliness\/preflight\/catalog"/);
  assert.match(source, /additive-residue-and-powder-state/);
  assert.match(source, /machining-coolant-chip-and-fod-state/);
  assert.match(source, /assembly-interface-and-release-cleanliness/);
  assert.match(source, /async fn interface_preflight_catalog_http/);
  assert.match(source, /fn interface_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.interface-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/interfaces\/preflight\/catalog"/);
  assert.match(source, /datum-and-locating-interface-state/);
  assert.match(source, /fit-tolerance-and-stackup-state/);
  assert.match(source, /joining-hardware-and-service-interface-state/);
  assert.match(
    source,
    /subtractive_catalog_endpoint_exposes_machining_fleet_and_release_contract/,
  );
  assert.match(
    source,
    /subtractive_preflight_catalog_endpoint_exposes_setup_process_and_simulation_gates/,
  );
  assert.match(
    source,
    /cleanliness_preflight_catalog_endpoint_exposes_residue_fod_and_release_gates/,
  );
  assert.match(
    source,
    /interface_preflight_catalog_endpoint_exposes_datum_stackup_and_join_gates/,
  );
  assert.match(source, /async fn cnc_catalog_http/);
  assert.match(source, /fn cnc_catalog_response/);
  assert.match(source, /dd\.fabrication\.cnc-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/cnc\/catalog"/);
  assert.match(
    source,
    /cnc_catalog_endpoint_exposes_import_generation_and_release_contract/,
  );
  assert.match(source, /async fn cell_catalog_http/);
  assert.match(source, /fn cell_catalog_response/);
  assert.match(source, /dd\.fabrication\.cell-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/cells\/catalog"/);
  assert.match(source, /cell_catalog_endpoint_exposes_hybrid_robotic_and_process_cells/);
  assert.match(source, /async fn machine_select_http/);
  assert.match(source, /fn machine_selection_response/);
  assert.match(source, /dd\.fabrication\.machine-selection\.v1/);
  assert.match(source, /"POST \/fabrication\/machines\/select"/);
  assert.match(source, /fabrication\.machines\.selected/);
  assert.match(source, /"machineSelection\.candidates\.status"/);
  assert.match(source, /machine_selection_endpoint_returns_candidates_and_release_contract/);
  assert.match(source, /async fn controller_catalog_http/);
  assert.match(source, /fn controller_postprocessor_catalog_response/);
  assert.match(source, /fn controller_dialect_assumption_checklist/);
  assert.match(source, /dd\.fabrication\.controller-postprocessor-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/controllers\/catalog"/);
  assert.match(source, /"dialectAssumptionChecklist"/);
  assert.match(source, /modal-defaults-and-reset-state/);
  assert.match(source, /offset-table-and-compensation-state/);
  assert.match(source, /macro-subprogram-and-controller-state/);
  assert.match(source, /postprocessed-output-and-dry-run-proof/);
  assert.match(source, /controller_postprocessor_catalog_endpoint_exposes_controller_release_contract/);
  assert.match(source, /async fn controller_preflight_catalog_http/);
  assert.match(source, /fn controller_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.controller-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/controllers\/preflight\/catalog"/);
  assert.match(source, /"modal-state"/);
  assert.match(source, /"offset-and-setup-state"/);
  assert.match(source, /"program-dependency-state"/);
  assert.match(source, /controller_preflight_catalog_endpoint_exposes_modal_offset_and_macro_gates/);
  assert.match(source, /struct ControllerPostprocessorResultReviewRequest/);
  assert.match(source, /async fn controller_postprocessor_result_http/);
  assert.match(source, /fn controller_postprocessor_result_review_response/);
  assert.match(source, /fn store_controller_postprocessor_result_response/);
  assert.match(source, /dd\.fabrication\.controller-postprocessor-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/controllers\/result"/);
  assert.match(source, /controller-postprocessor-result-target-release-blocked/);
  assert.match(source, /controller-postprocessor-targets/);
  assert.match(source, /controller-postprocessor-checks/);
  assert.match(source, /controller-postprocessor-learning-observations/);
  assert.match(source, /dd\.fabrication\.controller-postprocessor-learning-outcome-draft\.v1/);
  assert.match(source, /"sourceKind": "controller-postprocessor-result"/);
  assert.match(source, /"targetIds": targets/);
  assert.match(source, /"programIds": targets/);
  assert.match(source, /"checkHints": checks/);
  assert.match(
    source,
    /controller_postprocessor_result_endpoint_reviews_targets_checks_and_learning/,
  );
  assert.match(source, /async fn material_catalog_http/);
  assert.match(source, /fn material_catalog_response/);
  assert.match(source, /fn material_catalog_targets/);
  assert.match(source, /fn material_catalog_conditioning/);
  assert.match(source, /fn material_readiness_checklist/);
  assert.match(source, /dd\.fabrication\.material-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/materials\/catalog"/);
  assert.match(source, /"materialPlan\.routeRequirements"/);
  assert.match(source, /"materialReadinessChecklist"/);
  assert.match(source, /lot-certificate-and-traceability/);
  assert.match(source, /conditioning-and-shelf-life-state/);
  assert.match(source, /quantity-scrap-and-runout-capacity/);
  assert.match(source, /machine-material-process-compatibility/);
  assert.match(source, /material-machine-boundary:aluminum/);
  assert.match(source, /material_catalog_endpoint_exposes_feedstock_compatibility_and_release_contract/);
  assert.match(source, /async fn material_plan_http/);
  assert.match(source, /fn material_planning_response/);
  assert.match(source, /dd\.fabrication\.material-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/materials\/plan"/);
  assert.match(source, /fabrication\.materials\.planned/);
  assert.match(source, /"materialPlan\.routeRequirements\.requiredEvidence"/);
  assert.match(source, /struct MaterialResultReviewRequest/);
  assert.match(source, /async fn material_result_http/);
  assert.match(source, /fn material_result_review_response/);
  assert.match(source, /fn stored_material_result_job/);
  assert.match(source, /fn store_material_result_response/);
  assert.match(source, /dd\.fabrication\.material-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.material-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/materials\/result"/);
  assert.match(source, /"materialResultJobId"/);
  assert.match(source, /"materialResult"/);
  assert.match(source, /"lotHints"/);
  assert.match(source, /"conditioningHints"/);
  assert.match(source, /"checkHints"/);
  assert.match(source, /"material-lots"/);
  assert.match(source, /"material-conditioning"/);
  assert.match(source, /"material-learning-observations"/);
  assert.match(source, /material:certificate-missing/);
  assert.match(source, /material-conditioning-status:/);
  assert.match(
    source,
    /material_planning_endpoint_returns_feedstock_conditioning_and_release_contract/,
  );
  assert.match(
    source,
    /material_result_endpoint_reviews_lots_conditioning_artifacts_and_learning/,
  );
  assert.match(source, /async fn instruction_languages/);
  assert.match(source, /fn instruction_language_catalog_response/);
  assert.match(source, /dd\.fabrication\.instruction-language-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/languages"/);
  assert.match(source, /"analysisRoutes"/);
  assert.match(source, /instruction_language_catalog_endpoint_exposes_machine_program_and_review_contract/);
  assert.match(source, /async fn instruction_review_pipeline_catalog_http/);
  assert.match(source, /fn instruction_review_pipeline_catalog_response/);
  assert.match(source, /dd\.fabrication\.instruction-review-pipeline-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/review-pipeline\/catalog"/);
  assert.match(source, /discover-language-and-machine-context/);
  assert.match(source, /retain-import-or-generated-artifact/);
  assert.match(source, /validate-and-find-boundaries/);
  assert.match(source, /improve-or-route-for-human-review/);
  assert.match(source, /simulate-release-and-learn/);
  assert.match(source, /never patch the only copy of an imported instruction stream/);
  assert.match(
    source,
    /instruction_review_pipeline_catalog_orders_import_validation_improvement_release_and_learning/,
  );
  assert.match(source, /fn result_review_catalog_response/);
  assert.match(source, /async fn result_review_catalog_http/);
  assert.match(source, /dd\.fabrication\.result-review-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/results\/catalog"/);
  assert.match(source, /"resultReviewCatalog"/);
  assert.match(source, /"resultReviewFamilies"/);
  assert.match(source, /result_review_catalog_endpoint_exposes_worker_review_routes_and_release_gates/);
  assert.match(source, /async fn instruction_validation_catalog_http/);
  assert.match(source, /async fn instruction_validation_preflight_catalog_http/);
  assert.match(source, /fn instruction_validation_catalog_response/);
  assert.match(source, /fn instruction_validation_preflight_catalog_response/);
  assert.match(source, /fn instruction_validation_stream_readiness_matrix/);
  assert.match(source, /fn instruction_validation_catalog_check_contracts/);
  assert.match(source, /dd\.fabrication\.instruction-validation-catalog\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-validation-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/validation\/catalog"/);
  assert.match(source, /"GET \/fabrication\/instructions\/validation\/preflight\/catalog"/);
  assert.match(source, /"streamReadinessMatrix"/);
  assert.match(source, /imported-cnc-controller-program/);
  assert.match(source, /additive-slicer-or-printer-gcode/);
  assert.match(source, /non-gcode-job-sheet-or-operator-instructions/);
  assert.match(source, /hybrid-split-combine-instruction-package/);
  assert.match(source, /"instructionValidationCatalog"/);
  assert.match(source, /"instructionValidationPreflightCatalog"/);
  assert.match(source, /source-provenance-language-and-dialect-state/);
  assert.match(source, /machine-process-simulation-and-setup-state/);
  assert.match(source, /boundary-improvement-release-and-learning-state/);
  assert.match(source, /"validation\.failureBoundaries"/);
  assert.match(source, /"additive-printer-state"/);
  assert.match(source, /"split-combine-and-release-review"/);
  assert.match(
    source,
    /instruction_validation_catalog_endpoint_exposes_validation_boundary_and_learning_contract/,
  );
  assert.match(
    source,
    /instruction_validation_preflight_catalog_endpoint_exposes_release_blocking_gates/,
  );
  assert.match(source, /async fn instruction_generation_catalog_http/);
  assert.match(source, /fn instruction_generation_catalog_response/);
  assert.match(source, /fn instruction_generation_catalog_program_contracts/);
  assert.match(source, /dd\.fabrication\.instruction-generation-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/generation\/catalog"/);
  assert.match(source, /"generatedPrograms\.instructions"/);
  assert.match(source, /"generatedLanguages": \["sla-job", "resin-job", "ctb-resin-job", "photon-resin-job", "lychee-resin-job", "chitubox-resin-job"/);
  assert.match(source, /"family": "plastic-joining-release"/);
  assert.match(source, /"plastic-joining-job-sheet"/);
  assert.match(source, /"plastic-joining-cell", "manual-or-special-process"/);
  assert.match(source, /"plastic-joining-setup-boundary", "plastic-joining-release-boundary"/);
  assert.match(source, /"lathe-and-mill-turn"/);
  assert.match(source, /async fn instruction_generation_preflight_catalog_http/);
  assert.match(source, /fn instruction_generation_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.instruction-generation-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/generation\/preflight\/catalog"/);
  assert.match(source, /request-design-and-machine-state/);
  assert.match(source, /program-draft-and-controller-state/);
  assert.match(source, /validation-simulation-release-and-learning-state/);
  assert.match(source, /async fn instruction_import_catalog_http/);
  assert.match(source, /fn instruction_import_catalog_response/);
  assert.match(source, /dd\.fabrication\.instruction-import-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/import\/catalog"/);
  assert.match(source, /controller-and-cam-machine-code/);
  assert.match(source, /slicer-and-additive-job-files/);
  assert.match(source, /operator-assembly-postprocess-and-quality-work/);
  assert.match(
    source,
    /instruction_import_catalog_endpoint_exposes_external_instruction_intake_contract/,
  );
  assert.match(source, /struct InstructionImportReviewRequest/);
  assert.match(source, /async fn instruction_import_review_http/);
  assert.match(source, /fn instruction_import_review_response/);
  assert.match(source, /dd\.fabrication\.instruction-import-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-import-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/import\/review"/);
  assert.match(source, /"generatedAtMs": generated_at_ms/);
  assert.match(source, /"importReleaseBlocked"/);
  assert.match(source, /"packageActions"/);
  assert.match(source, /fn stored_instruction_import_review_job/);
  assert.match(source, /fn store_instruction_import_review_response/);
  assert.match(source, /"instruction-import-machine-release"/);
  assert.match(source, /"instruction-import-learning-observations"/);
  assert.match(source, /instruction_import_review_endpoint_packages_submitted_streams_for_validation/);
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/generation\/preflight\/catalog",\s*get\(instruction_generation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/generation\/preflight\/catalog",\s*get\(instruction_generation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /async fn instruction_generate_http/);
  assert.match(source, /fn instruction_generation_response/);
  assert.match(source, /dd\.fabrication\.instruction-generation\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/generate"/);
  assert.match(source, /instruction_generation_endpoint_returns_generated_program_package/);
  assert.match(source, /struct InstructionGenerationResultReviewRequest/);
  assert.match(source, /async fn instruction_generation_result_http/);
  assert.match(source, /fn instruction_generation_priority_dispositions/);
  assert.match(source, /fn instruction_generation_result_review_response/);
  assert.match(source, /fn stored_instruction_generation_result_job/);
  assert.match(source, /fn store_instruction_generation_result_response/);
  assert.match(source, /dd\.fabrication\.instruction-generation-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-generation-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/generation\/result"/);
  assert.match(source, /"generationResultJobId"/);
  assert.match(source, /"instructionGenerationResult"/);
  assert.match(source, /"instruction-generation-artifacts"/);
  assert.match(source, /"instruction-generation-release-update"/);
  assert.match(source, /"instruction-generation-priority-dispositions"/);
  assert.match(source, /instruction-generation-priority:generated-artifact-readiness:closed/);
  assert.match(source, /blocker-count:/);
  assert.match(source, /store_instruction_generation_result_response\(&state, &response\)/);
  assert.match(source, /instruction_generation_result_endpoint_reviews_artifacts_and_learning/);
  assert.match(source, /struct InstructionReviewResultReviewRequest/);
  assert.match(source, /async fn instruction_review_result_http/);
  assert.match(source, /fn instruction_review_result_review_response/);
  assert.match(source, /fn stored_instruction_review_result_job/);
  assert.match(source, /fn store_instruction_review_result_response/);
  assert.match(source, /fn instruction_review_priority_dispositions/);
  assert.match(source, /"priorityDispositions": priority_dispositions/);
  assert.match(source, /instruction-review-priority-dispositions/);
  assert.match(source, /"instruction-review-priority"/);
  assert.match(source, /format!\("\{observation_prefix\}:\{priority_id\}:\{\}"/);
  assert.match(source, /instruction_validation_result_endpoint_reviews_findings_boundaries_and_learning/);
  assert.match(source, /dd\.fabrication\.instruction-review-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-review-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/review\/result"/);
  assert.match(source, /"reviewResultJobId"/);
  assert.match(source, /"instructionReviewResult"/);
  assert.match(source, /"instruction-review-findings"/);
  assert.match(source, /"instruction-review-failure-boundaries"/);
  assert.match(source, /"instruction-review-improvement-drafts"/);
  assert.match(source, /"instructionIntentMap\.reviewPriorities"/);
  assert.match(source, /"instruction-review-release-update"/);
  assert.match(source, /store_instruction_review_result_response\(&state, &response\)/);
  assert.match(source, /instruction-review-boundary-kind:/);
  assert.match(source, /instruction-review-recommended-action:/);
  assert.match(source, /instruction-review-improvement:/);
  assert.match(source, /human-approval-drafts:/);
  assert.match(source, /instruction_review_result_endpoint_reviews_findings_boundaries_and_learning/);
  assert.match(source, /struct InstructionValidationResultReviewRequest/);
  assert.match(source, /async fn instruction_validation_result_http/);
  assert.match(source, /fn instruction_validation_result_review_response/);
  assert.match(source, /fn stored_instruction_validation_result_job/);
  assert.match(source, /fn store_instruction_validation_result_response/);
  assert.match(source, /fn instruction_validation_priority_dispositions/);
  assert.match(source, /instruction-validation-priority-dispositions/);
  assert.match(source, /"instruction-validation-priority"/);
  assert.match(source, /instruction-validation-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /dd\.fabrication\.instruction-validation-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-validation-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/instructions\/validation\/result"/);
  assert.match(source, /"POST \/fabrication\/instructions\/validation\/result"/);
  assert.match(source, /"validationResultJobId"/);
  assert.match(source, /"instructionValidationResult"/);
  assert.match(source, /"instruction-validation-findings"/);
  assert.match(source, /"instruction-validation-boundaries"/);
  assert.match(source, /"instruction-validation-improvements"/);
  assert.match(source, /"instruction-validation-artifacts"/);
  assert.match(source, /"instructionIntentMap\.reviewPriorities"/);
  assert.match(source, /"instruction-validation-learning-observations"/);
  assert.match(source, /instruction-validation-validator:/);
  assert.match(source, /instruction-validation-boundary-code:/);
  assert.match(source, /instruction-validation-improvement:/);
  assert.match(source, /split-or-combine-required:/);
  assert.match(source, /instruction_validation_result_endpoint_reviews_findings_boundaries_and_learning/);
  assert.match(source, /async fn machine_code_catalog_http/);
  assert.match(source, /fn machine_code_catalog_response/);
  assert.match(source, /dd\.fabrication\.machine-code-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/machine-code\/catalog"/);
  assert.match(source, /async fn machine_code_preflight_catalog_http/);
  assert.match(source, /fn machine_code_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.machine-code-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/machine-code\/preflight\/catalog"/);
  assert.match(source, /program-source-and-design-state/);
  assert.match(source, /controller-postprocessor-and-dialect-state/);
  assert.match(source, /machine-setup-toolpath-and-process-state/);
  assert.match(source, /validation-simulation-release-and-learning-state/);
  assert.match(
    source,
    /\.route\("\/machine-code\/catalog", get\(machine_code_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/machine-code\/catalog",\s*get\(machine_code_catalog_http\),\s*\)/,
  );
  assert.match(source, /"programContracts": program_contracts/);
  assert.match(source, /"controllerTargets": controller_targets/);
  assert.match(source, /"machineCodePolicy"/);
  assert.match(source, /fn machine_code_target_selection_matrix/);
  assert.match(source, /"targetSelectionMatrix": machine_code_target_selection_matrix\(\)/);
  assert.match(source, /additive-printer-firmware/);
  assert.match(source, /subtractive-mill-router-controller/);
  assert.match(source, /turning-and-mill-turn-controller/);
  assert.match(source, /sheet-cutting-edm-and-special-process/);
  assert.match(source, /hybrid-assembly-and-human-reviewed-instructions/);
  assert.match(source, /part-off support/);
  assert.match(
    source,
    /machine_code_catalog_endpoint_exposes_program_controller_and_learning_contract/,
  );
  assert.match(
    source,
    /machine_code_preflight_catalog_endpoint_exposes_controller_release_gates/,
  );
  assert.match(source, /async fn machine_code_generate_http/);
  assert.match(source, /fn machine_code_generation_response/);
  assert.match(source, /dd\.fabrication\.machine-code-generation\.v1/);
  assert.match(source, /"POST \/fabrication\/machine-code\/generate"/);
  assert.match(source, /fabrication\.machine_code\.generated/);
  assert.match(source, /machine_code_generation_endpoint_returns_controller_release_package/);
  assert.match(source, /struct MachineCodeResultReviewRequest/);
  assert.match(source, /async fn machine_code_result_http/);
  assert.match(source, /fn machine_code_result_review_response/);
  assert.match(source, /fn stored_machine_code_result_job/);
  assert.match(source, /fn store_machine_code_result_response/);
  assert.match(source, /fn machine_code_priority_dispositions/);
  assert.match(source, /machine-code-priority-dispositions/);
  assert.match(source, /machine-code-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /machine_code_result_endpoint_reviews_priority_dispositions/);
  assert.match(source, /dd\.fabrication\.machine-code-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.machine-code-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/machine-code\/result"/);
  assert.match(source, /"POST \/fabrication\/machine-code\/result"/);
  assert.match(source, /"machineCodeResultJobId"/);
  assert.match(source, /"machineCodeResult"/);
  assert.match(source, /"machine-code-controller-checks"/);
  assert.match(source, /"machine-code-failure-boundaries"/);
  assert.match(source, /"instructionIntentMap\.reviewPriorities"/);
  assert.match(source, /"machine-code-learning-observations"/);
  assert.match(source, /machine-code-check:/);
  assert.match(source, /machine-code-boundary:/);
  assert.match(source, /machine-code-artifact:/);
  assert.match(source, /controller-check-blockers:/);
  assert.match(source, /machine_code_result_endpoint_reviews_controller_checks_and_learning/);
  assert.match(source, /async fn toolpath_catalog_http/);
  assert.match(source, /fn toolpath_catalog_response/);
  assert.match(source, /fn toolpath_catalog_entries/);
  assert.match(source, /dd\.fabrication\.toolpath-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/toolpaths\/catalog"/);
  assert.match(source, /"toolpathCatalog"/);
  assert.match(source, /"sheet-cut-nesting-kerf-pierce-and-retention"/);
  assert.match(source, /"mdp-request\.artifacts\.toolpathCatalog"/);
  assert.match(
    source,
    /toolpath_catalog_endpoint_exposes_path_release_and_learning_contract/,
  );
  assert.match(source, /async fn toolpath_plan_http/);
  assert.match(source, /fn toolpath_planning_response/);
  assert.match(source, /dd\.fabrication\.toolpath-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/toolpaths\/plan"/);
  assert.match(source, /fabrication\.toolpaths\.planned/);
  assert.match(source, /"toolpathPlan\.simulationTrace"/);
  assert.match(
    source,
    /toolpath_planning_endpoint_returns_cam_simulation_and_release_contract/,
  );
  assert.match(source, /struct ToolpathResultReviewRequest/);
  assert.match(source, /async fn toolpath_result_http/);
  assert.match(source, /fn toolpath_result_review_response/);
  assert.match(source, /fn stored_toolpath_result_job/);
  assert.match(source, /fn store_toolpath_result_response/);
  assert.match(source, /dd\.fabrication\.toolpath-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.toolpath-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/toolpaths\/result"/);
  assert.match(source, /"toolpathResultJobId"/);
  assert.match(source, /"toolpathResult"/);
  assert.match(source, /"segmentIds"/);
  assert.match(source, /"simulationIds"/);
  assert.match(source, /"checkStatusHints"/);
  assert.match(source, /"toolpath-segments"/);
  assert.match(source, /"toolpath-simulations"/);
  assert.match(source, /"toolpath-checks"/);
  assert.match(source, /fn toolpath_priority_dispositions/);
  assert.match(source, /"toolpath-priority-dispositions"/);
  assert.match(source, /"toolpath-learning-observations"/);
  assert.match(source, /toolpath-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /toolpath-simulation-status:/);
  assert.match(source, /toolpath-check:/);
  assert.match(source, /toolpath-artifact:/);
  assert.match(source, /dd\.fabrication\.toolpath-learning-outcome-draft\.v1/);
  assert.match(source, /"sourceKind": "toolpath-result"/);
  assert.match(source, /"segmentIds": toolpaths/);
  assert.match(source, /"simulationIds": simulations/);
  assert.match(source, /"dryRunBlockerCount": dry_run_blocker_count/);
  assert.match(
    source,
    /toolpath_result_endpoint_reviews_simulation_checks_artifacts_and_learning/,
  );
  assert.match(source, /async fn instruction_validate_http/);
  assert.match(source, /fn instruction_validation_response/);
  assert.match(source, /dd\.fabrication\.instruction-validation\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/validate"/);
  assert.match(source, /fabrication\.instructions\.validated/);
  assert.match(source, /instruction_validation_endpoint_returns_release_blocking_evidence/);
  assert.match(
    source,
    /instruction_generation_catalog_endpoint_exposes_generated_program_contract/,
  );
  assert.match(
    source,
    /instruction_generation_preflight_catalog_endpoint_exposes_draft_release_gates/,
  );
  assert.match(source, /async fn instruction_improvement_catalog_http/);
  assert.match(source, /fn instruction_improvement_catalog_response/);
  assert.match(source, /fn instruction_improvement_catalog_action_contracts/);
  assert.match(source, /fn instruction_improvement_catalog_patch_operations/);
  assert.match(source, /dd\.fabrication\.instruction-improvement-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/improvements\/catalog"/);
  assert.match(source, /async fn instruction_improvement_preflight_catalog_http/);
  assert.match(source, /fn instruction_improvement_preflight_catalog_response/);
  assert.match(source, /fn instruction_improvement_patch_review_matrix/);
  assert.match(source, /dd\.fabrication\.instruction-improvement-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/improvements\/preflight\/catalog"/);
  assert.match(source, /"patchReviewMatrix"/);
  assert.match(source, /modal-controller-state-repair/);
  assert.match(source, /additive-printer-state-repair/);
  assert.match(source, /non-gcode-evidence-checkpoint/);
  assert.match(source, /split-combine-route-repair/);
  assert.match(source, /source-program-and-finding-state/);
  assert.match(source, /patch-review-and-simulation-state/);
  assert.match(source, /learning-and-release-feedback-state/);
  assert.match(source, /async fn instruction_improve_http/);
  assert.match(source, /fn instruction_improvement_review_response/);
  assert.match(source, /dd\.fabrication\.instruction-improvement-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-improvement-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/improve"/);
  assert.match(source, /"sourceKind": "instruction-improvement-review"/);
  assert.match(source, /"humanReviewPatchCount"/);
  assert.match(source, /"patchActionHints"/);
  assert.match(source, /async fn instruction_boundary_review_http/);
  assert.match(source, /fn instruction_boundary_review_response/);
  assert.match(source, /dd\.fabrication\.instruction-boundary-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-boundary-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/boundaries\/review"/);
  assert.match(source, /"sourceKind": "instruction-boundary-review"/);
  assert.match(source, /"humanInterventionActions"/);
  assert.match(source, /"splitCombineActions"/);
  assert.match(source, /"improvedPrograms\.patchManifest\.operations"/);
  assert.match(source, /"add-structured-text-checkpoints"/);
  assert.match(
    source,
    /instruction_improvement_catalog_endpoint_exposes_patch_and_review_contract/,
  );
  assert.match(
    source,
    /instruction_improvement_preflight_catalog_endpoint_exposes_patch_release_gates/,
  );
  assert.match(
    source,
    /instruction_improvement_review_endpoint_returns_patch_manifest_contract/,
  );
  assert.match(
    source,
    /instruction_boundary_review_endpoint_returns_resolution_and_intervention_contract/,
  );
  assert.match(source, /async fn boundary_catalog_http/);
  assert.match(source, /fn boundary_catalog_response/);
  assert.match(source, /fn boundary_catalog_release_evidence/);
  assert.match(source, /fn boundary_decision_matrix/);
  assert.match(source, /dd\.fabrication\.boundary-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/boundaries\/catalog"/);
  assert.match(source, /"responseSurfaces"/);
  assert.match(source, /"decisionMatrix"/);
  assert.match(source, /machine-failure-stop-or-regenerate/);
  assert.match(source, /human-intervention-or-automation-proof/);
  assert.match(source, /split-combine-or-interface-control/);
  assert.match(source, /record-outcome-before-policy-promotion/);
  assert.match(source, /boundary-decision:learning-feedback/);
  assert.match(source, /"boundary-kind:split-boundary"/);
  assert.match(source, /boundary_catalog_endpoint_exposes_failure_intervention_and_split_combine_contract/);
  assert.match(source, /async fn boundary_preflight_catalog_http/);
  assert.match(source, /fn boundary_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.boundary-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/boundaries\/preflight\/catalog"/);
  assert.match(source, /machine-failure-boundary-evidence-state/);
  assert.match(source, /human-intervention-and-automation-gap-state/);
  assert.match(source, /split-combine-and-remediation-boundary-state/);
  assert.match(
    source,
    /boundary_preflight_catalog_endpoint_exposes_machine_failure_and_split_gates/,
  );
  assert.match(source, /async fn boundary_remediation_catalog_http/);
  assert.match(source, /fn boundary_remediation_catalog_response/);
  assert.match(source, /fn boundary_remediation_contracts/);
  assert.match(source, /dd\.fabrication\.boundary-remediation-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/remediation\/catalog"/);
  assert.match(source, /"boundaryRemediationCatalog"/);
  assert.match(source, /"remediationContracts"/);
  assert.match(source, /"machineReadyAfterRemediation": false/);
  assert.match(source, /remediation-catalog:\{boundary_kind\}/);
  assert.match(
    source,
    /boundary_remediation_catalog_endpoint_exposes_release_actions_and_learning_contract/,
  );
  assert.match(source, /async fn boundary_remediation_plan_http/);
  assert.match(source, /fn instruction_remediation_plan_response/);
  assert.match(source, /fn instruction_remediation_plan_actions/);
  assert.match(source, /dd\.fabrication\.boundary-remediation-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/remediation\/plan"/);
  assert.match(source, /"boundaryRemediationPlan"/);
  assert.match(source, /"remediationPlan"/);
  assert.match(source, /"machineReadyAfterPlan": false/);
  assert.match(
    source,
    /boundary_remediation_plan_endpoint_derives_actions_and_handoffs_from_boundaries/,
  );
  assert.match(source, /async fn boundary_analysis_result_http/);
  assert.match(source, /fn boundary_analysis_priority_dispositions/);
  assert.match(source, /fn boundary_analysis_result_review_response/);
  assert.match(source, /fn stored_boundary_analysis_result_job/);
  assert.match(source, /dd\.fabrication\.boundary-analysis-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.boundary-analysis-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/boundaries\/result"/);
  assert.match(source, /"sourceKind": "boundary-analysis-result"/);
  assert.match(source, /"boundary-analysis-priority-dispositions"/);
  assert.match(source, /boundary-analysis-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /"boundaryAnalysisResult"/);
  assert.match(source, /"boundaryResultJobId"/);
  assert.match(source, /"machineFailureBoundaryCount"/);
  assert.match(source, /"splitCombineDecisionCount"/);
  assert.match(
    source,
    /boundary_analysis_result_endpoint_reviews_machine_failure_split_and_learning/,
  );
  assert.match(source, /async fn boundary_remediation_result_http/);
  assert.match(source, /fn boundary_remediation_priority_dispositions/);
  assert.match(source, /fn boundary_remediation_result_review_response/);
  assert.match(source, /fn stored_boundary_remediation_result_job/);
  assert.match(source, /dd\.fabrication\.boundary-remediation-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.boundary-remediation-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/remediation\/result"/);
  assert.match(source, /"sourceKind": "boundary-remediation-result"/);
  assert.match(source, /"remediation-result-priority-dispositions"/);
  assert.match(source, /boundary-remediation-priority:validation-simulation-proof:blocked/);
  assert.match(source, /"blockerHints"/);
  assert.match(source, /"humanSignoffRequiredCount"/);
  assert.match(source, /"boundaryRemediationResult"/);
  assert.match(source, /"remediationResultJobId"/);
  assert.match(
    source,
    /boundary_remediation_result_endpoint_reviews_actions_artifacts_and_learning/,
  );
  assert.match(source, /async fn decomposition_catalog_http/);
  assert.match(source, /fn decomposition_catalog_response/);
  assert.match(source, /fn decomposition_catalog_target_contracts/);
  assert.match(source, /fn decomposition_catalog_interface_modes/);
  assert.match(source, /dd\.fabrication\.decomposition-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/decomposition\/catalog"/);
  assert.match(source, /"interfaceControlPlan\.controls"/);
  assert.match(source, /decomposition-target:split-boundary-decomposition/);
  assert.match(source, /decomposition_catalog_endpoint_exposes_split_combine_and_interface_contract/);
  assert.match(source, /async fn decomposition_plan_http/);
  assert.match(source, /fn decomposition_planning_response/);
  assert.match(source, /dd\.fabrication\.decomposition-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/decomposition\/plan"/);
  assert.match(source, /fabrication\.decomposition\.planned/);
  assert.match(source, /decomposition_planning_endpoint_returns_split_combine_release_contract/);
  assert.match(source, /struct DecompositionResultReviewRequest/);
  assert.match(source, /async fn decomposition_result_http/);
  assert.match(source, /fn decomposition_priority_dispositions/);
  assert.match(source, /fn decomposition_result_review_response/);
  assert.match(source, /fn stored_decomposition_result_job/);
  assert.match(source, /fn store_decomposition_result_response/);
  assert.match(source, /dd\.fabrication\.decomposition-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.decomposition-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/decomposition\/result"/);
  assert.match(source, /"POST \/fabrication\/decomposition\/result"/);
  assert.match(
    source,
    /\.route\("\/decomposition\/result", post\(decomposition_result_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/decomposition\/result",\s*post\(decomposition_result_http\),\s*\)/,
  );
  assert.match(source, /"decompositionResultJobId"/);
  assert.match(source, /"decompositionResult"/);
  assert.match(source, /"decomposition-route-reviews"/);
  assert.match(source, /"decomposition-split-combine-decisions"/);
  assert.match(source, /"decomposition-priority-dispositions"/);
  assert.match(source, /"decomposition-learning-observations"/);
  assert.match(source, /decomposition-priority:split-combine-boundary-first:blocked/);
  assert.match(source, /decomposition-priority:redesign-or-reroute-required:blocked/);
  assert.match(source, /"splitCombineHints"/);
  assert.match(source, /decomposition-route:/);
  assert.match(source, /decomposition-decision:/);
  assert.match(source, /decomposition-artifact:/);
  assert.match(source, /decomposition_result_endpoint_reviews_split_combine_interfaces_and_learning/);
  assert.match(source, /async fn assembly_catalog_http/);
  assert.match(source, /fn assembly_catalog_response/);
  assert.match(source, /fn assembly_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.assembly-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/assembly\/catalog"/);
  assert.match(source, /"assembly\.assemblyGraph"/);
  assert.match(source, /"join-recipe-and-lock-in"/);
  assert.match(source, /assembly_catalog_endpoint_exposes_recomposition_and_join_contracts/);
  assert.match(source, /async fn assembly_preflight_catalog_http/);
  assert.match(source, /fn assembly_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.assembly-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/assembly\/preflight\/catalog"/);
  assert.match(source, /child-route-package-and-interface-state/);
  assert.match(source, /join-recipe-fixture-and-process-state/);
  assert.match(source, /final-fit-quality-release-and-learning-state/);
  assert.match(
    source,
    /assembly_preflight_catalog_endpoint_exposes_recomposition_release_gates/,
  );
  assert.match(source, /async fn assembly_plan_http/);
  assert.match(source, /fn assembly_planning_response/);
  assert.match(source, /dd\.fabrication\.assembly-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/assembly\/plan"/);
  assert.match(source, /fabrication\.assembly\.planned/);
  assert.match(source, /assembly_planning_endpoint_returns_recomposition_release_contract/);
  assert.match(source, /struct AssemblyPlanningResultReviewRequest/);
  assert.match(source, /async fn assembly_planning_result_http/);
  assert.match(source, /fn assembly_priority_dispositions/);
  assert.match(source, /fn assembly_planning_result_review_response/);
  assert.match(source, /fn stored_assembly_planning_result_job/);
  assert.match(source, /fn store_assembly_planning_result_response/);
  assert.match(source, /dd\.fabrication\.assembly-planning-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.assembly-planning-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/assembly\/result"/);
  assert.match(source, /"sourceKind": "assembly-planning-result"/);
  assert.match(source, /"assemblyResultJobId"/);
  assert.match(source, /"joinKindHints"/);
  assert.match(source, /"interfaceCheckHints"/);
  assert.match(source, /FABRICATION_ASSEMBLY_PLANNING_RESULTS_SUBJECT/);
  assert.match(source, /"assemblyPlanningResult"/);
  assert.match(source, /"assembly-part-routes"/);
  assert.match(source, /"assembly-join-operations"/);
  assert.match(source, /"assembly-split-combine-decisions"/);
  assert.match(source, /"assembly-interface-checks"/);
  assert.match(source, /"assembly-priority-dispositions"/);
  assert.match(source, /"assembly-learning-observations"/);
  assert.match(source, /assembly-priority:recomposition-boundary-first:blocked/);
  assert.match(source, /assembly-priority:human-intervention-required:blocked/);
  assert.match(source, /assembly-part-route:/);
  assert.match(source, /assembly-join:/);
  assert.match(source, /assembly-split-combine:/);
  assert.match(source, /assembly-interface-check:/);
  assert.match(source, /assembly-artifact:/);
  assert.match(
    source,
    /assembly_planning_result_endpoint_reviews_split_combine_interfaces_and_learning/,
  );
  assert.match(source, /struct InterfaceResultReviewRequest/);
  assert.match(source, /async fn interface_result_http/);
  assert.match(source, /fn interface_priority_dispositions/);
  assert.match(source, /fn interface_result_review_response/);
  assert.match(source, /fn stored_interface_result_job/);
  assert.match(source, /fn store_interface_result_response/);
  assert.match(source, /dd\.fabrication\.interface-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.interface-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/interfaces\/result"/);
  assert.match(source, /"sourceKind": "interface-result"/);
  assert.match(source, /"interfaceResultJobId"/);
  assert.match(source, /"interface-join-evidence"/);
  assert.match(source, /"interface-split-combine-decisions"/);
  assert.match(source, /"interface-priority-dispositions"/);
  assert.match(source, /interface-priority:interface-fit-and-datum-first:blocked/);
  assert.match(source, /interface-result:human-intervention-required/);
  assert.match(source, /interface-kind:/);
  assert.match(source, /interface-join:/);
  assert.match(source, /interface-split-combine:/);
  assert.match(source, /interface-artifact:/);
  assert.match(source, /interface_result_endpoint_reviews_fit_join_decisions_and_learning/);
  assert.match(source, /async fn instruction_import_preflight_catalog_http/);
  assert.match(source, /fn instruction_import_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.instruction-import-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/instructions\/import\/preflight\/catalog"/);
  assert.match(source, /source-provenance-language-and-artifact-state/);
  assert.match(source, /machine-controller-setup-and-process-state/);
  assert.match(source, /analysis-validation-simulation-improvement-and-learning-state/);
  assert.match(source, /machineReady remains false/);
  assert.match(source, /instruction_import_preflight_catalog_endpoint_exposes_evidence_gates/);
  assert.match(source, /struct InstructionImportReviewRequest/);
  assert.match(source, /async fn instruction_import_review_http/);
  assert.match(source, /fn instruction_import_review_response/);
  assert.match(source, /dd\.fabrication\.instruction-import-review\.v1/);
  assert.match(source, /"POST \/fabrication\/instructions\/import\/review"/);
  assert.match(source, /"generatedAtMs": generated_at_ms/);
  assert.match(source, /"importReleaseBlocked"/);
  assert.match(source, /"retain-original-instruction-artifacts"/);
  assert.match(source, /"run-validation-and-boundary-review"/);
  assert.match(source, /dd\.fabrication\.instruction-import-learning-outcome-draft\.v1/);
  assert.match(source, /fn instruction_import_review_job_severity/);
  assert.match(source, /fn stored_instruction_import_review_job/);
  assert.match(source, /fn store_instruction_import_review_response/);
  assert.match(source, /"instruction-import-validation"/);
  assert.match(source, /"instruction-import-package-actions"/);
  assert.match(
    source,
    /instruction_import_review_endpoint_packages_submitted_streams_for_validation/,
  );
  assert.match(source, /async fn release_catalog_http/);
  assert.match(source, /fn release_catalog_response/);
  assert.match(source, /fn release_catalog_gate_contracts/);
  assert.match(source, /fn release_catalog_blocker_sources/);
  assert.match(source, /dd\.fabrication\.release-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/release\/catalog"/);
  assert.match(source, /"machineRelease\.blockers"/);
  assert.match(source, /"split-combine-interface-release"/);
  assert.match(source, /async fn release_preflight_catalog_http/);
  assert.match(source, /fn release_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.release-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/release\/preflight\/catalog"/);
  assert.match(source, /manifest-artifact-and-checksum-state/);
  assert.match(source, /machine-controller-simulation-and-process-state/);
  assert.match(source, /quality-disposition-signoff-and-learning-state/);
  assert.match(
    source,
    /release_preflight_catalog_endpoint_exposes_machine_ready_handoff_gates/,
  );
  assert.match(source, /async fn release_preview_http/);
  assert.match(source, /fn release_preview_response/);
  assert.match(source, /fn stored_release_preview_job/);
  assert.match(source, /fn store_release_preview_response/);
  assert.match(source, /dd\.fabrication\.release-preview\.v1/);
  assert.match(source, /"POST \/fabrication\/release\/preview"/);
  assert.match(source, /"release-preview-machine-release"/);
  assert.match(source, /"release-preview-package-plan"/);
  assert.match(source, /release_preview_endpoint_exposes_machine_release_and_package_blockers/);
  assert.match(source, /async fn workflow_plan_http/);
  assert.match(source, /async fn workflow_catalog_http/);
  assert.match(source, /fn workflow_catalog_response/);
  assert.match(source, /fn workflow_catalog_stages/);
  assert.match(source, /dd\.fabrication\.workflow-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/workflow\/catalog"/);
  assert.match(source, /"workflowCatalog"/);
  assert.match(source, /"workerCatalogRoutes"/);
  assert.match(source, /"resultReviewCatalogRoutes"/);
  assert.match(source, /"learningCatalogRoutes"/);
  assert.match(source, /"learningOutcomeRoutes"/);
  assert.match(source, /"GET \/fabrication\/learning\/engines\/catalog"/);
  assert.match(source, /"GET \/fabrication\/learning\/models\/catalog"/);
  assert.match(source, /"GET \/fabrication\/learning\/outcomes"/);
  assert.match(source, /"POST \/fabrication\/learning\/outcomes"/);
  assert.match(source, /"stageResultHandoffs"/);
  assert.match(source, /"POST \/fabrication\/learning\/optimizers\/result"/);
  assert.match(
    source,
    /workflow_catalog_endpoint_exposes_stage_handoffs_and_learning_contract/,
  );
  assert.match(source, /fn workflow_planning_response/);
  assert.match(source, /fn workflow_planning_stage/);
  assert.match(source, /fn workflow_action_queue/);
  assert.match(source, /dd\.fabrication\.workflow-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/workflow\/plan"/);
  assert.match(source, /"workflowPlan"/);
  assert.match(source, /"workflowActionQueue"/);
  assert.match(source, /"workflowActionCount"/);
  assert.match(source, /"instruction-intent-map"/);
  assert.match(source, /"instructionIntentMap": &response\.instruction_intent_map/);
  assert.match(source, /generate-or-review-machine-instructions/);
  assert.match(source, /analyze-remediate-and-simulate-before-release/);
  assert.match(source, /resolve-split-combine-interface-control/);
  assert.match(source, /hold-release-and-record-learning-outcome/);
  assert.match(source, /workflow-action:validation-remediation-simulation/);
  assert.match(source, /"workflow-plan"/);
  assert.match(source, /fabrication\.workflow\.planned/);
  assert.match(
    source,
    /workflow_planning_endpoint_returns_route_evidence_and_learning_handoffs/,
  );
  assert.match(
    source,
    /"workflowPlan": \["POST \/workflow\/plan", "POST \/fabrication\/workflow\/plan"\]/,
  );
  assert.match(source, /release_catalog_endpoint_exposes_machine_ready_package_contract/);
  assert.match(source, /struct ReleaseReadinessResultReviewRequest/);
  assert.match(source, /async fn release_readiness_result_http/);
  assert.match(source, /fn release_readiness_result_review_response/);
  assert.match(source, /fn stored_release_readiness_result_job/);
  assert.match(source, /fn store_release_readiness_result_response/);
  assert.match(source, /dd\.fabrication\.release-readiness-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.release-readiness-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/release\/result"/);
  assert.match(source, /"releaseResultJobId"/);
  assert.match(source, /FABRICATION_RELEASE_READINESS_RESULTS_SUBJECT/);
  assert.match(source, /"releaseReadinessResult"/);
  assert.match(source, /"release-readiness-decisions"/);
  assert.match(source, /"release-readiness-manifest-artifacts"/);
  assert.match(source, /"release-readiness-blockers"/);
  assert.match(source, /"release-readiness-human-interventions"/);
  assert.match(source, /fn release_readiness_priority_dispositions/);
  assert.match(source, /"release-readiness-priority"/);
  assert.match(source, /"release-readiness-priority-dispositions"/);
  assert.match(source, /"release-readiness-learning-observations"/);
  assert.match(source, /release-readiness-decision:/);
  assert.match(source, /release-readiness-blocker:/);
  assert.match(source, /release-readiness-intervention:/);
  assert.match(source, /release-readiness-artifact:/);
  assert.match(source, /release-readiness-priority:final-decision-closure:blocked/);
  assert.match(
    source,
    /release_readiness_result_endpoint_reviews_final_gate_and_stores_artifacts/,
  );
  assert.match(source, /async fn execution_plan_http/);
  assert.match(source, /async fn execution_preflight_catalog_http/);
  assert.match(source, /fn execution_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.execution-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/execution\/preflight\/catalog"/);
  assert.match(source, /program-run-and-machine-state/);
  assert.match(source, /stop-point-human-intervention-and-automation-state/);
  assert.match(source, /monitoring-recovery-and-release-state/);
  assert.match(source, /fn execution_planning_response/);
  assert.match(source, /dd\.fabrication\.execution-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/execution\/plan"/);
  assert.match(source, /fabrication\.execution\.planned/);
  assert.match(source, /"operatorInterventionPlan\.evidenceGates"/);
  assert.match(
    source,
    /execution_preflight_catalog_endpoint_exposes_run_readiness_gates/,
  );
  assert.match(
    source,
    /execution_planning_endpoint_returns_stop_points_operator_actions_and_schedule_contract/,
  );
  assert.match(source, /struct ExecutionResultReviewRequest/);
  assert.match(source, /async fn execution_result_http/);
  assert.match(source, /fn execution_result_review_response/);
  assert.match(source, /fn stored_execution_result_job/);
  assert.match(source, /fn store_execution_result_response/);
  assert.match(source, /dd\.fabrication\.execution-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.execution-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/execution\/result"/);
  assert.match(source, /"sourceKind": "execution-result"/);
  assert.match(source, /"executionResultJobId"/);
  assert.match(source, /"operatorActionHints"/);
  assert.match(source, /"splitCombineHints"/);
  assert.match(source, /FABRICATION_EXECUTION_TELEMETRY_RESULTS_SUBJECT/);
  assert.match(source, /"executionResult"/);
  assert.match(source, /"execution-run-segments"/);
  assert.match(source, /"execution-machine-stops"/);
  assert.match(source, /"execution-operator-interventions"/);
  assert.match(source, /"execution-split-combine-decisions"/);
  assert.match(source, /"execution-learning-observations"/);
  assert.match(source, /execution-stop:/);
  assert.match(source, /execution-operator-action:/);
  assert.match(source, /execution-split-combine:/);
  assert.match(source, /execution-artifact:/);
  assert.match(
    source,
    /execution_result_endpoint_reviews_machine_stops_interventions_and_learning/,
  );
  assert.match(source, /async fn strategy_catalog_http/);
  assert.match(source, /fn strategy_catalog_response/);
  assert.match(source, /fn strategy_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.strategy-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/strategy\/catalog"/);
  assert.match(source, /"strategyCandidates\.score"/);
  assert.match(source, /"mdp-request\.desPomdpSolution"/);
  assert.match(source, /"hybrid-route-candidate-scoring"/);
  assert.match(source, /async fn hybrid_catalog_http/);
  assert.match(source, /fn hybrid_catalog_response/);
  assert.match(source, /dd\.fabrication\.hybrid-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/hybrid\/catalog"/);
  assert.match(
    source,
    /hybrid_catalog_endpoint_exposes_split_combine_method_and_learning_contract/,
  );
  assert.match(source, /async fn manufacturing_method_catalog_http/);
  assert.match(source, /fn manufacturing_method_catalog_response/);
  assert.match(source, /fn manufacturing_method_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.manufacturing-method-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/methods\/catalog"/);
  assert.match(source, /"manufacturingMethodCatalog"/);
  assert.match(source, /"hybrid-split-combine-assembly"/);
  assert.match(
    source,
    /manufacturing_method_catalog_endpoint_exposes_process_families_and_learning_routes/,
  );
  assert.match(source, /async fn subject_catalog_http/);
  assert.match(source, /fn subject_catalog_response/);
  assert.match(source, /fn subject_catalog_lanes/);
  assert.match(source, /dd\.fabrication\.subject-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/subjects\/catalog"/);
  assert.match(source, /"subjectCatalog"/);
  assert.match(source, /"design-conversion-workers"/);
  assert.match(source, /"release-readiness-workers"/);
  assert.match(source, /FABRICATION_INSTRUCTION_SIMULATION_REQUESTS_SUBJECT/);
  assert.match(source, /subject_catalog_endpoint_exposes_worker_subjects_and_queue_groups/);
  assert.match(source, /async fn worker_catalog_http/);
  assert.match(source, /fn worker_catalog_response/);
  assert.match(source, /dd\.fabrication\.worker-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/workers\/catalog"/);
  assert.match(source, /"workerCatalog"/);
  assert.match(source, /worker_catalog_endpoint_exposes_dispatch_lanes_and_review_contracts/);
  assert.match(source, /async fn strategy_recommend_http/);
  assert.match(source, /fn strategy_recommendation_response/);
  assert.match(source, /dd\.fabrication\.strategy-recommendation\.v1/);
  assert.match(source, /"POST \/fabrication\/strategy\/recommend"/);
  assert.match(source, /"learningOutcomeQuality"/);
  assert.match(source, /"policySummary\.successRate"/);
  assert.match(source, /review-learned-route-quality-before-release/);
  assert.match(source, /struct StrategyResultReviewRequest/);
  assert.match(source, /struct StrategyResultRouteReview/);
  assert.match(source, /struct StrategyResultSplitCombineDecision/);
  assert.match(source, /struct StrategyResultLearningUpdate/);
  assert.match(source, /struct StrategyResultArtifact/);
  assert.match(source, /async fn strategy_result_http/);
  assert.match(source, /fn strategy_result_review_response/);
  assert.match(source, /fn store_strategy_result_response/);
  assert.match(source, /dd\.fabrication\.strategy-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.strategy-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/strategy\/result"/);
  assert.match(source, /"recommendedMethodHints"/);
  assert.match(source, /"learningUpdateHints"/);
  assert.match(source, /strategy-result-routes-release-blocked/);
  assert.match(source, /strategy_result_endpoint_reviews_routes_split_combine_and_learning/);
  assert.match(source, /strategy_recommendation_endpoint_exposes_learned_hybrid_preview/);
  assert.match(
    source,
    /strategy_catalog_endpoint_exposes_hybrid_learning_and_policy_contract/,
  );
  assert.match(source, /async fn schedule_catalog_http/);
  assert.match(source, /fn schedule_catalog_response/);
  assert.match(source, /fn schedule_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.schedule-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/schedule\/catalog"/);
  assert.match(source, /"productionPlan\.batches"/);
  assert.match(source, /"machineSchedule\.machineLanes\.utilizationRatio"/);
  assert.match(source, /"desScheduleModel\.laneModels"/);
  assert.match(source, /struct ScheduleResultReviewRequest/);
  assert.match(source, /async fn schedule_result_http/);
  assert.match(source, /fn schedule_result_review_response/);
  assert.match(source, /fn stored_schedule_result_job/);
  assert.match(source, /fn store_schedule_result_response/);
  assert.match(source, /dd\.fabrication\.schedule-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.schedule-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/schedule\/result"/);
  assert.match(source, /"scheduleResultJobId"/);
  assert.match(source, /"scheduleResult"/);
  assert.match(source, /"schedule-lanes"/);
  assert.match(source, /"schedule-des-models"/);
  assert.match(source, /"schedule-learning-observations"/);
  assert.match(source, /schedule:overcapacity/);
  assert.match(source, /schedule-des-status:/);
  assert.match(source, /schedule_catalog_endpoint_exposes_batch_lane_and_des_contract/);
  assert.match(
    source,
    /schedule_result_endpoint_reviews_lanes_holds_des_and_learning/,
  );
  assert.match(source, /async fn simulation_catalog_http/);
  assert.match(source, /fn simulation_catalog_response/);
  assert.match(source, /fn simulation_catalog_risk_contracts/);
  assert.match(source, /fn simulation_catalog_dry_run_contracts/);
  assert.match(source, /dd\.fabrication\.simulation-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/simulation\/catalog"/);
  assert.match(source, /"simulation\.riskProfile"/);
  assert.match(source, /"toolpath-envelope-excursion"/);
  assert.match(source, /simulation_catalog_endpoint_exposes_dry_run_and_risk_contract/);
  assert.match(source, /async fn simulation_preflight_catalog_http/);
  assert.match(source, /fn simulation_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.simulation-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/simulation\/preflight\/catalog"/);
  assert.match(source, /machine-envelope-fixture-and-datum-state/);
  assert.match(source, /controller-process-and-program-state/);
  assert.match(source, /dry-run-release-and-learning-state/);
  assert.match(source, /simulation_preflight_catalog_endpoint_exposes_release_gates/);
  assert.match(source, /async fn simulation_run_http/);
  assert.match(source, /fn simulation_run_response/);
  assert.match(source, /dd\.fabrication\.simulation-run\.v1/);
  assert.match(source, /"POST \/fabrication\/simulation\/run"/);
  assert.match(source, /fabrication\.simulation\.run/);
  assert.match(source, /simulation_run_endpoint_returns_dry_run_boundaries_and_release_contract/);
  assert.match(source, /struct InstructionSimulationResultReviewRequest/);
  assert.match(source, /async fn instruction_simulation_result_http/);
  assert.match(source, /fn instruction_simulation_result_review_response/);
  assert.match(source, /fn stored_instruction_simulation_result_job/);
  assert.match(source, /fn store_instruction_simulation_result_response/);
  assert.match(source, /dd\.fabrication\.instruction-simulation-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.instruction-simulation-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/simulation\/result"/);
  assert.match(source, /"sourceKind": "instruction-simulation-result"/);
  assert.match(source, /"simulationResultJobId"/);
  assert.match(source, /"checkHints"/);
  assert.match(source, /"artifactHints"/);
  assert.match(source, /artifact-evidence-missing:/);
  assert.match(source, /FABRICATION_INSTRUCTION_SIMULATION_RESULTS_SUBJECT/);
  assert.match(source, /"instructionSimulationResult"/);
  assert.match(source, /"instruction-simulation-envelope-checks"/);
  assert.match(source, /"instruction-simulation-failure-boundaries"/);
  assert.match(source, /fn simulation_priority_dispositions/);
  assert.match(source, /"instruction-simulation-priority-dispositions"/);
  assert.match(source, /"instruction-simulation-learning-observations"/);
  assert.match(source, /simulation-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /instruction-simulation-boundary-kind:/);
  assert.match(source, /instruction-simulation-recommended-action:/);
  assert.match(source, /instruction-simulation-artifact:/);
  assert.match(
    source,
    /instruction_simulation_result_endpoint_reviews_boundaries_artifacts_and_learning/,
  );
  assert.match(source, /async fn quality_catalog_http/);
  assert.match(source, /fn quality_catalog_response/);
  assert.match(source, /fn quality_catalog_inspection_contracts/);
  assert.match(source, /fn quality_catalog_measurement_contracts/);
  assert.match(source, /dd\.fabrication\.quality-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/quality\/catalog"/);
  assert.match(source, /"qualityPlan\.inspectionPoints"/);
  assert.match(source, /"interface-fit-and-assembly-lock"/);
  assert.match(source, /quality_catalog_endpoint_exposes_inspection_metrology_and_release_contract/);
  assert.match(source, /async fn quality_preflight_catalog_http/);
  assert.match(source, /fn quality_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.quality-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/quality\/preflight\/catalog"/);
  assert.match(source, /metrology-instrument-and-datum-state/);
  assert.match(source, /first-article-final-fit-and-surface-state/);
  assert.match(source, /nonconformance-disposition-and-learning-state/);
  assert.match(
    source,
    /quality_preflight_catalog_endpoint_exposes_metrology_fit_and_disposition_gates/,
  );
  assert.match(source, /async fn disposition_catalog_http/);
  assert.match(source, /fn disposition_catalog_response/);
  assert.match(source, /fn disposition_catalog_entries/);
  assert.match(source, /dd\.fabrication\.disposition-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/dispositions\/catalog"/);
  assert.match(source, /"dispositionCatalog"/);
  assert.match(
    source,
    /disposition_catalog_endpoint_exposes_rework_scrap_and_split_learning_contract/,
  );
  assert.match(source, /struct DispositionResultReviewRequest/);
  assert.match(source, /struct DispositionResultDecision/);
  assert.match(source, /struct DispositionResultRemediationAction/);
  assert.match(source, /struct DispositionResultAuthorityReview/);
  assert.match(source, /async fn disposition_result_http/);
  assert.match(source, /fn disposition_result_review_response/);
  assert.match(source, /fn store_disposition_result_response/);
  assert.match(source, /dd\.fabrication\.disposition-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/dispositions\/result"/);
  assert.match(source, /disposition-result-decisions-release-blocked/);
  assert.match(source, /dd\.fabrication\.disposition-learning-outcome-draft\.v1/);
  assert.match(
    source,
    /disposition_result_endpoint_reviews_rework_authority_split_and_learning/,
  );
  assert.match(source, /async fn costing_catalog_http/);
  assert.match(source, /fn costing_catalog_response/);
  assert.match(source, /fn costing_catalog_entries/);
  assert.match(source, /dd\.fabrication\.costing-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/costing\/catalog"/);
  assert.match(source, /"material-yield-estimate"/);
  assert.match(source, /"split-combine-route-economics"/);
  assert.match(
    source,
    /costing_catalog_endpoint_exposes_yield_quote_and_split_learning_contract/,
  );
  assert.match(source, /struct CostingResultReviewRequest/);
  assert.match(source, /struct CostingResultReview/);
  assert.match(source, /struct CostingResultYieldReview/);
  assert.match(source, /struct CostingResultRouteComparison/);
  assert.match(source, /async fn costing_result_http/);
  assert.match(source, /fn costing_result_review_response/);
  assert.match(source, /fn store_costing_result_response/);
  assert.match(source, /dd\.fabrication\.costing-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/costing\/result"/);
  assert.match(source, /costing-result-cost-release-blocked/);
  assert.match(source, /costing-route-comparisons/);
  assert.match(source, /costing-learning-observations/);
  assert.match(source, /dd\.fabrication\.costing-learning-outcome-draft\.v1/);
  assert.match(source, /recommendedSubmitRoute/);
  assert.match(
    source,
    /costing_result_endpoint_reviews_yield_routes_and_learning/,
  );
  assert.match(source, /\.route\("\/costing\/result", post\(costing_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/costing\/result", post\(costing_result_http\)\)/,
  );
  assert.match(source, /async fn utilities_catalog_http/);
  assert.match(source, /fn utilities_catalog_response/);
  assert.match(source, /fn utilities_catalog_entries/);
  assert.match(source, /dd\.fabrication\.utilities-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/utilities\/catalog"/);
  assert.match(source, /"sheet-cut-process-support"/);
  assert.match(source, /"coolant-chip-dust-state-record"/);
  assert.match(
    source,
    /utilities_catalog_endpoint_exposes_process_support_and_recovery_learning_contract/,
  );
  assert.match(source, /async fn energy_catalog_http/);
  assert.match(source, /fn energy_catalog_response/);
  assert.match(source, /fn energy_catalog_entries/);
  assert.match(source, /dd\.fabrication\.energy-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/energy\/catalog"/);
  assert.match(source, /"sheet-cut-beam-jet-plasma-and-edm-energy"/);
  assert.match(source, /"power-load-record"/);
  assert.match(source, /"mdp-request\.artifacts\.energy"/);
  assert.match(
    source,
    /energy_catalog_endpoint_exposes_power_load_and_route_learning_contract/,
  );
  assert.match(source, /\.route\("\/energy\/catalog", get\(energy_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/energy\/catalog", get\(energy_catalog_http\)\)/,
  );
  assert.match(source, /struct EnergyResultReviewRequest/);
  assert.match(source, /struct EnergyResultPowerCheck/);
  assert.match(source, /struct EnergyResultThermalCheck/);
  assert.match(source, /struct EnergyResultRecoveryAction/);
  assert.match(source, /async fn energy_result_http/);
  assert.match(source, /fn energy_result_review_response/);
  assert.match(source, /fn store_energy_result_response/);
  assert.match(source, /dd\.fabrication\.energy-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/energy\/result"/);
  assert.match(source, /energy-result-power-release-blocked/);
  assert.match(source, /dd\.fabrication\.energy-learning-outcome-draft\.v1/);
  assert.match(source, /energy-learning-observations/);
  assert.match(
    source,
    /energy_result_endpoint_reviews_power_thermal_recovery_and_learning/,
  );
  assert.match(source, /\.route\("\/energy\/result", post\(energy_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/energy\/result", post\(energy_result_http\)\)/,
  );
  assert.match(source, /struct UtilitiesResultReviewRequest/);
  assert.match(source, /struct UtilitiesResultCheck/);
  assert.match(source, /struct UtilitiesResultRecoveryAction/);
  assert.match(source, /struct UtilitiesResultOutageEvent/);
  assert.match(source, /async fn utilities_result_http/);
  assert.match(source, /fn utilities_result_review_response/);
  assert.match(source, /fn store_utilities_result_response/);
  assert.match(source, /dd\.fabrication\.utilities-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/utilities\/result"/);
  assert.match(source, /utilities-result-utilities-release-blocked/);
  assert.match(source, /dd\.fabrication\.utilities-learning-outcome-draft\.v1/);
  assert.match(
    source,
    /utilities_result_endpoint_reviews_outages_recovery_and_learning/,
  );
  assert.match(source, /\.route\("\/utilities\/result", post\(utilities_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/utilities\/result",\s*post\(utilities_result_http\)\s*\)/,
  );
  assert.match(source, /async fn telemetry_catalog_http/);
  assert.match(source, /fn telemetry_catalog_response/);
  assert.match(source, /fn telemetry_catalog_entries/);
  assert.match(source, /dd\.fabrication\.telemetry-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/telemetry\/catalog"/);
  assert.match(source, /"runtime-boundary-correlation"/);
  assert.match(source, /"boundary:false-negative"/);
  assert.match(
    source,
    /telemetry_catalog_endpoint_exposes_runtime_boundary_and_learning_contract/,
  );
  assert.match(source, /async fn availability_catalog_http/);
  assert.match(source, /fn availability_catalog_response/);
  assert.match(source, /fn availability_catalog_entries/);
  assert.match(source, /dd\.fabrication\.availability-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/availability\/catalog"/);
  assert.match(source, /"live-machine-state-and-queue-capacity"/);
  assert.match(source, /"machineSchedule\.machineLanes"/);
  assert.match(source, /"split-combine-capacity:\*"/);
  assert.match(
    source,
    /availability_catalog_endpoint_exposes_capacity_fallback_and_learning_contract/,
  );
  assert.match(source, /struct AvailabilityResultReviewRequest/);
  assert.match(source, /async fn availability_result_http/);
  assert.match(source, /fn availability_result_review_response/);
  assert.match(source, /fn store_availability_result_response/);
  assert.match(source, /dd\.fabrication\.availability-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/availability\/result"/);
  assert.match(source, /availability-result-machine-window-release-blocked/);
  assert.match(source, /dd\.fabrication\.availability-learning-outcome-draft\.v1/);
  assert.match(source, /availability-fallback-options/);
  assert.match(source, /availability-learning-observations/);
  assert.match(
    source,
    /availability_result_endpoint_reviews_capacity_fallback_and_learning/,
  );
  assert.match(source, /\.route\("\/availability\/result", post\(availability_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/availability\/result",\s*post\(availability_result_http\),?\s*\)/,
  );
  assert.match(source, /async fn maintenance_catalog_http/);
  assert.match(source, /fn maintenance_catalog_response/);
  assert.match(source, /fn maintenance_catalog_entries/);
  assert.match(source, /dd\.fabrication\.maintenance-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/maintenance\/catalog"/);
  assert.match(source, /"lockout-tagout-and-service-release"/);
  assert.match(source, /"machineProfile\.evidence\.maintenance"/);
  assert.match(source, /"stale-sensor-risk:\*"/);
  assert.match(
    source,
    /maintenance_catalog_endpoint_exposes_lockout_service_and_learning_contract/,
  );
  assert.match(source, /struct MaintenanceResultReviewRequest/);
  assert.match(source, /async fn maintenance_result_http/);
  assert.match(source, /fn maintenance_result_review_response/);
  assert.match(source, /fn store_maintenance_result_response/);
  assert.match(source, /dd\.fabrication\.maintenance-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/maintenance\/result"/);
  assert.match(source, /maintenance-result-service-release-blocked/);
  assert.match(source, /dd\.fabrication\.maintenance-learning-outcome-draft\.v1/);
  assert.match(source, /maintenance-lockout-clearances/);
  assert.match(source, /maintenance-learning-observations/);
  assert.match(
    source,
    /maintenance_result_endpoint_reviews_lockout_service_and_learning/,
  );
  assert.match(source, /\.route\("\/maintenance\/result", post\(maintenance_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/maintenance\/result",\s*post\(maintenance_result_http\),?\s*\)/,
  );
  assert.match(source, /struct TelemetryResultReviewRequest/);
  assert.match(source, /struct TelemetryResultSensorWindow/);
  assert.match(source, /struct TelemetryResultBoundaryCorrelation/);
  assert.match(source, /async fn telemetry_result_http/);
  assert.match(source, /fn telemetry_result_review_response/);
  assert.match(source, /fn store_telemetry_result_response/);
  assert.match(source, /dd\.fabrication\.telemetry-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/telemetry\/result"/);
  assert.match(source, /telemetry-result-boundary-correlation-release-blocked/);
  assert.match(source, /dd\.fabrication\.telemetry-learning-outcome-draft\.v1/);
  assert.match(
    source,
    /telemetry_result_endpoint_reviews_runtime_boundaries_and_learning/,
  );
  assert.match(source, /async fn quality_plan_http/);
  assert.match(source, /fn quality_planning_response/);
  assert.match(source, /dd\.fabrication\.quality-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/quality\/plan"/);
  assert.match(source, /fabrication\.quality\.planned/);
  assert.match(source, /"qualityPlan\.inspectionPoints\.recordsToCapture"/);
  assert.match(
    source,
    /quality_planning_endpoint_returns_inspection_metrology_and_release_gates/,
  );
  assert.match(source, /struct QualityResultReviewRequest/);
  assert.match(source, /async fn quality_result_http/);
  assert.match(source, /fn quality_result_review_response/);
  assert.match(source, /fn stored_quality_result_job/);
  assert.match(source, /fn store_quality_result_response/);
  assert.match(source, /dd\.fabrication\.quality-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/quality\/result"/);
  assert.match(source, /"qualityResultJobId"/);
  assert.match(source, /"qualityResult"/);
  assert.match(source, /"quality-measurements"/);
  assert.match(source, /"quality-findings"/);
  assert.match(source, /"quality-inspection-gates"/);
  assert.match(source, /fn quality_priority_dispositions/);
  assert.match(source, /"quality-priority-dispositions"/);
  assert.match(source, /"quality-learning-observations"/);
  assert.match(source, /quality-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /dd\.fabrication\.quality-learning-outcome-draft\.v1/);
  assert.match(source, /quality-measurement-target:/);
  assert.match(source, /quality-finding:/);
  assert.match(source, /quality-gate:/);
  assert.match(source, /quality-artifact:/);
  assert.match(
    source,
    /quality_result_endpoint_reviews_metrology_findings_gates_and_learning/,
  );
  assert.match(source, /async fn calibration_catalog_http/);
  assert.match(source, /fn calibration_catalog_response/);
  assert.match(source, /fn calibration_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.calibration-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/calibration\/catalog"/);
  assert.match(source, /"machineProfile\.profileEvidence\.calibration"/);
  assert.match(source, /calibration_catalog_endpoint_exposes_probe_offset_and_release_contract/);
  assert.match(source, /async fn calibration_plan_http/);
  assert.match(source, /fn calibration_planning_response/);
  assert.match(source, /dd\.fabrication\.calibration-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/calibration\/plan"/);
  assert.match(source, /fabrication\.calibration\.planned/);
  assert.match(source, /"releaseProbePlan\.probes\.requiredBeforeState"/);
  assert.match(source, /calibration_planning_endpoint_returns_probe_offset_and_release_contract/);
  assert.match(source, /struct CalibrationResultReviewRequest/);
  assert.match(source, /async fn calibration_result_http/);
  assert.match(source, /fn calibration_result_review_response/);
  assert.match(source, /fn stored_calibration_result_job/);
  assert.match(source, /fn store_calibration_result_response/);
  assert.match(source, /dd\.fabrication\.calibration-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/calibration\/result"/);
  assert.match(source, /"calibrationResultJobId"/);
  assert.match(source, /"calibrationResult"/);
  assert.match(source, /"calibration-checks"/);
  assert.match(source, /"calibration-offsets"/);
  assert.match(source, /"calibration-probes"/);
  assert.match(source, /fn calibration_priority_dispositions/);
  assert.match(source, /"calibration-priority-dispositions"/);
  assert.match(source, /"calibration-learning-observations"/);
  assert.match(source, /calibration-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /dd\.fabrication\.calibration-learning-outcome-draft\.v1/);
  assert.match(source, /calibration-check:/);
  assert.match(source, /calibration-offset:/);
  assert.match(source, /calibration-probe:/);
  assert.match(source, /calibration-artifact:/);
  assert.match(
    source,
    /calibration_result_endpoint_reviews_offsets_probes_artifacts_and_learning/,
  );
  assert.match(source, /async fn intervention_catalog_http/);
  assert.match(source, /fn intervention_catalog_response/);
  assert.match(source, /fn intervention_catalog_action_contracts/);
  assert.match(source, /fn intervention_catalog_automation_contracts/);
  assert.match(source, /dd\.fabrication\.intervention-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/interventions\/catalog"/);
  assert.match(source, /"operatorInterventionPlan\.requiredOperatorActions"/);
  assert.match(source, /"add-verified-automation"/);
  assert.match(
    source,
    /intervention_catalog_endpoint_exposes_operator_automation_and_execution_contract/,
  );
  assert.match(source, /struct InterventionResultReviewRequest/);
  assert.match(source, /async fn intervention_result_http/);
  assert.match(source, /fn intervention_result_review_response/);
  assert.match(source, /fn store_intervention_result_response/);
  assert.match(source, /dd\.fabrication\.intervention-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.intervention-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/interventions\/result"/);
  assert.match(source, /"interventionResult"/);
  assert.match(source, /intervention-operator-actions/);
  assert.match(source, /intervention-automation-handoffs/);
  assert.match(source, /fn intervention_priority_dispositions/);
  assert.match(source, /intervention-priority-dispositions/);
  assert.match(source, /intervention-learning-observations/);
  assert.match(source, /intervention-priority:machine-failure-boundary-first:blocked/);
  assert.match(
    source,
    /intervention_result_endpoint_reviews_operator_automation_and_learning/,
  );
  assert.match(source, /async fn setup_catalog_http/);
  assert.match(source, /fn setup_catalog_response/);
  assert.match(source, /fn setup_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.setup-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/setup\/catalog"/);
  assert.match(source, /"toolingPlan\.requirements"/);
  assert.match(source, /"fixturePlan\.setups"/);
  assert.match(source, /"monitoringPlan\.alertRules"/);
  assert.match(source, /async fn tooling_catalog_http/);
  assert.match(source, /fn tooling_catalog_response/);
  assert.match(source, /fn tooling_catalog_entries/);
  assert.match(source, /dd\.fabrication\.tooling-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/tooling\/catalog"/);
  assert.match(source, /"toolingCatalog"/);
  assert.match(source, /tooling_catalog_endpoint_exposes_machine_tooling_release_contract/);
  assert.match(source, /struct ToolingResultReviewRequest/);
  assert.match(source, /async fn tooling_result_http/);
  assert.match(source, /fn tooling_priority_dispositions/);
  assert.match(source, /fn tooling_result_review_response/);
  assert.match(source, /fn store_tooling_result_response/);
  assert.match(source, /dd\.fabrication\.tooling-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.tooling-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/tooling\/result"/);
  assert.match(source, /tooling-result-tool-release-blocked/);
  assert.match(source, /"toolLifeHints"/);
  assert.match(source, /"supportMediaHints"/);
  assert.match(source, /tooling-tool-life-checks/);
  assert.match(source, /tooling-priority-dispositions/);
  assert.match(source, /tooling-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /tooling-priority:split-combine-or-interface-review:blocked/);
  assert.match(source, /tooling-learning-observations/);
  assert.match(
    source,
    /tooling_result_endpoint_reviews_tool_offset_life_support_and_learning/,
  );
  assert.match(source, /async fn consumables_catalog_http/);
  assert.match(source, /fn consumables_catalog_response/);
  assert.match(source, /fn consumables_catalog_entries/);
  assert.match(source, /dd\.fabrication\.consumables-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/consumables\/catalog"/);
  assert.match(source, /"tool-life-record"/);
  assert.match(source, /"support-media-depletion:\*"/);
  assert.match(
    source,
    /consumables_catalog_endpoint_exposes_tool_life_material_and_support_media_contract/,
  );
  assert.match(source, /struct ConsumablesResultReviewRequest/);
  assert.match(source, /async fn consumables_result_http/);
  assert.match(source, /fn consumables_priority_dispositions/);
  assert.match(source, /fn consumables_result_review_response/);
  assert.match(source, /fn store_consumables_result_response/);
  assert.match(source, /dd\.fabrication\.consumables-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.consumables-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/consumables\/result"/);
  assert.match(source, /consumables-result-inventory-release-blocked/);
  assert.match(source, /"inventoryHints"/);
  assert.match(source, /"supportMediaHints"/);
  assert.match(source, /consumables-tool-life-checks/);
  assert.match(source, /consumables-priority-dispositions/);
  assert.match(source, /consumables-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /consumables-priority:split-combine-or-interface-review:blocked/);
  assert.match(source, /consumables-learning-observations/);
  assert.match(
    source,
    /consumables_result_endpoint_reviews_capacity_tool_life_and_learning/,
  );
  assert.match(source, /async fn workholding_catalog_http/);
  assert.match(source, /fn workholding_catalog_response/);
  assert.match(source, /fn workholding_catalog_entries/);
  assert.match(source, /dd\.fabrication\.workholding-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/workholding\/catalog"/);
  assert.match(source, /"workholdingCatalog"/);
  assert.match(
    source,
    /workholding_catalog_endpoint_exposes_fixture_release_and_learning_contract/,
  );
  assert.match(source, /async fn workholding_preflight_catalog_http/);
  assert.match(source, /fn workholding_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.workholding-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/workholding\/preflight\/catalog"/);
  assert.match(source, /stock-build-surface-and-primary-hold-state/);
  assert.match(source, /datum-transfer-reprobe-and-clearance-state/);
  assert.match(source, /split-combine-fixture-and-human-intervention-state/);
  assert.match(
    source,
    /workholding_preflight_catalog_endpoint_exposes_fixture_release_gates/,
  );
  assert.match(source, /async fn nesting_catalog_http/);
  assert.match(source, /fn nesting_catalog_response/);
  assert.match(source, /fn nesting_catalog_entries/);
  assert.match(source, /dd\.fabrication\.nesting-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/nesting\/catalog"/);
  assert.match(source, /"nestingCatalog"/);
  assert.match(source, /"designExports\.partExports\.content\.nesting"/);
  assert.match(source, /"dd-sheet-nesting-json"/);
  assert.match(source, /"nesting:hybrid-kit"/);
  assert.match(
    source,
    /nesting_catalog_endpoint_exposes_layout_traceability_and_release_contract/,
  );
  assert.match(source, /struct NestingResultReviewRequest/);
  assert.match(source, /struct NestingResultLayoutCheck/);
  assert.match(source, /struct NestingResultTraceabilityCheck/);
  assert.match(source, /struct NestingResultRetentionCheck/);
  assert.match(source, /async fn nesting_result_http/);
  assert.match(source, /fn nesting_result_review_response/);
  assert.match(source, /fn store_nesting_result_response/);
  assert.match(source, /dd\.fabrication\.nesting-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.nesting-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/nesting\/result"/);
  assert.match(source, /"nestingResult"/);
  assert.match(source, /"retentionHints"/);
  assert.match(source, /nesting-result-layout-release-blocked/);
  assert.match(source, /nesting-traceability-checks/);
  assert.match(source, /nesting-split-combine-holds/);
  assert.match(source, /nesting-learning-observations/);
  assert.match(
    source,
    /nesting_result_endpoint_reviews_layout_traceability_retention_and_learning/,
  );
  assert.match(source, /struct WorkholdingResultReviewRequest/);
  assert.match(source, /async fn workholding_result_http/);
  assert.match(source, /fn workholding_priority_dispositions/);
  assert.match(source, /fn workholding_result_review_response/);
  assert.match(source, /fn store_workholding_result_response/);
  assert.match(source, /dd\.fabrication\.workholding-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.workholding-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/workholding\/result"/);
  assert.match(source, /"fixtureHints"/);
  assert.match(source, /"datumTransferHints"/);
  assert.match(source, /workholding-result-fixture-release-blocked/);
  assert.match(source, /workholding-datum-transfers/);
  assert.match(source, /workholding-split-combine-holds/);
  assert.match(source, /workholding-priority-dispositions/);
  assert.match(source, /workholding-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /workholding-priority:split-combine-or-interface-review:blocked/);
  assert.match(source, /workholding-learning-observations/);
  assert.match(
    source,
    /workholding_result_endpoint_reviews_fixture_datum_split_and_learning/,
  );
  assert.match(source, /async fn support_strategy_catalog_http/);
  assert.match(source, /fn support_strategy_catalog_response/);
  assert.match(source, /fn support_strategy_catalog_entries/);
  assert.match(source, /dd\.fabrication\.support-strategy-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/support-strategies\/catalog"/);
  assert.match(source, /"supportStrategyCatalog"/);
  assert.match(
    source,
    /support_strategy_catalog_endpoint_exposes_orientation_split_and_learning_contract/,
  );
  assert.match(source, /struct SupportStrategyResultReviewRequest/);
  assert.match(source, /struct SupportStrategyResultOrientationReview/);
  assert.match(source, /struct SupportStrategyResultSupportReview/);
  assert.match(source, /async fn support_strategy_result_http/);
  assert.match(source, /fn support_strategy_result_review_response/);
  assert.match(source, /fn store_support_strategy_result_response/);
  assert.match(source, /dd\.fabrication\.support-strategy-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.support-strategy-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/support-strategies\/result"/);
  assert.match(source, /"orientationHints"/);
  assert.match(source, /"interventionHints"/);
  assert.match(source, /support-strategy-result-orientation-release-blocked/);
  assert.match(
    source,
    /support_strategy_result_endpoint_reviews_orientation_support_and_learning/,
  );
  assert.match(source, /async fn process_recipe_catalog_http/);
  assert.match(source, /fn process_recipe_catalog_response/);
  assert.match(source, /fn process_recipe_catalog_entries/);
  assert.match(source, /dd\.fabrication\.process-recipe-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/process-recipes\/catalog"/);
  assert.match(source, /"processRecipeCatalog"/);
  assert.match(source, /process_recipe_catalog_endpoint_exposes_parameter_release_contract/);
  assert.match(source, /struct ProcessRecipeResultReviewRequest/);
  assert.match(source, /struct ProcessRecipeResultRecipeReview/);
  assert.match(source, /struct ProcessRecipeResultParameterCheck/);
  assert.match(source, /async fn process_recipe_result_http/);
  assert.match(source, /fn process_recipe_result_review_response/);
  assert.match(source, /fn store_process_recipe_result_response/);
  assert.match(source, /dd\.fabrication\.process-recipe-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.process-recipe-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/process-recipes\/result"/);
  assert.match(source, /process-recipe-result-recipes-release-blocked/);
  assert.match(source, /"recipeHints"/);
  assert.match(source, /"couponHints"/);
  assert.match(
    source,
    /process_recipe_result_endpoint_reviews_parameters_coupons_and_learning/,
  );
  assert.match(source, /async fn kinematics_catalog_http/);
  assert.match(source, /fn kinematics_catalog_response/);
  assert.match(source, /fn kinematics_catalog_entries/);
  assert.match(source, /dd\.fabrication\.kinematics-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/kinematics\/catalog"/);
  assert.match(source, /"kinematicsCatalog"/);
  assert.match(source, /kinematics_catalog_endpoint_exposes_axis_release_contract/);
  assert.match(source, /struct KinematicsResultReviewRequest/);
  assert.match(source, /struct KinematicsResultAxisCheck/);
  assert.match(source, /struct KinematicsResultCoordinateReview/);
  assert.match(source, /async fn kinematics_result_http/);
  assert.match(source, /fn kinematics_result_review_response/);
  assert.match(source, /fn store_kinematics_result_response/);
  assert.match(source, /dd\.fabrication\.kinematics-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.kinematics-learning-outcome-draft\.v1/);
  assert.match(source, /"axisHints"/);
  assert.match(source, /"coordinateStateHints"/);
  assert.match(source, /"POST \/fabrication\/kinematics\/result"/);
  assert.match(source, /kinematics-result-axis-release-blocked/);
  assert.match(source, /kinematics_result_endpoint_reviews_axes_frames_and_learning/);
  assert.match(source, /async fn tolerance_catalog_http/);
  assert.match(source, /fn tolerance_catalog_response/);
  assert.match(source, /fn tolerance_catalog_entries/);
  assert.match(source, /dd\.fabrication\.tolerance-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/tolerances\/catalog"/);
  assert.match(source, /"toleranceCatalog"/);
  assert.match(source, /tolerance_catalog_endpoint_exposes_fit_stackup_release_contract/);
  assert.match(source, /struct ToleranceResultReviewRequest/);
  assert.match(source, /struct ToleranceResultCheck/);
  assert.match(source, /struct ToleranceResultFitCheck/);
  assert.match(source, /async fn tolerance_result_http/);
  assert.match(source, /fn tolerance_result_review_response/);
  assert.match(source, /fn store_tolerance_result_response/);
  assert.match(source, /dd\.fabrication\.tolerance-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.tolerance-learning-outcome-draft\.v1/);
  assert.match(source, /"toleranceFamilyHints"/);
  assert.match(source, /"compensationHints"/);
  assert.match(source, /"POST \/fabrication\/tolerances\/result"/);
  assert.match(source, /tolerance-result-tolerance-release-blocked/);
  assert.match(source, /tolerance_result_endpoint_reviews_fit_compensation_and_learning/);
  assert.match(source, /async fn process_capability_catalog_http/);
  assert.match(source, /fn process_capability_catalog_response/);
  assert.match(source, /fn process_capability_catalog_entries/);
  assert.match(source, /dd\.fabrication\.process-capability-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/process-capabilities\/catalog"/);
  assert.match(source, /"processCapabilityCatalog"/);
  assert.match(
    source,
    /process_capability_catalog_endpoint_exposes_geometry_release_boundaries/,
  );
  assert.match(source, /struct ProcessCapabilityResultReviewRequest/);
  assert.match(source, /struct ProcessCapabilityResultFinding/);
  assert.match(source, /struct ProcessCapabilityResultAlternateRoute/);
  assert.match(source, /async fn process_capability_result_http/);
  assert.match(source, /fn process_capability_result_review_response/);
  assert.match(source, /fn store_process_capability_result_response/);
  assert.match(source, /dd\.fabrication\.process-capability-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.process-capability-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/process-capabilities\/result"/);
  assert.match(source, /"capabilityFamilyHints"/);
  assert.match(source, /"alternateRouteHints"/);
  assert.match(source, /process-capability-result-findings-release-blocked/);
  assert.match(source, /fn process_capability_priority_dispositions/);
  assert.match(source, /"process-capability-priority-dispositions"/);
  assert.match(source, /process-capability-priority:machine-failure-boundary-first:blocked/);
  assert.match(
    source,
    /process_capability_result_endpoint_reviews_routes_measurements_and_learning/,
  );
  assert.match(source, /async fn manufacturability_catalog_http/);
  assert.match(source, /fn manufacturability_catalog_response/);
  assert.match(source, /fn manufacturability_catalog_entries/);
  assert.match(source, /dd\.fabrication\.manufacturability-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/manufacturability\/catalog"/);
  assert.match(source, /"manufacturabilityCatalog"/);
  assert.match(source, /manufacturability_catalog_endpoint_exposes_dfm_release_boundaries/);
  assert.match(source, /struct ManufacturabilityResultReviewRequest/);
  assert.match(source, /struct ManufacturabilityResultFinding/);
  assert.match(source, /struct ManufacturabilityResultRouteReview/);
  assert.match(source, /async fn manufacturability_result_http/);
  assert.match(source, /fn manufacturability_result_review_response/);
  assert.match(source, /fn store_manufacturability_result_response/);
  assert.match(source, /dd\.fabrication\.manufacturability-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.manufacturability-learning-outcome-draft\.v1/);
  assert.match(source, /"reviewFamilyHints"/);
  assert.match(source, /"decisionHints"/);
  assert.match(source, /"POST \/fabrication\/manufacturability\/result"/);
  assert.match(source, /manufacturability-result-findings-release-blocked/);
  assert.match(source, /manufacturability_result_endpoint_reviews_dfm_split_and_learning/);
  assert.match(source, /async fn failure_mode_catalog_http/);
  assert.match(source, /fn failure_mode_catalog_response/);
  assert.match(source, /fn failure_mode_catalog_entries/);
  assert.match(source, /dd\.fabrication\.failure-mode-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/failure-modes\/catalog"/);
  assert.match(source, /"failureModeCatalog"/);
  assert.match(
    source,
    /failure_mode_catalog_endpoint_exposes_process_failure_learning_contract/,
  );
  assert.match(source, /struct FailureModeResultReviewRequest/);
  assert.match(source, /struct FailureModeResultEvent/);
  assert.match(source, /struct FailureModeResultRecoveryAction/);
  assert.match(source, /struct FailureModeResultIntervention/);
  assert.match(source, /struct FailureModeResultArtifact/);
  assert.match(source, /async fn failure_mode_result_http/);
  assert.match(source, /fn failure_mode_result_review_response/);
  assert.match(source, /fn store_failure_mode_result_response/);
  assert.match(source, /dd\.fabrication\.failure-mode-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.failure-mode-learning-outcome-draft\.v1/);
  assert.match(source, /"failureFamilyHints"/);
  assert.match(source, /"recoveryActionHints"/);
  assert.match(source, /"POST \/fabrication\/failure-modes\/result"/);
  assert.match(source, /failure-mode-result-events-release-blocked/);
  assert.match(source, /fn failure_mode_priority_dispositions/);
  assert.match(source, /"failure-mode-priority-dispositions"/);
  assert.match(source, /failure-mode-priority:machine-failure-boundary-first:blocked/);
  assert.match(
    source,
    /failure_mode_result_endpoint_reviews_recovery_intervention_and_learning/,
  );
  assert.match(source, /async fn safety_catalog_http/);
  assert.match(source, /fn safety_catalog_response/);
  assert.match(source, /fn safety_catalog_entries/);
  assert.match(source, /dd\.fabrication\.safety-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/safety\/catalog"/);
  assert.match(source, /"safetyCatalog"/);
  assert.match(source, /safety_catalog_endpoint_exposes_interlock_release_contract/);
  assert.match(source, /struct SafetyResultReviewRequest/);
  assert.match(source, /struct SafetyResultCheck/);
  assert.match(source, /struct SafetyResultInterlockCheck/);
  assert.match(source, /async fn safety_result_http/);
  assert.match(source, /fn safety_result_review_response/);
  assert.match(source, /fn store_safety_result_response/);
  assert.match(source, /dd\.fabrication\.safety-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.safety-learning-outcome-draft\.v1/);
  assert.match(source, /"interlockHints"/);
  assert.match(source, /"emergencyActionHints"/);
  assert.match(source, /"POST \/fabrication\/safety\/result"/);
  assert.match(source, /safety-result-checks-release-blocked/);
  assert.match(source, /fn safety_priority_dispositions/);
  assert.match(source, /"safety-priority-dispositions"/);
  assert.match(source, /safety-priority:machine-failure-boundary-first:blocked/);
  assert.match(
    source,
    /safety_result_endpoint_reviews_interlocks_emergency_actions_and_learning/,
  );
  assert.match(source, /async fn environment_catalog_http/);
  assert.match(source, /fn environment_catalog_response/);
  assert.match(source, /fn environment_catalog_entries/);
  assert.match(source, /dd\.fabrication\.environment-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/environment\/catalog"/);
  assert.match(source, /"environmentCatalog"/);
  assert.match(source, /environment_catalog_endpoint_exposes_condition_release_contract/);
  assert.match(source, /struct EnvironmentResultReviewRequest/);
  assert.match(source, /struct EnvironmentResultConditionCheck/);
  assert.match(source, /struct EnvironmentResultUtilityCheck/);
  assert.match(source, /async fn environment_result_http/);
  assert.match(source, /fn environment_result_review_response/);
  assert.match(source, /fn store_environment_result_response/);
  assert.match(source, /dd\.fabrication\.environment-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.environment-learning-outcome-draft\.v1/);
  assert.match(source, /"environmentFamilyHints"/);
  assert.match(source, /"conditionScopeHints"/);
  assert.match(source, /"utilityHints"/);
  assert.match(source, /"metrologyHints"/);
  assert.match(source, /"POST \/fabrication\/environment\/result"/);
  assert.match(source, /environment-result-conditions-release-blocked/);
  assert.match(source, /fn environment_priority_dispositions/);
  assert.match(source, /"environment-priority-dispositions"/);
  assert.match(source, /environment-priority:machine-failure-boundary-first:blocked/);
  assert.match(
    source,
    /environment_result_endpoint_reviews_conditions_utilities_metrology_and_learning/,
  );
  assert.match(source, /async fn provenance_catalog_http/);
  assert.match(source, /fn provenance_catalog_response/);
  assert.match(source, /fn provenance_catalog_entries/);
  assert.match(source, /dd\.fabrication\.provenance-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/provenance\/catalog"/);
  assert.match(source, /"provenanceCatalog"/);
  assert.match(source, /provenance_catalog_endpoint_exposes_traceability_release_contract/);
  assert.match(source, /async fn as_built_catalog_http/);
  assert.match(source, /fn as_built_catalog_response/);
  assert.match(source, /fn as_built_catalog_entries/);
  assert.match(source, /dd\.fabrication\.as-built-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/as-built\/catalog"/);
  assert.match(source, /"asBuiltCatalog"/);
  assert.match(source, /"as-built-deviation-map"/);
  assert.match(source, /"hybrid-split-combine-as-built-interface-evidence"/);
  assert.match(source, /as_built_catalog_endpoint_exposes_deviation_scan_and_learning_contract/);
  assert.match(source, /struct AsBuiltResultReviewRequest/);
  assert.match(source, /struct AsBuiltResultMeasurementCheck/);
  assert.match(source, /struct AsBuiltResultDeviationMap/);
  assert.match(source, /struct AsBuiltResultInterfaceCheck/);
  assert.match(source, /async fn as_built_result_http/);
  assert.match(source, /fn as_built_result_review_response/);
  assert.match(source, /fn stored_as_built_result_job/);
  assert.match(source, /fn store_as_built_result_response/);
  assert.match(source, /dd\.fabrication\.as-built-result-review\.v1/);
  assert.match(source, /"POST \/fabrication\/as-built\/result"/);
  assert.match(source, /as-built-result-measurement-release-blocked/);
  assert.match(source, /as-built-deviation-maps/);
  assert.match(source, /fn as_built_priority_dispositions/);
  assert.match(source, /"as-built-priority-dispositions"/);
  assert.match(source, /as-built-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /as-built-learning-observations/);
  assert.match(source, /dd\.fabrication\.as-built-learning-outcome-draft\.v1/);
  assert.match(
    source,
    /as_built_result_endpoint_reviews_deviation_interface_artifacts_and_learning/,
  );
  assert.match(source, /struct ProvenanceResultReviewRequest/);
  assert.match(source, /struct ProvenanceResultLineageCheck/);
  assert.match(source, /struct ProvenanceResultArtifactCheck/);
  assert.match(source, /struct ProvenanceResultCustodyEvent/);
  assert.match(source, /async fn provenance_result_http/);
  assert.match(source, /fn provenance_result_review_response/);
  assert.match(source, /fn store_provenance_result_response/);
  assert.match(source, /dd\.fabrication\.provenance-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.provenance-learning-outcome-draft\.v1/);
  assert.match(source, /"evidenceScopeHints"/);
  assert.match(source, /"custodyEventHints"/);
  assert.match(source, /"POST \/fabrication\/provenance\/result"/);
  assert.match(source, /provenance-result-lineage-release-blocked/);
  assert.match(source, /fn provenance_priority_dispositions/);
  assert.match(source, /"provenance-priority-dispositions"/);
  assert.match(source, /provenance-priority:machine-failure-boundary-first:blocked/);
  assert.match(
    source,
    /provenance_result_endpoint_reviews_lineage_artifacts_and_learning/,
  );
  assert.match(
    source,
    /setup_catalog_endpoint_exposes_tooling_fixture_and_monitoring_contract/,
  );
  assert.match(source, /async fn setup_plan_http/);
  assert.match(source, /fn setup_planning_response/);
  assert.match(source, /dd\.fabrication\.setup-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/setup\/plan"/);
  assert.match(source, /fabrication\.setup\.planned/);
  assert.match(source, /"fixturePlan\.setups\.requiredEvidence"/);
  assert.match(
    source,
    /setup_planning_endpoint_returns_tooling_fixture_monitoring_release_contract/,
  );
  assert.match(source, /struct SetupResultReviewRequest/);
  assert.match(source, /async fn setup_result_http/);
  assert.match(source, /fn setup_priority_dispositions/);
  assert.match(source, /fn setup_result_review_response/);
  assert.match(source, /fn stored_setup_result_job/);
  assert.match(source, /fn store_setup_result_response/);
  assert.match(source, /dd\.fabrication\.setup-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.setup-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/setup\/result"/);
  assert.match(source, /"setupResultJobId"/);
  assert.match(source, /"setupResult"/);
  assert.match(source, /"setup-checks"/);
  assert.match(source, /"setup-datum-transfers"/);
  assert.match(source, /"setup-monitoring-channels"/);
  assert.match(source, /"setup-priority-dispositions"/);
  assert.match(source, /"setup-learning-observations"/);
  assert.match(source, /setup-priority:machine-failure-boundary-first:blocked/);
  assert.match(source, /setup-priority:split-combine-or-interface-review:blocked/);
  assert.match(source, /setup-check:/);
  assert.match(source, /setup-datum:/);
  assert.match(source, /setup-monitoring-channel:/);
  assert.match(source, /setup-artifact:/);
  assert.match(
    source,
    /setup_result_endpoint_reviews_workholding_datum_monitoring_and_learning/,
  );
  assert.match(source, /async fn monitoring_catalog_http/);
  assert.match(source, /fn monitoring_catalog_response/);
  assert.match(source, /fn monitoring_catalog_contracts/);
  assert.match(source, /dd\.fabrication\.monitoring-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/monitoring\/catalog"/);
  assert.match(source, /"GET \/monitoring\/catalog"/);
  assert.match(source, /"monitoringPlan\.monitorPoints"/);
  assert.match(source, /"monitoringPlan\.recoveryActions"/);
  assert.match(source, /"safe-stop-and-restart-governance"/);
  assert.match(
    source,
    /monitoring_catalog_endpoint_exposes_runtime_recovery_contract/,
  );
  assert.match(source, /async fn monitoring_plan_http/);
  assert.match(source, /fn monitoring_planning_response/);
  assert.match(source, /dd\.fabrication\.monitoring-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/monitoring\/plan"/);
  assert.match(source, /fabrication\.monitoring\.planned/);
  assert.match(source, /"monitoringPlan\.monitorPoints\.channels"/);
  assert.match(
    source,
    /monitoring_planning_endpoint_returns_alert_recovery_and_release_contract/,
  );
  assert.match(source, /struct MonitoringResultReviewRequest/);
  assert.match(source, /async fn monitoring_result_http/);
  assert.match(source, /fn monitoring_result_review_response/);
  assert.match(source, /fn stored_monitoring_result_job/);
  assert.match(source, /fn store_monitoring_result_response/);
  assert.match(source, /dd\.fabrication\.monitoring-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.monitoring-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/monitoring\/result"/);
  assert.match(source, /"monitoringResultJobId"/);
  assert.match(source, /"monitoringResult"/);
  assert.match(source, /"monitoring-alerts"/);
  assert.match(source, /"monitoring-channels"/);
  assert.match(source, /"monitoring-recovery-actions"/);
  assert.match(source, /"monitoring-operator-interventions"/);
  assert.match(source, /"monitoring-learning-observations"/);
  assert.match(source, /monitoring-channel:/);
  assert.match(source, /monitoring-alert-severity:/);
  assert.match(source, /monitoring-recovery:/);
  assert.match(source, /monitoring-operator-intervention:/);
  assert.match(
    source,
    /monitoring_result_endpoint_reviews_alerts_recovery_interventions_and_learning/,
  );
  assert.match(source, /async fn postprocess_catalog_http/);
  assert.match(source, /fn postprocess_catalog_response/);
  assert.match(source, /fn postprocess_catalog_target_contracts/);
  assert.match(source, /fn postprocess_catalog_artifact_contracts/);
  assert.match(source, /dd\.fabrication\.postprocess-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/postprocess\/catalog"/);
  assert.match(source, /"postprocessPlan\.requiredArtifacts"/);
  assert.match(source, /"postprocess-traveler"/);
  assert.match(
    source,
    /postprocess_catalog_endpoint_exposes_finishing_traveler_and_release_contract/,
  );
  assert.match(source, /async fn process_catalog_http/);
  assert.match(source, /fn process_catalog_response/);
  assert.match(source, /dd\.fabrication\.process-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/process\/catalog"/);
  assert.match(source, /"additive-print-process"/);
  assert.match(source, /"subtractive-machining-process"/);
  assert.match(source, /"hybrid-split-combine-process"/);
  assert.match(
    source,
    /process_catalog_endpoint_exposes_operation_graph_and_learning_contract/,
  );
  assert.match(source, /async fn postprocess_plan_http/);
  assert.match(source, /fn postprocess_planning_response/);
  assert.match(source, /dd\.fabrication\.postprocess-planning\.v1/);
  assert.match(source, /"POST \/fabrication\/postprocess\/plan"/);
  assert.match(source, /fabrication\.postprocess\.planned/);
  assert.match(source, /"postprocessPlan\.controllerTargets\.gates"/);
  assert.match(
    source,
    /postprocess_planning_endpoint_returns_controller_output_and_traveler_contract/,
  );
  assert.match(source, /struct PostprocessResultReviewRequest/);
  assert.match(source, /async fn postprocess_result_http/);
  assert.match(source, /fn postprocess_result_review_response/);
  assert.match(source, /fn stored_postprocess_result_job/);
  assert.match(source, /fn store_postprocess_result_response/);
  assert.match(source, /dd\.fabrication\.postprocess-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.postprocess-learning-outcome-draft\.v1/);
  assert.match(source, /"targetStatusHints"/);
  assert.match(source, /"travelerStepHints"/);
  assert.match(source, /"signoffHints"/);
  assert.match(source, /"POST \/fabrication\/postprocess\/result"/);
  assert.match(source, /"postprocessResultJobId"/);
  assert.match(source, /"postprocessResult"/);
  assert.match(source, /"postprocess-target-results"/);
  assert.match(source, /"postprocess-gates"/);
  assert.match(source, /"postprocess-traveler-steps"/);
  assert.match(source, /"postprocess-signoffs"/);
  assert.match(source, /"postprocess-learning-observations"/);
  assert.match(source, /postprocess-gate:/);
  assert.match(source, /postprocess-traveler-step:/);
  assert.match(source, /postprocess-signoff:/);
  assert.match(source, /postprocess-artifact:/);
  assert.match(
    source,
    /postprocess_result_endpoint_reviews_traveler_signoff_artifacts_and_learning/,
  );
  assert.match(source, /async fn artifact_catalog_http/);
  assert.match(source, /fn artifact_catalog_response/);
  assert.match(source, /fn artifact_catalog_contracts/);
  assert.match(source, /async fn job_evidence_catalog_http/);
  assert.match(source, /fn job_evidence_catalog_response/);
  assert.match(source, /dd\.fabrication\.job-evidence-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/jobs\/catalog"/);
  assert.match(source, /"jobEvidenceCatalog"/);
  assert.match(source, /"releaseGateMatrix"/);
  assert.match(source, /"releaseGateSummary"/);
  assert.match(source, /"summary\.releaseGateBlockedCount"/);
  assert.match(source, /async fn get_job_release_bundle/);
  assert.match(source, /fn job_release_bundle_response/);
  assert.match(source, /dd\.fabrication\.job-release-bundle\.v1/);
  assert.match(source, /"bundleManifest"/);
  assert.match(source, /let release_gate_matrix = vec!\[/);
  assert.match(source, /"releaseGateSummary": release_gate_summary/);
  assert.match(source, /"releaseGateBlockedCount": release_gate_blocked_count/);
  assert.match(source, /"blockedGateIds": blocked_release_gate_ids/);
  assert.match(source, /"GET \/fabrication\/jobs\/:job_id\/release-bundle"/);
  assert.match(source, /"split\/combine release"/);
  assert.match(source, /"manifestCategoryCount"/);
  assert.match(source, /design-and-source-definition/);
  assert.match(source, /machine-code-and-instruction-programs/);
  assert.match(source, /simulation-quality-and-release-review/);
  assert.match(source, /learning-and-policy-feedback/);
  assert.match(source, /async fn evidence_catalog_http/);
  assert.match(source, /fn evidence_catalog_response/);
  assert.match(source, /dd\.fabrication\.evidence-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/evidence\/catalog"/);
  assert.match(source, /"evidenceCatalog"/);
  assert.match(source, /design-source-evidence/);
  assert.match(source, /instruction-controller-evidence/);
  assert.match(source, /learning-outcome-evidence/);
  assert.match(source, /"releaseGateMatrix": \[/);
  assert.match(source, /"gateId": "source-provenance"/);
  assert.match(source, /"gateId": "human-or-automation-handoff"/);
  assert.match(source, /"restart or split\/combine join evidence"/);
  assert.match(source, /"unattended repeat-run release"/);
  assert.match(source, /machineReady remains false/);
  assert.match(source, /evidence_catalog_endpoint_exposes_release_gate_evidence_taxonomy/);
  assert.match(source, /dd\.fabrication\.artifact-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/artifacts\/catalog"/);
  assert.match(source, /"GET \/fabrication\/jobs\/:job_id\/release-bundle"/);
  assert.match(source, /"job\.releaseBundle"/);
  assert.match(source, /"generated-and-imported-instruction-work"/);
  assert.match(source, /"release-and-execution-evidence"/);
  assert.match(source, /"learning-policy-snapshot"/);
  assert.match(source, /"learning-outcome-memory"/);
  assert.match(source, /"learning-corpus"/);
  assert.match(source, /"mdp-pomdp-neural-learning-evidence"/);
  assert.match(
    source,
    /artifact_catalog_endpoint_exposes_generated_release_and_learning_artifacts/,
  );
  assert.match(source, /async fn fabrication_package_catalog_http/);
  assert.match(source, /fn fabrication_package_catalog_response/);
  assert.match(source, /dd\.fabrication\.package-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/packages\/catalog"/);
  assert.match(source, /"packageCatalog"/);
  assert.match(source, /design-and-source-package/);
  assert.match(source, /instruction-and-controller-package/);
  assert.match(source, /hybrid-boundary-and-release-package/);
  assert.match(source, /releaseHandoffMatrix/);
  assert.match(source, /generated-design-export-release/);
  assert.match(source, /generated-machine-code-release/);
  assert.match(source, /imported-instruction-release/);
  assert.match(source, /improved-instruction-patch-release/);
  assert.match(source, /hybrid-recomposition-release/);
  assert.match(source, /learning-feedback-release/);
  assert.match(source, /immutable original instruction stream/);
  assert.match(source, /attempted release-gate bypass/);
  assert.match(
    source,
    /package_catalog_endpoint_exposes_request_to_release_evidence_contract/,
  );
  assert.match(source, /async fn fabrication_package_plan_http/);
  assert.match(source, /fn fabrication_package_planning_response/);
  assert.match(source, /dd\.fabrication\.package-planning\.v1/);
  assert.match(source, /dd\.fabrication\.package-plan\.v1/);
  assert.match(source, /"POST \/fabrication\/packages\/plan"/);
  assert.match(source, /fabrication\.package\.planned/);
  assert.match(
    source,
    /package_planning_endpoint_projects_request_artifacts_and_release_gates/,
  );
  assert.match(source, /job_evidence_catalog_endpoint_exposes_retained_ledger_contract/);
  assert.match(
    source,
    /job_release_bundle_exposes_design_program_release_and_learning_artifacts/,
  );
  assert.match(source, /async fn learning_capabilities/);
  assert.match(source, /fn learning_capability_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-capability-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/capabilities"/);
  assert.match(source, /"GET \/learning\/engines\/catalog"/);
  assert.match(source, /"GET \/fabrication\/learning\/engines\/catalog"/);
  assert.match(source, /"GET \/fabrication\/learning\/outcomes"/);
  assert.match(source, /"outcomeQualitySurfaces"/);
  assert.match(source, /"learningOutcomes\.qualityBuckets\.policyUse"/);
  assert.match(
    source,
    /"strategyRecommendation\.learningOutcomeQuality\.releasePolicy"/,
  );
  assert.match(source, /learning_capability_catalog_endpoint_exposes_des_mdp_pomdp_and_neural_contract/);
  assert.match(source, /des_engine::des::decision::solve_mdp/);
  assert.match(source, /des_engine::des::decision::solve_pomdp_underlying/);
  assert.match(source, /des_engine::des::studio::StudioModelSpec/);
  assert.match(source, /des_engine::des::general::neural_network::FeedForwardNetwork/);
  assert.match(source, /machine-ready release stays blocked/);
  assert.match(source, /async fn learning_preflight_catalog_http/);
  assert.match(source, /fn learning_preflight_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-preflight-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/preflight\/catalog"/);
  assert.match(
    source,
    /\.route\(\s*"\/learning\/preflight\/catalog",\s*get\(learning_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/learning\/preflight\/catalog",\s*get\(learning_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /learning-outcome-artifact-and-reward-state/);
  assert.match(source, /mdp-pomdp-belief-and-policy-state/);
  assert.match(source, /neural-corpus-quality-and-promotion-state/);
  assert.match(source, /learning_preflight_catalog_endpoint_exposes_policy_promotion_gates/);
  assert.match(source, /async fn learning_feature_catalog_http/);
  assert.match(source, /fn learning_feature_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-feature-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/features\/catalog"/);
  assert.match(source, /plan-route-and-material-state/);
  assert.match(source, /instruction-validation-boundary-state/);
  assert.match(source, /split-combine-interface-state/);
  assert.match(source, /release-evidence-and-outcome-state/);
  assert.match(source, /neural-policy-input-vector/);
  assert.match(source, /hybridDecisionFeatureContracts/);
  assert.match(source, /attempt-single-piece-fabrication/);
  assert.match(source, /split-print-mill-or-turn/);
  assert.match(source, /recompose-and-release-interfaces/);
  assert.match(source, /split-for-printing/);
  assert.match(source, /split-for-milling/);
  assert.match(source, /split-for-turning/);
  assert.match(source, /interface-criticality/);
  assert.match(source, /toolpath-token-sequence/);
  assert.match(source, /learning_feature_catalog_endpoint_exposes_feature_map_for_policy_workers/);
  assert.match(source, /async fn learning_reward_catalog_http/);
  assert.match(source, /fn learning_reward_catalog_response/);
  assert.match(source, /fn learning_reward_catalog_entries/);
  assert.match(source, /dd\.fabrication\.learning-reward-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/rewards\/catalog"/);
  assert.match(source, /"machine-failure:-5"/);
  assert.match(source, /"split-combine-recovery-and-route-improvement"/);
  assert.match(
    source,
    /learning_reward_catalog_endpoint_exposes_reward_terms_and_training_contract/,
  );
  assert.match(source, /async fn learning_model_catalog_http/);
  assert.match(source, /fn learning_model_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-model-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/models\/catalog"/);
  assert.match(source, /"mdp-policy-snapshot"/);
  assert.match(source, /"pomdp-belief-policy"/);
  assert.match(source, /"bounded-neural-policy-sketch"/);
  assert.match(
    source,
    /learning_model_catalog_endpoint_exposes_retained_policy_artifact_contracts/,
  );
  assert.match(source, /async fn learning_replay_catalog_http/);
  assert.match(source, /fn learning_replay_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-replay-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/replay\/catalog"/);
  assert.match(source, /"failure-boundary-and-human-intervention-regression"/);
  assert.match(source, /"machine-route-and-controller-regression"/);
  assert.match(source, /"outcome-quality-and-reward-counterfactual"/);
  assert.match(
    source,
    /learning_replay_catalog_endpoint_exposes_policy_promotion_replay_contract/,
  );
  assert.match(source, /async fn learning_belief_catalog_http/);
  assert.match(source, /fn learning_belief_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-belief-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/beliefs\/catalog"/);
  assert.match(source, /"pomdpBeliefState\.hiddenStates"/);
  assert.match(source, /"releaseProbePlan\.probes"/);
  assert.match(source, /"mdp-request\.desPomdpSpec"/);
  assert.match(source, /learning_belief_catalog_endpoint_exposes_pomdp_probe_contract/);
  assert.match(source, /async fn learning_optimizer_catalog_http/);
  assert.match(source, /fn learning_optimizer_catalog_response/);
  assert.match(source, /dd\.fabrication\.learning-optimizer-catalog\.v1/);
  assert.match(source, /"GET \/fabrication\/learning\/optimizers\/catalog"/);
  assert.match(source, /"mdp-route-action-optimizer"/);
  assert.match(source, /"pomdp-hidden-risk-optimizer"/);
  assert.match(source, /"des-schedule-capacity-optimizer"/);
  assert.match(source, /"bounded-neural-policy-optimizer"/);
  assert.match(source, /"expectedReward"/);
  assert.match(source, /"simulationVerified=true"/);
  assert.match(
    source,
    /learning_optimizer_catalog_endpoint_exposes_candidate_review_contracts/,
  );
  assert.match(source, /struct LearningModelResultReviewRequest/);
  assert.match(source, /fn learning_model_result_review_response/);
  assert.match(source, /fn learning_model_card_compatibility_review/);
  assert.match(source, /async fn learning_model_result_http/);
  assert.match(source, /dd\.fabrication\.learning-model-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.learning-model-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/learning\/models\/result"/);
  assert.match(source, /"learning-model-promotion-blockers"/);
  assert.match(source, /"learning-model-card-compatibility"/);
  assert.match(source, /learning_model_result_endpoint_retains_blocked_policy_artifacts/);
  assert.match(source, /learning_model_result_blocks_neural_promotion_on_feature_schema_mismatch/);
  assert.match(source, /struct LearningOptimizerResultReviewRequest/);
  assert.match(source, /fn learning_optimizer_result_review_response/);
  assert.match(source, /async fn learning_optimizer_result_http/);
  assert.match(source, /dd\.fabrication\.learning-optimizer-result-review\.v1/);
  assert.match(source, /dd\.fabrication\.learning-optimizer-learning-outcome-draft\.v1/);
  assert.match(source, /"POST \/fabrication\/learning\/optimizers\/result"/);
  assert.match(source, /"learning-optimizer-promotion-blockers"/);
  assert.match(
    source,
    /learning_optimizer_result_endpoint_reviews_candidate_promotion_and_learning/,
  );
  assert.match(source, /"defaultMachines": default_machines\(\)/);
  assert.match(source, /fn accepted_instruction_languages/);
  assert.match(source, /accepted_instruction_languages_cover_generated_default_program_languages/);
  assert.match(source, /"metal-pbf-printer"/);
  assert.match(source, /"pellet-fgf-printer"/);
  assert.match(source, /"paste-extrusion-printer"/);
  assert.match(source, /"bound-metal-fff-printer"/);
  assert.match(source, /"multi-material-fdm-printer"/);
  assert.match(source, /"material-jetting-printer"/);
  assert.match(source, /"directed-energy-deposition-cell"/);
  assert.match(source, /"continuous-fiber-composite-printer"/);
  assert.match(source, /"binder-jet-printer"/);
  assert.match(source, /"mill-turn-center"/);
  assert.match(source, /"robotic-assembly-cell"/);
  assert.match(source, /"wire-edm-sheet-cutter"/);
  assert.match(source, /"sinker-edm-cell"/);
  assert.match(source, /"precision-grinder"/);
  assert.match(source, /"cmm-inspection-cell"/);
  assert.match(source, /"thermal-postprocess-furnace"/);
  assert.match(source, /"acceptedInstructionKinds"/);
  assert.match(source, /"iso-gcode"/);
  assert.match(source, /"siemens-sinumerik"/);
  assert.match(source, /"heidenhain-conversational"/);
  assert.match(source, /"mazak-mazatrol"/);
  assert.match(source, /"okuma-osp"/);
  assert.match(source, /"linuxcnc"/);
  assert.match(source, /sinumerik_entry/);
  assert.match(source, /Some\("controller-gcode"\)/);
  assert.match(source, /siemens-sinumerik-postprocessor/);
  assert.match(source, /heidenhain-conversational-postprocessor/);
  assert.match(source, /mazatrol-conversational-postprocessor/);
  assert.match(source, /okuma-osp-postprocessor/);
  assert.match(source, /linuxcnc-gcode-postprocessor/);
  assert.match(source, /"apt-cldata"/);
  assert.match(source, /"cldata-toolpath"/);
  assert.match(source, /"cutter-location-file"/);
  assert.match(source, /"postprocessor-deck"/);
  assert.match(source, /"cam-intermediate-instruction"/);
  assert.match(
    source,
    /attach-cam-source-setup-tool-table-and-cutter-location-provenance/,
  );
  assert.match(
    source,
    /attach-postprocessor-deck-controller-target-and-translated-program-review/,
  );
  assert.match(source, /"slicer-job"/);
  assert.match(source, /"sla-job"/);
  assert.match(source, /"ctb-resin-job"/);
  assert.match(source, /"photon-resin-job"/);
  assert.match(source, /"lychee-resin-job"/);
  assert.match(source, /"chitubox-resin-job"/);
  assert.match(
    source,
    /attach-resin-exposure-layer-manifest-peel-lift-and-wash-cure-release-evidence/,
  );
  assert.match(source, /"pellet-fgf-job"/);
  assert.match(source, /"paste-extrusion-job"/);
  assert.match(source, /"clay-print-job"/);
  assert.match(source, /"bound-metal-fff-job"/);
  assert.match(source, /"metal-filament-job"/);
  assert.match(source, /"multi-material-fdm-job"/);
  assert.match(source, /"ams-mmu-job"/);
  assert.match(source, /"idex-toolchanger-job"/);
  assert.match(source, /"material-jetting-job"/);
  assert.match(source, /"directed-energy-deposition-job"/);
  assert.match(source, /"composite-fiber-job"/);
  assert.match(source, /"sls-job"/);
  assert.match(source, /"powder-job"/);
  assert.match(source, /"binder-jet-job"/);
  assert.match(source, /"mill-turn-gcode"/);
  assert.match(source, /"mill-turn-job"/);
  assert.match(source, /"lathe-job"/);
  assert.match(source, /"turning-job"/);
  assert.match(source, /"assembly-cell-job"/);
  assert.match(source, /"assembly-checklist"/);
  assert.match(source, /"part-separation-checklist"/);
  assert.match(source, /"laser-job"/);
  assert.match(source, /"waterjet-job"/);
  assert.match(source, /"plasma-job"/);
  assert.match(source, /"wire-edm-job"/);
  assert.match(source, /"sinker-edm-job"/);
  assert.match(source, /"grinding-job"/);
  assert.match(source, /"surface-grinder-job"/);
  assert.match(source, /"cylindrical-grinder-job"/);
  assert.match(source, /"cmm-inspection-job"/);
  assert.match(source, /"vision-inspection-job"/);
  assert.match(source, /"metrology-job"/);
  assert.match(source, /"thermal-postprocess-job"/);
  assert.match(source, /"furnace-job"/);
  assert.match(source, /"heat-treatment-job"/);
  assert.match(source, /dimensional-inspection-job-sheet/);
  assert.match(source, /dimensional-inspection-release/);
  assert.match(source, /thermal-postprocess-job-sheet/);
  assert.match(source, /thermal-postprocess-release/);
  assert.match(source, /"acceptedLanguages": accepted_instruction_languages\(\)/);
  assert.match(source, /"safetyBoundaryClasses"/);
  assert.match(source, /"machine-profile-evidence"/);
  assert.match(source, /"machine-profile-blocker"/);
  assert.match(source, /fn intake_guide\(\) -> Value/);
  assert.match(source, /fn intake_request_package_checklist\(\) -> Value/);
  assert.match(source, /async fn intake_catalog_http/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.intake-catalog\.v1"/);
  assert.match(source, /"routes": \["GET \/intake\/catalog", "GET \/fabrication\/intake\/catalog"\]/);
  assert.match(source, /"requestPackageChecklist": intake_request_package_checklist\(\)/);
  assert.match(source, /design-source-and-intent/);
  assert.match(source, /instruction-source-and-controller-state/);
  assert.match(source, /analysis-simulation-and-boundary-review/);
  assert.match(source, /release-package-and-learning-feedback/);
  assert.match(source, /"releasePolicy": \[/);
  assert.match(source, /learning observations can bias future plans but do not bypass release gates/);
  assert.match(source, /intake_guide_exposes_release_gated_fabrication_flow/);
  assert.match(
    source,
    /intake_request_package_checklist_exposes_design_instruction_release_and_learning_evidence/,
  );
  assert.match(source, /fn request_templates\(\) -> Value/);
  assert.match(source, /async fn request_templates_catalog_http/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.request-templates-catalog\.v1"/);
  assert.match(source, /"routes": \["GET \/templates\/catalog", "GET \/fabrication\/templates\/catalog"\]/);
  assert.match(source, /"releaseCatalog": "\/fabrication\/release\/catalog"/);
  assert.match(source, /"releaseGateHints": \[/);
  assert.match(source, /controllerPlan\.releaseGates/);
  assert.match(source, /decompositionPlan\.releaseGates/);
  assert.match(source, /releasePackagePlan\.releaseGates/);
  assert.match(source, /learningFeedbackRetained/);
  assert.match(source, /"templateId": "fdm-print-functional-part"/);
  assert.match(source, /"templateVersion": "v1"/);
  assert.match(source, /templateId and templateVersion are trace labels/);
  assert.match(source, /fdm-print-functional-part/);
  assert.match(source, /native-cad-intake-review/);
  assert.match(source, /POST \/fabrication\/design\/import\/review/);
  assert.match(source, /design-import-review/);
  assert.match(source, /native-cad-translation/);
  assert.match(source, /neutral-export-review/);
  assert.match(source, /SOLIDWORKS/);
  assert.match(source, /PTC Creo/);
  assert.match(source, /design-to-machine-code-generation/);
  assert.match(source, /POST \/fabrication\/design\/generate/);
  assert.match(source, /machine-code-fdm-slicer-handoff/);
  assert.match(source, /machine-code-cnc-controller-handoff/);
  assert.match(source, /POST \/fabrication\/machine-code\/generate/);
  assert.match(source, /generated-fdm-instruction-handoff/);
  assert.match(source, /generated-cnc-instruction-handoff/);
  assert.match(source, /POST \/fabrication\/instructions\/generate/);
  assert.match(source, /instructionGeneration\.generatedPrograms/);
  assert.match(source, /request_template_instruction_generation_bodies_match_plan_contract/);
  assert.match(source, /instruction generation template should match plan schema/);
  assert.match(source, /instruction generation template should require dry run/);
  assert.match(source, /imported-cnc-dry-run-simulation/);
  assert.match(source, /POST \/fabrication\/simulation\/run/);
  assert.match(source, /simulation-dry-run/);
  assert.match(source, /simulation\.programs\.axisExtents/);
  assert.match(source, /request_template_simulation_run_body_matches_plan_contract/);
  assert.match(source, /simulation template should include imported instructions/);
  assert.match(source, /machine-code-generation/);
  assert.match(source, /slicer-profile-handoff/);
  assert.match(source, /postprocessor-handoff/);
  assert.match(source, /decomposition-planning/);
  assert.match(source, /assembly-planning/);
  assert.match(source, /interface-control/);
  assert.match(source, /hybrid-route-costing-result/);
  assert.match(source, /POST \/fabrication\/costing\/result/);
  assert.match(source, /costing-result/);
  assert.match(source, /split-combine-route-economics/);
  assert.match(source, /costingResult\.routeComparisons/);
  assert.match(source, /costingLearningOutcomeDraft/);
  assert.match(source, /request_template_costing_result_body_matches_review_contract/);
  assert.match(source, /costing result template should match review schema/);
  assert.match(source, /operator-intervention-result-feedback/);
  assert.match(source, /POST \/fabrication\/interventions\/result/);
  assert.match(source, /operator-checkpoint-review/);
  assert.match(source, /automation-fallback-review/);
  assert.match(source, /interventionResult\.operatorActions/);
  assert.match(source, /interventionResult\.automationHandoffs/);
  assert.match(source, /interventionLearningOutcomeDraft/);
  assert.match(source, /request_template_intervention_result_body_matches_review_contract/);
  assert.match(source, /intervention result template should match review schema/);
  assert.match(source, /runtime-monitoring-result-feedback/);
  assert.match(source, /POST \/fabrication\/monitoring\/result/);
  assert.match(source, /unattended-run-review/);
  assert.match(source, /safe-stop-recovery/);
  assert.match(source, /monitoringResult\.channels/);
  assert.match(source, /monitoringResult\.alerts/);
  assert.match(source, /monitoringLearningOutcomeDraft/);
  assert.match(source, /request_template_monitoring_result_body_matches_review_contract/);
  assert.match(source, /monitoring result template should match review schema/);
  assert.match(source, /quality-metrology-result-feedback/);
  assert.match(source, /POST \/fabrication\/quality\/result/);
  assert.match(source, /metrology-review/);
  assert.match(source, /split-combine-quality-review/);
  assert.match(source, /qualityResult\.measurements/);
  assert.match(source, /qualityResult\.inspectionGates/);
  assert.match(source, /qualityLearningOutcomeDraft/);
  assert.match(source, /request_template_quality_result_body_matches_review_contract/);
  assert.match(source, /quality result template should match review schema/);
  assert.match(source, /release-readiness-result-feedback/);
  assert.match(source, /POST \/fabrication\/release\/result/);
  assert.match(source, /machine-release-review/);
  assert.match(source, /release-manifest-review/);
  assert.match(source, /releaseReadinessResult\.decisions/);
  assert.match(source, /releaseReadinessResult\.blockers/);
  assert.match(source, /releaseReadinessLearningOutcomeDraft/);
  assert.match(source, /request_template_release_result_body_matches_review_contract/);
  assert.match(source, /release result template should match review schema/);
  assert.match(source, /instruction-improvement/);
  assert.match(source, /controller-patch-review/);
  assert.match(source, /instruction-generation/);
  assert.match(source, /designExports\.reviewGates/);
  assert.match(source, /postprocessPlan\.releaseGates/);
  assert.match(source, /machineCode\.releaseGates/);
  assert.match(source, /postprocessPlan\.controllerTargets/);
  assert.match(source, /decompositionPlan\.routeContracts/);
  assert.match(source, /assemblyPlan\.splitCombineDecisions/);
  assert.match(source, /improvedPrograms\.patchManifest/);
  assert.match(source, /request_template_plan_bodies_match_fabrication_plan_contract/);
  assert.match(source, /request_template_design_import_bodies_match_review_contract/);
  assert.match(source, /request_template_instruction_bodies_match_analysis_contract/);
  assert.match(source, /request_template_instruction_improvement_body_matches_analysis_contract/);
  assert.match(source, /hybrid_request_template_keeps_split_combine_part_routes/);
  assert.match(source, /split_combine_route_templates_match_plan_contract/);
  assert.match(source, /request_template_learning_bodies_match_outcome_contract/);
  assert.match(source, /serde_json::from_value\(request\)/);
  assert.match(source, /design import template request should match review schema/);
  assert.match(source, /instruction template request should match analysis schema/);
  assert.match(source, /instruction improvement template should match analysis schema/);
  assert.match(source, /instruction improvement template should include arc geometry needing review/);
  assert.match(source, /instruction templates should include \{expected\}/);
  assert.match(source, /hybrid template should include \{expected\} part route/);
  assert.match(source, /split\/combine route template should include \{expected\} part route/);
  assert.match(source, /template request part \{\} should include description/);
  assert.match(source, /learning template request should match outcome schema/);
  assert.match(source, /learning templates should include \{expected\}/);
  assert.match(source, /imported-cnc-program-review/);
  assert.match(source, /imported-cnc-improvement-review/);
  assert.match(source, /POST \/fabrication\/instructions\/improve/);
  assert.match(source, /imported-printer-gcode-review/);
  assert.match(source, /imported-resin-job-review/);
  assert.match(source, /imported-powder-bed-build-review/);
  assert.match(source, /machine-code-improvement/);
  assert.match(source, /slicer-gcode-validation/);
  assert.match(source, /resin-job-validation/);
  assert.match(source, /powder-bed-build-validation/);
  assert.match(source, /improvedProgramReview/);
  assert.match(source, /temperatureStateEvidence/);
  assert.match(source, /extrusionStateEvidence/);
  assert.match(source, /resinPostprocessEvidence/);
  assert.match(source, /recoaterClearanceEvidence/);
  assert.match(source, /powderHandlingEvidence/);
  assert.match(source, /vertical-mill-fixture-plate/);
  assert.match(source, /horizontal-mill-side-feature/);
  assert.match(source, /clearanceSweepEvidence/);
  assert.match(source, /lathe-turned-insert/);
  assert.match(source, /hybrid-printed-milled-turned-assembly/);
  assert.match(source, /hybrid-decomposition-plan/);
  assert.match(source, /hybrid-assembly-plan/);
  assert.match(source, /hybrid-outcome-learning-feedback/);
  assert.match(source, /boundary-failure-learning-feedback/);
  assert.match(source, /POST \/fabrication\/decomposition\/plan/);
  assert.match(source, /POST \/fabrication\/assembly\/plan/);
  assert.match(source, /POST \/fabrication\/learning\/outcomes/);
  assert.match(source, /mdp-pomdp-feedback/);
  assert.match(source, /neural-training-example/);
  assert.match(source, /learning\.outcomeMemory/);
  assert.match(source, /learning\.boundaryMemory/);
  assert.match(source, /remediation-risk-learning/);
  assert.match(source, /boundary-kind:machine-failure/);
  assert.match(source, /human-intervention-required/);
  assert.match(source, /split-combine-boundary:split-required/);
  assert.match(source, /operationSequence/);
  assert.match(source, /rewardHint/);
  assert.match(source, /request_templates_cover_core_machine_classes/);
  assert.match(source, /async fn request_schema/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.request-schema\.v1"/);
  assert.match(source, /"intakeGuide": intake_guide\(\)/);
  assert.match(source, /"step": "discover"/);
  assert.match(source, /"step": "review-design-inputs"/);
  assert.match(source, /"step": "attach-machine-profile"/);
  assert.match(source, /"step": "analyze-or-generate-instructions"/);
  assert.match(source, /"step": "plan-hybrid-build"/);
  assert.match(source, /"step": "release-and-learn"/);
  assert.match(source, /"split\/combine candidates"/);
  assert.match(source, /"planRequest"/);
  assert.match(source, /"profileEvidence"/);
  assert.match(source, /"machineProfileEvidence"/);
  assert.match(source, /"instructionProgram"/);
  assert.match(source, /async fn examples/);
  assert.match(source, /"schemaVersion": "dd\.fabrication\.examples\.v1"/);
  assert.match(source, /"templateDrivenPlan"/);
  assert.match(source, /"sourceCatalog": "GET \/fabrication\/templates\/catalog"/);
  assert.match(source, /"releaseCatalog": "GET \/fabrication\/release\/catalog"/);
  assert.match(
    source,
    /"releasePreflightCatalog": \["GET \/release\/preflight\/catalog", "GET \/fabrication\/release\/preflight\/catalog"\]/,
  );
  assert.match(source, /dd\.fabrication\.release-preflight-catalog\.v1/);
  assert.match(source, /release_preflight_catalog_endpoint_exposes_machine_ready_handoff_gates/);
  assert.match(source, /"templateTrace": \{/);
  assert.match(source, /"retainWith": \["job", "artifacts", "releasePackagePlan", "learningOutcome"\]/);
  assert.match(source, /"hybridPlan"/);
  assert.match(source, /"instructionAnalysis"/);
  assert.match(source, /async fn list_jobs/);
  assert.match(source, /struct FabricationJobDetail/);
  assert.match(source, /release_gate_summary: Value/);
  assert.match(source, /release_bundle_route: String/);
  assert.match(source, /fn release_gate_summaries\(&self\) -> Vec<Value>/);
  assert.match(source, /"releaseGateSummaries": release_gate_summaries/);
  assert.match(source, /release_bundle_route: format!\("\/fabrication\/jobs\/\{job_id\}\/release-bundle"\)/);
  assert.match(source, /"releaseBundleRoute": format!\("\/fabrication\/jobs\/\{\}\/release-bundle"/);
  assert.match(source, /async fn get_artifact/);
  assert.match(source, /async fn learning_observe_http/);
  assert.match(source, /fn learning_policy_response/);
  assert.match(source, /dd\.fabrication\.learning-policy-snapshot\.v1/);
  assert.match(source, /learning_policy_endpoint_exposes_self_describing_policy_snapshot/);
  assert.match(source, /"promotionPolicy"/);
  assert.match(source, /async fn learning_policy_http/);
  assert.match(source, /fn learning_corpus_response/);
  assert.match(source, /async fn learning_corpus_http/);
  assert.match(source, /dd\.fabrication\.learning-corpus\.v1/);
  assert.match(source, /learning_corpus_endpoint_exposes_neural_training_examples/);
  assert.match(source, /mdp-request\.artifacts\.neuralTrainingCorpus/);
  assert.match(source, /async fn learning_outcomes_http/);
  assert.match(source, /fn learning_outcomes_memory_response/);
  assert.match(source, /dd\.fabrication\.learning-outcome-memory\.v1/);
  assert.match(source, /"qualitySummary"/);
  assert.match(source, /"qualityBuckets"/);
  assert.match(source, /"failed-or-negative-reward"/);
  assert.match(source, /"intervention-heavy"/);
  assert.match(source, /"policyImpactPreview"/);
  assert.match(source, /"methodCombinationPreferences"/);
  assert.match(source, /"machineKindPreferences"/);
  assert.match(source, /"operationSequencePreferences"/);
  assert.match(source, /split_combine_preferences: Vec<LearningPreference>/);
  assert.match(source, /"splitCombinePreferences"/);
  assert.match(source, /fn outcome_split_combine_keys/);
  assert.match(source, /"remediationRisks"/);
  assert.match(source, /"neuralTrainingExamples"/);
  assert.match(source, /learning_outcomes_memory_endpoint_exposes_bounded_records_and_policy_snapshot/);
  assert.match(source, /\.route\("\/jobs", get\(list_jobs\)\)/);
  assert.match(source, /\.route\("\/fabrication\/jobs", get\(list_jobs\)\)/);
  assert.match(source, /\.route\("\/capabilities", get\(capabilities\)\)/);
  assert.match(source, /\.route\("\/fabrication\/capabilities", get\(capabilities\)\)/);
  assert.match(source, /\.route\("\/machines\/catalog", get\(machine_catalog\)\)/);
  assert.match(source, /\.route\("\/fabrication\/machines\/catalog", get\(machine_catalog\)\)/);
  assert.match(source, /\.route\("\/printers\/catalog", get\(printer_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/printers\/catalog", get\(printer_catalog_http\)\)/);
  assert.match(source, /\.route\("\/subtractive\/catalog", get\(subtractive_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/subtractive\/catalog",\s*get\(subtractive_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/subtractive\/preflight\/catalog",\s*get\(subtractive_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/subtractive\/preflight\/catalog",\s*get\(subtractive_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/turning\/preflight\/catalog",\s*get\(turning_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/turning\/preflight\/catalog",\s*get\(turning_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/cleanliness\/preflight\/catalog",\s*get\(cleanliness_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/cleanliness\/preflight\/catalog",\s*get\(cleanliness_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/interfaces\/preflight\/catalog",\s*get\(interface_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/interfaces\/preflight\/catalog",\s*get\(interface_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/cnc\/catalog", get\(cnc_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/cnc\/catalog", get\(cnc_catalog_http\)\)/);
  assert.match(source, /\.route\("\/machines\/select", post\(machine_select_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/machines\/select", post\(machine_select_http\)\)/,
  );
  assert.match(source, /\.route\("\/controllers\/catalog", get\(controller_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/controllers\/catalog",\s*get\(controller_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/controllers\/result",\s*post\(controller_postprocessor_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/controllers\/result",\s*post\(controller_postprocessor_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/materials\/catalog", get\(material_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/materials\/catalog",\s*get\(material_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/materials\/plan", post\(material_plan_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/materials\/plan", post\(material_plan_http\)\)/,
  );
  assert.match(source, /\.route\("\/design\/formats", get\(design_formats\)\)/);
  assert.match(source, /\.route\("\/fabrication\/design\/formats", get\(design_formats\)\)/);
  assert.match(
    source,
    /\.route\("\/slicers\/catalog", get\(slicer_profile_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/slicers\/catalog",\s*get\(slicer_profile_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\("\/slicers\/result", post\(slicer_profile_result_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/slicers\/result",\s*post\(slicer_profile_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\("\/formats\/catalog", get\(design_import_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/formats\/catalog",\s*get\(design_import_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\("\/design\/import\/catalog", get\(design_import_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/import\/catalog",\s*get\(design_import_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/design\/preflight\/catalog",\s*get\(design_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/preflight\/catalog",\s*get\(design_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/design\/import\/review", post\(design_import_review_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/import\/review",\s*post\(design_import_review_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/design\/import\/result", post\(design_import_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/import\/result",\s*post\(design_import_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/design\/convert\/plan",\s*post\(design_conversion_plan_http\),?\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/convert\/plan",\s*post\(design_conversion_plan_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/design\/convert\/result",\s*post\(design_conversion_result_http\),?\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/convert\/result",\s*post\(design_conversion_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/design\/synthesis\/result",\s*post\(design_synthesis_result_http\),?\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/synthesis\/result",\s*post\(design_synthesis_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/design\/generation\/catalog",\s*get\(design_generation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/design\/generation\/catalog",\s*get\(design_generation_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/design\/generate", post\(design_generate_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/design\/generate", post\(design_generate_http\)\)/,
  );
  assert.match(source, /\.route\("\/handoff\/catalog", get\(handoff_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/handoff\/catalog", get\(handoff_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/handoff\/result", post\(handoff_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/handoff\/result", post\(handoff_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/workers\/catalog", get\(worker_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/workers\/catalog", get\(worker_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/instructions\/languages", get\(instruction_languages\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/languages",\s*get\(instruction_languages\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/validation\/catalog",\s*get\(instruction_validation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/validation\/catalog",\s*get\(instruction_validation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/validation\/preflight\/catalog",\s*get\(instruction_validation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/validation\/preflight\/catalog",\s*get\(instruction_validation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/generation\/catalog",\s*get\(instruction_generation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/generation\/catalog",\s*get\(instruction_generation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/generation\/preflight\/catalog",\s*get\(instruction_generation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/generation\/preflight\/catalog",\s*get\(instruction_generation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/import\/catalog",\s*get\(instruction_import_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/import\/catalog",\s*get\(instruction_import_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/import\/review",\s*post\(instruction_import_review_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/import\/review",\s*post\(instruction_import_review_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/instructions\/generate", post\(instruction_generate_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/generate",\s*post\(instruction_generate_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/generation\/result",\s*post\(instruction_generation_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/generation\/result",\s*post\(instruction_generation_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/validation\/result",\s*post\(instruction_validation_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/validation\/result",\s*post\(instruction_validation_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/machine-code\/catalog", get\(machine_code_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/machine-code\/catalog",\s*get\(machine_code_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/machine-code\/preflight\/catalog",\s*get\(machine_code_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/machine-code\/preflight\/catalog",\s*get\(machine_code_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/machine-code\/generate", post\(machine_code_generate_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/machine-code\/generate",\s*post\(machine_code_generate_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/machine-code\/result", post\(machine_code_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/machine-code\/result",\s*post\(machine_code_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/toolpaths\/catalog", get\(toolpath_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/toolpaths\/catalog", get\(toolpath_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/toolpaths\/plan", post\(toolpath_plan_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/toolpaths\/plan", post\(toolpath_plan_http\)\)/,
  );
  assert.match(source, /\.route\("\/materials\/result", post\(material_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/materials\/result", post\(material_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/toolpaths\/result", post\(toolpath_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/toolpaths\/result", post\(toolpath_result_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/improvements\/catalog",\s*get\(instruction_improvement_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/improvements\/catalog",\s*get\(instruction_improvement_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/improvements\/preflight\/catalog",\s*get\(instruction_improvement_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/improvements\/preflight\/catalog",\s*get\(instruction_improvement_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/instructions\/improve", post\(instruction_improve_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/improve",\s*post\(instruction_improve_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/boundaries\/review",\s*post\(instruction_boundary_review_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/boundaries\/review",\s*post\(instruction_boundary_review_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/boundaries\/catalog", get\(boundary_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/boundaries\/catalog",\s*get\(boundary_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/boundaries\/preflight\/catalog",\s*get\(boundary_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/boundaries\/preflight\/catalog",\s*get\(boundary_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/remediation\/catalog",\s*get\(boundary_remediation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/remediation\/catalog",\s*get\(boundary_remediation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/remediation\/plan",\s*post\(boundary_remediation_plan_http\)\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/remediation\/plan",\s*post\(boundary_remediation_plan_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/decomposition\/catalog", get\(decomposition_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/decomposition\/catalog",\s*get\(decomposition_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/decomposition\/plan", post\(decomposition_plan_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/decomposition\/plan",\s*post\(decomposition_plan_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/assembly\/catalog", get\(assembly_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/assembly\/catalog", get\(assembly_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/assembly\/preflight\/catalog",\s*get\(assembly_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/assembly\/preflight\/catalog",\s*get\(assembly_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/cells\/catalog", get\(cell_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/cells\/catalog", get\(cell_catalog_http\)\)/);
  assert.match(source, /\.route\("\/assembly\/plan", post\(assembly_plan_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/assembly\/plan", post\(assembly_plan_http\)\)/,
  );
  assert.match(source, /\.route\("\/assembly\/result", post\(assembly_planning_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/assembly\/result",\s*post\(assembly_planning_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/interfaces\/result", post\(interface_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/interfaces\/result",\s*post\(interface_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/instructions\/import\/preflight\/catalog",\s*get\(instruction_import_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/import\/preflight\/catalog",\s*get\(instruction_import_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/boundaries\/result", post\(boundary_analysis_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/boundaries\/result",\s*post\(boundary_analysis_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/release\/catalog", get\(release_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/release\/catalog", get\(release_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/release\/preflight\/catalog",\s*get\(release_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/release\/preflight\/catalog",\s*get\(release_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/methods\/catalog", get\(manufacturing_method_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/methods\/catalog",\s*get\(manufacturing_method_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/schedule\/catalog", get\(schedule_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/schedule\/catalog", get\(schedule_catalog_http\)\)/);
  assert.match(source, /\.route\("\/schedule\/result", post\(schedule_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/schedule\/result", post\(schedule_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/simulation\/catalog", get\(simulation_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/simulation\/catalog",\s*get\(simulation_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/simulation\/preflight\/catalog",\s*get\(simulation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/simulation\/preflight\/catalog",\s*get\(simulation_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/simulation\/run", post\(simulation_run_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/simulation\/run", post\(simulation_run_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/simulation\/result",\s*post\(instruction_simulation_result_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/simulation\/result",\s*post\(instruction_simulation_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/telemetry\/result", post\(telemetry_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/telemetry\/result", post\(telemetry_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/quality\/catalog", get\(quality_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/quality\/catalog", get\(quality_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/quality\/preflight\/catalog",\s*get\(quality_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/quality\/preflight\/catalog",\s*get\(quality_preflight_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/quality\/plan", post\(quality_plan_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/quality\/plan", post\(quality_plan_http\)\)/);
  assert.match(source, /\.route\("\/quality\/result", post\(quality_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/quality\/result", post\(quality_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/calibration\/plan", post\(calibration_plan_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/calibration\/plan", post\(calibration_plan_http\)\)/,
  );
  assert.match(source, /\.route\("\/calibration\/result", post\(calibration_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/calibration\/result",\s*post\(calibration_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/interventions\/catalog", get\(intervention_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/interventions\/catalog",\s*get\(intervention_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/interventions\/result", post\(intervention_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/interventions\/result",\s*post\(intervention_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/setup\/catalog", get\(setup_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/setup\/catalog",\s*get\(setup_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/tooling\/catalog", get\(tooling_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/tooling\/catalog", get\(tooling_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/tooling\/result", post\(tooling_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/tooling\/result", post\(tooling_result_http\)\)/,
  );
  assert.match(
    source,
    /\.route\("\/consumables\/catalog", get\(consumables_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/consumables\/catalog",\s*get\(consumables_catalog_http\),?\s*\)/,
  );
  assert.match(source, /\.route\("\/consumables\/result", post\(consumables_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/consumables\/result",\s*post\(consumables_result_http\),?\s*\)/,
  );
  assert.match(source, /\.route\("\/workholding\/result", post\(workholding_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/workholding\/result",\s*post\(workholding_result_http\),?\s*\)/,
  );
  assert.match(source, /\.route\("\/nesting\/catalog", get\(nesting_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/nesting\/catalog", get\(nesting_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/nesting\/result", post\(nesting_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/nesting\/result", post\(nesting_result_http\)\)/,
  );
  assert.match(
    source,
    /\.route\("\/process-recipes\/catalog", get\(process_recipe_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/process-recipes\/catalog",\s*get\(process_recipe_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/kinematics\/catalog", get\(kinematics_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/kinematics\/catalog",\s*get\(kinematics_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/tolerances\/catalog", get\(tolerance_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/tolerances\/catalog",\s*get\(tolerance_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/tolerances\/result", post\(tolerance_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/tolerances\/result",\s*post\(tolerance_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/safety\/catalog", get\(safety_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/safety\/catalog", get\(safety_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/safety\/result", post\(safety_result_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/safety\/result", post\(safety_result_http\)\)/);
  assert.match(source, /\.route\("\/environment\/catalog", get\(environment_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/environment\/catalog",\s*get\(environment_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/environment\/result", post\(environment_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/environment\/result",\s*post\(environment_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/provenance\/catalog", get\(provenance_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/provenance\/catalog",\s*get\(provenance_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/as-built\/catalog", get\(as_built_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/as-built\/catalog",\s*get\(as_built_catalog_http\),?\s*\)/,
  );
  assert.match(source, /\.route\("\/as-built\/result", post\(as_built_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/as-built\/result",\s*post\(as_built_result_http\),?\s*\)/,
  );
  assert.match(source, /\.route\("\/provenance\/result", post\(provenance_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/provenance\/result",\s*post\(provenance_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/setup\/plan", post\(setup_plan_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/setup\/plan", post\(setup_plan_http\)\)/);
  assert.match(source, /\.route\("\/setup\/result", post\(setup_result_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/setup\/result", post\(setup_result_http\)\)/);
  assert.match(source, /\.route\("\/monitoring\/plan", post\(monitoring_plan_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/monitoring\/plan", post\(monitoring_plan_http\)\)/);
  assert.match(source, /\.route\("\/monitoring\/result", post\(monitoring_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/monitoring\/result",\s*post\(monitoring_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/postprocess\/catalog", get\(postprocess_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/postprocess\/catalog",\s*get\(postprocess_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/process\/catalog", get\(process_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/process\/catalog", get\(process_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/postprocess\/plan", post\(postprocess_plan_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/postprocess\/plan", post\(postprocess_plan_http\)\)/,
  );
  assert.match(source, /\.route\("\/postprocess\/result", post\(postprocess_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/postprocess\/result",\s*post\(postprocess_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/evidence\/catalog", get\(evidence_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/evidence\/catalog", get\(evidence_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/artifacts\/catalog", get\(artifact_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/artifacts\/catalog", get\(artifact_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\("\/packages\/catalog", get\(fabrication_package_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/packages\/catalog",\s*get\(fabrication_package_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/packages\/plan", post\(fabrication_package_plan_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/packages\/plan",\s*post\(fabrication_package_plan_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/learning\/capabilities", get\(learning_capabilities\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/learning\/capabilities",\s*get\(learning_capabilities\),\s*\)/,
  );
  assert.match(source, /\.route\("\/intake\/catalog", get\(intake_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/intake\/catalog", get\(intake_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\("\/templates\/catalog", get\(request_templates_catalog_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/templates\/catalog",\s*get\(request_templates_catalog_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/schema", get\(request_schema\)\)/);
  assert.match(source, /\.route\("\/fabrication\/schema", get\(request_schema\)\)/);
  assert.match(source, /\.route\("\/examples", get\(examples\)\)/);
  assert.match(source, /\.route\("\/fabrication\/examples", get\(examples\)\)/);
  assert.match(source, /\.route\("\/jobs\/catalog", get\(job_evidence_catalog_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/jobs\/catalog", get\(job_evidence_catalog_http\)\)/,
  );
  assert.match(source, /\.route\("\/jobs\/:job_id", get\(get_job\)\)/);
  assert.match(source, /\.route\("\/fabrication\/jobs\/:job_id", get\(get_job\)\)/);
  assert.match(
    source,
    /\.route\("\/jobs\/:job_id\/release-bundle", get\(get_job_release_bundle\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/jobs\/:job_id\/release-bundle",\s*get\(get_job_release_bundle\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\("\/jobs\/:job_id\/artifacts\/:artifact_id", get\(get_artifact\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/jobs\/:job_id\/artifacts\/:artifact_id",\s*get\(get_artifact\),\s*\)/,
  );
  assert.match(source, /\.route\("\/plan", post\(plan_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/plan", post\(plan_http\)\)/);
  assert.match(source, /\.route\("\/workflow\/catalog", get\(workflow_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/workflow\/catalog", get\(workflow_catalog_http\)\)/);
  assert.match(source, /\.route\("\/workflow\/plan", post\(workflow_plan_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/workflow\/plan", post\(workflow_plan_http\)\)/);
  assert.match(source, /\.route\("\/release\/preview", post\(release_preview_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/release\/preview",\s*post\(release_preview_http\)\s*\)/,
  );
  assert.match(source, /\.route\("\/release\/result", post\(release_readiness_result_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/release\/result",\s*post\(release_readiness_result_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/execution\/plan", post\(execution_plan_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/execution\/preflight\/catalog",\s*get\(execution_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/execution\/preflight\/catalog",\s*get\(execution_preflight_catalog_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\("\/fabrication\/execution\/plan", post\(execution_plan_http\)\)/,
  );
  assert.match(source, /\.route\("\/execution\/result", post\(execution_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/execution\/result", post\(execution_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/hybrid\/catalog", get\(hybrid_catalog_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/hybrid\/catalog", get\(hybrid_catalog_http\)\)/);
  assert.match(source, /\.route\("\/strategy\/recommend", post\(strategy_recommend_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/strategy\/recommend",\s*post\(strategy_recommend_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/strategy\/result", post\(strategy_result_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/strategy\/result", post\(strategy_result_http\)\)/,
  );
  assert.match(source, /\.route\("\/instructions\/analyze", post\(analyze_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/instructions\/analyze", post\(analyze_http\)\)/);
  assert.match(
    source,
    /\.route\("\/instructions\/validate", post\(instruction_validate_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/validate",\s*post\(instruction_validate_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/instructions\/improve", post\(instruction_improve_http\)\)/);
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/instructions\/improve",\s*post\(instruction_improve_http\),\s*\)/,
  );
  assert.match(source, /\.route\("\/learning\/policy", get\(learning_policy_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/learning\/policy", get\(learning_policy_http\)\)/);
  assert.match(source, /\.route\("\/learning\/corpus", get\(learning_corpus_http\)\)/);
  assert.match(source, /\.route\("\/fabrication\/learning\/corpus", get\(learning_corpus_http\)\)/);
  assert.match(source, /\.route\("\/learning\/observe", post\(learning_observe_http\)\)/);
  assert.match(
    source,
    /\.route\("\/fabrication\/learning\/observe", post\(learning_observe_http\)\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/learning\/outcomes",\s*get\(learning_outcomes_http\)\.post\(learning_outcome_http\),\s*\)/,
  );
  assert.match(
    source,
    /\.route\(\s*"\/fabrication\/learning\/outcomes",\s*get\(learning_outcomes_http\)\.post\(learning_outcome_http\),\s*\)/,
  );

  assert.match(readme, /`GET \/jobs\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/jobs\/catalog`/);
  assert.match(readme, /dd\.fabrication\.job-evidence-catalog\.v1/);
  assert.match(readme, /learningPolicySnapshot/);
  assert.match(readme, /release-bundle surface list includes `bundleManifest`, `releaseGateMatrix`/);
  assert.match(readme, /`releaseGateSummary`, and the `summary\.releaseGate\*` count fields/);
  assert.match(readme, /discover gate triage fields before fetching a retained bundle/);
  assert.match(readme, /`GET \/jobs`/);
  assert.match(readme, /`GET \/fabrication\/jobs`/);
  assert.match(readme, /It also includes `releaseGateSummaries`/);
  assert.match(readme, /compact per-job triage list/);
  assert.match(readme, /prioritize blocked\s+jobs before fetching full bundles/);
  assert.match(readme, /`GET \/fabrication\/jobs\/:job_id`/);
  assert.match(readme, /artifact summaries, `releaseGateSummary`, and `releaseBundleRoute`/);
  assert.match(readme, /compact\s+single-job gate triage/);
  assert.match(readme, /`GET \/jobs\/:job_id\/release-bundle`/);
  assert.match(readme, /`GET \/fabrication\/jobs\/:job_id\/release-bundle`/);
  assert.match(readme, /dd\.fabrication\.job-release-bundle\.v1/);
  assert.match(readme, /`bundleManifest`/);
  assert.match(readme, /design\/source/);
  assert.match(readme, /machine-code\/instruction/);
  assert.match(readme, /simulation\/quality\/release/);
  assert.match(readme, /learning\/policy feedback/);
  assert.match(readme, /present\/missing counts/);
  assert.match(readme, /Its `releaseGateMatrix` maps/);
  assert.match(readme, /retained\s+manifest categories, present\/missing counts/);
  assert.match(readme, /blocked release surfaces, and\s+evidence routes/);
  assert.match(readme, /`releaseGateSummary` and the `summary`\s+release-gate counts/);
  assert.match(readme, /ready gate count, blocked gate count/);
  assert.match(readme, /blocked gate\s+IDs/);
  assert.match(readme, /releaseBundle\.releaseSurfaces/);
  assert.match(readme, /`GET \/fabrication\/jobs\/:job_id\/artifacts\/:artifact_id`/);
  assert.match(readme, /`GET \/capabilities`/);
  assert.match(readme, /`GET \/fabrication\/capabilities`/);
  assert.match(source, /"POST \/fabrication\/machines\/select"/);
  assert.match(readme, /`POST \/fabrication\/machines\/select`/);
  assert.match(readme, /`GET \/cells\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/cells\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/cnc\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/hybrid\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/methods\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/machine-code\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/engines\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/rewards\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/corpus`/);
  assert.match(readme, /bounded machine-selection/);
  assert.match(source, /"POST \/fabrication\/workflow\/plan"/);
  assert.match(readme, /workflow route\/evidence planning/);
  assert.match(source, /"GET \/fabrication\/costing\/catalog"/);
  assert.match(source, /"POST \/fabrication\/costing\/result"/);
  assert.match(source, /"GET \/fabrication\/utilities\/catalog"/);
  assert.match(source, /"POST \/fabrication\/utilities\/result"/);
  assert.match(source, /"GET \/fabrication\/telemetry\/catalog"/);
  assert.match(source, /"POST \/fabrication\/telemetry\/result"/);
  assert.match(source, /"GET \/fabrication\/consumables\/catalog"/);
  assert.match(source, /"POST \/fabrication\/consumables\/result"/);
  assert.match(source, /"POST \/fabrication\/process-capabilities\/result"/);
  assert.match(source, /"POST \/fabrication\/provenance\/result"/);
  assert.match(readme, /costing, utilities, energy,\s+availability, maintenance, telemetry, consumables/);
  assert.match(readme, /workholding, process-capability,\s+safety\/environment, provenance/);
  assert.match(readme, /DES engine, reward shaping,\s+and neural training-corpus\s+capabilities/);
  assert.match(readme, /`GET \/fabrication\/workers\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/results\/catalog`/);
  assert.match(readme, /result-review intake routes from the top-level capability/);
  assert.match(readme, /`strategyQualitySurfaces`/);
  assert.match(readme, /`policySummary\.learnedQuality`/);
  assert.match(readme, /`learningOutcomeQuality\.riskReviewRequired`/);
  assert.match(readme, /`GET \/objective\/coverage`/);
  assert.match(readme, /`GET \/fabrication\/objective\/coverage`/);
  assert.match(readme, /dd\.fabrication\.objective-coverage\.v1/);
  assert.match(readme, /`objectiveCoverageMatrix`/);
  assert.match(readme, /3D-printing and hybrid intake/);
  assert.match(readme, /machine-code and instruction\s+generation/);
  assert.match(readme, /existing-instruction validation and improvement/);
  assert.match(readme, /machine-failure and\s+human-intervention boundaries/);
  assert.match(readme, /split\/combine multi-process learning/);
  assert.match(readme, /MDP\/POMDP\/DES\/neural learning/);
  assert.match(readme, /`learning-policy-snapshot`/);
  assert.match(readme, /`learning-outcome-memory`/);
  assert.match(readme, /`learning-corpus`/);
  assert.match(readme, /`GET \/controllers\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/controllers\/catalog`/);
  assert.match(source, /"POST \/fabrication\/controllers\/result"/);
  assert.match(readme, /dd\.fabrication\.controller-postprocessor-catalog\.v1/);
  assert.match(readme, /postprocessor-known counts/);
  assert.match(readme, /`controllerPlan\.compatibilityTargets`/);
  assert.match(readme, /`dialectAssumptionChecklist`/);
  assert.match(readme, /modal defaults and\s+reset state/);
  assert.match(readme, /offset tables and compensation/);
  assert.match(readme, /macro\/subprogram dependencies/);
  assert.match(readme, /`controller-modal-defaults:\*`/);
  assert.match(readme, /`GET \/controllers\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/controllers\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.controller-preflight-catalog\.v1/);
  assert.match(readme, /modal-state, offset\/setup-state, and program-dependency evidence/);
  assert.match(readme, /postprocessor-version, tool map, and\s+dry-run\/simulation evidence/);
  assert.match(readme, /`POST \/controllers\/result`/);
  assert.match(readme, /`POST \/fabrication\/controllers\/result`/);
  assert.match(readme, /controller\/postprocessor\s+review, draft machine-code\s+generation/);
  assert.match(readme, /dd\.fabrication\.controller-postprocessor-result-review\.v1/);
  assert.match(readme, /controller-postprocessor-targets/);
  assert.match(readme, /controller-postprocessor-learning-observations/);
  assert.match(readme, /dd\.fabrication\.controller-postprocessor-learning-outcome-draft\.v1/);
  assert.match(readme, /target,\s+program,\s+controller,\s+postprocessor,\s+target-status,\s+check,\s+artifact/);
  assert.match(readme, /`GET \/design\/formats`/);
  assert.match(readme, /`GET \/fabrication\/design\/formats`/);
  assert.match(readme, /`GET \/printers\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/printers\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.printer-preflight-catalog\.v1/);
  assert.match(readme, /thermal\/motion state, extrusion\/material\/resume state/);
  assert.match(readme, /bed mesh\/Z-offset evidence, extrusion reset, purge\s+or prime evidence/);
  assert.match(readme, /`GET \/subtractive\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/subtractive\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.subtractive-preflight-catalog\.v1/);
  assert.match(readme, /stock\/workholding\/datum state/);
  assert.match(readme, /tool\/process\/media state/);
  assert.match(readme, /controller\/geometry\/simulation state/);
  assert.match(readme, /fixtures, vises, chucks, collets/);
  assert.match(readme, /`GET \/turning\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/turning\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.turning-preflight-catalog\.v1/);
  assert.match(readme, /chuck\/collet\/bar-stock\/support state/);
  assert.match(readme, /turning tooling\/offset\/threading state/);
  assert.match(readme, /mill-turn live-tool\/spindle-transfer state/);
  assert.match(readme, /`GET \/cleanliness\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/cleanliness\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.cleanliness-preflight-catalog\.v1/);
  assert.match(readme, /residue,\s+FOD,\s+drying,\s+and interface cleanliness/);
  assert.match(readme, /resin drip\/wash\/cure evidence/);
  assert.match(readme, /coolant\/oil\/abrasive\/dielectric\/chip removal/);
  assert.match(readme, /`GET \/interfaces\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/interfaces\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.interface-preflight-catalog\.v1/);
  assert.match(readme, /datum and locating-interface state/);
  assert.match(readme, /fit\/tolerance\/stackup state/);
  assert.match(readme, /joining hardware\/service-interface state/);
  assert.match(readme, /split differently, add datums/);
  assert.match(readme, /`GET \/slicers\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/slicers\/catalog`/);
  assert.match(readme, /`POST \/slicers\/result`/);
  assert.match(readme, /`POST \/fabrication\/slicers\/result`/);
  assert.match(source, /"POST \/fabrication\/mesh-repair\/result"/);
  assert.match(readme, /mesh\/topology repair review/);
  assert.match(readme, /`GET \/formats\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/formats\/catalog`/);
  assert.match(readme, /`GET \/design\/import\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/design\/import\/catalog`/);
  assert.match(readme, /`POST \/design\/import\/review`/);
  assert.match(readme, /`POST \/fabrication\/design\/import\/review`/);
  assert.match(readme, /`POST \/design\/import\/result`/);
  assert.match(readme, /`POST \/fabrication\/design\/import\/result`/);
  assert.match(readme, /`POST \/design\/convert\/plan`/);
  assert.match(readme, /`POST \/fabrication\/design\/convert\/plan`/);
  assert.match(readme, /`POST \/design\/convert\/result`/);
  assert.match(readme, /`POST \/fabrication\/design\/convert\/result`/);
  assert.match(readme, /`GET \/design\/generation\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/design\/generation\/catalog`/);
  assert.match(readme, /`POST \/design\/generate`/);
  assert.match(readme, /`POST \/fabrication\/design\/generate`/);
  assert.match(readme, /`POST \/design\/synthesis\/result`/);
  assert.match(readme, /`POST \/fabrication\/design\/synthesis\/result`/);
  assert.match(readme, /dd\.fabrication\.design-import-catalog\.v1/);
  assert.match(readme, /`translatorReadinessChecklist`/);
  assert.match(readme, /native CAD\s+translator provenance/);
  assert.match(readme, /neutral-kernel\/PMI preservation/);
  assert.match(readme, /sheet-profile\/CAM handoff/);
  assert.match(readme, /`cad-translator:\*`/);
  assert.match(readme, /`GET \/design\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/design\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.design-preflight-catalog\.v1/);
  assert.match(readme, /source identity and provenance state/);
  assert.match(readme, /geometry\/units\/feature\s+state/);
  assert.match(readme, /conversion\/simulation\/learning state/);
  assert.match(readme, /ambiguous\s+`\.prt`\/`\.asm` disambiguation/);
  assert.match(readme, /dd\.fabrication\.slicer-profile-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.slicer-profile-result-review\.v1/);
  assert.match(readme, /slicer-print-preparation/);
  assert.match(readme, /slicer-machine-code-checks/);
  assert.match(readme, /dd\.fabrication\.slicer-profile-learning-outcome-draft\.v1/);
  assert.match(readme, /slicer,\s+printer-family,\s+material,\s+profile-check,\s+preparation,\s+machine-code-check/);
  assert.match(readme, /artifact,\s+human-intervention,\s+blocker,\s+reward,\s+and submit-route hints/);
  assert.match(readme, /PrusaSlicer, OrcaSlicer, Cura, Bambu Studio, Lychee Slicer, and Chitubox/);
  assert.match(readme, /exposure, lift\/retract, support/);
  assert.match(readme, /wash\/cure, resin lot, and PPE evidence/);
  assert.match(readme, /not certified\s+printer-ready G-code/);
  assert.match(readme, /dd\.fabrication\.mesh-repair-catalog\.v1/);
  assert.match(readme, /watertight topology repair/);
  assert.match(readme, /not certified printable geometry/);
  assert.match(readme, /`POST \/mesh-repair\/result`/);
  assert.match(readme, /`POST \/fabrication\/mesh-repair\/result`/);
  assert.match(readme, /dd\.fabrication\.mesh-repair-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.mesh-repair-learning-outcome-draft\.v1/);
  assert.match(readme, /mesh-repair-dimensional-reviews/);
  assert.match(readme, /mesh-repair-learning-observations/);
  assert.match(readme, /topology,\s+dimensional-drift, orientation\/support/);
  assert.match(readme, /dd\.fabrication\.design-import-review\.v1/);
  assert.match(readme, /dd\.fabrication\.design-import-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.design-import-learning-outcome-draft\.v1/);
  assert.match(readme, /design-import-failure-boundaries/);
  assert.match(readme, /design-import-priority-dispositions/);
  assert.match(readme, /design-import-learning-observations/);
  assert.match(readme, /`priorityDispositions` rows for source\s+context, import-check closure/);
  assert.match(readme, /source-format, check, boundary, recommended-action, artifact,\s+priority/);
  assert.match(readme, /dd\.fabrication\.design-conversion-plan\.v1/);
  assert.match(readme, /dd\.fabrication\.design-conversion-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.design-conversion-learning-outcome-draft\.v1/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /conversion success,\s+neutral export evidence,\s+blocker closure,\s+source context/);
  assert.match(readme, /input,\s+source-format,\s+source-system,\s+worker-lane,\s+status,\s+converted,\s+priority/);
  assert.match(readme, /neutral-export,\s+blocker,\s+evidence,\s+reward,\s+and submit-route hints/);
  assert.match(readme, /dd\.fabrication\.design-synthesis-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.design-synthesis-learning-outcome-draft\.v1/);
  assert.match(readme, /`manufacturabilityEvidenceMissing`/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /accepted-candidate,\s+candidate,\s+part,\s+export-format,\s+manufacturing-method/);
  assert.match(readme, /priority-disposition,\s+reward, and submit-route/);
  assert.match(readme, /split\/combine and hybrid manufacturing candidates/);
  assert.match(readme, /`design-synthesis-priority:<priority>:<disposition>`/);
  assert.match(readme, /`workerDispatch`/);
  assert.match(readme, /`releaseUpdate`/);
  assert.match(readme, /translator and import worker-lane/);
  assert.match(readme, /Creo\/Pro\/ENGINEER, SOLIDWORKS, Fusion/);
  assert.match(readme, /professional-cad-converter/);
  assert.match(readme, /lightweight-cad-pmi-inspector/);
  assert.match(readme, /ambiguous `\.prt`\/`\.asm` policies/);
  assert.match(readme, /`translatorReadinessChecklist`/);
  assert.match(readme, /native CAD\s+translator provenance/);
  assert.match(readme, /neutral-kernel\/PMI preservation/);
  assert.match(readme, /mesh or slicer profile\s+readiness/);
  assert.match(readme, /sheet-profile\/CAM handoff/);
  assert.match(readme, /`cad-translator:\*`/);
  assert.match(readme, /`designInputReview\.conversionPlan`/);
  assert.match(readme, /same bounded\s+`designInputs` validation used by `\/fabrication\/plan`/);
  assert.match(readme, /`machineReady` remains false until translator\/export\s+results/);
  assert.match(readme, /dd\.fabrication\.design-generation-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.design-generation\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.design\.synthesis\.results/);
  assert.match(readme, /`designSynthesisResult`/);
  assert.match(readme, /`GET \/subjects\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/subjects\/catalog`/);
  assert.match(readme, /dd\.fabrication\.subject-catalog\.v1/);
  assert.match(readme, /NATS worker-dispatch contract/);
  assert.match(readme, /design conversion,\s+design synthesis,\s+instruction generation/);
  assert.match(readme, /not guaranteed worker\s+availability/);
  assert.match(readme, /`GET \/fabrication\/tooling\/catalog`/);
  assert.match(readme, /dd\.fabrication\.tooling-catalog\.v1/);
  assert.match(readme, /subtractive cutters\/holders\/probes/);
  assert.match(readme, /not certified tooling setup sheets/);
  assert.match(readme, /`POST \/tooling\/result`/);
  assert.match(readme, /`POST \/fabrication\/tooling\/result`/);
  assert.match(readme, /dd\.fabrication\.tooling-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.tooling-learning-outcome-draft\.v1/);
  assert.match(readme, /tooling-tool-life-checks/);
  assert.match(readme, /tooling-support-media-checks/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /`tooling-priority:<priority>:<disposition>`/);
  assert.match(readme, /`tooling-priority-dispositions`/);
  assert.match(readme, /tooling-learning-observations/);
  assert.match(readme, /tool,\s+offset, tool-life, support-media, artifact/);
  assert.match(readme, /change tools, split\s+setups, refresh offsets/);
  assert.match(readme, /`GET \/fabrication\/consumables\/catalog`/);
  assert.match(readme, /dd\.fabrication\.consumables-catalog\.v1/);
  assert.match(readme, /subtractive cutters\/inserts\/coolant/);
  assert.match(readme, /tool-life risk, material capacity,\s+support-media depletion/);
  assert.match(readme, /`POST \/consumables\/result`/);
  assert.match(readme, /`POST \/fabrication\/consumables\/result`/);
  assert.match(readme, /dd\.fabrication\.consumables-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.consumables-learning-outcome-draft\.v1/);
  assert.match(readme, /consumables-tool-life-checks/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /`consumables-priority:<priority>:<disposition>`/);
  assert.match(readme, /consumables-priority-dispositions/);
  assert.match(readme, /consumables-learning-observations/);
  assert.match(readme, /inventory, tool-life, support-media, artifact/);
  assert.match(readme, /operator refill checkpoints/);
  assert.match(readme, /`GET \/fabrication\/workholding\/catalog`/);
  assert.match(readme, /dd\.fabrication\.workholding-catalog\.v1/);
  assert.match(readme, /lathe chucks, collets, guide bushings/);
  assert.match(readme, /not\s+certified fixture designs/);
  assert.match(readme, /`GET \/workholding\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/workholding\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.workholding-preflight-catalog\.v1/);
  assert.match(readme, /stock\/build-surface primary hold state/);
  assert.match(readme, /datum-transfer\/re-probe\/clearance state/);
  assert.match(readme, /split-combine fixture plus\s+human-intervention state/);
  assert.match(readme, /`POST \/workholding\/result`/);
  assert.match(readme, /`POST \/fabrication\/workholding\/result`/);
  assert.match(readme, /dd\.fabrication\.workholding-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.workholding-learning-outcome-draft\.v1/);
  assert.match(readme, /workholding-datum-transfers/);
  assert.match(readme, /workholding-split-combine-holds/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /`workholding-priority:<priority>:<disposition>`/);
  assert.match(readme, /workholding-priority-dispositions/);
  assert.match(readme, /workholding-learning-observations/);
  assert.match(readme, /fixture, datum-transfer, clearance, split\/combine/);
  assert.match(readme, /`GET \/nesting\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/nesting\/catalog`/);
  assert.match(readme, /dd\.fabrication\.nesting-catalog\.v1/);
  assert.match(readme, /build-plate, powder-bed, sheet-cut,\s+flat-blank, and hybrid kit layout/);
  assert.match(readme, /`designExports\.partExports\.content\.nesting`/);
  assert.match(readme, /not certified\s+CAM or slicer nests/);
  assert.match(readme, /adjust orientation, split jobs,\s+change batch layout/);
  assert.match(readme, /`POST \/nesting\/result`/);
  assert.match(readme, /`POST \/fabrication\/nesting\/result`/);
  assert.match(readme, /dd\.fabrication\.nesting-result-review\.v1/);
  assert.match(readme, /layout, traceability, retention, and kit recomposition evidence/);
  assert.match(readme, /nesting-traceability-checks/);
  assert.match(readme, /nesting-learning-observations/);
  assert.match(readme, /`GET \/fabrication\/support-strategies\/catalog`/);
  assert.match(readme, /dd\.fabrication\.support-strategy-catalog\.v1/);
  assert.match(readme, /one-piece, split, combine, or alternate-machine routes/);
  assert.match(readme, /not certified manufacturing instructions/);
  assert.match(readme, /`POST \/support-strategies\/result`/);
  assert.match(readme, /`POST \/fabrication\/support-strategies\/result`/);
  assert.match(readme, /dd\.fabrication\.support-strategy-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.support-strategy-learning-outcome-draft\.v1/);
  assert.match(readme, /support-strategy-orientation-reviews/);
  assert.match(readme, /support-strategy-learning-observations/);
  assert.match(readme, /orientation, support, split\/combine, intervention/);
  assert.match(readme, /support-strategy:split-combine-required/);
  assert.match(readme, /`GET \/fabrication\/process-recipes\/catalog`/);
  assert.match(readme, /dd\.fabrication\.process-recipe-catalog\.v1/);
  assert.match(readme, /subtractive feeds\/speeds and cutter engagement/);
  assert.match(readme, /not certified production recipes/);
  assert.match(readme, /`POST \/process-recipes\/result`/);
  assert.match(readme, /`POST \/fabrication\/process-recipes\/result`/);
  assert.match(readme, /dd\.fabrication\.process-recipe-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.process-recipe-learning-outcome-draft\.v1/);
  assert.match(readme, /process-recipe-parameter-checks/);
  assert.match(readme, /process-recipe-learning-observations/);
  assert.match(readme, /recipe, parameter, coupon, artifact/);
  assert.match(readme, /process-recipe:parameter-change-required/);
  assert.match(readme, /`GET \/fabrication\/kinematics\/catalog`/);
  assert.match(readme, /dd\.fabrication\.kinematics-catalog\.v1/);
  assert.match(readme, /rotary\/five-axis milling/);
  assert.match(readme, /not certified kinematic\s+calibration records/);
  assert.match(readme, /`POST \/kinematics\/result`/);
  assert.match(readme, /`POST \/fabrication\/kinematics\/result`/);
  assert.match(readme, /dd\.fabrication\.kinematics-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.kinematics-learning-outcome-draft\.v1/);
  assert.match(readme, /kinematics-axis-checks/);
  assert.match(readme, /kinematics-learning-observations/);
  assert.match(readme, /axis,\s+coordinate-state, frame, artifact/);
  assert.match(readme, /kinematics:human-intervention-required/);
  assert.match(readme, /`GET \/fabrication\/tolerances\/catalog`/);
  assert.match(readme, /dd\.fabrication\.tolerance-catalog\.v1/);
  assert.match(readme, /hybrid assembly interface stackups/);
  assert.match(readme, /not certified inspection plans/);
  assert.match(readme, /`POST \/tolerances\/result`/);
  assert.match(readme, /`POST \/fabrication\/tolerances\/result`/);
  assert.match(readme, /dd\.fabrication\.tolerance-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.tolerance-learning-outcome-draft\.v1/);
  assert.match(readme, /tolerance-family, geometry-scope, fit, compensation/);
  assert.match(readme, /tolerance-compensations/);
  assert.match(readme, /tolerance-learning-observations/);
  assert.match(readme, /split\/combine planning, or human fit-up/);
  assert.match(readme, /`GET \/fabrication\/process-capabilities\/catalog`/);
  assert.match(readme, /dd\.fabrication\.process-capability-catalog\.v1/);
  assert.match(readme, /subtractive tool access and chip-load envelopes/);
  assert.match(readme, /not certified machine capability\s+studies/);
  assert.match(readme, /`POST \/process-capabilities\/result`/);
  assert.match(readme, /`POST \/fabrication\/process-capabilities\/result`/);
  assert.match(readme, /dd\.fabrication\.process-capability-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.process-capability-learning-outcome-draft\.v1/);
  assert.match(readme, /process-capability-alternate-routes/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /`process-capability-priority-dispositions`/);
  assert.match(readme, /`process-capability-priority:<priority>:<disposition>`/);
  assert.match(readme, /process-capability-learning-observations/);
  assert.match(readme, /capability-family, capability-scope, alternate-route/);
  assert.match(readme, /printer, mill, lathe, sheet-cut, or hybrid routes/);
  assert.match(readme, /`GET \/fabrication\/manufacturability\/catalog`/);
  assert.match(readme, /dd\.fabrication\.manufacturability-catalog\.v1/);
  assert.match(readme, /additive DFM print-or-split review/);
  assert.match(readme, /not certified design approvals/);
  assert.match(readme, /`POST \/manufacturability\/result`/);
  assert.match(readme, /`POST \/fabrication\/manufacturability\/result`/);
  assert.match(readme, /dd\.fabrication\.manufacturability-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.manufacturability-learning-outcome-draft\.v1/);
  assert.match(readme, /review-family, check-scope, route, split\/combine decision/);
  assert.match(readme, /manufacturability-split-combine-decisions/);
  assert.match(readme, /manufacturability:split-combine-required/);
  assert.match(readme, /`GET \/fabrication\/failure-modes\/catalog`/);
  assert.match(readme, /dd\.fabrication\.failure-mode-catalog\.v1/);
  assert.match(readme, /subtractive tool and fixture failures/);
  assert.match(readme, /not certified machine\s+diagnostics/);
  assert.match(readme, /`POST \/failure-modes\/result`/);
  assert.match(readme, /`POST \/fabrication\/failure-modes\/result`/);
  assert.match(readme, /dd\.fabrication\.failure-mode-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.failure-mode-learning-outcome-draft\.v1/);
  assert.match(readme, /failure-family, failure-mode, recovery-action/);
  assert.match(readme, /failure-mode-recovery-actions/);
  assert.match(readme, /failure-mode-priority-dispositions/);
  assert.match(readme, /failure-mode-learning-observations/);
  assert.match(readme, /failure-mode:split-combine-required/);
  assert.match(readme, /failure-mode-priority:<priority>:<disposition>/);
  assert.match(readme, /priority-disposition, split\/combine/);
  assert.match(readme, /`GET \/fabrication\/safety\/catalog`/);
  assert.match(readme, /dd\.fabrication\.safety-catalog\.v1/);
  assert.match(readme, /robotic-cell and\s+external-axis interlocks/);
  assert.match(readme, /not certified machine-safety approvals/);
  assert.match(readme, /`POST \/safety\/result`/);
  assert.match(readme, /`POST \/fabrication\/safety\/result`/);
  assert.match(readme, /dd\.fabrication\.safety-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.safety-learning-outcome-draft\.v1/);
  assert.match(readme, /safety-interlock-checks/);
  assert.match(readme, /safety-priority-dispositions/);
  assert.match(readme, /safety-learning-observations/);
  assert.match(readme, /safety\s+family, hazard, interlock, emergency-action/);
  assert.match(readme, /priority-disposition,\s+stop-point/);
  assert.match(readme, /safety-priority:<priority>:<disposition>/);
  assert.match(readme, /generated\/imported instructions need safe stops/);
  assert.match(readme, /`GET \/fabrication\/environment\/catalog`/);
  assert.match(readme, /dd\.fabrication\.environment-catalog\.v1/);
  assert.match(readme, /additive material\s+storage and printroom state/);
  assert.match(readme, /not certified facility\s+qualifications/);
  assert.match(readme, /`POST \/environment\/result`/);
  assert.match(readme, /`POST \/fabrication\/environment\/result`/);
  assert.match(readme, /dd\.fabrication\.environment-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.environment-learning-outcome-draft\.v1/);
  assert.match(readme, /environment-family, condition-scope, utility, metrology/);
  assert.match(readme, /environment-utility-checks/);
  assert.match(readme, /environment-priority-dispositions/);
  assert.match(readme, /environment-learning-observations/);
  assert.match(readme, /priority-disposition,\s+recovery/);
  assert.match(readme, /environment-priority:<priority>:<disposition>/);
  assert.match(readme, /ambient conditions made\s+generated\/imported instructions releasable/);
  assert.match(readme, /`GET \/fabrication\/provenance\/catalog`/);
  assert.match(readme, /dd\.fabrication\.provenance-catalog\.v1/);
  assert.match(readme, /machine-program and controller artifact lineage/);
  assert.match(readme, /not certified quality records/);
  assert.match(readme, /`POST \/provenance\/result`/);
  assert.match(readme, /`POST \/fabrication\/provenance\/result`/);
  assert.match(readme, /dd\.fabrication\.provenance-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.provenance-learning-outcome-draft\.v1/);
  assert.match(readme, /provenance-priority-dispositions/);
  assert.match(readme, /provenance-priority:<priority>:<disposition>/);
  assert.match(readme, /source, controller, release, and\s+custody evidence/);
  assert.match(readme, /`GET \/as-built\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/as-built\/catalog`/);
  assert.match(readme, /dd\.fabrication\.as-built-catalog\.v1/);
  assert.match(readme, /actual-geometry evidence catalog/);
  assert.match(readme, /`as-built-deviation-map`/);
  assert.match(readme, /split\/combine as-built interface evidence/);
  assert.match(readme, /not certified metrology\s+acceptance/);
  assert.match(readme, /`POST \/as-built\/result`/);
  assert.match(readme, /`POST \/fabrication\/as-built\/result`/);
  assert.match(readme, /dd\.fabrication\.as-built-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.as-built-learning-outcome-draft\.v1/);
  assert.match(readme, /deviation maps, interface checks/);
  assert.match(readme, /as-built-priority-dispositions/);
  assert.match(readme, /as-built-priority:<priority>:<disposition>/);
  assert.match(readme, /priority-disposition, remeasure, rework/);
  assert.match(readme, /`as-built-deviation-maps`/);
  assert.match(readme, /`as-built-learning-observations`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`POST \/provenance\/result`/);
  assert.match(readme, /`POST \/fabrication\/provenance\/result`/);
  assert.match(readme, /dd\.fabrication\.provenance-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.provenance-learning-outcome-draft\.v1/);
  assert.match(readme, /provenance-artifact-checks/);
  assert.match(readme, /provenance-learning-observations/);
  assert.match(readme, /provenance-family, evidence-scope, artifact-kind/);
  assert.match(readme, /release-package evidence made generated or imported\s+instructions releasable/);
  assert.match(readme, /`designPackage`/);
  assert.match(readme, /`designExports`/);
  assert.match(readme, /`manufacturingHandoff\.parts`/);
  assert.match(readme, /retain the normal plan artifacts/);
  assert.match(readme, /Generated design packages,\s+native\/neutral CAD, mesh, CAM, and slicer export payloads remain deterministic\s+drafts/);
  assert.match(readme, /Machine-ready release remains blocked\s+while generated exports/);
  assert.match(readme, /MDP\/POMDP\/neural workers/);
  assert.match(readme, /`GET \/handoff\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/handoff\/catalog`/);
  assert.match(readme, /dd\.fabrication\.handoff-catalog\.v1/);
  assert.match(readme, /downstream worker-lane catalog/);
  assert.match(readme, /source CAD\/model\/slicer\s+conversion/);
  assert.match(readme, /`releasePackagePlan\.packages`/);
  assert.match(readme, /machine-ready release remains blocked while conversion/);
  assert.match(readme, /`POST \/handoff\/result`/);
  assert.match(readme, /`POST \/fabrication\/handoff\/result`/);
  assert.match(readme, /dd\.fabrication\.handoff-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.handoff-learning-outcome-draft\.v1/);
  assert.match(readme, /handoff-datum-transfers/);
  assert.match(readme, /handoff-transport-holds/);
  assert.match(readme, /handoff-learning-observations/);
  assert.match(readme, /segment, datum-transfer, transport-hold, artifact/);
  assert.match(readme, /`GET \/workers\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/workers\/catalog`/);
  assert.match(readme, /dd\.fabrication\.worker-catalog\.v1/);
  assert.match(readme, /worker-facing view of the same dispatch\s+lanes/);
  assert.match(readme, /retained-evidence requirements/);
  assert.match(readme, /`GET \/results\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/results\/catalog`/);
  assert.match(readme, /dd\.fabrication\.result-review-catalog\.v1/);
  assert.match(readme, /worker\s+result-review intake routes/);
  assert.match(readme, /job evidence routes, and learning outcome routes/);
  assert.match(readme, /`GET \/landing`/);
  assert.match(readme, /`GET \/fabrication\/landing`/);
  assert.match(readme, /`GET \/how-it-works`/);
  assert.match(readme, /`GET \/fabrication\/how-it-works`/);
  assert.match(readme, /`GET \/` returns the machine-readable service inventory/);
  assert.match(readme, /`landingPage`\s+block for the human fabrication overview/);
  assert.match(readme, /`startHere` map to\s+`\/fabrication\/landing`/);
  assert.match(readme, /`\/fabrication\/how-it-works`, capabilities, schema/);
  assert.match(
    readme,
    /The landing page and JSON how-it-works\s+overview explain how the service turns fabrication goals/,
  );
  assert.match(readme, /split\/combine plans/);
  assert.match(readme, /release gates, and learned\s+outcomes/);
  assert.match(readme, /serve a human\s+landing page for operators and integration authors/);
  assert.match(readme, /fabrication server's intake-to-release flow/);
  assert.match(readme, /CAD\/model\/slicer and CAM\s+intermediate intake/);
  assert.match(readme, /MDP\/POMDP\/DES\/neural learning/);
  assert.match(readme, /controller\/postprocessor review, setup, quality/);
  assert.match(readme, /native CAD,\s+cloud CAD, mesh, neutral exchange, and slicer ecosystems/);
  assert.match(readme, /PTC Creo \/ Pro\/ENGINEER, SOLIDWORKS, Autodesk Fusion/);
  assert.match(readme, /Siemens NX, CATIA, Onshape, FreeCAD, OpenSCAD/);
  assert.match(readme, /PrusaSlicer, OrcaSlicer, Cura, and Bambu Studio/);
  assert.match(readme, /ambiguous `\.prt`\/`\.asm` extensions/);
  assert.match(readme, /operator-facing release-gate matrix/);
  assert.match(readme, /source provenance, machine envelope, process readiness, simulation evidence/);
  assert.match(readme, /human or automation handoff, and learning disposition/);
  assert.match(readme, /toolpaths, slicer plans, G-code, controller programs, and text job-sheet/);
  assert.match(readme, /`priorityDispositions`, the result-review lanes/);
  assert.match(readme, /pending-blocker-resolution, and ready-for-learning/);
  assert.match(readme, /`dd\.fabrication\.how-it-works\.v1` payload/);
  assert.match(readme, /six-step\s+intake-to-release flow for discovery, intake, generation, validation, release,\s+and learning/);
  assert.match(readme, /generated machine code, printer instructions, imported\s+CNC\/controller streams/);
  assert.match(readme, /vertical mills,\s+horizontal mills/);
  assert.match(readme, /hybrid split\/combine routes/);
  assert.match(readme, /`releaseGateMatrix`/);
  assert.match(readme, /source-provenance, machine-envelope, process-readiness, simulation-evidence/);
  assert.match(readme, /human-or-automation-handoff, and learning-disposition gates/);
  assert.match(readme, /evidence routes and release surfaces each gate can\s+block/);
  assert.match(readme, /`remote\/submodules\/discrete-event-system\.rs` \/ `des_engine`/);
  assert.match(readme, /MDP, POMDP, DES, neural-policy evidence/);
  assert.match(readme, /`priorityDispositionContract` section names the shared/);
  assert.match(readme, /`<family>:<priority>:<disposition>` learning-observation shape/);
  assert.match(readme, /`GET \/intake\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/intake\/catalog`/);
  assert.match(readme, /`dd\.fabrication\.intake-catalog\.v1` discovery contract/);
  assert.match(readme, /instruction analysis or generation/);
  assert.match(readme, /reviews clear the machine-ready gates/);
  assert.match(readme, /`GET \/templates\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/templates\/catalog`/);
  assert.match(readme, /`dd\.fabrication\.request-templates-catalog\.v1` starter-request catalog/);
  assert.match(readme, /FDM printed functional parts/);
  assert.match(readme, /native CAD\/3MF intake review for SOLIDWORKS, Creo\/ProE/);
  assert.match(readme, /`POST \/fabrication\/design\/import\/review`/);
  assert.match(readme, /design-to-machine-code generation/);
  assert.match(readme, /direct FDM slicer machine-code generation/);
  assert.match(readme, /direct CNC controller\/postprocessor machine-code generation/);
  assert.match(readme, /`POST \/fabrication\/machine-code\/generate`/);
  assert.match(readme, /direct FDM printer instruction generation/);
  assert.match(readme, /direct CNC setup\/controller instruction generation/);
  assert.match(readme, /`POST \/fabrication\/instructions\/generate`/);
  assert.match(readme, /imported CNC dry-run simulation/);
  assert.match(readme, /`POST \/fabrication\/simulation\/run`/);
  assert.match(readme, /imported CNC program review/);
  assert.match(readme, /direct imported CNC improvement\/patch review/);
  assert.match(readme, /`POST \/fabrication\/instructions\/improve`/);
  assert.match(readme, /imported slicer G-code review/);
  assert.match(readme, /imported resin\/SLA job review/);
  assert.match(readme, /imported powder-bed build review/);
  assert.match(readme, /vertical-mill fixture plates/);
  assert.match(readme, /horizontal-mill side-slot\/keyway work/);
  assert.match(readme, /lathe turned inserts/);
  assert.match(readme, /hybrid printed\/milled\/turned assemblies/);
  assert.match(readme, /direct hybrid decomposition planning/);
  assert.match(readme, /direct hybrid assembly planning/);
  assert.match(readme, /hybrid route costing result feedback/);
  assert.match(readme, /`POST \/fabrication\/costing\/result`/);
  assert.match(readme, /operator intervention result feedback/);
  assert.match(readme, /`POST \/fabrication\/interventions\/result`/);
  assert.match(readme, /runtime monitoring result feedback/);
  assert.match(readme, /`POST \/fabrication\/monitoring\/result`/);
  assert.match(readme, /quality metrology result feedback/);
  assert.match(readme, /`POST \/fabrication\/quality\/result`/);
  assert.match(readme, /release-readiness result feedback/);
  assert.match(readme, /`POST \/fabrication\/release\/result`/);
  assert.match(readme, /hybrid outcome learning feedback/);
  assert.match(readme, /boundary-failure learning feedback/);
  assert.match(readme, /not machine-ready instructions/);
  assert.match(readme, /instruction-generation starter bodies deserialize as\s+`FabricationPlanRequest` examples/);
  assert.match(readme, /part `description` and `toleranceMm`\s+hints/);
  assert.match(readme, /FDM slicer\/profile, nozzle and bed temperature, extrusion, purge or prime/);
  assert.match(readme, /CNC\s+controller\/postprocessor, tooling, workholding, and dry-run evidence visible/);
  assert.match(readme, /imported CNC dry-run simulation starter also deserializes as a\s+`FabricationPlanRequest`/);
  assert.match(readme, /machine envelope,\s+fixture\/work-offset review, simulation-risk findings/);
  assert.match(readme, /hybrid printed\/milled\/turned starter\s+includes explicit printed-body, milled-datum-pad, and turned-insert part routes/);
  assert.match(readme, /split\/combine and interface-control review starts from concrete child parts/);
  assert.match(readme, /Direct decomposition and assembly starter bodies reuse those concrete child\s+routes/);
  assert.match(readme, /`decompositionPlan\.routeContracts`/);
  assert.match(readme, /`assemblyPlan\.splitCombineDecisions`/);
  assert.match(readme, /hybrid route costing result starter deserializes as a\s+`CostingResultReviewRequest`/);
  assert.match(readme, /machine-time\/setup estimates,\s+material yield and scrap allowances/);
  assert.match(readme, /split\/combine route\s+economics, human-intervention cost review/);
  assert.match(readme, /`costingLearningOutcomeDraft` feedback/);
  assert.match(readme, /operator intervention result starter deserializes as an\s+`InterventionResultReviewRequest`/);
  assert.match(readme, /blocked operator actions,\s+automation fallback, split\/combine interface review/);
  assert.match(readme, /`interventionLearningOutcomeDraft`\s+feedback/);
  assert.match(readme, /runtime monitoring result starter deserializes as a\s+`MonitoringResultReviewRequest`/);
  assert.match(readme, /channel heartbeat blockers,\s+critical alerts, safe-stop\/restart recovery actions/);
  assert.match(readme, /`monitoringLearningOutcomeDraft`\s+feedback/);
  assert.match(readme, /quality metrology result starter deserializes as a\s+`QualityResultReviewRequest`/);
  assert.match(readme, /out-of-tolerance measurements,\s+nonconformance findings, blocked inspection gates/);
  assert.match(readme, /human disposition or rework\/split decisions/);
  assert.match(readme, /`qualityLearningOutcomeDraft`\s+feedback/);
  assert.match(readme, /release-readiness result starter deserializes as a\s+`ReleaseReadinessResultReviewRequest`/);
  assert.match(readme, /blocked release\s+decisions, retained manifest artifact evidence/);
  assert.match(readme, /split\/combine release conditions/);
  assert.match(readme, /`releaseReadinessLearningOutcomeDraft`\s+feedback/);
  assert.match(readme, /Design import starter bodies deserialize as\s+`DesignImportReviewRequest` examples/);
  assert.match(readme, /translator evidence, units,\s+topology, PMI, and neutral export review/);
  assert.match(readme, /Imported instruction starter bodies\s+deserialize as `InstructionAnalysisRequest` examples/);
  assert.match(readme, /Fanuc-style CNC G-code/);
  assert.match(readme, /non-G-code fabrication instructions/);
  assert.match(readme, /direct\s+instruction-improvement starter uses the same request contract/);
  assert.match(readme, /`improvedPrograms\.patchManifest`/);
  assert.match(readme, /conservative patch review, simulation, and\s+human approval gates/);
  assert.match(readme, /Learning feedback starter bodies deserialize\s+as `LearningOutcomeRequest` examples/);
  assert.match(readme, /`rewardHint`, manufacturing methods/);
  assert.match(readme, /split\/combine, machine-failure, and human-intervention observations/);
  assert.match(readme, /`templateId` and `templateVersion` trace labels/);
  assert.match(readme, /job, artifact, release, learning outcome memory, boundary memory, remediation-risk, and neural training evidence/);
  assert.match(readme, /`releaseGateHints`/);
  assert.match(readme, /instruction\s+validation boundaries/);
  assert.match(readme, /design export review gates/);
  assert.match(readme, /direct machine-code generation/);
  assert.match(readme, /slicer-profile and postprocessor handoff evidence/);
  assert.match(readme, /instruction generation/);
  assert.match(readme, /improved-program review/);
  assert.match(readme, /resin postprocess evidence/);
  assert.match(readme, /powder\s+handling evidence/);
  assert.match(readme, /tooling\/workholding evidence/);
  assert.match(readme, /decomposition\/interface-control gates/);
  assert.match(readme, /machine-failure and human-intervention blockers/);
  assert.match(readme, /split\/combine boundary observations/);
  assert.match(readme, /MDP\/POMDP feedback/);
  assert.match(readme, /neural-training examples/);
  assert.match(readme, /release-package gates/);
  assert.match(readme, /`\/fabrication\/release\/catalog` contract/);
  assert.match(readme, /The schema also includes an `intakeGuide`/);
  assert.match(readme, /template-driven FDM request/);
  assert.match(readme, /`templateId`\/`templateVersion` trace labels/);
  assert.match(readme, /release-gate hints/);
  assert.match(readme, /review CAD\/model\/slicer inputs/);
  assert.match(readme, /plan hybrid\s+split\/combine builds/);
  assert.match(readme, /release and learn from retained outcome evidence/);
  assert.match(readme, /`GET \/instructions\/languages`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/languages`/);
  assert.match(readme, /`dd\.fabrication\.instruction-language-catalog\.v1` intake catalog/);
  assert.match(readme, /imported CNC,\s+CAM intermediate, printer, slicer, cutting, EDM/);
  assert.match(readme, /`siemens-sinumerik`/);
  assert.match(readme, /`heidenhain-conversational`/);
  assert.match(readme, /`mazatrol`/);
  assert.match(readme, /`okuma-osp`/);
  assert.match(readme, /`linuxcnc`/);
  assert.match(readme, /controller-specific modal-state,\s+postprocessor, dry-run/);
  assert.match(readme, /`siemens-sinumerik-postprocessor`/);
  assert.match(readme, /`heidenhain-conversational-postprocessor`/);
  assert.match(readme, /`mazatrol-conversational-postprocessor`/);
  assert.match(readme, /`okuma-osp-postprocessor`/);
  assert.match(readme, /`linuxcnc-gcode-postprocessor`/);
  assert.match(readme, /`apt-cldata`/);
  assert.match(readme, /`cldata-toolpath`/);
  assert.match(readme, /`postprocessor-deck`/);
  assert.match(readme, /APT\/CLDATA, cutter-location, and postprocessor deck handoffs/);
  assert.match(readme, /tool-axis\/contact-point, controller target/);
  assert.match(readme, /analysis route aliases/);
  assert.match(readme, /Machine-ready release remains blocked/);
  assert.match(readme, /`GET \/instructions\/review-pipeline\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/review-pipeline\/catalog`/);
  assert.match(readme, /dd\.fabrication\.instruction-review-pipeline-catalog\.v1/);
  assert.match(readme, /stage order for generated\s+or imported instruction streams/);
  assert.match(readme, /discover language\s+and machine context/);
  assert.match(readme, /retain the original import or generated artifact/);
  assert.match(readme, /validate\s+and find machine-failure\/human-intervention\/split-combine boundaries/);
  assert.match(readme, /`instructionImportReview\.originalPrograms`/);
  assert.match(readme, /`improvedPrograms\.patchManifest`/);
  assert.match(readme, /original stream immutable review evidence/);
  assert.match(readme, /`GET \/instructions\/validation\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/validation\/catalog`/);
  assert.match(readme, /`GET \/instructions\/validation\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/validation\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.instruction-validation-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-validation-preflight-catalog\.v1/);
  assert.match(readme, /source provenance\/language\/dialect state/);
  assert.match(readme, /machine\/process\/simulation\s+setup state/);
  assert.match(readme, /boundary\/improvement\/release\/learning state/);
  assert.match(readme, /`streamReadinessMatrix`/);
  assert.match(readme, /imported CNC\/controller\s+programs/);
  assert.match(readme, /additive slicer or printer G-code/);
  assert.match(readme, /hybrid split\/combine instruction\s+packages/);
  assert.match(readme, /`interventionMap\.splitCombineDecisions`/);
  assert.match(readme, /controller modal state/);
  assert.match(readme, /additive printer\s+heat\/extrusion\/material state/);
  assert.match(readme, /split\/combine release review/);
  assert.match(readme, /`machineReady=false`/);
  assert.match(readme, /`GET \/instructions\/generation\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/generation\/catalog`/);
  assert.match(readme, /dd\.fabrication\.instruction-generation-catalog\.v1/);
  assert.match(readme, /generated machine-program and\s+job-sheet catalog/);
  assert.match(readme, /Swiss\/sliding-headstock turning/);
  assert.match(readme, /robotic\/gantry additive/);
  assert.match(readme, /plastic joining\/ultrasonic welding\/heat staking\/solvent\/hot-plate\/vibration\/spin welding/);
  assert.match(readme, /`robotic-additive-job`/);
  assert.match(readme, /`robotic-pellet-job`/);
  assert.match(readme, /`robotic-extrusion-job`/);
  assert.match(readme, /`plastic-joining-job`/);
  assert.match(readme, /`ultrasonic-welding-job`/);
  assert.match(readme, /`heat-staking-job`/);
  assert.match(readme, /`swiss-turning-gcode`/);
  assert.match(readme, /`swiss-turning-job`/);
  assert.match(readme, /`generatedPrograms\.instructions`/);
  assert.match(readme, /`machineReady=false`/);
  assert.match(readme, /Program generation observations feed\s+MDP\/POMDP\/neural workers/);
  assert.match(readme, /`GET \/instructions\/generation\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/generation\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.instruction-generation-preflight-catalog\.v1/);
  assert.match(readme, /request\/design\/machine\s+state/);
  assert.match(readme, /program draft\/controller state/);
  assert.match(readme, /validation\/simulation\/release\/learning\s+state/);
  assert.match(readme, /`GET \/instructions\/import\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/import\/catalog`/);
  assert.match(readme, /dd\.fabrication\.instruction-import-catalog\.v1/);
  assert.match(readme, /controller\/CAM machine\s+code/);
  assert.match(readme, /Imported instructions are accepted as review inputs/);
  assert.match(readme, /`POST \/fabrication\/instructions\/import\/review`/);
  assert.match(readme, /dd\.fabrication\.instruction-import-review\.v1/);
  assert.match(readme, /validation and boundary analyzer/);
  assert.match(readme, /validation, simulation, machine-failure, human-intervention/);
  assert.match(readme, /`packageActions`/);
  assert.match(readme, /instruction-import-learning-outcome-draft\.v1/);
  assert.match(readme, /`POST \/instructions\/generate`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/generate`/);
  assert.match(readme, /dd\.fabrication\.instruction-generation\.v1/);
  assert.match(readme, /retain the normal plan artifacts/);
  assert.match(readme, /`manufacturingHandoff`/);
  assert.match(readme, /Generated instruction packages keep\s+`draft=true`/);
  assert.match(readme, /`POST \/instructions\/generation\/result`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/generation\/result`/);
  assert.match(readme, /dd\.fabrication\.instruction-generation-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-generation-learning-outcome-draft\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.instructions\.generation\.results/);
  assert.match(readme, /`instructionGenerationResult`/);
  assert.match(readme, /`generationResultJobId`/);
  assert.match(readme, /`instruction-generation-result`/);
  assert.match(readme, /`instruction-generation-artifacts`/);
  assert.match(readme, /`instruction-generation-release-update`/);
  assert.match(readme, /`instruction-generation-priority:<priority>:<disposition>`/);
  assert.match(readme, /`instruction-generation-priority-dispositions`/);
  assert.match(readme, /`instruction-generation-learning-observations`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`POST \/instructions\/review\/result`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/review\/result`/);
  assert.match(readme, /dd\.fabrication\.instruction-review-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-review-learning-outcome-draft\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.instructions\.review\.results/);
  assert.match(readme, /`instructionReviewResult`/);
  assert.match(readme, /`priorityDispositions` rows/);
  assert.match(readme, /`instructionIntentMap\.reviewPriorities`/);
  assert.match(readme, /`reviewResultJobId`/);
  assert.match(readme, /`instruction-review-result`/);
  assert.match(readme, /`instruction-review-findings`/);
  assert.match(readme, /`instruction-review-failure-boundaries`/);
  assert.match(readme, /`instruction-review-improvement-drafts`/);
  assert.match(readme, /`instruction-review-priority-dispositions`/);
  assert.match(readme, /`instruction-review-release-update`/);
  assert.match(readme, /`instruction-review-learning-observations`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`humanInterventionBoundaryCount`/);
  assert.match(readme, /`humanApprovalDraftCount`/);
  assert.match(readme, /`instruction-review-boundary-kind:\*`/);
  assert.match(readme, /`instruction-review-improvement:\*`/);
  assert.match(readme, /`instruction-review-priority:<priority>:<disposition>`/);
  assert.match(readme, /`GET \/machine-code\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/machine-code\/catalog`/);
  assert.match(readme, /dd\.fabrication\.machine-code-catalog\.v1/);
  assert.match(readme, /`programContracts`/);
  assert.match(readme, /`controllerTargets`/);
  assert.match(readme, /printer firmware G-code/);
  assert.match(readme, /CTB\/Photon\/Lychee\/Chitubox resin\s+package jobs/);
  assert.match(readme, /Machine-ready=false|machineReady=false/);
  assert.match(readme, /Program-generation, controller-release, simulation-risk/);
  assert.match(readme, /`targetSelectionMatrix`/);
  assert.match(readme, /additive printer\s+firmware/);
  assert.match(readme, /subtractive mill\/router controllers including vertical and horizontal\s+mills/);
  assert.match(readme, /turning and mill-turn controllers including lathes/);
  assert.match(readme, /sheet-cutting\/EDM and\s+special-process outputs/);
  assert.match(readme, /hybrid assembly or human-reviewed travelers/);
  assert.match(readme, /part-off support, cut-chart\/support-media/);
  assert.match(readme, /`GET \/machine-code\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/machine-code\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.machine-code-preflight-catalog\.v1/);
  assert.match(readme, /program source\/design state/);
  assert.match(readme, /controller\/postprocessor\/dialect state/);
  assert.match(readme, /machine\/setup\/toolpath\/process state/);
  assert.match(readme, /validation\/simulation\/release\/learning state/);
  assert.match(readme, /`POST \/machine-code\/generate`/);
  assert.match(readme, /`POST \/fabrication\/machine-code\/generate`/);
  assert.match(readme, /dd\.fabrication\.machine-code-generation\.v1/);
  assert.match(readme, /`controllerPlan\.releaseGates`/);
  assert.match(readme, /Machine-code generation remains a\s+draft controller\/postprocessor release package/);
  assert.match(readme, /`POST \/machine-code\/result`/);
  assert.match(readme, /`POST \/fabrication\/machine-code\/result`/);
  assert.match(readme, /dd\.fabrication\.machine-code-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.machine-code-learning-outcome-draft\.v1/);
  assert.match(readme, /`machineCodeResult`/);
  assert.match(readme, /`priorityDispositions` rows/);
  assert.match(readme, /`machineCodeResultJobId`/);
  assert.match(readme, /`machine-code-controller-checks`/);
  assert.match(readme, /`machine-code-failure-boundaries`/);
  assert.match(readme, /`machine-code-priority-dispositions`/);
  assert.match(readme, /`machine-code-learning-observations`/);
  assert.match(readme, /`machine-code-check:\*`/);
  assert.match(readme, /`machine-code-priority:<priority>:<disposition>`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`POST \/materials\/result`/);
  assert.match(readme, /`POST \/fabrication\/materials\/result`/);
  assert.match(readme, /dd\.fabrication\.material-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.material-learning-outcome-draft\.v1/);
  assert.match(readme, /missing certificates/);
  assert.match(readme, /conditioning windows/);
  assert.match(readme, /lot,\s+conditioning, check, artifact, blocker/);
  assert.match(readme, /`material-lots`/);
  assert.match(readme, /`material-conditioning`/);
  assert.match(readme, /`material-learning-observations`/);
  assert.match(readme, /`GET \/toolpaths\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/toolpaths\/catalog`/);
  assert.match(readme, /dd\.fabrication\.toolpath-catalog\.v1/);
  assert.match(readme, /additive slicer\/extrusion paths/);
  assert.match(readme, /split\/combine recomposition paths/);
  assert.match(readme, /Toolpath catalog entries are evidence\s+contracts, not certified machine programs/);
  assert.match(readme, /`POST \/toolpaths\/plan`/);
  assert.match(readme, /`POST \/fabrication\/toolpaths\/plan`/);
  assert.match(readme, /dd\.fabrication\.toolpath-planning\.v1/);
  assert.match(readme, /`toolpathPlan\.simulationTrace`/);
  assert.match(readme, /draft CAM\/slicer\/controller handoffs/);
  assert.match(readme, /Toolpath risk and generated-program\s+observations feed MDP\/POMDP\/neural workers/);
  assert.match(readme, /`POST \/toolpaths\/result`/);
  assert.match(readme, /`POST \/fabrication\/toolpaths\/result`/);
  assert.match(readme, /dd\.fabrication\.toolpath-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.toolpath-learning-outcome-draft\.v1/);
  assert.match(readme, /blocker counts for collision evidence/);
  assert.match(readme, /required dry-runs that have not passed/);
  assert.match(readme, /`priorityDispositions` array/);
  assert.match(readme, /`toolpath-priority-dispositions`/);
  assert.match(readme, /`toolpath-priority:<priority>:<disposition>`/);
  assert.match(readme, /segment, part,\s+operation, simulation, check, artifact/);
  assert.match(readme, /`toolpath-simulations`/);
  assert.match(readme, /`toolpath-checks`/);
  assert.match(readme, /`toolpath-learning-observations`/);
  assert.match(readme, /dd\.fabrication\.toolpath-learning-outcome-draft\.v1/);
  assert.match(readme, /segment,\s+part,\s+operation,\s+simulation,\s+check,\s+artifact/);
  assert.match(readme, /collision,\s+envelope,\s+clearance,\s+dry-run,\s+human-intervention/);
  assert.match(readme, /`GET \/improvements\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/improvements\/catalog`/);
  assert.match(readme, /`GET \/improvements\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/improvements\/preflight\/catalog`/);
  assert.match(readme, /`POST \/instructions\/improve`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/improve`/);
  assert.match(readme, /`POST \/instructions\/boundaries\/review`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/boundaries\/review`/);
  assert.match(readme, /dd\.fabrication\.instruction-improvement-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-improvement-preflight-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-improvement-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-boundary-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-boundary-learning-outcome-draft\.v1/);
  assert.match(readme, /repair-draft catalog/);
  assert.match(readme, /patch-review envelope/);
  assert.match(readme, /fail without human\s+intervention, verified automation, regeneration, split\/combine work/);
  assert.match(readme, /boundary, resolution-action, human-intervention, split\/combine/);
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`improvedPrograms\.patchManifest\.operations`/);
  assert.match(readme, /`insert-before-first-risk-motion`/);
  assert.match(readme, /`machineReady=false`/);
  assert.match(readme, /source-program and finding state/);
  assert.match(readme, /patch-review and\s+simulation state/);
  assert.match(readme, /learning plus release feedback state/);
  assert.match(readme, /`patchReviewMatrix`/);
  assert.match(readme, /modal controller-state\s+repairs/);
  assert.match(readme, /additive printer-state repairs/);
  assert.match(readme, /split\/combine route repairs/);
  assert.match(readme, /`patch-review:split-combine`/);
  assert.match(readme, /Instruction-patch observations are emitted for MDP\/POMDP\/neural workers/);
  assert.match(readme, /`GET \/boundaries\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/boundaries\/catalog`/);
  assert.match(readme, /`dd\.fabrication\.boundary-catalog\.v1` analyzer boundary catalog/);
  assert.match(readme, /boundary families, family counts/);
  assert.match(readme, /`releaseProbePlan`,\s+`decompositionPlan`, and `releasePackagePlan`/);
  assert.match(readme, /MDP\/POMDP\/neural learning signals/);
  assert.match(readme, /`decisionMatrix`/);
  assert.match(readme, /Machine-failure boundaries route to instruction improvement/);
  assert.match(readme, /human-intervention and\s+automation gaps route to intervention/);
  assert.match(readme, /split\/combine boundaries route to decomposition/);
  assert.match(readme, /`interventionMap\.splitCombineDecisions`/);
  assert.match(readme, /`boundary-decision:learning-feedback`/);
  assert.match(readme, /`GET \/boundaries\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/boundaries\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.boundary-preflight-catalog\.v1/);
  assert.match(readme, /machine-failure boundary evidence state/);
  assert.match(readme, /human-intervention and automation\s+gap state/);
  assert.match(readme, /split-combine\/remediation boundary state/);
  assert.match(readme, /`POST \/boundaries\/result`/);
  assert.match(readme, /`POST \/fabrication\/boundaries\/result`/);
  assert.match(readme, /dd\.fabrication\.boundary-analysis-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.boundary-analysis-learning-outcome-draft\.v1/);
  assert.match(readme, /boundary-analysis result job/);
  assert.match(readme, /`priorityDispositions` covers machine-failure boundaries/);
  assert.match(readme, /`boundary-analysis-priority:<priority>:<disposition>`/);
  assert.match(readme, /split work earlier, combine parts\s+deliberately/);
  assert.match(readme, /`GET \/remediation\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/remediation\/catalog`/);
  assert.match(readme, /dd\.fabrication\.boundary-remediation-catalog\.v1/);
  assert.match(readme, /remediation-lane catalog/);
  assert.match(readme, /`resolutionPlan\.steps`/);
  assert.match(readme, /`machineReady=false` remains mandatory/);
  assert.match(readme, /`POST \/remediation\/plan`/);
  assert.match(readme, /`POST \/fabrication\/remediation\/plan`/);
  assert.match(readme, /dd\.fabrication\.boundary-remediation-planning\.v1/);
  assert.match(readme, /`remediationPlan\.actions`/);
  assert.match(readme, /worker-handoff contracts/);
  assert.match(readme, /`POST \/remediation\/result`/);
  assert.match(readme, /`POST \/fabrication\/remediation\/result`/);
  assert.match(readme, /dd\.fabrication\.boundary-remediation-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.boundary-remediation-learning-outcome-draft\.v1/);
  assert.match(readme, /`boundaryRemediationResult\.actions`/);
  assert.match(readme, /retained remediation artifacts/);
  assert.match(readme, /remediator, action, boundary, blocker, artifact/);
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`priorityDispositions` array for\s+remediation-action closure/);
  assert.match(readme, /`boundary-remediation-priority:<priority>:<disposition>`/);
  assert.match(readme, /`GET \/decomposition\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/decomposition\/catalog`/);
  assert.match(readme, /dd\.fabrication\.decomposition-catalog\.v1/);
  assert.match(readme, /split\/combine and interface-control\s+catalog/);
  assert.match(readme, /decomposition target families, family counts, target kinds/);
  assert.match(readme, /`interfaceControlPlan\.controls`/);
  assert.match(readme, /machine-ready\s+release remains blocked until child geometry/);
  assert.match(readme, /`POST \/decomposition\/plan`/);
  assert.match(readme, /`POST \/fabrication\/decomposition\/plan`/);
  assert.match(readme, /dd\.fabrication\.decomposition-planning\.v1/);
  assert.match(readme, /`decompositionPlan\.recompositionInterfaces`/);
  assert.match(readme, /`hybridMakePlan\.splitCombineDecisions`/);
  assert.match(readme, /single-piece, split-route, or\s+recomposed fabrication succeeds/);
  assert.match(readme, /`POST \/decomposition\/result`/);
  assert.match(readme, /`POST \/fabrication\/decomposition\/result`/);
  assert.match(readme, /dd\.fabrication\.decomposition-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.decomposition-learning-outcome-draft\.v1/);
  assert.match(readme, /`decompositionResult`/);
  assert.match(readme, /`decompositionResultJobId`/);
  assert.match(readme, /`releaseBlocked`/);
  assert.match(readme, /`priorityDispositions` array for split\/combine boundary-first review/);
  assert.match(readme, /`decomposition-priority:<priority>:<disposition>`/);
  assert.match(readme, /`decomposition-route-reviews`/);
  assert.match(readme, /`decomposition-split-combine-decisions`/);
  assert.match(readme, /`decomposition-priority-dispositions`/);
  assert.match(readme, /`decomposition-learning-observations`/);
  assert.match(readme, /target, route, interface, split\/combine, artifact, blocker/);
  assert.match(readme, /`decomposition-decision:\*`/);
  assert.match(readme, /`GET \/assembly\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/assembly\/catalog`/);
  assert.match(readme, /dd\.fabrication\.assembly-catalog\.v1/);
  assert.match(readme, /hybrid assembly, recomposition, and joining\s+catalog/);
  assert.match(readme, /`assembly\.assemblyGraph`/);
  assert.match(readme, /`hybridMakePlan\.joinOperations`/);
  assert.match(readme, /join recipe evidence/);
  assert.match(readme, /learn when to split, combine,\s+recompose, or keep a part single-piece/);
  assert.match(readme, /`GET \/assembly\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/assembly\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.assembly-preflight-catalog\.v1/);
  assert.match(readme, /child-route package and interface state/);
  assert.match(readme, /join-recipe fixture and process state/);
  assert.match(readme, /final-fit quality release plus learning state/);
  assert.match(readme, /`POST \/assembly\/plan`/);
  assert.match(readme, /`POST \/fabrication\/assembly\/plan`/);
  assert.match(readme, /dd\.fabrication\.assembly-planning\.v1/);
  assert.match(readme, /`assembly\.assemblyGraph\.sequence`/);
  assert.match(readme, /`qualityPlan\.inspectionPoints`/);
  assert.match(readme, /recomposition and join\s+strategies complete without hidden human intervention/);
  assert.match(readme, /`POST \/assembly\/result`/);
  assert.match(readme, /`POST \/fabrication\/assembly\/result`/);
  assert.match(readme, /dd\.fabrication\.assembly-planning-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.assembly-planning-learning-outcome-draft\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.assembly\.planning\.results/);
  assert.match(readme, /`assemblyPlanningResult`/);
  assert.match(readme, /`assemblyResultJobId`/);
  assert.match(readme, /join, split\/combine, interface-check, artifact/);
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`priorityDispositions` array for recomposition-boundary review/);
  assert.match(readme, /`assembly-priority:<priority>:<disposition>`/);
  assert.match(readme, /`joinBlockerCount`/);
  assert.match(readme, /`interfaceBlockerCount`/);
  assert.match(readme, /`assembly-join-operations`/);
  assert.match(readme, /`assembly-split-combine-decisions`/);
  assert.match(readme, /`assembly-interface-checks`/);
  assert.match(readme, /`assembly-priority-dispositions`/);
  assert.match(readme, /`assembly-learning-observations`/);
  assert.match(readme, /`assembly-join:\*`/);
  assert.match(readme, /`assembly-split-combine:\*`/);
  assert.match(readme, /`assembly-interface-check:\*`/);
  assert.match(readme, /`POST \/interfaces\/result`/);
  assert.match(readme, /`POST \/fabrication\/interfaces\/result`/);
  assert.match(readme, /dd\.fabrication\.interface-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.interface-learning-outcome-draft\.v1/);
  assert.match(readme, /`interfaceResult`/);
  assert.match(readme, /`interfaceResultJobId`/);
  assert.match(readme, /datum-transfer and fit checks, join evidence/);
  assert.match(readme, /`interface-priority:<priority>:<disposition>`/);
  assert.match(readme, /`interface-join-evidence`/);
  assert.match(readme, /`interface-split-combine-decisions`/);
  assert.match(readme, /`interface-priority-dispositions`/);
  assert.match(readme, /`interface-result`/);
  assert.match(readme, /`interface-join:\*`/);
  assert.match(readme, /`interface-split-combine:\*`/);
  assert.match(readme, /`GET \/instructions\/import\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/instructions\/import\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.instruction-import-preflight-catalog\.v1/);
  assert.match(readme, /source provenance\/language\/artifact state/);
  assert.match(readme, /machine\/controller\/setup\/process state/);
  assert.match(readme, /analysis\/validation\/simulation\/improvement\/learning state/);
  assert.match(readme, /`machineReady` remains false/);
  assert.match(readme, /`GET \/release\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/release\/catalog`/);
  assert.match(readme, /dd\.fabrication\.release-catalog\.v1/);
  assert.match(readme, /machine-ready release catalog/);
  assert.match(readme, /release package kinds, package\s+states, gate types, blocker sources/);
  assert.match(readme, /`machineRelease\.blockers`/);
  assert.match(readme, /`releasePackagePlan\.releaseGates`/);
  assert.match(readme, /`GET \/release\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/release\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.release-preflight-catalog\.v1/);
  assert.match(readme, /manifest\/artifact\/checksum state/);
  assert.match(readme, /machine\/controller\/simulation\/process state/);
  assert.match(readme, /quality\/disposition\/signoff\/learning state/);
  assert.match(readme, /`POST \/release\/result`/);
  assert.match(readme, /`POST \/fabrication\/release\/result`/);
  assert.match(readme, /dd\.fabrication\.release-readiness-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.release-readiness-learning-outcome-draft\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.release\.readiness\.results/);
  assert.match(readme, /`releaseReadinessResult`/);
  assert.match(readme, /`releaseResultJobId`/);
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`blockedDecisionCount`/);
  assert.match(readme, /`missingManifestEvidenceCount`/);
  assert.match(readme, /`pendingHumanInterventionCount`/);
  assert.match(readme, /`release-readiness-decisions`/);
  assert.match(readme, /`release-readiness-manifest-artifacts`/);
  assert.match(readme, /`release-readiness-blockers`/);
  assert.match(readme, /`release-readiness-human-interventions`/);
  assert.match(readme, /`release-readiness-priority-dispositions`/);
  assert.match(readme, /`release-readiness-learning-observations`/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /`release-readiness-decision:\*`/);
  assert.match(readme, /`release-readiness-blocker:\*`/);
  assert.match(readme, /`release-readiness-intervention:\*`/);
  assert.match(readme, /`release-readiness-artifact:\*`/);
  assert.match(readme, /`release-readiness-priority:\*`/);
  assert.match(readme, /`POST \/execution\/result`/);
  assert.match(readme, /`POST \/fabrication\/execution\/result`/);
  assert.match(readme, /dd\.fabrication\.execution-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.execution-learning-outcome-draft\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.execution\.telemetry\.results/);
  assert.match(readme, /`executionResult`/);
  assert.match(readme, /`executionResultJobId`/);
  assert.match(readme, /stop, operator-action, split\/combine, artifact, reward/);
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`blockingMachineStopCount`/);
  assert.match(readme, /`restartBlockingOperatorInterventionCount`/);
  assert.match(readme, /`splitCombineBlockerCount`/);
  assert.match(readme, /`execution-machine-stops`/);
  assert.match(readme, /`execution-operator-interventions`/);
  assert.match(readme, /`execution-split-combine-decisions`/);
  assert.match(readme, /`execution-learning-observations`/);
  assert.match(readme, /`execution-stop:\*`/);
  assert.match(readme, /`execution-operator-action:\*`/);
  assert.match(readme, /`execution-split-combine:\*`/);
  assert.match(readme, /`execution-artifact:\*`/);
  assert.match(readme, /`GET \/schedule\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/schedule\/catalog`/);
  assert.match(readme, /dd\.fabrication\.schedule-catalog\.v1/);
  assert.match(readme, /production batching, machine-lane\s+scheduling/);
  assert.match(readme, /`productionPlan\.batches`/);
  assert.match(readme, /`machineSchedule\.dependencyHolds`/);
  assert.match(readme, /`desScheduleModel\.laneModels`/);
  assert.match(readme, /Schedule and\s+DES observations are retained for MDP\/POMDP\/neural workers/);
  assert.match(readme, /`POST \/schedule\/result`/);
  assert.match(readme, /`POST \/fabrication\/schedule\/result`/);
  assert.match(readme, /dd\.fabrication\.schedule-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.schedule-learning-outcome-draft\.v1/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /blocked lanes, overcapacity, invalid operation\s+windows/);
  assert.match(readme, /unstable DES models/);
  assert.match(readme, /`schedule-lanes`/);
  assert.match(readme, /`schedule-des-models`/);
  assert.match(readme, /`schedule-learning-observations`/);
  assert.match(readme, /`GET \/simulation\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/simulation\/catalog`/);
  assert.match(readme, /dd\.fabrication\.simulation-catalog\.v1/);
  assert.match(readme, /dry-run and simulation-risk catalog/);
  assert.match(readme, /`simulation\.programs\.axisExtents`/);
  assert.match(readme, /`simulation\.riskProfile\.learningObservations`/);
  assert.match(
    readme,
    /machine-ready release remains blocked\s+while simulation risk is blocked/,
  );
  assert.match(readme, /Simulation-risk\s+observations are emitted for MDP\/POMDP\/neural workers/);
  assert.match(readme, /`GET \/simulation\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/simulation\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.simulation-preflight-catalog\.v1/);
  assert.match(readme, /machine envelope\/fixture\/datum state/);
  assert.match(readme, /controller\/process\/program state/);
  assert.match(readme, /dry-run\/release\/learning state/);
  assert.match(readme, /`POST \/simulation\/run`/);
  assert.match(readme, /`POST \/fabrication\/simulation\/run`/);
  assert.match(readme, /dd\.fabrication\.simulation-run\.v1/);
  assert.match(readme, /`simulation\.riskProfile\.programRisks`/);
  assert.match(readme, /`simulation\.failureBoundaries`/);
  assert.match(
    readme,
    /reroute, split parts, add clearance, regenerate programs, or require\s+operator review/,
  );
  assert.match(readme, /`POST \/simulation\/result`/);
  assert.match(readme, /`POST \/fabrication\/simulation\/result`/);
  assert.match(readme, /dd\.fabrication\.instruction-simulation-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-simulation-learning-outcome-draft\.v1/);
  assert.match(readme, /dd\.remote\.fabrication\.instructions\.simulation\.results/);
  assert.match(readme, /`priorityDispositions` array/);
  assert.match(readme, /`instructionSimulationResult`/);
  assert.match(readme, /`simulationResultJobId`/);
  assert.match(
    readme,
    /simulator, check, finding, boundary, recommended-action, artifact,\s+priority-disposition, reward/,
  );
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`\/jobs\/:job_id\/artifacts\/:artifact_id`/);
  assert.match(readme, /`blockedEnvelopeCheckCount`/);
  assert.match(readme, /`missingArtifactEvidenceCount`/);
  assert.match(readme, /`instruction-simulation-envelope-checks`/);
  assert.match(readme, /`instruction-simulation-failure-boundaries`/);
  assert.match(readme, /`instruction-simulation-priority-dispositions`/);
  assert.match(readme, /`instruction-simulation-learning-observations`/);
  assert.match(readme, /`instruction-simulation-boundary-kind:\*`/);
  assert.match(readme, /`instruction-simulation-artifact:\*`/);
  assert.match(readme, /`simulation-priority:<priority>:<disposition>`/);
  assert.match(readme, /`GET \/quality\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/quality\/catalog`/);
  assert.match(readme, /dd\.fabrication\.quality-catalog\.v1/);
  assert.match(readme, /inspection and metrology catalog/);
  assert.match(readme, /`qualityPlan\.inspectionPoints`/);
  assert.match(readme, /machine-ready release remains blocked while required quality\s+inspection/);
  assert.match(readme, /Quality observations are retained for MDP\/POMDP\/neural\s+workers/);
  assert.match(readme, /`GET \/quality\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/quality\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.quality-preflight-catalog\.v1/);
  assert.match(readme, /quality-release checklist before\s+parts are assembled/);
  assert.match(readme, /metrology instrument\/datum state/);
  assert.match(readme, /first-article\/final-fit\/surface\s+state/);
  assert.match(readme, /nonconformance\/disposition\/learning state/);
  assert.match(readme, /`GET \/fabrication\/dispositions\/catalog`/);
  assert.match(readme, /dd\.fabrication\.disposition-catalog\.v1/);
  assert.match(readme, /rework-and-reinspect/);
  assert.match(readme, /not certified quality acceptance/);
  assert.match(readme, /`POST \/dispositions\/result`/);
  assert.match(readme, /`POST \/fabrication\/dispositions\/result`/);
  assert.match(readme, /dd\.fabrication\.disposition-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.disposition-learning-outcome-draft\.v1/);
  assert.match(readme, /disposition-remediation-actions/);
  assert.match(readme, /disposition-authority-reviews/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /avoid the failed route/);
  assert.match(readme, /`GET \/costing\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/costing\/catalog`/);
  assert.match(readme, /dd\.fabrication\.costing-catalog\.v1/);
  assert.match(readme, /material yield and scrap allowance/);
  assert.match(readme, /not binding quotes/);
  assert.match(readme, /Cost, yield, scrap, cycle-time,\s+and rework outcomes/);
  assert.match(readme, /`POST \/costing\/result`/);
  assert.match(readme, /`POST \/fabrication\/costing\/result`/);
  assert.match(readme, /dd\.fabrication\.costing-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.costing-learning-outcome-draft\.v1/);
  assert.match(readme, /costing-route-comparisons/);
  assert.match(readme, /costing-learning-observations/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /split\/combine route outcome reviews/);
  assert.match(readme, /`GET \/utilities\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/utilities\/catalog`/);
  assert.match(readme, /dd\.fabrication\.utilities-catalog\.v1/);
  assert.match(readme, /subtractive coolant\/chip\/dust\/air support/);
  assert.match(readme, /not certified machine\s+safety approval/);
  assert.match(readme, /Utility outages, restarts, operator recovery/);
  assert.match(readme, /`GET \/energy\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/energy\/catalog`/);
  assert.match(readme, /dd\.fabrication\.energy-catalog\.v1/);
  assert.match(readme, /subtractive spindle\/axis\/coolant load/);
  assert.match(readme, /not utility billing, certified electrical design/);
  assert.match(readme, /split, combine, defer, or reroute/);
  assert.match(readme, /`POST \/energy\/result`/);
  assert.match(readme, /`POST \/fabrication\/energy\/result`/);
  assert.match(readme, /dd\.fabrication\.energy-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.energy-learning-outcome-draft\.v1/);
  assert.match(readme, /power checks, thermal-load\s+checks/);
  assert.match(readme, /energy-power-checks/);
  assert.match(readme, /power budgets, cooldown windows/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`POST \/utilities\/result`/);
  assert.match(readme, /`POST \/fabrication\/utilities\/result`/);
  assert.match(readme, /dd\.fabrication\.utilities-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.utilities-learning-outcome-draft\.v1/);
  assert.match(readme, /utilities-recovery-actions/);
  assert.match(readme, /utilities-learning-observations/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /generated\s+or imported instructions releasable/);
  assert.match(readme, /`GET \/telemetry\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/telemetry\/catalog`/);
  assert.match(readme, /dd\.fabrication\.telemetry-catalog\.v1/);
  assert.match(readme, /simulation-to-runtime\s+boundary correlation/);
  assert.match(readme, /not certified machine safety\s+validation/);
  assert.match(readme, /Telemetry outcomes feed MDP\/POMDP\/neural workers/);
  assert.match(readme, /`GET \/availability\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/availability\/catalog`/);
  assert.match(readme, /dd\.fabrication\.availability-catalog\.v1/);
  assert.match(readme, /live machine\s+state and queue capacity/);
  assert.match(readme, /machineSchedule\.machineLanes/);
  assert.match(readme, /fallback machines,\s+split\/combine capacity/);
  assert.match(readme, /`POST \/availability\/result`/);
  assert.match(readme, /`POST \/fabrication\/availability\/result`/);
  assert.match(readme, /dd\.fabrication\.availability-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.availability-learning-outcome-draft\.v1/);
  assert.match(readme, /availability-fallback-options/);
  assert.match(readme, /availability-learning-observations/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`GET \/maintenance\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/maintenance\/catalog`/);
  assert.match(readme, /dd\.fabrication\.maintenance-catalog\.v1/);
  assert.match(readme, /lockout\/tagout release/);
  assert.match(readme, /machineProfile\.evidence\.maintenance/);
  assert.match(readme, /schedule service before release/);
  assert.match(readme, /`POST \/maintenance\/result`/);
  assert.match(readme, /`POST \/fabrication\/maintenance\/result`/);
  assert.match(readme, /dd\.fabrication\.maintenance-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.maintenance-learning-outcome-draft\.v1/);
  assert.match(readme, /maintenance-lockout-clearances/);
  assert.match(readme, /maintenance-learning-observations/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /route-across-healthier-equipment/);
  assert.match(readme, /`POST \/telemetry\/result`/);
  assert.match(readme, /`POST \/fabrication\/telemetry\/result`/);
  assert.match(readme, /dd\.fabrication\.telemetry-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.telemetry-learning-outcome-draft\.v1/);
  assert.match(readme, /telemetry-boundary-correlations/);
  assert.match(readme, /telemetry-learning-observations/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /prevented or caused\s+runtime failures/);
  assert.match(readme, /`POST \/quality\/plan`/);
  assert.match(readme, /`POST \/fabrication\/quality\/plan`/);
  assert.match(readme, /dd\.fabrication\.quality-planning\.v1/);
  assert.match(readme, /`qualityPlan\.inspectionPoints\.recordsToCapture`/);
  assert.match(readme, /`qualityPlan\.measurementTargets`/);
  assert.match(readme, /`postprocessPlan\.requiredArtifacts`/);
  assert.match(
    readme,
    /add inspection, split parts, adjust\s+processes, regenerate instructions, or require human signoff/,
  );
  assert.match(readme, /`POST \/quality\/result`/);
  assert.match(readme, /`POST \/fabrication\/quality\/result`/);
  assert.match(readme, /dd\.fabrication\.quality-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.quality-learning-outcome-draft\.v1/);
  assert.match(readme, /out-of-tolerance\s+measurements/);
  assert.match(readme, /nonconformance or human-intervention findings/);
  assert.match(readme, /`priorityDispositions` array/);
  assert.match(readme, /`quality-priority-dispositions`/);
  assert.match(readme, /`quality-priority:<priority>:<disposition>`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`quality-result`/);
  assert.match(readme, /`quality-measurements`/);
  assert.match(readme, /`quality-learning-observations`/);
  assert.match(readme, /`GET \/calibration\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/calibration\/catalog`/);
  assert.match(readme, /dd\.fabrication\.calibration-catalog\.v1/);
  assert.match(readme, /homing, work-offset, tool-length,\s+probe, thermal/);
  assert.match(readme, /`releaseProbePlan\.probes`/);
  assert.match(readme, /`improvedPrograms\.patchManifest\.operations`/);
  assert.match(readme, /machine-ready release remains blocked while homing/);
  assert.match(readme, /Calibration observations are retained for MDP\/POMDP\/neural\s+workers/);
  assert.match(readme, /`POST \/calibration\/plan`/);
  assert.match(readme, /`POST \/fabrication\/calibration\/plan`/);
  assert.match(readme, /dd\.fabrication\.calibration-planning\.v1/);
  assert.match(readme, /`releaseProbePlan\.probes\.requiredBeforeState`/);
  assert.match(readme, /`fixturePlan\.setups\.datumScheme`/);
  assert.match(readme, /Stored\s+artifacts include `release-probe-plan`, `machine-release`, `tooling-plan`/);
  assert.match(readme, /`POST \/calibration\/result`/);
  assert.match(readme, /`POST \/fabrication\/calibration\/result`/);
  assert.match(readme, /dd\.fabrication\.calibration-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.calibration-learning-outcome-draft\.v1/);
  assert.match(readme, /out-of-tolerance offsets/);
  assert.match(readme, /unresolved release probes/);
  assert.match(readme, /`priorityDispositions` array/);
  assert.match(readme, /`calibration-priority-dispositions`/);
  assert.match(readme, /`calibration-priority:<priority>:<disposition>`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /`calibration-result`/);
  assert.match(readme, /`calibration-offsets`/);
  assert.match(readme, /`calibration-learning-observations`/);
  assert.match(readme, /`GET \/interventions\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/interventions\/catalog`/);
  assert.match(readme, /dd\.fabrication\.intervention-catalog\.v1/);
  assert.match(readme, /action contracts, automation types, evidence-gate\s+contracts/);
  assert.match(readme, /`operatorInterventionPlan\.requiredOperatorActions`/);
  assert.match(readme, /`executionPlan\.stopPoints`/);
  assert.match(
    readme,
    /machine-ready release\s+remains blocked while required operator actions/,
  );
  assert.match(readme, /Human-intervention and automation observations/);
  assert.match(readme, /`POST \/interventions\/result`/);
  assert.match(readme, /`POST \/fabrication\/interventions\/result`/);
  assert.match(readme, /dd\.fabrication\.intervention-result-review\.v1/);
  assert.match(readme, /operator actions are incomplete/);
  assert.match(readme, /`priorityDispositions` array/);
  assert.match(readme, /intervention-priority-dispositions/);
  assert.match(readme, /`intervention-priority:<priority>:<disposition>`/);
  assert.match(readme, /intervention-automation-handoffs/);
  assert.match(readme, /dd\.fabrication\.intervention-learning-outcome-draft\.v1/);
  assert.match(readme, /`GET \/setup\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/setup\/catalog`/);
  assert.match(readme, /dd\.fabrication\.setup-catalog\.v1/);
  assert.match(readme, /tooling, fixture, datum, workholding, runtime\s+monitoring/);
  assert.match(readme, /Swiss guide-bushing\/bar-feed grip\/support/);
  assert.match(readme, /`toolingPlan\.requirements`/);
  assert.match(readme, /`fixturePlan\.setups\.clearanceChecks`/);
  assert.match(readme, /`monitoringPlan\.alertRules`/);
  assert.match(readme, /machine-ready\s+release remains blocked while required tools/);
  assert.match(readme, /Setup, fixture, and\s+monitoring observations are retained/);
  assert.match(readme, /`POST \/setup\/plan`/);
  assert.match(readme, /`POST \/fabrication\/setup\/plan`/);
  assert.match(readme, /dd\.fabrication\.setup-planning\.v1/);
  assert.match(readme, /`fixturePlan\.setups\.requiredEvidence`/);
  assert.match(readme, /`monitoringPlan\.monitorPoints\.channels`/);
  assert.match(
    readme,
    /change\s+workholding, split setups, add automation, regenerate\s+instructions, or require human intervention/,
  );
  assert.match(readme, /`POST \/setup\/result`/);
  assert.match(readme, /`POST \/fabrication\/setup\/result`/);
  assert.match(readme, /dd\.fabrication\.setup-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.setup-learning-outcome-draft\.v1/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /out-of-tolerance datum transfers/);
  assert.match(readme, /monitoring\s+channels without heartbeat or safe-stop evidence/);
  assert.match(readme, /`priorityDispositions`/);
  assert.match(readme, /`setup-priority:<priority>:<disposition>`/);
  assert.match(readme, /`setup-result`/);
  assert.match(readme, /`setup-datum-transfers`/);
  assert.match(readme, /`setup-monitoring-channels`/);
  assert.match(readme, /`setup-priority-dispositions`/);
  assert.match(readme, /`setup-learning-observations`/);
  assert.match(readme, /`GET \/monitoring\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/monitoring\/catalog`/);
  assert.match(readme, /dd\.fabrication\.monitoring-catalog\.v1/);
  assert.match(readme, /runtime monitoring, safe-stop, recovery/);
  assert.match(readme, /`monitoringPlan\.monitorPoints`/);
  assert.match(readme, /`monitoringPlan\.recoveryActions`/);
  assert.match(readme, /machine-ready and unattended release remain blocked/);
  assert.match(readme, /Monitoring and recovery observations are retained/);
  assert.match(readme, /`POST \/monitoring\/plan`/);
  assert.match(readme, /`POST \/fabrication\/monitoring\/plan`/);
  assert.match(readme, /dd\.fabrication\.monitoring-planning\.v1/);
  assert.match(readme, /`monitoringPlan\.monitorPoints\.channels`/);
  assert.match(readme, /`monitoringPlan\.alertRules\.automatedResponse`/);
  assert.match(
    readme,
    /add sensors, split\s+jobs, require operators, add automation, or improve\s+generated instructions/,
  );
  assert.match(readme, /`POST \/monitoring\/result`/);
  assert.match(readme, /`POST \/fabrication\/monitoring\/result`/);
  assert.match(readme, /dd\.fabrication\.monitoring-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.monitoring-learning-outcome-draft\.v1/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /missing channel heartbeat or signal-envelope\s+evidence/);
  assert.match(readme, /safe-stop triggers, restart blockers/);
  assert.match(readme, /`monitoring-alerts`/);
  assert.match(readme, /`monitoring-recovery-actions`/);
  assert.match(readme, /`monitoring-operator-interventions`/);
  assert.match(readme, /`monitoring-learning-observations`/);
  assert.match(readme, /`GET \/evidence\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/evidence\/catalog`/);
  assert.match(readme, /dd\.fabrication\.evidence-catalog\.v1/);
  assert.match(readme, /global evidence taxonomy/);
  assert.match(readme, /design-source evidence/);
  assert.match(readme, /instruction\/controller evidence/);
  assert.match(readme, /learning-outcome evidence/);
  assert.match(readme, /`machineRelease\.blockers`/);
  assert.match(readme, /split\/combine results/);
  assert.match(readme, /`releaseGateMatrix` crosswalk/);
  assert.match(readme, /source-provenance`, `machine-envelope`, `process-readiness`/);
  assert.match(readme, /`human-or-automation-handoff`, and\s+`learning-disposition`/);
  assert.match(readme, /map evidence requirements to machine-ready blockers/);
  assert.match(readme, /`GET \/artifacts\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/artifacts\/catalog`/);
  assert.match(readme, /dd\.fabrication\.artifact-catalog\.v1/);
  assert.match(readme, /generated or imported machine instruction work/);
  assert.match(readme, /DES-backed MDP\/POMDP\/neural learning evidence/);
  assert.match(readme, /`learning-policy-snapshot`/);
  assert.match(readme, /`learning-outcome-memory`/);
  assert.match(readme, /`learning-corpus`/);
  assert.match(readme, /`GET \/fabrication\/jobs`, `GET \/jobs\/:job_id`/);
  assert.match(readme, /`GET \/jobs\/:job_id\/artifacts\/:artifact_id`/);
  assert.match(readme, /`GET \/fabrication\/jobs\/:job_id\/artifacts\/:artifact_id`/);
  assert.match(readme, /`releasePackagePlan`/);
  assert.match(readme, /generated design exports, machine programs, improved\s+programs/);
  assert.match(readme, /`GET \/packages\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/packages\/catalog`/);
  assert.match(readme, /dd\.fabrication\.package-catalog\.v1/);
  assert.match(readme, /retained package contract/);
  assert.match(readme, /`machine-code-result`/);
  assert.match(readme, /`instruction-validation-result`/);
  assert.match(readme, /`interface-control-plan`/);
  assert.match(readme, /`releaseHandoffMatrix`/);
  assert.match(readme, /generated design exports, generated machine code,\s+imported instruction streams/);
  assert.match(readme, /immutable\s+original instruction retention/);
  assert.match(readme, /printed, milled, turned, cut,\s+or manual operations/);
  assert.match(readme, /MDP\/POMDP\/DES\/neural\s+primitive provenance/);
  assert.match(readme, /attempted release-gate bypasses/);
  assert.match(readme, /cannot bypass package release gates/);
  assert.match(readme, /`POST \/packages\/plan`/);
  assert.match(readme, /`POST \/fabrication\/packages\/plan`/);
  assert.match(readme, /dd\.fabrication\.package-planning\.v1/);
  assert.match(readme, /dd\.fabrication\.package-plan\.v1/);
  assert.match(readme, /same `FabricationPlanRequest`/);
  assert.match(readme, /generated programs, machine-ready\s+programs/);
  assert.match(readme, /cannot\s+bypass release gates/);
  assert.match(readme, /`GET \/learning\/capabilities`/);
  assert.match(readme, /`GET \/fabrication\/learning\/capabilities`/);
  assert.match(readme, /`GET \/learning\/engines\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/engines\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-capability-catalog\.v1/);
  assert.match(readme, /solve_pomdp_underlying/);
  assert.match(readme, /FeedForwardNetwork/);
  assert.match(readme, /`GET\s+\/fabrication\/learning\/outcomes`/);
  assert.match(readme, /`outcomeQualitySurfaces`/);
  assert.match(readme, /`learningOutcomes\.qualityBuckets\.policyUse`/);
  assert.match(
    readme,
    /`strategyRecommendation\.learningOutcomeQuality\.releasePolicy`/,
  );
  assert.match(readme, /`GET \/learning\/preflight\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/preflight\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-preflight-catalog\.v1/);
  assert.match(readme, /`pomdpBeliefState\.hiddenStates`/);
  assert.match(readme, /`neuralTrainingCorpus\.examples`/);
  assert.match(readme, /machine-ready release remains blocked while artifact provenance/);
  assert.match(readme, /`GET \/learning\/features\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/features\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-feature-catalog\.v1/);
  assert.match(readme, /feature-map discovery contract/);
  assert.match(readme, /plan route\/material state/);
  assert.match(readme, /instruction-validation boundary state/);
  assert.match(readme, /split\/combine interface state/);
  assert.match(readme, /`toolpath-token-sequence`/);
  assert.match(readme, /`hybridMakePlan\.splitCombineDecisions`/);
  assert.match(readme, /`hybridDecisionFeatureContracts`/);
  assert.match(readme, /attempt a one-piece build/);
  assert.match(readme, /split work across printed, milled, turned, or\s+sheet-cut subparts/);
  assert.match(readme, /`route-decomposition-action`/);
  assert.match(readme, /`split-for-printing`/);
  assert.match(readme, /`split-for-milling`/);
  assert.match(readme, /`split-for-turning`/);
  assert.match(readme, /`interfaceControlPlan\.controls`/);
  assert.match(readme, /Feature vectors are deterministic planning\s+evidence only/);
  assert.match(readme, /`GET \/learning\/rewards\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/rewards\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-reward-catalog\.v1/);
  assert.match(readme, /machine-failure boundary penalties/);
  assert.match(readme, /Positive rewards cannot bypass validation/);
  assert.match(readme, /`GET \/learning\/models\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/models\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-model-catalog\.v1/);
  assert.match(readme, /MDP policy snapshots, POMDP belief policies/);
  assert.match(readme, /first-class\s+`splitCombinePreferences`/);
  assert.match(readme, /split-combine-preference/);
  assert.match(readme, /`splitCombineHints` to seed learned assembly strategies and first-class\s+`splitCombinePreferences`/);
  assert.match(readme, /cannot bypass validation findings/);
  assert.match(readme, /`GET \/learning\/beliefs\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/beliefs\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-belief-catalog\.v1/);
  assert.match(readme, /`pomdpBeliefState\.hiddenStates`/);
  assert.match(readme, /Belief probabilities are advisory priors/);
  assert.match(readme, /`GET \/learning\/optimizers\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/optimizers\/catalog`/);
  assert.match(readme, /dd\.fabrication\.learning-optimizer-catalog\.v1/);
  assert.match(readme, /MDP route-action,\s+POMDP hidden-risk/);
  assert.match(readme, /`POST \/learning\/models\/result`/);
  assert.match(readme, /`POST \/fabrication\/learning\/models\/result`/);
  assert.match(readme, /dd\.fabrication\.learning-model-result-review\.v1/);
  assert.match(readme, /`learning-model-card-compatibility` artifact/);
  assert.match(readme, /`neuralTrainingCorpus\.featureNames` contract/);
  assert.match(readme, /`learning\.outcomeDraft`/);
  assert.match(readme, /promotion for future advisory\s+planning requires retained artifacts/);
  assert.match(readme, /`GET \/learning\/replay\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/learning\/replay\/catalog`/);
  assert.match(readme, /baseline and candidate actions/);
  assert.match(readme, /`POST \/learning\/optimizers\/result`/);
  assert.match(readme, /`POST \/fabrication\/learning\/optimizers\/result`/);
  assert.match(readme, /dd\.fabrication\.learning-optimizer-result-review\.v1/);
  assert.match(readme, /learning-optimizer-promotion-blockers/);
  assert.match(readme, /selected candidate, replay verification, simulation verification/);
  assert.match(readme, /`GET \/schema`/);
  assert.match(readme, /`GET \/fabrication\/schema`/);
  assert.match(readme, /`GET \/examples`/);
  assert.match(readme, /`GET \/fabrication\/examples`/);
  assert.match(readme, /local `des_engine` crate/);
  assert.match(readme, /remote\/submodules\/discrete-event-system\.rs/);
  assert.match(readme, /DES-compatible `desMdpSpec`\/`desPomdpSpec`/);
  assert.match(readme, /DES Studio\s+`desScheduleModel` queue graph/);
  assert.match(readme, /Plan responses also expose a DES Studio\s+`desScheduleModel` queue graph and `instructionIntentMap`/);
  assert.match(readme, /generated\/submitted\s+instruction intent/);
  assert.match(readme, /per-machine `Constant -> Queue -> Sink`/);
  assert.match(readme, /DES Studio `desInstructionModel` queue graph/);
  assert.match(readme, /`instructionIntentMap`/);
  assert.match(readme, /`reviewPriorities` rows/);
  assert.match(readme, /machine-failure boundaries, human-intervention checkpoints/);
  assert.match(readme, /non-G-code job-sheet evidence extraction/);
  assert.match(readme, /normalized process intent, and release handoff lane/);
  assert.match(readme, /`analysis-instruction-intent-map` artifact/);
  assert.match(readme, /machine-failure watchpoints; human-intervention watchpoints/);
  assert.match(readme, /`operatorInterventionPlan\.requiredOperatorActions`/);
  assert.match(readme, /keeps `machineReady=false`/);
  assert.match(readme, /`instruction-intent:\*`/);
  assert.match(readme, /failure-boundary\s+pressure/);
  assert.match(
    readme,
    /value-iteration `desMdpSolution` and QMDP-underlying `desPomdpSolution`/,
  );
  assert.match(readme, /built-in `defaultMachines`/);
  assert.match(readme, /`machineFleetLimits` block/);
  assert.match(readme, /fallback\/submitted machine fleet of up to 96 profiles/);
  assert.match(readme, /Swiss\/sliding-headstock turning centers/);
  assert.match(readme, /accepted instruction\s+kinds/);
  assert.match(readme, /large-format pellet\/FGF/);
  assert.match(readme, /robotic\/gantry additive cells/);
  assert.match(readme, /sheet-lamination\/LOM\/UAM printers/);
  assert.match(readme, /robotic\/gantry additive job sheets with robot frame\/TCP/);
  assert.match(readme, /sheet-lamination\/LOM\/UAM job sheets with sheet\/foil lot/);
  assert.match(readme, /pellet\/FGF pellet-lot\/drying\/moisture\/hopper\/purge/);
  assert.match(readme, /robotic additive robot frame\/TCP\/reach\/collision\/interlock\/external-axis evidence/);
  assert.match(
    readme,
    /sheet-lamination sheet\/foil stock\/stack-order\/surface-prep evidence from generated `LOAD_SHEET_STACK` records/,
  );
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
  assert.match(readme, /`GET \/fabrication\/jobs\/:job_id` is the prefixed alias/);
  assert.match(readme, /`GET \/jobs\/:job_id\/artifacts\/:artifact_id`/);
  assert.match(
    readme,
    /`GET \/fabrication\/jobs\/:job_id\/artifacts\/:artifact_id` as the prefixed\s+alias/,
  );
  assert.match(readme, /`GET \/learning\/policy`/);
  assert.match(readme, /`POST \/learning\/observe`/);
  assert.match(readme, /`POST \/fabrication\/learning\/observe`/);
  assert.match(readme, /`GET \/learning\/outcomes`/);
  assert.match(readme, /`GET \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /dd\.fabrication\.learning-outcome-memory\.v1/);
  assert.match(readme, /retained compact\/rich learning records/);
  assert.match(readme, /`qualitySummary`/);
  assert.match(readme, /`qualityBuckets`/);
  assert.match(readme, /`policyImpactPreview`/);
  assert.match(readme, /method-combination/);
  assert.match(readme, /machine-kind/);
  assert.match(readme, /operation-sequence/);
  assert.match(readme, /remediation-risk/);
  assert.match(readme, /neural-training/);
  assert.match(readme, /failed-or-negative-reward/);
  assert.match(readme, /intervention-heavy history/);
  assert.match(readme, /learned preferences remain advisory/);
  assert.match(readme, /Outcome Learning/);
  assert.match(readme, /When `sourceJobId` points at a retained fabrication-plan job/);
  assert.match(
    readme,
    /missing `programId`,\s+`partId`, `machineId`, `machineKind`, `material`, and `operationSequence`/,
  );
  assert.match(readme, /`source-plan-\*` signals/);
  assert.match(readme, /reward-signal/);
  assert.match(readme, /outcome-remediation-plan/);
  assert.match(readme, /`outcomeRemediation` plan/);
  assert.match(readme, /`remediationRisks`/);
  assert.match(readme, /material-specific `remediationRisks`/);
  assert.match(readme, /learned-remediation-risk/);
  assert.match(readme, /avoid-learned-risk-milling-petg/);
  assert.match(readme, /ordered operation\s+sequences/);
  assert.match(readme, /operation-sequence preferences/);
  assert.match(readme, /`additive-print\+plastic-joining`/);
  assert.match(readme, /plastic-joining policy can add a join lane/);
  assert.match(readme, /learned-operation-sequence-preference/);
  assert.match(readme, /prefer-learned-operation-sequence/);
  assert.match(readme, /machine-kind preferences/);
  assert.match(readme, /learned-machine-kind-preference/);
  assert.match(readme, /prefer-learned-machine-kind/);
  assert.match(readme, /machine-failure hidden-state evidence/);
  assert.match(
    readme,
    /learned-remediation-risk:review-prior-failure-outcome-before-release/,
  );
  assert.match(readme, /review\/avoid policy actions/);
  assert.match(readme, /pomdp-observations/);
  assert.match(readme, /`pomdpBeliefState`/);
  assert.match(readme, /`pomdp-belief-state`/);
  assert.match(readme, /`releaseProbePlan`/);
  assert.match(readme, /`release-probe-plan`/);
  assert.match(readme, /priority evidence probes/);
  assert.match(readme, /required-before-release actions/);
  assert.match(readme, /required review of matching learned\s+remediation-risk memory/);
  assert.match(readme, /feeds those release\s+probes back into `machineRelease`/);
  assert.match(readme, /`release-probe` blockers and checklist\s+evidence/);
  assert.match(readme, /hidden-state probabilities/);
  assert.match(readme, /observation likelihoods/);
  assert.match(readme, /recommended probe actions/);
  assert.match(readme, /neural-example/);
  assert.match(readme, /neural-policy sketch/);
  assert.match(readme, /DES `FeedForwardNetwork`-backed neural-policy sketch/);
  assert.match(readme, /`neuralPolicy\.engineInference`/);
  assert.match(readme, /parameter counts, output scores, top signal/);
  assert.match(readme, /`neuralTrainingCorpus`/);
  assert.match(readme, /`neural-training-corpus`/);
  assert.match(readme, /feature vectors, labels, inference candidates/);
  assert.match(readme, /per-boundary `validation-boundary` examples/);
  assert.match(readme, /linked to resolution actions/);
  assert.match(readme, /`instruction-patch` examples/);
  assert.match(readme, /line-level repair actions/);
  assert.match(readme, /policy-memory examples/);
  assert.match(readme, /`boundaryLearningExamples`/);
  assert.match(readme, /`boundary-memory`/);
  assert.match(readme, /`learned-boundary-memory:\*`/);
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
  assert.match(readme, /`hybridMakePlan`/);
  assert.match(readme, /part routes, join operations, split\/combine decisions/);
  assert.match(readme, /single-piece, split-piece, and assembled outcomes/);
  assert.match(readme, /`materialPlan`/);
  assert.match(readme, /route feedstock, stock forms/);
  assert.match(readme, /material\/stock\/feedstock planning state/);
  assert.match(readme, /without explicit\s+`preferredMethods`/);
  assert.match(readme, /learned hybrid join strategies/);
  assert.match(readme, /`POST \/plan`/);
  assert.match(readme, /`POST \/fabrication\/plan`/);
  assert.match(readme, /`POST \/instructions\/analyze`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/analyze`/);
  assert.match(readme, /`POST \/instructions\/validate`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/validate`/);
  assert.match(readme, /`POST \/instructions\/improve`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/improve`/);
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
  assert.match(readme, /release probe plans/);
  assert.match(readme, /neural training corpora/);
  assert.match(readme, /manufacturing\s+handoffs/);
  assert.match(readme, /material plans/);
  assert.match(readme, /design packages/);
  assert.match(readme, /quality plans/);
  assert.match(readme, /tooling plans/);
  assert.match(readme, /machine-selection traces/);
  assert.match(readme, /production plans/);
  assert.match(readme, /process\s+graphs/);
  assert.match(readme, /intervention maps/);
  assert.match(readme, /assembly graphs/);
  assert.match(readme, /structured `processGraph`/);
  assert.match(readme, /`instructionPrograms` stream/);
  assert.match(readme, /`instruction-programs` artifact/);
  assert.match(readme, /generated drafts and submitted existing instructions together/);
  assert.match(readme, /`boundarySummary` object/);
  assert.match(readme, /typed `automationRequirements`/);
  assert.match(readme, /`resolutionPlan`/);
  assert.match(readme, /`machineRelease` report/);
  assert.match(readme, /release-probe blockers from\s+`releaseProbePlan`/);
  assert.match(readme, /`executionPlan` preflight/);
  assert.match(readme, /program runs, checkpoints, stop points/);
  assert.match(readme, /`postprocessPlan` preflight/);
  assert.match(readme, /controller-specific targets/);
  assert.match(readme, /postprocessor selection/);
  assert.match(readme, /dry-run gates/);
  assert.match(readme, /operator signoff/);
  assert.match(readme, /`controllerPlan` compatibility contract/);
  assert.match(readme, /controller dialect families/);
  assert.match(readme, /postprocessor-known status/);
  assert.match(readme, /required controller checks/);
  assert.match(readme, /controller release gates/);
  assert.match(readme, /`controller-\*` learning observations/);
  assert.match(readme, /`machineSelection` trace/);
  assert.match(readme, /`designPackage`/);
  assert.match(readme, /`designExports` bundle/);
  assert.match(readme, /`designInputReview`/);
  assert.match(readme, /Its `conversionPlan` lists per-input CAD\/model\/slicer conversion worker lanes/);
  assert.match(readme, /design-conversion NATS request\/result subjects/);
  assert.match(readme, /per-input `conversionPlan` worker handoffs/);
  assert.match(readme, /`dd\.remote\.fabrication\.design\.conversion\.requests`/);
  assert.match(readme, /per-input conversion worker\s+lanes, required evidence, review gates, and release blockers/);
  assert.match(readme, /generated design export payloads/);
  assert.match(readme, /3MF\/STL\/STEP\/DXF\/CAM setup\/nesting\/assembly payloads/);
  assert.match(readme, /neutral export targets/);
  assert.match(readme, /Creo\/Pro\/ENGINEER, SOLIDWORKS, Fusion/);
  assert.match(readme, /Siemens NX, CATIA, Onshape/);
  assert.match(readme, /FreeCAD, OpenSCAD, Blender, ZBrush/);
  assert.match(readme, /lightweight CAD\/PMI exchange/);
  assert.match(readme, /STEP\/IGES/);
  assert.match(readme, /JT lightweight CAD\/PMI/);
  assert.match(readme, /PMI\/tessellation/);
  assert.match(readme, /CAD-kernel\s+exchange/);
  assert.match(readme, /color\/scan mesh/);
  assert.match(readme, /PLY\/VRML\/glTF\/AMF color or scan mesh\/package inputs/);
  assert.match(readme, /color\/material\/texture/);
  assert.match(readme, /Parasolid\/ACIS kernel files/);
  assert.match(readme, /kernel-version\/body-count/);
  assert.match(
    readme,
    /lightweight CAD\/PMI,\s+CAD-kernel, color\/scan mesh, and 2D sheet-profile intake/,
  );
  assert.match(readme, /2D sheet\/profile CAD/);
  assert.match(readme, /DXF\/DWG sheet-profile drawings/);
  assert.match(readme, /layer\/kerf\/revision/);
  assert.match(readme, /layer\/kerf\/revision gates/);
  assert.match(readme, /PrusaSlicer\/OrcaSlicer\/Cura\/Bambu Studio FDM project sources/);
  assert.match(readme, /Lychee Slicer\/Chitubox resin project sources/);
  assert.match(readme, /resin-exposure\/support\/wash-cure evidence/);
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
  assert.match(readme, /`fixturePlan`/);
  assert.match(readme, /per-part setup strategies/);
  assert.match(readme, /datum schemes/);
  assert.match(readme, /clearance checks/);
  assert.match(readme, /datum-transfer records/);
  assert.match(readme, /`fixture-\*` learning observations/);
  assert.match(readme, /`monitoringPlan`/);
  assert.match(readme, /runtime sensor channels/);
  assert.match(readme, /expected signals/);
  assert.match(readme, /alert rules/);
  assert.match(readme, /recovery actions/);
  assert.match(readme, /`monitoring-\*` learning observations/);
  assert.match(readme, /`interfaceControlPlan`/);
  assert.match(readme, /`interface-control-plan`/);
  assert.match(readme, /join\/split interfaces/);
  assert.match(readme, /mating-surface evidence/);
  assert.match(readme, /acceptance criteria/);
  assert.match(readme, /decision links/);
  assert.match(readme, /`interface-\*` learning observations/);
  assert.match(readme, /`decompositionPlan`/);
  assert.match(readme, /`decomposition-plan`/);
  assert.match(readme, /explicit split targets/);
  assert.match(readme, /route contracts/);
  assert.match(readme, /recomposition interfaces/);
  assert.match(readme, /`decomposition-\*` learning observations/);
  assert.match(readme, /`releasePackagePlan`/);
  assert.match(readme, /`release-package-plan`/);
  assert.match(readme, /imported\s+`instructionPrograms` stream/);
  assert.match(readme, /retained\s+`instruction-programs` analysis/);
  assert.match(readme, /assembly\/recomposition handoff/);
  assert.match(readme, /design export IDs, controller targets/);
  assert.match(readme, /fixture setups, monitoring points, quality inspections/);
  assert.match(readme, /`release-package\*` learning observations/);
  assert.match(readme, /candidate scores, material\/process/);
  assert.match(readme, /operation gaps, and fallback warnings/);
  assert.match(readme, /`manufacturingHandoff` package/);
  assert.match(readme, /`materialPlan` with route feedstock/);
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
  assert.match(readme, /`controller-plan`/);
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
  assert.match(readme, /`des-schedule-model`/);
  assert.match(readme, /`hybrid-make-plan`/);
  assert.match(readme, /`material-plan`/);
  assert.match(readme, /`analysis-des-instruction-model`/);
  assert.match(readme, /`quality-plan`/);
  assert.match(readme, /`tooling-plan`/);
  assert.match(readme, /`fixture-plan`/);
  assert.match(readme, /`monitoring-plan`/);
  assert.match(readme, /`decomposition-plan`/);
  assert.match(readme, /`release-package-plan`/);
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
    /`mdp-request` artifact includes\s+`learningEngine`, `desMdpSpec`, `desMdpSolution`, `desPomdpSpec`,\s+`desPomdpSolution`, `strategyCandidates`, `interventionSignals`, `pomdpBeliefState`,\s+`releaseProbePlan`, `neuralTrainingCorpus`/,
  );
  assert.match(
    readme,
    /local `des_engine` crate from\s+`remote\/submodules\/discrete-event-system\.rs`/,
  );
  assert.match(readme, /canonical DES MDP\/POMDP schema names/);
  assert.match(readme, /`desMdpSpec`\/`desPomdpSpec` payloads/);
  assert.match(readme, /QMDP-underlying `desPomdpSolution`/);
  assert.match(readme, /dd\.fabrication\.learning-policy-snapshot\.v1/);
  assert.match(readme, /dd\.fabrication\.learning-corpus\.v1/);
  assert.match(readme, /`GET \/fabrication\/learning\/corpus`/);
  assert.match(readme, /`promotionPolicy` notes/);
  assert.match(readme, /DES-backed policy preview/);
  assert.match(readme, /DES queue-capacity model/);
  assert.match(readme, /`desScheduleModel` DES Studio queue blocks/);
  assert.match(readme, /`desInstructionModel` DES Studio review queues/);
  assert.match(readme, /per-program service-rate\s+signals/);
  assert.match(readme, /split\/combine `interfacePlan` objects/);
  assert.match(readme, /decomposition\/recombination gates/);
  assert.match(
    readme,
    /`designPackage`, `designExports`, `designInputReview`, `productionPlan`,\s+`machineSchedule`, `desScheduleModel`, `machineSelection`, `manufacturingHandoff`,\s+`materialPlan`, `qualityPlan`, `toolingPlan`, `fixturePlan`,\s+`monitoringPlan`, `interfaceControlPlan`, and `releasePackagePlan`/,
  );
  assert.match(readme, /execution stop points/);
  assert.match(readme, /unattended-run eligibility/);
  assert.match(readme, /postprocessor gates/);
  assert.match(readme, /inspection calibration records/);
  assert.match(readme, /datum alignment and\s+uncertainty records/);
  assert.match(readme, /first-article measured-values reports/);
  assert.match(readme, /nonconformance\s+disposition records/);
  assert.match(readme, /thermal profile and furnace logs/);
  assert.match(readme, /fixture\/setter and\s+atmosphere records/);
  assert.match(readme, /cooldown\/quench and PPE records/);
  assert.match(readme, /distortion-hardness-release inspection records/);
  assert.match(readme, /assembly-kit travelers/);
  assert.match(readme, /robot-path or fixture simulation\s+reports/);
  assert.match(readme, /final-fit metrology records/);
  assert.match(readme, /`riskProfile`/);
  assert.match(readme, /per-program risk scores/);
  assert.match(readme, /`simulation-risk:\*`/);
  assert.match(readme, /simulation risk profiles/);
  assert.match(readme, /`designExports` generated design export payloads/);
  assert.match(readme, /source previews, media types, blockers/);
  assert.match(readme, /design export state/);
  assert.match(readme, /CAD\/model\/slicer source assumptions/);
  assert.match(
    readme,
    /accepted instruction\s+kinds including slicer, multi-material FDM\/toolchanger, pellet-FGF, robotic-additive, robotic-pellet, robotic-extrusion, sheet-lamination, laminated-object, ultrasonic-additive, paste\/clay extrusion, bound-metal FFF,\s+metal-filament, SLA\/resin,\s+material-jetting, DED\/WAAM, composite-fiber, composite-layup, wet-layup, prepreg-layup, vacuum-bag, autoclave-cure, resin-infusion, hot-wire-foam, hot-wire, foam-cutting, foam-core, wing-core, binder-jet, SLS\/powder, metal-PBF,\s+mill-turn, swiss-turning, lathe\/turning, indexed-mill, assembly-cell, part-separation, laser\/waterjet\/plasma,\s+wire-EDM, sinker-EDM, grinding, CMM inspection, vision inspection, metrology, furnace, heat-treatment, thermal-postprocess, surface-finishing, coating, plating, anodizing, media-blasting, powder-coating, deburr-polish, metal-joining, welding, brazing, soldering, molding-casting, casting, molding, urethane-casting, silicone-molding, vacuum-casting, injection-molding, PCB assembly, SMT assembly, pick-and-place, reflow, press-brake, sheet-forming, bend, gear-cutting, gear-hobbing, and spline-broaching job sheets/,
  );
  assert.match(readme, /batch-planning\s+state/);
  assert.match(readme, /machine-schedule state/);
  assert.match(readme, /hybrid make\/split decisions/);
  assert.match(readme, /`machine-schedule`/);
  assert.match(readme, /operation windows/);
  assert.match(readme, /quality evidence targets/);
  assert.match(readme, /tooling\/setup,\s+fixture\/setup,\s+runtime monitoring\s+requirements/);
  assert.match(readme, /intervention\s+paths/);
  assert.match(readme, /DES-backed policy preview/);
  assert.match(readme, /machine-choice\s+alternatives/);
  assert.match(readme, /CAD\/CAM\s+handoff\s+assumptions/);
  assert.match(readme, /ordered release-blocking remediation steps/);
  assert.match(readme, /attempts machine-ready release/);
  assert.match(readme, /vertical\/5-axis\/4th-axis\/horizontal mills/);
  assert.match(readme, /mill-turn\/swiss-turning centers/);
  assert.match(readme, /ISO-style five-axis TCP\/RTCP milling G-code/);
  assert.match(readme, /ISO-style 4th-axis rotary-indexed milling G-code/);
  assert.match(readme, /brake\/clamp, clearance-sweep, and re-probe checkpoints/);
  assert.match(readme, /horizontal side-slot\/keyway milling/);
  assert.match(readme, /mill-turn\s+G-code with C\/Y-axis live-tooling and subspindle transfer/);
  assert.match(
    readme,
    /Swiss\/sliding-headstock G-code with guide-bushing, bar-feed, gang-tool\/live-tool,\s+subspindle pickoff, cutoff\/ejection, and first-article runout checkpoints/,
  );
  assert.match(readme, /Fanuc-style turning G-code with chuck\/stick-out\/runout/);
  assert.match(readme, /G50\/G95, threading,\s+part-off catcher\/support, coolant shutdown, and turret-stop checkpoints/);
  assert.match(readme, /mill-turn live-tooling C\/Y-axis\/polar-interpolation evidence/);
  assert.match(readme, /mill-turn main\/sub-spindle transfer evidence/);
  assert.match(readme, /Swiss guide-bushing\/bar-feed\/collet\/remnant evidence/);
  assert.match(readme, /Swiss gang-tool\/live-tool clearance/);
  assert.match(readme, /Swiss subspindle pickoff\/cutoff\/ejection\/runout evidence/);
  assert.match(readme, /horizontal-milled side\s+slots\/keyways/);
  assert.match(readme, /SLA\/MSLA resin print-wash-cure job sheets/);
  assert.match(
    readme,
    /multi-material FDM\/toolchanger job sheets with material\/color map,\s+AMS\/MMU\/IDEX\/toolhead slots, purge\/wipe tower, tool-change script, runout\/resume-state,\s+and interface inspection gates/,
  );
  assert.match(readme, /large-format pellet\/FGF job sheets with pellet lot,\s+drying\/moisture/);
  assert.match(readme, /paste\/clay extrusion job sheets with rheology\/slump,\s+nozzle\/pressure, drying\/humidity, shrinkage, green-part support, and kiln\/firing gates/);
  assert.match(readme, /bound-metal filament FFF job sheets with filament\/profile, hardened-nozzle,\s+green-part, debind, sinter, furnace-atmosphere, shrinkage-coupon, density, and\s+inspection gates/);
  assert.match(readme, /PolyJet\/material-jetting\s+photopolymer job sheets/);
  assert.match(readme, /cartridge, channel-map, printhead, support-removal,\s+UV, and color\/material inspection gates/);
  assert.match(readme, /continuous-fiber composite\s+matrix\/fiber-layup job sheets/);
  assert.match(readme, /fiber orientation, cutter, spool, coupon, and\s+delamination gates/);
  assert.match(readme, /composite layup\/vacuum-bag\/autoclave job sheets/);
  assert.match(
    readme,
    /mold\/mandrel revision, release film or agent, ply kit\/schedule,\s+resin\/prepreg\/core lots, vacuum-bag leak-down/,
  );
  assert.match(readme, /hot-wire foam cutting job sheets/);
  assert.match(
    readme,
    /foam density, blank thickness, template\s+or CNC profile, bow\/wire tension, wire heat\/current, kerf coupon/,
  );
  assert.match(readme, /SLS\/MJF-style powder-bed/);
  assert.match(readme, /DMLS\/SLM\/LPBF metal powder-bed fusion job/);
  assert.match(readme, /inert-gas\/recoater\/stress-relief\/plate-removal gates/);
  assert.match(readme, /DED\/WAAM\s+directed-energy deposition job sheets/);
  assert.match(readme, /feedstock, bead-path, shielding-gas,\s+melt-pool, interpass, NDE\/coupon, and finish-machining allowance gates/);
  assert.match(readme, /binder-jet\s+green-part cure\/depowder\/sinter or infiltration job sheets/);
  assert.match(readme, /binder-saturation,\s+printhead, green-strength, and shrink-coupon gates/);
  assert.match(readme, /laser, waterjet, plasma, wire EDM\/sheet cutters/);
  assert.match(readme, /sinker\/ram EDM cells/);
  assert.match(readme, /CMM\/vision inspection/);
  assert.match(readme, /CMM\/vision inspection cells/);
  assert.match(readme, /CMM\/vision first-article inspection releases/);
  assert.match(
    readme,
    /surface finishing\/coating\/plating\/anodizing\/media-blasting\/powder-coating\/deburr-polish job sheets/,
  );
  assert.match(
    readme,
    /SDS\/media, masking\/plugs, ventilation\/PPE\/waste, dry\/cure, thickness,\s+adhesion\/color\/roughness/,
  );
  assert.match(
    readme,
    /metal-joining\/welding\/brazing\/soldering job sheets with WPS\/procedure/,
  );
  assert.match(
    readme,
    /filler\/flux\/gas lots, joint prep, fit-up, fixture\/clamps,\s+fume controls, heat input, interpass temperature/,
  );
  assert.match(
    readme,
    /molding\/casting\/vacuum-casting\/urethane\/silicone\/injection-molding job\s+sheets/,
  );
  assert.match(
    readme,
    /master\/tool revision, mold material, parting line,\s+vents\/sprues\/runners\/gates, release agent/,
  );
  assert.match(
    readme,
    /mix ratio, pot life,\s+degas\/vacuum\/pressure, cure\/exotherm, demold, shrinkage/,
  );
  assert.match(
    readme,
    /press-brake\/sheet-forming job sheets with flat-pattern revision/,
  );
  assert.match(
    readme,
    /bend allowance\/K-factor, punch\/V-die tooling,\s+tonnage, backgauge, bend sequence, springback/,
  );
  assert.match(readme, /gear-cutting\/hobbing\/spline-broaching job sheets/);
  assert.match(readme, /gear drawing, blank\s+datum, arbor\/runout/);
  assert.match(readme, /module or diametral pitch/);
  assert.match(readme, /robotic assembly\/joining cells/);
  assert.match(readme, /molding\/casting\/vacuum-casting\/urethane\/silicone\/injection-molding cells/);
  assert.match(readme, /composite layup\/vacuum-bag\/autoclave cells/);
  assert.match(readme, /hot-wire foam cutters/);
  assert.match(readme, /gear-cutting\/hobbing\/spline-broaching cells/);
  assert.match(readme, /waterjet cutter, plasma cutter/);
  assert.match(readme, /wire\s+EDM cutter/);
  assert.match(readme, /sinker\s+EDM cell/);
  assert.match(readme, /robotic assembly cell/);
  assert.match(
    readme,
    /slicer, multi-material FDM\/toolchanger, pellet-FGF, robotic-additive, robotic-pellet, robotic-extrusion, sheet-lamination, laminated-object, ultrasonic-additive, paste\/clay extrusion, bound-metal FFF,\s+metal-filament, SLA\/resin,\s+material-jetting, DED\/WAAM, composite-fiber, composite-layup, wet-layup, prepreg-layup, vacuum-bag, autoclave-cure, resin-infusion, hot-wire-foam, hot-wire, foam-cutting, foam-core, wing-core, binder-jet, SLS\/powder, metal-PBF,\s+mill-turn, swiss-turning, lathe\/turning, indexed-mill, assembly-cell, part-separation, laser\/waterjet\/plasma,\s+wire-EDM, sinker-EDM, grinding, CMM inspection, vision inspection, metrology, furnace, heat-treatment, thermal-postprocess, surface-finishing, coating, plating, anodizing, media-blasting, powder-coating, deburr-polish, metal-joining, welding, brazing, soldering, molding-casting, casting, molding, urethane-casting, silicone-molding, vacuum-casting, injection-molding, PCB assembly, SMT assembly, pick-and-place, reflow, press-brake, sheet-forming, bend, gear-cutting, gear-hobbing, and spline-broaching job sheets/,
  );
  assert.match(readme, /`molding-casting-job`/);
  assert.match(readme, /`urethane-casting-job`/);
  assert.match(readme, /`vacuum-casting-job`/);
  assert.match(readme, /`pcb-assembly-job`/);
  assert.match(readme, /`smt-assembly-job`/);
  assert.match(readme, /`pick-and-place-job`/);
  assert.match(readme, /`reflow-job`/);
  assert.match(readme, /`composite-layup-job`/);
  assert.match(readme, /`prepreg-layup-job`/);
  assert.match(readme, /`vacuum-bag-job`/);
  assert.match(readme, /`autoclave-cure-job`/);
  assert.match(readme, /`resin-infusion-job`/);
  assert.match(readme, /`hot-wire-foam-job`/);
  assert.match(readme, /`hot-wire-job`/);
  assert.match(readme, /`foam-cutting-job`/);
  assert.match(readme, /`foam-core-job`/);
  assert.match(readme, /`wing-core-job`/);
  assert.match(readme, /molding\/casting\/vacuum-casting\/urethane\/silicone release/);
  assert.match(readme, /composite layup\/prepreg\/wet-layup\/vacuum-bag\/autoclave\/resin-infusion release/);
  assert.match(readme, /hot-wire foam cutting release/);
  assert.match(
    readme,
    /kerf\s+tests, wire-thread\/skim-pass\/slug-retention gates,\s+and\s+fire\/fume\/dielectric\/flushing gates/,
  );
  assert.match(readme, /sinker\/ram EDM cavity burn sheets/);
  assert.match(readme, /CMM\/vision dimensional inspection/);
  assert.match(readme, /CMM\/vision inspection job sheets/);
  assert.match(readme, /thermal postprocess furnace\/oven release/);
  assert.match(
    readme,
    /surface finishing\/coating\/plating\/anodizing\/media-blasting\/powder-coating\/deburr release/,
  );
  assert.match(readme, /metal-joining\/welding\/brazing\/soldering release/);
  assert.match(readme, /press-brake\/sheet-forming release/);
  assert.match(readme, /gear\/spline cutting release/);
  assert.match(readme, /thermal postprocess furnace job\s+sheets/);
  assert.match(
    readme,
    /material batch, fixture\/setter, ramp\/soak, atmosphere,\s+cooldown\/quench, PPE, distortion, hardness\/cure/,
  );
  assert.match(
    readme,
    /probe or vision\s+calibration, datum alignment, first-article measured values/,
  );
  assert.match(readme, /`cmm-inspection-job`/);
  assert.match(readme, /`vision-inspection-job`/);
  assert.match(readme, /`metrology-job`/);
  assert.match(readme, /`thermal-postprocess-job`/);
  assert.match(readme, /`furnace-job`/);
  assert.match(readme, /`heat-treatment-job`/);
  assert.match(readme, /`surface-finishing-job`/);
  assert.match(readme, /`coating-job`/);
  assert.match(readme, /`plating-job`/);
  assert.match(readme, /`anodizing-job`/);
  assert.match(readme, /`media-blasting-job`/);
  assert.match(readme, /`powder-coating-job`/);
  assert.match(readme, /`deburr-polish-job`/);
  assert.match(readme, /`metal-joining-job`/);
  assert.match(readme, /`welding-job`/);
  assert.match(readme, /`brazing-job`/);
  assert.match(readme, /`soldering-job`/);
  assert.match(readme, /`press-brake-job`/);
  assert.match(readme, /`sheet-forming-job`/);
  assert.match(readme, /`bend-job`/);
  assert.match(readme, /`gear-cutting-job`/);
  assert.match(readme, /`gear-hobbing-job`/);
  assert.match(readme, /`spline-broaching-job`/);
  assert.match(
    readme,
    /electrode, dielectric\/flushing, orbit-finish, depth-stop, and wear-compensation\s+gates/,
  );
  assert.match(readme, /robotic assembly-cell job sheets/);
  assert.match(readme, /robot path\/gripper\/fixture\/vision evidence/);
  assert.match(readme, /press\/heat-set\/torque\/adhesive\s+join recipes/);
  assert.match(readme, /SLA resin printer, multi-material FDM\/toolchanger printer, paste\/clay extrusion printer, bound-metal filament FFF printer, material-jetting printer, continuous-fiber composite printer, composite layup cell, SLS powder-bed printer, DED\/WAAM directed-energy deposition cell/);
  assert.match(readme, /hot-wire foam cutter/);
  assert.match(readme, /thermal postprocess furnace/);
  assert.match(readme, /surface finishing cell/);
  assert.match(readme, /metal-joining cell/);
  assert.match(readme, /press-brake forming cell/);
  assert.match(readme, /gear-cutting\/hobbing\/spline-broaching cell/);
  assert.match(readme, /thermal anneal\/stress-relief\/heat-treatment\/post-cure releases/);
  assert.match(
    readme,
    /surface finishing\/coating\/plating\/anodizing\/media-blasting\/powder-coating\/deburr releases/,
  );
  assert.match(readme, /metal-joining\/welding\/brazing\/soldering releases/);
  assert.match(readme, /press-brake sheet-metal flanges\/bend sequences\/formed brackets/);
  assert.match(readme, /gear teeth\/splines\/racks\/keyways\/worm profiles/);
  assert.match(readme, /multi-material-fdm-print/);
  assert.match(readme, /paste-extrusion-print/);
  assert.match(readme, /bound-metal-fff-print/);
  assert.match(readme, /material-jetting-print/);
  assert.match(readme, /directed-energy-deposition/);
  assert.match(readme, /composite-fiber-print/);
  assert.match(readme, /composite-layup laminate releases/);
  assert.match(readme, /hot-wire foam cores\/patterns/);
  assert.match(readme, /metal PBF printer/);
  assert.match(readme, /binder jet printer/);
  assert.match(readme, /five-axis mill/);
  assert.match(readme, /rotary-indexer mill/);
  assert.match(readme, /five-axis-milled impellers\/undercuts/);
  assert.match(readme, /4th-axis indexed multi-face milling/);
  assert.match(readme, /binder-jet-print/);
  assert.match(readme, /metal PBF-print/);
  assert.match(readme, /laser cutter/);
  assert.match(readme, /sheet-cutting\s+kerf\/fire\/fume checks/);
  assert.match(readme, /method\s+combination preferences/);
  assert.match(readme, /five-axis-milling/);
  assert.match(readme, /rotary-indexed milling/);
  assert.match(readme, /additive-print\+milling/);
  assert.match(readme, /additive material\/color\/tool-change/);
  assert.match(readme, /slicer job\s+sheets/);
  assert.match(
    readme,
    /additive slicer profile\/support\/\s+orientation\/first-layer, mesh unit\/scale\/topology\/wall-thickness evidence,\s+high-speed kinematic evidence/,
  );
  assert.match(
    readme,
    /multi-material FDM material\/color map, slot, filament-lot,\s+support-interface, purge\/wipe tower, tool-change script, runout-sensor, and resume-state evidence/,
  );
  assert.match(readme, /slicer profile\/support\/orientation\/first-layer evidence/);
  assert.match(readme, /mesh unit\/scale\/topology\/wall-thickness evidence/);
  assert.match(readme, /additive thin-wall geometry/);
  assert.match(
    readme,
    /resin\s+vat-capacity\/refill evidence, resin-handling\/postprocess evidence/,
  );
  assert.match(
    readme,
    /pellet\/FGF pellet-lot\/drying\/moisture\/hopper\/purge evidence from generated `DRY_PELLETS`\/`PURGE_EXTRUDER` records and bead\/screw\/melt\/cooling\/gantry-clearance\/warpage\/trim-allowance evidence from generated `PRINT_BEAD_PATH`\/`MONITOR` records/,
  );
  assert.match(
    readme,
    /robotic additive robot frame\/TCP\/reach\/collision\/interlock\/external-axis evidence from generated `LOAD_ROBOT_PATH`\/`DRY_RUN_ROBOT` records and feedstock\/nozzle\/purge\/bead\/flow\/cooling\/cure\/dimensional-scan evidence from generated `PURGE_ROBOTIC_EXTRUDER`\/`DEPOSIT_ROBOTIC_BEAD_PATH`\/`MONITOR` records/,
  );
  assert.match(
    readme,
    /sheet-lamination sheet\/foil stock\/stack-order\/surface-prep evidence from generated `LOAD_SHEET_STACK` records and registration\/trim\/bond\/consolidation\/delamination\/dimensional-release evidence from generated `REGISTER_LAYER_STACK`\/`CUT_OR_TRIM_LAYERS`\/`BOND_OR_CONSOLIDATE_LAYERS`\/`INSPECT_LAMINATION` records/,
  );
  assert.match(
    readme,
    /material-jetting cartridge\/channel-map\/printhead\/tray plus generated `PACK_TRAY`\/`JET_MATERIALS` evidence and support-removal\/UV\/color\/material inspection evidence from generated `REMOVE_SUPPORT`\/`UV_CURE_INLINE` records/,
  );
  assert.match(
    readme,
    /paste\/clay rheology\/slump\/deairing\/nozzle\/pressure evidence from generated `CONDITION_PASTE`\/`PURGE_SYRINGE_OR_AUGER` records and drying\/humidity\/shrinkage\/green-part\/firing evidence from generated `PRINT_PASTE_PATH`\/`DRY_GREEN_PART` records/,
  );
  assert.match(
    readme,
    /bound-metal filament profile\/nozzle\/dry-storage\/shrinkage evidence from generated `LOAD_BOUND_METAL_FILAMENT`\/`SLICE_BOUND_METAL_FFF`\/`PRINT_GREEN_PART` records and debind\/sinter\/furnace\/atmosphere\/density inspection evidence from generated `DEBIND_GREEN_PART`\/`SINTER_PART` records/,
  );
  assert.match(
    readme,
    /DED\/WAAM feedstock\/substrate\/bead-path\/standoff plus generated `PREP_SUBSTRATE`\/`PLAN_BEADS` evidence and laser\/arc\/shielding\/interpass\/NDE\/coupon evidence from generated `START_DEPOSITION`\/`MONITOR_MELT_POOL`\/`INSPECT_DEPOSIT` records/,
  );
  assert.match(
    readme,
    /composite-fiber layup\/orientation\/load-case evidence from generated `FIBER_LAYUP` records and spool\/cutter\/matrix\/coupon\/continuity evidence from generated `FIBER_CUT_ANCHOR`\/`PRINT_COMPOSITE`\/inspection records/,
  );
  assert.match(
    readme,
    /powder-bed build profile\/powder lot\/nesting evidence from generated `NEST`\/`PRINT` records, powder-handling\/cooldown-depowder evidence from generated `DEPOWDER`\/cooldown records/,
  );
  assert.match(
    readme,
    /composite layup mold\/mandrel\/release-system\/ply-schedule\/resin-prepreg-core-lot evidence from generated `PREPARE_LAYUP_TOOL`\/`LAYUP_PLIES` records and vacuum-bag\/leak-down\/cure\/demold-trim-inspection evidence from generated `VACUUM_BAG_AND_LEAK_TEST`\/`CURE_LAMINATE`\/`DEMOLD_TRIM_INSPECT` records/,
  );
  assert.match(
    readme,
    /hot-wire foam blank\/density\/thickness\/support evidence from generated `FOAM_BLANK_SETUP` records and wire-heat\/tension\/kerf\/feed\/taper\/cut\/release evidence from generated `WIRE_HEAT_TENSION_CHECK`\/`KERF_COUPON`\/`HOT_WIRE_CUT` records/,
  );
  assert.match(
    readme,
    /press-brake flat-blank\/tooling\/bend-sequence evidence from generated `LOAD_FLAT_BLANK`\/`SET_BRAKE_TOOLING`\/`RUN_BEND_SEQUENCE` records and formed-part angle\/flange\/radius\/flatness release evidence from generated `INSPECT_FORMED_PART` records/,
  );
  assert.match(
    readme,
    /gear\/spline blank-datum\/tool\/module-or-DP\/index-ratio\/tooth\/deburr\/inspection evidence from generated `LOAD_GEAR_BLANK`\/`SET_GEAR_TOOL`\/`CUT_GEAR_TEETH`\/`DEBURR_PROFILE`\/`INSPECT_GEAR` records/,
  );
  assert.match(readme, /powder-bed build profile\/powder lot\/nesting evidence/);
  assert.match(
    readme,
    /metal powder-bed fusion alloy-lot\/oxygen\/recoater\/stress-relief\/plate-removal evidence from generated `BUILD_ORIENT`\/`INERT_GAS_PURGE`\/`RECOATER_CLEARANCE_CHECK`\/`PRINT_METAL_PBF`\/`STRESS_RELIEF`\/`PLATE_REMOVAL` records/,
  );
  assert.match(
    readme,
    /binder-jet binder-lot\/saturation\/printhead\/green-strength plus generated `BINDER_JET_PRINT` evidence and cure\/debind\/sinter\/infiltration\/shrink-compensation evidence from generated `CURE_GREEN_PART`\/`SINTER_OR_INFILTRATE` records/,
  );
  assert.match(readme, /powder-bed recoater clearance\/thermal spacing\/cooldown evidence/);
  assert.match(readme, /resin exposure\/profile\/layer\/support evidence/);
  assert.match(
    readme,
    /resin\s+vat-capacity\/refill evidence/,
  );
  assert.match(readme, /subtractive text setup\/process evidence/);
  assert.match(readme, /workholding\/datum\/tool-length/);
  assert.match(readme, /spindle\/feed\/coolant\/kerf\/pierce\/cut-chart/);
  assert.match(readme, /Submitted `existingInstructions` are analyzed beside generated drafts/);
  assert.match(readme, /resolved machine profile material lists/);
  assert.match(readme, /`improvements` and `improvedPrograms` review drafts/);
  assert.match(readme, /`patchManifest`/);
  assert.match(readme, /line-level repair\s+operations/);
  assert.match(readme, /`insert-before-line`/);
  assert.match(readme, /`apply-instruction-patch-\*` policy actions/);
  assert.match(readme, /`instruction-patch:\*`\s+learning observations/);
  assert.match(readme, /same request body as\s+`POST \/fabrication\/instructions\/analyze`/);
  assert.match(readme, /dd\.fabrication\.instruction-validation\.v1/);
  assert.match(readme, /`validation\.failureBoundaries`/);
  assert.match(readme, /validation and release-blocking\s+contract/);
  assert.match(readme, /`POST \/instructions\/validation\/result`/);
  assert.match(readme, /`POST \/fabrication\/instructions\/validation\/result`/);
  assert.match(readme, /dd\.fabrication\.instruction-validation-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-validation-learning-outcome-draft\.v1/);
  assert.match(readme, /`instructionValidationResult`/);
  assert.match(readme, /`priorityDispositions` array/);
  assert.match(readme, /`instructionIntentMap\.reviewPriorities`/);
  assert.match(readme, /`validationResultJobId`/);
  assert.match(readme, /`instruction-validation-findings`/);
  assert.match(readme, /`instruction-validation-priority-dispositions`/);
  assert.match(readme, /`instruction-validation-learning-observations`/);
  assert.match(readme, /`instruction-validation-priority:<priority>:<disposition>`/);
  assert.match(readme, /POST \/fabrication\/learning\/outcomes/);
  assert.match(readme, /human\/split\/combine boundaries/);
  assert.match(readme, /dd\.fabrication\.instruction-improvement-review\.v1/);
  assert.match(readme, /dd\.fabrication\.instruction-improvement-learning-outcome-draft\.v1/);
  assert.match(readme, /changed program counts, patch operation counts/);
  assert.match(readme, /`improvedPrograms\.patchManifest`/);
  assert.match(readme, /`improvedPrograms\.patchManifest` includes a `reviewSummary`/);
  assert.match(readme, /modal\/controller-state, process-evidence,\s+split-combine\/interface, human-review, or general-review/);
  assert.match(readme, /`linePatchCount`, `humanReviewCount`, `machineCodePatch`, `releasePosture`/);
  assert.match(readme, /patch-action, reward, and submit-route hints/);
  assert.match(readme, /`POST \/fabrication\/learning\/outcomes`/);
  assert.match(readme, /machine-ready release/);
  assert.match(readme, /nonzero axis counts/);
  assert.match(readme, /positioning-mode reset state/);
  assert.match(readme, /G10 fixture\/work-offset table write review state/);
  assert.match(readme, /units-mode change\/conversion review state/);
  assert.match(readme, /coordinate transform rotation\/scaling\/mirroring review and cancel state/);
  assert.match(readme, /G92 work-coordinate offset review and cancel state/);
  assert.match(readme, /inverse-time feed review and G94 cancel state/);
  assert.match(readme, /G43\.4\/G234 tool-center-point review and G49 cancel state/);
  assert.match(readme, /dwell-duration state/);
  assert.match(
    readme,
    /unreviewed `G51` scaling\/mirroring or `G68` coordinate rotation and missing `G50\.1`\/`G69` transform cancellation/,
  );
  assert.match(
    readme,
    /`G92` work-coordinate offsets before motion or program end without temporary-offset review and `G92\.1`\/`G92\.2` cancellation/,
  );
  assert.match(
    readme,
    /`G10 L2`\/`G10 L20` fixture\/work-offset table writes without controller offset-table backup or review evidence/,
  );
  assert.match(
    readme,
    /late or mid-program `G20`\/`G21` unit-mode changes after motion without conversion review/,
  );
  assert.match(
    readme,
    /`G4`\/`G04` dwell commands without positive `P`\/`S`\/`X`\/`U` duration or operator-timed dwell review/,
  );
  assert.match(
    readme,
    /CNC inverse-time `G93` feed motion without timing review or program end before `G94` cancel/,
  );
  assert.match(
    readme,
    /`G43\.4`\/`G234` tool-center-point mode before rotary\/linear motion or program end without TCP kinematic review and `G49` cancellation/,
  );
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
  assert.match(readme, /mid-print homing\/resume-position state/);
  assert.match(readme, /additive inch-units\/slicer conversion state/);
  assert.match(readme, /printer coordinate\/home-offset state/);
  assert.match(readme, /post-mode-switch extrusion reset state/);
  assert.match(readme, /negative-Z extrusion\/Z-offset probe state/);
  assert.match(readme, /bed-leveling\/mesh restore state/);
  assert.match(readme, /material-capacity\/runout evidence/);
  assert.match(readme, /extrusion calibration\/flow\/pressure-advance evidence/);
  assert.match(readme, /volumetric-extrusion\/M200 state/);
  assert.match(readme, /firmware retraction\/recover settings evidence/);
  assert.match(readme, /printer G2\/G3 arc-support evidence/);
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
    /printer\s+async-nozzle-wait state, async-bed-target re-wait state, nozzle-cooldown\/\s+reheat state, bed-cooldown\/re-wait state, stepper-idle\/re-home state,\s+mid-print homing\/resume-position state, additive inch-units\/slicer conversion state,\s+printer coordinate\/home-offset state,\s+extrusion-mode\/reset state, post-mode-switch extrusion reset state,\s+negative-Z extrusion\/Z-offset probe state, bed-leveling\/mesh restore state,\s+filament lot\/dry-storage\s+conditioning evidence, material-capacity\/runout evidence,\s+extrusion calibration\/flow\/pressure-advance evidence,\s+volumetric-extrusion\/M200 state,\s+firmware retraction\/recover settings evidence,\s+printer G2\/G3 arc-support evidence,\s+high-speed input-shaper\/acceleration\/volumetric-flow evidence,\s+chamber\/enclosure\/thermal-soak evidence for warp-prone filament,\s+bed-adhesion, first-layer, fan-timing/,
  );
  assert.match(
    readme,
    /`M200` volumetric extrusion before filament-diameter\/slicer volumetric E-unit evidence/,
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
    /after mid-print `G28` homing without safe-park,\s+Z-hop, or resume-position evidence/,
  );
  assert.match(readme, /after `G20` inch-mode selection without slicer\/printer\s+unit-conversion evidence/);
  assert.match(
    readme,
    /after `M206`\/`G92 X\/Y\/Z` printer coordinate\/home\s+offsets without offset-probe or dry-run evidence/,
  );
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
    /positive extrusion after\s+`M420 S0` or bed-leveling\/mesh-compensation disable without `M420 S1`, `G29`, or\s+equivalent bed-mesh\/Z-offset verification/,
  );
  assert.match(
    readme,
    /missing\s+`M82`\/`M83`\s+extrusion\s+mode and `G92 E0` reset state before\s+priming/,
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
    /printer `G2`\/`G3` arcs without firmware\/slicer arc-support evidence/,
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
  assert.match(readme, /resin layer\/exposure manifest image-hash\/checksum and peel\/lift\/recoat evidence/);
  assert.match(readme, /including generated `EXPOSE`\/`PEEL` image-stack records/);
  assert.match(readme, /missing resin layer\/exposure manifest image hash\/checksum or peel\/lift\/recoat evidence/);
  assert.match(readme, /missing resin vat-volume\/level\/refill evidence for large resin jobs/);
  assert.match(readme, /missing resin postprocess evidence/);
  assert.match(readme, /missing paste\/clay rheology\/slump\/deairing\/nozzle\/pressure evidence/);
  assert.match(readme, /missing paste\/clay drying\/humidity\/shrinkage\/green-part\/firing evidence/);
  assert.match(readme, /missing bound-metal filament lot\/profile\/hardened-nozzle\/dry-storage\/shrinkage-scale evidence/);
  assert.match(readme, /missing bound-metal debind\/brown-part\/sinter-furnace\/atmosphere\/shrinkage-coupon\/density evidence/);
  assert.match(readme, /missing material-jetting cartridge\/material-channel\/printhead\/tray evidence/);
  assert.match(readme, /missing material-jetting support-removal\/UV\/color\/material inspection evidence/);
  assert.match(readme, /missing DED\/WAAM feedstock\/substrate\/bead-path\/standoff\/machining-allowance evidence/);
  assert.match(readme, /missing DED\/WAAM energy\/shielding\/melt-pool\/interpass\/NDE\/coupon evidence/);
  assert.match(readme, /missing composite-fiber layup\/orientation\/load-case evidence/);
  assert.match(
    readme,
    /missing composite-fiber spool\/cutter\/matrix\/coupon\/continuity inspection evidence/,
  );
  assert.match(readme, /missing composite layup mold\/mandrel\/release-film\/ply-schedule\/resin-prepreg-core-lot\/out-time evidence/);
  assert.match(readme, /missing composite layup vacuum-bag\/leak-down\/debulk\/cure-trace\/demold\/trim-drill\/coupon\/NDI\/dimensional-release evidence/);
  assert.match(readme, /missing hot-wire foam setup evidence/);
  assert.match(readme, /missing hot-wire foam process evidence/);
  assert.match(
    readme,
    /powder-bed build profile\/powder lot\/nesting evidence/,
  );
  assert.match(
    readme,
    /cooldown\/depowder\/recovery controls or missing powder-bed handling evidence/,
  );
  assert.match(readme, /missing powder-bed build\/profile evidence/);
  assert.match(readme, /missing powder-bed handling evidence/);
  assert.match(
    readme,
    /missing\s+metal-PBF alloy-lot\/oxygen\/recoater\/stress-relief\/plate-removal evidence/,
  );
  assert.match(readme, /powder-bed recoater clearance\/thermal spacing\/cooldown evidence/);
  assert.match(readme, /missing binder-jet binder\/saturation\/printhead\/green-strength evidence/);
  assert.match(readme, /missing binder-jet cure\/debind\/sinter\/infiltration\/shrink-compensation evidence/);
  assert.match(readme, /subtractive text setup\/process evidence/);
  assert.match(readme, /missing mill-turn live-tooling C-axis\/Y-axis\/polar-interpolation evidence/);
  assert.match(
    readme,
    /missing mill-turn subspindle pickup\/clamp\/sync\/pull-force\/transfer-clearance evidence/,
  );
  assert.match(readme, /assembly fit\/metrology\/datum\/torque\/cure evidence/);
  assert.match(
    readme,
    /precision tolerance\/surface-finish metrology evidence, precision-grinding wheel dress\/workholding evidence from generated `DRESS_WHEEL`\/`SETUP_WORKHOLDING` records and spark-out\/final-metrology release evidence from generated `SPARK_OUT`\/`INSPECT_GRIND` records, and CMM\/vision calibration\/datum\/feature evidence from generated `CALIBRATE_PROBE`\/`ALIGN_DATUMS`\/`MEASURE_FEATURE` records plus measured-values\/pass-fail\/nonconformance release evidence from generated `REPORT_INSPECTION` records/,
  );
  assert.match(
    readme,
    /unattended\/batch monitoring evidence plus separate restart\/recovery\/operator-check-in\/batch-inspection evidence/,
  );
  assert.match(
    readme,
    /generated `KERF_TEST`\/`PIERCE`\/`VECTOR_CUT`\/`WATERJET_CUT`\/`PLASMA_CUT` records and generated `ELECTRODE_VERIFY`\/`DIELECTRIC_FLUSH_TEST`\/`ROUGH_BURN`\/`DEPTH_CHECK`\/`ORBIT_FINISH` records/,
  );
  assert.match(
    readme,
    /thermal postprocess batch\/fixture\/setter\/spacing evidence from generated `LOAD_THERMAL_BATCH` records, profile\/ramp\/soak\/atmosphere evidence from generated `RUN_THERMAL_PROFILE` records, cooldown\/quench\/safe-handling evidence from generated `CONTROL_COOLDOWN` records, and distortion\/shrinkage\/hardness-or-cure\/pass-fail release evidence from generated `INSPECT_THERMAL_RELEASE` records/,
  );
  assert.match(
    readme,
    /surface\/chemical finishing protected-surface\/thread\/datum\/cosmetic-face evidence from generated `MASK_FEATURES` records, process\/media-or-chemistry\/dwell\/agitation-or-blast-pressure evidence from generated `RUN_SURFACE_FINISH` records, and thickness\/roughness-or-color\/adhesion\/dimension\/pass-fail release evidence from generated `INSPECT_SURFACE_FINISH` records/,
  );
  assert.match(
    readme,
    /metal-joining joint-design\/edge-prep\/fit-up\/fixture evidence from generated `PREP_JOINTS` records, process\/WPS\/filler-or-solder\/shielding-or-flux evidence from generated `SET_JOINING_PROCESS` records, heat-input\/travel-speed\/interpass\/tack-sequence\/distortion-control evidence from generated `RUN_METAL_JOIN` records, and visual\/fillet-or-penetration\/distortion\/NDE-or-leak-test\/pass-fail release evidence from generated `INSPECT_JOIN` records/,
  );
  assert.match(
    readme,
    /molding\/casting master\/tool-revision\/release-agent\/vent\/parting-line evidence from generated `PREPARE_MOLD` records, material\/mix-ratio\/pot-life\/batch evidence from generated `MIX_CASTING_MATERIAL` records, vacuum\/pressure\/fill-strategy\/temperature evidence from generated `DEGAS_AND_CAST` records, and demold\/flash\/void\/shrinkage\/dimensional-release evidence from generated `DEMOLD_AND_INSPECT` records/,
  );
  assert.match(
    readme,
    /composite layup mold\/mandrel\/release-film\/ply-schedule\/resin-prepreg-core-lot\/out-time\/vacuum-bag\/leak-down\/debulk\/cure-trace\/demold\/trim-drill\/coupon\/NDI\/dimensional-release evidence/,
  );
  assert.match(
    readme,
    /press-brake\/sheet-forming flat-pattern\/bend-allowance\/tooling\/tonnage\/backgauge\/springback\/angle-inspection evidence/,
  );
  assert.match(
    readme,
    /gear-cutting gear-drawing\/tooth-count\/module-or-DP\/pressure-angle\/helix-lead\/cutter-arbor\/index-ratio\/blank-runout\/deburr\/over-pins\/span\/profile\/backlash inspection evidence/,
  );
  assert.match(readme, /indexed setup clamp\/index\/clearance\/re-probe evidence/);
  assert.match(
    readme,
    /assembly-cell kit\/revision\/join-graph and dry-fit\/datum evidence from generated `KIT_PARTS`\/`VERIFY_DATUMS` records, robot-path\/gripper\/collision\/vision evidence from generated `PICK_PLACE` records, and press-fit\/heat-set\/torque\/adhesive-cure plus vision\/pull-or-torque\/go-no-go\/final-metrology release evidence from generated `JOIN`\/`INSPECT_JOIN` records/,
  );
  assert.match(
    readme,
    /part-separation fixture\/hold-down\/cut-path\/kerf evidence from structured `LOAD_SEPARATION_FIXTURE`\/`CUT_PATH` records and tab-release\/deburr\/traceability\/final-inspection evidence from structured `RELEASE_RETAINED_TABS`\/`DEBURR_EDGES`\/`TRACE_PARTS`\/`INSPECT_SEPARATION` records/,
  );
  assert.match(readme, /assembly\s+dry-fit\/metrology\/datum\/torque\/cure controls/);
  assert.match(readme, /missing assembly fit\/metrology evidence/);
  assert.match(readme, /missing assembly-cell robot-path\/gripper\/fixture\/vision\/interlock evidence/);
  assert.match(
    readme,
    /missing assembly-cell press\/heat-set\/torque\/adhesive\/cure\/final-metrology evidence/,
  );
  assert.match(
    readme,
    /missing part-separation cut-path\/fixture\/kerf\/heat\/deburr\/traceability\/final-inspection evidence/,
  );
  assert.match(
    readme,
    /missing part-separation retained-tab release\/deburr\/traceability\/final-inspection evidence/,
  );
  assert.match(readme, /missing precision tolerance\/surface-finish metrology evidence/);
  assert.match(
    readme,
    /missing CMM\/vision inspection probe or vision calibration, datum alignment, uncertainty, measured-values, pass\/fail disposition, nonconformance-routing evidence/,
  );
  assert.match(readme, /missing unattended\/batch monitoring and recovery evidence/);
  assert.match(readme, /missing unattended\/batch restart\/recovery\/operator-check-in evidence/);
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
    /missing plastic-joining polymer-compatibility\/joint-design\/energy-director\/staking-boss\/fixture-nest\/weld-stake-solvent-recipe\/collapse\/melt-flow\/cooling evidence/,
  );
  assert.match(
    readme,
    /missing plastic-joining weld-collapse\/stake-head\/flash\/cracks-crazing\/proof\/leak-or-visual\/dimensional-fit\/first-article release evidence/,
  );
  assert.match(
    readme,
    /missing gear-cutting\/hobbing\/spline-broaching gear-drawing\/tooth-count\/module-or-DP\/pressure-angle\/helix-lead\/cutter-arbor\/index-ratio\/blank-runout\/deburr\/over-pins\/span\/profile\/backlash inspection evidence/,
  );
  assert.match(
    readme,
    /missing indexed setup clamp\/brake\/index-angle\/clearance\/re-probe evidence/,
  );
  assert.match(
    readme,
    /sheet-cutting material\/thickness\/cut-chart\/recipe evidence, generated sheet-cutting setup\/cut-path\/release evidence, pierce\/kerf\/focus\/gas\/fume\/support, retained-tab\/microjoint\/part-release evidence, waterjet pressure\/abrasive-flow, plasma work-clamp evidence, wire EDM start-hole\/thread\/tension\/dielectric\/flushing\/slug-retention\/skim-pass evidence plus profile\/skim-cut setup-order evidence, and sinker EDM electrode\/dielectric\/depth\/wear\/orbit-finish\/recast release-gate evidence/,
  );
  assert.match(
    readme,
    /missing sheet-cutting material\/thickness\/cut-chart recipe evidence/,
  );
  assert.match(
    readme,
    /missing generated sheet-cutting setup\/cut-path\/release evidence/,
  );
  assert.match(
    readme,
    /missing wire EDM start-hole\/threading\/slug-retention\/dielectric\/flushing\/skim-pass evidence/,
  );
  assert.match(
    readme,
    /wire EDM profile\/skim cuts before start-hole, wire-threading, guide\/tension, conductive workholding, or slug-retention setup evidence/,
  );
  assert.match(
    readme,
    /missing sinker EDM electrode\/dielectric\/flushing\/debris-removal\/depth\/orbit-finish\/recast evidence/,
  );
  assert.match(readme, /mill-turn or Swiss center,\s+router, sheet cutter, lathe/);
  assert.match(readme, /routers, mill-turn or Swiss centers, wire EDM/);
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
  assert.match(
    readme,
    /CNC\/subtractive program end before explicit\s+`M5`\/`M05` process stop or `M9`\/`M09` coolant\/support-media shutdown/,
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
  assert.match(
    readme,
    /mill\/router programs ending in `G18`\/`G19` without `G17` plane restoration/,
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
  assert.match(readme, /Instruction-analysis responses also include a `learning` plan/);
  assert.match(readme, /`analysis-learning-plan`/);
  assert.match(readme, /`analysis-pomdp-belief-state`/);
  assert.match(readme, /`analysis-release-probe-plan`/);
  assert.match(readme, /`analysis-neural-training-corpus`/);
  assert.match(readme, /`analysis-mdp-request`/);
  assert.match(readme, /imported CNC, slicer, printer, probing, joining, and\s+text instruction streams/);
  assert.match(readme, /dd\.remote\.fabrication\.requests/);
  assert.match(readme, /dd\.remote\.fabrication\.results/);
  assert.match(readme, /direct instruction-analysis payloads/);
  assert.match(readme, /fabrication\.instructions\.analyze/);
  assert.match(readme, /are published to the fabrication result subject/);
  assert.match(readme, /dd\.fabrication\.machine-catalog\.v1/);
  assert.match(readme, /catalog derived from `default_machines\(\)`/);
  assert.match(readme, /supported default fleet for additive printers/);
  assert.match(readme, /process-class\s+counts, controllers, supported materials, operation tags/);
  assert.match(readme, /accepted instruction languages, planning and instruction-analysis route aliases/);
  assert.match(readme, /`selectionEvidenceMatrix`/);
  assert.match(readme, /FDM or\s+multi-material\/pellet printers/);
  assert.match(readme, /vertical,\s+horizontal, five-axis, or indexed mills/);
  assert.match(readme, /lathes,\s+mill-turn, or Swiss machines/);
  assert.match(readme, /`machineSelection\.candidates`/);
  assert.match(readme, /threading\/feed synchronization/);
  assert.match(readme, /Resin-printer entries advertise `ctb-resin-job`/);
  assert.match(readme, /Lychee\/Chitubox\/Photon\/CTB slice packages/);
  assert.match(readme, /default planning profiles,\s+not certified shop-floor assets/);
  assert.match(readme, /`GET \/printers\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/printers\/catalog`/);
  assert.match(readme, /dd\.fabrication\.printer-catalog\.v1/);
  assert.match(readme, /FDM, multi-material FDM\/toolchanger, pellet\/FGF/);
  assert.match(readme, /material\/feedstock\s+conditioning, slicer or generated job profile/);
  assert.match(readme, /`GET \/cells\/catalog`/);
  assert.match(readme, /`GET \/fabrication\/cells\/catalog`/);
  assert.match(readme, /dd\.fabrication\.cell-catalog\.v1/);
  assert.match(readme, /robotic additive, directed-energy\s+deposition, robotic assembly/);
  assert.match(readme, /fixture\/workholding or\s+end-effector proof/);
  assert.match(readme, /POST \/machines\/select/);
  assert.match(readme, /POST \/fabrication\/machines\/select/);
  assert.match(readme, /dd\.fabrication\.machine-selection\.v1/);
  assert.match(readme, /`machineSelection\.candidates\.status`/);
  assert.match(readme, /Stored\s+artifacts include `machine-selection`,\s+`machine-schedule`/);
  assert.match(readme, /dd\.fabrication\.controller-postprocessor-catalog\.v1/);
  assert.match(readme, /postprocessor discovery catalog derived from the current `default_machines\(\)`/);
  assert.match(readme, /`dialectAssumptionChecklist`/);
  assert.match(readme, /`dry-run-proof:\*`/);
  assert.match(readme, /exact postprocessed output, controller setup sheet, dry-run or simulation/);
  assert.match(readme, /postprocessor is unknown, output is not retained, dry-run or simulation did not\s+pass/);
  assert.match(readme, /reliable postprocessors, manual-review routes, and controller\s+failure boundaries/);
  assert.match(readme, /controller-postprocessor-learning-outcome-draft\.v1/);
  assert.match(readme, /human-intervention,\s+blocker,\s+reward,\s+and submit-route hints/);
  assert.match(readme, /GET \/process\/catalog/);
  assert.match(readme, /GET \/fabrication\/process\/catalog/);
  assert.match(readme, /dd\.fabrication\.process-catalog\.v1/);
  assert.match(readme, /operation-sequencing discovery contract/);
  assert.match(readme, /additive print processes, subtractive machining/);
  assert.match(readme, /`process-plan`/);
  assert.match(readme, /`process-graph`/);
  assert.match(readme, /`hybrid-make-plan`/);
  assert.match(readme, /`interventionMap`/);
  assert.match(readme, /draft operation sequencing contracts/);
  assert.match(readme, /GET \/materials\/catalog/);
  assert.match(readme, /GET \/fabrication\/materials\/catalog/);
  assert.match(readme, /dd\.fabrication\.material-catalog\.v1/);
  assert.match(readme, /material families, family counts/);
  assert.match(readme, /feedstock or stock forms/);
  assert.match(readme, /materialPlan\.routeRequirements/);
  assert.match(readme, /`materialReadinessChecklist`/);
  assert.match(readme, /lot\/certificate\s+traceability/);
  assert.match(readme, /conditioning and shelf-life state/);
  assert.match(readme, /quantity\/scrap\/runout capacity/);
  assert.match(readme, /`runout-risk:\*`/);
  assert.match(readme, /default planning labels, not certified inventory/);
  assert.match(readme, /material-machine-boundary/);
  assert.match(readme, /POST \/materials\/plan/);
  assert.match(readme, /POST \/fabrication\/materials\/plan/);
  assert.match(readme, /dd\.fabrication\.material-planning\.v1/);
  assert.match(readme, /`materialPlan\.routeRequirements\.requiredEvidence`/);
  assert.match(readme, /Stored\s+artifacts include `material-plan`, `machine-selection`, `tooling-plan`/);
  assert.match(readme, /dd\.fabrication\.design-format-catalog\.v1/);
  assert.match(readme, /source\s+systems, ecosystems, categories, category counts/);
  assert.match(readme, /CAD design-conversion NATS request\/result subjects/);
  assert.match(readme, /topology\/scale\/profile review/);
  assert.match(readme, /dd\.fabrication\.design-import-catalog\.v1/);
  assert.match(readme, /GET \/formats\/catalog/);
  assert.match(readme, /GET \/fabrication\/formats\/catalog/);
  assert.match(readme, /dd\.fabrication\.design-import-review\.v1/);
  assert.match(readme, /POST \/design\/import\/result/);
  assert.match(readme, /POST \/fabrication\/design\/import\/result/);
  assert.match(readme, /dd\.fabrication\.design-import-result-review\.v1/);
  assert.match(readme, /POST \/fabrication\/design\/convert\/plan/);
  assert.match(readme, /POST \/fabrication\/design\/convert\/result/);
  assert.match(readme, /MDP\/POMDP\/neural learning surfaces/);
  assert.match(readme, /URI redaction and ambiguous `\.prt`\/`\.asm` policies/);
  assert.match(readme, /neutral export checksums, simulation, and operator or automation signoff/);
  assert.match(readme, /`translatorReadinessChecklist`/);
  assert.match(readme, /`cam-handoff:\*`/);
  assert.match(readme, /dd\.fabrication\.instruction-language-catalog\.v1/);
  assert.match(readme, /`ctb-resin-job`/);
  assert.match(readme, /`photon-resin-job`/);
  assert.match(readme, /`lychee-resin-job`/);
  assert.match(readme, /`chitubox-resin-job`/);
  assert.match(readme, /exposure image stack, peel\/lift\/recoat/);
  assert.match(readme, /language families, family counts, machine classes/);
  assert.match(readme, /analysis route aliases/);
  assert.match(readme, /part-separation,\s+setup, and operator instruction streams/);
  assert.match(readme, /parse or review evidence, simulation or\s+equivalent controller review/);
  assert.match(readme, /dd\.fabrication\.boundary-catalog\.v1/);
  assert.match(readme, /representative detection sources, release evidence requirements/);
  assert.match(readme, /Machine-ready release remains\s+blocked while any cataloged machine-failure/);
  assert.match(readme, /dd\.fabrication\.decomposition-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.decomposition-planning\.v1/);
  assert.match(readme, /dd\.fabrication\.decomposition-result-review\.v1/);
  assert.match(readme, /required child-geometry and per-route evidence/);
  assert.match(readme, /interface-control\s+fit modes, release gates/);
  assert.match(readme, /single-piece, split-route, and recomposed outcomes/);
  assert.match(readme, /dd\.fabrication\.assembly-catalog\.v1/);
  assert.match(readme, /dd\.fabrication\.assembly-planning\.v1/);
  assert.match(readme, /dd\.fabrication\.assembly-planning-result-review\.v1/);
  assert.match(readme, /assembly-planning request\/queue\/result\s+subjects/);
  assert.match(readme, /split\/combine and join boundaries cleared or blocked/);
  assert.match(readme, /child route\s+packages, datum transfer, dry-fit or metrology/);
  assert.match(readme, /Assembly, interface, quality, release, and outcome observations/);
  assert.match(readme, /dd\.fabrication\.release-catalog\.v1/);
  assert.match(readme, /controller\/postprocessor checks, simulation or dry-run evidence/);
  assert.match(readme, /which evidence cleared or blocked printed, milled, turned/);
  assert.match(readme, /POST \/release\/preview/);
  assert.match(readme, /POST \/fabrication\/release\/preview/);
  assert.match(readme, /dd\.fabrication\.release-preview\.v1/);
  assert.match(readme, /retained as compact\s+`release-preview` jobs/);
  assert.match(readme, /machine-release, package, execution, simulation/);
  assert.match(readme, /GET \/workflow\/catalog/);
  assert.match(readme, /GET \/fabrication\/workflow\/catalog/);
  assert.match(readme, /dd\.fabrication\.workflow-catalog\.v1/);
  assert.match(readme, /`workerCatalogRoutes`, `resultReviewCatalogRoutes`,/);
  assert.match(readme, /`learningCatalogRoutes`, `learningOutcomeRoutes`, and/);
  assert.match(readme, /`stageResultHandoffs`/);
  assert.match(readme, /DES\/MDP\/POMDP\/neural learning catalogs, and retained learning outcome\s+memory\/submission routes/);
  assert.match(readme, /Workflow catalog entries are route and evidence contracts/);
  assert.match(readme, /POST \/workflow\/plan/);
  assert.match(readme, /POST \/fabrication\/workflow\/plan/);
  assert.match(readme, /dd\.fabrication\.workflow-planning\.v1/);
  assert.match(readme, /workflowPlan\.stages/);
  assert.match(readme, /`workflowActionQueue`/);
  assert.match(readme, /`workflowPlan\.actionQueue`/);
  assert.match(readme, /generate-or-review-machine-instructions/);
  assert.match(readme, /analyze-remediate-and-simulate-before-release/);
  assert.match(readme, /resolve-split-combine-interface-control/);
  assert.match(readme, /hold-release-and-record-learning-outcome/);
  assert.match(readme, /`workflow-action:validation-remediation-simulation`/);
  assert.match(readme, /`routeHandoffs`/);
  assert.match(readme, /`workflow-plan`, `instruction-intent-map`, and `mdp-request`/);
  assert.match(readme, /plan-level `instructionIntentMap` is retained as the `instruction-intent-map`/);
  assert.match(readme, /submitted existing instructions share the same intent/);
  assert.match(readme, /POST \/release\/result/);
  assert.match(readme, /POST \/fabrication\/release\/result/);
  assert.match(readme, /release-readiness request\/queue\/result subjects/);
  assert.match(readme, /`releasePackagePlan\.requiredArtifacts`/);
  assert.match(readme, /do not publish controller code/);
  assert.match(readme, /machine-release, controller, postprocess, simulation/);
  assert.match(readme, /POST \/execution\/plan/);
  assert.match(readme, /POST \/fabrication\/execution\/plan/);
  assert.match(readme, /GET \/execution\/preflight\/catalog/);
  assert.match(readme, /GET \/fabrication\/execution\/preflight\/catalog/);
  assert.match(readme, /dd\.fabrication\.execution-preflight-catalog\.v1/);
  assert.match(readme, /program-run\/machine state/);
  assert.match(readme, /stop-point\/human-intervention\/automation state/);
  assert.match(readme, /monitoring\/recovery\/release state/);
  assert.match(readme, /dd\.fabrication\.execution-planning\.v1/);
  assert.match(readme, /`executionPlan\.programRuns`/);
  assert.match(readme, /`operatorInterventionPlan\.evidenceGates`/);
  assert.match(readme, /`machineSchedule\.dependencyHolds`/);
  assert.match(
    readme,
    /add automation, split jobs,\s+regenerate instructions, or keep human checkpoints/,
  );
  assert.match(readme, /GET \/strategy\/catalog/);
  assert.match(readme, /GET \/fabrication\/strategy\/catalog/);
  assert.match(readme, /dd\.fabrication\.strategy-catalog\.v1/);
  assert.match(readme, /advisory hybrid route,\s+learned preference, MDP\/POMDP policy/);
  assert.match(readme, /strategyCandidates\.score/);
  assert.match(readme, /mdp-request` strategy\s+candidates/);
  assert.match(readme, /not certified manufacturing strategy\s+approval/);
  assert.match(readme, /GET \/hybrid\/catalog/);
  assert.match(readme, /GET \/fabrication\/hybrid\/catalog/);
  assert.match(readme, /dd\.fabrication\.hybrid-catalog\.v1/);
  assert.match(readme, /split\/combine discovery view/);
  assert.match(readme, /one-piece, split-route, recomposed, or\s+human-intervention paths/);
  assert.match(readme, /GET \/methods\/catalog/);
  assert.match(readme, /GET \/fabrication\/methods\/catalog/);
  assert.match(readme, /dd\.fabrication\.manufacturing-method-catalog\.v1/);
  assert.match(readme, /additive printing, subtractive\s+milling\/routing/);
  assert.match(readme, /hybrid split\/combine assembly/);
  assert.match(readme, /strategyCandidates\.methods/);
  assert.match(readme, /not certified live machine availability/);
  assert.match(readme, /POST \/strategy\/recommend/);
  assert.match(readme, /POST \/fabrication\/strategy\/recommend/);
  assert.match(readme, /dd\.fabrication\.strategy-recommendation\.v1/);
  assert.match(readme, /apply the current bounded learning-policy memory/);
  assert.match(readme, /top scored candidate/);
  assert.match(readme, /`learningOutcomeQuality`/);
  assert.match(readme, /`policySummary\.successRate`/);
  assert.match(readme, /`policySummary\.failureRate`/);
  assert.match(readme, /review-learned-route-quality-before-release/);
  assert.match(readme, /do not retain full plan jobs/);
  assert.match(readme, /POST \/strategy\/result/);
  assert.match(readme, /POST \/fabrication\/strategy\/result/);
  assert.match(readme, /dd\.fabrication\.strategy-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.strategy-learning-outcome-draft\.v1/);
  assert.match(readme, /strategy-route-reviews/);
  assert.match(readme, /strategy-learning-observations/);
  assert.match(readme, /method, machine-kind, route, split\/combine/);
  assert.match(readme, /GET \/calibration\/catalog/);
  assert.match(readme, /GET \/fabrication\/calibration\/catalog/);
  assert.match(readme, /GET \/postprocess\/catalog/);
  assert.match(readme, /GET \/fabrication\/postprocess\/catalog/);
  assert.match(readme, /dd\.fabrication\.postprocess-catalog\.v1/);
  assert.match(readme, /finishing, traveler, controller-output/);
  assert.match(readme, /FDM support removal,\s+resin wash\/cure, powder-bed cooldown and depowdering/);
  assert.match(readme, /postprocessPlan\.requiredArtifacts/);
  assert.match(readme, /robot frame\/TCP and collision records/);
  assert.match(readme, /robotic extruder\s+feedstock and purge records/);
  assert.match(readme, /robotic bead coupon and flow records/);
  assert.match(readme, /robotic cell\s+interlock records/);
  assert.match(readme, /sheet-lamination stock\/stack records/);
  assert.match(readme, /registration\/trim\s+records/);
  assert.match(readme, /bond\/consolidation records/);
  assert.match(readme, /delamination\/dimensional-release records/);
  assert.match(readme, /surface media\/chemistry and SDS\s+records/);
  assert.match(readme, /masking\/plugging\/protected-feature records/);
  assert.match(readme, /ventilation\/PPE\/waste\s+records/);
  assert.match(readme, /finish thickness\/adhesion\/inspection records/);
  assert.match(readme, /welding procedure and\s+qualification records/);
  assert.match(readme, /joint fit-up\/fixture\/clamp records/);
  assert.match(readme, /filler\/flux\/gas and\s+fume-control records/);
  assert.match(readme, /heat-input\/interpass\/distortion records/);
  assert.match(readme, /weld-inspection\/NDE\/repair records/);
  assert.match(readme, /mold master\/tooling\/release records/);
  assert.match(readme, /mix-ratio\/pot-life\/batch records/);
  assert.match(readme, /degas\/vacuum\/pressure\/cure records/);
  assert.match(readme, /demold\/shrinkage\/void\/dimensional records/);
  assert.match(readme, /flat-pattern and\s+bend-allowance records/);
  assert.match(readme, /press-brake tooling and tonnage records/);
  assert.match(readme, /backgauge\/bend-sequence\/angle-inspection records/);
  assert.match(readme, /formed-part dimensional\s+release records/);
  assert.match(readme, /foam blank density\/template records/);
  assert.match(readme, /wire\s+temperature\/tension\/kerf records/);
  assert.match(readme, /fume\/fire-watch\/PPE records/);
  assert.match(readme, /foam core\s+surface\/taper\/dimensional records/);
  assert.match(readme, /gear drawing\/blank datum records/);
  assert.match(readme, /gear\s+cutter\/arbor\/indexing records/);
  assert.match(readme, /gear deburr\/burr-control records/);
  assert.match(readme, /gear\s+over-pins\/span\/profile inspection records/);
  assert.match(readme, /Swiss guide-bushing\/bar-feed\s+records/);
  assert.match(readme, /gang-tool\/live-tool clearance records/);
  assert.match(readme, /pickoff\/cutoff\/ejection records/);
  assert.match(readme, /first-article runout\/remnant records/);
  assert.match(readme, /machine-ready release remains blocked while postprocess\s+targets/);
  assert.match(readme, /learn when to add finishing operations, split parts, combine assemblies/);
  assert.match(readme, /POST \/postprocess\/plan/);
  assert.match(readme, /POST \/fabrication\/postprocess\/plan/);
  assert.match(readme, /dd\.fabrication\.postprocess-planning\.v1/);
  assert.match(readme, /postprocessPlan\.controllerTargets\.gates/);
  assert.match(readme, /postprocessPlan\.controllerTargets\.postprocessor/);
  assert.match(readme, /change\s+postprocessors, add finishing operations, split parts, combine assemblies/);
  assert.match(readme, /POST \/postprocess\/result/);
  assert.match(readme, /POST \/fabrication\/postprocess\/result/);
  assert.match(readme, /dd\.fabrication\.postprocess-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.postprocess-learning-outcome-draft\.v1/);
  assert.match(readme, /unresolved dry-run or simulation gates/);
  assert.match(readme, /incomplete\s+traveler steps/);
  assert.match(readme, /missing operator\/automation signoff/);
  assert.match(readme, /target\s+status, postprocessor, gate, traveler-step/);
  assert.match(readme, /`postprocess-target-results`/);
  assert.match(readme, /`postprocess-traveler-steps`/);
  assert.match(readme, /`postprocess-signoffs`/);
  assert.match(readme, /`postprocess-learning-observations`/);
  assert.match(readme, /dd\.fabrication\.learning-capability-catalog\.v1/);
  assert.match(readme, /canonical\s+MDP\/POMDP\/DES Studio schema names/);
  assert.match(readme, /machine-ready release stays blocked while validation\s+findings/);
  assert.match(readme, /dd\.fabrication\.learning-reward-catalog\.v1/);
  assert.match(readme, /Reward terms are retained so DES\/MDP\/POMDP\/neural workers/);
  assert.match(readme, /dd\.fabrication\.learning-model-catalog\.v1/);
  assert.match(readme, /retained model-artifact catalog/);
  assert.match(readme, /queue surrogates, and neural action scores/);
  assert.match(readme, /dd\.fabrication\.learning-replay-catalog\.v1/);
  assert.match(readme, /policy-promotion replay contract/);
  assert.match(readme, /failure-boundary and human-intervention regression/);
  assert.match(readme, /dd\.fabrication\.learning-belief-catalog\.v1/);
  assert.match(readme, /POMDP belief and release-probe\s+contract/);
  assert.match(readme, /hidden\s+machine-failure, human-intervention, split\/combine/);
  assert.match(readme, /dd\.fabrication\.learning-optimizer-catalog\.v1/);
  assert.match(readme, /optimizer discovery contract/);
  assert.match(readme, /dd\.fabrication\.learning-model-result-review\.v1/);
  assert.match(readme, /metric-failure counts, blocker\s+hints, model-card compatibility status, and artifact hints/);
  assert.match(readme, /replay verification, metric\s+review, neural model-card compatibility,\s+and cleared promotion blockers/);
  assert.match(readme, /dd\.fabrication\.learning-optimizer-result-review\.v1/);
  assert.match(readme, /dd\.fabrication\.learning-optimizer-learning-outcome-draft\.v1/);
  assert.match(readme, /candidate\s+scores remain advisory and keep `machineReady=false`/);
  assert.match(readme, /dd\.fabrication\.learning-outcome-memory\.v1/);
  assert.match(readme, /retained compact\/rich learning records/);
  assert.match(readme, /`policyImpactPreview` entries/);
  assert.match(
    readme,
    /Compact learning outcomes fan\s+out `fabrication\.learning\.outcome\.result`/,
  );
  assert.match(readme, /FABRICATION_MDP_AUTOPUBLISH=true/);
  assert.match(readme, /default local port is `8113`/);

  assert.match(subjectSchema, /dd\.remote\.fabrication\.requests/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.assembly\.planning\.requests/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.assembly\.planning\.results/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.execution\.telemetry\.requests/);
  assert.match(subjectSchema, /dd\.remote\.fabrication\.execution\.telemetry\.results/);
  assert.match(subjectSchema, /"queueGroup": "dd-fabrication-server"/);
  assert.match(subjectSchema, /"queueGroup": "dd-fabrication-assembly-planners"/);
  assert.match(subjectSchema, /"queueGroup": "dd-fabrication-execution-reviewers"/);

  assert.match(docs, /"path": "\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/plan"/);
  assert.match(docs, /"path": "\/workflow\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/workflow\/catalog"/);
  assert.match(docs, /"path": "\/workflow\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/workflow\/plan"/);
  assert.match(docs, /"path": "\/capabilities"/);
  assert.match(docs, /"path": "\/fabrication\/capabilities"/);
  assert.match(docs, /"path": "\/machines\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/machines\/catalog"/);
  assert.match(docs, /"path": "\/machines\/select"/);
  assert.match(docs, /"path": "\/fabrication\/machines\/select"/);
  assert.match(docs, /"path": "\/controllers\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/controllers\/catalog"/);
  assert.match(docs, /"path": "\/controllers\/result"/);
  assert.match(docs, /"path": "\/fabrication\/controllers\/result"/);
  assert.match(docs, /"path": "\/materials\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/materials\/catalog"/);
  assert.match(docs, /"path": "\/materials\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/materials\/plan"/);
  assert.match(docs, /"path": "\/design\/formats"/);
  assert.match(docs, /"path": "\/fabrication\/design\/formats"/);
  assert.match(docs, /"path": "\/slicers\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/slicers\/catalog"/);
  assert.match(docs, /"path": "\/slicers\/result"/);
  assert.match(docs, /"path": "\/fabrication\/slicers\/result"/);
  assert.match(docs, /"path": "\/mesh-repair\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/mesh-repair\/catalog"/);
  assert.match(docs, /"path": "\/mesh-repair\/result"/);
  assert.match(docs, /"path": "\/fabrication\/mesh-repair\/result"/);
  assert.match(docs, /"path": "\/turning\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/turning\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/formats\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/formats\/catalog"/);
  assert.match(docs, /"path": "\/design\/import\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/design\/import\/catalog"/);
  assert.match(docs, /"path": "\/design\/import\/review"/);
  assert.match(docs, /"path": "\/fabrication\/design\/import\/review"/);
  assert.match(docs, /"path": "\/design\/import\/result"/);
  assert.match(docs, /"path": "\/fabrication\/design\/import\/result"/);
  assert.match(docs, /"path": "\/design\/convert\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/design\/convert\/plan"/);
  assert.match(docs, /"path": "\/design\/convert\/result"/);
  assert.match(docs, /"path": "\/fabrication\/design\/convert\/result"/);
  assert.match(docs, /"path": "\/design\/synthesis\/result"/);
  assert.match(docs, /"path": "\/fabrication\/design\/synthesis\/result"/);
  assert.match(docs, /"path": "\/design\/generation\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/design\/generation\/catalog"/);
  assert.match(docs, /"path": "\/design\/generate"/);
  assert.match(docs, /"path": "\/fabrication\/design\/generate"/);
  assert.match(docs, /"path": "\/handoff\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/handoff\/catalog"/);
  assert.match(docs, /"path": "\/handoff\/result"/);
  assert.match(docs, /"path": "\/fabrication\/handoff\/result"/);
  assert.match(docs, /"path": "\/workers\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/workers\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/languages"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/languages"/);
  assert.match(docs, /"path": "\/instructions\/import\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/import\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/validation\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/validation\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/generation\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/generation\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/generation\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/generation\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/generate"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/generate"/);
  assert.match(docs, /"path": "\/instructions\/generation\/result"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/generation\/result"/);
  assert.match(docs, /"path": "\/instructions\/review\/result"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/review\/result"/);
  assert.match(docs, /"path": "\/instructions\/validation\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/validation\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/validation\/result"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/validation\/result"/);
  assert.match(docs, /"path": "\/instructions\/import\/review"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/import\/review"/);
  assert.match(docs, /"path": "\/machine-code\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/machine-code\/catalog"/);
  assert.match(docs, /"path": "\/machine-code\/generate"/);
  assert.match(docs, /"path": "\/fabrication\/machine-code\/generate"/);
  assert.match(docs, /"path": "\/machine-code\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/machine-code\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/machine-code\/result"/);
  assert.match(docs, /"path": "\/fabrication\/machine-code\/result"/);
  assert.match(docs, /"path": "\/materials\/result"/);
  assert.match(docs, /"path": "\/fabrication\/materials\/result"/);
  assert.match(docs, /"path": "\/toolpaths\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/toolpaths\/catalog"/);
  assert.match(docs, /"path": "\/toolpaths\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/toolpaths\/plan"/);
  assert.match(docs, /"path": "\/toolpaths\/result"/);
  assert.match(docs, /"path": "\/fabrication\/toolpaths\/result"/);
  assert.match(docs, /"path": "\/schedule\/result"/);
  assert.match(docs, /"path": "\/fabrication\/schedule\/result"/);
  assert.match(docs, /"path": "\/improvements\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/improvements\/catalog"/);
  assert.match(docs, /"path": "\/improvements\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/improvements\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/improve"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/improve"/);
  assert.match(docs, /"path": "\/instructions\/boundaries\/review"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/boundaries\/review"/);
  assert.match(docs, /"path": "\/boundaries\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/boundaries\/catalog"/);
  assert.match(docs, /"path": "\/boundaries\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/boundaries\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/remediation\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/remediation\/catalog"/);
  assert.match(docs, /"path": "\/remediation\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/remediation\/plan"/);
  assert.match(docs, /"path": "\/remediation\/result"/);
  assert.match(docs, /"path": "\/fabrication\/remediation\/result"/);
  assert.match(docs, /"path": "\/decomposition\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/decomposition\/catalog"/);
  assert.match(docs, /"path": "\/decomposition\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/decomposition\/plan"/);
  assert.match(docs, /"path": "\/decomposition\/result"/);
  assert.match(docs, /"path": "\/fabrication\/decomposition\/result"/);
  assert.match(docs, /"path": "\/assembly\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/assembly\/catalog"/);
  assert.match(docs, /"path": "\/assembly\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/assembly\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/assembly\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/assembly\/plan"/);
  assert.match(docs, /"path": "\/assembly\/result"/);
  assert.match(docs, /"path": "\/fabrication\/assembly\/result"/);
  assert.match(docs, /"path": "\/interfaces\/result"/);
  assert.match(docs, /"path": "\/fabrication\/interfaces\/result"/);
  assert.match(docs, /"path": "\/instructions\/import\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/import\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/instructions\/import\/review"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/import\/review"/);
  assert.match(docs, /"path": "\/release\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/release\/catalog"/);
  assert.match(docs, /"path": "\/release\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/release\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/release\/preview"/);
  assert.match(docs, /"path": "\/fabrication\/release\/preview"/);
  assert.match(docs, /"path": "\/release\/result"/);
  assert.match(docs, /"path": "\/fabrication\/release\/result"/);
  assert.match(docs, /"path": "\/execution\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/execution\/plan"/);
  assert.match(docs, /"path": "\/execution\/result"/);
  assert.match(docs, /"path": "\/fabrication\/execution\/result"/);
  assert.match(docs, /"path": "\/strategy\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/strategy\/catalog"/);
  assert.match(docs, /"path": "\/strategy\/recommend"/);
  assert.match(docs, /"path": "\/fabrication\/strategy\/recommend"/);
  assert.match(docs, /"path": "\/strategy\/result"/);
  assert.match(docs, /"path": "\/fabrication\/strategy\/result"/);
  assert.match(docs, /"path": "\/learning\/corpus"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/corpus"/);
  assert.match(docs, /"path": "\/schedule\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/schedule\/catalog"/);
  assert.match(docs, /"path": "\/simulation\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/simulation\/catalog"/);
  assert.match(docs, /"path": "\/simulation\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/simulation\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/simulation\/run"/);
  assert.match(docs, /"path": "\/fabrication\/simulation\/run"/);
  assert.match(docs, /"path": "\/simulation\/result"/);
  assert.match(docs, /"path": "\/fabrication\/simulation\/result"/);
  assert.match(docs, /"path": "\/quality\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/quality\/catalog"/);
  assert.match(docs, /"path": "\/quality\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/quality\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/dispositions\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/dispositions\/catalog"/);
  assert.match(docs, /"path": "\/dispositions\/result"/);
  assert.match(docs, /"path": "\/fabrication\/dispositions\/result"/);
  assert.match(docs, /"path": "\/utilities\/result"/);
  assert.match(docs, /"path": "\/fabrication\/utilities\/result"/);
  assert.match(docs, /"path": "\/availability\/result"/);
  assert.match(docs, /"path": "\/fabrication\/availability\/result"/);
  assert.match(docs, /"path": "\/maintenance\/result"/);
  assert.match(docs, /"path": "\/fabrication\/maintenance\/result"/);
  assert.match(docs, /"path": "\/quality\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/quality\/plan"/);
  assert.match(docs, /"path": "\/quality\/result"/);
  assert.match(docs, /"path": "\/fabrication\/quality\/result"/);
  assert.match(docs, /"path": "\/calibration\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/calibration\/catalog"/);
  assert.match(docs, /"path": "\/calibration\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/calibration\/plan"/);
  assert.match(docs, /"path": "\/calibration\/result"/);
  assert.match(docs, /"path": "\/fabrication\/calibration\/result"/);
  assert.match(docs, /"path": "\/interventions\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/interventions\/catalog"/);
  assert.match(docs, /"path": "\/interventions\/result"/);
  assert.match(docs, /"path": "\/fabrication\/interventions\/result"/);
  assert.match(docs, /"path": "\/setup\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/setup\/catalog"/);
  assert.match(docs, /"path": "\/tooling\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/tooling\/catalog"/);
  assert.match(docs, /"path": "\/tooling\/result"/);
  assert.match(docs, /"path": "\/fabrication\/tooling\/result"/);
  assert.match(docs, /"path": "\/consumables\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/consumables\/catalog"/);
  assert.match(docs, /"path": "\/consumables\/result"/);
  assert.match(docs, /"path": "\/fabrication\/consumables\/result"/);
  assert.match(docs, /"path": "\/workholding\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/workholding\/catalog"/);
  assert.match(docs, /"path": "\/workholding\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/workholding\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/workholding\/result"/);
  assert.match(docs, /"path": "\/fabrication\/workholding\/result"/);
  assert.match(docs, /"path": "\/nesting\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/nesting\/catalog"/);
  assert.match(docs, /"path": "\/nesting\/result"/);
  assert.match(docs, /"path": "\/fabrication\/nesting\/result"/);
  assert.match(docs, /"path": "\/support-strategies\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/support-strategies\/catalog"/);
  assert.match(docs, /"path": "\/support-strategies\/result"/);
  assert.match(docs, /"path": "\/fabrication\/support-strategies\/result"/);
  assert.match(docs, /"path": "\/process-recipes\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/process-recipes\/catalog"/);
  assert.match(docs, /"path": "\/process-recipes\/result"/);
  assert.match(docs, /"path": "\/fabrication\/process-recipes\/result"/);
  assert.match(docs, /"path": "\/kinematics\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/kinematics\/catalog"/);
  assert.match(docs, /"path": "\/kinematics\/result"/);
  assert.match(docs, /"path": "\/fabrication\/kinematics\/result"/);
  assert.match(docs, /"path": "\/tolerances\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/tolerances\/catalog"/);
  assert.match(docs, /"path": "\/tolerances\/result"/);
  assert.match(docs, /"path": "\/fabrication\/tolerances\/result"/);
  assert.match(docs, /"path": "\/process-capabilities\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/process-capabilities\/catalog"/);
  assert.match(docs, /"path": "\/process-capabilities\/result"/);
  assert.match(docs, /"path": "\/fabrication\/process-capabilities\/result"/);
  assert.match(docs, /"path": "\/manufacturability\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/manufacturability\/catalog"/);
  assert.match(docs, /"path": "\/manufacturability\/result"/);
  assert.match(docs, /"path": "\/fabrication\/manufacturability\/result"/);
  assert.match(docs, /"path": "\/failure-modes\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/failure-modes\/catalog"/);
  assert.match(docs, /"path": "\/failure-modes\/result"/);
  assert.match(docs, /"path": "\/fabrication\/failure-modes\/result"/);
  assert.match(docs, /"path": "\/safety\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/safety\/catalog"/);
  assert.match(docs, /"path": "\/safety\/result"/);
  assert.match(docs, /"path": "\/fabrication\/safety\/result"/);
  assert.match(docs, /"path": "\/environment\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/environment\/catalog"/);
  assert.match(docs, /"path": "\/environment\/result"/);
  assert.match(docs, /"path": "\/fabrication\/environment\/result"/);
  assert.match(docs, /"path": "\/provenance\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/provenance\/catalog"/);
  assert.match(docs, /"path": "\/as-built\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/as-built\/catalog"/);
  assert.match(docs, /"path": "\/as-built\/result"/);
  assert.match(docs, /"path": "\/fabrication\/as-built\/result"/);
  assert.match(docs, /"path": "\/setup\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/setup\/plan"/);
  assert.match(docs, /"path": "\/setup\/result"/);
  assert.match(docs, /"path": "\/fabrication\/setup\/result"/);
  assert.match(docs, /"path": "\/monitoring\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/monitoring\/catalog"/);
  assert.match(docs, /"path": "\/monitoring\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/monitoring\/plan"/);
  assert.match(docs, /"path": "\/monitoring\/result"/);
  assert.match(docs, /"path": "\/fabrication\/monitoring\/result"/);
  assert.match(docs, /"path": "\/postprocess\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/postprocess\/catalog"/);
  assert.match(docs, /"path": "\/postprocess\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/postprocess\/plan"/);
  assert.match(docs, /"path": "\/postprocess\/result"/);
  assert.match(docs, /"path": "\/fabrication\/postprocess\/result"/);
  assert.match(docs, /"path": "\/evidence\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/evidence\/catalog"/);
  assert.match(docs, /"path": "\/artifacts\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/artifacts\/catalog"/);
  assert.match(docs, /"path": "\/packages\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/packages\/catalog"/);
  assert.match(docs, /"path": "\/packages\/plan"/);
  assert.match(docs, /"path": "\/fabrication\/packages\/plan"/);
  assert.match(docs, /"path": "\/methods\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/methods\/catalog"/);
  assert.match(docs, /"path": "\/process\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/process\/catalog"/);
  assert.match(docs, /"path": "\/subjects\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/subjects\/catalog"/);
  assert.match(docs, /"path": "\/results\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/results\/catalog"/);
  assert.match(docs, /"path": "\/learning\/capabilities"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/capabilities"/);
  assert.match(docs, /"path": "\/learning\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/preflight\/catalog"/);
  assert.match(docs, /"path": "\/learning\/replay\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/replay\/catalog"/);
  assert.match(docs, /"path": "\/schema"/);
  assert.match(docs, /"path": "\/fabrication\/schema"/);
  assert.match(docs, /"path": "\/examples"/);
  assert.match(docs, /"path": "\/fabrication\/examples"/);
  assert.match(docs, /"path": "\/instructions\/analyze"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/analyze"/);
  assert.match(docs, /"path": "\/instructions\/validate"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/validate"/);
  assert.match(docs, /"path": "\/instructions\/improve"/);
  assert.match(docs, /"path": "\/fabrication\/instructions\/improve"/);
  assert.match(docs, /"path": "\/jobs\/catalog"/);
  assert.match(docs, /"path": "\/fabrication\/jobs\/catalog"/);
  assert.match(docs, /"path": "\/jobs"/);
  assert.match(docs, /"path": "\/fabrication\/jobs"/);
  assert.match(docs, /"path": "\/jobs\/:job_id"/);
  assert.match(docs, /"path": "\/fabrication\/jobs\/:job_id"/);
  assert.match(docs, /"path": "\/jobs\/:job_id\/release-bundle"/);
  assert.match(docs, /"path": "\/fabrication\/jobs\/:job_id\/release-bundle"/);
  assert.match(docs, /"path": "\/jobs\/:job_id\/artifacts\/:artifact_id"/);
  assert.match(docs, /"path": "\/fabrication\/jobs\/:job_id\/artifacts\/:artifact_id"/);
  assert.match(docs, /"path": "\/learning\/policy"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/policy"/);
  assert.match(docs, /"path": "\/learning\/observe"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/observe"/);
  assert.match(docs, /"path": "\/learning\/outcomes"/);
  assert.match(docs, /"path": "\/fabrication\/learning\/outcomes"/);
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
  const grafanaDashboards = await readRepoFile(
    'remote/argocd/observability/grafana.dashboards.configmap.yaml',
  );
  const observabilityReadme = await readRepoFile('remote/argocd/observability/readme.md');
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
  assert.match(
    deployment,
    /FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT: &str = "dd\.remote\.fabrication\.design\.conversion\.requests"/,
  );
  assert.match(
    deployment,
    /FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP: &str = "dd-fabrication-design-converters"/,
  );
  assert.match(
    deployment,
    /FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT: &str = "dd\.remote\.fabrication\.design\.conversion\.results"/,
  );
  assert.match(deployment, /FABRICATION_MDP_OPTIMIZE_SUBJECT[\s\S]*dd\.remote\.mdp\.optimize/);
  assert.match(deployment, /FABRICATION_MDP_AUTOPUBLISH[\s\S]*value:\s*'true'/);
  assert.match(deployment, /RUNTIME_CONFIG_APPLY_URL[\s\S]*dd-fabrication-server\.default\.svc\.cluster\.local:8113/);
  assert.match(deployment, /"path": "\/fabrication\/subtractive\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/subtractive\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/turning\/preflight\/catalog"/);
  assert.match(deployment, /revisionHistoryLimit:\s*3/);
  assert.match(deployment, /topologySpreadConstraints:[\s\S]*topologyKey:\s*kubernetes\.io\/hostname/);
  assert.match(deployment, /podAntiAffinity:[\s\S]*preferredDuringSchedulingIgnoredDuringExecution/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /"path": "\/fabrication\/capabilities"/);
  assert.match(deployment, /"path": "\/fabrication\/schema"/);
  assert.match(deployment, /"path": "\/fabrication\/examples"/);
  assert.match(deployment, /"path": "\/fabrication\/workflow\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/workflow\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/machines\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/printers\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/printers\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/subtractive\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/cleanliness\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/interfaces\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/cnc\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/hybrid\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/cells\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/machines\/select"/);
  assert.match(deployment, /"path": "\/fabrication\/controllers\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/controllers\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/materials\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/materials\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/formats\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/slicers\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/slicers\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/mesh-repair\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/mesh-repair\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/import\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/import\/review"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/import\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/convert\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/convert\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/validate"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/improve"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/boundaries\/review"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/generation\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/generate"/);
  assert.match(deployment, /"path": "\/fabrication\/design\/synthesis\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/handoff\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/handoff\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/subjects\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/workers\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/results\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/validation\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/validation\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/generation\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/generation\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/generate"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/generation\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/review\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/validation\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/machine-code\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/machine-code\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/machine-code\/generate"/);
  assert.match(deployment, /"path": "\/fabrication\/machine-code\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/toolpaths\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/toolpaths\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/toolpaths\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/improvements\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/improvements\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/boundaries\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/boundaries\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/remediation\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/remediation\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/remediation\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/models\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/replay\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/beliefs\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/optimizers\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/models\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/optimizers\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/decomposition\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/decomposition\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/assembly\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/assembly\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/assembly\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/assembly\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/interfaces\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/import\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/instructions\/import\/review"/);
  assert.match(deployment, /"path": "\/fabrication\/release\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/release\/preview"/);
  assert.match(deployment, /"path": "\/fabrication\/release\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/evidence\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/strategy\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/methods\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/process\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/packages\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/packages\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/strategy\/recommend"/);
  assert.match(deployment, /"path": "\/fabrication\/strategy\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/schedule\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/schedule\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/execution\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/execution\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/simulation\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/simulation\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/simulation\/run"/);
  assert.match(deployment, /"path": "\/fabrication\/simulation\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/quality\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/quality\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/dispositions\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/dispositions\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/costing\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/energy\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/energy\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/utilities\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/availability\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/availability\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/maintenance\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/maintenance\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/telemetry\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/quality\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/quality\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/manufacturability\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/calibration\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/calibration\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/calibration\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/interventions\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/interventions\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/setup\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/tooling\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/tooling\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/consumables\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/consumables\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/workholding\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/workholding\/preflight\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/workholding\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/nesting\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/nesting\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/support-strategies\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/support-strategies\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/process-recipes\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/process-recipes\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/kinematics\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/kinematics\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/tolerances\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/tolerances\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/process-capabilities\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/process-capabilities\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/manufacturability\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/manufacturability\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/failure-modes\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/failure-modes\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/safety\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/safety\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/environment\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/environment\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/as-built\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/as-built\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/provenance\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/provenance\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/setup\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/setup\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/monitoring\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/monitoring\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/monitoring\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/postprocess\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/postprocess\/plan"/);
  assert.match(deployment, /"path": "\/fabrication\/postprocess\/result"/);
  assert.match(deployment, /"path": "\/fabrication\/artifacts\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/jobs\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/jobs"/);
  assert.match(deployment, /"path": "\/fabrication\/jobs\/:job_id"/);
  assert.match(deployment, /"path": "\/jobs\/:job_id\/release-bundle"/);
  assert.match(deployment, /"path": "\/fabrication\/jobs\/:job_id\/release-bundle"/);
  assert.match(deployment, /"path": "\/fabrication\/jobs\/:job_id\/artifacts\/:artifact_id"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/engines\/catalog"/);
  assert.match(deployment, /"path": "\/fabrication\/learning\/corpus"/);
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
  assert.match(grafanaDashboards, /fabrication-planner\.json/);
  assert.match(grafanaDashboards, /"title": "Fabrication Planner"/);
  assert.match(grafanaDashboards, /"uid": "dd-fabrication-planner"/);
  assert.match(grafanaDashboards, /"url": "\/grafana\/fabrication"/);
  assert.match(grafanaDashboards, /dd_fabrication_server_plan_requests_total/);
  assert.match(grafanaDashboards, /dd_fabrication_server_failure_boundaries_total/);
  assert.match(grafanaDashboards, /dd_fabrication_server_validation_findings_total/);
  assert.match(grafanaDashboards, /dd_fabrication_server_operator_actions_total/);
  assert.match(grafanaDashboards, /dd_fabrication_server_fixture_release_blockers_total/);
  assert.match(grafanaDashboards, /dd_fabrication_server_split_combine_reviews_total/);
  assert.match(grafanaDashboards, /Failure, Intervention, and Setup Pressure/);
  assert.match(grafanaDashboards, /dd_fabrication_server_current_artifacts/);
  assert.match(grafanaDashboards, /dd_fabrication_server_artifact_requests_total/);
  assert.match(grafanaDashboards, /Result, Learning, and Outcome Fanout/);
  assert.match(grafanaDashboards, /learning outcome submissions/);
  assert.match(grafanaDashboards, /dd_fabrication_server_learning_requests_total/);
  assert.match(grafanaDashboards, /learning events stored/);
  assert.match(grafanaDashboards, /dd_fabrication_server_learning_events_stored_total/);
  assert.match(grafanaDashboards, /costing result reviews/);
  assert.match(grafanaDashboards, /dd_fabrication_server_costing_result_reviews_total/);
  assert.match(grafanaDashboards, /Generated Programs, Artifacts, Learning Events, and Fetches/);
  assert.match(grafanaDashboards, /Catalog Discovery, CAD Intake, Design Export, and Instruction Review/);
  assert.match(grafanaDashboards, /CAD Intake, Design Export, and Instruction Review/);
  assert.match(grafanaDashboards, /Worker Result Review/);
  assert.match(grafanaDashboards, /\/fabrication\/landing/);
  assert.match(grafanaDashboards, /landing page/);
  assert.match(grafanaDashboards, /\/fabrication\/intake\/catalog/);
  assert.match(grafanaDashboards, /intake catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/templates\/catalog/);
  assert.match(grafanaDashboards, /request templates catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/capabilities/);
  assert.match(grafanaDashboards, /capabilities discovery/);
  assert.match(grafanaDashboards, /\/fabrication\/objective\/coverage/);
  assert.match(grafanaDashboards, /objective coverage discovery/);
  assert.match(grafanaDashboards, /\/fabrication\/schema/);
  assert.match(grafanaDashboards, /schema discovery/);
  assert.match(grafanaDashboards, /\/fabrication\/examples/);
  assert.match(grafanaDashboards, /example discovery/);
  assert.match(grafanaDashboards, /\/fabrication\/formats\/catalog/);
  assert.match(grafanaDashboards, /format import catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/results\/catalog/);
  assert.match(grafanaDashboards, /result review catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/machines\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/printers\/catalog/);
  assert.match(grafanaDashboards, /printer catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/printers\/preflight\/catalog/);
  assert.match(grafanaDashboards, /printer preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/subtractive\/catalog/);
  assert.match(grafanaDashboards, /subtractive catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/subtractive\/preflight\/catalog/);
  assert.match(grafanaDashboards, /subtractive preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/turning\/preflight\/catalog/);
  assert.match(grafanaDashboards, /turning preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/cleanliness\/preflight\/catalog/);
  assert.match(grafanaDashboards, /cleanliness preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/interfaces\/preflight\/catalog/);
  assert.match(grafanaDashboards, /interface preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/cnc\/catalog/);
  assert.match(grafanaDashboards, /CNC intake catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/hybrid\/catalog/);
  assert.match(grafanaDashboards, /hybrid catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/cells\/catalog/);
  assert.match(grafanaDashboards, /cell catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/machines\/select/);
  assert.match(grafanaDashboards, /\/fabrication\/controllers\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/controllers\/preflight\/catalog/);
  assert.match(grafanaDashboards, /controller preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/controllers\/result/);
  assert.match(grafanaDashboards, /controller postprocessor result review/);
  assert.match(grafanaDashboards, /\/fabrication\/materials\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/materials\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/materials\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/formats/);
  assert.match(grafanaDashboards, /\/fabrication\/slicers\/catalog/);
  assert.match(grafanaDashboards, /slicer catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/slicers\/result/);
  assert.match(grafanaDashboards, /slicer profile result review/);
  assert.match(grafanaDashboards, /\/fabrication\/mesh-repair\/catalog/);
  assert.match(grafanaDashboards, /mesh repair catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/mesh-repair\/result/);
  assert.match(grafanaDashboards, /mesh repair result review/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/import\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/preflight\/catalog/);
  assert.match(grafanaDashboards, /design preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/import\/review/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/import\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/convert\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/convert\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/generation\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/generate/);
  assert.match(grafanaDashboards, /\/fabrication\/design\/synthesis\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/workflow\/catalog/);
  assert.match(grafanaDashboards, /workflow catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/workflow\/plan/);
  assert.match(grafanaDashboards, /workflow planning/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/generate/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/generation\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/review\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/validation\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/machine-code\/catalog/);
  assert.match(grafanaDashboards, /machine-code catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/machine-code\/preflight\/catalog/);
  assert.match(grafanaDashboards, /machine-code preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/machine-code\/generate/);
  assert.match(grafanaDashboards, /\/fabrication\/machine-code\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/toolpaths\/catalog/);
  assert.match(grafanaDashboards, /toolpath catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/toolpaths\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/toolpaths\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/improvements\/preflight\/catalog/);
  assert.match(grafanaDashboards, /improvement preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/decomposition\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/decomposition\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/assembly\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/assembly\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/interfaces\/result/);
  assert.match(grafanaDashboards, /interface result review/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/import\/preflight\/catalog/);
  assert.match(grafanaDashboards, /instruction import preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/execution\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/execution\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/simulation\/preflight\/catalog/);
  assert.match(grafanaDashboards, /simulation preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/simulation\/run/);
  assert.match(grafanaDashboards, /\/fabrication\/simulation\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/quality\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/quality\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/setup\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/setup\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/monitoring\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/monitoring\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/postprocess\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/postprocess\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/release\/preview/);
  assert.match(grafanaDashboards, /\/fabrication\/release\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/methods\/catalog/);
  assert.match(grafanaDashboards, /method catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/strategy\/recommend/);
  assert.match(grafanaDashboards, /\/fabrication\/strategy\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/artifacts\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/jobs\/catalog/);
  assert.match(grafanaDashboards, /job evidence catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/jobs/);
  assert.match(grafanaDashboards, /job list/);
  assert.match(grafanaDashboards, /job detail/);
  assert.match(grafanaDashboards, /\/release-bundle/);
  assert.match(grafanaDashboards, /artifact detail fetch/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/capabilities/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/policy/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/corpus/);
  assert.match(grafanaDashboards, /learning corpus/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/observe/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/outcomes/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/analyze/);
  assert.match(grafanaDashboards, /\/fabrication\/handoff\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/handoff\/result/);
  assert.match(grafanaDashboards, /handoff result review/);
  assert.match(grafanaDashboards, /\/fabrication\/subjects\/catalog/);
  assert.match(grafanaDashboards, /subject catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/workers\/catalog/);
  assert.match(grafanaDashboards, /worker catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/assembly\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/assembly\/preflight\/catalog/);
  assert.match(grafanaDashboards, /assembly preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/release\/preflight\/catalog/);
  assert.match(grafanaDashboards, /release preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/calibration\/catalog/);
  assert.match(grafanaDashboards, /calibration catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/calibration\/plan/);
  assert.match(grafanaDashboards, /\/fabrication\/calibration\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/generation\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/generation\/preflight\/catalog/);
  assert.match(grafanaDashboards, /instruction generation preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/languages/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/import\/catalog/);
  assert.match(grafanaDashboards, /instruction import catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/validation\/catalog/);
  assert.match(grafanaDashboards, /instruction validation catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/validation\/preflight\/catalog/);
  assert.match(grafanaDashboards, /instruction validation preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/validate/);
  assert.match(grafanaDashboards, /\/fabrication\/improvements\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/boundaries\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/boundaries\/preflight\/catalog/);
  assert.match(grafanaDashboards, /boundary preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/remediation\/catalog/);
  assert.match(grafanaDashboards, /remediation catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/remediation\/plan/);
  assert.match(grafanaDashboards, /remediation planning/);
  assert.match(grafanaDashboards, /\/fabrication\/remediation\/result/);
  assert.match(grafanaDashboards, /remediation result review/);
  assert.match(grafanaDashboards, /\/fabrication\/decomposition\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/release\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/schedule\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/schedule\/result/);
  assert.match(grafanaDashboards, /\/fabrication\/simulation\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/simulation\/preflight\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/quality\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/quality\/preflight\/catalog/);
  assert.match(grafanaDashboards, /quality preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/dispositions\/catalog/);
  assert.match(grafanaDashboards, /disposition catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/dispositions\/result/);
  assert.match(grafanaDashboards, /disposition result review/);
  assert.match(grafanaDashboards, /\/fabrication\/costing\/catalog/);
  assert.match(grafanaDashboards, /costing catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/costing\/result/);
  assert.match(grafanaDashboards, /costing result review/);
  assert.match(grafanaDashboards, /\/fabrication\/utilities\/catalog/);
  assert.match(grafanaDashboards, /utilities catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/energy\/catalog/);
  assert.match(grafanaDashboards, /energy catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/energy\/result/);
  assert.match(grafanaDashboards, /energy result review/);
  assert.match(grafanaDashboards, /\/fabrication\/utilities\/result/);
  assert.match(grafanaDashboards, /utilities result review/);
  assert.match(grafanaDashboards, /\/fabrication\/telemetry\/catalog/);
  assert.match(grafanaDashboards, /telemetry catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/availability\/catalog/);
  assert.match(grafanaDashboards, /availability catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/availability\/result/);
  assert.match(grafanaDashboards, /availability result review/);
  assert.match(grafanaDashboards, /\/fabrication\/maintenance\/catalog/);
  assert.match(grafanaDashboards, /maintenance catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/maintenance\/result/);
  assert.match(grafanaDashboards, /maintenance result review/);
  assert.match(grafanaDashboards, /\/fabrication\/telemetry\/result/);
  assert.match(grafanaDashboards, /telemetry result review/);
  assert.match(grafanaDashboards, /\/fabrication\/manufacturability\/result/);
  assert.match(grafanaDashboards, /manufacturability result review/);
  assert.match(grafanaDashboards, /\/fabrication\/interventions\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/interventions\/result/);
  assert.match(grafanaDashboards, /intervention result review/);
  assert.match(grafanaDashboards, /\/fabrication\/setup\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/tooling\/catalog/);
  assert.match(grafanaDashboards, /tooling catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/tooling\/result/);
  assert.match(grafanaDashboards, /tooling result review/);
  assert.match(grafanaDashboards, /\/fabrication\/consumables\/catalog/);
  assert.match(grafanaDashboards, /consumables catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/consumables\/result/);
  assert.match(grafanaDashboards, /consumables result review/);
  assert.match(grafanaDashboards, /\/fabrication\/workholding\/catalog/);
  assert.match(grafanaDashboards, /workholding catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/workholding\/preflight\/catalog/);
  assert.match(grafanaDashboards, /workholding preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/workholding\/result/);
  assert.match(grafanaDashboards, /workholding result review/);
  assert.match(grafanaDashboards, /\/fabrication\/nesting\/catalog/);
  assert.match(grafanaDashboards, /nesting catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/nesting\/result/);
  assert.match(grafanaDashboards, /nesting result review/);
  assert.match(grafanaDashboards, /\/fabrication\/support-strategies\/catalog/);
  assert.match(grafanaDashboards, /support strategy catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/support-strategies\/result/);
  assert.match(grafanaDashboards, /support strategy result review/);
  assert.match(grafanaDashboards, /\/fabrication\/process-recipes\/catalog/);
  assert.match(grafanaDashboards, /process recipe catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/process-recipes\/result/);
  assert.match(grafanaDashboards, /process recipe result review/);
  assert.match(grafanaDashboards, /\/fabrication\/kinematics\/catalog/);
  assert.match(grafanaDashboards, /kinematics catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/kinematics\/result/);
  assert.match(grafanaDashboards, /kinematics result review/);
  assert.match(grafanaDashboards, /\/fabrication\/tolerances\/catalog/);
  assert.match(grafanaDashboards, /tolerance catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/tolerances\/result/);
  assert.match(grafanaDashboards, /tolerance result review/);
  assert.match(grafanaDashboards, /\/fabrication\/process-capabilities\/catalog/);
  assert.match(grafanaDashboards, /process capability catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/process-capabilities\/result/);
  assert.match(grafanaDashboards, /process capability result review/);
  assert.match(grafanaDashboards, /\/fabrication\/process\/catalog/);
  assert.match(grafanaDashboards, /process catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/evidence\/catalog/);
  assert.match(grafanaDashboards, /evidence catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/packages\/catalog/);
  assert.match(grafanaDashboards, /package catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/packages\/plan/);
  assert.match(grafanaDashboards, /package planning/);
  assert.match(grafanaDashboards, /\/fabrication\/manufacturability\/catalog/);
  assert.match(grafanaDashboards, /manufacturability catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/failure-modes\/catalog/);
  assert.match(grafanaDashboards, /failure mode catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/failure-modes\/result/);
  assert.match(grafanaDashboards, /failure mode result review/);
  assert.match(grafanaDashboards, /\/fabrication\/safety\/catalog/);
  assert.match(grafanaDashboards, /safety catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/safety\/result/);
  assert.match(grafanaDashboards, /safety result review/);
  assert.match(grafanaDashboards, /\/fabrication\/environment\/catalog/);
  assert.match(grafanaDashboards, /environment catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/environment\/result/);
  assert.match(grafanaDashboards, /environment result review/);
  assert.match(grafanaDashboards, /\/fabrication\/provenance\/catalog/);
  assert.match(grafanaDashboards, /provenance catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/as-built\/catalog/);
  assert.match(grafanaDashboards, /as-built catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/as-built\/result/);
  assert.match(grafanaDashboards, /as-built result review/);
  assert.match(grafanaDashboards, /\/fabrication\/provenance\/result/);
  assert.match(grafanaDashboards, /provenance result review/);
  assert.match(grafanaDashboards, /\/fabrication\/monitoring\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/postprocess\/catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/engines\/catalog/);
  assert.match(grafanaDashboards, /learning engine catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/preflight\/catalog/);
  assert.match(grafanaDashboards, /learning preflight catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/rewards\/catalog/);
  assert.match(grafanaDashboards, /learning reward catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/models\/catalog/);
  assert.match(grafanaDashboards, /learning model catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/replay\/catalog/);
  assert.match(grafanaDashboards, /learning replay catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/beliefs\/catalog/);
  assert.match(grafanaDashboards, /learning POMDP belief catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/optimizers\/catalog/);
  assert.match(grafanaDashboards, /learning optimizer catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/models\/result/);
  assert.match(grafanaDashboards, /learning model result review/);
  assert.match(grafanaDashboards, /\/fabrication\/learning\/optimizers\/result/);
  assert.match(grafanaDashboards, /learning optimizer result review/);
  assert.match(grafanaDashboards, /\/fabrication\/strategy\/catalog/);
  assert.match(grafanaDashboards, /strategy catalog/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/improve/);
  assert.match(grafanaDashboards, /\/fabrication\/instructions\/boundaries\/review/);
  const source = await readRepoFile('remote/deployments/fabrication-server-rs/src/main.rs');
  assertGrafanaCoversFabricationRootRoutes(source, grafanaDashboards);
  assert.match(home, /dd-fabrication-server/);
  assert.match(home, /\/fabrication\/jobs/);
  assert.match(home, /POST \/fabrication\/plan/);
  assert.match(home, /label: FABRICATION_REQUESTS_SUBJECT/);
  assert.match(home, /label: FABRICATION_RESULTS_SUBJECT/);
  assert.match(runtimeReadme, /dd-fabrication-server/);
  assert.match(runtimeReadme, /\/fabrication\/capabilities/);
  assert.match(runtimeReadme, /\/fabrication\/schema/);
  assert.match(runtimeReadme, /\/fabrication\/examples/);
  assert.match(runtimeReadme, /capabilities\/schema\/example discovery/);
  assert.match(runtimeReadme, /format-import catalog discovery/);
  assert.match(runtimeReadme, /strategy and calibration catalog discovery/);
  assert.match(runtimeReadme, /\/fabrication\/printers\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/interfaces\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/cells\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/machines\/select/);
  assert.match(runtimeReadme, /POST \/fabrication\/controllers\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/materials\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/materials\/result/);
  assert.match(runtimeReadme, /\/fabrication\/slicers\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/slicers\/result/);
  assert.match(runtimeReadme, /\/fabrication\/mesh-repair\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/mesh-repair\/result/);
  assert.match(runtimeReadme, /\/fabrication\/design\/import\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/design\/import\/review/);
  assert.match(runtimeReadme, /POST \/fabrication\/design\/import\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/design\/convert\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/design\/convert\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/improve/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/boundaries\/review/);
  assert.match(runtimeReadme, /\/fabrication\/design\/generation\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/design\/generate/);
  assert.match(runtimeReadme, /POST \/fabrication\/design\/synthesis\/result/);
  assert.match(runtimeReadme, /\/fabrication\/handoff\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/handoff\/result/);
  assert.match(runtimeReadme, /\/fabrication\/subjects\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/workers\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/results\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/instructions\/import\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/instructions\/validation\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/instructions\/generation\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/instructions\/generation\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/generate/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/generation\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/review\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/validation\/result/);
  assert.match(runtimeReadme, /\/fabrication\/machine-code\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/machine-code\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/machine-code\/generate/);
  assert.match(runtimeReadme, /POST \/fabrication\/machine-code\/result/);
  assert.match(runtimeReadme, /\/fabrication\/toolpaths\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/toolpaths\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/toolpaths\/result/);
  assert.match(runtimeReadme, /\/fabrication\/boundaries\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/decomposition\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/decomposition\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/assembly\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/assembly\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/interfaces\/result/);
  assert.match(runtimeReadme, /\/fabrication\/instructions\/import\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/release\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/execution\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/execution\/result/);
  assert.match(runtimeReadme, /\/fabrication\/simulation\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/simulation\/run/);
  assert.match(runtimeReadme, /POST \/fabrication\/simulation\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/quality\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/quality\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/manufacturability\/result/);
  assert.match(runtimeReadme, /\/fabrication\/improvements\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/remediation\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/remediation\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/remediation\/result/);
  assert.match(runtimeReadme, /\/fabrication\/improvements\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/assembly\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/assembly\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/release\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/release\/preview/);
  assert.match(runtimeReadme, /POST \/fabrication\/release\/result/);
  assert.match(runtimeReadme, /\/fabrication\/evidence\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/strategy\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/methods\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/process\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/packages\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/packages\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/strategy\/recommend/);
  assert.match(runtimeReadme, /POST \/fabrication\/strategy\/result/);
  assert.match(runtimeReadme, /\/fabrication\/schedule\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/schedule\/result/);
  assert.match(runtimeReadme, /\/fabrication\/simulation\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/jobs\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/quality\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/quality\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/dispositions\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/dispositions\/result/);
  assert.match(runtimeReadme, /\/fabrication\/costing\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/costing\/result/);
  assert.match(runtimeReadme, /\/fabrication\/utilities\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/energy\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/energy\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/utilities\/result/);
  assert.match(runtimeReadme, /\/fabrication\/telemetry\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/availability\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/availability\/result/);
  assert.match(runtimeReadme, /\/fabrication\/maintenance\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/maintenance\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/telemetry\/result/);
  assert.match(runtimeReadme, /\/fabrication\/calibration\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/calibration\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/calibration\/result/);
  assert.match(runtimeReadme, /\/fabrication\/interventions\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/interventions\/result/);
  assert.match(runtimeReadme, /\/fabrication\/setup\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/tooling\/result/);
  assert.match(runtimeReadme, /\/fabrication\/consumables\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/consumables\/result/);
  assert.match(runtimeReadme, /\/fabrication\/workholding\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/workholding\/preflight\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/workholding\/result/);
  assert.match(runtimeReadme, /\/fabrication\/nesting\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/nesting\/result/);
  assert.match(runtimeReadme, /\/fabrication\/support-strategies\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/support-strategies\/result/);
  assert.match(runtimeReadme, /\/fabrication\/process-recipes\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/process-recipes\/result/);
  assert.match(runtimeReadme, /\/fabrication\/kinematics\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/kinematics\/result/);
  assert.match(runtimeReadme, /\/fabrication\/tolerances\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/tolerances\/result/);
  assert.match(runtimeReadme, /\/fabrication\/process-capabilities\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/process-capabilities\/result/);
  assert.match(runtimeReadme, /\/fabrication\/manufacturability\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/failure-modes\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/failure-modes\/result/);
  assert.match(runtimeReadme, /\/fabrication\/safety\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/safety\/result/);
  assert.match(runtimeReadme, /\/fabrication\/environment\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/environment\/result/);
  assert.match(runtimeReadme, /\/fabrication\/as-built\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/as-built\/result/);
  assert.match(runtimeReadme, /\/fabrication\/provenance\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/provenance\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/setup\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/setup\/result/);
  assert.match(runtimeReadme, /\/fabrication\/monitoring\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/tooling\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/monitoring\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/monitoring\/result/);
  assert.match(runtimeReadme, /\/fabrication\/postprocess\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/postprocess\/plan/);
  assert.match(runtimeReadme, /POST \/fabrication\/postprocess\/result/);
  assert.match(runtimeReadme, /\/fabrication\/artifacts\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/jobs/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/engines\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/rewards\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/models\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/replay\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/optimizers\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/learning\/models\/result/);
  assert.match(runtimeReadme, /POST \/fabrication\/learning\/optimizers\/result/);
  assert.match(runtimeReadme, /\/fabrication\/learning\/corpus/);
  assert.match(runtimeReadme, /\/fabrication\/subtractive\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/subtractive\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/turning\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/cleanliness\/preflight\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/cnc\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/hybrid\/catalog/);
  assert.match(runtimeReadme, /\/fabrication\/jobs\/<jobId>\/release-bundle/);
  assert.match(runtimeReadme, /`POST \/fabrication\/plan`/);
  assert.match(runtimeReadme, /\/fabrication\/workflow\/catalog/);
  assert.match(runtimeReadme, /POST \/fabrication\/workflow\/plan/);
  assert.match(runtimeReadme, /fabrication\s+learning contracts/);
  assert.match(runtimeReadme, /`POST \/fabrication\/instructions\/analyze`/);
  assert.match(runtimeReadme, /POST \/fabrication\/instructions\/validate/);
  assert.match(runtimeReadme, /Gateway-generated `\/fabrication` redirects/);
  assert.match(runtimeReadme, /return `X-Request-ID`/);
  assert.match(runtimeReadme, /JSON `not_found` 404/);
  assert.match(runtimeReadme, /JSON `method_not_allowed` 405/);
  assert.match(runtimeReadme, /JSON `payload_too_large` 413/);
  assert.match(runtimeReadme, /JSON `rate_limited` 429/);
  assert.match(runtimeReadme, /explicit runtime hardening/);
  assert.match(runtimeReadme, /dedicated NetworkPolicy/);
  assert.match(runtimeReadme, /CAD design-conversion request\/queue\/result handoffs/);
  assert.match(runtimeReadme, /\/grafana\/fabrication/);
  assert.match(runtimeReadme, /dd-fabrication-planner/);
  assert.match(runtimeReadme, /validation findings, machine-failure boundaries/);
  assert.match(runtimeReadme, /required operator actions, fixture\/setup\s+blockers, split\/combine reviews/);
  assert.match(runtimeReadme, /capabilities\/schema\/example discovery, CAD\/design format\s+discovery, format-import catalog discovery, design-import review/);
  assert.match(runtimeReadme, /\/fabrication\/design\/preflight\/catalog/);
  assert.match(runtimeReadme, /design-import result review/);
  assert.match(runtimeReadme, /worker result-review route traffic/);
  assert.match(runtimeReadme, /instruction-improvement review/);
  assert.match(runtimeReadme, /instruction-boundary review/);
  assert.match(runtimeReadme, /POST \/fabrication\/boundaries\/result/);
  assert.match(observabilityReadme, /Fabrication Planner/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/corpus/);
  assert.match(observabilityReadme, /\/fabrication\/boundaries\/result/);
  assert.match(observabilityReadme, /\/fabrication\/remediation\/result/);
  assert.match(observabilityReadme, /uid `dd-fabrication-planner`/);
  assert.match(
    observabilityReadme,
    /validation-finding,\s+machine-failure boundary, required operator-action, fixture\/setup blocker, and\s+split\/combine review rates/,
  );
  assert.match(observabilityReadme, /artifact detail-request throughput/);
  assert.match(observabilityReadme, /costing-result review throughput/);
  assert.match(
    observabilityReadme,
    /catalog\s+discovery,\s+CAD intake,\s+design export,\s+instruction review,\s+validation-result,\s+and worker result\s+review panel/,
  );
  assert.match(observabilityReadme, /worker result\s+review panel/);
  assert.match(observabilityReadme, /\/fabrication\/machines\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/printers\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/subtractive\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/subtractive\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/turning\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/cleanliness\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/interfaces\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/cnc\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/hybrid\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/cells\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/boundaries\/result/);
  assert.match(observabilityReadme, /\/fabrication\/machines\/select/);
  assert.match(observabilityReadme, /\/fabrication\/controllers\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/controllers\/result/);
  assert.match(observabilityReadme, /\/fabrication\/materials\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/materials\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/materials\/result/);
  assert.match(observabilityReadme, /\/fabrication\/design\/formats/);
  assert.match(observabilityReadme, /\/fabrication\/design\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/slicers\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/slicers\/result/);
  assert.match(observabilityReadme, /\/fabrication\/mesh-repair\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/mesh-repair\/result/);
  assert.match(observabilityReadme, /\/fabrication\/design\/import\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/design\/import\/review/);
  assert.match(observabilityReadme, /\/fabrication\/design\/import\/result/);
  assert.match(observabilityReadme, /\/fabrication\/design\/convert\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/design\/convert\/result/);
  assert.match(observabilityReadme, /\/fabrication\/design\/generation\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/design\/generate/);
  assert.match(observabilityReadme, /\/fabrication\/design\/synthesis\/result/);
  assert.match(observabilityReadme, /\/fabrication\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/workflow\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/workflow\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/subjects\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/workers\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/generate/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/generation\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/import\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/generation\/result/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/review\/result/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/validation\/result/);
  assert.match(observabilityReadme, /\/fabrication\/machine-code\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/machine-code\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/machine-code\/generate/);
  assert.match(observabilityReadme, /\/fabrication\/machine-code\/result/);
  assert.match(observabilityReadme, /\/fabrication\/toolpaths\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/toolpaths\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/toolpaths\/result/);
  assert.match(observabilityReadme, /\/fabrication\/jobs\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/evidence\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/methods\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/process\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/packages\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/packages\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/decomposition\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/decomposition\/result/);
  assert.match(observabilityReadme, /\/fabrication\/assembly\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/assembly\/result/);
  assert.match(observabilityReadme, /\/fabrication\/interfaces\/result/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/import\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/release\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/execution\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/execution\/result/);
  assert.match(observabilityReadme, /\/fabrication\/simulation\/run/);
  assert.match(observabilityReadme, /\/fabrication\/simulation\/result/);
  assert.match(observabilityReadme, /\/fabrication\/quality\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/quality\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/quality\/result/);
  assert.match(observabilityReadme, /\/fabrication\/setup\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/setup\/result/);
  assert.match(observabilityReadme, /\/fabrication\/monitoring\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/monitoring\/result/);
  assert.match(observabilityReadme, /\/fabrication\/postprocess\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/postprocess\/result/);
  assert.match(observabilityReadme, /\/fabrication\/release\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/release\/preview/);
  assert.match(observabilityReadme, /\/fabrication\/release\/result/);
  assert.match(observabilityReadme, /\/fabrication\/strategy\/recommend/);
  assert.match(observabilityReadme, /\/fabrication\/strategy\/result/);
  assert.match(observabilityReadme, /\/fabrication\/artifacts\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/jobs/);
  assert.match(observabilityReadme, /\/fabrication\/jobs\/<jobId>/);
  assert.match(observabilityReadme, /\/fabrication\/jobs\/<jobId>\/release-bundle/);
  assert.match(
    observabilityReadme,
    /\/fabrication\/jobs\/<jobId>\/artifacts\/<artifactId>/,
  );
  assert.match(observabilityReadme, /\/fabrication\/learning\/capabilities/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/policy/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/observe/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/outcomes/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/analyze/);
  assert.match(observabilityReadme, /\/fabrication\/handoff\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/handoff\/result/);
  assert.match(observabilityReadme, /\/fabrication\/assembly\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/assembly\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/calibration\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/calibration\/result/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/languages/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/validation\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/validate/);
  assert.match(observabilityReadme, /\/fabrication\/improvements\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/improvements\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/boundaries\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/remediation\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/remediation\/plan/);
  assert.match(observabilityReadme, /\/fabrication\/decomposition\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/release\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/schedule\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/schedule\/result/);
  assert.match(observabilityReadme, /\/fabrication\/simulation\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/simulation\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/quality\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/quality\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/dispositions\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/dispositions\/result/);
  assert.match(observabilityReadme, /\/fabrication\/as-built\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/as-built\/result/);
  assert.match(observabilityReadme, /\/fabrication\/costing\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/costing\/result/);
  assert.match(observabilityReadme, /\/fabrication\/energy\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/energy\/result/);
  assert.match(observabilityReadme, /\/fabrication\/availability\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/availability\/result/);
  assert.match(observabilityReadme, /\/fabrication\/maintenance\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/maintenance\/result/);
  assert.match(observabilityReadme, /\/fabrication\/utilities\/result/);
  assert.match(observabilityReadme, /\/fabrication\/telemetry\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/telemetry\/result/);
  assert.match(observabilityReadme, /\/fabrication\/manufacturability\/result/);
  assert.match(observabilityReadme, /\/fabrication\/interventions\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/interventions\/result/);
  assert.match(observabilityReadme, /\/fabrication\/setup\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/tooling\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/tooling\/result/);
  assert.match(observabilityReadme, /\/fabrication\/consumables\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/consumables\/result/);
  assert.match(observabilityReadme, /\/fabrication\/workholding\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/workholding\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/workholding\/result/);
  assert.match(observabilityReadme, /\/fabrication\/nesting\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/nesting\/result/);
  assert.match(observabilityReadme, /\/fabrication\/support-strategies\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/support-strategies\/result/);
  assert.match(observabilityReadme, /\/fabrication\/process-recipes\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/process-recipes\/result/);
  assert.match(observabilityReadme, /\/fabrication\/kinematics\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/kinematics\/result/);
  assert.match(observabilityReadme, /\/fabrication\/tolerances\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/tolerances\/result/);
  assert.match(observabilityReadme, /\/fabrication\/process-capabilities\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/process-capabilities\/result/);
  assert.match(observabilityReadme, /\/fabrication\/manufacturability\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/failure-modes\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/failure-modes\/result/);
  assert.match(observabilityReadme, /\/fabrication\/safety\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/safety\/result/);
  assert.match(observabilityReadme, /\/fabrication\/environment\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/environment\/result/);
  assert.match(observabilityReadme, /\/fabrication\/provenance\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/provenance\/result/);
  assert.match(observabilityReadme, /\/fabrication\/monitoring\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/postprocess\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/engines\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/preflight\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/rewards\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/models\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/replay\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/optimizers\/catalog/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/models\/result/);
  assert.match(observabilityReadme, /\/fabrication\/learning\/optimizers\/result/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/improve/);
  assert.match(observabilityReadme, /\/fabrication\/instructions\/boundaries\/review/);
  assert.match(observabilityReadme, /\/fabrication\/boundaries\/preflight\/catalog/);
  assert.match(observabilityReadme, /job\/artifact\/learning evidence ledgers/);
  assert.match(remoteReadme, /fabrication-server-rs/);
});
