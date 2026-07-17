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

// Per-server hardening pins for the Rust fleet (2026-07 audit). Pure
// config-file assertions, one test per Rust server, so a manifest edit that
// silently drops a securityContext field, probe, NetworkPolicy, or PDB fails
// CI instead of landing on the cluster.

const RUNTIME_DIR = "remote/argocd/dd-next-runtime";
const FIDUCIA_DIR = "remote/argocd/fiducia";

const CAPS_DROP_ALL = /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/;
const SECCOMP_RUNTIME_DEFAULT = /seccompProfile:\s*\n\s*type: RuntimeDefault/;
const NO_PRIV_ESCALATION = /allowPrivilegeEscalation: false/;

async function assertNetworkPolicyRegistered(dir: string, name: string): Promise<string> {
  const policyPath = `${dir}/${name}.networkpolicy.yaml`;
  assert.ok(
    existsSync(resolve(repoRoot, policyPath)),
    `expected ${policyPath} to exist`,
  );

  const kustomization = await readRepoFile(`${dir}/kustomization.yaml`);
  assert.match(
    kustomization,
    new RegExp(`- ${name.replace(/[.*+?^${}()|[\\]\\\\]/g, "\\$&")}\\.networkpolicy\\.yaml`),
    `expected ${name}.networkpolicy.yaml registered in ${dir}/kustomization.yaml`,
  );

  const policy = await readRepoFile(policyPath);
  assert.match(policy, /kind: NetworkPolicy/);
  assert.match(policy, new RegExp(`app: ${name}`));
  return policy;
}

test("dd-remote-rest-api: networkpolicy + non-root cargo pod with read-only source mount", async () => {
  await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-remote-rest-api");

  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-remote-rest-api.deployment.yaml`);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, NO_PRIV_ESCALATION);
  // Source checkout stays read-only; all writes go to the emptyDir /tmp.
  assert.match(deployment, /mountPath: \/opt\/dd-next-1\s*\n\s*readOnly: true/);
  assert.match(deployment, /emptyDir: \{\}/);
});

test("dd-remote-web-home (athlet-o server): read-only rootfs + no public egress", async () => {
  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-remote-web-home.deployment.yaml`);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /mountPath: \/tmp/);
  assert.match(deployment, /emptyDir: \{\}/);

  const policy = await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-remote-web-home");
  // Prebuilt image: unlike the cargo-run pods there is no cold-build fetch,
  // so the policy must not open public internet egress.
  assert.doesNotMatch(policy, /0\.0\.0\.0\/0/);
});

test("dd-rust-wss-server: networkpolicy + container hardening trio", async () => {
  await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-rust-wss-server");

  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-rust-wss-server.deployment.yaml`);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, NO_PRIV_ESCALATION);
  assert.match(deployment, /automountServiceAccountToken: false/);
});

test("dd-trading-server: networkpolicy + container hardening trio", async () => {
  await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-trading-server");

  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-trading-server.deployment.yaml`);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, NO_PRIV_ESCALATION);
  assert.match(deployment, /automountServiceAccountToken: false/);
});

test("dd-economics-server: networkpolicy + non-root read-only-rootfs build pod", async () => {
  await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-economics-server");

  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-economics-server.deployment.yaml`);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
});

test("dd-soccer-rs: networkpolicy + non-root read-only-rootfs build pod", async () => {
  await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-soccer-rs");

  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-soccer-rs.deployment.yaml`);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, /automountServiceAccountToken: false/);
});

test("dd-agent-sim-server: networkpolicy + container hardening trio", async () => {
  await assertNetworkPolicyRegistered(RUNTIME_DIR, "dd-agent-sim-server");

  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-agent-sim-server.deployment.yaml`);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, NO_PRIV_ESCALATION);
  assert.match(deployment, /automountServiceAccountToken: false/);
});

test("dd-idle-reaper: exec probes + seccomp; API token stays intentional via RBAC", async () => {
  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-idle-reaper.deployment.yaml`);
  // No HTTP surface (reqwest/NATS only), so liveness is pgrep on the compiled
  // binary name — it never matches cargo/rustc during the cold build.
  assert.match(deployment, /startupProbe:[\s\S]{0,120}\/usr\/bin\/pgrep/);
  assert.match(deployment, /livenessProbe:[\s\S]{0,120}\/usr\/bin\/pgrep/);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, NO_PRIV_ESCALATION);
  // The reaper drives the Kubernetes API on purpose; the token mount is paired
  // with least-privilege RBAC rather than removed.
  assert.match(deployment, /automountServiceAccountToken: true/);
  assert.ok(
    existsSync(resolve(repoRoot, `${RUNTIME_DIR}/dd-idle-reaper-rbac.yaml`)),
    "expected dd-idle-reaper-rbac.yaml beside the deployment",
  );
});

