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
const vendorRoot = "remote/deployments/athleto-backend-rs";

// The vendored athleto-backend-rs tree is a SECONDARY submodule checkout. When
// the submodule is uninitialized/empty the manifest-content assertions cannot
// run, so they are skipped with a clear message; the .gitmodules pin assertion
// (a superproject fact) always runs.
const vendorPresent = existsSync(resolve(repoRoot, vendorRoot, "Cargo.toml"));
const skipIfAbsent = vendorPresent
  ? false
  : `${vendorRoot} submodule not checked out; skipping vendored-manifest assertions`;

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), "utf8");
}

test("athleto-backend-rs is pinned as an athlet-o org submodule", async () => {
  const gitmodules = await readRepoFile(".gitmodules");

  assert.match(gitmodules, /path = remote\/deployments\/athleto-backend-rs/);
  assert.match(gitmodules, /url = git@github\.com:athlet-o\/athleto-backend\.rs\.git/);
  assert.match(
    gitmodules,
    /\[submodule "remote\/deployments\/athleto-backend-rs"\][\s\S]*?branch = main/,
  );
});

test("vendored k8s manifests exist and are wired by the kustomization", { skip: skipIfAbsent }, async () => {
  const deployment = await readRepoFile(`${vendorRoot}/k8s/ec2/dd-athleto-backend.deployment.yaml`);
  const service = await readRepoFile(`${vendorRoot}/k8s/ec2/dd-athleto-backend.service.yaml`);
  const kustomization = await readRepoFile(`${vendorRoot}/k8s/ec2/kustomization.yaml`);

  assert.match(deployment, /kind:\s*Deployment/);
  assert.match(deployment, /name:\s*dd-athleto-backend/);
  assert.match(service, /kind:\s*Service/);
  assert.match(service, /type:\s*ClusterIP/);
  assert.match(kustomization, /dd-athleto-backend\.deployment\.yaml/);
  assert.match(kustomization, /dd-athleto-backend\.service\.yaml/);
});

test("deployment declares resource requests and limits", { skip: skipIfAbsent }, async () => {
  const deployment = await readRepoFile(`${vendorRoot}/k8s/ec2/dd-athleto-backend.deployment.yaml`);

  assert.match(deployment, /requests:[\s\S]*?cpu:\s*100m/);
  assert.match(deployment, /requests:[\s\S]*?memory:\s*256Mi/);
  assert.match(deployment, /limits:[\s\S]*?cpu:\s*'?1'?/);
  assert.match(deployment, /limits:[\s\S]*?memory:\s*2Gi/);
});

test("deployment encodes the shipped securityContext hardening", { skip: skipIfAbsent }, async () => {
  const deployment = await readRepoFile(`${vendorRoot}/k8s/ec2/dd-athleto-backend.deployment.yaml`);

  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /capabilities:\s*\n\s*drop:\s*\n\s*-\s*ALL/);
  assert.match(deployment, /seccompProfile:\s*\n\s*type:\s*RuntimeDefault/);
});

test("deployment ships a startupProbe plus readiness/liveness probes on /healthz", { skip: skipIfAbsent }, async () => {
  const deployment = await readRepoFile(`${vendorRoot}/k8s/ec2/dd-athleto-backend.deployment.yaml`);

  assert.match(deployment, /startupProbe:\s*\n\s*httpGet:\s*\n\s*path:\s*\/healthz/);
  assert.match(deployment, /readinessProbe:\s*\n\s*httpGet:\s*\n\s*path:\s*\/healthz/);
  assert.match(deployment, /livenessProbe:\s*\n\s*httpGet:\s*\n\s*path:\s*\/healthz/);
  // The cold-start cargo build needs a long startup grace window.
  assert.match(deployment, /startupProbe:[\s\S]*?failureThreshold:\s*120/);
});

test("backend Cargo.toml declares axum with the ws feature and the MASH stack", { skip: skipIfAbsent }, async () => {
  const cargo = await readRepoFile(`${vendorRoot}/Cargo.toml`);

  assert.match(cargo, /name\s*=\s*"athleto-backend"/);
  assert.match(cargo, /axum\s*=\s*\{[^}]*features\s*=\s*\[[^\]]*"ws"[^\]]*\]/);
  assert.match(cargo, /maud\s*=/);
  assert.match(cargo, /sea-orm\s*=/);
});

test("backend readme documents the /ws and /readyz surface", { skip: skipIfAbsent }, async () => {
  const readme = await readRepoFile(`${vendorRoot}/readme.md`);

  assert.match(readme, /`GET \/ws`|\/ws`/);
  assert.match(readme, /`GET \/readyz`|\/readyz`/);
  assert.match(readme, /`GET \/healthz`|\/healthz`/);
});
