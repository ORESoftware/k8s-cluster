import Fastify, { type FastifyRequest } from 'fastify';
import { randomUUID } from 'node:crypto';

type ScrapeBody = {
  requestId?: string;
  url?: string;
  strategy?: string;
  renderJavaScript?: boolean;
  selector?: string;
  selectors?: Record<string, string>;
  includeHtml?: boolean;
  includeText?: boolean;
  includeLinks?: boolean;
  includeContacts?: boolean;
  includePhones?: boolean;
  includeEmails?: boolean;
  proxy?: string;
  useProxy?: boolean;
  solveCaptcha?: boolean;
  respectRobots?: boolean;
  timeoutMs?: number;
};

type UpstreamFailure = {
  ok?: false;
  requestId?: string;
  strategy?: string;
  requestedStrategy?: string;
  error?: string;
};

type ApifyItem = {
  url?: string;
  loadedUrl?: string;
  html?: string;
  text?: string;
  markdown?: string;
  metadata?: {
    title?: string;
    canonicalUrl?: string;
  };
};

const externalHost = process.env.HOST ?? '0.0.0.0';
const externalPort = readPositiveInt('PORT', 8097);
const internalPort = readPositiveInt('SCRAPER_INTERNAL_PORT', 18097);
if (externalPort === internalPort) {
  throw new Error('SCRAPER_INTERNAL_PORT must differ from PORT');
}

const apifyConfig = {
  enabled: readBoolean('SCRAPER_APIFY_FALLBACK', false),
  apiBase: (process.env.SCRAPER_APIFY_API_BASE ?? 'https://api.apify.com').replace(/\/$/, ''),
  token: process.env.APIFY_API_TOKEN ?? process.env.SCRAPER_APIFY_API_TOKEN ?? '',
  actor: process.env.SCRAPER_APIFY_ACTOR ?? 'apify~website-content-crawler',
  timeoutMs: Math.min(readPositiveInt('SCRAPER_APIFY_TIMEOUT_MS', 90_000), 300_000),
  maxConcurrent: readPositiveInt('SCRAPER_APIFY_MAX_CONCURRENT', 2),
  maxPerMinute: readPositiveInt('SCRAPER_APIFY_MAX_PER_MINUTE', 20),
  maxResponseBytes: Math.min(readPositiveInt('SCRAPER_APIFY_MAX_RESPONSE_BYTES', 2_000_000), 10_000_000),
};

let activeApifyCalls = 0;
const apifyStarts: number[] = [];

// Preserve the existing service as the policy/auth/execution authority. It binds
// only to loopback; this gateway owns the externally visible listener.
process.env.HOST = '127.0.0.1';
process.env.PORT = String(internalPort);
await import('./server.js');
await waitForUpstream();

const gateway = Fastify({ logger: true, bodyLimit: 1_048_576 });

gateway.all('/*', async (request, reply) => {
  const startedAt = Date.now();
  const upstream = await forwardToLocalScraper(request);
  const contentType = upstream.headers.get('content-type') ?? 'application/octet-stream';
  const raw = Buffer.from(await upstream.arrayBuffer());

  if (request.method === 'POST' && request.url.split('?')[0] === '/scrape') {
    const body = isRecord(request.body) ? (request.body as ScrapeBody) : null;
    const failure = parseFailure(raw, contentType);
    if (body && shouldFallbackToApify(upstream.status, failure, body)) {
      try {
        const result = await scrapeWithApify(body, failure, startedAt);
        reply.header('x-dd-scraper-fallback', 'apify');
        return reply.code(200).send(result);
      } catch (error) {
        request.log.warn(
          {
            err: error instanceof Error ? error.message : String(error),
            upstreamStatus: upstream.status,
          },
          'Apify fallback failed; returning original local scraper failure',
        );
      }
    }
  }

  copyResponseHeaders(upstream, reply);
  return reply.code(upstream.status).send(raw);
});

await gateway.listen({ host: externalHost, port: externalPort });

async function forwardToLocalScraper(request: FastifyRequest): Promise<Response> {
  const headers = new Headers();
  for (const [name, value] of Object.entries(request.headers)) {
    const lower = name.toLowerCase();
    if (['host', 'connection', 'content-length', 'transfer-encoding'].includes(lower)) {
      continue;
    }
    if (Array.isArray(value)) {
      for (const item of value) headers.append(name, item);
    } else if (value !== undefined) {
      headers.set(name, String(value));
    }
  }

  let body: string | undefined;
  if (!['GET', 'HEAD'].includes(request.method) && request.body !== undefined) {
    body = typeof request.body === 'string' ? request.body : JSON.stringify(request.body);
    if (!headers.has('content-type')) headers.set('content-type', 'application/json');
  }

  return fetch(`http://127.0.0.1:${internalPort}${request.url}`, {
    method: request.method,
    headers,
    body,
    redirect: 'manual',
  });
}

function parseFailure(raw: Buffer, contentType: string): UpstreamFailure | null {
  if (!contentType.includes('json')) return null;
  try {
    const value = JSON.parse(raw.toString('utf8')) as unknown;
    return isRecord(value) ? (value as UpstreamFailure) : null;
  } catch {
    return null;
  }
}

