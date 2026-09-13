import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/messaging/nats.service.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('nats messaging app is gitops-managed and exposes client plus metrics ports', async () => {
  const app = await readRepoFile('remote/argocd/apps/dd-messaging.application.yaml');
  const config = await readRepoFile('remote/argocd/messaging/nats.configmap.yaml');
  const deployment = await readRepoFile('remote/argocd/messaging/nats.deployment.yaml');
  const kustomization = await readRepoFile('remote/argocd/messaging/kustomization.yaml');
  const networkPolicy = await readRepoFile('remote/argocd/messaging/nats.networkpolicy.yaml');
  const pdb = await readRepoFile('remote/argocd/messaging/nats.pdb.yaml');
  const readme = await readRepoFile('remote/argocd/messaging/readme.md');
  const service = await readRepoFile('remote/argocd/messaging/nats.service.yaml');

  assert.match(app, /name:\s*dd-messaging/);
  assert.match(app, /path:\s*remote\/argocd\/messaging/);
  assert.match(config, /jetstream/i);
  assert.match(config, /server_name:\s*dd-nats/);
  assert.match(config, /port:\s*4222/);
  assert.match(config, /http:\s*8222/);
  assert.match(config, /max_connections:\s*4096/);
  assert.match(config, /max_subscriptions:\s*8192/);
  assert.match(config, /max_control_line:\s*4KB/);
  assert.match(config, /max_payload:\s*1MB/);
  assert.match(config, /max_pending:\s*16MB/);
  assert.match(config, /ping_max:\s*2/);
  assert.match(config, /max_mem_store:\s*512MB/);
  assert.doesNotMatch(config, /authorization\s*\{/);
  assert.match(
    deployment,
    /image:\s*nats:2\.14\.5-alpine@sha256:d4ac35882ac65aff236cd65b9d3fa4d24332c681e1a85f94eedccd3cdd65b1da/,
  );
  assert.match(
    deployment,
    /image:\s*natsio\/prometheus-nats-exporter:0\.20\.2@sha256:c623b608e148e31e1c1c878673a197f1828e58ce90de4f01d22f1baa84c8fee9/,
  );
  assert.match(deployment, /priorityClassName:\s*system-cluster-critical/);
  assert.match(deployment, /enableServiceLinks:\s*false/);
  assert.match(
    deployment,
    /name:\s*prepare-nats-data[\s\S]*chown -R 1000:1000 \/data[\s\S]*add:[\s\S]*-\s*CHOWN/,
  );
  assert.match(
    deployment,
    /name:\s*nats[\s\S]*readOnlyRootFilesystem:\s*true[\s\S]*runAsNonRoot:\s*true[\s\S]*runAsUser:\s*1000/,
  );
  assert.match(
    deployment,
    /name:\s*prometheus-exporter[\s\S]*readOnlyRootFilesystem:\s*true[\s\S]*runAsNonRoot:\s*true[\s\S]*runAsUser:\s*65532/,
  );
  assert.match(deployment, /name:\s*config[\s\S]*mountPath:\s*\/etc\/nats[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /startupProbe:[\s\S]*path:\s*\/healthz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-varz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-connz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-routez/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-subz/);
  assert.match(deployment, /args:\s*[\s\S]*-\s*-jsz=all/);
  assert.match(deployment, /args:\s*[\s\S]*http:\/\/127\.0\.0\.1:8222/);
  assert.match(service, /name:\s*client[\s\S]*port:\s*4222/);
  assert.match(service, /name:\s*monitor[\s\S]*port:\s*8222/);
  assert.match(service, /name:\s*metrics[\s\S]*port:\s*7777/);
  assert.match(kustomization, /nats\.networkpolicy\.yaml/);
  assert.match(kustomization, /nats\.pdb\.yaml/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*default/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*ai-ml/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*daedalus/);
  assert.match(networkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(networkPolicy, /app:\s*dd-cluster-mcp-rs/);
  assert.match(networkPolicy, /app:\s*dd-gleam-mcp-server/);
  assert.match(networkPolicy, /port:\s*4222/);
  assert.match(networkPolicy, /port:\s*8222/);
  assert.match(networkPolicy, /port:\s*7777/);
  assert.match(networkPolicy, /egress:\s*\[\]/);
  assert.match(readme, /default-deny `NetworkPolicy` limits the client port/);
  assert.match(readme, /no NATS-native authentication\/authorization/);
  assert.doesNotMatch(readme, /\*\*no NetworkPolicy\*\*/);
  assert.match(pdb, /kind:\s*PodDisruptionBudget/);
  assert.match(pdb, /minAvailable:\s*1/);
});

test('unauthenticated NATS cannot enable contract settlement broadcast', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-contract-service.deployment.yaml',
  );

  for (const failClosedGate of [
    'SOLANA_SEND_ENABLED',
    'SOLANA_MAINNET_SETTLEMENT_ENABLED',
    'CONTRACT_NATS_SETTLEMENT_ENABLED',
    'CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS',
  ]) {
    assert.match(
      deployment,
      new RegExp(`name:\\s*${failClosedGate}[\\s\\S]{0,120}?value:\\s*['"]false['"]`),
    );
  }
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
  const design = await readRepoFile('remote/deployments/nats/future.md');

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
