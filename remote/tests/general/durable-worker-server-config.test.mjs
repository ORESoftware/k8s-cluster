import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = (path) => readFileSync(resolve(repoRoot, path), 'utf8');

const deploymentPath = 'remote/argocd/dd-next-runtime/dd-durable-worker-server.deployment.yaml';
const servicePath = 'remote/argocd/dd-next-runtime/dd-durable-worker-server.service.yaml';
const networkPolicyPath = 'remote/argocd/dd-next-runtime/dd-durable-worker-server.networkpolicy.yaml';
const pdbPath = 'remote/argocd/dd-next-runtime/dd-durable-worker-server.pdb.yaml';
const kustomizationPath = 'remote/argocd/dd-next-runtime/kustomization.yaml';
const workflowPath = '.github/workflows/durable-worker-server-rs.yml';
const dockerfilePath = 'remote/deployments/durable-worker-server-rs/Dockerfile';
const protocolPath = 'remote/deployments/durable-worker-server-rs/PROTOCOL.md';
const operationsPath = 'remote/deployments/durable-worker-server-rs/OPERATIONS.md';
const enginePath = 'remote/deployments/durable-worker-server-rs/src/engine/mod.rs';
const smokePath = 'remote/deployments/durable-worker-server-rs/tests/gha_smoke.mjs';

const deployment = read(deploymentPath);
const service = read(servicePath);
const networkPolicy = read(networkPolicyPath);
const pdb = read(pdbPath);
const kustomization = read(kustomizationPath);
const workflow = read(workflowPath);
const dockerfile = read(dockerfilePath);
const protocol = read(protocolPath);
const operations = read(operationsPath);
const engine = read(enginePath);
const smoke = read(smokePath);

test('durable worker deployment is portable, hardened, and inert by default', () => {
  assert.match(deployment, /\breplicas:\s*0\b/);
  assert.match(deployment, /name:\s*DURABLE_WORKER_SHADOW_MODE\s+value:\s*'true'/);
  assert.match(deployment, /image:\s*ghcr\.io\/oresoftware\/dd-durable-worker-server(?::[A-Za-z0-9._-]+|@sha256:[a-f0-9]{64})/);
  assert.match(deployment, /\bimagePullPolicy:\s*Always\b/);
  assert.match(deployment, /\bautomountServiceAccountToken:\s*false\b/);
  assert.match(deployment, /\benableServiceLinks:\s*false\b/);
  assert.match(deployment, /\breadOnlyRootFilesystem:\s*true\b/);
  assert.match(deployment, /capabilities:\s+drop:\s+- ALL/);
  assert.match(deployment, /seccompProfile:\s+type:\s*RuntimeDefault/);
  assert.match(deployment, /name:\s*DURABLE_WORKER_AUTH_SECRET\s+valueFrom:\s+secretKeyRef:/);
  assert.match(deployment, /name:\s*dd-agent-secrets\s+key:\s*SERVER_AUTH_SECRET/);
  assert.doesNotMatch(deployment, /cargo run --release --locked/);
  assert.doesNotMatch(deployment, /docker\.io\/library\/rust:/);
  assert.doesNotMatch(deployment, /\bhostPath:/);
  assert.doesNotMatch(deployment, /\/home\/ec2-user\/codes/);
  assert.doesNotMatch(
    deployment,
    /name:\s*DURABLE_WORKER_AUTH_SECRET\s+value:/,
    'the deployment must never inline worker authentication material',
  );
});

test('service and network policy expose only the intended internal control-plane port', () => {
  assert.match(service, /\btype:\s*ClusterIP\b/);
  assert.match(service, /\bport:\s*8152\b/);
  assert.match(service, /\btargetPort:\s*http\b/);
  assert.match(networkPolicy, /policyTypes:\s+- Ingress\s+- Egress/);
  assert.match(networkPolicy, /app:\s*dd-remote-rest-api/);
  assert.match(networkPolicy, /app:\s*dd-agent-worker-broker/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(networkPolicy, /app:\s*dd-nats/);
  assert.match(networkPolicy, /\bport:\s*4222\b/);
  assert.match(networkPolicy, /169\.254\.0\.0\/16/);
  assert.doesNotMatch(networkPolicy, /\bport:\s*80\b/);
});

test('the disruption budget remains compatible with the zero-replica rollout gate', () => {
  assert.match(pdb, /\bmaxUnavailable:\s*1\b/);
  assert.doesNotMatch(pdb, /\bminAvailable:/);
});

test('dd-next-runtime renders every durable-worker resource', () => {
  for (const file of [deploymentPath, servicePath, networkPolicyPath, pdbPath]) {
    const name = file.split('/').at(-1);
    assert.match(kustomization, new RegExp(`^\\s*- ${name.replaceAll('.', '\\.')}$`, 'm'));
  }
});

test('CI uses ephemeral credentials, locked dependencies, pinned actions, and GHCR publication', () => {
  assert.match(workflow, /secrets\.token_hex\(32\)/);
  assert.doesNotMatch(workflow, /^\s*DURABLE_WORKER_AUTH_SECRET:\s*\S+/m);
  assert.match(workflow, /permissions:\s+contents:\s*read\s+packages:\s*write/);
  assert.match(workflow, /actions\/checkout@[0-9a-f]{40}/);
  assert.match(workflow, /dtolnay\/rust-toolchain@[0-9a-f]{40}/);
  assert.match(workflow, /imranismail\/setup-kustomize@[0-9a-f]{40}/);
  assert.match(workflow, /docker\/setup-buildx-action@[0-9a-f]{40}/);
  assert.match(workflow, /docker\/login-action@[0-9a-f]{40}/);
  assert.match(workflow, /docker\/metadata-action@[0-9a-f]{40}/);
  assert.match(workflow, /docker\/build-push-action@[0-9a-f]{40}/);
  assert.match(workflow, /cargo clippy --locked/);
  assert.match(workflow, /cargo test --locked/);
  assert.match(workflow, /nats:2\.14\.3-alpine/);
  assert.match(workflow, /ghcr\.io\/oresoftware\/dd-durable-worker-server/);
  assert.match(workflow, /publish-image:[\s\S]*if:\s*github\.event_name == 'push'/);
  assert.match(workflow, /push:\s*true/);
});

test('the production image, protocol, and rollout runbook are reproducible and explicit', () => {
  assert.match(dockerfile, /COPY Cargo\.toml Cargo\.lock/);
  assert.match(dockerfile, /cargo build --release --locked/);
  assert.match(dockerfile, /gcr\.io\/distroless\/cc-debian12:nonroot/);
  assert.match(protocol, /does not replay a worker language stack/i);
  assert.match(protocol, /lease token/i);
  assert.match(protocol, /replicas: 0/);
  assert.match(operations, /digest/i);
  assert.match(operations, /scale the Deployment to zero/i);
  assert.match(operations, /fencing token/i);
});

test('run deadlines are durable, observable, and fenced end to end', () => {
  assert.match(protocol, /deadlineMs/);
  assert.match(protocol, /run\.deadline_exceeded/);
  assert.match(engine, /dd_durable_run_deadlines_exceeded_total/);
  assert.match(engine, /ensure_run_open_for_mutation/);
  assert.match(smoke, /deadlineSubmitted/);
  assert.match(smoke, /staleCompletion\.status, 409/);
});
