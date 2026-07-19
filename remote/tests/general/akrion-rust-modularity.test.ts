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
const runtimeDir = "remote/argocd/dd-next-runtime";

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

function assertOtelHttpRuntime(manifest: string, serviceName: string): void {
  assert.match(manifest, /name: (?:AKRION|SOCCER)_TELEMETRY_ENABLED\s*\n\s*value: ['"]true['"]/);
  assert.match(manifest, /name: (?:AKRION|SOCCER)_LOG_JSON\s*\n\s*value: ['"]true['"]/);
  assert.match(
    manifest,
    /name: OTEL_EXPORTER_OTLP_ENDPOINT\s*\n\s*value: http:\/\/dd-otel-collector\.observability\.svc\.cluster\.local:4318/,
  );
  assert.match(manifest, /name: (?:AKRION|SOCCER)_OTEL_TRACES\s*\n\s*value: ['"]true['"]/);
  assert.match(manifest, /name: (?:AKRION|SOCCER)_OTEL_METRICS\s*\n\s*value: ['"]true['"]/);
  assert.match(manifest, /name: POD_NAMESPACE\s*\n\s*valueFrom:/);
  assert.match(manifest, /name: POD_NAME\s*\n\s*valueFrom:/);
  assert.match(manifest, /name: NODE_NAME\s*\n\s*valueFrom:/);
  assert.match(manifest, new RegExp(`name: (?:AKRION|SOCCER)_SERVICE_NAME\\s*\\n\\s*value: ${serviceName}`));
}

test("Akrion backend components remain separate canonical git submodules", async () => {
  const gitmodules = await readRepoFile(".gitmodules");
  const expectedSubmodules = [
    ["remote/deployments/soccer-rs", "akrion-sim/akrion-backend.rs.git"],
    ["remote/deployments/akrion-web-server-rs", "akrion-sim/akrion-web-server.rs.git"],
    ["remote/submodules/soccer-sim-game-engine.rs", "ORESoftware/soccer-sim-game-engine.rs.git"],
  ] as const;

  for (const [path, upstream] of expectedSubmodules) {
    assert.match(gitmodules, new RegExp(`path = ${path.replaceAll(".", "\\.")}`));
    assert.match(gitmodules, new RegExp(`url = [^\\n]*${upstream.replaceAll(".", "\\.")}`));
  }
});

test("soccer runtime composes backend, engine, and DES sources without merging repositories", async () => {
  const deployment = await readRepoFile(`${runtimeDir}/dd-soccer-rs.deployment.yaml`);
  assert.match(deployment, /server_src="\/opt\/dd-next-1\/remote\/deployments\/soccer-rs"/);
  assert.match(
    deployment,
    /soccer_src="\/opt\/dd-next-1\/remote\/submodules\/soccer-sim-game-engine\.rs"/,
  );
  assert.match(
    deployment,
    /engine_src="\/opt\/dd-next-1\/remote\/submodules\/discrete-event-system\.rs"/,
  );
  assert.match(deployment, /build_src="\$\{build_root\}\/remote\/deployments\/soccer-rs"/);
  assert.match(
    deployment,
    /build_soccer="\$\{build_root\}\/remote\/submodules\/soccer-sim-game-engine\.rs"/,
  );
  assert.match(
    deployment,
    /build_engine="\$\{build_root\}\/remote\/submodules\/discrete-event-system\.rs"/,
  );
});

test("Akrion HTTP services retain readiness and shared collector wiring", async () => {
  const services = [
    ["dd-soccer-rs", "dd-soccer-rs"],
    ["dd-akrion-web-server-rs", "dd-akrion-web-server-rs"],
  ] as const;

  for (const [manifestName, serviceName] of services) {
    const deployment = await readRepoFile(`${runtimeDir}/${manifestName}.deployment.yaml`);
    const networkPolicy = await readRepoFile(`${runtimeDir}/${manifestName}.networkpolicy.yaml`);
    assertOtelHttpRuntime(deployment, serviceName);
    assert.match(deployment, /readinessProbe:[\s\S]{0,180}path: \/readyz/);
    assert.match(networkPolicy, /kubernetes\.io\/metadata\.name: observability/);
    assert.match(networkPolicy, /port: 4318/);
  }
});

test("soccer learning workloads use the engine telemetry module contract", async () => {
  const workloads = [
    ["dd-soccer-learning-queue.cronjob.yaml", "dd-soccer-learning-queue"],
    ["dd-soccer-tournament-nightly.cronjob.yaml", "dd-soccer-tournament-nightly"],
  ] as const;

  for (const [manifestName, serviceName] of workloads) {
    const manifest = await readRepoFile(`${runtimeDir}/${manifestName}`);
    assertOtelHttpRuntime(manifest, serviceName);
  }
});
