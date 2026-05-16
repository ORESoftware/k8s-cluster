import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";

const repoRoot = resolve(process.cwd(), "..", "..");

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
