import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
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

test('exact policy canonicalizes aliases before broad prefix fallback', () => {
  assert.match(policy, /const EXACT_RULE_PREFIX: &str = "exact-id:"/);
  assert.match(policy, /github_repository_identity/);
  assert.match(policy, /to_ascii_lowercase/);
  assert.match(policy, /if let Some\(allowed_profiles\) = exact_profiles/);
  assert.match(policy, /not allowed for exact repository identity/);
  for (const evidence of [
    'exact_repository_rule_overrides_every_supported_alias',
    'exact_match_never_falls_back_to_a_broad_prefix',
    'unsafe_query_fragment_and_nested_paths_fail_before_prefix_fallback',
    'exact_policy_keys_require_canonical_https_git_urls',
    'duplicate_repository_identities_are_case_insensitive',
    'git@github.com:ORESoftware/k8s-cluster.git',
    'ssh://git@github.com/ORESoftware/k8s-cluster.git',
    'https://github.com/oresoftware/K8S-CLUSTER.git/',
  ]) {
    assert.ok(policy.includes(evidence), `missing canonical-identity evidence: ${evidence}`);
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

test('temporary branch-writing workflows are absent from the review surface', () => {
  for (const path of [
    '.github/workflows/format-build-server-profile-policy-once.yml',
    '.github/workflows/den-1550-profile-policy-finalize-once.yml',
    '.github/workflows/format-canonical-profile-policy-once.yml',
  ]) {
    assert.equal(existsSync(join(root, path)), false, `${path} must self-delete before review`);
  }
});

test('documentation states exact precedence, alias handling, and startup failure', () => {
  assert.match(documentation, /Exact rules override prefix fallback/);
  assert.match(documentation, /Invalid policy is a startup error/);
  assert.match(documentation, /lower-case `owner\/repository` identity/);
  assert.match(documentation, /SSH, case, optional `\.git`, or trailing-slash alias/);
  assert.match(documentation, /k8s-cluster\.git -> rust-verify/);
  assert.match(documentation, /rejecting a downgrade of the same repository identity/);
});
