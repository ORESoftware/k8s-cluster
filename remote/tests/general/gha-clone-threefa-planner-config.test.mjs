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
const httpTest = read(
  'remote/deployments/gha-clone-server-rs/tests/threefa_http.rs',
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
  assert.match(planner, /const THREEFA_REPOSITORY: &str = "3FA-app\/3fa-interfaces"/);
  assert.match(planner, /const THREEFA_WORKFLOW_PATH: &str = "\.github\/workflows\/gha-clone-contracts\.yml"/);
  assert.match(planner, /if !is_threefa_bounded_workflow\(request\)/);
  assert.match(planner, /StreemPilot\/streempilot-interfaces/);
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

test('exact 3FA setup actions are an ordered pinned allowlist', () => {
  for (const action of [
    'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
    'actions/setup-node@820762786026740c76f36085b0efc47a31fe5020',
    'dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30',
  ]) {
    assert.ok(planner.includes(action), `planner is missing ${action}`);
    assert.ok(fixture.includes(action), `fixture is missing ${action}`);
  }
  assert.match(planner, /const NODE_ACTIONS: \[&str; 2\]/);
  assert.match(planner, /const GENERATED_RUST_ACTIONS: \[&str; 2\]/);
  assert.match(planner, /fn enforce_exact_actions/);
  assert.match(planner, /exact reviewed pinned action sequence with no extra actions/);
  assert.doesNotMatch(fixture, /@(main|master|stable|latest)\b/);
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
    'cargo publish',
  ]) {
    assert.ok(!fixture.includes(forbidden), `fixture contains ${forbidden}`);
  }
});

test('planner tests cover command, action, and revision mutations', () => {
  assert.match(plannerTest, /node-hardened-test/);
  assert.match(plannerTest, /rust-generated-verify/);
  assert.match(plannerTest, /reordered/);
  assert.match(plannerTest, /cargo publish/);
  assert.match(plannerTest, /npm audit --audit-level=high/);
  assert.match(plannerTest, /actions\/setup-node@main/);
  assert.match(plannerTest, /dtolnay\/rust-toolchain@stable/);
  assert.match(plannerTest, /owner\/extra-action@0123456789abcdef0123456789abcdef01234567/);
  assert.match(plannerTest, /revision: "main"/);
  assert.match(plannerTest, /exact 40-hex commit SHA/);
});

test('real-process proof exercises ordered dispatch and zero-submission action rejection', () => {
  assert.match(httpTest, /CARGO_BIN_EXE_gha-clone-server/);
  assert.match(httpTest, /route\("\/builds", post\(mock_submit\)\)/);
  assert.match(httpTest, /x-build-server-auth/);
  assert.match(httpTest, /node_contracts[\s\S]*node-hardened-test/);
  assert.match(httpTest, /generated_rust[\s\S]*rust-generated-verify/);
  assert.match(httpTest, /gha-clone/);
  assert.match(httpTest, /exact_retry_reuses_each_deterministic_build_request_identity/);
  assert.match(httpTest, /actions\/setup-node@main/);
  assert.match(httpTest, /owner\/extra-action@0123456789abcdef0123456789abcdef01234567/);
  assert.match(httpTest, /assert_rejected_without_dispatch/);
  assert.match(httpTest, /submissions\.lock\(\)\.await\.is_empty\(\)/);
  assert.doesNotMatch(httpTest, /GHA_CLONE_GITHUB_TOKEN",\s*[^)]/);
});

test('documentation and permanent workflow preserve exact action and inactive live boundaries', () => {
  assert.match(documentation, /Exact setup-action authority/);
  assert.match(documentation, /mutable ref such as `@main` or `@stable`/);
  assert.match(documentation, /actual `gha-clone-server` binary/);
  assert.match(documentation, /zero replicas/);
  assert.match(documentation, /not a live private-source run/);
  assert.match(documentation, /least-privilege GitHub App/);
  assert.match(workflow, /remote\/deployments\/gha-clone-server-rs\/\*\*/);
  assert.match(workflow, /remote\/tests\/general\/gha-clone-\*\.test\.(?:ts|mjs)/);
});
