import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const generatorPath = path.join(packageRoot, 'src', 'generate.mjs');

async function readJson(relativePath) {
  return JSON.parse(await readFile(path.join(packageRoot, relativePath), 'utf8'));
}

function validateSchemaValue(schemaDoc, schema, value, label) {
  if (schema.$ref) {
    const match = /^#\/\$defs\/(.+)$/.exec(schema.$ref);
    assert.ok(match, `unsupported ref at ${label}: ${schema.$ref}`);
    const named = schemaDoc.$defs?.[match[1]];
    assert.ok(named, `missing schema def ${match[1]} at ${label}`);
    validateSchemaValue(schemaDoc, named, value, `${label}.${match[1]}`);
    return;
  }

  if (Array.isArray(schema.type)) {
    const errors = [];
    for (const type of schema.type) {
      try {
        validateSchemaValue(schemaDoc, { ...schema, type }, value, label);
        return;
      } catch (error) {
        errors.push(error.message);
      }
    }
    assert.fail(`${label} did not match any schema union type: ${errors.join('; ')}`);
  }

  switch (schema.type) {
    case 'object': {
      assert.equal(typeof value, 'object', `${label} must be an object`);
      assert.notEqual(value, null, `${label} must not be null`);
      assert.equal(Array.isArray(value), false, `${label} must not be an array`);
      const required = new Set(schema.required ?? []);
      for (const field of required) {
        assert.ok(Object.hasOwn(value, field), `${label}.${field} is required`);
      }
      const properties = schema.properties ?? {};
      if (schema.additionalProperties === false) {
        for (const field of Object.keys(value)) {
          assert.ok(Object.hasOwn(properties, field), `${label}.${field} is not declared`);
        }
      }
      for (const [field, fieldSchema] of Object.entries(properties)) {
        if (Object.hasOwn(value, field)) {
          validateSchemaValue(schemaDoc, fieldSchema, value[field], `${label}.${field}`);
        }
      }
      return;
    }
    case 'array':
      assert.ok(Array.isArray(value), `${label} must be an array`);
      value.forEach((entry, index) => {
        validateSchemaValue(schemaDoc, schema.items, entry, `${label}[${index}]`);
      });
      return;
    case 'string':
      assert.equal(typeof value, 'string', `${label} must be a string`);
      return;
    case 'integer':
      assert.equal(Number.isInteger(value), true, `${label} must be an integer`);
      if (schema.minimum !== undefined) assert.ok(value >= schema.minimum, `${label} below minimum`);
      return;
    case 'number':
      assert.equal(typeof value, 'number', `${label} must be a number`);
      assert.equal(Number.isFinite(value), true, `${label} must be finite`);
      if (schema.minimum !== undefined) assert.ok(value >= schema.minimum, `${label} below minimum`);
      return;
    case 'boolean':
      assert.equal(typeof value, 'boolean', `${label} must be a boolean`);
      return;
    case 'null':
      assert.equal(value, null, `${label} must be null`);
      return;
    case undefined:
      return;
    default:
      assert.fail(`unsupported schema type at ${label}: ${schema.type}`);
  }
}

function assertNoCredentialBearingUris(value, label = 'fixture') {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => assertNoCredentialBearingUris(entry, `${label}[${index}]`));
    return;
  }
  if (value === null || typeof value !== 'object') return;

  for (const [key, entry] of Object.entries(value)) {
    const nextLabel = `${label}.${key}`;
    if (/uri/i.test(key) && typeof entry === 'string') {
      const parsed = new URL(entry);
      assert.equal(parsed.username, '', `${nextLabel} must not contain URI username`);
      assert.equal(parsed.password, '', `${nextLabel} must not contain URI password`);
      assert.equal(parsed.search, '', `${nextLabel} must not contain query string`);
      assert.equal(parsed.hash, '', `${nextLabel} must not contain fragment`);
    }
    assertNoCredentialBearingUris(entry, nextLabel);
  }
}

