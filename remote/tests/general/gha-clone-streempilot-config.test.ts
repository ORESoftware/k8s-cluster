import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const config = read(
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml',
);
const profiles = read('remote/deployments/build-server-rs/src/profiles.rs');
const apiFixture = read(
  'remote/deployments/gha-clone-server-rs/fixtures/streempilot-api-ci-mirror.yml',
);
const webFixture = read(
  'remote/deployments/gha-clone-server-rs/fixtures/streempilot-web-ci-mirror.yml',
);
const interfacesFixture = read(
  'remote/deployments/gha-clone-server-rs/fixtures/streempilot-interfaces-ci-mirror.yml',
);

const mirrorRepositories = [
  'StreemPilot/streempilot-api-server.rs',
  'StreemPilot/streempilot-web-server.rs',
  'StreemPilot/streempilot-interfaces',
];

const reviewedSetupActions = new Set([
  'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
  'actions/setup-node@820762786026740c76f36085b0efc47a31fe5020',
  'dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30',
]);

const fixtures = [apiFixture, webFixture, interfacesFixture];

test('StreemPilot repositories and mirror paths are exact allowlist entries', () => {
  for (const repository of mirrorRepositories) {
    assert.ok(config.includes(repository), `${repository} is not allowlisted`);
    const escaped = repository.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    assert.match(
      config,
      new RegExp(
        `"${escaped}": \\["\\.github/workflows/ci-mirror\\.yml"\\]`,
      ),
    );
  }
  assert.doesNotMatch(config, /StreemPilot\/\*|"StreemPilot"\s*:/);
  assert.doesNotMatch(config, /streempilot-(?:api|web).*ci\.yml/);
});

test('mirror fixtures match the reviewed workflow path and remain manual-only', () => {
  for (const fixture of fixtures) {
    assert.match(fixture, /^name: CI mirror/m);
    assert.match(fixture, /^on:\s*\n\s+workflow_dispatch:/m);
    assert.doesNotMatch(fixture, /^\s+(?:push|pull_request|schedule):/m);
    assert.doesNotMatch(fixture, /\$\{\{|secrets\.|permissions:|environment:/);
    assert.doesNotMatch(
      fixture,
      /strategy:|services:|container:|working-directory:|timeout-minutes:/,
    );
  }
});

test('mirror setup actions use only exact reviewed commit identities', () => {
  for (const fixture of fixtures) {
    const actions = [...fixture.matchAll(/^\s+- uses:\s+([^\s#]+)\s*$/gm)].map(
      (match) => match[1],
    );
    assert.ok(actions.length > 0, 'fixture has no setup actions');
    for (const action of actions) {
      assert.ok(reviewedSetupActions.has(action), `unreviewed action ${action}`);
      assert.match(action, /@[0-9a-f]{40}$/);
    }
    assert.doesNotMatch(fixture, /@(v\d+|main|master|stable)(?:\s|$)/m);
  }
});

test('API and web mirrors map only their core Rust verification', () => {
  for (const fixture of [apiFixture, webFixture]) {
    assert.match(fixture, /^\s+rust:/m);
    assert.match(fixture, /cargo fmt --all -- --check/);
    assert.match(fixture, /cargo check --all-targets --all-features/);
    assert.match(fixture, /cargo clippy --all-targets --all-features -- -D warnings/);
    assert.match(fixture, /cargo test --all-targets --all-features/);
    assert.doesNotMatch(fixture, /playwright|upload-artifact|flutter|dart/);
  }
});

test('interfaces mirror preserves Node contracts before generated Rust bindings', () => {
  assert.match(interfacesFixture, /^\s+contracts:/m);
  assert.match(interfacesFixture, /npm ci && npm test && npm run check:typescript/);
  assert.match(interfacesFixture, /^\s+rust-bindings:/m);
  assert.match(interfacesFixture, /needs: contracts/);
  assert.match(interfacesFixture, /generated\/rust\/Cargo\.toml/);
  assert.doesNotMatch(interfacesFixture, /dart analyze/);
});

test('fixed profiles support only the reviewed generated-interface shape', () => {
  assert.match(profiles, /generated\/rust\/Cargo\.toml/);
  assert.match(profiles, /generated\/rust\/Cargo\.lock/);
  assert.match(profiles, /schema\/domain\.schema\.json/);
  assert.match(profiles, /nats\/subjects\.json/);
  assert.match(profiles, /check:typescript/);
  assert.match(profiles, /npm ci/);
  assert.match(profiles, /cargo test --locked --all-targets --all-features/);
  assert.doesNotMatch(profiles, /find .*Cargo\.toml|for crate in/);
});
