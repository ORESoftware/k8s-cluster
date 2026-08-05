import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (
      existsSync(
        resolve(candidate, 'remote/deployments/build-server-rs/src/gha_workflow.rs'),
      )
    ) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const readRepoFile = (path: string): Promise<string> =>
  readFile(resolve(repoRoot, path), 'utf8');

test('the production server mounts authenticated workflow routes', async () => {
  const main = await readRepoFile('remote/deployments/build-server-rs/src/main.rs');
  const source = await readRepoFile(
    'remote/deployments/build-server-rs/src/gha_workflow.rs',
  );

  assert.match(main, /mod gha_workflow;/);
  assert.match(main, /build_router\(state\.clone\(\)\)\.merge\(gha_workflow::router\(state\)\)/);
  for (const route of [
    '/gha/workflows/capabilities',
    '/gha/workflows/plan',
    '/gha/workflows/runs',
    '/gha/workflows/runs/:run_id',
  ]) {
    assert.ok(source.includes(`"${route}"`), `missing production route ${route}`);
  }
  assert.match(source, /require_auth\(&headers, &state\.build\)/);
});

test('workflow YAML is bounded before it can reach fixed-profile execution', async () => {
  const source = await readRepoFile(
    'remote/deployments/build-server-rs/src/gha_workflow.rs',
  );

  for (const boundary of [
    'max_yaml_bytes',
    'max_jobs',
    'max_steps_per_job',
    'max_yaml_nodes',
    'max_yaml_depth',
    'YAML tags are unsupported',
    'workflowYaml must not contain NUL bytes',
    'workflowPath must stay under .github/workflows',
    'workflow job dependency graph contains a cycle',
    'unknown dependency',
  ]) {
    assert.ok(source.includes(boundary), `missing fail-closed boundary: ${boundary}`);
  }
  assert.match(source, /serde_yaml::from_str/);
  assert.match(source, /validate_yaml_shape/);
  assert.match(source, /is_full_commit_sha/);
});

test('the compiler emits only immutable fixed-profile build requests', async () => {
  const source = await readRepoFile(
    'remote/deployments/build-server-rs/src/gha_workflow.rs',
  );

  assert.match(source, /job_kind: Some\("run-profile"\.to_string\(\)\)/);
  assert.match(source, /repo_url: format!\("https:\/\/github\.com\/\{\}\.git", plan\.repository\)/);
  assert.match(source, /git_ref: Some\(plan\.revision\.clone\(\)\)/);
  assert.match(source, /profile: job\.profile\.clone\(\)/);
  assert.match(
    source,
    /request_id:\s*Some\(format!\(\s*"gha:\{\}:\{\}",\s*plan\.plan_id,\s*job\.id\s*\)\s*\)/s,
  );
  assert.match(source, /validate_build_request\(&state\.build\.config, &request\)/);
  assert.match(source, /enqueue_build\(&state\.build, build_request, "gha-yaml"\)/);
  assert.doesNotMatch(source, /command:\s|runner_image|build_args: Some|deploy: Some/);
});

test('unsupported GitHub Actions semantics fail closed instead of being approximated', async () => {
  const source = await readRepoFile(
    'remote/deployments/build-server-rs/src/gha_workflow.rs',
  );

  assert.match(source, /for key in job\.keys\(\)/);
  assert.match(source, /Some\("name" \| "needs" \| "runs-on" \| "steps"\) => \{\}/);
  assert.match(source, /job-level \{key\} is unsupported by the independent worker/);
  assert.match(source, /for key in step\.keys\(\)/);
  assert.match(source, /Some\("name" \| "id" \| "run" \| "uses" \| "with"\) => \{\}/);
  assert.match(source, /step must contain exactly one of run or uses/);

  for (const rejected of [
    'working-directory',
    'expressions inside run commands are unsupported',
    'must use an exact 40-hex commit SHA',
    'secret-bearing setup-action inputs are unsupported',
    'job mixes multiple language toolchains',
    'non-Linux native execution is unavailable',
    'checkout input',
    'bounded static label',
  ]) {
    assert.ok(source.includes(rejected), `missing rejection contract: ${rejected}`);
  }
  assert.match(source, /known_setup_action/);
  assert.match(source, /immutable_action_ref/);
  assert.match(source, /contains_secret_expression/);
  assert.match(source, /validate_setup_inputs/);
});

test('execution is deployment-disabled by default and resource bounded', async () => {
  const patch = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
  );

  assert.match(
    patch,
    /name:\s*BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED\s+value:\s*'false'/,
  );
  for (const name of [
    'BUILD_SERVER_GHA_MAX_YAML_BYTES',
    'BUILD_SERVER_GHA_MAX_JOBS',
    'BUILD_SERVER_GHA_MAX_STEPS_PER_JOB',
    'BUILD_SERVER_GHA_MAX_YAML_NODES',
    'BUILD_SERVER_GHA_MAX_YAML_DEPTH',
    'BUILD_SERVER_GHA_RUN_TIMEOUT_SECONDS',
    'BUILD_SERVER_GHA_MAX_RUNS',
  ]) {
    assert.match(patch, new RegExp(`name:\\s*${name}`));
  }
});

test('unit tests cover valid DAGs, authenticated HTTP, and adversarial structures', async () => {
  const source = await readRepoFile(
    'remote/deployments/build-server-rs/src/gha_workflow.rs',
  );

  for (const unitTest of [
    'http_plan_requires_auth_and_returns_executable_plan',
    'http_invalid_yaml_is_bad_request_and_unsupported_yaml_is_a_plan',
    'http_execution_is_disabled_before_any_build_is_enqueued',
    'plans_static_multi_job_workflow_into_fixed_profiles',
    'branch_revision_can_be_planned_but_not_executed',
    'rejects_mutable_actions_secrets_and_caller_selected_environments',
    'rejects_services_matrices_conditions_and_working_directories',
    'rejects_cycles_and_unknown_dependencies',
    'rejects_mixed_language_jobs_instead_of_guessing',
    'rejects_unknown_keys_and_steps_with_both_run_and_uses',
    'checkout_inputs_cannot_change_repository_or_credentials',
    'malformed_action_refs_and_invalid_request_ids_fail_closed',
    'retention_pruning_is_strict_and_preserves_active_runs',
    'plan_id_is_stable_and_changes_with_yaml',
    'build_request_is_profile_only_and_immutable',
    'workflow_path_and_yaml_limits_fail_closed',
  ]) {
    assert.match(source, new RegExp(`fn ${unitTest}\\(`));
  }
});

test('standalone publication provenance remains explicit', async () => {
  const operations = await readRepoFile(
    'docs/operations/gha-indie-worker-ephemeral-publication-v2.md',
  );
  const design = await readRepoFile('docs/gha-indie-worker-yaml-engine.md');

  assert.match(
    operations,
    /gha-indie-worker\.rs` from `remote\/deployments\/build-server-rs/,
  );
  assert.match(design, /canonical source for\s+`gha-indie-worker\/gha-indie-worker\.rs`/);
  assert.match(design, /currently cannot create a branch/);
});
