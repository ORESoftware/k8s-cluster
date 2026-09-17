#!/usr/bin/env node
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';

import {
  buildContractIr,
  runCheck,
} from '@oresoftware/typespec-json-schema-validator';
import {
  verifyLanguageBoundariesAgainstCurrentInputs,
} from '@oresoftware/typespec-json-schema-validator/language-boundary-current-inputs';

const root = resolve(import.meta.dirname, '../..');
const typespec = resolve(root, 'contracts/local-k8s-platform/main.tsp');
const authoredSchema = resolve(root, 'contracts/local-k8s-platform/authored.schema.json');
const manifestPath = resolve(root, 'contracts/local-k8s-platform/language-boundaries.json');
const sourceRevision = execFileSync('git', ['rev-parse', 'HEAD'], {
  cwd: root,
  encoding: 'utf8',
}).trim();

assert.match(sourceRevision, /^[0-9a-f]{40}$/u);

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function hasRule(result, ruleId) {
  return result.findings.some((finding) => finding.ruleId === ruleId);
}

const scratch = await mkdtemp(join(tmpdir(), 'den-1032-current-tjsv-hardening-'));
const generatedSchema = join(scratch, 'generated-schema');
const parityReport = await runCheck({
  typespec,
  authoredSchema,
  outputDir: generatedSchema,
  maxFindings: 250,
  maxProbes: 64,
});
assert.equal(parityReport.status, 'passed');
assert.equal(parityReport.zeroUnexplainedFindings, true);

const contractIr = await buildContractIr({
  report: parityReport,
  typespec,
  generatedSchema,
  authoredSchema,
});
assert.equal(contractIr.status, 'passed');
assert.equal(contractIr.admissible, true);

const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));

async function createEvidenceMap(currentManifest = manifest) {
  const evidenceByPath = new Map();
  for (const target of currentManifest.targets) {
    const artifact = Buffer.from(
      JSON.stringify({
        schema: 'ores.local-k8s.generated-runtime-adapter-evidence/v1',
        language: target.language,
        runtime: target.runtime,
        parityRunId: parityReport.runId,
        contractIrId: contractIr.irId,
      }, null, 2) + '\n',
      'utf8',
    );
    const artifactPath = join(scratch, 'runtime-artifacts', target.evidence);
    await mkdir(dirname(artifactPath), { recursive: true });
    await writeFile(artifactPath, artifact);
    evidenceByPath.set(target.evidence, {
      schema: 'ores.typespec-json-schema-validator.language-boundary-evidence/v1',
      language: target.language,
      runtime: target.runtime,
      status: 'passed',
      sourceRevision,
      artifactDigest: `sha256:${sha256(artifact)}`,
      receiptRunId: parityReport.runId,
      contractIrId: contractIr.irId,
      toolchain: {
        name: `${target.language}-${target.runtime}-contract-admission`,
        version: '1.0.0',
      },
      generator: {
        name: 'den-1032-current-tjsv-boundary-hardening',
        version: '1.0.0',
      },
      validation: { ingress: 'passed', egress: 'passed' },
    });
  }
  return evidenceByPath;
}

async function freshInput() {
  return {
    typespec,
    generatedSchema,
    authoredSchema,
    report: parityReport,
    contractIr,
    manifest: structuredClone(manifest),
    evidenceByPath: await createEvidenceMap(),
  };
}

const baseline = await verifyLanguageBoundariesAgainstCurrentInputs(await freshInput());
assert.equal(baseline.status, 'passed');
assert.equal(baseline.zeroUnexplainedFindings, true);
assert.equal(baseline.counts.admittedEvidence, 5);

const missingPathInput = await freshInput();
missingPathInput.authoredSchema = '';
const missingPath = await verifyLanguageBoundariesAgainstCurrentInputs(missingPathInput);
assert.equal(missingPath.status, 'stopped_for_evaluation');
assert.ok(hasRule(missingPath, 'boundary-current-input-path-missing'));
assert.equal(missingPath.counts.admittedEvidence, 0);

const tamperedIrInput = await freshInput();
tamperedIrInput.contractIr = structuredClone(contractIr);
tamperedIrInput.contractIr.irId = 'a'.repeat(64);
const tamperedIr = await verifyLanguageBoundariesAgainstCurrentInputs(tamperedIrInput);
assert.equal(tamperedIr.status, 'stopped_for_evaluation');
assert.ok(hasRule(tamperedIr, 'boundary-current-contract-ir-verification-failed'));
assert.equal(tamperedIr.counts.admittedEvidence, 0);

const identityInput = await freshInput();
identityInput.evidenceByPath.get('rust/native.json').language = 'go';
const identity = await verifyLanguageBoundariesAgainstCurrentInputs(identityInput);
assert.equal(identity.status, 'stopped_for_evaluation');
assert.ok(hasRule(identity, 'boundary-evidence-target-mismatch'));

const receiptInput = await freshInput();
receiptInput.evidenceByPath.get('typescript/node.json').receiptRunId = 'b'.repeat(64);
const receipt = await verifyLanguageBoundariesAgainstCurrentInputs(receiptInput);
assert.equal(receipt.status, 'stopped_for_evaluation');
assert.ok(hasRule(receipt, 'boundary-evidence-receipt-mismatch'));

const evidenceIrInput = await freshInput();
evidenceIrInput.evidenceByPath.get('dart/flutter.json').contractIrId = 'c'.repeat(64);
const evidenceIr = await verifyLanguageBoundariesAgainstCurrentInputs(evidenceIrInput);
assert.equal(evidenceIr.status, 'stopped_for_evaluation');
assert.ok(hasRule(evidenceIr, 'boundary-evidence-contract-ir-mismatch'));

const duplicateInput = await freshInput();
duplicateInput.manifest.targets[1].language = duplicateInput.manifest.targets[0].language;
duplicateInput.manifest.targets[1].runtime = duplicateInput.manifest.targets[0].runtime;
const duplicate = await verifyLanguageBoundariesAgainstCurrentInputs(duplicateInput);
assert.equal(duplicate.status, 'stopped_for_evaluation');
assert.ok(hasRule(duplicate, 'boundary-target-duplicate'));
assert.ok(duplicate.counts.distinctRequiredLanguages < duplicate.counts.requiredTargets);

const outputDirectory = process.env.TJSV_EVIDENCE_DIR;
if (outputDirectory) {
  await mkdir(outputDirectory, { recursive: true });
  await writeFile(
    join(outputDirectory, 'current-tjsv-language-boundary-hardening.json'),
    JSON.stringify({
      sourceRevision,
      parityRunId: parityReport.runId,
      contractIrId: contractIr.irId,
      baseline: baseline.status,
      negativeCases: {
        missingCurrentInputPath: missingPath.status,
        tamperedContractIr: tamperedIr.status,
        targetIdentityMismatch: identity.status,
        parityReceiptMismatch: receipt.status,
        evidenceContractIrMismatch: evidenceIr.status,
        duplicateTargetIdentity: duplicate.status,
      },
    }, null, 2) + '\n',
    'utf8',
  );
}

console.log('Current TJSV language-boundary hardening passed: provenance, receipt/IR, runtime identity, and duplicate-target mutations all stopped fail-closed');
