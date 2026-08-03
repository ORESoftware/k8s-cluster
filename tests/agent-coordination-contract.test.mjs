import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';

const page = await readFile(new URL('../src/pages/index.astro', import.meta.url), 'utf8');
const css = await readFile(new URL('../src/styles/global.css', import.meta.url), 'utf8');

test('agent pitch separates durable data, coordination, and worker ownership', () => {
  const layers = [
    'Durable workflow layer',
    'Fiducia coordination layer',
    'Agent worker layer',
  ];
  let previous = -1;
  for (const layer of layers) {
    const position = page.indexOf(layer);
    assert.ok(position > previous, `missing or out-of-order ownership layer: ${layer}`);
    previous = position;
  }

  for (const durableStore of ['Postgres', 'queue', 'object store', 'vector DB']) {
    assert.match(page, new RegExp(durableStore, 'i'));
  }
  for (const primitive of ['Leases', 'fencing', 'elections', 'quotas', 'watches']) {
    assert.match(page, new RegExp(primitive, 'i'));
  }
});

test('agent pitch explicitly limits Fiducia to coordination rather than reasoning', () => {
  assert.match(page, /Coordination, not cognition/);
  assert.match(page, /does not plan tasks, choose models/);
  assert.match(page, /prevent hallucinations/);
  assert.match(page, /Exactly-once external effects still require/);
  assert.match(page, /fencing token or stable idempotency key/);
});

test('sample keeps claim renewal and fenced commit in safety order', () => {
  const claim = page.indexOf('fiducia.claim');
  const renew = page.indexOf('fiducia.renew');
  const commit = page.indexOf('commit_if_fence_is_current');
  assert.ok(claim >= 0, 'missing task claim');
  assert.ok(renew > claim, 'lease renewal must follow claim');
  assert.ok(commit > renew, 'fenced durable commit must follow renewal');
});

test('reference implementation and responsive ownership layout stay linked', () => {
  assert.match(
    page,
    /https:\/\/github\.com\/fiducia-cloud\/fiducia-ai-agent-manager\.rs\/tree\/main\/tools\/reference-fleet/,
  );
  assert.match(css, /\.agent-boundary__layers\s*\{[^}]*grid-template-columns: repeat\(3, 1fr\)/s);
  assert.match(
    css,
    /@media \(max-width: 880px\)[\s\S]*\.agent-boundary__layers\s*\{\s*grid-template-columns: 1fr;/,
  );
});
