export type ApifyFallbackConfig = {
  token: string | null;
  actorId: string;
  apiBaseUrl: string;
  timeoutMs: number;
  maxResponseBytes: number;
  useApifyProxy: boolean;
};

export type ApifyFallbackInput = {
  targetUrl: URL;
  maxHtmlChars: number;
  userAgent: string;
  requiresHtml?: boolean;
};

export type ApifyFetchedDocument = {
  html: string;
  finalUrl: string;
  status?: number;
  contentType?: string;
  truncated: boolean;
};

type FetchLike = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

type ApifyDatasetItem = {
  url?: unknown;
  loadedUrl?: unknown;
  html?: unknown;
  text?: unknown;
  markdown?: unknown;
  httpStatusCode?: unknown;
  statusCode?: unknown;
  metadata?: {
    title?: unknown;
    canonicalUrl?: unknown;
  };
  crawl?: {
    loadedUrl?: unknown;
    httpStatusCode?: unknown;
  };
};

const DEFAULT_APIFY_ACTOR_ID = 'apify~website-content-crawler';
const DEFAULT_APIFY_API_BASE_URL = 'https://api.apify.com/v2';

export function defaultApifyActorId(): string {
  return DEFAULT_APIFY_ACTOR_ID;
}

export function defaultApifyApiBaseUrl(): string {
  return DEFAULT_APIFY_API_BASE_URL;
}

export function isApifyConfigured(config: Pick<ApifyFallbackConfig, 'token'>): boolean {
  return Boolean(config.token?.trim());
}

export function buildApifyRunUrl(
  config: Pick<ApifyFallbackConfig, 'actorId' | 'apiBaseUrl'>,
): string {
  const base = new URL(config.apiBaseUrl);
  if (base.protocol !== 'https:') {
    throw new Error('Apify API base URL must use https');
  }
  const actorId = normalizeActorId(config.actorId);
  const basePath = base.pathname.replace(/\/+$/, '');
  base.pathname = `${basePath}/actors/${encodeURIComponent(actorId)}/run-sync-get-dataset-items`;
  base.search = '';
  base.hash = '';
  base.searchParams.set('format', 'json');
  base.searchParams.set('clean', 'true');
  base.searchParams.set('limit', '1');
  return base.toString();
}

export function buildApifyActorInput(
  input: Pick<ApifyFallbackInput, 'targetUrl' | 'userAgent'>,
  config: Pick<ApifyFallbackConfig, 'useApifyProxy'>,
): Record<string, unknown> {
  return {
    startUrls: [{ url: input.targetUrl.toString() }],
    crawlerType: 'playwright:adaptive',
    maxCrawlDepth: 0,
    maxCrawlPages: 1,
    useSitemaps: false,
    useLlmsTxt: false,
    respectRobotsTxtFile: true,
    proxyConfiguration: { useApifyProxy: config.useApifyProxy },
    customHttpHeaders: {
      'user-agent': input.userAgent,
    },
    blockMedia: true,
    saveHtml: true,
    saveMarkdown: true,
    summarize: false,
  };
}

