import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

const root = join(import.meta.dirname, '../../..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const observabilityInventories = [
  'remote/argocd/observability/k8s-resource-exporter.configmap.yaml',
  'remote/argocd/observability/k8s-resource-exporter.deployment.yaml',
];

const routerSources = [
  'remote/deployments/gha-clone-server-rs/src/bin/gha-executor-router.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/assignment.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/security.rs',
  'remote/deployments/gha-clone-server-rs/src/executor_router_service/upstream.rs',
];

const routerLibrary =
  'remote/deployments/gha-clone-server-rs/src/executor_router.rs';
const routerTests = [
  'remote/deployments/gha-clone-server-rs/tests/executor_router_http.rs',
  'remote/deployments/gha-clone-server-rs/tests/executor_router_assignment.rs',
];
const profilesPath = 'remote/deployments/build-server-rs/src/profiles.rs';
const plannerPath = 'remote/deployments/gha-clone-server-rs/src/lib.rs';
const metaWorkflowPath = '.github/workflows/gha-clone-server-meta.yml';
const continuityWorkflowPath = '.github/workflows/gha-clone-server.yml';
const activationRunbookPath =
  'docs/operations/gha-clone-webhook-activation.md';
const buildServerValidationPath =
  'remote/deployments/build-server-rs/src/validation.rs';
const buildServerWebhookPath =
  'remote/deployments/build-server-rs/src/webhooks.rs';
const buildServerDbPath = 'remote/deployments/build-server-rs/src/db.rs';
const buildServerHttpPath = 'remote/deployments/build-server-rs/src/http.rs';
const buildServerConfigPath =
  'remote/argocd/dd-next-runtime/dd-build-server.configmap.yaml';

test('resource exporter inventories retain both continuity services', () => {
  for (const path of observabilityInventories) {
    const inventory = read(path);
    assert.match(
      inventory,
      /dd-build-server,dd-gha-clone-server,/,
      `${path} must watch dd-gha-clone-server after dd-build-server`,
    );
    assert.match(
      inventory,
      /dd-gha-clone-server,dd-gha-executor-router,/,
      `${path} must watch dd-gha-executor-router after dd-gha-clone-server`,
    );
  }
});

test('current-dev assignment and authentication hardening survives the inert GitOps port', () => {
  const source = routerSources.map(read).join('\n');
  const library = read(routerLibrary);
  const tests = routerTests.map(read).join('\n');

  for (const expression of [
    /postSubmissionFailover": false/,
    /ambiguous_submissions_total/,
    /first_ready_executor/,
    /automaticFailover": false/,
    /get_all\("x-build-server-auth"\)/,
    /get_all\("x-server-auth"\)/,
  ]) {
    assert.match(source, expression);
  }

  assert.match(
    source,
    /const DEFAULT_SECRET_ROOT: &str = "\/var\/run\/secrets\/gha-executor-router";/,
  );
  assert.match(
    source,
    /env_optional\("GHA_EXECUTOR_ROUTER_SECRET_ROOT"\)[\s\S]{0,180}DEFAULT_SECRET_ROOT\.to_string\(\)/,
  );
  assert.match(library, /disabled executors must omit url and authPath/);
  assert.match(library, /authPath must be a direct child/);
  assert.match(library, /lowercase 40-hex commit SHA/);

  for (const expression of [
    /selects_first_ready_aws_executor_and_pins_status_to_it/,
    /readiness_failure_routes_to_hetzner_before_any_submission/,
    /ambiguous_submission_never_fails_over_or_leaks_upstream_body/,
    /accepted_build_status_failure_remains_pinned_without_resubmission/,
    /sequential_and_concurrent_identical_requests_submit_once/,
    /ambiguous_assignment_is_retained_and_retry_never_switches_provider/,
  ]) {
    assert.match(tests, expression);
  }
});

test('fixed-profile, planner, and meta-workflow ratchets survive the inert GitOps port', () => {
  const profiles = read(profilesPath);
  assert.match(
    profiles,
    /remote\/deployments\/gha-clone-server-rs\/Cargo\.toml/,
    'rust-verify must retain the reviewed monorepo crate fallback',
  );

  const imageAssignments = [
    ...profiles.matchAll(/const\s+[A-Z_]+_IMAGE:\s*&str\s*=\s*"([^"]+)";/g),
  ];
  assert.ok(
    imageAssignments.length >= 5,
    'expected the complete fixed-profile runner image inventory',
  );
  for (const [, image] of imageAssignments) {
    assert.ok(!image.endsWith(':latest'), `runner image must be pinned: ${image}`);
  }

  const planner = read(plannerPath);
  assert.match(
    planner,
    /unsupported by the fixed-profile executor/,
    'planner must keep its explicit fixed-profile rejection boundary',
  );

  const metaWorkflow = read(metaWorkflowPath);
  assert.doesNotMatch(
    metaWorkflow,
    /working-directory:|timeout-minutes:|permissions:/,
    'the bounded meta workflow must stay inside the independently supported subset',
  );

  const continuityWorkflow = read(continuityWorkflowPath);
  assert.match(continuityWorkflow, /dd-gha-executor-router\*/);
  assert.match(continuityWorkflow, /gha-clone-server-meta\.yml/);
  assert.match(continuityWorkflow, /persist-credentials:\s*false/);
});

test('failure-only webhook and immutable profile admission remain explicit', () => {
  const runbook = read(activationRunbookPath);
  assert.match(runbook, /only the `workflow_run` event/);
  assert.match(runbook, /signed non-`workflow_run` no-op behavior/);
  assert.match(runbook, /both clone-server execution gates are `false`/);
  assert.match(runbook, /does not publish a Check Run or commit status/);

  const validation = read(buildServerValidationPath);
  assert.match(
    validation,
    /gitRef must be a full 40- or 64-hex commit object ID for jobKind=run-profile/,
  );

  const webhook = read(buildServerWebhookPath);
  assert.match(webhook, /request\.request_id = Some\(github_request_id\(&delivery_id\)\)/);
  assert.match(webhook, /status\.is_server_error\(\)/);
  assert.match(webhook, /release_webhook_delivery_claim/);
  const unsupportedGate = webhook.indexOf('if event != "workflow_run"');
  const completedGate = webhook.indexOf('if action != "completed"');
  const exactRuleGate = webhook.indexOf('let matched = state');
  const deliveryClaim = webhook.indexOf('db::record_webhook_delivery(');
  assert.ok(unsupportedGate > 0 && unsupportedGate < deliveryClaim);
  assert.ok(completedGate > unsupportedGate && completedGate < deliveryClaim);
  assert.ok(exactRuleGate > completedGate && exactRuleGate < deliveryClaim);

  const httpTests = read(buildServerHttpPath);
  assert.match(
    httpTests,
    /github_webhook_unsupported_successful_and_noncompleted_runs_have_no_side_effects/,
  );
  assert.match(
    httpTests,
    /github_webhook_admitted_completed_failure_enqueues_exact_sha_once/,
  );

  const runtimeRules = read(buildServerConfigPath);
  assert.match(runtimeRules, /webhook-rules\.json: \|\n    \[\]/);
  assert.doesNotMatch(runtimeRules, /"events": \["push"\]/);

  const database = read(buildServerDbPath);
  assert.match(database, /Column::Action\.eq\("received"\)/);
  assert.match(database, /Column::DeliveryId\.eq\(delivery_id\)/);
});
