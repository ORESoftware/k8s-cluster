// Model-level hardening lints for the shared NATS subject contract.
//
// conformance.test.mjs proves that every generated language target agrees with
// the schema model. These tests instead harden the schema model itself: they
// lock in operational invariants (JetStream stream disjointness, stream-binding
// hygiene) and naming conventions that the generator does not enforce, so a new
// schema file cannot silently regress them.

import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

import { buildModel } from './generate.mjs';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const schemaRoot = path.join(packageRoot, 'schema');

// Subject literal tokens: lowercase alphanumerics joined by '-' or '_'.
const SUBJECT_TOKEN = /^[a-z0-9]+(?:[-_][a-z0-9]+)*$/;
// Every subject lives in an approved namespace (its first token). Extend this
// set deliberately when a new top-level namespace is introduced — do not let a
// typo in a schema file mint one by accident.
const ALLOWED_SUBJECT_ROOTS = new Set(['dd', 'presence']);
// JetStream stream names: UPPER_SNAKE.
const STREAM_NAME = /^[A-Z][A-Z0-9_]*$/;
// Queue group values: dd-prefixed lowercase kebab.
const QUEUE_GROUP_VALUE = /^dd(?:-[a-z0-9]+)+$/;

// Subjects that are covered by a JetStream stream's subject space but predate
// the `stream` binding field and do not declare it. Do not add entries here:
// new subjects placed inside a stream's subject space must declare
// `"stream": "<NAME>"` so consumers can rely on the binding. Remove entries as
// the legacy schemas gain their bindings.
const LEGACY_UNBOUND_STREAM_COVERED_SUBJECTS = new Set([
  'BuildServerRequests',
  'FabricationAssemblyPlanningRequests',
  'FabricationAssemblyPlanningResults',
  'FabricationDesignConversionRequests',
  'FabricationDesignConversionResults',
  'FabricationDesignSynthesisRequests',
  'FabricationDesignSynthesisResults',
  'FabricationExecutionTelemetryRequests',
  'FabricationExecutionTelemetryResults',
  'FabricationInstructionGenerationRequests',
  'FabricationInstructionGenerationResults',
  'FabricationInstructionReviewRequests',
  'FabricationInstructionReviewResults',
  'FabricationInstructionSimulationRequests',
  'FabricationInstructionSimulationResults',
  'FabricationLearningOutcomeRequests',
  'FabricationLearningOutcomeResults',
  'FabricationMachineProfileRequests',
  'FabricationMachineProfileResults',
  'FabricationReleaseReadinessRequests',
  'FabricationReleaseReadinessResults',
  'FabricationRequests',
  'FabricationResults',
]);

async function loadSchemaModel() {
  const index = JSON.parse(await readFile(path.join(schemaRoot, 'index.json'), 'utf8'));
  const schemas = await Promise.all(index.schemas.map(async (filename) => ({
    filename,
    doc: JSON.parse(await readFile(path.join(schemaRoot, filename), 'utf8')),
  })));
  return buildModel(schemas);
}

// True when two subscription filters (each may contain '*' and a trailing '>')
// can both match at least one concrete subject.
function filtersOverlap(a, b) {
  const tokensA = a.split('.');
  const tokensB = b.split('.');
  for (let i = 0; i < Math.max(tokensA.length, tokensB.length); i += 1) {
    const tokenA = tokensA[i];
    const tokenB = tokensB[i];
    if (tokenA === '>' || tokenB === '>') return true;
    if (tokenA === undefined || tokenB === undefined) return false;
    if (tokenA !== '*' && tokenB !== '*' && tokenA !== tokenB) return false;
  }
  return true;
}

function sampleSubject(subject) {
  return subject.kind === 'static'
    ? subject.subject
    : subject.pattern.replace(/\{[A-Za-z_][A-Za-z0-9_]*\}/g, 'sampletoken');
}

