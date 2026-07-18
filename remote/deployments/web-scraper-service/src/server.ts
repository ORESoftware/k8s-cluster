import Fastify from 'fastify';
import { initTelemetry, instrumentFastify, loggerMixin } from '@dd/telemetry';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import { lookup } from 'node:dns/promises';
import { lookup as lookupCallback } from 'node:dns';
import { access, readFile, readdir } from 'node:fs/promises';
import { availableParallelism } from 'node:os';
import { join } from 'node:path';
import { isIP, type LookupFunction } from 'node:net';
import { Worker } from 'node:worker_threads';
import { z } from 'zod';

import { Agent, ProxyAgent, type Dispatcher } from 'undici';
import robotsParser from 'robots-parser';

import type { Browser as PlaywrightBrowser, Page as PlaywrightPage } from 'playwright';
import type { Browser as PuppeteerBrowser, Page as PuppeteerPage } from 'puppeteer';

import {
  ProxyPool,
  parseProxyEntry,
  parseProxyList,
  PROXY_ROTATIONS,
  type ProxyEntry,
  type ProxyRotation,
} from './proxy-pool.js';
import {
  CaptchaSolveError,
  buildInjectionScript,
  detectCaptcha,
  solveCaptcha,
  SOLVABLE_CAPTCHA_TYPES,
  type CaptchaDetection,
  type CaptchaType,
} from './captcha.js';
import { captchaAutoSolveAllowed } from './scrape-policy.js';

const STRATEGIES = [
  'native-fetch',
  'cheerio',
  'jsdom',
  'linkedom',
  'playwright',
  'puppeteer',
  'browserless',
] as const;

type StrategyName = (typeof STRATEGIES)[number];
type StrategyInput = StrategyName | 'auto';
type ScrapeResultStatus = 'ok' | 'error';

const strategyAliases: Record<string, StrategyInput> = {
  auto: 'auto',
  fetch: 'native-fetch',
  native: 'native-fetch',
  'native-fetch': 'native-fetch',
  'plain-fetch': 'native-fetch',
  'plain/native-fetch': 'native-fetch',
  cheerio: 'cheerio',
  jsdom: 'jsdom',
  linkedom: 'linkedom',
  playwright: 'playwright',
  puppeteer: 'puppeteer',
  browserless: 'browserless',
  'browserless.io': 'browserless',
};

const serverStartedAt = new Date().toISOString();
const serverInstanceId = randomUUID();

const ALWAYS_BLOCKED_OUTBOUND_HEADERS = new Set([
  'connection',
  'content-length',
  'expect',
  'host',
  'keep-alive',
  'proxy-authenticate',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
]);
const SENSITIVE_OUTBOUND_HEADERS = new Set(['authorization', 'cookie', 'proxy-authorization']);

const config = {
  host: process.env.HOST ?? '0.0.0.0',
  port: readNumberEnv('PORT', 8097),
  serverAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
  allowUnauthenticated: process.env.SCRAPER_ALLOW_UNAUTHENTICATED === 'true',
  defaultStrategy: normalizeStrategyInput(process.env.SCRAPER_DEFAULT_STRATEGY ?? 'auto'),
  maxConcurrent: readNumberEnv('SCRAPER_MAX_CONCURRENT', 4),
  parserWorkerConcurrency: readNumberEnv(
    'SCRAPER_PARSER_WORKERS',
    Math.max(1, Math.min(4, availableParallelism() - 1)),
  ),
  parserWorkerMemoryMb: readNumberEnv('SCRAPER_PARSER_WORKER_MEMORY_MB', 128),
  maxTimeoutMs: readNumberEnv('SCRAPER_MAX_TIMEOUT_MS', 60_000),
  defaultTimeoutMs: readNumberEnv('SCRAPER_DEFAULT_TIMEOUT_MS', 30_000),
  dnsTimeoutMs: readNumberEnv('SCRAPER_DNS_TIMEOUT_MS', 5_000),
  maxRedirects: readNumberEnv('SCRAPER_MAX_REDIRECTS', 5),
  maxHtmlChars: readNumberEnv('SCRAPER_MAX_HTML_CHARS', 1_000_000),
  maxTextChars: readNumberEnv('SCRAPER_MAX_TEXT_CHARS', 40_000),
  maxLinks: readNumberEnv('SCRAPER_MAX_LINKS', 250),
  userAgent:
    process.env.SCRAPER_USER_AGENT ??
    'dd-web-scraper/0.1 (+https://github.com/ORESoftware/k8s-cluster)',
  respectRobots: readBooleanEnv('SCRAPER_RESPECT_ROBOTS', true),
  allowRobotsOverride: readBooleanEnv('SCRAPER_ALLOW_ROBOTS_OVERRIDE', false),
  robotsCacheTtlMs: readNumberEnv('SCRAPER_ROBOTS_CACHE_TTL_MS', 3_600_000),
  minOriginDelayMs: readNumberEnv('SCRAPER_MIN_ORIGIN_DELAY_MS', 1_000),
  browserHeadless: readBooleanEnv('SCRAPER_BROWSER_HEADLESS', true),
  captureFailureScreenshots: readBooleanEnv('SCRAPER_CAPTURE_FAILURE_SCREENSHOTS', true),
  failureScreenshotQuality: clampNumber(
    readNumberEnv('SCRAPER_FAILURE_SCREENSHOT_QUALITY', 65),
    1,
    100,
  ),
  failureScreenshotMaxBytes: readNumberEnv('SCRAPER_FAILURE_SCREENSHOT_MAX_BYTES', 512_000),
  autoUseBrowserless: process.env.SCRAPER_AUTO_BROWSERLESS === 'true',
  allowPrivateNetworks: process.env.SCRAPER_ALLOW_PRIVATE_NETWORKS === 'true',
  allowSensitiveHeaders: process.env.SCRAPER_ALLOW_SENSITIVE_HEADERS === 'true',
  allowUrlCredentials: process.env.SCRAPER_ALLOW_URL_CREDENTIALS === 'true',
  browserlessEndpoint: process.env.BROWSERLESS_ENDPOINT ?? 'https://production-sfo.browserless.io',
  browserlessContentUrl: process.env.BROWSERLESS_CONTENT_URL ?? null,
  browserlessToken: process.env.BROWSERLESS_TOKEN ?? null,
  proxies: parseProxyList(process.env.SCRAPER_PROXIES),
  proxyRotation: normalizeProxyRotation(process.env.SCRAPER_PROXY_ROTATION ?? 'sticky'),
  proxyCooldownMs: readNumberEnv('SCRAPER_PROXY_COOLDOWN_MS', 60_000),
  allowRequestProxy: readBooleanEnv('SCRAPER_ALLOW_REQUEST_PROXY', true),
  detectCaptchas: readBooleanEnv('SCRAPER_DETECT_CAPTCHAS', true),
  captchaAutoSolve: readBooleanEnv('SCRAPER_CAPTCHA_AUTOSOLVE', false),
  allowCaptchaSolving: readBooleanEnv('SCRAPER_ALLOW_CAPTCHA_SOLVING', false),
  captchaProviderUrl: process.env.SCRAPER_CAPTCHA_PROVIDER_URL ?? 'https://2captcha.com',
  captchaApiKey: process.env.SCRAPER_CAPTCHA_API_KEY ?? null,
  captchaPollIntervalMs: readNumberEnv('SCRAPER_CAPTCHA_POLL_INTERVAL_MS', 5_000),
  captchaTimeoutMs: readNumberEnv('SCRAPER_CAPTCHA_TIMEOUT_MS', 120_000),
  captchaMaxAttempts: readNumberEnv('SCRAPER_CAPTCHA_MAX_ATTEMPTS', 2),
  captchaMaxConcurrent: readNumberEnv('SCRAPER_CAPTCHA_MAX_CONCURRENT', 2),
};

const proxyPool = new ProxyPool(config.proxies, config.proxyRotation, config.proxyCooldownMs);

const ScrapeRequestSchema = z.object({
  requestId: z.string().min(1).max(120).optional(),
  url: z.string().url(),
  strategy: z.string().min(1).max(80).optional(),
  renderJavaScript: z.boolean().optional(),
  selector: z.string().min(1).max(800).optional(),
  selectors: z
    .record(z.string().min(1).max(120), z.string().min(1).max(800))
    .refine((value) => Object.keys(value).length <= 50, {
      message: 'at most 50 selectors are allowed',
    })
    .optional(),
  includeHtml: z.boolean().optional(),
  includeText: z.boolean().optional(),
  includeLinks: z.boolean().optional(),
  captureFailureScreenshot: z.boolean().optional(),
  timeoutMs: z.number().int().min(500).optional(),
  maxHtmlChars: z.number().int().min(1_000).optional(),
  maxTextChars: z.number().int().min(500).optional(),
  waitUntil: z.enum(['load', 'domcontentloaded', 'networkidle']).optional(),
  headers: z.record(z.string().min(1).max(120), z.string().max(2_000)).optional(),
  userAgent: z
    .string()
    .min(1)
    .max(500)
    // Block control chars so a UA can't smuggle a header break into the browser
    // strategies (setUserAgent / extraHTTPHeaders bypass the fetch header guard).
    .refine((value) => !/[\u0000-\u001f\u007f]/.test(value), {
      message: 'user agent contains control characters',
    })
    .optional(),
  proxy: z.string().min(3).max(500).optional(),
  useProxy: z.boolean().optional(),
  detectCaptcha: z.boolean().optional(),
  solveCaptcha: z.boolean().optional(),
  respectRobots: z.boolean().optional(),
});

