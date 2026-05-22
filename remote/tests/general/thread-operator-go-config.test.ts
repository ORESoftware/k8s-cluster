import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/thread-operator-go/go.mod'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('dd-thread-operator-go is an opt-in CRD-driven thread controller', async () => {
  const goMod = await readRepoFile('remote/deployments/thread-operator-go/go.mod');
  const main = await readRepoFile('remote/deployments/thread-operator-go/cmd/operator/main.go');
  const types = await readRepoFile(
    'remote/deployments/thread-operator-go/api/v1alpha1/thread_types.go',
  );
  const builders = await readRepoFile(
    'remote/deployments/thread-operator-go/internal/controller/builders.go',
  );
  const reconciler = await readRepoFile(
    'remote/deployments/thread-operator-go/internal/controller/thread_controller.go',
  );

  assert.match(
    goMod,
    /module github\.com\/ORESoftware\/k8s-cluster\/remote\/deployments\/thread-operator-go/,
  );
  assert.match(goMod, /sigs\.k8s\.io\/controller-runtime/);
  assert.match(goMod, /k8s\.io\/client-go/);

  // main.go must wire the controller manager + leader election + probes.
  assert.match(main, /ctrl\.NewManager/);
  assert.match(main, /ThreadReconciler/);
  assert.match(main, /AddHealthzCheck/);
  assert.match(main, /AddReadyzCheck/);
  assert.match(main, /SetupSignalHandler/);
  assert.match(main, /metrics-bind-address/);

  // CR contract: v1alpha1, opt-in fields, idle/sleep/TTL knobs.
  assert.match(types, /package v1alpha1/);
  assert.match(types, /ThreadSpec/);
  assert.match(types, /ThreadID\s+string/);
  assert.match(types, /UserID\s+string/);
  assert.match(types, /IngressHost\s+string/);
  assert.match(types, /Image\s+string/);
  assert.match(types, /DesiredState\s+ThreadDesiredState/);
  assert.match(types, /IdleTimeoutSeconds\s+int64/);
  assert.match(types, /TTLSecondsAfterIdle\s+\*int64/);
  assert.match(types, /LastActivityAt\s+\*metav1\.Time/);
  assert.match(types, /ThreadDesiredStateRunning ThreadDesiredState = "Running"/);
  assert.match(types, /ThreadDesiredStateSleeping ThreadDesiredState = "Sleeping"/);

  // Safety contract: managed-by label + structural refusal to adopt.
  assert.match(builders, /ManagedByLabel\s+= "dd\.dev\/managed-by"/);
  assert.match(builders, /ManagedByValue\s+= "dd-thread-operator"/);
  assert.match(builders, /func HasManagedByLabel/);
  assert.match(reconciler, /HasManagedByLabel\(existing\.Labels\)/);
  assert.match(reconciler, /UnmanagedConflict/);
  assert.match(reconciler, /controllerutil\.SetControllerReference/);

  // Secure pod spec for the per-thread Pod the operator builds.
  assert.match(builders, /AutomountServiceAccountToken: ptrBool\(false\)/);
  assert.match(builders, /RunAsNonRoot:\s+&runAsNonRoot/);
  assert.match(builders, /AllowPrivilegeEscalation:\s+&allowPrivilegeEscalation/);
  assert.match(builders, /Drop:\s+\[\]corev1\.Capability\{"ALL"\}/);
  assert.match(builders, /SeccompProfileTypeRuntimeDefault/);
  assert.match(builders, /RecreateDeploymentStrategyType/);

  // Update-when-changed hardening: the reconciler must skip Update on no-diff.
  assert.match(reconciler, /equality\.Semantic\.DeepEqual/);
  assert.match(reconciler, /InSync/);

  // TTL math hardening: nil LastActivityAt must not request a long custom requeue.
  assert.match(reconciler, /func evaluateTTL/);
  assert.match(
    reconciler,
    /if t\.Spec\.LastActivityAt == nil \{[\s\S]*?return false, 0/,
  );
});

