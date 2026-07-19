import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/canonical-cloud/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const overlay = 'remote/argocd/canonical-cloud';

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function imageReference(manifest: string): string {
  const match = manifest.match(/^\s*image:\s*(\S+)\s*$/m);
  assert.ok(match, 'deployment must declare an image');
  return match[1];
}

function releaseShas(manifest: string): string[] {
  return [...manifest.matchAll(/canonical\.cloud\/release-sha: "([0-9a-f]{40})"/g)].map(
    (match) => match[1],
  );
}

test('canonical-cloud has a dedicated, self-healing Argo application on dev', async () => {
  const application = await readRepoFile(
    'remote/argocd/apps/canonical-cloud.application.yaml',
  );
  const namespace = await readRepoFile(`${overlay}/namespace.yaml`);

  assert.match(application, /kind: Application/);
  assert.match(application, /name: canonical-cloud/);
  assert.match(application, /targetRevision: dev/);
  assert.match(application, /path: remote\/argocd\/canonical-cloud/);
  assert.match(application, /destination:[\s\S]*namespace: canonical-cloud/);
  assert.match(application, /automated:\s*\n\s*prune: true\s*\n\s*selfHeal: true/);
  assert.match(application, /CreateNamespace=true/);
  assert.match(application, /ServerSideApply=true/);

  assert.match(namespace, /kind: Namespace/);
  assert.match(namespace, /name: canonical-cloud/);
  assert.match(namespace, /pod-security\.kubernetes\.io\/enforce: restricted/);
});

test('web and no-ingress revoker use one coherent immutable release', async () => {
  const web = await readRepoFile(`${overlay}/web.deployment.yaml`);
  const revoker = await readRepoFile(`${overlay}/revoker.deployment.yaml`);
  const kustomization = await readRepoFile(`${overlay}/kustomization.yaml`);

  const webShas = releaseShas(web);
  const revokerShas = releaseShas(revoker);
  assert.equal(webShas.length, 2);
  assert.equal(revokerShas.length, 2);
  assert.ok(webShas.every((sha) => sha === webShas[0]));
  assert.ok(revokerShas.every((sha) => sha === webShas[0]));

  const webImage = imageReference(web);
  const revokerImage = imageReference(revoker);
  const shaTag = webShas[0];
  assert.match(
    webImage,
    new RegExp(
      `^ghcr\\.io/canonical-cloud/canonical-web-server(?::${shaTag}|@sha256:[0-9a-f]{64})$`,
    ),
  );
  assert.match(
    revokerImage,
    new RegExp(
      `^ghcr\\.io/canonical-cloud/canonical-session-revoker(?::${shaTag}|@sha256:[0-9a-f]{64})$`,
    ),
  );
  assert.doesNotMatch(`${webImage}\n${revokerImage}`, /:(?:main|latest)$/);

  assert.match(web, /args:\s*\n\s*- serve/);
  assert.match(revoker, /args:\s*\n\s*- run/);
  assert.doesNotMatch(web, /git clone|cargo (?:run|build)|npm (?:ci|run)|\bmigrate\b/i);
  assert.doesNotMatch(revoker, /git clone|cargo (?:run|build)|npm (?:ci|run)|\bmigrate\b/i);
  assert.doesNotMatch(`${web}\n${revoker}`, /MIGRATION_DATABASE_URL|argocd\.argoproj\.io\/hook/);
  assert.doesNotMatch(kustomization, /migration|\.job\.yaml/i);
});

test('web exposure preserves the gateway boundary and production probes', async () => {
  const deployment = await readRepoFile(`${overlay}/web.deployment.yaml`);
  const service = await readRepoFile(`${overlay}/web.service.yaml`);
  const policy = await readRepoFile(`${overlay}/web.networkpolicy.yaml`);

  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /capabilities:\s*\n\s*drop:\s*\n\s*- ALL/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz/);
  assert.match(deployment, /containerPort: 8081/);

  assert.match(service, /kind: Service/);
  assert.match(service, /type: ClusterIP/);
  assert.match(service, /port: 8081/);
  assert.match(service, /targetPort: http/);

  assert.match(
    policy,
    /namespaceSelector:[\s\S]*kubernetes\.io\/metadata\.name: default[\s\S]*podSelector:[\s\S]*app: dd-remote-gateway/,
  );
  assert.match(policy, /kubernetes\.io\/metadata\.name: observability/);
  assert.match(policy, /port: 4317/);
  assert.match(policy, /port: 443/);
  assert.match(policy, /port: 5432/);
  assert.doesNotMatch(policy, /port: 80\s*$/m);
});

test('revoker has no network listener, Service, or admitted ingress', async () => {
  const deployment = await readRepoFile(`${overlay}/revoker.deployment.yaml`);
  const policy = await readRepoFile(`${overlay}/revoker.networkpolicy.yaml`);
  const kustomization = await readRepoFile(`${overlay}/kustomization.yaml`);

  assert.match(deployment, /name: canonical-session-revoker/);
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.doesNotMatch(deployment, /^\s*ports:\s*$/m);
  assert.doesNotMatch(deployment, /containerPort:/);
  assert.doesNotMatch(kustomization, /revoker\.service\.yaml|revoker\.ingress\.yaml/);
  assert.equal(existsSync(resolve(repoRoot, overlay, 'revoker.service.yaml')), false);
  assert.match(policy, /policyTypes:[\s\S]*- Ingress/);
  assert.match(policy, /ingress: \[\]/);
  assert.doesNotMatch(policy, /dd-remote-gateway|port: 8081/);
  assert.match(policy, /port: 443/);
  assert.match(policy, /port: 5432/);
  assert.doesNotMatch(policy, /port: 80\s*$/m);
});

