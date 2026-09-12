import {
  effectiveProviderTimeoutMs,
  isSafeFallbackTarget,
  normalizeApifyItem,
  runApifyFallback,
  type ApifyFallbackConfig,
  type ApifyRunResult,
  type ScrapeFallbackRequest,
} from './apify-fallback.js';

export type ApifyActorProfile = 'web-scraper' | 'playwright-scraper' | 'puppeteer-scraper';

const APIFY_BROWSER_PAGE_FUNCTION = `async function pageFunction(context) {
  const { page, request, response } = context;
  const customData = request?.userData?.dd || {};
  const payload = await page.evaluate((options) => {
    const doc = document;
    const maxHtmlChars = Math.max(1000, Number(options.maxHtmlChars) || 1000000);
    const maxTextChars = Math.max(500, Number(options.maxTextChars) || 40000);
    const maxLinks = Math.max(0, Number(options.maxLinks) || 250);
    const includeHtml = options.includeHtml === true;
    const includeText = options.includeText !== false;
    const includeLinks = options.includeLinks === true;
    const collectContacts = options.collectContacts === true;
    const normalizeText = (value) => String(value || '').replace(/\\s+/g, ' ').trim();
    const clamp = (value, max) => String(value || '').slice(0, max);
    const allHtml = doc.documentElement ? doc.documentElement.outerHTML : '';
    const allText = normalizeText(doc.body ? doc.body.innerText : doc.documentElement?.textContent || '');
    const result = {
      title: normalizeText(doc.title),
      truncated: allHtml.length > maxHtmlChars || allText.length > maxTextChars,
    };

    if (includeText) result.text = clamp(allText, maxTextChars);
    if (includeHtml) result.html = clamp(allHtml, maxHtmlChars);
    if (collectContacts) {
      result.rawHtml = clamp(allHtml, maxHtmlChars);
      result.rawText = clamp(allText, Math.max(maxTextChars, 100000));
    }

    const queryAll = (selector) => {
      try {
        return Array.from(doc.querySelectorAll(selector));
      } catch (error) {
        throw new Error('invalid CSS selector: ' + String(error?.message || error));
      }
    };

    if (typeof options.selector === 'string' && options.selector.length > 0) {
      const nodes = queryAll(options.selector);
      const first = nodes[0];
      result.selection = {
        selector: options.selector,
        count: nodes.length,
        text: first ? clamp(normalizeText(first.textContent), maxTextChars) : '',
        html: includeHtml && first ? clamp(first.innerHTML || '', maxHtmlChars) : undefined,
      };
    }

    if (options.selectors && typeof options.selectors === 'object') {
      result.fields = {};
      for (const [name, selector] of Object.entries(options.selectors)) {
        const first = queryAll(String(selector))[0];
        result.fields[name] = first ? clamp(normalizeText(first.textContent), maxTextChars) : '';
      }
    }

    if (includeLinks) {
      const links = [];
      for (const anchor of Array.from(doc.querySelectorAll('a[href]'))) {
        if (links.length >= maxLinks) break;
        try {
          links.push(new URL(anchor.getAttribute('href'), location.href).href);
        } catch {
          // Match the local scraper: malformed hrefs are ignored.
        }
      }
      result.links = links;
    }
    return result;
  }, customData);

  return {
    finalUrl: request?.loadedUrl || request?.url || page.url(),
    statusCode: typeof response?.status === 'function' ? response.status() : undefined,
    contentType:
      typeof response?.headers === 'function'
        ? (await response.headers())?.['content-type']
        : undefined,
    ...payload,
  };
}`;

export function actorProfile(actorId: string): ApifyActorProfile {
  switch (actorId.trim().toLowerCase()) {
    case 'apify/playwright-scraper':
      return 'playwright-scraper';
    case 'apify/puppeteer-scraper':
      return 'puppeteer-scraper';
    default:
      return 'web-scraper';
  }
}

export async function runApifyActorFallback(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
  localFailure: { statusCode: number; strategy?: string; error?: string },
): Promise<ApifyRunResult> {
  const profile = actorProfile(config.actorId);
  if (profile === 'web-scraper') return runApifyFallback(request, config, localFailure);
  return runBrowserActorFallback(request, config, localFailure, profile);
}