export function shouldFallbackToApify(
  upstreamStatus: number,
  failure: UpstreamFailure | null,
  body: ScrapeBody,
): boolean {
  if (!apifyConfig.enabled || !apifyConfig.token) return false;
  if (upstreamStatus < 500 || upstreamStatus > 599) return false;
  if (!body.url || body.selector || body.selectors) return false;
  if (body.includeContacts || body.includePhones || body.includeEmails) return false;
  if (body.proxy || body.useProxy === true || body.solveCaptcha === true) return false;
  if (body.respectRobots === false) return false;

  const error = failure?.error?.toLowerCase() ?? '';
  if (!error) return false;
  if (
    error.includes('robots.txt') ||
    error.includes('blocked by scraper') ||
    error.includes('captcha') ||
    error.includes('authorization') ||
    error.includes('unauthorized')
  ) {
    return false;
  }

  return [
    'timeout',
    'timed out',
    'fetch failed',
    'navigation failed',
    'target closed',
    'browser has been closed',
    'page crashed',
    'net::err_',
    'extraction worker',
    'browserless content api returned 5',
  ].some((marker) => error.includes(marker));
}

async function scrapeWithApify(
  body: ScrapeBody,
  failure: UpstreamFailure | null,
  startedAt: number,
): Promise<Record<string, unknown>> {
  acquireApifyBudget();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), apifyConfig.timeoutMs);
  activeApifyCalls += 1;
  try {
    const endpoint = new URL(
      `/v2/actors/${encodeURIComponent(apifyConfig.actor)}/run-sync-get-dataset-items`,
      apifyConfig.apiBase,
    );
    endpoint.searchParams.set('timeout', String(Math.ceil(apifyConfig.timeoutMs / 1000)));
    endpoint.searchParams.set('maxItems', '1');
    endpoint.searchParams.set('clean', 'true');

    const response = await fetch(endpoint, {
      method: 'POST',
      headers: {
        authorization: `Bearer ${apifyConfig.token}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify({
        startUrls: [{ url: body.url }],
        maxCrawlDepth: 0,
        maxCrawlPages: 1,
        useSitemaps: false,
        saveMarkdown: true,
        saveHtml: body.includeHtml === true,
        saveFiles: false,
        saveScreenshots: false,
      }),
      signal: controller.signal,
    });
    if (!response.ok) {
      const text = await readLimitedText(response, Math.min(apifyConfig.maxResponseBytes, 16_384));
      throw new Error(`Apify Actor returned ${response.status}: ${text.slice(0, 300)}`);
    }

    const textPayload = await readLimitedText(response, apifyConfig.maxResponseBytes);
    const value = JSON.parse(textPayload) as unknown;
    if (!Array.isArray(value) || value.length === 0 || !isRecord(value[0])) {
      throw new Error('Apify Actor returned no dataset item');
    }
    const item = value[0] as ApifyItem;
    const finalUrl = item.loadedUrl ?? item.url ?? item.metadata?.canonicalUrl ?? body.url!;
    const text = item.text ?? item.markdown ?? '';
    const extraction: Record<string, unknown> = {
      parser: 'cheerio',
      ...(item.metadata?.title ? { title: item.metadata.title } : {}),
      ...(body.includeText === false ? {} : { text }),
      ...(body.includeHtml === true && item.html ? { html: item.html } : {}),
    };

    // Keep the existing strategy enum stable for strict clients. Provider
    // provenance is additive under fallback/executionProvider.
    const localStrategy = failure?.strategy ?? body.strategy ?? 'native-fetch';
    return {
      ok: true,
      requestId: body.requestId ?? failure?.requestId ?? randomUUID(),
      strategy: localStrategy,
      requestedStrategy: body.strategy ?? failure?.requestedStrategy ?? 'auto',
      url: body.url,
      finalUrl,
      durationMs: Date.now() - startedAt,
      truncated: false,
      extraction,
      executionProvider: 'apify',
      fallback: {
        provider: 'apify',
        actor: apifyConfig.actor,
        upstreamStrategy: failure?.strategy,
        upstreamError: failure?.error,
      },
    };
  } finally {
    clearTimeout(timeout);
    activeApifyCalls -= 1;
  }
}

async function readLimitedText(response: Response, maxBytes: number): Promise<string> {
  if (!response.body) return '';
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    if (!value) continue;
    const remaining = maxBytes - bytes;
    if (remaining <= 0 || value.byteLength > remaining) {
      if (remaining > 0) chunks.push(value.slice(0, remaining));
      await reader.cancel().catch(() => undefined);
      throw new Error(`Apify response exceeded ${maxBytes} bytes`);
    }
    chunks.push(value);
    bytes += value.byteLength;
  }
  return Buffer.concat(chunks).toString('utf8');
}

function acquireApifyBudget(): void {
  const now = Date.now();
  while (apifyStarts.length > 0 && apifyStarts[0]! <= now - 60_000) apifyStarts.shift();
  if (activeApifyCalls >= apifyConfig.maxConcurrent) {
    throw new Error('Apify fallback concurrency budget exhausted');
  }
  if (apifyStarts.length >= apifyConfig.maxPerMinute) {
    throw new Error('Apify fallback per-minute budget exhausted');
  }
  apifyStarts.push(now);
}

async function waitForUpstream(): Promise<void> {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${internalPort}/healthz`);
      if (response.ok) return;
    } catch {
      // Service import starts the listener asynchronously; retry briefly.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`local scraper failed to become ready on port ${internalPort}`);
}

function copyResponseHeaders(response: Response, reply: { header(name: string, value: string): unknown }): void {
  for (const [name, value] of response.headers) {
    const lower = name.toLowerCase();
    if (['content-length', 'transfer-encoding', 'connection'].includes(lower)) continue;
    reply.header(name, value);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function readPositiveInt(name: string, fallback: number): number {
  const value = Number(process.env[name]);
  return Number.isInteger(value) && value > 0 ? value : fallback;
}

function readBoolean(name: string, fallback: boolean): boolean {
  const value = process.env[name]?.trim().toLowerCase();
  if (!value) return fallback;
  if (['1', 'true', 'yes', 'on'].includes(value)) return true;
  if (['0', 'false', 'no', 'off'].includes(value)) return false;
  return fallback;
}