test('generated outputs are up to date with schema source', () => {
  execFileSync(process.execPath, [generatorPath, '--check'], {
    cwd: packageRoot,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
});

test('typescript output exposes the agent task queue envelope', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type AgentTaskQueueMessage = \{/);
  assert.match(ts, /threadId: string;/);
  assert.match(ts, /taskId: string;/);
  assert.match(ts, /containerPoolDispatch\?: boolean;/);
});

test('typescript output exposes fabrication CAD conversion payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationDesignConversionRequest = \{/);
  assert.match(ts, /designInputs: FabricationDesignInputRef\[\];/);
  assert.match(ts, /targets: FabricationDesignConversionTarget\[\];/);
  assert.match(ts, /resultSubject\?: string \| null;/);
  assert.match(ts, /export type FabricationDesignConversionResult = \{/);
  assert.match(ts, /machineReady: boolean;/);
  assert.match(ts, /translatorVersion\?: string \| null;/);
  assert.match(ts, /artifacts: FabricationNeutralExportArtifact\[\];/);
  assert.match(ts, /export type FabricationDesignConversionBlocker = \{/);
});

test('typescript output exposes fabrication machine profile payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationMachineProfileRequest = \{/);
  assert.match(ts, /scopes: FabricationMachineProfileScope\[\];/);
  assert.match(ts, /preferredMachineIds\?: string\[\];/);
  assert.match(ts, /requiredMachineClasses\?: string\[\];/);
  assert.match(ts, /export type FabricationMachineProfileResult = \{/);
  assert.match(ts, /machineReady: boolean;/);
  assert.match(ts, /machines: FabricationMachineCapabilitySnapshot\[\];/);
  assert.match(ts, /calibrations\?: FabricationMachineCalibrationState\[\];/);
  assert.match(ts, /tools\?: FabricationMachineToolState\[\];/);
  assert.match(ts, /fixtures\?: FabricationMachineFixtureState\[\];/);
  assert.match(ts, /materials\?: FabricationMachineMaterialState\[\];/);
});

test('typescript output exposes fabrication design synthesis payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationDesignSynthesisRequest = \{/);
  assert.match(ts, /intent: FabricationDesignIntent;/);
  assert.match(ts, /references: FabricationDesignReference\[\];/);
  assert.match(ts, /templates\?: FabricationDesignTemplate\[\];/);
  assert.match(ts, /targetFormats: string\[\];/);
  assert.match(ts, /learningHints\?: FabricationDesignLearningHint\[\];/);
  assert.match(ts, /export type FabricationDesignSynthesisResult = \{/);
  assert.match(ts, /selectedCandidateId\?: string \| null;/);
  assert.match(ts, /candidates: FabricationDesignCandidate\[\];/);
  assert.match(ts, /artifacts: FabricationGeneratedDesignArtifact\[\];/);
});

test('typescript output exposes fabrication instruction generation payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationInstructionGenerationRequest = \{/);
  assert.match(ts, /sourceArtifacts: FabricationInstructionSourceArtifact\[\];/);
  assert.match(ts, /machineProfiles: FabricationInstructionMachineProfile\[\];/);
  assert.match(ts, /operations: FabricationInstructionOperation\[\];/);
  assert.match(ts, /targets: FabricationInstructionGenerationTarget\[\];/);
  assert.match(ts, /resultSubject\?: string \| null;/);
  assert.match(ts, /export type FabricationInstructionGenerationResult = \{/);
  assert.match(ts, /machineReady: boolean;/);
  assert.match(ts, /generatorVersion\?: string \| null;/);
  assert.match(ts, /artifacts: FabricationGeneratedInstructionArtifact\[\];/);
  assert.match(ts, /export type FabricationInstructionBlocker = \{/);
});

test('typescript output exposes fabrication instruction review payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationInstructionReviewRequest = \{/);
  assert.match(ts, /instructions: FabricationSubmittedInstruction\[\];/);
  assert.match(ts, /reviewScopes: FabricationInstructionReviewScope\[\];/);
  assert.match(ts, /resultSubject\?: string \| null;/);
  assert.match(ts, /export type FabricationInstructionReviewResult = \{/);
  assert.match(ts, /machineReady: boolean;/);
  assert.match(ts, /findings: FabricationInstructionReviewFinding\[\];/);
  assert.match(ts, /failureBoundaries\?: FabricationInstructionFailureBoundary\[\];/);
  assert.match(ts, /improvementDrafts\?: FabricationInstructionImprovementDraft\[\];/);
});

test('typescript output exposes fabrication instruction simulation payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationInstructionSimulationRequest = \{/);
  assert.match(ts, /instructions: FabricationSimulationInstructionArtifact\[\];/);
  assert.match(ts, /machineContexts: FabricationSimulationMachineContext\[\];/);
  assert.match(ts, /scopes: FabricationSimulationScope\[\];/);
  assert.match(ts, /resultSubject\?: string \| null;/);
  assert.match(ts, /export type FabricationInstructionSimulationResult = \{/);
  assert.match(ts, /machineReady: boolean;/);
  assert.match(ts, /envelopeChecks: FabricationSimulationEnvelopeCheck\[\];/);
  assert.match(ts, /findings: FabricationSimulationFinding\[\];/);
  assert.match(ts, /failureBoundaries: FabricationSimulationFailureBoundary\[\];/);
});

test('typescript output exposes fabrication assembly planning payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationAssemblyPlanningRequest = \{/);
  assert.match(ts, /sourceArtifacts: FabricationAssemblySourceArtifact\[\];/);
  assert.match(ts, /capabilities: FabricationAssemblyMachineCapability\[\];/);
  assert.match(ts, /candidateParts: FabricationAssemblyCandidatePart\[\];/);
  assert.match(ts, /resultSubject\?: string \| null;/);
  assert.match(ts, /export type FabricationAssemblyPlanningResult = \{/);
  assert.match(ts, /selectedPlanId\?: string \| null;/);
  assert.match(ts, /candidates: FabricationAssemblyPlanCandidate\[\];/);
  assert.match(ts, /learningSignals\?: FabricationAssemblyLearningSignal\[\];/);
  assert.match(ts, /export type FabricationAssemblyBlocker = \{/);
});

test('typescript output exposes fabrication learning outcome payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationLearningOutcomeRequest = \{/);
  assert.match(ts, /outcomeStatus: string;/);
  assert.match(ts, /sources: FabricationLearningSourceRef\[\];/);
  assert.match(ts, /observations: FabricationLearningObservation\[\];/);
  assert.match(ts, /failureBoundaries\?: FabricationLearningFailureBoundary\[\];/);
  assert.match(ts, /rewardSignals\?: FabricationLearningRewardSignal\[\];/);
  assert.match(ts, /export type FabricationLearningOutcomeResult = \{/);
  assert.match(ts, /learnerVersion\?: string \| null;/);
  assert.match(ts, /updates: FabricationLearningModelUpdate\[\];/);
  assert.match(ts, /retainedBoundaries\?: FabricationLearningFailureBoundary\[\];/);
});

test('typescript output exposes fabrication release readiness payloads', async () => {
  const ts = await readFile(path.join(packageRoot, 'generated', 'typescript', 'index.ts'), 'utf8');

  assert.match(ts, /export type FabricationReleaseReadinessRequest = \{/);
  assert.match(ts, /evidenceRefs: FabricationReleaseEvidenceRef\[\];/);
  assert.match(ts, /machineGates: FabricationReleaseMachineGate\[\];/);
  assert.match(ts, /humanInterventions: FabricationReleaseHumanIntervention\[\];/);
  assert.match(ts, /requestedArtifacts: FabricationReleaseManifestArtifact\[\];/);
  assert.match(ts, /knownBlockers: FabricationReleaseBlocker\[\];/);
  assert.match(ts, /export type FabricationReleaseReadinessResult = \{/);
  assert.match(ts, /machineReady: boolean;/);
  assert.match(ts, /decisions: FabricationReleaseDecision\[\];/);
  assert.match(ts, /manifestArtifacts: FabricationReleaseManifestArtifact\[\];/);
});

test('rust output exposes the agent task queue envelope', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct AgentTaskQueueMessage \{/);
  assert.match(rs, /pub thread_id: String,/);
  assert.match(rs, /pub task_id: String,/);
  assert.match(rs, /pub container_pool_dispatch: Option<bool>,/);
});

test('rust output exposes fabrication CAD conversion payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationDesignConversionRequest \{/);
  assert.match(rs, /pub design_inputs: Vec<FabricationDesignInputRef>,/);
  assert.match(rs, /pub targets: Vec<FabricationDesignConversionTarget>,/);
  assert.match(rs, /pub result_subject: Option<String>,/);
  assert.match(rs, /pub struct FabricationDesignConversionResult \{/);
  assert.match(rs, /pub machine_ready: bool,/);
  assert.match(rs, /pub translator_version: Option<String>,/);
  assert.match(rs, /pub artifacts: Vec<FabricationNeutralExportArtifact>,/);
  assert.match(rs, /pub struct FabricationDesignConversionBlocker \{/);
});

test('rust output exposes fabrication machine profile payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationMachineProfileRequest \{/);
  assert.match(rs, /pub scopes: Vec<FabricationMachineProfileScope>,/);
  assert.match(rs, /pub preferred_machine_ids: Option<Vec<String>>,/);
  assert.match(rs, /pub required_machine_classes: Option<Vec<String>>,/);
  assert.match(rs, /pub struct FabricationMachineProfileResult \{/);
  assert.match(rs, /pub machine_ready: bool,/);
  assert.match(rs, /pub machines: Vec<FabricationMachineCapabilitySnapshot>,/);
  assert.match(rs, /pub calibrations: Option<Vec<FabricationMachineCalibrationState>>,/);
  assert.match(rs, /pub tools: Option<Vec<FabricationMachineToolState>>,/);
  assert.match(rs, /pub fixtures: Option<Vec<FabricationMachineFixtureState>>,/);
  assert.match(rs, /pub materials: Option<Vec<FabricationMachineMaterialState>>,/);
});

test('rust output exposes fabrication design synthesis payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationDesignSynthesisRequest \{/);
  assert.match(rs, /pub intent: FabricationDesignIntent,/);
  assert.match(rs, /pub references: Vec<FabricationDesignReference>,/);
  assert.match(rs, /pub templates: Option<Vec<FabricationDesignTemplate>>,/);
  assert.match(rs, /pub target_formats: Vec<String>,/);
  assert.match(rs, /pub learning_hints: Option<Vec<FabricationDesignLearningHint>>,/);
  assert.match(rs, /pub struct FabricationDesignSynthesisResult \{/);
  assert.match(rs, /pub selected_candidate_id: Option<String>,/);
  assert.match(rs, /pub candidates: Vec<FabricationDesignCandidate>,/);
  assert.match(rs, /pub artifacts: Vec<FabricationGeneratedDesignArtifact>,/);
});

test('rust output exposes fabrication instruction generation payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationInstructionGenerationRequest \{/);
  assert.match(rs, /pub source_artifacts: Vec<FabricationInstructionSourceArtifact>,/);
  assert.match(rs, /pub machine_profiles: Vec<FabricationInstructionMachineProfile>,/);
  assert.match(rs, /pub operations: Vec<FabricationInstructionOperation>,/);
  assert.match(rs, /pub targets: Vec<FabricationInstructionGenerationTarget>,/);
  assert.match(rs, /pub result_subject: Option<String>,/);
  assert.match(rs, /pub struct FabricationInstructionGenerationResult \{/);
  assert.match(rs, /pub machine_ready: bool,/);
  assert.match(rs, /pub generator_version: Option<String>,/);
  assert.match(rs, /pub artifacts: Vec<FabricationGeneratedInstructionArtifact>,/);
  assert.match(rs, /pub struct FabricationInstructionBlocker \{/);
});

test('rust output exposes fabrication instruction review payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationInstructionReviewRequest \{/);
  assert.match(rs, /pub instructions: Vec<FabricationSubmittedInstruction>,/);
  assert.match(rs, /pub review_scopes: Vec<FabricationInstructionReviewScope>,/);
  assert.match(rs, /pub result_subject: Option<String>,/);
  assert.match(rs, /pub struct FabricationInstructionReviewResult \{/);
  assert.match(rs, /pub machine_ready: bool,/);
  assert.match(rs, /pub findings: Vec<FabricationInstructionReviewFinding>,/);
  assert.match(rs, /pub failure_boundaries: Option<Vec<FabricationInstructionFailureBoundary>>,/);
  assert.match(rs, /pub improvement_drafts: Option<Vec<FabricationInstructionImprovementDraft>>,/);
});

test('rust output exposes fabrication instruction simulation payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationInstructionSimulationRequest \{/);
  assert.match(rs, /pub instructions: Vec<FabricationSimulationInstructionArtifact>,/);
  assert.match(rs, /pub machine_contexts: Vec<FabricationSimulationMachineContext>,/);
  assert.match(rs, /pub scopes: Vec<FabricationSimulationScope>,/);
  assert.match(rs, /pub result_subject: Option<String>,/);
  assert.match(rs, /pub struct FabricationInstructionSimulationResult \{/);
  assert.match(rs, /pub machine_ready: bool,/);
  assert.match(rs, /pub envelope_checks: Vec<FabricationSimulationEnvelopeCheck>,/);
  assert.match(rs, /pub findings: Vec<FabricationSimulationFinding>,/);
  assert.match(rs, /pub failure_boundaries: Vec<FabricationSimulationFailureBoundary>,/);
});

test('rust output exposes fabrication assembly planning payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationAssemblyPlanningRequest \{/);
  assert.match(rs, /pub source_artifacts: Vec<FabricationAssemblySourceArtifact>,/);
  assert.match(rs, /pub capabilities: Vec<FabricationAssemblyMachineCapability>,/);
  assert.match(rs, /pub candidate_parts: Vec<FabricationAssemblyCandidatePart>,/);
  assert.match(rs, /pub result_subject: Option<String>,/);
  assert.match(rs, /pub struct FabricationAssemblyPlanningResult \{/);
  assert.match(rs, /pub selected_plan_id: Option<String>,/);
  assert.match(rs, /pub candidates: Vec<FabricationAssemblyPlanCandidate>,/);
  assert.match(rs, /pub learning_signals: Option<Vec<FabricationAssemblyLearningSignal>>,/);
  assert.match(rs, /pub struct FabricationAssemblyBlocker \{/);
});

test('rust output exposes fabrication learning outcome payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationLearningOutcomeRequest \{/);
  assert.match(rs, /pub outcome_status: String,/);
  assert.match(rs, /pub sources: Vec<FabricationLearningSourceRef>,/);
  assert.match(rs, /pub observations: Vec<FabricationLearningObservation>,/);
  assert.match(rs, /pub failure_boundaries: Option<Vec<FabricationLearningFailureBoundary>>,/);
  assert.match(rs, /pub reward_signals: Option<Vec<FabricationLearningRewardSignal>>,/);
  assert.match(rs, /pub struct FabricationLearningOutcomeResult \{/);
  assert.match(rs, /pub learner_version: Option<String>,/);
  assert.match(rs, /pub updates: Vec<FabricationLearningModelUpdate>,/);
  assert.match(rs, /pub retained_boundaries: Option<Vec<FabricationLearningFailureBoundary>>,/);
});

test('rust output exposes fabrication release readiness payloads', async () => {
  const rs = await readFile(path.join(packageRoot, 'generated', 'rust', 'src', 'lib.rs'), 'utf8');

  assert.match(rs, /pub struct FabricationReleaseReadinessRequest \{/);
  assert.match(rs, /pub evidence_refs: Vec<FabricationReleaseEvidenceRef>,/);
  assert.match(rs, /pub machine_gates: Vec<FabricationReleaseMachineGate>,/);
  assert.match(rs, /pub human_interventions: Vec<FabricationReleaseHumanIntervention>,/);
  assert.match(rs, /pub requested_artifacts: Vec<FabricationReleaseManifestArtifact>,/);
  assert.match(rs, /pub known_blockers: Vec<FabricationReleaseBlocker>,/);
  assert.match(rs, /pub struct FabricationReleaseReadinessResult \{/);
  assert.match(rs, /pub machine_ready: bool,/);
  assert.match(rs, /pub decisions: Vec<FabricationReleaseDecision>,/);
  assert.match(rs, /pub manifest_artifacts: Vec<FabricationReleaseManifestArtifact>,/);
});

test('fabrication CAD conversion examples validate and correlate request/result envelopes', async () => {
  const schemaDoc = await readJson('schema/fabrication-cad-conversion.schema.json');
  const request = await readJson('examples/fabrication-design-conversion-request.json');
  const result = await readJson('examples/fabrication-design-conversion-result.json');

  validateSchemaValue(
    schemaDoc,
    { $ref: '#/$defs/FabricationDesignConversionRequest' },
    request,
    'request',
  );
  validateSchemaValue(
    schemaDoc,
    { $ref: '#/$defs/FabricationDesignConversionResult' },
    result,
    'result',
  );

  assert.equal(request.schema, 'dd.fabrication.design-conversion.request.v1');
  assert.equal(result.schema, 'dd.fabrication.design-conversion.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.design.conversion.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const inputIds = new Set(request.designInputs.map((input) => input.inputId));
  const targetIds = new Set(request.targets.map((target) => target.targetId));
  for (const target of request.targets) {
    if (target.sourceInputId) assert.ok(inputIds.has(target.sourceInputId));
  }
  for (const blocker of request.blockers) {
    if (blocker.inputId) assert.ok(inputIds.has(blocker.inputId));
    if (blocker.targetId) assert.ok(targetIds.has(blocker.targetId));
  }
  for (const artifact of result.artifacts) {
    if (artifact.sourceInputId) assert.ok(inputIds.has(artifact.sourceInputId));
    if (artifact.targetId) assert.ok(targetIds.has(artifact.targetId));
    assert.notEqual(artifact.sha256, request.designInputs[0].sourceSha256);
  }
  for (const blocker of result.blockers) {
    if (blocker.inputId) assert.ok(inputIds.has(blocker.inputId));
    if (blocker.targetId) assert.ok(targetIds.has(blocker.targetId));
  }

  assert.equal(result.machineReady, false);
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'STEP'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === '3MF'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'needs-operator-review'));
  assertNoCredentialBearingUris(request, 'request');
  assertNoCredentialBearingUris(result, 'result');
});
