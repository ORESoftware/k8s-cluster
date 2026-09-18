import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');
const fleetPath = 'remote/worker-sdks/fleet-v1.json';
const fixturePath = 'remote/worker-sdks/fixtures/durable-worker-protocol-v1.json';
const workflowPath = '.github/workflows/durable-worker-sdk-fleet.yml';
const docsPath = 'docs/durable-worker-sdk-fleet.md';

const fleet = JSON.parse(read(fleetPath));
const fixture = JSON.parse(read(fixturePath));
const workflow = read(workflowPath);
const docs = read(docsPath);

test('fleet manifest is versioned and separates lifecycle SDKs from generated clients', () => {
  assert.equal(fleet.schema, 'ores.durable-worker-sdk-fleet/v1');
  assert.equal(fleet.version, 1);
  assert.equal(fleet.protocolFixture, fixturePath);
  assert.equal(fleet.lifecycleRoot, 'remote/worker-sdks');
  assert.equal(fleet.generatedClientRoot, 'remote/api-sdks');
  assert.notEqual(fleet.lifecycleRoot, fleet.generatedClientRoot);
});

test('fleet manifest covers the exact five landed hand-authored SDKs', () => {
  const expected = ['typescript', 'python', 'go', 'rust', 'dart'];
  assert.deepEqual(fleet.sdks.map((sdk) => sdk.language), expected);
  assert.deepEqual(fixture.handAuthoredSdks, expected);
  assert.equal(new Set(fleet.sdks.map((sdk) => sdk.language)).size, expected.length);

  for (const sdk of fleet.sdks) {
    assert.equal(sdk.root.startsWith(`${fleet.lifecycleRoot}/`), true, sdk.language);
    assert.equal(sdk.root.startsWith(`${fleet.generatedClientRoot}/`), false, sdk.language);
    assert.equal(existsSync(resolve(repoRoot, sdk.root, sdk.marker)), true, sdk.language);
    assert.equal(existsSync(resolve(repoRoot, sdk.root, 'README.md')), true, sdk.language);
    assert.equal(typeof sdk.minimumRuntime, 'string', sdk.language);
    assert.equal(typeof sdk.nativeCheck, 'string', sdk.language);
    assert.notEqual(sdk.nativeCheck.trim(), '', sdk.language);
  }
});

test('every fleet SDK declares the shared retry, transport, and fencing capabilities', () => {
  assert.deepEqual(fleet.requiredCapabilities, fixture.conformanceDimensions);
  for (const sdk of fleet.sdks) {
    assert.deepEqual(sdk.capabilities, fleet.requiredCapabilities, sdk.language);
  }
});

test('aggregate workflow is pinned, read-only, source-scoped, and report-preserving', () => {
  assert.match(workflow, /permissions:\n  contents: read/u);
  assert.match(workflow, /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/u);
  assert.match(workflow, /actions\/setup-node@820762786026740c76f36085b0efc47a31fe5020/u);
  assert.match(workflow, /actions\/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1/u);
  assert.match(workflow, /actions\/setup-go@924ae3a1cded613372ab5595356fb5720e22ba16/u);
  assert.match(workflow, /dtolnay\/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30/u);
  assert.match(workflow, /dart-lang\/setup-dart@65eb853c7ba17dde3be364c3d2858773e7144260/u);
  assert.match(workflow, /actions\/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02/u);
  assert.match(workflow, /persist-credentials: false/u);
  assert.match(workflow, /submodules: false/u);
  assert.doesNotMatch(workflow, /contents:\s*write|packages:\s*write|persist-credentials:\s*true/u);
  assert.match(workflow, /if: always\(\)/u);
  assert.match(workflow, /sdk-fleet-report\.json/u);
  assert.match(workflow, /sdk-fleet-report\.sha256/u);
  assert.match(workflow, /sort_keys=True/u);
  assert.match(workflow, /separators=\(',', ':'\)/u);
});

test('aggregate workflow executes every manifest language natively', () => {
  for (const language of fleet.sdks.map((sdk) => sdk.language)) {
    assert.match(workflow, new RegExp(`id: ${language}\\b`, 'u'), language);
  }
  assert.match(workflow, /node --test remote\/worker-sdks\/typescript\/durable-worker\/test\/client\.test\.mjs/u);
  assert.match(workflow, /python3 -m unittest discover/u);
  assert.match(workflow, /go test \.\/\.\.\. -race -count=1/u);
  assert.match(workflow, /cargo test --locked/u);
  assert.match(workflow, /dart analyze --fatal-infos --fatal-warnings/u);
});

test('fleet documentation defines additive language onboarding and artifact semantics', () => {
  assert.match(docs, /generated OpenAPI clients are not lifecycle SDKs/iu);
  assert.match(docs, /Add a language/iu);
  assert.match(docs, /fleet-v1\.json/u);
  assert.match(docs, /durable-worker-protocol-v1\.json/u);
  assert.match(docs, /Actions artifact/iu);
  assert.match(docs, /not a package-registry release/iu);
});
