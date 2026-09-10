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

const scratch = await mkdtemp(join(tmpdir(), 'den-1032-language-boundaries-'));
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
assert.equal(manifest.minimumDistinctLanguages, 5);
assert.equal(manifest.targets.length, 5);
assert.deepEqual(
  new Set(manifest.targets.map((target) => target.language)),
  new Set(['rust', 'typescript', 'dart', 'go', 'gleam']),
);

async function createEvidenceMap() {
  const evidenceByPath = new Map();
  for (const target of manifest.targets) {
    const artifact = Buffer.from(
      JSON.stringify(
        {
          schema: 'ores.local-k8s.generated-runtime-adapter-evidence/v1',
          language: target.language,
          runtime: target.runtime,
          parityRunId: parityReport.runId,
          contractIrId: contractIr.irId,
        },
        null,
        2,
      ) + '\n',
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
        name: 'den-1032-tjsv-language-boundary-contract',
        version: '1.0.0',
      },
      validation: { ingress: 'passed', egress: 'passed' },
    });
  }
  return evidenceByPath;
}

async function inputWithFreshEvidence() {
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

const passingInput = await inputWithFreshEvidence();
const passed = await verifyLanguageBoundariesAgainstCurrentInputs(passingInput);
assert.equal(passed.status, 'passed');
assert.equal(passed.zeroUnexplainedFindings, true);
assert.equal(passed.counts.targets, 5);
assert.equal(passed.counts.requiredTargets, 5);
assert.equal(passed.counts.distinctRequiredLanguages, 5);
assert.equal(passed.counts.admittedEvidence, 5);
assert.deepEqual(passed.findings, []);

const missingInput = await inputWithFreshEvidence();
missingInput.evidenceByPath.delete('gleam/beam.json');
const missing = await verifyLanguageBoundariesAgainstCurrentInputs(missingInput);
assert.equal(missing.status, 'stopped_for_evaluation');
assert.ok(hasRule(missing, 'boundary-required-evidence-missing'));

const staleInput = await inputWithFreshEvidence();
staleInput.evidenceByPath.get('go/native.json').sourceRevision = 'd'.repeat(40);
const stale = await verifyLanguageBoundariesAgainstCurrentInputs(staleInput);
assert.equal(stale.status, 'stopped_for_evaluation');
assert.ok(hasRule(stale, 'boundary-source-revision-mismatch'));

const ingressInput = await inputWithFreshEvidence();
ingressInput.evidenceByPath.get('typescript/node.json').validation.ingress = 'failed';
const ingress = await verifyLanguageBoundariesAgainstCurrentInputs(ingressInput);
assert.equal(ingress.status, 'stopped_for_evaluation');
assert.ok(hasRule(ingress, 'boundary-ingress-not-verified'));

const egressInput = await inputWithFreshEvidence();
egressInput.evidenceByPath.get('dart/flutter.json').validation.egress = 'failed';
const egress = await verifyLanguageBoundariesAgainstCurrentInputs(egressInput);
assert.equal(egress.status, 'stopped_for_evaluation');
assert.ok(hasRule(egress, 'boundary-egress-not-verified'));

const outputDirectory = process.env.TJSV_EVIDENCE_DIR;
if (outputDirectory) {
  await mkdir(outputDirectory, { recursive: true });
  await writeFile(
    join(outputDirectory, 'language-boundary-report.json'),
    JSON.stringify(
      {
        sourceRevision,
        parityRunId: parityReport.runId,
        contractIrId: contractIr.irId,
        passed,
        negativeCases: {
          missingRequiredRuntime: missing.status,
          staleSourceRevision: stale.status,
          ingressFailure: ingress.status,
          egressFailure: egress.status,
        },
      },
      null,
      2,
    ) + '\n',
    'utf8',
  );
}

console.log('TJSV five-runtime current-input boundary admission passed; negative cases stopped fail-closed');
