#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const ROOT = new URL("../", import.meta.url);

export const applications = Object.freeze([
  {
    file: "remote/argocd/apps/shared-auth-web.application.yaml",
    name: "dd-shared-auth-web",
    repo: "git@github.com:shared-auth/shared-auth-web-server.rs.git",
    realm: "customer",
  },
  {
    file: "remote/argocd/apps/shared-auth-api.application.yaml",
    name: "dd-shared-auth-api",
    repo: "git@github.com:shared-auth/shared-auth-api-server.rs.git",
    realm: "customer",
  },
  {
    file: "remote/argocd/apps/shared-auth-admin-web.application.yaml",
    name: "dd-shared-auth-admin-web",
    repo: "git@github.com:shared-auth/shared-auth-admin-web-server.rs.git",
    realm: "admin",
  },
  {
    file: "remote/argocd/apps/shared-auth-admin-api.application.yaml",
    name: "dd-shared-auth-admin-api",
    repo: "git@github.com:shared-auth/shared-auth-admin-api-server.rs.git",
    realm: "admin",
  },
]);

export function validateApplication(text, expected) {
  const errors = [];
  const requires = (pattern, message) => {
    if (!pattern.test(text)) errors.push(message);
  };
  const forbids = (pattern, message) => {
    if (pattern.test(text)) errors.push(message);
  };

  requires(new RegExp(`\\n  name: ${escapeRegex(expected.name)}\\n`), "Application name drifted");
  requires(new RegExp(`repoURL: ${escapeRegex(expected.repo)}`), "Application must track its upstream repository directly");
  requires(/targetRevision: main/, "Application must promote only the service default branch");
  requires(/path: k8s/, "Application source path must be k8s");
  requires(/project: shared-auth/, "Application must use the strict shared-auth AppProject");
  requires(/namespace: shared-auth/, "Application destination must be namespace shared-auth");
  requires(/oresoftware\.com\/activation-state: inert/, "Application must remain explicitly inert");
  requires(/oresoftware\.com\/linear-issue: DEN-606/, "Application must retain DEN-606 traceability");
  requires(new RegExp(`oresoftware\\.com/realm: ${expected.realm}`), "Application realm annotation drifted");
  requires(/CreateNamespace=false/, "Application must not create the platform-owned namespace");
  requires(/ServerSideApply=true/, "Application must use server-side apply");
  requires(/PruneLast=true/, "Application must prune only after apply");
  forbids(/\nautomated\s*:/, "Application must not enable automated sync before activation evidence");
  forbids(/(?:token|password|secret|privateKey)\s*:\s*[^#\s][^\n]*/i, "Application must not embed credential values");
  return errors;
}

export function validateProject(text) {
  const errors = [];
  const requiredRepos = [
    "git@github.com:shared-auth/shared-auth-web-server.rs.git",
    "git@github.com:shared-auth/shared-auth-api-server.rs.git",
    "git@github.com:shared-auth/shared-auth-admin-web-server.rs.git",
    "git@github.com:shared-auth/shared-auth-admin-api-server.rs.git",
    "git@github.com:shared-auth/shared-auth-server.rs.git",
    "git@github.com:shared-auth/shared-auth-nats-bridge.rs.git",
  ];
  for (const repo of requiredRepos) {
    if (!text.includes(`- "${repo}"`)) errors.push(`AppProject is missing ${repo}`);
  }
  for (const required of [
    "clusterResourceWhitelist: []",
    "namespace: shared-auth",
    "kind: Namespace",
    "kind: ResourceQuota",
    "kind: LimitRange",
    "kind: RoleBinding",
    "oresoftware.com/linear-issue: DEN-606",
  ]) {
    if (!text.includes(required)) errors.push(`AppProject is missing ${required}`);
  }
  if (/\n\s*clusterResourceWhitelist:\s*\n\s*-/.test(text)) {
    errors.push("AppProject must not allow cluster-scoped resources");
  }
  return errors;
}

export async function validateRepository(root = ROOT) {
  const errors = [];
  for (const expected of applications) {
    const text = await readFile(new URL(expected.file, root), "utf8");
    for (const error of validateApplication(text, expected)) {
      errors.push(`${expected.file}: ${error}`);
    }
  }
  const projectPath = "remote/argocd/projects/shared-auth.appproject.yaml";
  const project = await readFile(new URL(projectPath, root), "utf8");
  for (const error of validateProject(project)) errors.push(`${projectPath}: ${error}`);
  const legacy = await readFile(
    new URL("remote/argocd/apps/shared-auth.application.yaml", root),
    "utf8",
  );
  if (!legacy.includes("name: dd-shared-auth")) {
    errors.push("legacy rollback Application must remain registered during migration");
  }
  return errors;
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function main() {
  const errors = await validateRepository();
  if (errors.length > 0) {
    for (const error of errors) console.error(`- ${error}`);
    process.exitCode = 1;
    return;
  }
  console.log("Shared Auth ArgoCD registrations are direct, isolated, and inert");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
