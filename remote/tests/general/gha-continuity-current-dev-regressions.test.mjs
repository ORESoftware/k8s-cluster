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
