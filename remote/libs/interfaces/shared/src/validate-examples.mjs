import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

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
    const uriField = key.toLowerCase() === 'uri' || key.toLowerCase().endsWith('uri') || key.toLowerCase().endsWith('uris');
    if (uriField && typeof entry === 'string') {
      const parsed = new URL(entry);
      assert.equal(parsed.username, '', `${nextLabel} must not contain URI username`);
      assert.equal(parsed.password, '', `${nextLabel} must not contain URI password`);
      assert.equal(parsed.search, '', `${nextLabel} must not contain query string`);
      assert.equal(parsed.hash, '', `${nextLabel} must not contain fragment`);
    }
    assertNoCredentialBearingUris(entry, nextLabel);
  }
}

function assertMachineProfileCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.machine-profile.request.v1');
  assert.equal(result.schema, 'dd.fabrication.machine-profile.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.machine.profiles.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const machineIds = new Set(result.machines.map((machine) => machine.machineId));
  const machineClasses = new Set(result.machines.map((machine) => machine.machineClass));
  for (const machineId of request.preferredMachineIds) {
    assert.ok(machineIds.has(machineId), `missing preferred machine ${machineId}`);
  }
  for (const machineClass of request.requiredMachineClasses) {
    assert.ok(machineClasses.has(machineClass), `missing required machine class ${machineClass}`);
  }
  for (const scope of request.scopes) {
    if (scope.machineId) assert.ok(machineIds.has(scope.machineId), `${scope.scopeId} references unknown machine`);
    if (scope.machineClass) assert.ok(machineClasses.has(scope.machineClass), `${scope.scopeId} references missing machine class`);
    assert.ok(scope.requestedEvidence.length > 0, `${scope.scopeId} should request concrete evidence`);
  }

  for (const calibration of result.calibrations) {
    assert.ok(machineIds.has(calibration.machineId), `${calibration.calibrationId} references unknown machine`);
  }
  for (const tool of result.tools) {
    assert.ok(machineIds.has(tool.machineId), `${tool.toolId} references unknown machine`);
  }
  for (const fixture of result.fixtures) {
    assert.ok(machineIds.has(fixture.machineId), `${fixture.fixtureId} references unknown machine`);
  }
  for (const material of result.materials) {
    assert.ok(machineIds.has(material.machineId), `${material.materialStateId} references unknown machine`);
  }
  for (const blocker of result.blockers) {
    if (blocker.machineId) assert.ok(machineIds.has(blocker.machineId), `${blocker.blockerId} references unknown machine`);
    assert.ok(blocker.evidenceRequired.length > 0, `${blocker.blockerId} should require evidence`);
  }

  const blockerCodes = new Set(result.blockers.map((blocker) => blocker.code));
  assert.equal(result.success, true);
  assert.equal(result.machineReady, false);
  assert.ok(result.machines.some((machine) => machine.machineClass === 'additive-fdm'));
  assert.ok(result.machines.some((machine) => machine.machineClass === 'vertical-mill'));
  assert.ok(result.machines.some((machine) => machine.machineClass === 'lathe'));
  assert.ok(result.machines.some((machine) => machine.machineClass === 'waterjet'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'blocks-machine-ready'));
  assert.ok(blockerCodes.has('fixture-proof-required'));
  assert.ok(blockerCodes.has('material-conditioning-required'));
  assert.ok(blockerCodes.has('partoff-support-required'));
  assert.ok(blockerCodes.has('support-media-required'));
  assertNoCredentialBearingUris(request, 'machineProfileRequest');
  assertNoCredentialBearingUris(result, 'machineProfileResult');
}

function assertDesignSynthesisCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.design-synthesis.request.v1');
  assert.equal(result.schema, 'dd.fabrication.design-synthesis.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.design.synthesis.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const referenceFormats = new Set(request.references.map((reference) => reference.format));
  for (const format of ['SOLIDWORKS', 'PROE', 'OPENSCAD']) {
    assert.ok(referenceFormats.has(format) || request.capabilities.some((capability) => capability.supportedFormats.includes(format)));
  }
  for (const format of ['FUSION', 'NX', 'CATIA', 'ONSHAPE', 'FREECAD', 'BLENDER', 'ZBRUSH']) {
    assert.ok(
      request.capabilities.some((capability) => capability.supportedFormats.includes(format)),
      `missing synthesis capability for ${format}`,
    );
  }

  const constraintIds = new Set(request.constraints.map((constraint) => constraint.constraintId));
  const candidateIds = new Set(result.candidates.map((candidate) => candidate.candidateId));
  const artifactIds = new Set(result.artifacts.map((artifact) => artifact.artifactId));
  assert.ok(result.selectedCandidateId, 'design synthesis result should recommend a selected candidate');
  assert.ok(candidateIds.has(result.selectedCandidateId), `${result.selectedCandidateId} does not match a candidate`);

  for (const candidate of result.candidates) {
    for (const artifactId of candidate.artifactIds) {
      assert.ok(artifactIds.has(artifactId), `${candidate.candidateId} references unknown artifact ${artifactId}`);
    }
    for (const constraintId of candidate.clearedConstraintIds) {
      assert.ok(constraintIds.has(constraintId), `${candidate.candidateId} clears unknown constraint ${constraintId}`);
    }
    for (const blocker of candidate.retainedBlockers) {
      if (blocker.candidateId) assert.equal(blocker.candidateId, candidate.candidateId);
      if (blocker.constraintId) assert.ok(constraintIds.has(blocker.constraintId), `${blocker.code} references unknown constraint`);
    }
  }
  for (const artifact of result.artifacts) {
    assert.ok(candidateIds.has(artifact.candidateId), `${artifact.artifactId} references unknown candidate`);
  }
  for (const blocker of result.blockers) {
    if (blocker.candidateId) assert.ok(candidateIds.has(blocker.candidateId), `${blocker.code} references unknown candidate`);
    if (blocker.constraintId) assert.ok(constraintIds.has(blocker.constraintId), `${blocker.code} references unknown constraint`);
  }

  assert.equal(result.success, true);
  assert.equal(result.machineReady, false);
  assert.ok(result.candidates.some((candidate) => candidate.candidateKind === 'split-print-and-machine'));
  assert.ok(result.candidates.some((candidate) => candidate.candidateKind === 'monolithic-print'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'OPENSCAD'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'STEP'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === '3MF'));
  assert.ok(result.artifacts.some((artifact) => artifact.artifactKind === 'assembly-graph'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'needs-operator-review'));
  assert.ok(result.learningHints.some((hint) => hint.hintKind === 'policy-action'));
  assertNoCredentialBearingUris(request, 'designSynthesisRequest');
  assertNoCredentialBearingUris(result, 'designSynthesisResult');
}

function assertConversionCorrelation(request, result) {
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
}

function assertInstructionCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.instruction-generation.request.v1');
  assert.equal(result.schema, 'dd.fabrication.instruction-generation.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.instructions.generation.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const sourceArtifactIds = new Set(request.sourceArtifacts.map((artifact) => artifact.artifactId));
  const machineIds = new Set(request.machineProfiles.map((machine) => machine.machineId));
  const operationIds = new Set(request.operations.map((operation) => operation.operationId));
  const targetIds = new Set(request.targets.map((target) => target.targetId));

  for (const operation of request.operations) {
    for (const sourceArtifactId of operation.sourceArtifactIds) {
      assert.ok(sourceArtifactIds.has(sourceArtifactId), `${operation.operationId} references unknown artifact`);
    }
  }
  for (const target of request.targets) {
    assert.ok(operationIds.has(target.operationId), `${target.targetId} references unknown operation`);
    if (target.machineId) assert.ok(machineIds.has(target.machineId), `${target.targetId} references unknown machine`);
  }
  for (const blocker of request.blockers) {
    if (blocker.operationId) assert.ok(operationIds.has(blocker.operationId));
    if (blocker.targetId) assert.ok(targetIds.has(blocker.targetId));
    if (blocker.machineId) assert.ok(machineIds.has(blocker.machineId));
  }
  for (const artifact of result.artifacts) {
    assert.ok(targetIds.has(artifact.targetId), `${artifact.artifactId} references unknown target`);
    assert.ok(operationIds.has(artifact.operationId), `${artifact.artifactId} references unknown operation`);
    if (artifact.machineId) assert.ok(machineIds.has(artifact.machineId), `${artifact.artifactId} references unknown machine`);
    assert.ok(artifact.previewLines.length > 0, `${artifact.artifactId} should include short preview lines`);
  }
  for (const blocker of result.blockers) {
    if (blocker.operationId) assert.ok(operationIds.has(blocker.operationId));
    if (blocker.targetId) assert.ok(targetIds.has(blocker.targetId));
    if (blocker.machineId) assert.ok(machineIds.has(blocker.machineId));
  }

  assert.equal(result.machineReady, false);
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'GCODE'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'NC'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'SETUP_SHEET_MD'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'blocks-machine-ready'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'needs-operator-review'));
  assertNoCredentialBearingUris(request, 'instructionRequest');
  assertNoCredentialBearingUris(result, 'instructionResult');
}

function assertInstructionReviewCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.instruction-review.request.v1');
  assert.equal(result.schema, 'dd.fabrication.instruction-review.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.instructions.review.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const instructionIds = new Set(request.instructions.map((instruction) => instruction.instructionId));
  const scopeIds = new Set(request.reviewScopes.map((scope) => scope.scopeId));
  for (const scope of request.reviewScopes) {
    assert.ok(instructionIds.has(scope.instructionId), `${scope.scopeId} references unknown instruction`);
  }
  for (const finding of result.findings) {
    if (finding.instructionId) assert.ok(instructionIds.has(finding.instructionId));
    if (finding.scopeId) assert.ok(scopeIds.has(finding.scopeId));
  }
  for (const boundary of result.failureBoundaries) {
    if (boundary.instructionId) assert.ok(instructionIds.has(boundary.instructionId));
  }
  for (const draft of result.improvementDrafts) {
    assert.ok(instructionIds.has(draft.instructionId), `${draft.draftId} references unknown instruction`);
    assert.ok(draft.previewLines.length > 0, `${draft.draftId} should include short preview lines`);
    assert.equal(draft.requiresHumanApproval, true, `${draft.draftId} should require approval in the fixture`);
  }

  assert.equal(result.machineReady, false);
  assert.ok(result.findings.some((finding) => finding.severity === 'blocker'));
  assert.ok(result.findings.some((finding) => finding.machineReadyImpact === 'blocks-machine-ready'));
  assert.ok(result.failureBoundaries.some((boundary) => boundary.humanInterventionRequired));
  assert.ok(result.failureBoundaries.some((boundary) => boundary.recommendedAction === 'add-temperature-wait'));
  assert.ok(result.improvementDrafts.some((draft) => draft.draftKind === 'add-workholding-check'));
  assert.ok(result.improvementDrafts.some((draft) => draft.draftKind === 'add-temperature-wait'));
  assertNoCredentialBearingUris(request, 'instructionReviewRequest');
  assertNoCredentialBearingUris(result, 'instructionReviewResult');
}

function assertInstructionSimulationCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.instruction-simulation.request.v1');
  assert.equal(result.schema, 'dd.fabrication.instruction-simulation.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.instructions.simulation.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const instructionIds = new Set(request.instructions.map((instruction) => instruction.instructionId));
  const machineIds = new Set(request.machineContexts.map((machine) => machine.machineId));
  for (const instruction of request.instructions) {
    if (instruction.machineId) assert.ok(machineIds.has(instruction.machineId), `${instruction.instructionId} references unknown machine`);
  }
  for (const scope of request.scopes) {
    assert.ok(instructionIds.has(scope.instructionId), `${scope.scopeId} references unknown instruction`);
    assert.ok(scope.checks.length > 0, `${scope.scopeId} should request concrete checks`);
  }
  for (const check of result.envelopeChecks) {
    assert.ok(machineIds.has(check.machineId), `${check.checkId} references unknown machine`);
    if (check.instructionId) assert.ok(instructionIds.has(check.instructionId), `${check.checkId} references unknown instruction`);
  }
  for (const finding of result.findings) {
    if (finding.instructionId) assert.ok(instructionIds.has(finding.instructionId), `${finding.findingId} references unknown instruction`);
    if (finding.machineId) assert.ok(machineIds.has(finding.machineId), `${finding.findingId} references unknown machine`);
  }
  for (const boundary of result.failureBoundaries) {
    if (boundary.instructionId) assert.ok(instructionIds.has(boundary.instructionId), `${boundary.boundaryId} references unknown instruction`);
    if (boundary.machineId) assert.ok(machineIds.has(boundary.machineId), `${boundary.boundaryId} references unknown machine`);
  }
  for (const artifact of result.artifacts) {
    if (artifact.instructionId) assert.ok(instructionIds.has(artifact.instructionId), `${artifact.artifactId} references unknown instruction`);
  }

  assert.equal(result.success, true);
  assert.equal(result.machineReady, false);
  assert.ok(result.envelopeChecks.some((check) => check.status === 'pass'));
  assert.ok(result.envelopeChecks.some((check) => check.status === 'blocked'));
  assert.ok(result.findings.some((finding) => finding.severity === 'blocker'));
  assert.ok(result.findings.some((finding) => finding.findingKind === 'workholding'));
  assert.ok(result.findings.some((finding) => finding.findingKind === 'material-state'));
  assert.ok(result.findings.some((finding) => finding.findingKind === 'partoff-support'));
  assert.ok(result.failureBoundaries.every((boundary) => boundary.humanInterventionRequired));
  assert.ok(result.failureBoundaries.some((boundary) => boundary.code === 'workholding-release-required'));
  assert.ok(result.artifacts.some((artifact) => artifact.artifactKind === 'simulation-report'));
  assert.ok(result.artifacts.some((artifact) => artifact.artifactKind === 'envelope-report'));
  assertNoCredentialBearingUris(request, 'instructionSimulationRequest');
  assertNoCredentialBearingUris(result, 'instructionSimulationResult');
}

function assertAssemblyCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.assembly-planning.request.v1');
  assert.equal(result.schema, 'dd.fabrication.assembly-planning.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.assembly.planning.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const sourceArtifactIds = new Set(request.sourceArtifacts.map((artifact) => artifact.artifactId));
  const capabilityIds = new Set(request.capabilities.map((capability) => capability.capabilityId));
  for (const part of request.candidateParts) {
    for (const sourceArtifactId of part.sourceArtifactIds) {
      assert.ok(sourceArtifactIds.has(sourceArtifactId), `${part.partId} references unknown source artifact`);
    }
  }

  const candidateIds = new Set(result.candidates.map((candidate) => candidate.planId));
  assert.ok(result.selectedPlanId, 'assembly result should recommend a selected plan');
  assert.ok(candidateIds.has(result.selectedPlanId), `${result.selectedPlanId} does not match a candidate plan`);

  for (const candidate of result.candidates) {
    const partIds = new Set(candidate.parts.map((part) => part.partId));
    const interfaceIds = new Set(candidate.interfaces.map((entry) => entry.interfaceId));
    const stepIds = new Set(candidate.processSteps.map((step) => step.stepId));

    for (const part of candidate.parts) {
      for (const sourceArtifactId of part.sourceArtifactIds) {
        assert.ok(sourceArtifactIds.has(sourceArtifactId), `${candidate.planId}.${part.partId} references unknown artifact`);
      }
    }
    for (const entry of candidate.interfaces) {
      assert.ok(partIds.has(entry.fromPartId), `${entry.interfaceId} references unknown fromPartId`);
      assert.ok(partIds.has(entry.toPartId), `${entry.interfaceId} references unknown toPartId`);
    }
    for (const step of candidate.processSteps) {
      for (const partId of step.partIds) {
        assert.ok(partIds.has(partId), `${step.stepId} references unknown part`);
      }
      if (step.capabilityId) assert.ok(capabilityIds.has(step.capabilityId), `${step.stepId} references unknown capability`);
      assert.ok(step.outputArtifactTargets.length > 0, `${step.stepId} should name output targets`);
    }
    for (const blocker of candidate.blockers) {
      if (blocker.partId) assert.ok(partIds.has(blocker.partId), `${blocker.code} references unknown part`);
      if (blocker.interfaceId) assert.ok(interfaceIds.has(blocker.interfaceId), `${blocker.code} references unknown interface`);
      if (blocker.stepId) assert.ok(stepIds.has(blocker.stepId), `${blocker.code} references unknown step`);
    }
  }

  const selected = result.candidates.find((candidate) => candidate.planId === result.selectedPlanId);
  assert.equal(selected.machineReady, false);
  assert.ok(selected.parts.some((part) => part.preferredProcess === 'fdm-print'));
  assert.ok(selected.parts.some((part) => part.preferredProcess === 'vertical-mill'));
  assert.ok(selected.parts.some((part) => part.preferredProcess === 'lathe-turning'));
  assert.ok(selected.interfaces.some((entry) => entry.joinMethod.includes('heat-set')));
  assert.ok(selected.processSteps.some((step) => step.operatorIntervention));
  assert.equal(result.machineReady, false);
  assert.ok(result.learningSignals.some((signal) => signal.signalKind === 'policy-action'));
  assert.ok(result.learningSignals.some((signal) => signal.signalKind === 'failure-boundary'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'blocks-machine-ready'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'needs-operator-review'));
  assertNoCredentialBearingUris(request, 'assemblyRequest');
  assertNoCredentialBearingUris(result, 'assemblyResult');
}

function assertLearningCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.learning-outcome.request.v1');
  assert.equal(result.schema, 'dd.fabrication.learning-outcome.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.learning.outcomes.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const sourceIds = new Set(request.sources.map((source) => source.refId));
  assert.ok(sourceIds.has(request.selectedPlanId), 'selected plan should be present as a learning source');
  assert.ok(request.observations.some((observation) => observation.outcome === 'success'));
  assert.ok(request.observations.some((observation) => observation.outcome === 'operator-intervention-required'));
  assert.ok(request.failureBoundaries.some((boundary) => boundary.humanInterventionRequired));
  assert.ok(request.rewardSignals.some((signal) => signal.value > 0));
  assert.ok(request.rewardSignals.some((signal) => signal.value < 0));

  const requestBoundaryIds = new Set(request.failureBoundaries.map((boundary) => boundary.boundaryId));
  for (const boundary of result.retainedBoundaries) {
    assert.ok(requestBoundaryIds.has(boundary.boundaryId), `${boundary.boundaryId} was not requested`);
  }
  for (const update of result.updates) {
    assert.equal(update.accepted, true, `${update.updateId} should be accepted in the fixture`);
    assert.ok(update.metrics.length > 0, `${update.updateId} should carry compact metrics`);
  }

  assert.equal(result.success, true);
  assert.ok(result.updates.some((update) => update.modelKind === 'mdp-policy'));
  assert.ok(result.updates.some((update) => update.modelKind === 'replay-buffer'));
  assert.ok(result.updates.some((update) => update.modelKind === 'failure-boundary-memory'));
  assert.ok(result.retainedBoundaries.some((boundary) => boundary.code === 'workholding-release-required'));
  assert.match(result.rewardSummary, /split printed shell plus milled insert/);
  assertNoCredentialBearingUris(request, 'learningRequest');
  assertNoCredentialBearingUris(result, 'learningResult');
}

function assertReleaseReadinessCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.release-readiness.request.v1');
  assert.equal(result.schema, 'dd.fabrication.release-readiness.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.release.readiness.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  assert.ok(request.candidatePlanId, 'release readiness request should name a selected candidate plan');
  assert.ok(
    request.evidenceRefs.some((evidence) => evidence.sourceRefId === request.candidatePlanId),
    'selected candidate plan should be present in release evidence refs',
  );

  const evidenceTokens = new Set();
  for (const evidence of request.evidenceRefs) {
    evidenceTokens.add(evidence.evidenceId);
    for (const label of evidence.labels) evidenceTokens.add(label);
  }
  for (const gate of request.machineGates) {
    for (const evidence of gate.satisfiedEvidence) {
      assert.ok(evidenceTokens.has(evidence), `${gate.gateId} satisfied unknown evidence ${evidence}`);
    }
  }

  const requestBlockerIds = new Set(request.knownBlockers.map((blocker) => blocker.blockerId));
  const resultBlockerIds = new Set(result.blockers.map((blocker) => blocker.blockerId));
  for (const blocker of result.blockers) {
    assert.ok(requestBlockerIds.has(blocker.blockerId), `${blocker.blockerId} was not requested`);
    assert.ok(blocker.evidenceRequired.length > 0, `${blocker.blockerId} should require release evidence`);
  }

  for (const decision of result.decisions) {
    for (const blockerId of decision.blockers) {
      assert.ok(resultBlockerIds.has(blockerId), `${decision.decisionId} references unknown blocker ${blockerId}`);
    }
  }

  const interventionEvidence = new Set();
  for (const intervention of result.humanInterventions) {
    for (const evidence of intervention.evidenceRequired) interventionEvidence.add(evidence);
  }
  assert.ok(
    result.blockers.some((blocker) => blocker.evidenceRequired.some((evidence) => interventionEvidence.has(evidence))),
    'at least one release blocker should require evidence from a human intervention',
  );

  assert.equal(result.success, true);
  assert.equal(result.machineReady, false);
  assert.ok(result.decisions.some((decision) => decision.machineReady === false));
  assert.ok(result.decisions.some((decision) => decision.releaseStatus === 'blocked'));
  assert.ok(result.blockers.some((blocker) => blocker.code === 'workholding-release-required'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'blocks-machine-ready'));
  assert.ok(result.humanInterventions.some((intervention) => intervention.required && intervention.status !== 'satisfied'));
  assert.ok(result.manifestArtifacts.some((artifact) => artifact.artifactKind === 'release-manifest'));
  assert.ok(result.manifestArtifacts.some((artifact) => artifact.format === 'GCODE'));
  assertNoCredentialBearingUris(request, 'releaseReadinessRequest');
  assertNoCredentialBearingUris(result, 'releaseReadinessResult');
}

