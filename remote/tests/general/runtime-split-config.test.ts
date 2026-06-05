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

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

test("runtime kustomization includes split web/api/gateway resources", async () => {
  const kustomization = await readRepoFile("remote/argocd/dd-next-runtime/kustomization.yaml");

  assert.match(kustomization, /availability-pdbs\.yaml/);
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
  assert.match(deployment, /progressDeadlineSeconds:\s*1800/);
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
  assert.match(gatewayConfig, /gzip\s+on;/);
  assert.match(gatewayConfig, /gzip_vary\s+on;/);
  assert.match(gatewayConfig, /gzip_min_length\s+1024;/);
  assert.match(gatewayConfig, /gzip_comp_level\s+5;/);
  assert.match(gatewayConfig, /gzip_proxied\s+any;/);
  assert.match(gatewayConfig, /gzip_types[\s\S]*application\/json[\s\S]*application\/javascript/);
});

test("gateway-backed stateless services use HA rolling deployment profile", async () => {
  const haDeployments = [
    "dd-remote-auth",
    "dd-remote-rest-api",
    "dd-agent-worker-broker",
    "dd-des-rs",
    "dd-contract-service",
    "dd-mdp-optimizer",
    "dd-trading-server",
    "dd-web-scraper",
    "dd-browser-test-server",
    "dd-selenium-server",
    "dd-rust-vapi-phone",
  ];
  const pdbs = await readRepoFile("remote/argocd/dd-next-runtime/availability-pdbs.yaml");

  for (const name of haDeployments) {
    const deployment = await readRepoFile(
      `remote/argocd/dd-next-runtime/${name}.deployment.yaml`,
    );
    const escapedName = escapeRegExp(name);

    assert.match(deployment, new RegExp(`name:\\s*${escapedName}`));
    assert.match(deployment, /replicas:\s*2/);
    assert.match(deployment, /minReadySeconds:\s*5/);
    assert.match(deployment, /progressDeadlineSeconds:\s*1800/);
    assert.match(deployment, /type:\s*RollingUpdate/);
    assert.match(deployment, /maxSurge:\s*1/);
    assert.match(deployment, /maxUnavailable:\s*0/);
    assert.match(deployment, /readinessProbe:[\s\S]*httpGet:/);
    assert.match(
      pdbs,
      new RegExp(
        `kind:\\s*PodDisruptionBudget[\\s\\S]*name:\\s*${escapedName}[\\s\\S]*minAvailable:\\s*1[\\s\\S]*app:\\s*${escapedName}`,
      ),
    );
  }

  const desRs = await readRepoFile("remote/argocd/dd-next-runtime/dd-des-rs.deployment.yaml");
  assert.match(
    desRs,
    /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/,
  );
});

test("single-owner runtime workloads stay intentionally recreate", async () => {
  const singleOwnerDeployments = [
    { name: "dd-browser-job-runner", file: "dd-browser-job-runner.deployment.yaml" },
    { name: "dd-build-server", file: "dd-build-server.deployment.yaml" },
    { name: "dd-container-pool", file: "dd-container-pool.deployment.yaml" },
    { name: "dd-des-simulator", file: "dd-des-simulator.deployment.yaml" },
    { name: "dd-dev-server-api", file: "dd-dev-server-home.deployment.yaml" },
    { name: "dd-go-wss-server", file: "dd-go-wss-server.deployment.yaml" },
    { name: "dd-idle-reaper", file: "dd-idle-reaper.deployment.yaml" },
    { name: "dd-live-mutex", file: "dd-live-mutex.deployment.yaml" },
    { name: "dd-live-mutex-submodule", file: "dd-live-mutex-submodule.deployment.yaml" },
    { name: "dd-redis-cache", file: "dd-redis-cache.deployment.yaml" },
    { name: "dd-remote-gateway", file: "dd-remote-gateway.deployment.yaml" },
    { name: "dd-runtime-config", file: "dd-runtime-config.deployment.yaml" },
    { name: "dd-rust-network-mutex", file: "dd-rust-network-mutex.deployment.yaml" },
    { name: "dd-rust-wss-server", file: "dd-rust-wss-server.deployment.yaml" },
    { name: "dd-webrtc-media", file: "dd-webrtc-media.deployment.yaml" },
    { name: "dd-webrtc-signaling", file: "dd-webrtc-signaling.deployment.yaml" },
  ];

  for (const { name, file } of singleOwnerDeployments) {
    const deployment = await readRepoFile(`remote/argocd/dd-next-runtime/${file}`);
    const escapedName = escapeRegExp(name);

    assert.match(deployment, new RegExp(`name:\\s*${escapedName}`));
    assert.match(deployment, /replicas:\s*1/);
    assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  }
});

