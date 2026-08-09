import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const canonicalApplicationsPath = "remote/argocd/apps/zed.applications.yaml";
const canonicalProjectPath = "remote/argocd/projects/zed.appproject.yaml";
const candidateApplicationName = "dd-zed-account-registry-candidate";
const candidateInfraRepository = "https://github.com/zed-pkg/zed-infra.git";
const candidateInfraRevision = "14cf61e795e3d4763785aebc9498c96c23dab60f";
const candidateInfraPath = "k8s/overlays/k8s-cluster";

const stages = [
  { name: "zed-tenant-bootstrap", include: "zed.tenant.yaml", wave: "-40" },
  { name: "zed-project-bootstrap", include: "zed.appproject.yaml", wave: "-30" },
  { name: "zed-postgres-bootstrap", include: "zed.bootstrap-postgres.yaml", wave: "-20" },
  { name: "zed-applications-bootstrap", include: "zed.applications.yaml", wave: "-10" }
];

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function applicationSection(document, applicationName) {
  const marker = `name: ${applicationName}`;
  const markerIndex = document.indexOf(marker);
  assert.notEqual(markerIndex, -1, `${applicationName} must exist`);

  const previousSeparator = document.lastIndexOf("\n---\n", markerIndex);
  const startIndex = previousSeparator === -1 ? 0 : previousSeparator + 5;
  const nextSeparator = document.indexOf("\n---\n", markerIndex);
  return document.slice(startIndex, nextSeparator === -1 ? document.length : nextSeparator);
}

for (const cluster of ["aws", "hetzner"]) {
  test(`${cluster} root surfaces the ordered Zed bootstrap chain`, async () => {
    const clusterRoot = resolve(repoRoot, `remote/argocd/clusters/${cluster}`);
    const kustomization = await readFile(resolve(clusterRoot, "kustomization.yaml"), "utf8");
    const bootstrap = await readFile(
      resolve(clusterRoot, "zed.bootstrap.application.yaml"),
      "utf8"
    );

    assert.match(
      kustomization,
      /^\s*-\s+zed\.bootstrap\.application\.yaml\s*$/m,
      `${cluster} root must include the Zed bootstrap manifest`
    );
    assert.equal(
      [...bootstrap.matchAll(/^kind:\s+Application\s*$/gm)].length,
      stages.length,
      `${cluster} bootstrap must contain exactly one Application per stage`
    );

    let previousIndex = -1;
    for (let index = 0; index < stages.length; index += 1) {
      const stage = stages[index];
      const stageIndex = bootstrap.indexOf(`name: ${stage.name}`);
      assert.ok(stageIndex > previousIndex, `${stage.name} must follow the preceding stage`);

      const nextStage = stages[index + 1];
      const endIndex = nextStage
        ? bootstrap.indexOf(`name: ${nextStage.name}`, stageIndex)
        : bootstrap.length;
      const section = bootstrap.slice(stageIndex, endIndex);

      assert.match(section, /project:\s+infrastructure\b/);
      assert.match(
        section,
        new RegExp(`argocd\\.argoproj\\.io/sync-wave:\\s+"${escapeRegExp(stage.wave)}"`)
      );
      assert.match(
        section,
        new RegExp(`include:\\s+${escapeRegExp(stage.include)}(?:\\s|$)`)
      );
      previousIndex = stageIndex;
    }
  });
}

test("the bootstrap targets existing canonical manifests", async () => {
  const requiredPaths = [
    "remote/argocd/projects/zed.tenant.yaml",
    canonicalProjectPath,
    "remote/argocd/projects/zed.bootstrap-postgres.yaml",
    canonicalApplicationsPath
  ];

  await Promise.all(requiredPaths.map((path) => readFile(resolve(repoRoot, path), "utf8")));

  const applications = await readFile(resolve(repoRoot, canonicalApplicationsPath), "utf8");
  const applicationCount = [...applications.matchAll(/^kind:\s+Application\s*$/gm)].length;
  const zedProjectCount = [...applications.matchAll(/^\s*project:\s+zed\s*$/gm)].length;
  assert.ok(applicationCount > 0, "the canonical Zed application manifest must define children");
  assert.equal(
    zedProjectCount,
    applicationCount,
    "every child Zed Application must use the dedicated Zed AppProject"
  );

  const catalog = await readFile(resolve(repoRoot, "catalog/applications.json"), "utf8");
  assert.match(
    catalog,
    new RegExp(escapeRegExp(canonicalApplicationsPath)),
    "the catalog must retain the canonical, provider-neutral Zed manifest path"
  );
});

test("the account registry candidate is exact-pinned, manual, and collision-safe", async () => {
  const applications = await readFile(resolve(repoRoot, canonicalApplicationsPath), "utf8");
  const section = applicationSection(applications, candidateApplicationName);

  assert.match(section, new RegExp(`repoURL:\\s+${escapeRegExp(candidateInfraRepository)}`));
  assert.match(section, new RegExp(`targetRevision:\\s+${candidateInfraRevision}\\b`));
  assert.match(section, new RegExp(`path:\\s+${escapeRegExp(candidateInfraPath)}\\b`));
  assert.match(section, /^\s*namespace:\s+zed\s*$/m);
  assert.match(section, /^\s*-\s+FailOnSharedResource=true\s*$/m);
  assert.doesNotMatch(
    section,
    /^\s*automated:/m,
    "the candidate must remain manual until release gates and ownership handoff are complete"
  );

  for (const bootstrapApplication of ["dd-zed-api-server", "dd-zed-web-server"]) {
    assert.match(
      applications,
      new RegExp(`name:\\s+${escapeRegExp(bootstrapApplication)}\\b`),
      `${bootstrapApplication} must remain during candidate review`
    );
  }

  const project = await readFile(resolve(repoRoot, canonicalProjectPath), "utf8");
  assert.match(
    project,
    new RegExp(`-\\s+"${escapeRegExp(candidateInfraRepository)}"`),
    "the strict Zed AppProject must explicitly allow the reviewed render source"
  );
  assert.match(project, /clusterResourceWhitelist:\s*\[\]/);
});