export async function fetchWithApifyFallback(
  input: ApifyFallbackInput,
  config: ApifyFallbackConfig,
  fetchImpl: FetchLike = fetch,
): Promise<ApifyFetchedDocument> {
  const token = config.token?.trim();
  if (!token) {
    throw new Error('Apify fallback is not configured (set APIFY_TOKEN)');
  }

  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), config.timeoutMs);
  timeout.unref?.();
  try {
    const response = await fetchImpl(buildApifyRunUrl(config), {
      method: 'POST',
      headers: {
        authorization: `Bearer ${token}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify(buildApifyActorInput(input, config)),
      signal: controller.signal,
    });

    if (!response.ok) {
      const detail = await readResponseTextCapped(response, 2_048).catch(() => '');
      const suffix = detail.text.trim() ? `: ${detail.text.trim().slice(0, 500)}` : '';
      throw new Error(`Apify Actor API returned ${response.status}${suffix}`);
    }

    const parsed = await readJsonCapped(response, config.maxResponseBytes);
    if (!Array.isArray(parsed) || parsed.length === 0) {
      throw new Error('Apify fallback returned no dataset items');
    }

    const item = parsed[0] as ApifyDatasetItem;
    const rawHtml = asNonEmptyString(item.html);
    const bodyText =
      asNonEmptyString(item.text) ??
      asNonEmptyString(item.markdown);

    if (input.requiresHtml && !rawHtml) {
      throw new Error('Apify fallback did not return HTML required for selector extraction');
    }
    if (!rawHtml && !bodyText) {
      throw new Error('Apify fallback returned an empty dataset item');
    }

    const htmlSource = rawHtml ?? synthesizeHtml(item, bodyText ?? '');
    const html = trimToMax(htmlSource, input.maxHtmlChars);
    const finalUrl =
      asNonEmptyString(item.crawl?.loadedUrl) ??
      asNonEmptyString(item.loadedUrl) ??
      asNonEmptyString(item.metadata?.canonicalUrl) ??
      asNonEmptyString(item.url) ??
      input.targetUrl.toString();
    const status =
      asHttpStatus(item.crawl?.httpStatusCode) ??
      asHttpStatus(item.httpStatusCode) ??
      asHttpStatus(item.statusCode);

    return {
      html,
      finalUrl,
      ...(status !== undefined ? { status } : {}),
      contentType: 'text/html; charset=utf-8',
      truncated: htmlSource.length > input.maxHtmlChars,
    };
  } finally {
    clearTimeout(timeout);
  }
}

function normalizeActorId(value: string): string {
  const normalized = value.trim().replace('/', '~');
  if (!normalized || normalized.length > 200 || !/^[A-Za-z0-9_.~-]+$/.test(normalized)) {
    throw new Error('invalid Apify actor ID');
  }
  return normalized;
}

async function readJsonCapped(response: Response, maxBytes: number): Promise<unknown> {
  const declared = Number.parseInt(response.headers.get('content-length') ?? '0', 10);
  if (Number.isFinite(declared) && declared > maxBytes) {
    throw new Error(`Apify response exceeded ${maxBytes} bytes`);
  }
  const read = await readResponseTextCapped(response, maxBytes);
  if (read.truncated) {
    throw new Error(`Apify response exceeded ${maxBytes} bytes`);
  }
  try {
    return JSON.parse(read.text);
  } catch {
    throw new Error('Apify fallback returned invalid JSON');
  }
}

async function readResponseTextCapped(
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
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value) continue;
      const remaining = maxBytes - bytes;
      if (remaining <= 0) {
        truncated = true;
        await reader.cancel().catch(() => undefined);
        break;
      }
      if (value.byteLength > remaining) {
        chunks.push(value.slice(0, remaining));
        bytes += remaining;
        truncated = true;
        await reader.cancel().catch(() => undefined);
        break;
      }
      chunks.push(value);
      bytes += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  return {
    text: Buffer.concat(chunks, bytes).toString('utf8'),
    truncated,
  };
}

function synthesizeHtml(item: ApifyDatasetItem, text: string): string {
  const title = asNonEmptyString(item.metadata?.title) ?? '';
  return `<!doctype html><html><head><meta charset="utf-8"><title>${escapeHtml(title)}</title></head><body><main><pre>${escapeHtml(text)}</pre></main></body></html>`;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function asNonEmptyString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value : undefined;
}

function asHttpStatus(value: unknown): number | undefined {
  return Number.isInteger(value) && Number(value) >= 100 && Number(value) <= 599
    ? Number(value)
    : undefined;
}

function trimToMax(value: string, maxChars: number): string {
  return value.length > maxChars ? value.slice(0, maxChars) : value;
}