test('runtime, worker, and registry credentials are strictly isolated', async () => {
  const webSecret = await readRepoFile(`${overlay}/web.externalsecret.yaml`);
  const revokerSecret = await readRepoFile(`${overlay}/revoker.externalsecret.yaml`);
  const registrySecret = await readRepoFile(`${overlay}/registry.externalsecret.yaml`);
  const webDeployment = await readRepoFile(`${overlay}/web.deployment.yaml`);
  const revokerDeployment = await readRepoFile(`${overlay}/revoker.deployment.yaml`);

  assert.match(webSecret, /key: dd\/remote-dev\/canonical-cloud-web/);
  assert.match(webSecret, /secretKey: DATABASE_URL/);
  assert.match(webSecret, /secretKey: APP_ALLOWED_ORIGINS/);
  assert.doesNotMatch(webSecret, /SESSION_REVOCATION_DATABASE_URL|dataFrom:/);

  assert.match(revokerSecret, /key: dd\/remote-dev\/canonical-cloud-revoker/);
  assert.match(revokerSecret, /secretKey: SESSION_REVOCATION_DATABASE_URL/);
  assert.doesNotMatch(revokerSecret, /secretKey: DATABASE_URL\s*$/m);
  assert.doesNotMatch(revokerSecret, /APP_BASE_URL|APP_ALLOWED_ORIGINS|dataFrom:/);

  assert.match(registrySecret, /type: kubernetes\.io\/dockerconfigjson/);
  assert.match(registrySecret, /key: dd\/remote-dev\/canonical-cloud-ghcr-pull/);
  assert.match(registrySecret, /property: dockerconfigjson/);
  assert.match(webDeployment, /imagePullSecrets:\s*\n\s*- name: canonical-cloud-ghcr-pull/);
  assert.match(
    revokerDeployment,
    /imagePullSecrets:\s*\n\s*- name: canonical-cloud-ghcr-pull/,
  );

  const allSecrets = `${webSecret}\n${revokerSecret}\n${registrySecret}`;
  assert.doesNotMatch(allSecrets, /SERVICE_ROLE|MIGRATION_DATABASE_URL|github_pat_|ghp_/i);
});

test('promotion helper changes only immutable image refs and release annotations', async () => {
  const { renderPromotion } = await import(
    '../../argocd/canonical-cloud/promote-release.mjs'
  );
  const source = [
    'metadata:',
    '  annotations:',
    '    canonical.cloud/release-sha: "1111111111111111111111111111111111111111"',
    'spec:',
    '  template:',
    '    metadata:',
    '      annotations:',
    '        canonical.cloud/release-sha: "1111111111111111111111111111111111111111"',
    '    spec:',
    '      containers:',
    '        - image: ghcr.io/canonical-cloud/canonical-web-server:1111111111111111111111111111111111111111',
    '          args: [serve]',
    '',
  ].join('\n');
  const digest = `sha256:${'b'.repeat(64)}`;
  const releaseSha = 'a'.repeat(40);
  const promoted = renderPromotion(source, {
    repository: 'ghcr.io/canonical-cloud/canonical-web-server',
    digest,
    releaseSha,
    label: 'web test',
  });

  assert.match(promoted, new RegExp(`image: ghcr\\.io/canonical-cloud/canonical-web-server@${digest}`));
  assert.equal((promoted.match(new RegExp(releaseSha, 'g')) ?? []).length, 2);
  assert.match(promoted, /args: \[serve\]/);
  assert.equal(
    renderPromotion(promoted, {
      repository: 'ghcr.io/canonical-cloud/canonical-web-server',
      digest,
      releaseSha,
      label: 'web test',
    }),
    promoted,
  );
  assert.throws(
    () =>
      renderPromotion(source, {
        repository: 'ghcr.io/canonical-cloud/canonical-web-server',
        digest: 'sha256:not-a-digest',
        releaseSha,
        label: 'web test',
      }),
    /digest must match/,
  );
});

test('runbook makes activation, migration, and rollback gates explicit', async () => {
  const readme = await readRepoFile(`${overlay}/README.md`);
  const todos = await readRepoFile('docs/canonical-cloud-deployment-todos.md');

  assert.match(readme, /canonical-cloud\/canonical-monorepo.*only deployable source/s);
  assert.match(readme, /never deploys the umbrella `canonical\.cloud`/);
  assert.match(readme, /Argo CD is the only runtime writer/);
  assert.match(readme, /Do not apply .*canonical-cloud\.application\.yaml.*until/s);
  assert.match(readme, /Migrations are never an Argo sync hook/);
  assert.match(readme, /dedicated HTTPS backend origin/);
  assert.match(readme, /WebSocket.*Upgrade.*Connection/s);
  assert.match(readme, /Rollback is a Git revert/);
  assert.match(readme, /legacy `dd-canonical-cloud`/);
  assert.match(readme, /--check/);
  assert.match(readme, /canonical-cloud-deployment-todos\.md/);

  assert.match(todos, /canonical-cloud\/canonical-monorepo.*only deployable source/s);
  assert.match(todos, /GitHub App/);
  assert.match(todos, /digest-only promotion PR/);
  assert.match(todos, /Retire the umbrella checkout/);
  assert.match(todos, /Never make a pod clone or build/);
  assert.match(todos, /No kubeconfig/);
  assert.match(todos, /No automatic schema migration/);
});