export function buildBrowserActorInput(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
  profile: Exclude<ApifyActorProfile, 'web-scraper'>,
): Record<string, unknown> {
  if (!request.url || !isSafeFallbackTarget(request.url)) {
    throw new TypeError('Apify fallback target is not eligible');
  }
  const collectPhones = request.includePhones ?? request.includeContacts ?? false;
  const collectEmails = request.includeEmails ?? request.includeContacts ?? false;
  const maxHtmlChars = clampInteger(request.maxHtmlChars ?? config.maxHtmlChars, 1_000, config.maxHtmlChars);
  const maxTextChars = clampInteger(request.maxTextChars ?? config.maxTextChars, 500, config.maxTextChars);
  const requestTimeoutMs = clampInteger(request.timeoutMs ?? config.timeoutMs, 1_000, config.timeoutMs);
  const pageLoadTimeoutSecs = Math.max(1, Math.min(300, Math.ceil(requestTimeoutMs / 1000)));

  return {
    startUrls: [
      {
        url: request.url,
        userData: {
          dd: {
            selector: request.selector,
            selectors: request.selectors,
            includeHtml: request.includeHtml === true,
            includeText: request.includeText !== false,
            includeLinks: request.includeLinks === true,
            collectContacts: collectPhones || collectEmails,
            maxHtmlChars,
            maxTextChars,
            maxLinks: config.maxLinks,
          },
        },
      },
    ],
    respectRobotsTxtFile: true,
    linkSelector: '',
    globs: [],
    pseudoUrls: [],
    excludes: [],
    pageFunction: APIFY_BROWSER_PAGE_FUNCTION,
    proxyConfiguration: { useApifyProxy: true },
    maxRequestRetries: config.maxRequestRetries,
    maxPagesPerCrawl: 1,
    maxResultsPerCrawl: 1,
    maxConcurrency: 1,
    pageLoadTimeoutSecs,
    pageFunctionTimeoutSecs: Math.max(1, Math.min(60, Math.ceil(config.timeoutMs / 1000))),
    waitUntil: mapBrowserWaitUntil(request.waitUntil, profile),
  };
}

async function runBrowserActorFallback(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
  localFailure: { statusCode: number; strategy?: string; error?: string },
  profile: Exclude<ApifyActorProfile, 'web-scraper'>,
): Promise<ApifyRunResult> {
  if (!config.enabled || !config.token) throw new Error('Apify fallback is not configured');
  const providerTimeoutMs = effectiveProviderTimeoutMs(request, config);
  const providerConfig = providerTimeoutMs === config.timeoutMs ? config : { ...config, timeoutMs: providerTimeoutMs };
  const input = buildBrowserActorInput(request, providerConfig, profile);
  const actorRef = encodeActorRef(config.actorId);
  const endpoint = new URL(
    `${config.apiBaseUrl.replace(/\/+$/, '')}/actors/${actorRef}/run-sync-get-dataset-items`,
  );
  endpoint.searchParams.set('clean', 'true');
  endpoint.searchParams.set('limit', '1');
  endpoint.searchParams.set('timeout', String(Math.max(5, Math.ceil(providerTimeoutMs / 1000))));
  endpoint.searchParams.set('maxItems', '1');
  endpoint.searchParams.set('maxTotalChargeUsd', String(config.maxTotalChargeUsd));

  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), providerTimeoutMs);
  const startedAt = Date.now();
  let response: Response;
  try {
    response = await fetch(endpoint, {
      method: 'POST',
      headers: {
        accept: 'application/json',
        authorization: `Bearer ${config.token}`,
        'content-type': 'application/json',
      },
      body: JSON.stringify(input),
      redirect: 'error',
      signal: controller.signal,
    });
  } catch (error) {
    if (controller.signal.aborted) {
      throw new Error(`Apify fallback timed out after ${providerTimeoutMs}ms`);
    }
    throw new Error(`Apify fallback request failed: ${safeErrorMessage(error)}`);
  } finally {
    clearTimeout(timeout);
  }

  const responseText = await readResponseTextBounded(response, config.maxResponseBytes);
  if (!response.ok) {
    throw new Error(`Apify fallback returned HTTP ${response.status}: ${summarizeProviderError(responseText)}`);
  }

  let items: unknown;
  try {
    items = JSON.parse(responseText);
  } catch {
    throw new Error('Apify fallback returned invalid JSON');
  }
  if (!Array.isArray(items) || items.length < 1 || !isRecord(items[0])) {
    throw new Error('Apify fallback returned no dataset item');
  }

  const durationMs = Date.now() - startedAt;
  return {
    response: normalizeApifyItem(request, providerConfig, items[0], localFailure, durationMs),
    durationMs,
  };
}

function mapBrowserWaitUntil(
  waitUntil: ScrapeFallbackRequest['waitUntil'],
  profile: Exclude<ApifyActorProfile, 'web-scraper'>,
): string {
  if (waitUntil === 'load') return 'load';
  if (waitUntil === 'networkidle') return profile === 'puppeteer-scraper' ? 'networkidle2' : 'networkidle';
  return 'domcontentloaded';
}

function encodeActorRef(actorId: string): string {
  const normalized = actorId.trim().replace('/', '~');
  return encodeURIComponent(normalized).replace(/%7E/gi, '~');
}

async function readResponseTextBounded(response: Response, maxBytes: number): Promise<string> {
  const declaredLength = Number(response.headers.get('content-length'));
  if (Number.isFinite(declaredLength) && declaredLength > maxBytes) {
    throw new Error(`Apify fallback response exceeds ${maxBytes} bytes`);
  }
  if (!response.body) return '';
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    if (!value) continue;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel();
      throw new Error(`Apify fallback response exceeds ${maxBytes} bytes`);
    }
    chunks.push(value);
  }
  const merged = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(merged);
}

function summarizeProviderError(value: string): string {
  return value.replace(/\s+/g, ' ').trim().slice(0, 300) || 'empty provider response';
}

function clampInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isFinite(value)) return minimum;
  return Math.max(minimum, Math.min(maximum, Math.floor(value)));
}

function safeErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
