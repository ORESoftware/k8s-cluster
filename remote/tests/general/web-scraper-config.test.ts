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

function regexEscape(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function assertDeploymentRow(source: string, deployment: string, service: string): void {
  assert.match(
    source,
    new RegExp(
      `DeploymentRow \\{ deployments: &\\[[^\\]]*"${regexEscape(
        deployment,
      )}"[^\\]]*\\], service: &\\[[^\\]]*"${regexEscape(service)}"[^\\]]*\\]`,
    ),
  );
}

test('web scraper service supports browser, DOM, fetch, and Browserless strategies', async () => {
  const packageJson = await readRepoFile('remote/deployments/web-scraper-service/package.json');
  const source = await readRepoFile('remote/deployments/web-scraper-service/src/server.ts');
  const extractionWorker = await readRepoFile(
    'remote/deployments/web-scraper-service/src/extraction-worker.ts',
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
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-web-scraper/);
  assert.match(deployment, /mcr\.microsoft\.com\/playwright:v1\.56\.0-noble/);
  assert.doesNotMatch(deployment, /corepack enable/);
  assert.match(deployment, /COREPACK_HOME[\s\S]*value:\s*\/tmp\/corepack/);
  assert.match(deployment, /corepack pnpm install --frozen-lockfile --ignore-workspace --prod=false/);
  assert.match(deployment, /corepack pnpm run build/);
  assert.match(deployment, /corepack pnpm run start/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
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
  assert.match(home, /dd-web-scraper Fastify deployment/);
  assertDeploymentRow(home, 'dd-web-scraper', 'dd-web-scraper:8097');
  assert.match(runtimeReadme, /`dd-web-scraper`/);
  assert.match(runtimeReadme, /worker_threads/);
  assert.match(runtimeReadme, /SCRAPER_PARSER_WORKERS=2/);
  assert.match(runtimeReadme, /SCRAPER_PARSER_WORKER_MEMORY_MB=128/);
  assert.match(runtimeReadme, /fails closed when `SERVER_AUTH_SECRET`/);
  assert.match(runtimeReadme, /revalidates redirect and browser subresource targets/);
  assert.match(runtimeReadme, /`linkedom`/);
});
