import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const config = read('remote/deployments/build-server-rs/src/config.rs');
const policy = read('remote/deployments/build-server-rs/src/config/profile_policy.rs');
const validation = read('remote/deployments/build-server-rs/src/validation.rs');
const patch = read(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const workflow = read('.github/workflows/gha-clone-server.yml');
const documentation = read('docs/build-server-profile-repository-policy.md');

test('startup compiles exact repository rules against the global profile registry', () => {
  assert.match(config, /pub\(crate\) mod profile_policy;/);
  assert.match(config, /BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON/);
  assert.match(config, /profile_policy::compile_rules/);
  assert.match(config, /invalid build-server profile repository policy/);
  assert.match(policy, /serde\(deny_unknown_fields\)/);
  assert.match(policy, /profiles::find/);
  assert.match(policy, /globally_allowed_profiles\.contains/);
  assert.match(policy, /MAX_POLICY_BYTES/);
  assert.match(policy, /MAX_EXACT_REPOSITORIES/);
  assert.match(policy, /MAX_PROFILES_PER_REPOSITORY/);
});

test('request validation performs an exact profile decision after global admission', () => {
  const globalGate = validation.indexOf('BUILD_SERVER_ALLOWED_PROFILES');
  const repositoryGate = validation.indexOf(
    'profile_policy::ensure_repository_profile_allowed',
  );
  assert.ok(globalGate >= 0, 'global profile gate is missing');
  assert.ok(repositoryGate > globalGate, 'exact repository gate must follow global profile admission');
  assert.doesNotMatch(
    validation,
    /ensure_allowed_prefix\(\s*"profile repoUrl"/,
    'profile requests must not bypass exact rules through raw prefix matching',
  );
});

test('exact policy overrides broad prefix fallback and rejects lookalikes', () => {
  assert.match(policy, /if let Some\(allowed_profiles\) = exact_profiles/);
  assert.match(policy, /not allowed for exact repository/);
  assert.match(policy, /exact_repository_rule_overrides_broad_prefix_fallback/);
  for (const evidence of [
    'k8s-cluster.git-evil',
    'k8s-cluster-extra.git',
    'git@github.com:ORESoftware/k8s-cluster.git',
    'exact_rule_compile_rejects_query_injection',
    'exact_rule_compile_rejects_fragment_injection',
    'profile_name_validation_rejects_separator_injection',
  ]) {
    assert.ok(policy.includes(evidence), `missing negative-policy evidence: ${evidence}`);
  }
});

test('GitOps binds k8s-cluster only to rust-verify', () => {
  assert.match(patch, /name:\s*BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON/);
  const jsonMatch = patch.match(/\[\{\"repository\":\"([^\"]+)\",\"profiles\":\[(.*?)\]\}\]/);
  assert.ok(jsonMatch, 'profile repository JSON was not found in the continuity patch');
  assert.equal(jsonMatch[1], 'https://github.com/ORESoftware/k8s-cluster.git');
  assert.equal(jsonMatch[2], '\"rust-verify\"');
  assert.doesNotMatch(patch, /node-verify.*k8s-cluster|k8s-cluster.*node-verify/);
});

test('dedicated GHA workflow formats and runs policy tests and static contracts', () => {
  assert.match(workflow, /src\/config\/profile_policy\.rs/);
  assert.match(workflow, /src\/config\.rs/);
  assert.match(workflow, /src\/validation\.rs/);
  assert.match(workflow, /config::profile_policy::tests/);
  assert.match(workflow, /build-server-profile-repository-policy\.test\.mjs/);
  assert.match(workflow, /dd-build-server-gha-continuity\.patch\.yaml/);
  assert.match(workflow, /docs\/build-server-profile-repository-policy\.md/);
});

test('documentation states exact-match precedence and startup failure behavior', () => {
  assert.match(documentation, /Exact rules override prefix fallback/);
  assert.match(documentation, /Invalid policy is a startup error/);
  assert.match(documentation, /k8s-cluster\.git -> rust-verify/);
  assert.match(documentation, /rejecting a downgrade to `node-verify`/);
});