test('JetStream stream subject spaces are pairwise disjoint', async () => {
  // nats-server refuses to create a stream whose subject filters overlap an
  // existing stream's, so an overlap introduced in a schema file would only
  // surface at deploy time. Catch it here instead.
  const model = await loadSchemaModel();
  assert.ok(model.streams.length > 1, 'expected multiple streams to compare');
  for (let i = 0; i < model.streams.length; i += 1) {
    for (let j = i + 1; j < model.streams.length; j += 1) {
      const left = model.streams[i];
      const right = model.streams[j];
      for (const filterA of left.subjects) {
        for (const filterB of right.subjects) {
          assert.ok(!filtersOverlap(filterA, filterB),
            `streams ${left.name} (${filterA}) and ${right.name} (${filterB}) claim overlapping subject space; `
            + 'nats-server will reject creating the second stream');
        }
      }
    }
  }
});

test('subjects, streams, and queue groups follow the shared naming conventions', async () => {
  const model = await loadSchemaModel();

  for (const subject of model.subjects) {
    const raw = subject.kind === 'static' ? subject.subject : subject.pattern;
    const tokens = raw.split('.');
    for (const token of tokens) {
      if (/^\{[A-Za-z_][A-Za-z0-9_]*\}$/.test(token) || token === '*' || token === '>') continue;
      assert.match(token, SUBJECT_TOKEN,
        `${subject.name}: subject token ${JSON.stringify(token)} in ${JSON.stringify(raw)} `
        + 'must be lowercase alphanumerics joined by - or _');
    }
    const root = tokens[0];
    if (!/^\{[A-Za-z_][A-Za-z0-9_]*\}$/.test(root)) {
      assert.ok(ALLOWED_SUBJECT_ROOTS.has(root),
        `${subject.name}: subject root ${JSON.stringify(root)} is not an approved namespace `
        + `(${[...ALLOWED_SUBJECT_ROOTS].join(', ')}); extend ALLOWED_SUBJECT_ROOTS deliberately if intended`);
    }
  }

  for (const stream of model.streams) {
    assert.match(stream.name, STREAM_NAME,
      `stream ${JSON.stringify(stream.name)} must be UPPER_SNAKE`);
  }

  const queueGroupValues = [
    ...model.queueGroups.map((queueGroup) => [queueGroup.name, queueGroup.value]),
    ...model.subjects.filter((subject) => subject.queueGroup)
      .map((subject) => [subject.name, subject.queueGroup]),
  ];
  for (const [owner, value] of queueGroupValues) {
    assert.match(value, QUEUE_GROUP_VALUE,
      `${owner}: queue group value ${JSON.stringify(value)} must be dd-prefixed lowercase kebab`);
  }
});

test('subjects inside a stream subject space declare their stream binding', async () => {
  const model = await loadSchemaModel();

  const covered = new Map();
  for (const subject of model.subjects) {
    const rendered = sampleSubject(subject);
    for (const stream of model.streams) {
      if (stream.subjects.some((filter) => filtersOverlap(filter, rendered))) {
        covered.set(subject.name, { subject, stream });
        // Streams are pairwise disjoint (asserted above), so at most one match.
        break;
      }
    }
  }

  for (const [name, { subject, stream }] of covered) {
    if (subject.stream === stream.name) continue;
    assert.ok(LEGACY_UNBOUND_STREAM_COVERED_SUBJECTS.has(name),
      `${name} (${sampleSubject(subject)}) is covered by stream ${stream.name} but declares `
      + `${subject.stream ? `stream ${subject.stream}` : 'no stream binding'}; `
      + `add "stream": "${stream.name}" to the subject definition`);
  }

  // Keep the legacy allowlist honest: entries must still exhibit the gap, so
  // fixing a schema forces the entry to be deleted here.
  for (const name of LEGACY_UNBOUND_STREAM_COVERED_SUBJECTS) {
    const entry = covered.get(name);
    assert.ok(entry && entry.subject.stream !== entry.stream.name,
      `${name} no longer needs its LEGACY_UNBOUND_STREAM_COVERED_SUBJECTS entry; remove it`);
  }
});