type ScrapeRequest = z.infer<typeof ScrapeRequestSchema>;

type FetchedDocument = {
  html: string;
  finalUrl: string;
  status?: number;
  contentType?: string;
  truncated: boolean;
  failureScreenshot?: FailureScreenshot;
};

type ProxyInfo = {
  label: string;
  protocol: string;
  rotation: ProxyRotation;
  fromPool: boolean;
};

type CaptchaOutcome = {
  detected: boolean;
  type: CaptchaType | null;
  sitekey: string | null;
  solved: boolean;
  attempts: number;
  provider?: string;
  solveMs?: number;
  signals?: string[];
  error?: string;
};

/** Per-request mutable state for proxy selection and captcha orchestration. */
type ScrapeContext = {
  proxy: ProxyEntry | null;
  proxyFromPool: boolean;
  captcha?: CaptchaOutcome;
};

type ParserName = 'native-fetch' | 'cheerio' | 'jsdom' | 'linkedom';
type BrowserStrategyName = 'playwright' | 'puppeteer';

type FailureScreenshot = {
  strategy: BrowserStrategyName;
  mimeType: 'image/jpeg';
  encoding: 'base64';
  byteLength: number;
  capturedAt: string;
  finalUrl?: string;
  data?: string;
  omitted?: boolean;
  omitReason?: string;
};

type ExtractionResult = {
  parser: ParserName;
  title?: string;
  text?: string;
  html?: string;
  selection?: {
    selector: string;
    count: number;
    text?: string;
    html?: string;
  };
  fields?: Record<string, string>;
  links?: string[];
};

type ExtractionWorkerResponse =
  | { ok: true; extraction: ExtractionResult }
  | { ok: false; error: string };

type ScrapeResponse = {
  ok: true;
  requestId: string;
  strategy: StrategyName;
  requestedStrategy: StrategyInput;
  url: string;
  finalUrl: string;
  status?: number;
  contentType?: string;
  durationMs: number;
  truncated: boolean;
  extraction: ExtractionResult;
  proxy?: ProxyInfo;
  captcha?: CaptchaOutcome;
};

type ServiceDescriptor = {
  service: 'dd-web-scraper';
  ok: true;
  endpoints: Record<'scrape' | 'strategies' | 'status' | 'healthz' | 'metrics', string>;
  strategies: readonly StrategyName[];
  defaultStrategy: StrategyInput;
  parserWorkerConcurrency: number;
  parserWorkerMemoryMb: number;
  browserHeadless: boolean;
  captureFailureScreenshots: boolean;
};

type StrategyDescriptor = {
  name: StrategyName;
  available: boolean;
  supportsJavaScript: boolean;
  supportsSelectors: boolean;
};

type StrategiesDescriptor = {
  default: StrategyInput;
  autoPolicy: Record<'javascript' | 'selectors' | 'fallback', StrategyName>;
  strategies: StrategyDescriptor[];
};

type StatusDescriptor = {
  ok: true;
  service: 'dd-web-scraper';
  serverStartedAt: string;
  serverInstanceId: string;
  inFlight: number;
  maxConcurrent: number;
  parserWorkerConcurrency: number;
  parserWorkerMemoryMb: number;
  blockPrivateNetworks: boolean;
  maxRedirects: number;
  allowSensitiveHeaders: boolean;
  browserlessConfigured: boolean;
  browserHeadless: boolean;
  captureFailureScreenshots: boolean;
  failureScreenshotQuality: number;
  failureScreenshotMaxBytes: number;
  proxyPoolSize: number;
  proxyRotation: ProxyRotation;
  allowRequestProxy: boolean;
  captchaDetection: boolean;
  captchaAutoSolve: boolean;
  captchaSolverConfigured: boolean;
  captchaMaxConcurrent: number;
  respectRobots: boolean;
  allowRobotsOverride: boolean;
  minOriginDelayMs: number;
  allowCaptchaSolving: boolean;
};

type HealthDescriptor = {
  ok: true;
  service: 'dd-web-scraper';
  serverStartedAt: string;
  serverInstanceId: string;
  inFlight: number;
};

class Semaphore {
  private active = 0;
  private readonly queue: Array<() => void> = [];

  constructor(private readonly limit: number) {}

  get activeCount(): number {
    return this.active;
  }

  get queuedCount(): number {
    return this.queue.length;
  }

  async run<T>(work: () => Promise<T>): Promise<T> {
    await this.acquire();
    try {
      return await work();
    } finally {
      this.release();
    }
  }

  private acquire(): Promise<void> {
    if (this.active < this.limit) {
      this.active += 1;
      return Promise.resolve();
    }

    return new Promise((resolve) => {
      this.queue.push(() => {
        this.active += 1;
        resolve();
      });
    });
  }

  private release(): void {
    this.active -= 1;
    const next = this.queue.shift();
    if (next) {
      next();
    }
  }
}

const metrics = {
  inFlight: 0,
  total: new Map<string, number>(),
  durationSumMs: new Map<StrategyName, number>(),
  durationCount: new Map<StrategyName, number>(),
  captcha: new Map<string, number>(),
  robotsChecks: 0,
  robotsDenials: 0,
  robotsOverrides: 0,
};

type CaptchaMetricEvent = 'detected' | 'solved' | 'failed';

const CAPTCHA_METRIC_TYPES = [
  'recaptcha-v2',
  'recaptcha-v3',
  'hcaptcha',
  'turnstile',
  'cloudflare-challenge',
  'unknown',
] as const;

let playwrightBrowser: PlaywrightBrowser | null = null;
let playwrightBrowserPromise: Promise<PlaywrightBrowser> | null = null;
let puppeteerBrowser: PuppeteerBrowser | null = null;
let puppeteerBrowserPromise: Promise<PuppeteerBrowser> | null = null;
const parserWorkerSemaphore = new Semaphore(config.parserWorkerConcurrency);
let activeCaptchaSolves = 0;
const robotsCache = new Map<string, { body: string; expiresAt: number }>();
const originNextRequestAt = new Map<string, number>();

/**
 * DNS lookup used at connection time so the address we connect to is the same
 * one we vet — closing the resolve-then-connect (DNS-rebinding) window that a
 * pre-flight `validateTargetUrl` check alone leaves open. A host that resolves
 * to a private/link-local/cloud-metadata address (e.g. 169.254.169.254) is
 * rejected here, at connect, unless private networks are explicitly allowed.
 */
const guardedLookup: LookupFunction = (hostname, options, callback): void => {
  const wantsAll = typeof options === 'object' && options?.all === true;
  lookupCallback(
    hostname,
    { ...(typeof options === 'object' ? options : {}), all: true, verbatim: true },
    (err, addresses) => {
      const done = callback as (
        err: NodeJS.ErrnoException | null,
        address?: unknown,
        family?: number,
      ) => void;
      if (err) {
        done(err);
        return;
      }
      const list = addresses as Array<{ address: string; family: number }>;
      if (list.length === 0) {
        done(new Error(`no addresses resolved for ${hostname}`));
        return;
      }
      if (!config.allowPrivateNetworks) {
        const blocked = list.find((entry) => isPrivateIp(entry.address));
        if (blocked) {
          done(new Error(`address ${blocked.address} is blocked by scraper network policy`));
          return;
        }
      }
      if (wantsAll) {
        done(null, list);
        return;
      }
      const first = list[0]!;
      done(null, first.address, first.family);
    },
  );
};

// Long-lived dispatcher for direct (un-proxied) target fetches; pins egress to
// connect-time-validated addresses.
const guardedAgent = new Agent({ connect: { lookup: guardedLookup } });

const telemetry = initTelemetry('dd-web-scraper');

const fastify = Fastify({
  logger: { mixin: loggerMixin },
  bodyLimit: 1_048_576,
});

instrumentFastify(fastify, { service: 'dd-web-scraper' });

fastify.addHook('onRequest', async (request, reply) => {
  const path = request.url.split('?')[0] ?? request.url;
  if (request.method !== 'POST' || path !== '/scrape') {
    return;
  }
  if (isAuthorized(request.headers)) {
    return;
  }
  return reply.code(401).send({ ok: false, error: 'unauthorized' });
});

