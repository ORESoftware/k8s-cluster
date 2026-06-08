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

// These tests pin the cluster-wide hardening invariants we apply on top of
// the live EC2 single-node cluster. They are pure config-file assertions so
// they run without network or kubectl access. Each invariant maps to a
// finding from the cluster security audit.

test("gateway sets baseline browser security headers", async () => {
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  const hstsMatch = gateway.match(/Strict-Transport-Security\s+"max-age=(\d+)"/);
  assert.ok(hstsMatch, "expected Strict-Transport-Security header in gateway config");
  const hstsSeconds = Number.parseInt(hstsMatch?.[1] ?? "0", 10);
  // 90 days minimum; bootstrap profile uses 180 days. Anything below 90d
  // is too short to be meaningful for HSTS.
  assert.ok(
    hstsSeconds >= 90 * 24 * 60 * 60,
    `HSTS max-age too short: ${hstsSeconds}`,
  );

  assert.match(gateway, /X-Frame-Options\s+"SAMEORIGIN"\s+always/);
  assert.match(gateway, /X-Content-Type-Options\s+"nosniff"\s+always/);
  assert.match(gateway, /Referrer-Policy\s+"strict-origin-when-cross-origin"\s+always/);
  assert.match(gateway, /server_tokens\s+off;/);
});

test("gateway quotes regex locations that contain quantifier braces", async () => {
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  assert.match(
    gateway,
    /location\s+~\s+"\^\/music\/songs\/\[0-9a-fA-F\]\{8\}-\[0-9a-fA-F\]\{4\}-\[0-9a-fA-F\]\{4\}-\[0-9a-fA-F\]\{4\}-\[0-9a-fA-F\]\{12\}\/votes\$"/,
  );
  assert.doesNotMatch(gateway, /location\s+~\s+\^\/music\/songs\/\[0-9a-fA-F\]\{8\}/);
});

test("dd-music-rs pins a rustc image new enough for the locked AWS SDK crates", async () => {
  const music = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-music-rs.deployment.yaml",
  );

  assert.match(music, /image:\s*docker\.io\/library\/rust:1\.91\.1-bookworm/);
  assert.doesNotMatch(music, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
});

test("/telemetry/ proxies websocket upgrades to grafana live", async () => {
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  // The /telemetry/ block must forward Upgrade and Connection so that
  // Grafana Live (/telemetry/api/live/ws) works through the gateway.
  const telemetryBlockMatch = gateway.match(
    /location\s+\/telemetry\/\s*\{[\s\S]*?dd-grafana[\s\S]*?\}/,
  );
  assert.ok(telemetryBlockMatch, "expected /telemetry/ location block");
  const telemetryBlock = telemetryBlockMatch?.[0] ?? "";
  assert.match(telemetryBlock, /proxy_set_header\s+Upgrade\s+\$http_upgrade;/);
  assert.match(telemetryBlock, /proxy_set_header\s+Connection\s+\$connection_upgrade;/);
  assert.match(telemetryBlock, /proxy_http_version\s+1\.1;/);
});

test("gateway defers optional cluster MCP DNS resolution until request time", async () => {
  const gateway = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml",
  );

  assert.match(
    gateway,
    /location\s+=\s+\/cluster-mcp\s*\{[\s\S]*set\s+\$dd_cluster_mcp_upstream\s+dd-cluster-mcp-rs\.default\.svc\.cluster\.local:8091;[\s\S]*proxy_pass\s+http:\/\/\$dd_cluster_mcp_upstream\/mcp;/,
  );
  assert.match(
    gateway,
    /location\s+\/cluster-mcp\/\s*\{[\s\S]*set\s+\$dd_cluster_mcp_upstream\s+dd-cluster-mcp-rs\.default\.svc\.cluster\.local:8091;[\s\S]*proxy_pass\s+http:\/\/\$dd_cluster_mcp_upstream\/;/,
  );
  assert.doesNotMatch(gateway, /proxy_pass\s+http:\/\/dd-cluster-mcp-rs\.default\.svc\.cluster\.local/);
});

test("dd-idle-reaper has additive baseline securityContext", async () => {
  const reaper = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-idle-reaper.deployment.yaml",
  );

  // Look at the container-level securityContext, not the absent pod-level
  // one. The "containers:" block should contain both fields below.
  assert.match(reaper, /allowPrivilegeEscalation:\s*false/);
  assert.match(reaper, /seccompProfile:\s*\n\s*type:\s*RuntimeDefault/);
  assert.doesNotMatch(reaper, /privileged:\s*true/);
  assert.doesNotMatch(reaper, /mountPropagation:\s*Bidirectional/);
});

