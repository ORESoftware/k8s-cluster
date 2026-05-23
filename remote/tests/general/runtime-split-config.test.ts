import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), "..", "..")]) {
    if (existsSync(resolve(candidate, "remote/argocd/dd-next-runtime/kustomization.yaml"))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

test("runtime kustomization includes split web/api/gateway resources", async () => {
  const kustomization = await readRepoFile("remote/argocd/dd-next-runtime/kustomization.yaml");

  assert.match(kustomization, /dd-dev-server-home\.deployment\.yaml/);
  assert.match(kustomization, /dd-dev-server-api\.service\.yaml/);
  assert.match(kustomization, /dd-remote-web-home\.deployment\.yaml/);
  assert.match(kustomization, /dd-remote-web-home\.pdb\.yaml/);
  assert.match(kustomization, /dd-remote-web-home\.service\.yaml/);
  assert.match(kustomization, /dd-remote-gateway\.configmap\.yaml/);
  assert.match(kustomization, /dd-remote-gateway\.deployment\.yaml/);
});

test("runtime is owned by an automated argocd application", async () => {
  const app = await readRepoFile("remote/argocd/apps/dd-next-runtime.application.yaml");

  assert.match(app, /name:\s*dd-next-runtime/);
  assert.match(app, /targetRevision:\s*dev/);
  assert.match(app, /path:\s*remote\/argocd\/dd-next-runtime/);
  assert.match(app, /namespace:\s*default/);
  assert.match(app, /automated:[\s\S]*prune:\s*true[\s\S]*selfHeal:\s*true/);
});

test("node deployment is api-only and no longer binds hostPort 80", async () => {
  const nodeDeployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml",
  );

  assert.match(nodeDeployment, /name:\s*dd-dev-server-api/);
  assert.doesNotMatch(nodeDeployment, /hostPort:\s*80/);
});

test("dev-server-api wires the full event-fanout env (REST + NATS + worker WS)", async () => {
  const nodeDeployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml",
  );

  assert.match(
    nodeDeployment,
    /name:\s*EVENT_INGEST_URL[\s\S]*value:\s*http:\/\/dd-remote-rest-api\.default\.svc\.cluster\.local:8082\/api\/agents\/events/,
  );
  assert.match(
    nodeDeployment,
    /name:\s*EVENT_INGEST_SECRET[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-agent-secrets[\s\S]*key:\s*SERVER_AUTH_SECRET/,
  );
  assert.match(
    nodeDeployment,
    /name:\s*NATS_URL[\s\S]*value:\s*nats:\/\/dd-nats\.messaging\.svc\.cluster\.local:4222/,
  );
  assert.match(nodeDeployment, /name:\s*NATS_EVENT_SUBJECT[\s\S]*value:\s*dd\.remote\.events/);
  assert.match(
    nodeDeployment,
    /name:\s*THREAD_CONTEXT_BASE_URL[\s\S]*value:\s*http:\/\/dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
  );
  // GLEAM_BROADCAST_SECRET lives in dd-gleamlang-server-secrets and is the
  // dev-server's third-fallback for WORKER_FANOUT_WS_SECRET (see
  // dev-server/src/ws-fanout.ts). Without this secretRef the outbound
  // /worker-ws fanout silently disables itself.
  assert.match(nodeDeployment, /dd-gleamlang-server-secrets/);
});

test("gateway routes homepage to rust and worker control paths to node api", async () => {
  const gatewayConfig = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  assert.match(
    gatewayConfig,
    /location\s+\/home[\s\S]*dd-remote-web-home\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/tasks[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/status[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/agents[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/healthz[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/stream\/[\s\S]*proxy_buffering off/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/stream\/[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gatewayConfig,
    /location\s+\/\s*\{[\s\S]*dd-remote-web-home\.default\.svc\.cluster\.local:8080/,
  );
});

test("web home route has rollout and gateway guards against transient 502s", async () => {
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-web-home.deployment.yaml",
  );
  const pdb = await readRepoFile("remote/argocd/dd-next-runtime/dd-remote-web-home.pdb.yaml");
  const gatewayConfig = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  assert.match(deployment, /replicas:\s*2/);
  assert.match(deployment, /minReadySeconds:\s*5/);
  assert.match(deployment, /type:\s*RollingUpdate/);
  assert.match(deployment, /maxSurge:\s*1/);
  assert.match(deployment, /maxUnavailable:\s*0/);
  assert.match(deployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);

  assert.match(pdb, /kind:\s*PodDisruptionBudget/);
  assert.match(pdb, /name:\s*dd-remote-web-home/);
  assert.match(pdb, /minAvailable:\s*1/);
  assert.match(pdb, /matchLabels:[\s\S]*app:\s*dd-remote-web-home/);

  assert.match(
    gatewayConfig,
    /proxy_next_upstream\s+error timeout invalid_header http_502 http_503 http_504;/,
  );
  assert.match(gatewayConfig, /proxy_next_upstream_tries\s+3;/);
  assert.match(gatewayConfig, /proxy_connect_timeout\s+5s;/);
});
