import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (
      existsSync(
        resolve(candidate, 'remote/deployments/browser-test-server/package.json'),
      )
    ) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('browser-test-server source wires playwright + puppeteer + selenium', async () => {
  const pkg = JSON.parse(
    await readRepoFile('remote/deployments/browser-test-server/package.json'),
  ) as {
    name: string;
    dependencies: Record<string, string>;
    devDependencies: Record<string, string>;
  };
  const source = await readRepoFile('remote/deployments/browser-test-server/src/server.ts');
  const readme = await readRepoFile('remote/deployments/browser-test-server/readme.md');
  const dockerfile = await readRepoFile('remote/deployments/browser-test-server/Dockerfile');

  assert.equal(pkg.name, 'dd-browser-test-server');
  assert.match(pkg.dependencies.playwright, /\d/);
  assert.match(pkg.dependencies.puppeteer, /\d/);
  assert.match(pkg.dependencies['selenium-webdriver'], /\d/);
  assert.match(pkg.dependencies.fastify, /\d/);
  assert.match(pkg.dependencies.zod, /\d/);

  // The same Chromium binary backs all three drivers; that fact is part of the
  // contract so the Dockerfile + runtime stay coherent.
  assert.match(source, /chromium as playwrightChromium/);
  assert.match(source, /import puppeteer/);
  assert.match(source, /from 'selenium-webdriver'/);
  assert.match(source, /from 'selenium-webdriver\/chrome\.js'/);
  assert.match(source, /playwrightChromium\.executablePath\(\)/);

  assert.match(source, /POST \/run/);
  assert.match(source, /\/healthz/);
  assert.match(source, /\/metrics/);
  assert.match(source, /\/status/);
  assert.match(source, /\/tools/);
  assert.match(source, /\/browser-test\/healthz/);
  assert.match(source, /\/browser-test\/metrics/);
  assert.match(source, /\/browser-test\/status/);
  assert.match(source, /\/browser-test\/tools/);

  // The scenario DSL and its hard limits are part of the security model -
  // arbitrary script execution must remain opt-in via env var.
  assert.match(source, /BROWSER_TEST_ALLOW_EVALUATE/);
  assert.match(source, /allowEvaluate: readBooleanEnv\('BROWSER_TEST_ALLOW_EVALUATE', false\)/);
  assert.match(source, /BROWSER_TEST_MAX_CONCURRENT/);
  assert.match(source, /BROWSER_TEST_MAX_STEPS/);
  assert.match(source, /BROWSER_TEST_MAX_TIMEOUT_MS/);
  assert.match(source, /timingSafeEqual/);
  assert.match(source, /SERVER_AUTH_SECRET/);
  assert.match(source, /x-server-auth/);

  // Scenario DSL — every action used in the readme must exist in the source.
  for (const action of [
    'goto',
    'click',
    'fill',
    'select',
    'press',
    'waitForSelector',
    'waitForUrl',
    'waitForTimeout',
    'extractText',
    'extractAttribute',
    'screenshot',
    'evaluate',
  ]) {
    assert.match(source, new RegExp(`action: z\\.literal\\('${action}'\\)`), `missing step: ${action}`);
  }

  assert.match(readme, /dd-browser-test-server/);
  assert.match(readme, /Playwright/);
  assert.match(readme, /Puppeteer/);
  assert.match(readme, /Selenium/);
  assert.match(readme, /POST \/run/);
  assert.match(readme, /BROWSER_TEST_ALLOW_EVALUATE/);
  assert.match(readme, /SERVER_AUTH_SECRET/);
  // Service is intentionally separate from dd-web-scraper but reuses its image.
  assert.match(readme, /dd-web-scraper/);

  assert.match(dockerfile, /mcr\.microsoft\.com\/playwright:v\$\{PLAYWRIGHT_VERSION\}-noble/);
  assert.match(dockerfile, /EXPOSE 8104/);
  assert.match(dockerfile, /PLAYWRIGHT_BROWSERS_PATH=\/ms-playwright/);
  assert.match(dockerfile, /PUPPETEER_SKIP_DOWNLOAD=true/);
});

test('browser-test-server is deployed through Argo runtime manifests and the gateway', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-browser-test-server.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-browser-test-server.service.yaml',
  );
  const kustomization = await readRepoFile(
    'remote/argocd/dd-next-runtime/kustomization.yaml',
  );
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );

  assert.match(deployment, /name:\s*dd-browser-test-server/);
  assert.match(deployment, /image:\s*mcr\.microsoft\.com\/playwright:v1\.56\.0-noble/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(
    deployment,
    /cd \/opt\/dd-next-1\/remote\/deployments\/browser-test-server/,
  );
  assert.match(deployment, /name:\s*PORT[\s\S]*value:\s*'8104'/);
  assert.match(deployment, /name:\s*BROWSER_TEST_HEADLESS[\s\S]*value:\s*'true'/);
  assert.match(deployment, /name:\s*BROWSER_TEST_DEFAULT_TOOL[\s\S]*value:\s*playwright/);
  assert.match(deployment, /name:\s*BROWSER_TEST_MAX_CONCURRENT[\s\S]*value:\s*'2'/);
  assert.match(deployment, /name:\s*BROWSER_TEST_MAX_STEPS[\s\S]*value:\s*'64'/);
  // Arbitrary script execution must default to off in production manifests.
  assert.match(deployment, /name:\s*BROWSER_TEST_ALLOW_EVALUATE[\s\S]*value:\s*'false'/);
  assert.match(
    deployment,
    /name:\s*SERVER_AUTH_SECRET[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-agent-secrets[\s\S]*key:\s*SERVER_AUTH_SECRET/,
  );

  assert.match(service, /name:\s*dd-browser-test-server/);
  assert.match(service, /port:\s*8104/);
  assert.match(service, /targetPort:\s*http/);

  assert.match(kustomization, /dd-browser-test-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-browser-test-server\.service\.yaml/);

  // Gateway routes share the auth shape used by /scrape.
  assert.match(
    gateway,
    /location = \/browser-test[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-browser-test-server\.default\.svc\.cluster\.local:8104/,
  );
  assert.match(
    gateway,
    /location \/browser-test\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-browser-test-server\.default\.svc\.cluster\.local:8104/,
  );
});
