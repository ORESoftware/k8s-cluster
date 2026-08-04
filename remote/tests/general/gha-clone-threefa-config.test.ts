import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dirname, '../../..');
const read = (path: string) => readFileSync(join(root, path), 'utf8');

const config = read(
  'remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml',
);
const buildPatch = read(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const workflow = read('.github/workflows/gha-clone-server.yml');
const planner = read('remote/deployments/gha-clone-server-rs/src/lib.rs');
const fixture = read(
  'remote/deployments/gha-clone-server-rs/tests/fixtures/threefa-interfaces-contracts.yml',
);
const processTest = read(
  'remote/deployments/gha-clone-server-rs/tests/threefa_interfaces.rs',
);
const profiles = read('remote/deployments/build-server-rs/src/profiles.rs');
const validation = read('remote/deployments/build-server-rs/src/validation.rs');
const admissionDoc = read('docs/gha-profile-repository-admission.md');
const continuityDoc = read('docs/gha-threefa-interfaces-continuity.md');

const repository = '3FA-app/3fa-interfaces';
const workflowPath = '.github/workflows/gha-clone-contracts.yml';
const revision = 'baea54bad288a36e36f6f484c1b5f2313bddfba8';

test('3FA repository, workflow, and profile admission are exact and additive', () => {
  assert.match(config, /3FA-app\/3fa-interfaces/);
  assert.match(
    config,
    /"3FA-app\/3fa-interfaces": \["\.github\/workflows\/gha-clone-contracts\.yml"\]/,
  );
  assert.match(config, /messaging-intel\/msgint-connectors/);
  assert.match(config, /sonus-auris\/sonus-auris-interfaces/);
  assert.doesNotMatch(config, /3FA-app\/\*|3fa-interfaces\/\*/);

  assert.match(
    buildPatch,
    /=https:\/\/github\.com\/3FA-app\/3fa-interfaces\.git/,
  );
  assert.match(
    admissionDoc,
    /=https:\/\/github\.com\/3FA-app\/3fa-interfaces\.git/,
  );
  assert.match(validation, /strip_prefix\('='\)/);
});

test('bounded workflow is immutable, credential free, and topologically exact', () => {
  assert.match(fixture, /^on:\n  workflow_dispatch:$/m);
  assert.match(
    fixture,
    /actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/g,
  );
  assert.match(
    fixture,
    /actions\/setup-node@820762786026740c76f36085b0efc47a31fe5020/,
  );
  assert.match(
    fixture,
    /dtolnay\/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30/,
  );
  assert.match(
    fixture,
    /node_contracts:[\s\S]*npm ci --ignore-scripts[\s\S]*npm test[\s\S]*generated_rust:[\s\S]*needs: node_contracts/,
  );
  assert.match(
    fixture,
    /cargo generate-lockfile --manifest-path generated\/rust\/Cargo\.toml[\s\S]*cargo fmt --manifest-path generated\/rust\/Cargo\.toml -- --check[\s\S]*cargo clippy --locked --manifest-path generated\/rust\/Cargo\.toml --all-targets -- -D warnings[\s\S]*cargo test --locked --manifest-path generated\/rust\/Cargo\.toml --all-targets/,
  );
  assert.doesNotMatch(
    fixture,
    /\benv:\s|\$\{\{\s*secrets|npm publish|cargo publish|pull_request_target|service[s]?:|container:/,
  );
});

test('generated Rust profile and planner use an exact command sequence', () => {
  assert.match(profiles, /name: "rust-generated-verify"/);
  assert.match(profiles, /const RUST_GENERATED_VERIFY_STEPS/);
  assert.match(profiles, /generated\/rust\/Cargo\.toml/);
  assert.match(profiles, /cargo generate-lockfile/);
  assert.match(profiles, /cargo clippy --locked/);

  assert.match(planner, /rust-generated-verify/);
  assert.match(planner, /generated_rust_intent/);
  assert.match(planner, /generated_rust_profile/);
  assert.match(planner, /generated Rust jobs must use one exact reviewed command sequence/);
  assert.match(planner, /exact 40-hex commit SHA/);
});

test('real-process proof pins one immutable source and refuses mutations', () => {
  assert.ok(processTest.includes(`const REVISION: &str = "${revision}"`));
  assert.ok(processTest.includes(`const REPOSITORY: &str = "${repository}"`));
  assert.ok(processTest.includes(`const WORKFLOW_PATH: &str = "${workflowPath}"`));
  assert.match(processTest, /node-hardened-test/);
  assert.match(processTest, /rust-generated-verify/);
  assert.match(processTest, /reordered generated Rust commands/);
  assert.match(processTest, /extra generated Rust command/);
  assert.match(processTest, /mutable Rust setup action/);
  assert.match(processTest, /3FA-app\/3fa-backend\.rs/);
  assert.match(processTest, /UNPROCESSABLE_ENTITY/);
});

test('normal CI and documentation preserve architecture and secret boundaries', () => {
  assert.match(workflow, /threefa-interfaces-contracts\.yml/);
  assert.match(workflow, /gha-clone-threefa-config\.test\.ts/);
  assert.match(workflow, /threefa_interfaces/);
  assert.match(workflow, /gha-clone-msgint-config\.test\.ts/);
  assert.match(workflow, /executor_router_http/);
  assert.match(continuityDoc, /TLA\+/);
  assert.match(continuityDoc, /official ARC/);
  assert.match(continuityDoc, /classic PAT/i);
  assert.doesNotMatch(
    [workflow, config, buildPatch, fixture, processTest, continuityDoc].join('\n'),
    /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY/,
  );
});
