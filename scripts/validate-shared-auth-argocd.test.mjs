import assert from "node:assert/strict";
import test from "node:test";

import {
  applications,
  validateApplication,
  validateProject,
  validateRepository,
} from "./validate-shared-auth-argocd.mjs";

function applicationFixture(expected) {
  return `
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: ${expected.name}
  namespace: argocd
  annotations:
    oresoftware.com/activation-state: inert
    oresoftware.com/linear-issue: DEN-606
    oresoftware.com/realm: ${expected.realm}
spec:
  project: shared-auth
  source:
    repoURL: ${expected.repo}
    targetRevision: main
    path: k8s
  destination:
    server: https://kubernetes.default.svc
    namespace: shared-auth
  syncPolicy:
    syncOptions:
      - CreateNamespace=false
      - ServerSideApply=true
      - PruneLast=true
`;
}

test("checked-in Shared Auth registrations satisfy the migration contract", async () => {
  assert.deepEqual(await validateRepository(), []);
});

test("each canonical direct application fixture is accepted", () => {
  for (const expected of applications) {
    assert.deepEqual(validateApplication(applicationFixture(expected), expected), []);
  }
});

test("automated sync, repository indirection, and realm collapse fail closed", () => {
  const expected = applications[2];
  const automated = applicationFixture(expected).replace(
    "syncPolicy:\n",
    "syncPolicy:\n    automated: { prune: true, selfHeal: true }\n",
  );
  assert.match(validateApplication(automated, expected).join("\n"), /automated sync/);

  const submodule = applicationFixture(expected).replace(
    expected.repo,
    "git@github.com:ORESoftware/k8s-cluster.git",
  );
  assert.match(validateApplication(submodule, expected).join("\n"), /upstream repository directly/);

  const customerRealm = applicationFixture(expected).replace(
    "oresoftware.com/realm: admin",
    "oresoftware.com/realm: customer",
  );
  assert.match(validateApplication(customerRealm, expected).join("\n"), /realm annotation drifted/);
});

test("the AppProject must keep all four services and forbid cluster resources", () => {
  const project = `
metadata:
  annotations:
    oresoftware.com/linear-issue: DEN-606
spec:
  sourceRepos:
    - "git@github.com:shared-auth/shared-auth-web-server.rs.git"
    - "git@github.com:shared-auth/shared-auth-api-server.rs.git"
    - "git@github.com:shared-auth/shared-auth-admin-web-server.rs.git"
    - "git@github.com:shared-auth/shared-auth-admin-api-server.rs.git"
    - "git@github.com:shared-auth/shared-auth-server.rs.git"
    - "git@github.com:shared-auth/shared-auth-nats-bridge.rs.git"
  destinations:
    - namespace: shared-auth
  clusterResourceWhitelist: []
  namespaceResourceBlacklist:
    - { kind: Namespace }
    - { kind: ResourceQuota }
    - { kind: LimitRange }
    - { kind: RoleBinding }
`;
  assert.deepEqual(validateProject(project), []);
  assert.match(
    validateProject(project.replace("clusterResourceWhitelist: []", "clusterResourceWhitelist:\n    - { group: '*', kind: '*' }")).join("\n"),
    /must not allow cluster-scoped resources|missing clusterResourceWhitelist/,
  );
  assert.match(
    validateProject(project.replace(/.*shared-auth-admin-api-server.*\n/, "")).join("\n"),
    /admin-api-server/,
  );
});
