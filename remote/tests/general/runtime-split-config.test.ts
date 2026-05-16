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
