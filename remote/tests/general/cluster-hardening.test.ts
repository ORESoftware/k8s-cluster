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

test("dd-idle-reaper has additive baseline securityContext", async () => {
  const reaper = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-idle-reaper.deployment.yaml",
  );

  // Look at the container-level securityContext, not the absent pod-level
  // one. The "containers:" block should contain both fields below.
  assert.match(reaper, /allowPrivilegeEscalation:\s*false/);
  assert.match(reaper, /seccompProfile:\s*\n\s*type:\s*RuntimeDefault/);
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

test("dd-dev-server-api declares resource requests and limits", async () => {
  const devServer = await readRepoFile(
    "remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml",
  );

  const resourcesMatch = devServer.match(
    /resources:\s*\n\s*requests:\s*\n\s*cpu:\s*([^\n]+)\n\s*memory:\s*([^\n]+)\n\s*limits:\s*\n\s*cpu:\s*([^\n]+)\n\s*memory:\s*([^\n]+)/,
  );
  assert.ok(resourcesMatch, "dd-dev-server-api should declare requests + limits");
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
