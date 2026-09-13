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
  return [
    ...manifest.matchAll(
      /^[ \t]*canonical\.cloud\/release-sha:[ \t]*"([0-9a-f]{40})"[ \t]*$/gm,
    ),
  ].map((match) => match[1]);
}

test('web, API, and revoker are promoted as one immutable release', async () => {
  const web = await readRepoFile(`${overlay}/web.deployment.yaml`);
  const api = await readRepoFile(`${overlay}/api.deployment.yaml`);
  const revoker = await readRepoFile(`${overlay}/revoker.deployment.yaml`);

  const webShas = releaseShas(web);
  const apiShas = releaseShas(api);
  const revokerShas = releaseShas(revoker);
  assert.equal(webShas.length, 2);
  assert.equal(apiShas.length, 2);
  assert.equal(revokerShas.length, 2);
  const releaseSha = webShas[0];
  assert.ok([...webShas, ...apiShas, ...revokerShas].every((sha) => sha === releaseSha));

  const webImage = imageReference(web);
  const apiImage = imageReference(api);
  const revokerImage = imageReference(revoker);
  assert.match(
    webImage,
    new RegExp(
      `^ghcr\\.io/canonical-cloud/(?:canonical-web-server|canonical-web-server-rs)(?::(?:web-)?${releaseSha}|@sha256:[0-9a-f]{64})$`,
    ),
  );
  assert.match(
    apiImage,
    new RegExp(
      `^ghcr\\.io/canonical-cloud/canonical-api-server(?::${releaseSha}|@sha256:[0-9a-f]{64})$`,
    ),
  );
  assert.match(
    revokerImage,
    new RegExp(
      `^ghcr\\.io/canonical-cloud/(?:canonical-session-revoker|canonical-web-server-rs)(?::(?:revoker-)?${releaseSha}|@sha256:[0-9a-f]{64})$`,
    ),
  );
  assert.doesNotMatch(`${webImage}\n${apiImage}\n${revokerImage}`, /:(?:main|latest)$/);
});

test('standalone API workload is hardened and independently authenticated', async () => {
  const deployment = await readRepoFile(`${overlay}/api.deployment.yaml`);
  const service = await readRepoFile(`${overlay}/api.service.yaml`);
  const policy = await readRepoFile(`${overlay}/api.networkpolicy.yaml`);

  assert.match(deployment, /name: canonical-api-server/);
  assert.match(deployment, /replicas: 2/);
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /runAsNonRoot: true/);
  assert.match(deployment, /runAsUser: 65532/);
  assert.match(deployment, /readOnlyRootFilesystem: true/);
  assert.match(deployment, /allowPrivilegeEscalation: false/);
  assert.match(deployment, /capabilities:\s*\n\s*drop: \["ALL"\]/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/readyz/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz/);
  assert.match(deployment, /containerPort: 8081/);
  assert.match(deployment, /APP_BASE_URL, value: "https:\/\/api\.canonical\.plus"/);
  assert.match(
    deployment,
    /SHARED_AUTH_VERIFY_URL, value: "http:\/\/dd-shared-auth-canonical-plus\.shared-auth\.svc\.cluster\.local:8120\/auth\/verify"/,
  );
  assert.match(deployment, /GEMINI_MODEL, value: "gemini-3\.6-pro"/);
  assert.match(deployment, /imagePullSecrets:\s*\n\s*- name: canonical-cloud-ghcr-pull/);

  assert.match(service, /kind: Service/);
  assert.match(service, /type: ClusterIP/);
  assert.match(service, /port: 8081/);
  assert.match(service, /targetPort: http/);

  assert.match(policy, /kubernetes\.io\/metadata\.name: ingress-nginx/);
  assert.match(policy, /kubernetes\.io\/metadata\.name: shared-auth/);
  assert.match(policy, /app\.kubernetes\.io\/instance: canonical-plus/);
  assert.match(policy, /port: 8120/);
  assert.match(policy, /cidr: 172\.31\.0\.0\/16/);
  assert.match(policy, /port: 5432/);
  assert.match(policy, /port: 443/);
  assert.match(policy, /port: 4317/);
  assert.doesNotMatch(policy, /port: 80\s*$/m);
});

