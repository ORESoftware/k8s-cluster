import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const prometheusPath =
  'remote/argocd/observability/prometheus.configmap.yaml';
const deploymentPath =
  'remote/argocd/observability/prometheus.deployment.yaml';
const networkPolicyPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.networkpolicy.yaml';

const prometheus = readFileSync(prometheusPath, 'utf8');
const deployment = readFileSync(deploymentPath, 'utf8');
const networkPolicy = readFileSync(networkPolicyPath, 'utf8');

function occurrences(text, needle) {
  return text.split(needle).length - 1;
}

function section(text, startMarker, endMarker) {
  const start = text.indexOf(startMarker);
  assert.notEqual(start, -1, `missing section marker: ${startMarker}`);
  const end = text.indexOf(endMarker, start + startMarker.length);
  assert.notEqual(end, -1, `missing section end marker: ${endMarker}`);
  return text.slice(start, end);
}

test('central Prometheus has one unauthenticated AI bridge scrape target', () => {
  assert.equal(occurrences(prometheus, '      - job_name: dd-ai-agent-bridge\n'), 1);
  const job = section(
    prometheus,
    '      - job_name: dd-ai-agent-bridge\n',
    '      - job_name:',
  );
  assert.match(job, /metrics_path: \/metrics/);
  assert.match(
    job,
    /dd-ai-agent-bridge\.default\.svc\.cluster\.local:8142/,
  );
  assert.doesNotMatch(job, /authorization|bearer|token|password|secret/i);
  assert.doesNotMatch(job, /8143/);
});

test('AI bridge alerts cover target health, saturation, shedding, leases, and dependencies', () => {
  const alertNames = [
    'DDAIAgentBridgeTargetMissing',
    'DDAIAgentBridgeTargetDown',
    'DDAIAgentBridgeCapacityNearLimit',
    'DDAIAgentBridgeHttpCapacityRejected',
    'DDAIAgentBridgePersistenceWritesShed',
    'DDAIAgentBridgeLeaseErrorsIncreasing',
    'DDAIAgentBridgeControlPlaneErrorsIncreasing',
  ];
  for (const name of alertNames) {
    assert.equal(
      occurrences(prometheus, `          - alert: ${name}\n`),
      1,
      `expected one ${name} alert`,
    );
  }

  assert.match(prometheus, /up\{job="dd-ai-agent-bridge"\} == 0/);
  assert.match(
    prometheus,
    /ai_agent_bridge_capacity\{kind="current"\}[\s\S]*clamp_min\([\s\S]*ai_agent_bridge_capacity\{kind="limit"\}/,
  );
  assert.match(
    prometheus,
    /increase\(ai_agent_bridge_http_rejected_total\{reason="capacity"\}\[5m\]\)/,
  );
  assert.match(
    prometheus,
    /increase\(ai_agent_bridge_persistence_shed_writes_total\[5m\]\)/,
  );
  assert.match(
    prometheus,
    /ai_agent_bridge_file_lease_errors_total\{reason=~"conflict\|owner_mismatch\|stale_fencing_token"\}/,
  );
  assert.match(
    prometheus,
    /ai_agent_bridge_dependency_configured\{dependency="control_plane"\} == 1\s+and on\(\)\s+sum\(increase\(ai_agent_bridge_control_plane_requests_total/,
  );
});

test('only the central Prometheus pod may scrape bridge HTTP through the observability rule', () => {
  const observability = section(
    networkPolicy,
    '    # Central Prometheus may scrape only the public HTTP metrics endpoint.',
    '    # Warm pool workers use the node network',
  );
  assert.match(
    observability,
    /kubernetes\.io\/metadata\.name: observability/,
  );
  assert.match(observability, /app: dd-prometheus/);
  assert.match(observability, /port: 8142/);
  assert.doesNotMatch(observability, /port: 8143/);
  assert.doesNotMatch(observability, /ipBlock:/);
});

test('Prometheus rollout revision records the AI bridge scrape contract', () => {
  assert.match(
    deployment,
    /dd\.dev\/config-revision: "2026-07-31-ai-agent-bridge-metrics"/,
  );
});
