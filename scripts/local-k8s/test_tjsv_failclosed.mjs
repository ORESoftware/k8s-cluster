import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const root = process.cwd();
const tjsv = path.join(root, 'node_modules', '.bin', 'tjsv');
const evidenceDir = process.env.TJSV_EVIDENCE_DIR;
assert.ok(evidenceDir, 'TJSV_EVIDENCE_DIR is required');

const typeSpecPath = path.join(root, 'contracts/local-k8s-platform/main.tsp');
const schemaPath = path.join(root, 'contracts/local-k8s-platform/authored.schema.json');
const instancesPath = path.join(root, 'contracts/local-k8s-platform/instances');

function digest(filePath) {
  return createHash('sha256').update(readFileSync(filePath)).digest('hex');
}

function readReport(reportPath) {
  return JSON.parse(readFileSync(reportPath, 'utf8'));
}

function runCheck({ typespec = typeSpecPath, schema = schemaPath, report }) {
  return spawnSync(
    tjsv,
    [
      'check',
      `--typespec=${typespec}`,
      `--schema=${schema}`,
      `--instances=${instancesPath}`,
      `--report=${report}`,
      '--quiet',
    ],
    { cwd: root, encoding: 'utf8' },
  );
}

function assertStopped(result, reportPath, label) {
  if (result.error) throw result.error;
  assert.equal(result.status, 2, `${label} must stop evaluation with exit code 2`);
  const report = readReport(reportPath);
  assert.equal(report.status, 'stopped_for_evaluation', `${label} report must stop evaluation`);
  assert.ok(Array.isArray(report.findings) && report.findings.length > 0, `${label} must retain findings`);
}

const beforeTypeSpec = digest(typeSpecPath);
const beforeSchema = digest(schemaPath);

const originalReportPath = path.join(evidenceDir, 'tjsv-report.json');
const repeatReportPath = path.join(evidenceDir, 'tjsv-repeat.json');
const repeat = runCheck({ report: repeatReportPath });
if (repeat.error) throw repeat.error;
assert.equal(repeat.status, 0, 'repeat TJSV run must pass');
assert.deepEqual(
  readReport(repeatReportPath),
  readReport(originalReportPath),
  'identical inputs and toolchain must yield an identical deterministic receipt',
);

mkdirSync(path.join(root, 'tmp'), { recursive: true });
const scratch = mkdtempSync(path.join(root, 'tmp', 'den-1032-tjsv-'));

const authoredDriftPath = path.join(scratch, 'authored-drift.schema.json');
const authoredDrift = JSON.parse(readFileSync(schemaPath, 'utf8'));
const runtimeKind = authoredDrift?.$defs?.RuntimeKind?.enum;
assert.ok(Array.isArray(runtimeKind), 'RuntimeKind enum is required in authored schema');
runtimeKind.push('docker-only');
writeFileSync(authoredDriftPath, `${JSON.stringify(authoredDrift, null, 2)}\n`, 'utf8');
const authoredDriftReport = path.join(evidenceDir, 'tjsv-authored-drift.json');
assertStopped(
  runCheck({ schema: authoredDriftPath, report: authoredDriftReport }),
  authoredDriftReport,
  'authored JSON Schema drift',
);

const typeSpecDriftPath = path.join(scratch, 'typespec-drift.tsp');
const typeSpecSource = readFileSync(typeSpecPath, 'utf8');
const typeSpecNeedle = '  multipassK3s: "multipass-k3s",\n}';
assert.ok(typeSpecSource.includes(typeSpecNeedle), 'RuntimeKind TypeSpec enum shape changed unexpectedly');
writeFileSync(
  typeSpecDriftPath,
  typeSpecSource.replace(
    typeSpecNeedle,
    '  multipassK3s: "multipass-k3s",\n  dockerOnly: "docker-only",\n}',
  ),
  'utf8',
);
const typeSpecDriftReport = path.join(evidenceDir, 'tjsv-typespec-drift.json');
assertStopped(
  runCheck({ typespec: typeSpecDriftPath, report: typeSpecDriftReport }),
  typeSpecDriftReport,
  'TypeSpec drift',
);

assert.equal(digest(typeSpecPath), beforeTypeSpec, 'authored TypeSpec source must remain unchanged');
assert.equal(digest(schemaPath), beforeSchema, 'authored JSON Schema source must remain unchanged');

console.log('TJSV deterministic receipt and two-way fail-closed drift tests passed');