test('public ingress sends browser pages to web and quote APIs to API', async () => {
  const ingress = await readRepoFile(`${overlay}/public.ingress.yaml`);
  const kustomization = await readRepoFile(`${overlay}/kustomization.yaml`);
  const webPolicy = await readRepoFile(`${overlay}/web.networkpolicy.yaml`);

  assert.match(ingress, /ingressClassName: nginx/);
  assert.match(ingress, /host: app\.canonical\.plus/);
  assert.match(ingress, /host: api\.canonical\.plus/);
  assert.match(ingress, /proxy-buffering: "off"/);
  assert.match(ingress, /proxy-read-timeout: "3600"/);
  assert.match(ingress, /proxy-send-timeout: "3600"/);
  assert.match(
    ingress,
    /host: app\.canonical\.plus[\s\S]*path: \/api\/v1\/quotes[\s\S]*name: canonical-api-server[\s\S]*path: \/[\s\S]*name: canonical-cloud-web/,
  );
  assert.match(
    ingress,
    /host: api\.canonical\.plus[\s\S]*path: \/[\s\S]*name: canonical-api-server/,
  );

  for (const resource of [
    'api.externalsecret.yaml',
    'api.deployment.yaml',
    'api.service.yaml',
    'api.networkpolicy.yaml',
    'public.ingress.yaml',
  ]) {
    assert.match(kustomization, new RegExp(`- ${resource.replace('.', '\\.')}`));
  }
  assert.match(webPolicy, /kubernetes\.io\/metadata\.name: ingress-nginx/);
  assert.match(webPolicy, /kubernetes\.io\/metadata\.name: shared-auth/);
  assert.match(webPolicy, /port: 8120/);
});

test('API-only Gemini credentials remain isolated from browser and revoker pods', async () => {
  const apiSecret = await readRepoFile(`${overlay}/api.externalsecret.yaml`);
  const webSecret = await readRepoFile(`${overlay}/web.externalsecret.yaml`);
  const revokerSecret = await readRepoFile(`${overlay}/revoker.externalsecret.yaml`);
  const webDeployment = await readRepoFile(`${overlay}/web.deployment.yaml`);
  const revokerDeployment = await readRepoFile(`${overlay}/revoker.deployment.yaml`);

  assert.match(apiSecret, /key: dd\/remote-dev\/canonical-cloud-api/);
  for (const key of [
    'DATABASE_URL',
    'APP_SESSION_ENCRYPTION_KEY',
    'SUPABASE_URL',
    'SUPABASE_PUBLISHABLE_KEY',
    'GEMINI_API_KEY',
  ]) {
    assert.match(apiSecret, new RegExp(`secretKey: ${key}`));
  }
  assert.doesNotMatch(apiSecret, /SERVICE_ROLE|MIGRATION_DATABASE_URL|AUTH_SIGNING_KEY/i);
  assert.doesNotMatch(`${webSecret}\n${revokerSecret}\n${webDeployment}\n${revokerDeployment}`, /GEMINI_API_KEY|GEMINI_MODEL/);
});

test('digest promotion requires and atomically updates the API artifact', async () => {
  const script = await readRepoFile(`${overlay}/promote-release.mjs`);
  assert.match(script, /optionValue\(arguments_, '--api-digest'\)/);
  assert.match(script, /path: resolve\(base, 'api\.deployment\.yaml'\)/);
  assert.match(script, /repository: 'ghcr\.io\/canonical-cloud\/canonical-api-server'/);
  assert.match(script, /digest: apiDigest/);
  assert.match(script, /label: 'api'/);
});
