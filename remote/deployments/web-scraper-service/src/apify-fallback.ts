import { randomUUID } from 'node:crypto';
import { isIP } from 'node:net';

import { extractContacts } from './contacts.js';

export type ScrapeFallbackRequest = {
  requestId?: string;
  url?: string;
  strategy?: string;
  selector?: string;
  selectors?: Record<string, string>;
  includeHtml?: boolean;
  includeText?: boolean;
  includeLinks?: boolean;
  includeContacts?: boolean;
  includePhones?: boolean;
  includeEmails?: boolean;
  contactRegion?: string;
  maxPhones?: number;
  maxEmails?: number;
  maxHtmlChars?: number;
  maxTextChars?: number;
  timeoutMs?: number;
  waitUntil?: 'load' | 'domcontentloaded' | 'networkidle';
  respectRobots?: boolean;
};

export type ApifyFallbackConfig = {
  enabled: boolean;
  token: string | null;
  actorId: string;
  apiBaseUrl: string;
  timeoutMs: number;
  localDefaultTimeoutMs: number;
  localMaxTimeoutMs: number;
  maxTotalTimeoutMs: number;
  maxRequestRetries: number;
  maxTotalChargeUsd: number;
  maxConcurrent: number;
  minDelayMs: number;
  failureCooldownMs: number;
  forExplicitStrategy: boolean;
  maxResponseBytes: number;
  maxHtmlChars: number;
  maxTextChars: number;
  maxLinks: number;
  maxPhones: number;
  maxEmails: number;
  contactRegion: string;
};

export type FallbackDecisionReason =
  | 'eligible'
  | 'disabled'
  | 'not-configured'
  | 'non-server-error'
  | 'explicit-strategy'
  | 'unsafe-target'
  | 'policy-or-access-error'
  | 'non-retriable-error'
  | 'provider-concurrency-limit'
  | 'provider-cooldown';

export type FallbackDecision = {
  eligible: boolean;
  reason: FallbackDecisionReason;
};

export type ApifyRunResult = {
  response: Record<string, unknown>;
  durationMs: number;
};

const POLICY_OR_ACCESS_ERROR =
  /(robots(?:\.txt)?|captcha|turnstile|recaptcha|hcaptcha|challenge|paywall|sign[ -]?in|log[ -]?in|authentication|authorization|unauthorized|forbidden|private network|metadata|policy|sensitive header|url credentials|access denied|not allowed|blocked by|operator authorization|proxy host)/i;

const RETRIABLE_ERROR =
  /(timeout|timed out|fetch failed|failed to fetch|network|navigation|net::err_|econn(?:reset|refused|aborted)|ehostunreach|enetunreach|enotfound|eai_again|socket|browser.*(?:closed|disconnect|crash|launch)|failed to launch|chromium.*(?:missing|not found)|target closed|page crashed|protocol error|parser worker|worker (?:exited|failed)|connection (?:closed|reset)|proxy (?:connection|connect|unavailable|failed)|unexpected eof|tls|certificate)/i;

const APIFY_PAGE_FUNCTION = `async function pageFunction(context) {
  const customData = context.customData || {};
  const doc = document;
  const maxHtmlChars = Math.max(1000, Number(customData.maxHtmlChars) || 1000000);
  const maxTextChars = Math.max(500, Number(customData.maxTextChars) || 40000);
  const maxLinks = Math.max(0, Number(customData.maxLinks) || 250);
  const includeHtml = customData.includeHtml === true;
  const includeText = customData.includeText !== false;
  const includeLinks = customData.includeLinks === true;
  const collectContacts = customData.collectContacts === true;

  const normalizeText = (value) => String(value || '').replace(/\\s+/g, ' ').trim();
  const clamp = (value, max) => String(value || '').slice(0, max);
  const allHtml = doc.documentElement ? doc.documentElement.outerHTML : '';
  const allText = normalizeText(doc.body ? doc.body.innerText : doc.documentElement?.textContent || '');

  const result = {
    finalUrl: context.request?.loadedUrl || context.request?.url || location.href,
    statusCode: context.response?.status?.(),
    contentType: context.response?.headers?.()['content-type'],
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

  if (typeof customData.selector === 'string' && customData.selector.length > 0) {
    const nodes = queryAll(customData.selector);
    const first = nodes[0];
    result.selection = {
      selector: customData.selector,
      count: nodes.length,
      text: first ? clamp(normalizeText(first.textContent), maxTextChars) : '',
      html: includeHtml && first ? clamp(first.innerHTML || '', maxHtmlChars) : undefined,
    };
  }

  if (customData.selectors && typeof customData.selectors === 'object') {
    result.fields = {};
    for (const [name, selector] of Object.entries(customData.selectors)) {
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
        // Ignore malformed hrefs, matching the local parser behavior.
      }
    }
    result.links = links;
  }

  return result;
}`;