test("nats main container drops all linux capabilities", async () => {
  const nats = await readRepoFile("remote/argocd/messaging/nats.deployment.yaml");

  // First container is `nats`; it must drop ALL caps. The exporter sidecar
  // already drops them; both should now share that posture.
  const natsContainer = nats.match(
    /-\s*name:\s*nats\b[\s\S]*?(?=^\s{8}-\s*name:|\Z)/m,
  );
  assert.ok(natsContainer, "expected nats container block");
  const block = natsContainer?.[0] ?? "";
  assert.match(block, /capabilities:\s*\n\s*drop:\s*\n\s*-\s*ALL/);
  assert.match(block, /allowPrivilegeEscalation:\s*false/);
  assert.match(block, /seccompProfile:\s*\n\s*type:\s*RuntimeDefault/);
});

test("promtail host log reader keeps root scope constrained", async () => {
  const promtail = await readRepoFile(
    "remote/argocd/observability/promtail.daemonset.yaml",
  );

  const promtailContainer = promtail.match(
    /-\s*name:\s*promtail\b[\s\S]*?(?=^\s{8}-\s*name:|\n\s*volumes:)/m,
  );
  assert.ok(promtailContainer, "expected promtail container block");
  const block = promtailContainer?.[0] ?? "";

  assert.match(block, /allowPrivilegeEscalation:\s*false/);
  assert.match(block, /runAsUser:\s*0/);
  assert.match(block, /runAsGroup:\s*0/);
  assert.match(block, /seccompProfile:\s*\n\s*type:\s*RuntimeDefault/);
  assert.doesNotMatch(block, /privileged:\s*true/);
  assert.match(promtail, /name:\s*varlog[\s\S]*mountPath:\s*\/var\/log[\s\S]*readOnly:\s*true/);
  assert.match(
    promtail,
    /name:\s*varlog[\s\S]*hostPath:\s*\n\s*path:\s*\/var\/log\s*\n\s*type:\s*Directory/,
  );
});

test("grafana anonymous access is viewer-only and git-provisioned", async () => {
  const grafana = await readRepoFile(
    "remote/argocd/observability/grafana.deployment.yaml",
  );
  const provisioning = await readRepoFile(
    "remote/argocd/observability/grafana.provisioning.configmap.yaml",
  );

  assert.match(grafana, /GF_AUTH_ANONYMOUS_ENABLED[\s\S]*value:\s*"true"/);
  assert.match(grafana, /GF_AUTH_ANONYMOUS_ORG_ROLE[\s\S]*value:\s*Viewer/);
  assert.doesNotMatch(grafana, /GF_AUTH_ANONYMOUS_ORG_ROLE[\s\S]*value:\s*Admin/);
  assert.match(grafana, /GF_AUTH_BASIC_ENABLED[\s\S]*value:\s*"false"/);
  assert.match(grafana, /GF_AUTH_DISABLE_LOGIN_FORM[\s\S]*value:\s*"true"/);
  assert.match(grafana, /GF_USERS_ALLOW_SIGN_UP[\s\S]*value:\s*"false"/);
  assert.match(provisioning, /allowUiUpdates:\s*false/);
});

test("dd-dev-server-api declares resource requests and limits", async () => {
  const devServer = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml",
  );

  const resourcesMatch = devServer.match(
    /resources:\s*\n\s*requests:\s*\n\s*cpu:\s*([^\n]+)\n\s*memory:\s*([^\n]+)\n\s*limits:\s*\n\s*cpu:\s*([^\n]+)\n\s*memory:\s*([^\n]+)/,
  );
  assert.ok(resourcesMatch, "dd-dev-server-api should declare requests + limits");
});

test("dd-next-runtime two-replica rollouts stay within single-node CPU capacity", async () => {
  const { readdir, readFile } = await import("node:fs/promises");
  const runtimeRoot = resolve(repoRoot, "remote/argocd/dd-next-runtime");
  const entries = await readdir(runtimeRoot, { withFileTypes: true });
  const offenders: string[] = [];

  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.endsWith(".deployment.yaml")) {
      continue;
    }

    const path = resolve(runtimeRoot, entry.name);
    const text = await readFile(path, "utf8");
    const replicas = Number.parseInt(text.match(/^\s*replicas:\s*(\d+)/m)?.[1] ?? "1", 10);
    if (replicas < 2 || !/type:\s*RollingUpdate/.test(text)) {
      continue;
    }

    if (!/maxSurge:\s*0/.test(text) || !/maxUnavailable:\s*1/.test(text)) {
      offenders.push(entry.name);
    }
  }

  assert.deepEqual(
    offenders,
    [],
    `two-replica rollouts should not require surge capacity on the single-node cluster:\n${offenders.join("\n")}`,
  );
});