fastify.get('/', async () => serviceDescriptor());
fastify.get('/scrape', async () => serviceDescriptor());
fastify.get('/strategies', async () => strategiesDescriptor());
fastify.get('/scrape/strategies', async () => strategiesDescriptor());
fastify.get('/status', async () => statusDescriptor());
fastify.get('/scrape/status', async () => statusDescriptor());
fastify.get('/healthz', async () => healthDescriptor());
fastify.get('/scrape/healthz', async () => healthDescriptor());
fastify.get('/docs/api', async (_request, reply) => {
  reply.header('content-type', 'text/html; charset=utf-8');
  return readFile(new URL('../generated/api-docs.html', import.meta.url), 'utf8');
});
fastify.get('/api/docs', async (_request, reply) => {
  reply.header('content-type', 'text/html; charset=utf-8');
  return readFile(new URL('../generated/api-docs.html', import.meta.url), 'utf8');
});
fastify.get('/api/docs.json', async (_request, reply) => {
  reply.header('content-type', 'application/json; charset=utf-8');
  return readFile(new URL('../generated/api-docs.json', import.meta.url), 'utf8');
});
fastify.get('/metrics', async (_request, reply) => {
  reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  return renderMetrics();
});
fastify.get('/scrape/metrics', async (_request, reply) => {
  reply.header('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  return renderMetrics();
});

fastify.post('/scrape', async (request, reply) => {
  const parsed = ScrapeRequestSchema.safeParse(request.body);
  if (!parsed.success) {
    return reply.code(400).send({ ok: false, error: parsed.error.format() });
  }

  if (metrics.inFlight >= config.maxConcurrent) {
    return reply.code(429).send({
      ok: false,
      error: 'scraper concurrency limit reached',
      maxConcurrent: config.maxConcurrent,
    });
  }

  const requestId = parsed.data.requestId ?? randomUUID();
  let requestedStrategy: StrategyInput;
  let strategy: StrategyName;
  try {
    requestedStrategy = normalizeStrategyInput(parsed.data.strategy ?? config.defaultStrategy);
    strategy = chooseStrategy(parsed.data, requestedStrategy);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return reply.code(400).send({ ok: false, requestId, error: message });
  }
  const startedAt = Date.now();
  metrics.inFlight += 1;

  try {
    const result = await runScrape(parsed.data, requestId, requestedStrategy, strategy);
    const durationMs = Date.now() - startedAt;
    recordMetric(strategy, 'ok', durationMs);
    return { ...result, durationMs };
  } catch (error) {
    const durationMs = Date.now() - startedAt;
    recordMetric(strategy, 'error', durationMs);
    const message = error instanceof Error ? error.message : String(error);
    const failureScreenshot = getFailureScreenshot(error);
    const statusCode = isClientPolicyError(message) ? 400 : 500;
    return reply.code(statusCode).send({
      ok: false,
      requestId,
      strategy,
      requestedStrategy,
      durationMs,
      error: message,
      ...(failureScreenshot ? { failureScreenshot } : {}),
    });
  } finally {
    metrics.inFlight -= 1;
  }
});

async function runScrape(
  input: ScrapeRequest,
  requestId: string,
  requestedStrategy: StrategyInput,
  strategy: StrategyName,
): Promise<Omit<ScrapeResponse, 'durationMs'>> {
  const targetUrl = await validateTargetUrl(input.url);
  const ctx = await createScrapeContext(input, targetUrl, strategy);
  await enforceResponsibleScrapingPolicy(input, targetUrl, ctx);
  let fetched: FetchedDocument;
  try {
    fetched = await fetchByStrategy(input, targetUrl, strategy, ctx);
  } catch (error) {
    reportProxyOutcome(ctx, false);
    throw error;
  }

  // Non-browser strategies can detect a challenge but cannot solve it (no page
  // to inject into); surface the detection so callers can retry via a browser.
  if (!ctx.captcha && shouldDetectCaptcha(input)) {
    ctx.captcha = detectionToOutcome(detectCaptcha(fetched.html));
    if (ctx.captcha.detected) {
      recordCaptchaMetric('detected', ctx.captcha.type);
    }
  }

  // Score proxy health off the response (block/challenge pages still "fetch ok").
  reportProxyOutcome(ctx, isHealthyProxyResponse(fetched, ctx));

  let extraction: ExtractionResult;
  try {
    extraction = await extractDocument(fetched.html, fetched.finalUrl, input, strategy);
  } catch (error) {
    throw attachFailureScreenshot(error, fetched.failureScreenshot);
  }

  return {
    ok: true,
    requestId,
    strategy,
    requestedStrategy,
    url: targetUrl.toString(),
    finalUrl: fetched.finalUrl,
    status: fetched.status,
    contentType: fetched.contentType,
    truncated: fetched.truncated,
    extraction,
    ...(ctx.proxy ? { proxy: proxyInfo(ctx) } : {}),
    ...(ctx.captcha?.detected ? { captcha: ctx.captcha } : {}),
  };
}

async function fetchByStrategy(
  input: ScrapeRequest,
  targetUrl: URL,
  strategy: StrategyName,
  ctx: ScrapeContext,
): Promise<FetchedDocument> {
  switch (strategy) {
    case 'native-fetch':
    case 'cheerio':
    case 'jsdom':
    case 'linkedom':
      return fetchStaticDocument(input, targetUrl, ctx);
    case 'playwright':
      return fetchWithPlaywright(input, targetUrl, ctx);
    case 'puppeteer':
      return fetchWithPuppeteer(input, targetUrl, ctx);
    case 'browserless':
      return fetchWithBrowserless(input, targetUrl);
  }
}

async function fetchStaticDocument(
  input: ScrapeRequest,
  targetUrl: URL,
  ctx: ScrapeContext,
): Promise<FetchedDocument> {
  const timeoutMs = getTimeoutMs(input);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  const proxyDispatcher = buildFetchDispatcher(ctx.proxy);
  // Un-proxied fetches go through the guarded agent (connect-time IP vetting);
  // proxied fetches connect to the validated proxy, which resolves the target.
  const dispatcher = proxyDispatcher ?? guardedAgent;
  try {
    let currentUrl = targetUrl;
    for (let redirectCount = 0; redirectCount <= config.maxRedirects; redirectCount += 1) {
      currentUrl = await validateTargetUrl(currentUrl.toString());
      const init: RequestInit & { dispatcher?: Dispatcher } = {
        method: 'GET',
        redirect: 'manual',
        headers: buildHeaders(input, currentUrl, targetUrl),
        signal: controller.signal,
        dispatcher,
      };
      const response = await fetch(currentUrl, init);

      const location = response.headers.get('location');
      if (isRedirectStatus(response.status) && location) {
        if (redirectCount === config.maxRedirects) {
          throw new Error(`maximum redirect count exceeded (${config.maxRedirects})`);
        }
        currentUrl = new URL(location, currentUrl);
        continue;
      }

      const read = await readResponseText(response, getMaxHtmlChars(input));
      return {
        html: read.text,
        finalUrl: response.url || currentUrl.toString(),
        status: response.status,
        contentType: response.headers.get('content-type') ?? undefined,
        truncated: read.truncated,
      };
    }

    throw new Error(`maximum redirect count exceeded (${config.maxRedirects})`);
  } finally {
    clearTimeout(timeout);
    // Only the per-request proxy agent is disposable; guardedAgent is shared.
    if (proxyDispatcher) {
      await proxyDispatcher.close().catch(() => undefined);
    }
  }
}

async function fetchWithPlaywright(
  input: ScrapeRequest,
  targetUrl: URL,
  ctx: ScrapeContext,
): Promise<FetchedDocument> {
  const browser = await getPlaywrightBrowser();
  const context = await browser.newContext({
    userAgent: effectiveUserAgent(input),
    extraHTTPHeaders: buildHeaders(input, targetUrl, targetUrl),
    ...(ctx.proxy ? { proxy: playwrightProxy(ctx.proxy) } : {}),
  });
  const page = await context.newPage();
  let blockedRequestError: Error | null = null;
  let failureScreenshot: FailureScreenshot | undefined;
  try {
    await page.route('**/*', async (route) => {
      try {
        await assertAllowedBrowserRequest(route.request().url());
        await route.continue();
      } catch (error) {
        blockedRequestError ??= error instanceof Error ? error : new Error(String(error));
        await route.abort('blockedbyclient').catch(() => undefined);
      }
    });
    const response = await page
      .goto(targetUrl.toString(), {
        waitUntil: input.waitUntil ?? 'domcontentloaded',
        timeout: getTimeoutMs(input),
      })
      .catch((error: unknown) => {
        if (blockedRequestError) {
          throw blockedRequestError;
        }
        throw error;
      });
    if (input.selector) {
      await page
        .waitForSelector(input.selector, { timeout: Math.min(getTimeoutMs(input), 5_000) })
        .catch(() => undefined);
    }
    await orchestrateCaptcha(
      {
        content: () => page.content(),
        url: () => page.url(),
        evaluate: (script) => page.evaluate(script),
      },
      input,
      ctx,
    );
    if (shouldCaptureFailureScreenshot(input, 'playwright')) {
      failureScreenshot = await capturePlaywrightFailureScreenshot(page, 'playwright');
    }
    const html = trimToMax(await page.content(), getMaxHtmlChars(input));
    return {
      html,
      finalUrl: page.url(),
      status: response?.status(),
      contentType: response?.headers()['content-type'],
      truncated: html.length >= getMaxHtmlChars(input),
      failureScreenshot,
    };
  } catch (error) {
    if (shouldCaptureFailureScreenshot(input, 'playwright')) {
      failureScreenshot ??= await capturePlaywrightFailureScreenshot(page, 'playwright');
    }
    throw attachFailureScreenshot(error, failureScreenshot);
  } finally {
    await context.close();
  }
}

async function fetchWithPuppeteer(
  input: ScrapeRequest,
  targetUrl: URL,
  ctx: ScrapeContext,
): Promise<FetchedDocument> {
  const browser = await getPuppeteerBrowser();
  const context = await browser.createBrowserContext(
    ctx.proxy ? { proxyServer: ctx.proxy.label } : {},
  );
  const page = await context.newPage();
  let blockedRequestError: Error | null = null;
  let failureScreenshot: FailureScreenshot | undefined;
  try {
    if (ctx.proxy && (ctx.proxy.username || ctx.proxy.password)) {
      await page.authenticate({ username: ctx.proxy.username, password: ctx.proxy.password });
    }
    await page.setUserAgent(effectiveUserAgent(input));
    if (input.headers) {
      await page.setExtraHTTPHeaders(buildHeaders(input, targetUrl, targetUrl));
    }
    await page.setRequestInterception(true);
    page.on('request', (interceptedRequest) => {
      assertAllowedBrowserRequest(interceptedRequest.url())
        .then(() => interceptedRequest.continue().catch(() => undefined))
        .catch((error: unknown) => {
          blockedRequestError ??= error instanceof Error ? error : new Error(String(error));
          interceptedRequest.abort('blockedbyclient').catch(() => undefined);
        });
    });
    const response = await page
      .goto(targetUrl.toString(), {
        waitUntil:
          input.waitUntil === 'networkidle'
            ? 'networkidle2'
            : (input.waitUntil ?? 'domcontentloaded'),
        timeout: getTimeoutMs(input),
      })
      .catch((error: unknown) => {
        if (blockedRequestError) {
          throw blockedRequestError;
        }
        throw error;
      });
    if (input.selector) {
      await page
        .waitForSelector(input.selector, { timeout: Math.min(getTimeoutMs(input), 5_000) })
        .catch(() => undefined);
    }
    await orchestrateCaptcha(
      {
        content: () => page.content(),
        url: () => page.url(),
        evaluate: (script) => page.evaluate(script) as Promise<unknown>,
      },
      input,
      ctx,
    );
    if (shouldCaptureFailureScreenshot(input, 'puppeteer')) {
      failureScreenshot = await capturePuppeteerFailureScreenshot(page, 'puppeteer');
    }
    const html = trimToMax(await page.content(), getMaxHtmlChars(input));
    return {
      html,
      finalUrl: page.url(),
      status: response?.status(),
      contentType: response?.headers()['content-type'],
      truncated: html.length >= getMaxHtmlChars(input),
      failureScreenshot,
    };
  } catch (error) {
    if (shouldCaptureFailureScreenshot(input, 'puppeteer')) {
      failureScreenshot ??= await capturePuppeteerFailureScreenshot(page, 'puppeteer');
    }
    throw attachFailureScreenshot(error, failureScreenshot);
  } finally {
    await context.close();
  }
}

async function fetchWithBrowserless(
  input: ScrapeRequest,
  targetUrl: URL,
): Promise<FetchedDocument> {
  const contentUrl = getBrowserlessContentUrl();
  const timeoutMs = getTimeoutMs(input);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(contentUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        url: targetUrl.toString(),
      }),
      signal: controller.signal,
    });
    if (!response.ok) {
      const body = await response.text().catch(() => '');
      throw new Error(
        `browserless content API returned ${response.status}: ${body.slice(0, 500)}`,
      );
    }
    const read = await readResponseText(response, getMaxHtmlChars(input));
    return {
      html: read.text,
      finalUrl: targetUrl.toString(),
      status: response.status,
      contentType: response.headers.get('content-type') ?? undefined,
      truncated: read.truncated,
    };
  } finally {
    clearTimeout(timeout);
  }
}