async function main() {
  const designSynthesisSchemaDoc = await readJson('schema/fabrication-design-synthesis.schema.json');
  const designSynthesisRequest = await readJson('examples/fabrication-design-synthesis-request.json');
  const designSynthesisResult = await readJson('examples/fabrication-design-synthesis-result.json');
  const machineProfileSchemaDoc = await readJson('schema/fabrication-machine-profiles.schema.json');
  const machineProfileRequest = await readJson('examples/fabrication-machine-profile-request.json');
  const machineProfileResult = await readJson('examples/fabrication-machine-profile-result.json');
  const conversionSchemaDoc = await readJson('schema/fabrication-cad-conversion.schema.json');
  const conversionRequest = await readJson('examples/fabrication-design-conversion-request.json');
  const conversionResult = await readJson('examples/fabrication-design-conversion-result.json');
  const instructionSchemaDoc = await readJson('schema/fabrication-instruction-generation.schema.json');
  const instructionRequest = await readJson('examples/fabrication-instruction-generation-request.json');
  const instructionResult = await readJson('examples/fabrication-instruction-generation-result.json');
  const instructionReviewSchemaDoc = await readJson('schema/fabrication-instruction-review.schema.json');
  const instructionReviewRequest = await readJson('examples/fabrication-instruction-review-request.json');
  const instructionReviewResult = await readJson('examples/fabrication-instruction-review-result.json');
  const instructionSimulationSchemaDoc = await readJson('schema/fabrication-instruction-simulation.schema.json');
  const instructionSimulationRequest = await readJson('examples/fabrication-instruction-simulation-request.json');
  const instructionSimulationResult = await readJson('examples/fabrication-instruction-simulation-result.json');
  const assemblySchemaDoc = await readJson('schema/fabrication-assembly-planning.schema.json');
  const assemblyRequest = await readJson('examples/fabrication-assembly-planning-request.json');
  const assemblyResult = await readJson('examples/fabrication-assembly-planning-result.json');
  const learningSchemaDoc = await readJson('schema/fabrication-learning-outcomes.schema.json');
  const learningRequest = await readJson('examples/fabrication-learning-outcome-request.json');
  const learningResult = await readJson('examples/fabrication-learning-outcome-result.json');
  const releaseReadinessSchemaDoc = await readJson('schema/fabrication-release-readiness.schema.json');
  const releaseReadinessRequest = await readJson('examples/fabrication-release-readiness-request.json');
  const releaseReadinessResult = await readJson('examples/fabrication-release-readiness-result.json');

  validateSchemaValue(
    designSynthesisSchemaDoc,
    { $ref: '#/$defs/FabricationDesignSynthesisRequest' },
    designSynthesisRequest,
    'designSynthesisRequest',
  );
  validateSchemaValue(
    designSynthesisSchemaDoc,
    { $ref: '#/$defs/FabricationDesignSynthesisResult' },
    designSynthesisResult,
    'designSynthesisResult',
  );
  assertDesignSynthesisCorrelation(designSynthesisRequest, designSynthesisResult);

  validateSchemaValue(
    machineProfileSchemaDoc,
    { $ref: '#/$defs/FabricationMachineProfileRequest' },
    machineProfileRequest,
    'machineProfileRequest',
  );
  validateSchemaValue(
    machineProfileSchemaDoc,
    { $ref: '#/$defs/FabricationMachineProfileResult' },
    machineProfileResult,
    'machineProfileResult',
  );
  assertMachineProfileCorrelation(machineProfileRequest, machineProfileResult);

  validateSchemaValue(
    conversionSchemaDoc,
    { $ref: '#/$defs/FabricationDesignConversionRequest' },
    conversionRequest,
    'conversionRequest',
  );
  validateSchemaValue(
    conversionSchemaDoc,
    { $ref: '#/$defs/FabricationDesignConversionResult' },
    conversionResult,
    'conversionResult',
  );
  assertConversionCorrelation(conversionRequest, conversionResult);

  validateSchemaValue(
    instructionSchemaDoc,
    { $ref: '#/$defs/FabricationInstructionGenerationRequest' },
    instructionRequest,
    'instructionRequest',
  );
  validateSchemaValue(
    instructionSchemaDoc,
    { $ref: '#/$defs/FabricationInstructionGenerationResult' },
    instructionResult,
    'instructionResult',
  );
  assertInstructionCorrelation(instructionRequest, instructionResult);

  validateSchemaValue(
    instructionReviewSchemaDoc,
    { $ref: '#/$defs/FabricationInstructionReviewRequest' },
    instructionReviewRequest,
    'instructionReviewRequest',
  );
  validateSchemaValue(
    instructionReviewSchemaDoc,
    { $ref: '#/$defs/FabricationInstructionReviewResult' },
    instructionReviewResult,
    'instructionReviewResult',
  );
  assertInstructionReviewCorrelation(instructionReviewRequest, instructionReviewResult);

  validateSchemaValue(
    instructionSimulationSchemaDoc,
    { $ref: '#/$defs/FabricationInstructionSimulationRequest' },
    instructionSimulationRequest,
    'instructionSimulationRequest',
  );
  validateSchemaValue(
    instructionSimulationSchemaDoc,
    { $ref: '#/$defs/FabricationInstructionSimulationResult' },
    instructionSimulationResult,
    'instructionSimulationResult',
  );
  assertInstructionSimulationCorrelation(instructionSimulationRequest, instructionSimulationResult);

  validateSchemaValue(
    assemblySchemaDoc,
    { $ref: '#/$defs/FabricationAssemblyPlanningRequest' },
    assemblyRequest,
    'assemblyRequest',
  );
  validateSchemaValue(
    assemblySchemaDoc,
    { $ref: '#/$defs/FabricationAssemblyPlanningResult' },
    assemblyResult,
    'assemblyResult',
  );
  assertAssemblyCorrelation(assemblyRequest, assemblyResult);

  validateSchemaValue(
    learningSchemaDoc,
    { $ref: '#/$defs/FabricationLearningOutcomeRequest' },
    learningRequest,
    'learningRequest',
  );
  validateSchemaValue(
    learningSchemaDoc,
    { $ref: '#/$defs/FabricationLearningOutcomeResult' },
    learningResult,
    'learningResult',
  );
  assertLearningCorrelation(learningRequest, learningResult);

  validateSchemaValue(
    releaseReadinessSchemaDoc,
    { $ref: '#/$defs/FabricationReleaseReadinessRequest' },
    releaseReadinessRequest,
    'releaseReadinessRequest',
  );
  validateSchemaValue(
    releaseReadinessSchemaDoc,
    { $ref: '#/$defs/FabricationReleaseReadinessResult' },
    releaseReadinessResult,
    'releaseReadinessResult',
  );
  assertReleaseReadinessCorrelation(releaseReadinessRequest, releaseReadinessResult);

  console.log('fabrication worker examples validate against shared schemas.');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
