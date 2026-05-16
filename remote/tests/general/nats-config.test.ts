import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('nats messaging app is gitops-managed and exposes client plus metrics ports', async () => {
  const app = await readRepoFile('remote/argocd/apps/dd-messaging.application.yaml');
  const config = await readRepoFile('remote/argocd/messaging/nats.configmap.yaml');
  const deployment = await readRepoFile('remote/argocd/messaging/nats.deployment.yaml');
  const service = await readRepoFile('remote/argocd/messaging/nats.service.yaml');

  assert.match(app, /name:\s*dd-messaging/);
  assert.match(app, /path:\s*remote\/argocd\/messaging/);
  assert.match(config, /jetstream/i);
  assert.match(config, /server_name:\s*dd-nats/);
  assert.match(config, /port:\s*4222/);
  assert.match(config, /http:\s*8222/);
  assert.match(deployment, /image:\s*nats:2\.11\.17-alpine/);
  assert.match(deployment, /image:\s*natsio\/prometheus-nats-exporter:0\.19\.2/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-varz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-connz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-routez/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-subz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-jsz=all/);
  assert.match(deployment, /args:\s*[\s\S]*http:\/\/127\.0\.0\.1:8222/);
  assert.match(service, /name:\s*client[\s\S]*port:\s*4222/);
  assert.match(service, /name:\s*monitor[\s\S]*port:\s*8222/);
  assert.match(service, /name:\s*metrics[\s\S]*port:\s*7777/);
});

test('observability stack scrapes nats exporter and dashboards nats metrics', async () => {
  const collector = await readRepoFile(
    'remote/argocd/observability/otel-collector.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const dashboard = await readRepoFile(
    'remote/argocd/observability/grafana.dashboards.configmap.yaml',
  );

  assert.match(collector, /job_name:\s*dd-nats/);
  assert.match(collector, /dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(prometheus, /job_name:\s*dd-nats/);
  assert.match(prometheus, /dd-nats\.messaging\.svc\.cluster\.local:7777/);
  assert.match(dashboard, /NATS Connections/);
  assert.match(dashboard, /gnatsd_varz_connections/);
  assert.match(dashboard, /gnatsd_varz_in_msgs/);
  assert.match(dashboard, /gnatsd_varz_out_msgs/);
});

test('future remote task queue design keeps thread affinity and shadow rollout constraints', async () => {
  const design = await readRepoFile('remote/nats/future.md');

  assert.match(design, /current production path should\s+stay direct/i);
  assert.match(design, /Do not put all Node\.js workers in one generic queue group/);
  assert.match(design, /Replicas are `0` or `1`/);
  assert.match(design, /dd\.remote\.thread\.<threadId>\.tasks/);
  assert.match(design, /dd\.remote\.thread\.<threadId>\.control/);
  assert.match(design, /dd\.remote\.orchestrator\.wakeup/);
  assert.match(design, /Nats-Msg-Id:\s*remote-task:<taskId>/);
  assert.match(design, /Postgres still remains the real\s+idempotency guard/);
  assert.match(design, /worker\.<threadShort>/);
  assert.match(design, /Filter it to:\s*[\s\S]*dd\.remote\.thread\.<threadId>\.tasks/);
  assert.match(design, /REST API also publishes the task message to NATS with `shadow: true`/);
  assert.match(design, /Switch one test thread to queue execution/);
});