// --- Proxy rotation ---------------------------------------------------------

async function createScrapeContext(
  input: ScrapeRequest,
  targetUrl: URL,
  strategy: StrategyName,
): Promise<ScrapeContext> {
  // browserless manages its own egress; refuse a proxy there rather than
  // silently ignoring it and reporting a proxy that was never applied.
  if (!strategyUsesProxy(strategy)) {
    if (input.proxy) {
      throw new Error(`the ${strategy} strategy does not support proxy rotation`);
    }
    return { proxy: null, proxyFromPool: false };
  }
  const resolved = await resolveProxy(input, targetUrl);
  return {
    proxy: resolved?.entry ?? null,
    proxyFromPool: resolved?.fromPool ?? false,
  };
}

function strategyUsesProxy(strategy: StrategyName): boolean {
  return strategy !== 'browserless';
}

/**
 * A proxy that returns proxy-auth, block, or challenge responses is unhealthy —
 * cool it out of the rotation so subsequent requests try a different egress IP.
 */
function isHealthyProxyResponse(fetched: FetchedDocument, ctx: ScrapeContext): boolean {
  if (fetched.status === 407 || fetched.status === 403 || fetched.status === 429) {
    return false;
  }
  return !ctx.captcha?.detected;
}

async function resolveProxy(
  input: ScrapeRequest,
  targetUrl: URL,
): Promise<{ entry: ProxyEntry; fromPool: boolean } | null> {
  if (input.useProxy === false) {
    return null;
  }
  if (input.proxy) {
    if (!config.allowRequestProxy) {
      throw new Error('per-request proxy is disabled by scraper policy');
    }
    const entry = parseRequestProxy(input.proxy);
    await validateProxyEntry(entry);
    return { entry, fromPool: false };
  }
  const entry = proxyPool.select(targetUrl.hostname);
  return entry ? { entry, fromPool: true } : null;
}

function parseRequestProxy(raw: string): ProxyEntry {
  try {
    return parseProxyEntry(raw);
  } catch (error) {
    // Surface as a client policy error (400) rather than a 500.
    throw new Error(error instanceof Error ? error.message : `invalid proxy URL: ${raw}`);
  }
}

/**
 * DNS resolution with a hard ceiling. The pre-flight SSRF checks run before any
 * fetch timeout applies, so an un-bounded `lookup` of an attacker-chosen hostname
 * whose authoritative server stalls would pin an in-flight slot for the full libc
 * `getaddrinfo` timeout. Racing against `SCRAPER_DNS_TIMEOUT_MS` bounds that.
 */
async function lookupAllWithTimeout(
  hostname: string,
): Promise<Array<{ address: string; family: number }>> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      lookup(hostname, { all: true, verbatim: true }),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error(`DNS resolution for ${hostname} timed out`)),
          config.dnsTimeoutMs,
        );
        timer.unref();
      }),
    ]);
  } finally {
    if (timer) {
      clearTimeout(timer);
    }
  }
}

/** SSRF guard for caller-supplied proxies; pooled proxies are operator-trusted. */
async function validateProxyEntry(entry: ProxyEntry): Promise<void> {
  if (config.allowPrivateNetworks) {
    return;
  }
  const hostname = normalizeHostname(entry.hostname);
  if (isBlockedHostname(hostname)) {
    throw new Error(`proxy host ${hostname} is blocked by scraper network policy`);
  }
  if (isIP(hostname)) {
    if (isPrivateIp(hostname)) {
      throw new Error(`proxy address ${hostname} is blocked by scraper network policy`);
    }
    return;
  }
  const addresses = await lookupAllWithTimeout(hostname);
  const blocked = addresses.find((address) => isPrivateIp(address.address));
  if (blocked) {
    throw new Error(
      `proxy host ${hostname} resolved to ${blocked.address}, blocked by scraper network policy`,
    );
  }
}

function buildFetchDispatcher(proxy: ProxyEntry | null): Dispatcher | null {
  if (!proxy) {
    return null;
  }
  if (proxy.isSocks) {
    throw new Error(
      'SOCKS proxies are only supported by the playwright and puppeteer strategies, not native fetch',
    );
  }
  if (proxy.username || proxy.password) {
    const token = `Basic ${Buffer.from(`${proxy.username}:${proxy.password}`).toString('base64')}`;
    return new ProxyAgent({ uri: proxy.label, token, connect: { lookup: guardedLookup } });
  }
  return new ProxyAgent({ uri: proxy.label, connect: { lookup: guardedLookup } });
}

function playwrightProxy(proxy: ProxyEntry): {
  server: string;
  username?: string;
  password?: string;
} {
  return {
    server: proxy.label,
    ...(proxy.username ? { username: proxy.username } : {}),
    ...(proxy.password ? { password: proxy.password } : {}),
  };
}

function reportProxyOutcome(ctx: ScrapeContext, ok: boolean): void {
  if (!ctx.proxy || !ctx.proxyFromPool) {
    return;
  }
  if (ok) {
    proxyPool.reportSuccess(ctx.proxy);
  } else {
    proxyPool.reportFailure(ctx.proxy);
  }
}

function proxyInfo(ctx: ScrapeContext): ProxyInfo {
  return {
    label: ctx.proxy!.label,
    protocol: ctx.proxy!.protocol,
    rotation: config.proxyRotation,
    fromPool: ctx.proxyFromPool,
  };
}

// --- CAPTCHA orchestration --------------------------------------------------