test("queue consumer rolls replacement before terminating the old consumer", async () => {
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-queue-consumer.deployment.yaml",
  );

  assert.match(deployment, /replicas:\s*1/);
  assert.match(deployment, /minReadySeconds:\s*5/);
  assert.match(deployment, /progressDeadlineSeconds:\s*1800/);
  assert.match(deployment, /type:\s*RollingUpdate/);
  assert.match(deployment, /maxSurge:\s*1/);
  assert.match(deployment, /maxUnavailable:\s*0/);
});

test("fabrication server runtime keeps planner replicas hardened and observable", async () => {
  const deployment = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-fabrication-server.deployment.yaml",
  );
  const service = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-fabrication-server.service.yaml",
  );
  const hpa = await readRepoFile("remote/argocd/dd-next-runtime/dd-fabrication-server.hpa.yaml");
  const networkPolicy = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-fabrication-server.networkpolicy.yaml",
  );
  const serviceAccount = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-fabrication-server.serviceaccount.yaml",
  );
  const pdbs = await readRepoFile("remote/argocd/dd-next-runtime/availability-pdbs.yaml");

  assert.match(deployment, /name:\s*dd-fabrication-server/);
  assert.match(deployment, /replicas:\s*2/);
  assert.match(deployment, /minReadySeconds:\s*5/);
  assert.match(deployment, /progressDeadlineSeconds:\s*1800/);
  assert.match(deployment, /type:\s*RollingUpdate/);
  assert.match(deployment, /maxSurge:\s*1/);
  assert.match(deployment, /maxUnavailable:\s*0/);
  assert.match(deployment, /serviceAccountName:\s*dd-fabrication-server/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /enableServiceLinks:\s*false/);
  assert.match(deployment, /hostNetwork:\s*false/);
  assert.match(deployment, /hostPID:\s*false/);
  assert.match(deployment, /hostIPC:\s*false/);
  assert.match(deployment, /shareProcessNamespace:\s*false/);
  assert.match(deployment, /dnsPolicy:\s*ClusterFirst/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path:\s*\/readyz[\s\S]*port:\s*http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(deployment, /requests:[\s\S]*ephemeral-storage:\s*4Gi/);
  assert.match(deployment, /limits:[\s\S]*ephemeral-storage:\s*8Gi/);
  assert.match(deployment, /emptyDir:[\s\S]*sizeLimit:\s*7Gi/);

  assert.match(serviceAccount, /name:\s*dd-fabrication-server/);
  assert.match(serviceAccount, /automountServiceAccountToken:\s*false/);

  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /prometheus\.io\/scrape:\s*'true'/);
  assert.match(service, /sessionAffinity:\s*ClientIP/);
  assert.match(service, /port:\s*8113/);
  assert.match(service, /targetPort:\s*http/);

  assert.match(hpa, /minReplicas:\s*2/);
  assert.match(hpa, /maxReplicas:\s*8/);
  assert.match(hpa, /name:\s*cpu[\s\S]*averageUtilization:\s*70/);
  assert.match(hpa, /name:\s*memory[\s\S]*averageUtilization:\s*80/);

  assert.match(
    pdbs,
    /kind:\s*PodDisruptionBudget[\s\S]*name:\s*dd-fabrication-server[\s\S]*minAvailable:\s*1[\s\S]*app:\s*dd-fabrication-server/,
  );

  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*- Ingress[\s\S]*- Egress/);
  assert.match(networkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(networkPolicy, /app:\s*dd-runtime-config/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(networkPolicy, /app:\s*dd-nats/);
  assert.match(networkPolicy, /app:\s*dd-mdp-optimizer/);
  assert.match(networkPolicy, /port:\s*8113/);
  assert.match(networkPolicy, /port:\s*4222/);
  assert.match(networkPolicy, /port:\s*8110/);
  assert.match(networkPolicy, /port:\s*8096/);
  assert.match(networkPolicy, /cidr:\s*0\.0\.0\.0\/0/);
  assert.match(networkPolicy, /cidr:\s*::\/0/);
});
