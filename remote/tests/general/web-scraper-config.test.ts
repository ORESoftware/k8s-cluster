import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/web-scraper-service/package.json'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('web scraper service supports browser, DOM, fetch, and Browserless strategies', async () => {
  const packageJson = await readRepoFile('remote/deployments/web-scraper-service/package.json');
  const source = await readRepoFile('remote/deployments/web-scraper-service/src/server.ts');
  const extractionWorker = await readRepoFile(
    'remote/deployments/web-scraper-service/src/extraction-worker.ts',
  );
  const browserAgent = await readRepoFile(
    'remote/deployments/web-scraper-service/src/browser-agent.ts',
  );
  const readme = await readRepoFile('remote/deployments/web-scraper-service/readme.md');

  assert.match(packageJson, /"fastify":/);
  assert.match(packageJson, /"cheerio":/);
  assert.match(packageJson, /"jsdom":/);
  assert.match(packageJson, /"linkedom":/);
  assert.match(packageJson, /"playwright":/);
  assert.match(packageJson, /"puppeteer":/);
  assert.match(source, /'native-fetch'/);
  assert.match(source, /'cheerio'/);
  assert.match(source, /'jsdom'/);
  assert.match(source, /'linkedom'/);
  assert.match(source, /'playwright'/);
  assert.match(source, /'puppeteer'/);
  assert.match(source, /'browserless'/);
  assert.match(source, /POST \/scrape/);
  assert.match(source, /SERVER_AUTH_SECRET/);
  assert.match(source, /SCRAPER_ALLOW_PRIVATE_NETWORKS/);
  assert.match(source, /SCRAPER_PARSER_WORKERS/);
  assert.match(source, /SCRAPER_PARSER_WORKER_MEMORY_MB/);
  assert.match(source, /SCRAPER_BROWSER_HEADLESS/);
  assert.match(source, /SCRAPER_CAPTURE_FAILURE_SCREENSHOTS/);
  assert.match(source, /failureScreenshot/);
  assert.match(source, /page\.screenshot/);
  assert.match(source, /new Worker\(workerEntry/);
  assert.match(source, /tsx\/esm\/api/);
  assert.match(source, /resourceLimits/);
  assert.match(source, /parserWorkerSemaphore/);
  assert.match(source, /redirect:\s*'manual'/);
  assert.match(source, /assertAllowedBrowserRequest/);
  assert.match(source, /ALWAYS_BLOCKED_OUTBOUND_HEADERS/);
  assert.match(source, /SENSITIVE_OUTBOUND_HEADERS/);
  assert.match(source, /timingSafeEqual/);
  assert.match(source, /SERVER_AUTH_SECRET is required unless SCRAPER_ALLOW_UNAUTHENTICATED=true/);
  assert.match(source, /target host .* blocked by scraper network policy/);
  assert.match(source, /BROWSERLESS_TOKEN/);
  assert.match(source, /type ServiceDescriptor = \{/);
  assert.match(source, /service: 'dd-web-scraper';/);
  assert.match(
    source,
    /endpoints: Record<'scrape' \| 'strategies' \| 'status' \| 'healthz' \| 'metrics', string>;/,
  );
  assert.match(source, /strategies: readonly StrategyName\[];/);
  assert.match(source, /defaultStrategy: StrategyInput;/);
  assert.match(source, /parserWorkerConcurrency: number;/);
  assert.match(source, /type StrategiesDescriptor = \{/);
  assert.match(
    source,
    /autoPolicy: Record<'javascript' \| 'selectors' \| 'fallback', StrategyName>;/,
  );
  assert.match(source, /supportsJavaScript: boolean;/);
  assert.match(source, /supportsSelectors: boolean;/);
  assert.match(source, /type StatusDescriptor = \{/);
  assert.match(source, /serverStartedAt: string;/);
  assert.match(source, /serverInstanceId: string;/);
  assert.match(source, /maxConcurrent: number;/);
  assert.match(source, /blockPrivateNetworks: boolean;/);
  assert.match(source, /browserlessConfigured: boolean;/);
  assert.match(source, /type HealthDescriptor = \{/);
  assert.match(source, /inFlight: number;/);
  assert.match(source, /fastify\.get\('\/', async \(\) => serviceDescriptor\(\)\);/);
  assert.match(source, /fastify\.get\('\/scrape', async \(\) => serviceDescriptor\(\)\);/);
  assert.match(source, /fastify\.get\('\/strategies', async \(\) => strategiesDescriptor\(\)\);/);
  assert.match(
    source,
    /fastify\.get\('\/scrape\/strategies', async \(\) => strategiesDescriptor\(\)\);/,
  );
  assert.match(source, /fastify\.get\('\/status', async \(\) => statusDescriptor\(\)\);/);
  assert.match(source, /fastify\.get\('\/scrape\/status', async \(\) => statusDescriptor\(\)\);/);
  assert.match(source, /fastify\.get\('\/healthz', async \(\) => healthDescriptor\(\)\);/);
  assert.match(source, /fastify\.get\('\/scrape\/healthz', async \(\) => healthDescriptor\(\)\);/);
  assert.match(source, /scrape: 'POST \/scrape'/);
  assert.match(source, /strategies: 'GET \/scrape\/strategies'/);
  assert.match(source, /status: 'GET \/scrape\/status'/);
  assert.match(source, /healthz: 'GET \/scrape\/healthz'/);
  assert.match(source, /metrics: 'GET \/scrape\/metrics'/);
  assert.match(source, /javascript:[\s\S]*\? 'browserless' : 'playwright'/);
  assert.match(source, /selectors: 'cheerio'/);
  assert.match(source, /fallback: 'native-fetch'/);
  assert.match(source, /available: strategy !== 'browserless' \|\| isBrowserlessConfigured\(\)/);
  assert.match(extractionWorker, /from 'node:worker_threads'/);
  assert.match(extractionWorker, /extractNative/);
  assert.match(extractionWorker, /extractWithJsdom/);
  assert.match(extractionWorker, /extractWithLinkedom/);
  assert.match(extractionWorker, /extractWithCheerio/);
  assert.match(browserAgent, /serviceWorkers:\s*'block'/);
  assert.match(browserAgent, /context\.route\('\*\*\/\*'/);
  assert.match(browserAgent, /context\.routeWebSocket\('\*\*'/);
  assert.match(browserAgent, /requestUrlAllowedByDomain/);
  assert.match(readme, /Fastify instead of Nest/);
  assert.match(readme, /worker_threads/);
  assert.match(readme, /SCRAPER_BROWSER_HEADLESS=true/);
  assert.match(readme, /failureScreenshot/);
  assert.match(readme, /`linkedom`/);
});

test('web scraper is deployed through Argo runtime manifests and gateway', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-web-scraper.service.yaml');
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-web-scraper/);
  assert.match(
    deployment,
    /image:\s*ghcr\.io\/oresoftware\/dd-web-scraper@sha256:[a-f0-9]{64}/,
    'The browser worker must run an immutable dedicated image, not a mutable branch tag.',
  );
  assert.doesNotMatch(deployment, /corepack enable/);
  assert.doesNotMatch(deployment, /pnpm install/);
  assert.doesNotMatch(deployment, /pnpm run build/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /automountServiceAccountToken:\s*false/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /HOME[\s\S]*value:\s*\/tmp/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8097'/);
  assert.match(deployment, /SCRAPER_PARSER_WORKERS[\s\S]*value:\s*'2'/);
  assert.match(deployment, /SCRAPER_PARSER_WORKER_MEMORY_MB[\s\S]*value:\s*'128'/);
  assert.match(deployment, /SCRAPER_BROWSER_HEADLESS[\s\S]*value:\s*'true'/);
  assert.match(deployment, /SCRAPER_CAPTURE_FAILURE_SCREENSHOTS[\s\S]*value:\s*'true'/);
  assert.match(deployment, /SCRAPER_FAILURE_SCREENSHOT_QUALITY[\s\S]*value:\s*'65'/);
  assert.match(deployment, /SCRAPER_FAILURE_SCREENSHOT_MAX_BYTES[\s\S]*value:\s*'512000'/);
  assert.match(deployment, /SCRAPER_MAX_REDIRECTS[\s\S]*value:\s*'5'/);
  assert.match(deployment, /SCRAPER_ALLOW_PRIVATE_NETWORKS[\s\S]*value:\s*'false'/);
  assert.match(deployment, /SCRAPER_ALLOW_SENSITIVE_HEADERS[\s\S]*value:\s*'false'/);
  assert.match(deployment, /SCRAPER_ALLOW_URL_CREDENTIALS[\s\S]*value:\s*'false'/);
  assert.match(
    deployment,
    /BROWSER_AGENT_ALLOWED_DOMAINS[\s\S]*value:\s*'[^']*talks\.devopsdays\.org[^']*'/,
  );
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /BROWSERLESS_TOKEN[\s\S]*optional:\s*true/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-web-scraper/);
  assert.match(service, /port:\s*8097/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(kustomization, /dd-web-scraper\.deployment\.yaml/);
  assert.match(kustomization, /dd-web-scraper\.service\.yaml/);
  assert.match(
    gateway,
    /location = \/scrape[\s\S]*dd-web-scraper\.default\.svc\.cluster\.local:8097/,
  );
  assert.match(
    gateway,
    /location \/scrape\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"/,
  );
  assert.match(
    prometheus,
    /job_name:\s*dd-web-scraper[\s\S]*dd-web-scraper\.default\.svc\.cluster\.local:8097/,
  );
  assert.match(
    otel,
    /job_name:\s*dd-web-scraper[\s\S]*dd-web-scraper\.default\.svc\.cluster\.local:8097/,
  );
  assert.match(runtimeReadme, /`dd-web-scraper`/);
  assert.match(runtimeReadme, /worker_threads/);
  assert.match(runtimeReadme, /SCRAPER_PARSER_WORKERS=2/);
  assert.match(runtimeReadme, /SCRAPER_PARSER_WORKER_MEMORY_MB=128/);
  assert.match(runtimeReadme, /fails closed when `SERVER_AUTH_SECRET`/);
  assert.match(runtimeReadme, /revalidates redirect and browser subresource targets/);
  assert.match(runtimeReadme, /`linkedom`/);
});

test('scraper CLI contract, secrets, supervisor limits, and pod hardening stay aligned', async () => {
  const flags = await readRepoFile('remote/deployments/web-scraper-service/.cli-flags.toml');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml',
  );

  function flagBlock(name: string): string {
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const match = flags.match(new RegExp(`\\[flags\\.${escaped}\\]([\\s\\S]*?)(?=\\n\\[|$)`));
    assert.ok(match, `missing scraper flag contract block: ${name}`);
    return match[1];
  }

  for (const [name, env, type, defaultValue] of [
    ['core-port', 'SCRAPER_CORE_PORT', 'integer', '18097'],
    ['core-startup-timeout-ms', 'SCRAPER_CORE_STARTUP_TIMEOUT_MS', 'integer', '30000'],
    ['supervisor-max-body-bytes', 'SCRAPER_SUPERVISOR_MAX_BODY_BYTES', 'integer', '1048576'],
    ['supervisor-max-core-response-bytes', 'SCRAPER_SUPERVISOR_MAX_CORE_RESPONSE_BYTES', 'integer', '4194304'],
    ['default-strategy', 'SCRAPER_DEFAULT_STRATEGY', 'string', 'auto'],
    ['max-concurrent', 'SCRAPER_MAX_CONCURRENT', 'integer', '4'],
    ['max-timeout-ms', 'SCRAPER_MAX_TIMEOUT_MS', 'integer', '60000'],
    ['dns-timeout-ms', 'SCRAPER_DNS_TIMEOUT_MS', 'integer', '5000'],
    ['max-redirects', 'SCRAPER_MAX_REDIRECTS', 'integer', '5'],
    ['respect-robots', 'SCRAPER_RESPECT_ROBOTS', 'bool', 'true'],
    ['min-origin-delay-ms', 'SCRAPER_MIN_ORIGIN_DELAY_MS', 'integer', '1000'],
    ['allow-private-networks', 'SCRAPER_ALLOW_PRIVATE_NETWORKS', 'bool', 'false'],
    ['allow-sensitive-headers', 'SCRAPER_ALLOW_SENSITIVE_HEADERS', 'bool', 'false'],
    ['allow-url-credentials', 'SCRAPER_ALLOW_URL_CREDENTIALS', 'bool', 'false'],
    ['apify-timeout-ms', 'APIFY_FALLBACK_TIMEOUT_MS', 'integer', '90000'],
    ['apify-max-request-retries', 'APIFY_MAX_REQUEST_RETRIES', 'integer', '2'],
    ['apify-max-concurrent', 'APIFY_FALLBACK_MAX_CONCURRENT', 'integer', '1'],
    ['apify-min-delay-ms', 'APIFY_FALLBACK_MIN_DELAY_MS', 'integer', '1000'],
    ['apify-failure-cooldown-ms', 'APIFY_FALLBACK_FAILURE_COOLDOWN_MS', 'integer', '60000'],
    ['apify-max-total-charge-usd', 'APIFY_MAX_TOTAL_CHARGE_USD', 'double', '0.25'],
    ['apify-max-response-bytes', 'APIFY_MAX_RESPONSE_BYTES', 'integer', '2000000'],
  ] as const) {
    const block = flagBlock(name);
    assert.match(block, new RegExp(`env\\s*=\\s*"${env}"`), `${name} env mapping drifted`);
    assert.match(block, new RegExp(`type\\s*=\\s*"${type}"`), `${name} type drifted`);
    assert.match(
      block,
      new RegExp(`default\\s*=\\s*(?:"${defaultValue.replace('.', '\\.')}"|${defaultValue.replace('.', '\\.')})`),
      `${name} default drifted`,
    );
    assert.match(
      deployment,
      new RegExp(`name:\\s*${env}[\\s\\S]*?value:\\s*['"]?${defaultValue.replace('.', '\\.')}['"]?`),
      `${env} deployment value drifted from the CLI contract`,
    );
  }

  const fallbackBlock = flagBlock('apify-fallback');
  assert.match(fallbackBlock, /env\s*=\s*"APIFY_FALLBACK_ENABLED"/);
  assert.match(fallbackBlock, /default\s*=\s*"false"/);
  assert.match(
    deployment,
    /name:\s*APIFY_FALLBACK_ENABLED[\s\S]*?value:\s*'true'/,
    'production may opt into fallback, but the reusable CLI contract must remain disabled by default',
  );
  assert.match(deployment, /name:\s*APIFY_FALLBACK_FOR_EXPLICIT_STRATEGY[\s\S]*?value:\s*'false'/);
  assert.match(
    deployment,
    /name:\s*APIFY_TOKEN[\s\S]*?secretKeyRef:[\s\S]*?key:\s*APIFY_TOKEN[\s\S]*?optional:\s*true/,
  );

  for (const secretFlag of [
    'apify-token',
    'server-auth-secret',
    'browserless-token',
    'browserless-content-url',
    'captcha-api-key',
    'proxies',
    'browser-agent-secrets-file',
  ]) {
    assert.doesNotMatch(
      flags,
      new RegExp(`\\[flags\\.${secretFlag.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\]`),
      `${secretFlag} must remain secret-env-only rather than argv-exposed`,
    );
  }

  for (const dangerousFlag of [
    'allow-robots-override',
    'allow-private-networks',
    'allow-sensitive-headers',
    'allow-url-credentials',
    'allow-unauthenticated',
    'captcha-autosolve',
    'allow-captcha-solving',
    'apify-explicit-strategies',
  ]) {
    const block = flagBlock(dangerousFlag);
    assert.match(block, /default\s*=\s*"false"/, `${dangerousFlag} must default fail-closed`);
    assert.match(block, /requires_tty\s*=\s*true/, `${dangerousFlag} must retain its operator TTY gate`);
  }

  assert.match(deployment, /runAsUser:\s*1000/);
  assert.match(deployment, /runAsGroup:\s*1000/);
  assert.match(deployment, /fsGroup:\s*1000/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /privileged:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*?drop:[\s\S]*?- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*?type:\s*RuntimeDefault/);
  assert.match(deployment, /volumeMounts:[\s\S]*?- name:\s*tmp[\s\S]*?mountPath:\s*\/tmp/);
  assert.match(deployment, /volumes:[\s\S]*?- name:\s*tmp[\s\S]*?emptyDir:[\s\S]*?sizeLimit:\s*2Gi/);
  assert.match(deployment, /PLAYWRIGHT_BROWSERS_PATH[\s\S]*?value:\s*\/ms-playwright/);
  assert.match(deployment, /PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD[\s\S]*?value:\s*'1'/);
  assert.match(deployment, /PUPPETEER_SKIP_DOWNLOAD[\s\S]*?value:\s*'true'/);
});