type PageOps = {
  content: () => Promise<string>;
  url: () => string;
  evaluate: (script: string) => Promise<unknown>;
};

function shouldDetectCaptcha(input: ScrapeRequest): boolean {
  return input.detectCaptcha ?? config.detectCaptchas;
}

function detectionToOutcome(detection: CaptchaDetection): CaptchaOutcome {
  return {
    detected: detection.detected,
    type: detection.type,
    sitekey: detection.sitekey,
    solved: false,
    attempts: 0,
    ...(detection.signals.length > 0 ? { signals: detection.signals } : {}),
  };
}

function isCaptchaSolverConfigured(): boolean {
  return Boolean(config.captchaApiKey);
}

/**
 * Detect a challenge on a live page and, when auto-solve is enabled and a solver
 * is configured, fetch a token from the provider and inject it. "solved" means a
 * token was obtained and applied; the caller still extracts whatever the page
 * then renders.
 */
async function orchestrateCaptcha(
  ops: PageOps,
  input: ScrapeRequest,
  ctx: ScrapeContext,
): Promise<void> {
  if (!shouldDetectCaptcha(input)) {
    return;
  }
  const detection = detectCaptcha(await ops.content());
  const outcome = detectionToOutcome(detection);
  ctx.captcha = outcome;
  if (!detection.detected || !detection.type) {
    return;
  }
  recordCaptchaMetric('detected', detection.type);

  // Per-request input may disable an operator-enabled solver, but it cannot turn
  // solving on. This keeps access-control/challenge automation behind an
  // explicit deployment decision instead of an ordinary authenticated request.
  const autoSolve = captchaAutoSolveAllowed(config.captchaAutoSolve, input.solveCaptcha);
  const solvable = SOLVABLE_CAPTCHA_TYPES.has(detection.type) && Boolean(detection.sitekey);
  if (!autoSolve || !solvable) {
    return;
  }
  if (!config.allowCaptchaSolving) {
    outcome.error = 'captcha solving is disabled by operator policy';
    return;
  }
  if (!isCaptchaSolverConfigured()) {
    outcome.error = 'captcha solver not configured (set SCRAPER_CAPTCHA_API_KEY)';
    return;
  }
  // A solve holds the in-flight slot (and the browser page) for up to the solver
  // timeout — longer than the request timeout — and costs money. A hostile target
  // can serve a fake sitekey to trigger that. Cap concurrent solves and shed the
  // excess (report detection only) so solves can't starve capacity or amplify spend.
  if (activeCaptchaSolves >= config.captchaMaxConcurrent) {
    outcome.error = 'captcha solver concurrency limit reached';
    recordCaptchaMetric('failed', detection.type);
    return;
  }

  activeCaptchaSolves += 1;
  try {
    const maxAttempts = Math.max(1, Math.floor(config.captchaMaxAttempts));
    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      outcome.attempts = attempt;
      try {
        const result = await solveCaptcha({
          config: {
            providerUrl: config.captchaProviderUrl,
            apiKey: config.captchaApiKey!,
            pollIntervalMs: config.captchaPollIntervalMs,
            timeoutMs: config.captchaTimeoutMs,
          },
          detection,
          pageUrl: ops.url(),
          userAgent: effectiveUserAgent(input),
        });
        await ops.evaluate(buildInjectionScript(detection.type, result.token));
        await sleep(1_500);
        outcome.solved = true;
        outcome.provider = result.provider;
        outcome.solveMs = result.solveMs;
        delete outcome.error;
        recordCaptchaMetric('solved', detection.type);
        return;
      } catch (error) {
        outcome.error =
          error instanceof CaptchaSolveError || error instanceof Error
            ? error.message
            : String(error);
        if (attempt >= maxAttempts) {
          recordCaptchaMetric('failed', detection.type);
          return;
        }
      }
    }
  } finally {
    activeCaptchaSolves -= 1;
  }
}

function recordCaptchaMetric(event: CaptchaMetricEvent, type: CaptchaType | null): void {
  const key = `${event}:${type ?? 'unknown'}`;
  metrics.captcha.set(key, (metrics.captcha.get(key) ?? 0) + 1);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function normalizeProxyRotation(value: string): ProxyRotation {
  const normalized = value.trim().toLowerCase() as ProxyRotation;
  return PROXY_ROTATIONS.includes(normalized) ? normalized : 'sticky';
}

async function extractDocument(
  html: string,
  baseUrl: string,
  input: ScrapeRequest,
  strategy: StrategyName,
): Promise<ExtractionResult> {
  if (strategy === 'native-fetch') {
    if (input.selector || input.selectors) {
      throw new Error(
        'native-fetch does not support CSS selectors; use cheerio, jsdom, linkedom, or a browser strategy',
      );
    }
  }

  return runExtractionWorker({
    parser: parserForStrategy(strategy),
    html,
    baseUrl,
    selector: input.selector,
    selectors: input.selectors,
    includeHtml: input.includeHtml,
    includeText: input.includeText,
    includeLinks: input.includeLinks,
    maxHtmlChars: getMaxHtmlChars(input),
    maxTextChars: getMaxTextChars(input),
    maxLinks: config.maxLinks,
    timeoutMs: getTimeoutMs(input),
  });
}

function parserForStrategy(strategy: StrategyName): ParserName {
  if (strategy === 'native-fetch' || strategy === 'jsdom' || strategy === 'linkedom') {
    return strategy;
  }
  return 'cheerio';
}

function runExtractionWorker(input: {
  parser: ParserName;
  html: string;
  baseUrl: string;
  selector?: string;
  selectors?: Record<string, string>;
  includeHtml?: boolean;
  includeText?: boolean;
  includeLinks?: boolean;
  maxHtmlChars: number;
  maxTextChars: number;
  maxLinks: number;
  timeoutMs: number;
}): Promise<ExtractionResult> {
  return parserWorkerSemaphore.run(
    () =>
      new Promise((resolve, reject) => {
        const workerUrl = new URL(
          import.meta.url.endsWith('.ts') ? './extraction-worker.ts' : './extraction-worker.js',
          import.meta.url,
        );
        const workerEntry = import.meta.url.endsWith('.ts')
          ? new URL(
              `data:text/javascript,${encodeURIComponent(
                `import { tsImport } from ${JSON.stringify(import.meta.resolve('tsx/esm/api'))}; await tsImport(${JSON.stringify(workerUrl.href)}, import.meta.url);`,
              )}`,
            )
          : workerUrl;
        const worker = new Worker(workerEntry, {
          resourceLimits: {
            maxOldGenerationSizeMb: config.parserWorkerMemoryMb,
            stackSizeMb: 4,
          },
        });
        let settled = false;
        let timeout: NodeJS.Timeout | null = null;

        const finish = (callback: () => void): void => {
          if (settled) {
            return;
          }
          settled = true;
          if (timeout) {
            clearTimeout(timeout);
          }
          worker.terminate().catch(() => undefined);
          callback();
        };

        timeout = setTimeout(() => {
          finish(() =>
            reject(new Error(`extraction worker timed out after ${input.timeoutMs}ms`)),
          );
        }, input.timeoutMs);

        worker.once('message', (message: ExtractionWorkerResponse) => {
          finish(() => {
            if (message.ok) {
              resolve(message.extraction);
            } else {
              reject(new Error(message.error));
            }
          });
        });
        worker.once('error', (error) => {
          finish(() => reject(error));
        });
        worker.once('exit', (code) => {
          if (code !== 0) {
            finish(() => reject(new Error(`extraction worker exited with code ${code}`)));
          }
        });
        worker.postMessage(input);
      }),
  );
}

async function getPlaywrightBrowser(): Promise<PlaywrightBrowser> {
  if (playwrightBrowser?.isConnected()) {
    return playwrightBrowser;
  }
  if (!playwrightBrowserPromise) {
    playwrightBrowserPromise = (async () => {
      const { chromium } = await import('playwright');
      const browser = await chromium.launch({
        headless: config.browserHeadless,
        args: chromiumLaunchArgs(),
      });
      browser.on('disconnected', () => {
        if (playwrightBrowser === browser) {
          playwrightBrowser = null;
        }
      });
      playwrightBrowser = browser;
      return browser;
    })().finally(() => {
      playwrightBrowserPromise = null;
    });
  }
  return playwrightBrowserPromise;
}

async function getPuppeteerBrowser(): Promise<PuppeteerBrowser> {
  if (puppeteerBrowser) {
    return puppeteerBrowser;
  }
  if (!puppeteerBrowserPromise) {
    puppeteerBrowserPromise = (async () => {
      const puppeteer = await import('puppeteer');
      const executablePath = await findChromiumExecutable();
      const browser = await puppeteer.default.launch({
        headless: config.browserHeadless,
        executablePath,
        args: chromiumLaunchArgs(),
      });
      browser.on('disconnected', () => {
        if (puppeteerBrowser === browser) {
          puppeteerBrowser = null;
        }
      });
      puppeteerBrowser = browser;
      return browser;
    })().finally(() => {
      puppeteerBrowserPromise = null;
    });
  }
  return puppeteerBrowserPromise;
}

function chromiumLaunchArgs(): string[] {
  return ['--no-sandbox', '--disable-setuid-sandbox', '--disable-dev-shm-usage'];
}

async function capturePlaywrightFailureScreenshot(
  page: PlaywrightPage,
  strategy: BrowserStrategyName,
): Promise<FailureScreenshot | undefined> {
  try {
    const buffer = await page.screenshot({
      type: 'jpeg',
      quality: config.failureScreenshotQuality,
      fullPage: false,
    });
    return encodeFailureScreenshot(Buffer.from(buffer), strategy, page.url());
  } catch {
    return undefined;
  }
}

async function capturePuppeteerFailureScreenshot(
  page: PuppeteerPage,
  strategy: BrowserStrategyName,
): Promise<FailureScreenshot | undefined> {
  try {
    const buffer = await page.screenshot({
      type: 'jpeg',
      quality: config.failureScreenshotQuality,
      fullPage: false,
    });
    return encodeFailureScreenshot(Buffer.from(buffer), strategy, page.url());
  } catch {
    return undefined;
  }
}

function encodeFailureScreenshot(
  buffer: Buffer,
  strategy: BrowserStrategyName,
  finalUrl?: string,
): FailureScreenshot {
  const base = {
    strategy,
    mimeType: 'image/jpeg' as const,
    encoding: 'base64' as const,
    byteLength: buffer.byteLength,
    capturedAt: new Date().toISOString(),
    finalUrl: finalUrl && finalUrl !== 'about:blank' ? finalUrl : undefined,
  };

  if (buffer.byteLength > config.failureScreenshotMaxBytes) {
    return {
      ...base,
      omitted: true,
      omitReason: `screenshot exceeded ${config.failureScreenshotMaxBytes} bytes`,
    };
  }

  return {
    ...base,
    data: buffer.toString('base64'),
  };
}

function shouldCaptureFailureScreenshot(
  input: ScrapeRequest,
  strategy: BrowserStrategyName,
): boolean {
  return Boolean(strategy) && (input.captureFailureScreenshot ?? config.captureFailureScreenshots);
}

class ScrapeFailureError extends Error {
  readonly failureScreenshot?: FailureScreenshot;

  constructor(error: unknown, failureScreenshot?: FailureScreenshot) {
    super(error instanceof Error ? error.message : String(error));
    this.name = error instanceof Error ? error.name : 'ScrapeFailureError';
    this.stack = error instanceof Error ? error.stack : this.stack;
    this.failureScreenshot = failureScreenshot;
  }
}

function attachFailureScreenshot(error: unknown, screenshot?: FailureScreenshot): Error {
  if (!screenshot) {
    return error instanceof Error ? error : new Error(String(error));
  }
  if (error instanceof ScrapeFailureError) {
    return new ScrapeFailureError(error, error.failureScreenshot ?? screenshot);
  }
  return new ScrapeFailureError(error, screenshot);
}

function getFailureScreenshot(error: unknown): FailureScreenshot | undefined {
  return error instanceof ScrapeFailureError ? error.failureScreenshot : undefined;
}

async function findChromiumExecutable(): Promise<string | undefined> {
  for (const envName of ['PUPPETEER_EXECUTABLE_PATH', 'CHROMIUM_EXECUTABLE_PATH']) {
    const value = process.env[envName];
    if (value && (await pathExists(value))) {
      return value;
    }
  }

  for (const candidate of [
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ]) {
    if (await pathExists(candidate)) {
      return candidate;
    }
  }

  return findExecutableUnder(
    '/ms-playwright',
    new Set(['chrome', 'chromium', 'chromium-browser']),
    5,
  );
}

async function findExecutableUnder(
  root: string,
  names: Set<string>,
  depth: number,
): Promise<string | undefined> {
  if (depth < 0) {
    return undefined;
  }
  const entries = await readdir(root, { withFileTypes: true }).catch(() => []);
  for (const entry of entries) {
    const absolute = join(root, entry.name);
    if (entry.isFile() && names.has(entry.name)) {
      return absolute;
    }
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) {
      continue;
    }
    const found = await findExecutableUnder(join(root, entry.name), names, depth - 1);
    if (found) {
      return found;
    }
  }
  return undefined;
}