test("dd-browser-job-runner: no API token, bounded resources, full probe set", async () => {
  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-browser-job-runner.deployment.yaml`);
  // privileged + hostNetwork are by design (nerdctl spawner); the pins here
  // are the compensating controls around that posture.
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /limits:\s*\n\s*cpu:/);
  assert.match(deployment, /startupProbe:/);
  assert.match(deployment, /readinessProbe:/);
  assert.match(deployment, /livenessProbe:/);
});

test("dd-document-rs: prebuilt image runs non-root with read-only rootfs", async () => {
  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-document-rs.deployment.yaml`);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, /automountServiceAccountToken: false/);
});

test("dd-ocr-rs: prebuilt image runs non-root with read-only rootfs", async () => {
  const deployment = await readRepoFile(`${RUNTIME_DIR}/dd-ocr-rs.deployment.yaml`);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, CAPS_DROP_ALL);
  assert.match(deployment, SECCOMP_RUNTIME_DEFAULT);
  assert.match(deployment, /automountServiceAccountToken: false/);
});

test("fiducia namespace enforces baseline PSA and audits restricted", async () => {
  const namespace = await readRepoFile(`${FIDUCIA_DIR}/namespace.yaml`);
  assert.match(namespace, /pod-security\.kubernetes\.io\/enforce: baseline/);
  assert.match(namespace, /pod-security\.kubernetes\.io\/audit: restricted/);
  assert.match(namespace, /pod-security\.kubernetes\.io\/warn: restricted/);
});

test("fiducia-admin: networkpolicy registered", async () => {
  await assertNetworkPolicyRegistered(FIDUCIA_DIR, "fiducia-admin");
});

test("fiducia-auth: networkpolicy registered", async () => {
  await assertNetworkPolicyRegistered(FIDUCIA_DIR, "fiducia-auth");
});

test("fiducia-backend: networkpolicy + PDB above the HPA floor", async () => {
  await assertNetworkPolicyRegistered(FIDUCIA_DIR, "fiducia-backend");

  const pdbs = await readRepoFile(`${FIDUCIA_DIR}/availability-pdbs.yaml`);
  assert.match(pdbs, /name: fiducia-backend[\s\S]{0,400}?minAvailable: 1/);
});

test("fiducia-load-balance: networkpolicy registered", async () => {
  await assertNetworkPolicyRegistered(FIDUCIA_DIR, "fiducia-load-balance");
});

test("fiducia-brain: networkpolicy + raft-quorum PDB + RBAC-paired API token", async () => {
  await assertNetworkPolicyRegistered(FIDUCIA_DIR, "fiducia-brain");

  const pdbs = await readRepoFile(`${FIDUCIA_DIR}/availability-pdbs.yaml`);
  assert.match(pdbs, /name: fiducia-brain[\s\S]{0,400}?minAvailable: 2/);

  // The KubeOracle reads pod state from the API server: the pod-level token
  // mount is deliberate and paired with a namespaced read-only Role.
  const statefulset = await readRepoFile(`${FIDUCIA_DIR}/fiducia-brain.statefulset.yaml`);
  assert.match(statefulset, /automountServiceAccountToken: true/);
  assert.match(statefulset, /kind: Role\b/);
  assert.match(statefulset, /kind: RoleBinding\b/);
});

test("fiducia-node: networkpolicy + raft-quorum PDB", async () => {
  await assertNetworkPolicyRegistered(FIDUCIA_DIR, "fiducia-node");

  const pdbs = await readRepoFile(`${FIDUCIA_DIR}/availability-pdbs.yaml`);
  assert.match(pdbs, /name: fiducia-node[\s\S]{0,400}?minAvailable: 2/);
});
