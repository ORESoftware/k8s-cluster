import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const config = read('remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml');
const planner = read('remote/deployments/gha-clone-server-rs/src/lib.rs');
const fixture = read(
  'remote/deployments/gha-clone-server-rs/fixtures/threefa-interfaces-contracts.yml',
);
const plannerTest = read(
  'remote/deployments/gha-clone-server-rs/tests/threefa_planner.rs',
);
const documentation = read('docs/gha-threefa-interfaces-planner.md');
const workflow = read('.github/workflows/gha-clone-server.yml');

test('GitOps grants only the exact 3FA repository and workflow path', () => {
  assert.match(config, /3FA-app\/3fa-interfaces/);
  assert.match(config, /"3FA-app\/3fa-interfaces": \["\.github\/workflows\/gha-clone-contracts\.yml"\]/);
  assert.doesNotMatch(config, /3FA-app\/\*/);
  assert.doesNotMatch(config, /"3FA-app\/3fa-interfaces": \[[^\]]*ci\.yml/);
  assert.doesNotMatch(config, /3FA-app\/3fa-backend/);
});

test('planner advertises and compiles only the reviewed generated Rust sequence', () => {
  assert.match(planner, /"rust-generated-verify"\.to_string\(\)/);
  assert.match(planner, /fn generated_rust_intent/);
  assert.match(planner, /fn generated_rust_profile/);
  assert.match(planner, /generated Rust jobs must use one exact reviewed command sequence/);
  for (const command of [
    'cargo generate-lockfile --manifest-path generated/rust/Cargo.toml',
    'cargo fmt --manifest-path generated/rust/Cargo.toml -- --check',
    'cargo clippy --locked --manifest-path generated/rust/Cargo.toml --all-targets -- -D warnings',
    'cargo test --locked --manifest-path generated/rust/Cargo.toml --all-targets',
  ]) {
    assert.ok(planner.includes(command), `planner is missing ${command}`);
  }
  assert.doesNotMatch(planner, /generated_rust_profile[\s\S]{0,1600}cargo publish/);
});

test('fixture preserves Node before generated Rust and contains no authority-bearing features', () => {
  assert.match(fixture, /generated_rust:[\s\S]*needs: node_contracts/);
  assert.match(fixture, /npm ci --ignore-scripts[\s\S]*npm test/);
  assert.match(fixture, /cargo generate-lockfile[\s\S]*cargo fmt[\s\S]*cargo clippy[\s\S]*cargo test/);
  for (const forbidden of [
    '${{',
    'secrets.',
    'permissions:',
    'environment:',
    'services:',
    'container:',
    'strategy:',
    '@main',
    '@master',
    '@stable',
    'cargo publish',
  ]) {
    assert.ok(!fixture.includes(forbidden), `fixture contains ${forbidden}`);
  }
});

test('planner tests cover exact mapping and adversarial mutations', () => {
  assert.match(plannerTest, /node-hardened-test/);
  assert.match(plannerTest, /rust-generated-verify/);
  assert.match(plannerTest, /reordered/);
  assert.match(plannerTest, /cargo publish/);
  assert.match(plannerTest, /dtolnay\/rust-toolchain@stable/);
  assert.match(plannerTest, /revision: "main"/);
  assert.match(plannerTest, /exact 40-hex commit SHA/);
});

test('documentation and permanent workflow preserve the plan-only boundary', () => {
  assert.match(documentation, /does not enable the continuity service/);
  assert.match(documentation, /deployment remains at zero replicas/);
  assert.match(documentation, /later PR must prove real-process dispatch/);
  assert.match(workflow, /remote\/deployments\/gha-clone-server-rs\/\*\*/);
  assert.match(workflow, /remote\/argocd\/dd-next-runtime\/dd-gha-clone-server\.configmap\.yaml/);
  assert.match(workflow, /remote\/tests\/general\/gha-clone-\*\.test\.(?:ts|mjs)/);
});