async function pathExists(path: string): Promise<boolean> {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function validateTargetUrl(rawUrl: string): Promise<URL> {
  const url = new URL(rawUrl);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('only http and https URLs are supported');
  }
  if (!config.allowUrlCredentials && (url.username || url.password)) {
    throw new Error(
      'URL credentials are blocked by scraper policy; use headers only when explicitly enabled',
    );
  }
  if (config.allowPrivateNetworks) {
    return url;
  }

  const hostname = normalizeHostname(url.hostname);
  if (isBlockedHostname(hostname)) {
    throw new Error(`target host ${hostname} is blocked by scraper network policy`);
  }

  if (isIP(hostname)) {
    if (isPrivateIp(hostname)) {
      throw new Error(`target address ${hostname} is blocked by scraper network policy`);
    }
    return url;
  }

  const addresses = await lookupAllWithTimeout(hostname);
  const blocked = addresses.find((entry) => isPrivateIp(entry.address));
  if (blocked) {
    throw new Error(
      `target host ${hostname} resolved to ${blocked.address}, blocked by scraper network policy`,
    );
  }
  return url;
}

async function enforceResponsibleScrapingPolicy(
  input: ScrapeRequest,
  targetUrl: URL,
  ctx: ScrapeContext,
): Promise<void> {
  const respectRobots = input.respectRobots ?? config.respectRobots;
  let crawlDelayMs = config.minOriginDelayMs;
  if (!respectRobots) {
    if (!config.allowRobotsOverride) {
      throw new Error('robots.txt override is blocked by scraper policy');
    }
    metrics.robotsOverrides += 1;
  } else {
    metrics.robotsChecks += 1;
    const robotsUrl = new URL('/robots.txt', targetUrl.origin);
    const body = await loadRobotsText(robotsUrl, ctx);
    const robots = (
      robotsParser as unknown as (
        url: string,
        text: string,
      ) => {
        isAllowed(url: string, userAgent?: string): boolean | undefined;
        getCrawlDelay(userAgent?: string): number | undefined;
      }
    )(robotsUrl.toString(), body);
    if (robots.isAllowed(targetUrl.toString(), effectiveUserAgent(input)) === false) {
      metrics.robotsDenials += 1;
      throw new Error(`robots.txt disallows ${targetUrl.pathname || '/'}`);
    }
    const declaredDelaySeconds = robots.getCrawlDelay(effectiveUserAgent(input));
    if (declaredDelaySeconds !== undefined && Number.isFinite(declaredDelaySeconds)) {
      crawlDelayMs = Math.max(
        crawlDelayMs,
        Math.min(60_000, Math.max(0, declaredDelaySeconds * 1_000)),
      );
    }
  }
  await waitForOriginTurn(targetUrl.origin, crawlDelayMs);
}

async function loadRobotsText(robotsUrl: URL, ctx: ScrapeContext): Promise<string> {
  const cached = robotsCache.get(robotsUrl.origin);
  if (cached && cached.expiresAt > Date.now()) {
    return cached.body;
  }

  await validateTargetUrl(robotsUrl.toString());
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), Math.min(config.dnsTimeoutMs, 5_000));
  const proxyDispatcher = buildFetchDispatcher(ctx.proxy);
  try {
    const response = await fetch(robotsUrl, {
      method: 'GET',
      redirect: 'error',
      headers: { 'user-agent': config.userAgent },
      signal: controller.signal,
      dispatcher: proxyDispatcher ?? guardedAgent,
    } as RequestInit & { dispatcher: Dispatcher });
    if (response.status >= 500) {
      throw new Error(`robots.txt unavailable with status ${response.status}`);
    }
    const body = response.ok ? (await readResponseText(response, 262_144)).text : '';
    if (robotsCache.size >= 256) {
      robotsCache.delete(robotsCache.keys().next().value!);
    }
    robotsCache.set(robotsUrl.origin, {
      body,
      expiresAt: Date.now() + config.robotsCacheTtlMs,
    });
    return body;
  } finally {
    clearTimeout(timeout);
    if (proxyDispatcher) {
      await proxyDispatcher.close().catch(() => undefined);
    }
  }
}

async function waitForOriginTurn(origin: string, delayMs: number): Promise<void> {
  const now = Date.now();
  const scheduledAt = Math.max(now, originNextRequestAt.get(origin) ?? now);
  originNextRequestAt.set(origin, scheduledAt + delayMs);
  if (originNextRequestAt.size > 1_024) {
    originNextRequestAt.delete(originNextRequestAt.keys().next().value!);
  }
  if (scheduledAt > now) {
    await sleep(scheduledAt - now);
  }
}

async function assertAllowedBrowserRequest(rawUrl: string): Promise<void> {
  const url = new URL(rawUrl);
  if (url.protocol === 'http:' || url.protocol === 'https:') {
    await validateTargetUrl(url.toString());
    return;
  }
  if (url.protocol === 'about:' || url.protocol === 'blob:' || url.protocol === 'data:') {
    return;
  }
  throw new Error(`target protocol ${url.protocol} is blocked by scraper network policy`);
}