test("hostPath Rust source-build services use bounded scheduler CPU requests", async () => {
  const sourceBuildDeployments = [
    "remote/argocd/dd-next-runtime/dd-economics-server.deployment.yaml",
    "remote/argocd/dd-next-runtime/dd-fabrication-server.deployment.yaml",
    "remote/argocd/dd-next-runtime/dd-public-data-server.deployment.yaml",
  ];
  const offenders: string[] = [];

  for (const relativePath of sourceBuildDeployments) {
    const text = await readRepoFile(relativePath);
    if (!/CARGO_BUILD_JOBS[\s\S]*value:\s*'1'/.test(text)) {
      offenders.push(`${relativePath}: missing CARGO_BUILD_JOBS=1`);
    }
    if (/requests:\s*\n\s*cpu:\s*250m/.test(text)) {
      offenders.push(`${relativePath}: request still uses 250m`);
    }
    if (!/requests:\s*\n\s*cpu:\s*100m/.test(text)) {
      offenders.push(`${relativePath}: missing 100m request`);
    }
  }

  assert.deepEqual(offenders, [], `source-build CPU request guard failed:\n${offenders.join("\n")}`);
});

test("benchmark websocket services do not reserve whole idle cores", async () => {
  const requestBudgets = [
    {
      path: "remote/deployments/dart-server/k8s/ec2/dd-dart-server.deployment.yaml",
      cpu: "250m",
    },
    {
      path: "remote/deployments/akka-ws-server/k8s/ec2/dd-akka-ws-server.deployment.yaml",
      cpu: "100m",
    },
    {
      path: "remote/argocd/dd-next-runtime/dd-go-wss-server.deployment.yaml",
      cpu: "100m",
    },
    {
      path: "remote/argocd/dd-next-runtime/dd-rust-wss-server.deployment.yaml",
      cpu: "100m",
    },
    {
      path: "remote/deployments/fsharp-ws-server/k8s/ec2/dd-fsharp-ws-server.deployment.yaml",
      cpu: "100m",
    },
  ];
  const offenders: string[] = [];

  for (const { path, cpu } of requestBudgets) {
    const text = await readRepoFile(path);
    if (!new RegExp(`requests:\\s*\\n\\s*cpu:\\s*${cpu}`).test(text)) {
      offenders.push(`${path}: expected CPU request ${cpu}`);
    }
  }

  assert.deepEqual(offenders, [], `websocket benchmark CPU request guard failed:\n${offenders.join("\n")}`);
});

test("no kubernetes manifest in argocd uses the :latest image tag", async () => {
  // Templates under remote/k8s/ are scaffold placeholders (REPLACE_ME) and
  // are intentionally exempt; production deployments live under remote/argocd.
  const { readdir, readFile } = await import("node:fs/promises");
  const argocdRoot = resolve(repoRoot, "remote/argocd");

  async function walk(dir: string): Promise<string[]> {
    const entries = await readdir(dir, { withFileTypes: true });
    const out: string[] = [];
    for (const entry of entries) {
      const full = resolve(dir, entry.name);
      if (entry.isDirectory()) {
        out.push(...(await walk(full)));
      } else if (entry.isFile() && /\.ya?ml$/i.test(entry.name)) {
        out.push(full);
      }
    }
    return out;
  }

  const files = await walk(argocdRoot);
  const offenders: string[] = [];
  for (const path of files) {
    const text = await readFile(path, "utf8");
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed.startsWith("image:")) {
        continue;
      }
      // The trading server license-key handler etc. may use comments;
      // ignore commented-out manifest lines.
      if (trimmed.startsWith("#")) {
        continue;
      }
      if (/:latest(\b|"|')/.test(trimmed)) {
        offenders.push(`${path}: ${trimmed}`);
      }
    }
  }
  assert.deepEqual(offenders, [], `:latest image tags found:\n${offenders.join("\n")}`);
});
