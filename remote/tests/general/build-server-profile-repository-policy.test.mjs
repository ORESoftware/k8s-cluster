import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const config = read('remote/deployments/build-server-rs/src/config.rs');
const policy = read('remote/deployments/build-server-rs/src/config/profile_policy.rs');
const profiles = read('remote/deployments/build-server-rs/src/profiles.rs');
const validation = read('remote/deployments/build-server-rs/src/validation.rs');
const patch = read(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const workflow = read('.github/workflows/gha-clone-server.yml');
const documentation = read('docs/build-server-profile-repository-policy.md');

function profileRulesFromPatch() {
  const match = patch.match(
    /name:\s*BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON[\s\S]*?value:\s*>-\s*\n\s*(\[[^\n]+\])/,
  );
  assert.ok(match, 'profile repository JSON was not found in the continuity patch');
  return JSON.parse(match[1]);
}

function profileConstantBody(name) {
  const marker = `const ${name}: &[ProfileStep] = &[ProfileStep {`;
  const start = profiles.indexOf(marker);
  assert.ok(start >= 0, `${name} constant is missing`);
  const end = profiles.indexOf('\n}];', start);
  assert.ok(end > start, `${name} constant is unterminated`);
  return profiles.slice(start, end + 4);
}

test('startup compiles exact repository rules against the global profile registry', () => {
  assert.match(config, /pub\(crate\) mod profile_policy;/);
  assert.match(config, /BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON/);
  assert.match(config, /raw_env\("BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"\)/);
  assert.doesNotMatch(
    config,
    /first_env\(&\["BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON"\]\)/,
  );
  assert.match(config, /profile_policy::compile_rules/);
  assert.match(config, /invalid build-server profile repository policy/);
  assert.match(policy, /serde\(deny_unknown_fields\)/);
  assert.match(policy, /profiles::find/);
  assert.match(policy, /globally_allowed_profiles\.contains/);
  assert.match(policy, /MAX_POLICY_BYTES/);
  assert.match(policy, /raw_untrimmed\.len\(\) > MAX_POLICY_BYTES/);
  assert.match(
    policy,
    /raw_policy_accepts_the_exact_byte_limit_and_rejects_one_more_byte/,
  );
  assert.match(
    policy,
    /raw_policy_limit_counts_utf8_bytes_before_unicode_whitespace_trimming/,
  );
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

test('GitOps binds each reviewed repository to only its fixed profiles', () => {
  assert.match(patch, /name:\s*BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON/);
  const rules = profileRulesFromPatch();
  assert.deepEqual(rules, [
    {
      repository: 'https://github.com/ORESoftware/k8s-cluster.git',
      profiles: ['rust-verify'],
    },
    {
      repository: 'https://github.com/messaging-intel/msgint-connectors.git',
      profiles: ['node-hardened-verify', 'node-hardened-test'],
    },
    {
      repository: 'https://github.com/3FA-app/3fa-interfaces.git',
      profiles: ['node-hardened-test', 'rust-generated-verify'],
    },
  ]);
  assert.doesNotMatch(patch, /msgint-connectors[^\n]*node-verify/);
  assert.doesNotMatch(patch, /msgint-connectors[^\n]*(playwright|python-verify|rust-verify)/);
  assert.doesNotMatch(patch, /3fa-interfaces[^\n]*"rust-verify"/);
  assert.doesNotMatch(patch, /3fa-interfaces[^\n]*(playwright|python-verify|node-verify)/);
  assert.doesNotMatch(patch, /messaging-intel\/\*|3FA-app\/\*/);
});

test('generated Rust profile is fixed, ordered, locked, and non-publishing', () => {
  const generated = profileConstantBody('RUST_GENERATED_VERIFY_STEPS');
  assert.match(profiles, /name: "rust-generated-verify"/);
  assert.match(generated, /generated\/rust\/Cargo\.toml/);
  assert.match(generated, /cargo generate-lockfile --manifest-path/);
  assert.match(generated, /cargo fmt --manifest-path/);
  assert.match(generated, /cargo clippy --locked --manifest-path/);
  assert.match(generated, /cargo test --locked --manifest-path/);
  assert.match(generated, /-D warnings/);
  assert.doesNotMatch(generated, /cargo publish|find |curl|wget|\|\| true/);
});

test('hardened Node profiles disable lifecycle scripts and preserve reviewed order', () => {
  const verify = profileConstantBody('NODE_HARDENED_VERIFY_STEPS');
  const repositoryTest = profileConstantBody('NODE_HARDENED_TEST_STEPS');
  for (const [name, body] of [
    ['node-hardened-verify', verify],
    ['node-hardened-test', repositoryTest],
  ]) {
    assert.match(profiles, new RegExp(`name: "${name}"`));
    assert.match(body, /npm ci --ignore-scripts/);
    assert.doesNotMatch(body, /npm install|curl|wget|--force|\|\| true/);
  }
  assert.match(verify, /npm run check/);
  assert.match(verify, /npm run test:operator-config/);
  assert.match(verify, /npm audit --audit-level=high/);
  assert.match(repositoryTest, /npm test/);
});

test('dedicated GHA workflow formats and runs policy and profile tests', () => {
  assert.match(workflow, /src\/config\/profile_policy\.rs/);
  assert.match(workflow, /src\/config\.rs/);
  assert.match(workflow, /src\/validation\.rs/);
  assert.match(workflow, /config::profile_policy::tests/);
  assert.match(workflow, /profiles::tests/);
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

test('documentation states exact precedence, alias handling, and repository-specific rollout', () => {
  assert.match(documentation, /Exact rules override prefix fallback/);
  assert.match(documentation, /Invalid policy is a startup error/);
  assert.match(documentation, /lower-case `owner\/repository` identity/);
  assert.match(documentation, /SSH, case, optional `\.git`, or trailing-slash alias/);
  assert.match(documentation, /k8s-cluster\.git -> rust-verify/);
  assert.match(documentation, /msgint-connectors\.git -> node-hardened-verify, node-hardened-test/);
  assert.match(documentation, /3fa-interfaces\.git -> node-hardened-test, rust-generated-verify/);
  assert.match(documentation, /generated Rust crate/);
  assert.match(documentation, /lifecycle scripts disabled/);
  assert.match(documentation, /rejecting a downgrade of the same repository identity/);
});