function isBlockedHostname(hostname: string): boolean {
  return (
    hostname === '' ||
    hostname === 'localhost' ||
    hostname.endsWith('.localhost') ||
    hostname === 'host.docker.internal' ||
    hostname === 'metadata.google.internal' ||
    hostname.endsWith('.svc') ||
    hostname.endsWith('.cluster.local') ||
    hostname.endsWith('.internal')
  );
}

function isPrivateIp(address: string): boolean {
  const normalized = stripIpv6Brackets(address).split('%')[0] ?? address;
  if (normalized.toLowerCase().startsWith('::ffff:')) {
    return true;
  }
  if (isIP(normalized) === 4) {
    const octets = normalized.split('.').map((value) => Number(value));
    const [a = 0, b = 0, c = 0] = octets;
    return (
      a === 0 ||
      a === 10 ||
      a === 127 ||
      (a === 100 && b >= 64 && b <= 127) ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 0) ||
      (a === 192 && b === 168) ||
      (a === 198 && (b === 18 || b === 19)) ||
      (a === 198 && b === 51 && c === 100) ||
      (a === 203 && b === 0 && c === 113) ||
      a >= 224
    );
  }
  if (isIP(normalized) === 6) {
    const lower = normalized.toLowerCase();
    return (
      lower === '::' ||
      lower === '::1' ||
      lower.startsWith('64:ff9b:') ||
      lower.startsWith('100:') ||
      lower.startsWith('2001:2:') ||
      lower.startsWith('2001:db8:') ||
      lower.startsWith('2002:') ||
      lower.startsWith('fc') ||
      lower.startsWith('fd') ||
      lower.startsWith('fe8') ||
      lower.startsWith('fe9') ||
      lower.startsWith('fea') ||
      lower.startsWith('feb') ||
      lower.startsWith('ff')
    );
  }
  return false;
}

function normalizeHostname(hostname: string): string {
  return stripIpv6Brackets(hostname.toLowerCase()).replace(/\.+$/, '');
}

function stripIpv6Brackets(hostname: string): string {
  return hostname.replace(/^\[/, '').replace(/\]$/, '');
}

function chooseStrategy(input: ScrapeRequest, requested: StrategyInput): StrategyName {
  if (requested !== 'auto') {
    return requested;
  }
  if (input.renderJavaScript) {
    return config.autoUseBrowserless && isBrowserlessConfigured() ? 'browserless' : 'playwright';
  }
  if (input.selector || input.selectors) {
    return 'cheerio';
  }
  return 'native-fetch';
}

function normalizeStrategyInput(value: string): StrategyInput {
  const normalized = value.trim().toLowerCase();
  const strategy = strategyAliases[normalized];
  if (!strategy) {
    throw new Error(`unsupported scrape strategy: ${value}`);
  }
  return strategy;
}

function isBrowserlessConfigured(): boolean {
  if (config.browserlessToken) {
    return true;
  }
  if (!config.browserlessContentUrl) {
    return false;
  }
  try {
    return new URL(config.browserlessContentUrl).searchParams.has('token');
  } catch {
    return false;
  }
}

function getBrowserlessContentUrl(): string {
  const raw =
    config.browserlessContentUrl ?? `${config.browserlessEndpoint.replace(/\/$/, '')}/content`;
  const url = new URL(raw);
  if (config.browserlessToken && !url.searchParams.has('token')) {
    url.searchParams.set('token', config.browserlessToken);
  }
  if (!url.searchParams.has('token')) {
    throw new Error(
      'browserless strategy requires BROWSERLESS_TOKEN or BROWSERLESS_CONTENT_URL with a token query param',
    );
  }
  return url.toString();
}

async function readResponseText(
  response: Response,
  maxBytes: number,
): Promise<{ text: string; truncated: boolean }> {
  if (!response.body) {
    return { text: '', truncated: false };
  }

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  let truncated = false;

  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    if (!value) {
      continue;
    }
    const remaining = maxBytes - bytes;
    if (remaining <= 0) {
      truncated = true;
      await reader.cancel();
      break;
    }
    if (value.byteLength > remaining) {
      chunks.push(value.slice(0, remaining));
      truncated = true;
      await reader.cancel();
      break;
    }
    chunks.push(value);
    bytes += value.byteLength;
  }

  return {
    text: Buffer.concat(chunks).toString('utf8'),
    truncated,
  };
}

function isRedirectStatus(status: number): boolean {
  return status === 301 || status === 302 || status === 303 || status === 307 || status === 308;
}

function buildHeaders(
  input: ScrapeRequest,
  currentUrl?: URL,
  initialUrl?: URL,
): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [name, value] of Object.entries(input.headers ?? {})) {
    const normalizedName = name.trim().toLowerCase();
    if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/i.test(normalizedName)) {
      throw new Error(`blocked outbound header with invalid name: ${name}`);
    }
    if (ALWAYS_BLOCKED_OUTBOUND_HEADERS.has(normalizedName)) {
      throw new Error(`blocked outbound header: ${normalizedName}`);
    }
    if (SENSITIVE_OUTBOUND_HEADERS.has(normalizedName)) {
      if (!config.allowSensitiveHeaders) {
        throw new Error(`blocked sensitive outbound header: ${normalizedName}`);
      }
      if (currentUrl && initialUrl && currentUrl.origin !== initialUrl.origin) {
        continue;
      }
    }
    if (/[\u0000-\u001f\u007f]/.test(value)) {
      throw new Error(`blocked outbound header with invalid value: ${normalizedName}`);
    }
    headers[normalizedName] = value;
  }
  headers['user-agent'] = effectiveUserAgent(input);
  return headers;
}

function effectiveUserAgent(input: ScrapeRequest): string {
  return input.userAgent ?? config.userAgent;
}

function isClientPolicyError(message: string): boolean {
  return (
    message.includes('blocked by scraper network policy') ||
    message.includes('blocked by scraper policy') ||
    message.includes('blocked outbound header') ||
    message.includes('blocked sensitive outbound header') ||
    message.includes('robots.txt disallows') ||
    message.includes('maximum redirect count exceeded') ||
    message.includes('only http and https URLs are supported') ||
    message.includes('unsupported scrape strategy') ||
    message.includes('does not support CSS selectors') ||
    message.includes('per-request proxy is disabled') ||
    message.includes('invalid proxy URL') ||
    message.includes('unsupported proxy protocol') ||
    message.includes('proxy URL is missing a host') ||
    message.includes('SOCKS proxies are only supported') ||
    message.includes('does not support proxy rotation')
  );
}

function getTimeoutMs(input: ScrapeRequest): number {
  return Math.min(input.timeoutMs ?? config.defaultTimeoutMs, config.maxTimeoutMs);
}

function getMaxHtmlChars(input: ScrapeRequest): number {
  return Math.min(input.maxHtmlChars ?? config.maxHtmlChars, config.maxHtmlChars);
}

function trimToMax(value: string, maxChars: number): string {
  return value.length > maxChars ? value.slice(0, maxChars) : value;
}

function getMaxTextChars(input: ScrapeRequest): number {
  return Math.min(input.maxTextChars ?? config.maxTextChars, config.maxTextChars);
}

function recordMetric(
  strategy: StrategyName,
  status: ScrapeResultStatus,
  durationMs: number,
): void {
  const key = `${strategy}:${status}`;
  metrics.total.set(key, (metrics.total.get(key) ?? 0) + 1);
  metrics.durationSumMs.set(strategy, (metrics.durationSumMs.get(strategy) ?? 0) + durationMs);
  metrics.durationCount.set(strategy, (metrics.durationCount.get(strategy) ?? 0) + 1);
}