export function readApifyFallbackConfig(env: NodeJS.ProcessEnv = process.env): ApifyFallbackConfig {
  const apiBaseUrl = normalizeApiBaseUrl(env.APIFY_API_BASE_URL ?? 'https://api.apify.com/v2');
  const localDefaultTimeoutMs = readInteger(env.SCRAPER_DEFAULT_TIMEOUT_MS, 30_000, 1_000, 300_000);
  const localMaxTimeoutMs = readInteger(env.SCRAPER_MAX_TIMEOUT_MS, 60_000, 1_000, 300_000);
  if (localDefaultTimeoutMs > localMaxTimeoutMs) {
    throw new TypeError('SCRAPER_DEFAULT_TIMEOUT_MS must not exceed SCRAPER_MAX_TIMEOUT_MS');
  }
  return {
    enabled: readBoolean(env.APIFY_FALLBACK_ENABLED, false),
    token: nonEmpty(env.APIFY_TOKEN),
    actorId: normalizeActorId(env.APIFY_ACTOR_ID ?? 'apify/web-scraper'),
    apiBaseUrl,
    timeoutMs: readInteger(env.APIFY_FALLBACK_TIMEOUT_MS, 90_000, 5_000, 300_000),
    localDefaultTimeoutMs,
    localMaxTimeoutMs,
    maxTotalTimeoutMs: readInteger(env.SCRAPER_MAX_TOTAL_TIMEOUT_MS, 120_000, 5_000, 600_000),
    maxRequestRetries: readInteger(env.APIFY_MAX_REQUEST_RETRIES, 2, 0, 10),
    maxTotalChargeUsd: readNumber(env.APIFY_MAX_TOTAL_CHARGE_USD, 0.25, 0.01, 100),
    maxConcurrent: readInteger(env.APIFY_FALLBACK_MAX_CONCURRENT, 1, 1, 16),
    minDelayMs: readInteger(
      env.APIFY_FALLBACK_MIN_DELAY_MS ?? env.SCRAPER_MIN_ORIGIN_DELAY_MS,
      1_000,
      0,
      300_000,
    ),
    failureCooldownMs: readInteger(env.APIFY_FALLBACK_FAILURE_COOLDOWN_MS, 60_000, 0, 3_600_000),
    forExplicitStrategy: readBoolean(env.APIFY_FALLBACK_FOR_EXPLICIT_STRATEGY, false),
    maxResponseBytes: readInteger(env.APIFY_MAX_RESPONSE_BYTES, 2_000_000, 16_384, 20_000_000),
    maxHtmlChars: readInteger(env.SCRAPER_MAX_HTML_CHARS, 1_000_000, 1_000, 10_000_000),
    maxTextChars: readInteger(env.SCRAPER_MAX_TEXT_CHARS, 40_000, 500, 1_000_000),
    maxLinks: readInteger(env.SCRAPER_MAX_LINKS, 250, 0, 10_000),
    maxPhones: readInteger(env.SCRAPER_MAX_PHONES, 50, 1, 500),
    maxEmails: readInteger(env.SCRAPER_MAX_EMAILS, 50, 1, 500),
    contactRegion: (env.SCRAPER_CONTACT_REGION ?? 'US').trim().toUpperCase(),
  };
}

export function effectiveProviderTimeoutMs(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
): number {
  const localBudgetMs = clampInteger(
    request.timeoutMs ?? config.localDefaultTimeoutMs,
    1_000,
    config.localMaxTimeoutMs,
  );
  const remainingMs = config.maxTotalTimeoutMs - localBudgetMs - config.minDelayMs;
  if (remainingMs < 1_000) {
    throw new Error(
      `Apify fallback total timeout budget is exhausted before provider call (remaining=${remainingMs}ms)`,
    );
  }
  return Math.min(config.timeoutMs, remainingMs);
}

