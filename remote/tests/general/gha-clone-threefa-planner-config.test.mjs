import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const config = read('remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml');
const manifest = read('remote/deployments/gha-clone-server-rs/Cargo.toml');
const planner = read('remote/deployments/gha-clone-server-rs/src/bounded_lib.rs');
const legacyPlanner = read('remote/deployments/gha-clone-server-rs/src/lib.rs');
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

test('crate routes through one hardened wrapper around the existing parser', () => {
  assert.match(manifest, /\[lib\][\s\S]*path = "src\/bounded_lib\.rs"/);
  assert.match(planner, /#\[path = "lib\.rs"\][\s\S]*mod legacy;/);
  assert.match(planner, /legacy::build_plan/);
  assert.match(planner, /legacy::capabilities/);
  assert.match(legacyPlanner, /fn classify_profile/);
  assert.doesNotMatch(legacyPlanner, /rust-generated-verify/);
});

test('planner advertises and compiles only reviewed hardened profiles', () => {
  for (const marker of [
    'rust-generated-verify',
    'node-hardened-verify',
    'node-hardened-test',
    'fn generated_rust_intent',
    'fn generated_rust_profile',
    'fn hardened_node_intent',
    'fn hardened_node_profile',
    'generated Rust jobs must use one exact reviewed command sequence',
    'hardened Node jobs must use one exact reviewed command sequence',
  ]) {
    assert.ok(planner.includes(marker), `planner is missing ${marker}`);
  }
  for (const command of [
    'npm ci --ignore-scripts',
    'npm test',
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

test('planner tests cover exact mappings, command mutations, and revision immutability', () => {
  assert.match(plannerTest, /node-hardened-test/);
  assert.match(plannerTest, /rust-generated-verify/);
  assert.match(plannerTest, /reordered/);
  assert.match(plannerTest, /cargo publish/);
  assert.match(plannerTest, /npm audit --audit-level=high/);
  assert.match(plannerTest, /revision: "main"/);
  assert.match(plannerTest, /exact 40-hex commit SHA/);
});

test('documentation and permanent workflow preserve the plan-only boundary', () => {
  assert.match(documentation, /does not enable the continuity service/);
  assert.match(documentation, /deployment remains at zero replicas/);
  assert.match(documentation, /later PR must prove real-process dispatch/);
  assert.match(documentation, /action-reference immutability is tracked separately/);
  assert.match(workflow, /remote\/deployments\/gha-clone-server-rs\/\*\*/);
  assert.match(workflow, /remote\/argocd\/dd-next-runtime\/dd-gha-clone-server\.configmap\.yaml/);
  assert.match(workflow, /remote\/tests\/general\/gha-clone-\*\.test\.(?:ts|mjs)/);
});