function renderMetrics(): string {
  const lines = [
    '# HELP dd_web_scraper_in_flight Current in-flight scrape requests.',
    '# TYPE dd_web_scraper_in_flight gauge',
    `dd_web_scraper_in_flight ${metrics.inFlight}`,
    '# HELP dd_web_scraper_requests_total Scrape requests by strategy and result.',
    '# TYPE dd_web_scraper_requests_total counter',
  ];

  for (const strategy of STRATEGIES) {
    for (const status of ['ok', 'error'] as const) {
      lines.push(
        `dd_web_scraper_requests_total{strategy="${strategy}",result="${status}"} ${metrics.total.get(`${strategy}:${status}`) ?? 0}`,
      );
    }
  }

  lines.push(
    '# HELP dd_web_scraper_duration_ms_sum Total scrape duration in milliseconds.',
    '# TYPE dd_web_scraper_duration_ms_sum counter',
  );
  for (const strategy of STRATEGIES) {
    lines.push(
      `dd_web_scraper_duration_ms_sum{strategy="${strategy}"} ${metrics.durationSumMs.get(strategy) ?? 0}`,
    );
  }

  lines.push(
    '# HELP dd_web_scraper_duration_ms_count Number of recorded scrape durations.',
    '# TYPE dd_web_scraper_duration_ms_count counter',
  );
  for (const strategy of STRATEGIES) {
    lines.push(
      `dd_web_scraper_duration_ms_count{strategy="${strategy}"} ${metrics.durationCount.get(strategy) ?? 0}`,
    );
  }

  lines.push(
    '# HELP dd_web_scraper_parser_workers Active parser worker threads.',
    '# TYPE dd_web_scraper_parser_workers gauge',
    `dd_web_scraper_parser_workers ${parserWorkerSemaphore.activeCount}`,
    '# HELP dd_web_scraper_parser_worker_queue Queued parser worker jobs.',
    '# TYPE dd_web_scraper_parser_worker_queue gauge',
    `dd_web_scraper_parser_worker_queue ${parserWorkerSemaphore.queuedCount}`,
    '# HELP dd_web_scraper_parser_worker_limit Configured parser worker concurrency.',
    '# TYPE dd_web_scraper_parser_worker_limit gauge',
    `dd_web_scraper_parser_worker_limit ${config.parserWorkerConcurrency}`,
    '# HELP dd_web_scraper_parser_worker_memory_mb Per-worker V8 old generation memory cap.',
    '# TYPE dd_web_scraper_parser_worker_memory_mb gauge',
    `dd_web_scraper_parser_worker_memory_mb ${config.parserWorkerMemoryMb}`,
  );

  const proxyStats = proxyPool.stats();
  lines.push(
    '# HELP dd_web_scraper_proxy_pool_size Configured proxies in the rotation pool.',
    '# TYPE dd_web_scraper_proxy_pool_size gauge',
    `dd_web_scraper_proxy_pool_size ${proxyStats.size}`,
    '# HELP dd_web_scraper_proxy_pool_available Proxies not currently on failure cooldown.',
    '# TYPE dd_web_scraper_proxy_pool_available gauge',
    `dd_web_scraper_proxy_pool_available ${proxyStats.available}`,
    '# HELP dd_web_scraper_proxy_selections_total Proxy selections handed out from the pool.',
    '# TYPE dd_web_scraper_proxy_selections_total counter',
    `dd_web_scraper_proxy_selections_total ${proxyStats.selections}`,
    '# HELP dd_web_scraper_proxy_failures_total Proxy failures reported back to the pool.',
    '# TYPE dd_web_scraper_proxy_failures_total counter',
    `dd_web_scraper_proxy_failures_total ${proxyStats.failures}`,
    '# HELP dd_web_scraper_robots_checks_total Scrapes checked against robots.txt.',
    '# TYPE dd_web_scraper_robots_checks_total counter',
    `dd_web_scraper_robots_checks_total ${metrics.robotsChecks}`,
    '# HELP dd_web_scraper_robots_denials_total Scrapes denied by robots.txt.',
    '# TYPE dd_web_scraper_robots_denials_total counter',
    `dd_web_scraper_robots_denials_total ${metrics.robotsDenials}`,
    '# HELP dd_web_scraper_robots_overrides_total Authorized robots.txt overrides used.',
    '# TYPE dd_web_scraper_robots_overrides_total counter',
    `dd_web_scraper_robots_overrides_total ${metrics.robotsOverrides}`,
    '# HELP dd_web_scraper_captcha_total CAPTCHA orchestration events by outcome and type.',
    '# TYPE dd_web_scraper_captcha_total counter',
  );
  for (const event of ['detected', 'solved', 'failed'] as const) {
    for (const type of CAPTCHA_METRIC_TYPES) {
      lines.push(
        `dd_web_scraper_captcha_total{event="${event}",type="${type}"} ${metrics.captcha.get(`${event}:${type}`) ?? 0}`,
      );
    }
  }

  return `${lines.join('\n')}\n`;
}

function serviceDescriptor(): ServiceDescriptor {
  return {
    service: 'dd-web-scraper',
    ok: true,
    endpoints: {
      scrape: 'POST /scrape',
      strategies: 'GET /scrape/strategies',
      status: 'GET /scrape/status',
      healthz: 'GET /scrape/healthz',
      metrics: 'GET /scrape/metrics',
    },
    strategies: STRATEGIES,
    defaultStrategy: config.defaultStrategy,
    parserWorkerConcurrency: config.parserWorkerConcurrency,
    parserWorkerMemoryMb: config.parserWorkerMemoryMb,
    browserHeadless: config.browserHeadless,
    captureFailureScreenshots: config.captureFailureScreenshots,
  };
}

function strategiesDescriptor(): StrategiesDescriptor {
  return {
    default: config.defaultStrategy,
    autoPolicy: {
      javascript:
        config.autoUseBrowserless && isBrowserlessConfigured() ? 'browserless' : 'playwright',
      selectors: 'cheerio',
      fallback: 'native-fetch',
    },
    strategies: STRATEGIES.map((strategy) => ({
      name: strategy,
      available: strategy !== 'browserless' || isBrowserlessConfigured(),
      supportsJavaScript: ['playwright', 'puppeteer', 'browserless'].includes(strategy),
      supportsSelectors: strategy !== 'native-fetch',
    })),
  };
}

function statusDescriptor(): StatusDescriptor {
  return {
    ok: true,
    service: 'dd-web-scraper',
    serverStartedAt,
    serverInstanceId,
    inFlight: metrics.inFlight,
    maxConcurrent: config.maxConcurrent,
    parserWorkerConcurrency: config.parserWorkerConcurrency,
    parserWorkerMemoryMb: config.parserWorkerMemoryMb,
    blockPrivateNetworks: !config.allowPrivateNetworks,
    maxRedirects: config.maxRedirects,
    allowSensitiveHeaders: config.allowSensitiveHeaders,
    browserlessConfigured: isBrowserlessConfigured(),
    browserHeadless: config.browserHeadless,
    captureFailureScreenshots: config.captureFailureScreenshots,
    failureScreenshotQuality: config.failureScreenshotQuality,
    failureScreenshotMaxBytes: config.failureScreenshotMaxBytes,
    proxyPoolSize: proxyPool.size,
    proxyRotation: config.proxyRotation,
    allowRequestProxy: config.allowRequestProxy,
    captchaDetection: config.detectCaptchas,
    captchaAutoSolve: config.captchaAutoSolve,
    captchaSolverConfigured: isCaptchaSolverConfigured(),
    captchaMaxConcurrent: config.captchaMaxConcurrent,
    respectRobots: config.respectRobots,
    allowRobotsOverride: config.allowRobotsOverride,
    minOriginDelayMs: config.minOriginDelayMs,
    allowCaptchaSolving: config.allowCaptchaSolving,
  };
}

function healthDescriptor(): HealthDescriptor {
  return {
    ok: true,
    service: 'dd-web-scraper',
    serverStartedAt,
    serverInstanceId,
    inFlight: metrics.inFlight,
  };
}

function isAuthorized(headers: Record<string, string | string[] | undefined>): boolean {
  if (!config.serverAuthSecret) {
    return config.allowUnauthenticated;
  }
  return (
    headerEquals(headers['x-server-auth'], config.serverAuthSecret) ||
    headerEquals(headers.auth, config.serverAuthSecret)
  );
}

function headerEquals(value: string | string[] | undefined, expected: string): boolean {
  if (Array.isArray(value)) {
    return value.some((item) => headerEquals(item, expected));
  }
  if (typeof value !== 'string') {
    return false;
  }
  const valueBuffer = Buffer.from(value);
  const expectedBuffer = Buffer.from(expected);
  if (valueBuffer.length !== expectedBuffer.length) {
    return false;
  }
  return timingSafeEqual(valueBuffer, expectedBuffer);
}

function readNumberEnv(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) {
    return fallback;
  }
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

function readBooleanEnv(name: string, fallback: boolean): boolean {
  const raw = process.env[name];
  if (!raw) {
    return fallback;
  }
  const normalized = raw.trim().toLowerCase();
  if (['1', 'true', 'yes', 'on'].includes(normalized)) {
    return true;
  }
  if (['0', 'false', 'no', 'off'].includes(normalized)) {
    return false;
  }
  return fallback;
}

function clampNumber(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

async function closeBrowsers(): Promise<void> {
  const closing: Promise<unknown>[] = [];
  if (playwrightBrowser) {
    closing.push(playwrightBrowser.close().catch(() => undefined));
    playwrightBrowser = null;
  }
  if (puppeteerBrowser) {
    closing.push(puppeteerBrowser.close().catch(() => undefined));
    puppeteerBrowser = null;
  }
  await Promise.all(closing);
}

async function main(): Promise<void> {
  if (!config.serverAuthSecret && !config.allowUnauthenticated) {
    throw new Error('SERVER_AUTH_SECRET is required unless SCRAPER_ALLOW_UNAUTHENTICATED=true');
  }
  if (!config.serverAuthSecret && config.allowUnauthenticated) {
    fastify.log.warn(
      'SCRAPER_ALLOW_UNAUTHENTICATED=true; POST /scrape will accept unauthenticated requests',
    );
  }
  await fastify.listen({ host: config.host, port: config.port });
}

function shutdown(signal: string): void {
  fastify.log.info(`${signal} received; shutting down`);
  void telemetry.shutdown();
  fastify.close().finally(() => {
    closeBrowsers().finally(() => process.exit(0));
  });
  setTimeout(() => process.exit(1), 10_000).unref();
}

process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));

main().catch((error) => {
  fastify.log.error(error);
  closeBrowsers().finally(() => process.exit(1));
});
