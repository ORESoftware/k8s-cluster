#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..');
const matrixPath = resolve(
  repoRoot,
  'docs/security/production-safety-release-gate.json',
);
const evidencePath = resolve(
  repoRoot,
  'docs/security/automated-evidence/den-1391.json',
);

const errors = [];
const warnings = [];
const fail = (message) => errors.push(message);

function parseJson(name, text) {
  try {
    return JSON.parse(text);
  } catch (error) {
    fail(`${name} is not valid JSON: ${error.message}`);
    return {};
  }
}

function nonEmpty(value) {
  return typeof value === 'string' && value.trim().length > 0;
}

const [matrixText, evidenceText] = await Promise.all([
  readFile(matrixPath, 'utf8'),
  readFile(evidencePath, 'utf8'),
]);
const matrix = parseJson('Production safety matrix', matrixText);
const overlay = parseJson('Automated evidence overlay', evidenceText);

if (overlay.schema_version !== 1) {
  fail(`Expected evidence schema_version 1, received ${String(overlay.schema_version)}.`);
}
if (overlay.gate_issue !== 'DEN-1391') {
  fail(`Expected evidence gate_issue DEN-1391, received ${String(overlay.gate_issue)}.`);
}
if (!nonEmpty(overlay.purpose)) {
  fail('Automated evidence overlay must declare its bounded purpose.');
}

const allowedStatuses = new Set(overlay.status_policy?.allowed ?? []);
const forbiddenStatuses = new Set(overlay.status_policy?.forbidden ?? []);
for (const required of ['automated', 'live_evidence_pending']) {
  if (!allowedStatuses.has(required)) {
    fail(`Evidence status policy is missing allowed status ${required}.`);
  }
}
if (!forbiddenStatuses.has('passed')) {
  fail('Evidence status policy must explicitly forbid passed.');
}
if (!nonEmpty(overlay.status_policy?.note)) {
  fail('Evidence status policy must explain that this overlay cannot certify launch.');
}

const matrixRows = new Map(
  (Array.isArray(matrix.tests) ? matrix.tests : []).map((row) => [row.id, row]),
);
const entries = Array.isArray(overlay.evidence) ? overlay.evidence : [];
if (entries.length === 0) {
  fail('Automated evidence overlay must contain at least one passing evidence entry.');
}

const seenIds = new Set();
const statusCounts = new Map();
for (const [index, entry] of entries.entries()) {
  const label = entry?.id || `evidence[${index}]`;

  if (!/^[A-Z]+-\d{3}$/.test(entry?.id ?? '')) {
    fail(`${label}.id must be a stable gate ID such as AUTH-001.`);
  }
  if (seenIds.has(entry.id)) {
    fail(`Duplicate automated evidence entry ${entry.id}.`);
  }
  seenIds.add(entry.id);

  const matrixRow = matrixRows.get(entry.id);
  if (!matrixRow) {
    fail(`${label} does not exist in the canonical production safety matrix.`);
  }

  if (!allowedStatuses.has(entry.status)) {
    fail(`${label}.status ${String(entry.status)} is not allowed.`);
  }
  if (forbiddenStatuses.has(entry.status)) {
    fail(`${label}.status ${entry.status} is explicitly forbidden in this overlay.`);
  }
  statusCounts.set(entry.status, (statusCounts.get(entry.status) ?? 0) + 1);

  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(entry.repository ?? '')) {
    fail(`${label}.repository must be owner/name.`);
  }
  if (!Number.isInteger(entry.pull_request) || entry.pull_request <= 0) {
    fail(`${label}.pull_request must be a positive integer.`);
  }
  if (!/^[0-9a-f]{40}$/.test(entry.commit ?? '')) {
    fail(`${label}.commit must be a full 40-character lowercase SHA.`);
  }
  if (
    !new RegExp(
      `^https://github\\.com/${String(entry.repository).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}/actions/runs/\\d+$`,
    ).test(entry.workflow_run ?? '')
  ) {
    fail(`${label}.workflow_run must be a GitHub Actions run URL in its repository.`);
  }
  if (!nonEmpty(entry.workflow) || !entry.workflow.startsWith('.github/workflows/')) {
    fail(`${label}.workflow must name a repository workflow path.`);
  }
  if (!Array.isArray(entry.source_tests) || entry.source_tests.length === 0) {
    fail(`${label}.source_tests must be a non-empty array.`);
  } else {
    for (const path of entry.source_tests) {
      if (!nonEmpty(path) || !path.startsWith('tests/')) {
        fail(`${label}.source_tests contains invalid test path ${String(path)}.`);
      }
    }
  }
  if (!nonEmpty(entry.environment)) {
    fail(`${label}.environment must describe what actually executed.`);
  }
  if (!Array.isArray(entry.assertions) || entry.assertions.length === 0) {
    fail(`${label}.assertions must be a non-empty array.`);
  } else if (entry.assertions.some((assertion) => !nonEmpty(assertion))) {
    fail(`${label}.assertions contains an empty assertion.`);
  }
  if (!Array.isArray(entry.limitations) || entry.limitations.length === 0) {
    fail(`${label}.limitations must be non-empty so automation cannot be mistaken for certification.`);
  } else if (entry.limitations.some((limitation) => !nonEmpty(limitation))) {
    fail(`${label}.limitations contains an empty limitation.`);
  }
  if (Number.isNaN(Date.parse(entry.executed_at ?? ''))) {
    fail(`${label}.executed_at must be an ISO-8601 timestamp.`);
  }

  if (matrixRow?.status === 'passed') {
    warnings.push(
      `${label} is passed in the canonical matrix; verify the exact-candidate evidence bundle remains authoritative rather than this automation overlay.`,
    );
  }
  if (entry.status === 'automated' && matrixRow?.status === 'passed') {
    fail(`${label} cannot remain merely automated while the canonical row is passed without a documented evidence transition.`);
  }
}

console.log(
  `Validated ${entries.length} automated evidence entries against ${matrixRows.size} canonical gate rows.`,
);
console.log(
  `Evidence statuses: ${[...statusCounts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([status, count]) => `${status}=${count}`)
    .join(', ') || 'none'}.`,
);
for (const warning of warnings) {
  console.warn(`WARN: ${warning}`);
}

if (errors.length > 0) {
  console.error(`\nAutomated evidence validation failed with ${errors.length} error(s):`);
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exitCode = 1;
} else {
  console.log(
    'Automated evidence is structurally valid and explicitly remains below production certification.',
  );
}