test('dd-thread-operator manifests pin least-privilege RBAC and CRD shape', async () => {
  const crd = await readRepoFile(
    'remote/deployments/thread-operator-go/k8s/ec2/00-crd-thread.yaml',
  );
  const rbac = await readRepoFile(
    'remote/deployments/thread-operator-go/k8s/ec2/01-rbac.yaml',
  );
  const dep = await readRepoFile(
    'remote/deployments/thread-operator-go/k8s/ec2/02-deployment.yaml',
  );
  const svc = await readRepoFile(
    'remote/deployments/thread-operator-go/k8s/ec2/03-service.yaml',
  );
  const kustomization = await readRepoFile(
    'remote/deployments/thread-operator-go/k8s/ec2/kustomization.yaml',
  );

  // CRD: dd.dev/v1alpha1 Thread, namespaced, with status subresource and required spec keys.
  assert.match(crd, /name: threads\.dd\.dev/);
  assert.match(crd, /group: dd\.dev/);
  assert.match(crd, /scope: Namespaced/);
  assert.match(crd, /name: v1alpha1/);
  assert.match(crd, /subresources:\s*\n\s*status: \{\}/);
  for (const key of ['threadId', 'userId', 'ingressHost', 'image']) {
    assert.match(crd, new RegExp(`-\\s+${key}`));
  }
  assert.match(crd, /enum: \[Running, Sleeping\]/);
  assert.match(crd, /enum: \[Pending, Active, Sleeping, Failed, Terminating\]/);

  // RBAC: cluster-scoped only on Threads + events; namespace-scoped CRUD on
  // child resources in dd-dev only; explicit Lease role for leader election.
  assert.match(rbac, /kind: ClusterRole\s*\nmetadata:\s*\n\s*name: dd-thread-operator-cr-watch/);
  assert.match(rbac, /apiGroups: \['dd\.dev'\]\s*\n\s*resources: \[threads\]/);
  assert.match(rbac, /kind: Role\s*\nmetadata:\s*\n\s*name: dd-thread-operator-children/);
  assert.match(rbac, /namespace: dd-dev/);
  assert.match(rbac, /resources: \[deployments\]/);
  assert.match(rbac, /resources: \[services, persistentvolumeclaims\]/);
  assert.match(rbac, /resources: \[ingresses\]/);
  assert.doesNotMatch(rbac, /resources: \[secrets\]/);
  assert.doesNotMatch(rbac, /resources: \[configmaps\]/);
  assert.match(rbac, /name: dd-thread-operator-leader-election/);
  assert.match(rbac, /resources: \[leases\]/);

  // Deployment: secure pod + container securityContext, hostPath build mount.
  assert.match(dep, /name: dd-thread-operator/);
  assert.match(dep, /securityContext:\s*\n\s*runAsNonRoot: true/);
  assert.match(dep, /runAsUser: 1000/);
  assert.match(dep, /allowPrivilegeEscalation: false/);
  assert.match(dep, /drop: \[ALL\]/);
  assert.match(dep, /type: RuntimeDefault/);
  assert.match(dep, /--leader-elect=true/);
  assert.match(dep, /\/home\/ec2-user\/codes\/dd\/dd-next-1\/remote\/deployments\/thread-operator-go/);

  // Service: ClusterIP, metrics on 9101.
  assert.match(svc, /type: ClusterIP/);
  assert.match(svc, /port: 9101/);

  // kustomization aggregates all four files.
  for (const f of [
    '00-crd-thread.yaml',
    '01-rbac.yaml',
    '02-deployment.yaml',
    '03-service.yaml',
  ]) {
    assert.match(kustomization, new RegExp(`-\\s+${f.replace('.', '\\.')}`));
  }
});

test('dd-thread-operator argocd Application points at this overlay', async () => {
  const app = await readRepoFile(
    'remote/argocd/apps/dd-thread-operator.application.yaml',
  );

  assert.match(app, /kind: Application/);
  assert.match(app, /name: dd-thread-operator/);
  assert.match(app, /repoURL: git@github\.com:ORESoftware\/k8s-cluster\.git/);
  assert.match(app, /path: remote\/deployments\/thread-operator-go\/k8s\/ec2/);
  assert.match(app, /automated:\s*\n\s*prune: true\s*\n\s*selfHeal: true/);
});