export function classifyFallback(
  config: ApifyFallbackConfig,
  request: ScrapeFallbackRequest,
  localStatusCode: number,
  localErrorMessage: string,
): FallbackDecision {
  if (!config.enabled) return { eligible: false, reason: 'disabled' };
  if (!config.token) return { eligible: false, reason: 'not-configured' };
  if (localStatusCode < 500 || localStatusCode > 599) {
    return { eligible: false, reason: 'non-server-error' };
  }

  const requestedStrategy = String(request.strategy ?? 'auto').trim().toLowerCase();
  if (!config.forExplicitStrategy && requestedStrategy !== '' && requestedStrategy !== 'auto') {
    return { eligible: false, reason: 'explicit-strategy' };
  }

  if (!isSafeFallbackTarget(request.url)) {
    return { eligible: false, reason: 'unsafe-target' };
  }

  if (POLICY_OR_ACCESS_ERROR.test(localErrorMessage)) {
    return { eligible: false, reason: 'policy-or-access-error' };
  }

  if (!RETRIABLE_ERROR.test(localErrorMessage)) {
    return { eligible: false, reason: 'non-retriable-error' };
  }

  return { eligible: true, reason: 'eligible' };
}

export function isSafeFallbackTarget(rawUrl: string | undefined): boolean {
  if (!rawUrl) return false;
  let url: URL;
  try {
    url = new URL(rawUrl);
  } catch {
    return false;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return false;
  if (url.username || url.password) return false;

  const hostname = url.hostname.replace(/^\[/, '').replace(/\]$/, '').toLowerCase();
  if (!hostname) return false;
  // The local core performs the authoritative DNS-aware SSRF check before a
  // scrape can reach this fallback. This guard is deliberately conservative:
  // remote fallback never targets raw IPs or local-only names.
  if (isIP(hostname) !== 0) return false;
  if (
    hostname === 'localhost' ||
    hostname.endsWith('.localhost') ||
    hostname.endsWith('.local') ||
    hostname.endsWith('.internal') ||
    hostname.endsWith('.lan') ||
    hostname.endsWith('.home')
  ) {
    return false;
  }
  return true;
}

export function buildApifyActorInput(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
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
    runMode: 'PRODUCTION',
    startUrls: [{ url: request.url }],
    respectRobotsTxtFile: true,
    linkSelector: '',
    globs: [],
    pseudoUrls: [],
    excludes: [],
    pageFunction: APIFY_PAGE_FUNCTION,
    injectJQuery: false,
    proxyConfiguration: { useApifyProxy: true },
    maxRequestRetries: config.maxRequestRetries,
    maxPagesPerCrawl: 1,
    maxResultsPerCrawl: 1,
    maxConcurrency: 1,
    pageLoadTimeoutSecs,
    pageFunctionTimeoutSecs: Math.max(1, Math.min(60, Math.ceil(config.timeoutMs / 1000))),
    maxScrollHeightPixels: 0,
    debugLog: false,
    browserLog: false,
    waitUntil: mapWaitUntil(request.waitUntil),
    customData: {
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
  };
}

export async function runApifyFallback(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
  localFailure: { statusCode: number; strategy?: string; error?: string },
): Promise<ApifyRunResult> {
  if (!config.enabled || !config.token) {
    throw new Error('Apify fallback is not configured');
  }

  const providerTimeoutMs = effectiveProviderTimeoutMs(request, config);
  const providerConfig = providerTimeoutMs === config.timeoutMs ? config : { ...config, timeoutMs: providerTimeoutMs };
  const input = buildApifyActorInput(request, providerConfig);
  const actorRef = actorIdToApiRef(config.actorId);
  const endpoint = new URL(`${config.apiBaseUrl.replace(/\/+$/, '')}/actors/${actorRef}/run-sync-get-dataset-items`);
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

  const responsePayload = normalizeApifyItem(request, providerConfig, items[0], localFailure, Date.now() - startedAt);
  return { response: responsePayload, durationMs: Date.now() - startedAt };
}

export function normalizeApifyItem(
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
  item: Record<string, unknown>,
  localFailure: { statusCode: number; strategy?: string; error?: string },
  fallbackDurationMs: number,
): Record<string, unknown> {
  const finalUrl = asString(item.finalUrl) ?? request.url ?? '';
  const rawHtml = asString(item.rawHtml) ?? asString(item.html) ?? '';
  const rawText = asString(item.rawText) ?? asString(item.text) ?? '';
  const includePhones = request.includePhones ?? request.includeContacts ?? false;
  const includeEmails = request.includeEmails ?? request.includeContacts ?? false;

  const extraction: Record<string, unknown> = { parser: 'apify-web-scraper' };
  copyString(item, extraction, 'title');
  if (request.includeText !== false) copyString(item, extraction, 'text');
  if (request.includeHtml === true) copyString(item, extraction, 'html');
  if (isRecord(item.selection)) extraction.selection = item.selection;
  if (isRecord(item.fields)) extraction.fields = item.fields;
  if (request.includeLinks === true && Array.isArray(item.links)) {
    extraction.links = item.links.filter((value): value is string => typeof value === 'string').slice(0, config.maxLinks);
  }

  if (includePhones || includeEmails) {
    extraction.contacts = extractContacts({
      html: rawHtml,
      text: rawText,
      defaultRegion: (request.contactRegion ?? config.contactRegion).toUpperCase(),
      includePhones,
      includeEmails,
      maxPhones: clampInteger(request.maxPhones ?? config.maxPhones, 1, config.maxPhones),
      maxEmails: clampInteger(request.maxEmails ?? config.maxEmails, 1, config.maxEmails),
    });
  }

  return {
    ok: true,
    requestId: request.requestId ?? randomUUID(),
    strategy: 'apify',
    requestedStrategy: request.strategy ?? 'auto',
    provider: 'apify',
    url: request.url,
    finalUrl,
    ...(typeof item.statusCode === 'number' ? { status: item.statusCode } : {}),
    ...(typeof item.contentType === 'string' ? { contentType: item.contentType } : {}),
    durationMs: fallbackDurationMs,
    truncated: item.truncated === true,
    extraction,
    fallback: {
      provider: 'apify',
      actorId: config.actorId,
      trigger: 'local-retriable-5xx',
      localStatusCode: localFailure.statusCode,
      ...(localFailure.strategy ? { localStrategy: localFailure.strategy } : {}),
      durationMs: fallbackDurationMs,
    },
  };
}

function mapWaitUntil(waitUntil: ScrapeFallbackRequest['waitUntil']): string[] {
  switch (waitUntil) {
    case 'load':
      return ['load'];
    case 'networkidle':
      return ['networkidle2'];
    case 'domcontentloaded':
    default:
      return ['domcontentloaded'];
  }
}

function actorIdToApiRef(actorId: string): string {
  const normalized = normalizeActorId(actorId).replace('/', '~');
  return encodeURIComponent(normalized).replace(/%7E/gi, '~');
}

function normalizeActorId(actorId: string): string {
  const value = actorId.trim();
  if (!value || value.length > 200 || !/^[A-Za-z0-9._~-]+(?:\/[A-Za-z0-9._~-]+)?$/.test(value)) {
    throw new TypeError('APIFY_ACTOR_ID is invalid');
  }
  return value;
}

function normalizeApiBaseUrl(value: string): string {
  const url = new URL(value);
  if (url.protocol !== 'https:' || url.username || url.password) {
    throw new TypeError('APIFY_API_BASE_URL must be an HTTPS URL without credentials');
  }
  return url.toString().replace(/\/+$/, '');
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
  const compact = value.replace(/\s+/g, ' ').trim();
  return compact.slice(0, 300) || 'empty provider response';
}

function safeErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readBoolean(value: string | undefined, fallback: boolean): boolean {
  if (value === undefined || value.trim() === '') return fallback;
  switch (value.trim().toLowerCase()) {
    case '1':
    case 'true':
    case 'yes':
    case 'on':
      return true;
    case '0':
    case 'false':
    case 'no':
    case 'off':
      return false;
    default:
      throw new TypeError(`invalid boolean value: ${value}`);
  }
}

function readInteger(value: string | undefined, fallback: number, minimum: number, maximum: number): number {
  if (value === undefined || value.trim() === '') return fallback;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new TypeError(`integer must be in [${minimum}, ${maximum}]`);
  }
  return parsed;
}

function readNumber(value: string | undefined, fallback: number, minimum: number, maximum: number): number {
  if (value === undefined || value.trim() === '') return fallback;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < minimum || parsed > maximum) {
    throw new TypeError(`number must be in [${minimum}, ${maximum}]`);
  }
  return parsed;
}

function clampInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isFinite(value)) return minimum;
  return Math.max(minimum, Math.min(maximum, Math.floor(value)));
}

function nonEmpty(value: string | undefined): string | null {
  const normalized = value?.trim();
  return normalized ? normalized : null;
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function copyString(source: Record<string, unknown>, target: Record<string, unknown>, key: string): void {
  const value = source[key];
  if (typeof value === 'string') target[key] = value;
}
