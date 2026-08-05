import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');

const roadmap = read('docs/durable-worker-roadmap.md');
const operatingModel = read('docs/durable-worker-project-operating-model.md');
const workflow = read('.github/workflows/durable-worker-project-docs.yml');

const forbiddenCredential = /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|contents:\s*write|persist-credentials:\s*true/;

test('roadmap preserves the independent service boundaries and effect contract', () => {
  for (const service of [
    'dd-agent-worker-broker',
    'dd-durable-worker-server',
    'dd-build-server',
  ]) {
    assert.match(roadmap, new RegExp(service));
  }
  assert.match(roadmap, /at least once/i);
  assert.match(roadmap, /idempotency key/i);
  assert.match(roadmap, /fencing token/i);
  assert.match(roadmap, /PR #714/);
  assert.match(roadmap, /PR #783/);
  assert.match(roadmap, /PR #791/);
  assert.match(roadmap, /PR #971/);
  assert.match(roadmap, /PR #999/);
});

test('roadmap records the merged Go SDK, artifact, and remaining M3 work', () => {
  assert.match(roadmap, /DEN-2289/);
  assert.match(roadmap, /dependency-free Go 1\.23\+/);
  assert.match(roadmap, /TypeScript, Python, and Go protocol fixture/);
  assert.match(roadmap, /durable-worker-go-sdk-a693040ad69a1f54f14dd65fb8b74ab11fee132b/);
  assert.match(roadmap, /sha256:b24060664d79c845c4b7370f4cabb5b0ac79b9a09fc5b00b45b596ad9948d78c/);
  for (const language of ['Rust;', 'Dart;', 'Gleam;', 'Erlang and Elixir interoperability']) {
    assert.match(roadmap, new RegExp(language));
  }
});

test('roadmap defines five gated milestones', () => {
  for (const milestone of [
    'Milestone M1',
    'Milestone M2',
    'Milestone M3',
    'Milestone M4',
    'Milestone M5',
  ]) {
    assert.match(roadmap, new RegExp(milestone));
  }
  assert.equal((roadmap.match(/Exit gate:/g) ?? []).length, 5);
  assert.match(roadmap, /resumable SSE/i);
  assert.match(roadmap, /continue-as-new/);
  assert.match(roadmap, /OpenTelemetry/);
  assert.match(roadmap, /Fiducia epochs/);
});

test('project operating model maps GitHub, Linear, artifacts, and exact-head delivery', () => {
  assert.match(operatingModel, /github\.com\/ORESoftware\/k8s-cluster/);
  assert.match(operatingModel, /github\.com\/ORESoftware\/k8s-cluster`/);
  assert.match(operatingModel, /DEN-1675/);
  assert.match(operatingModel, /DEN-2218/);
  assert.match(operatingModel, /DEN-2289/);
  assert.match(operatingModel, /PR #999/);
  assert.match(operatingModel, /source artifact published/);
  assert.match(operatingModel, /expected-head/i);
  assert.match(operatingModel, /semantic merge/i);
  assert.match(operatingModel, /Status \| single select/);
  assert.match(operatingModel, /Milestone \| single select/);
  assert.match(operatingModel, /Linear \| text/);
});

test('project docs track the private-backend CI blocker without recommending a PAT', () => {
  assert.match(roadmap, /DEN-2332/);
  assert.match(roadmap, /issue #886/);
  assert.match(operatingModel, /DEN-2332/);
  assert.match(operatingModel, /issue #886/);
  assert.match(operatingModel, /repository-scoped GitHub App/);
  assert.match(operatingModel, /not a user PAT/);
});

test('documentation and its CI contain no write credential or PAT shape', () => {
  assert.doesNotMatch(roadmap, forbiddenCredential);
  assert.doesNotMatch(operatingModel, forbiddenCredential);
  assert.doesNotMatch(workflow, forbiddenCredential);
});

test('documentation CI is pinned, read-only, and checks repository cleanliness', () => {
  assert.match(workflow, /permissions:\n  contents: read/);
  assert.match(workflow, /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/);
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /node --test remote\/tests\/general\/durable-worker-project-docs\.test\.mjs/);
  assert.match(workflow, /git diff --exit-code/);
});
