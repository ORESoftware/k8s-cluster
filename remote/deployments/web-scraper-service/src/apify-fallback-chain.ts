import { isIP } from 'node:net';

import {
  type ApifyFallbackConfig,
  type ApifyRunResult,
  type ScrapeFallbackRequest,
} from './apify-fallback.js';
import { runApifyActorFallback } from './apify-actor-dispatch.js';

export type DomainFallbackRoute = {
  pattern: string;
  actors: string[];
};

export type ApifyFallbackChainConfig = {
  defaultActors: string[];
  domainRoutes: DomainFallbackRoute[];
  maxAttempts: number;
  maxChainTotalChargeUsd: number;
};

export type FallbackAttemptOutcome = 'success' | 'error';

export type FallbackAttemptErrorClass =
  | 'timeout'
  | 'rate-limit'
  | 'provider-server-error'
  | 'network'
  | 'empty-result'
  | 'invalid-response'
  | 'response-too-large'
  | 'provider-auth'
  | 'invalid-request'
  | 'unknown';

export type FallbackAttempt = {
  actorId: string;
  outcome: FallbackAttemptOutcome;
  durationMs: number;
  errorClass?: FallbackAttemptErrorClass;
};

export type FallbackChainSelection = {
  route: string;
  actors: string[];
};

export type ApifyFallbackChainResult = {
  result: ApifyRunResult;
  selection: FallbackChainSelection;
  attempts: FallbackAttempt[];
};

export class ApifyFallbackChainError extends Error {
  readonly selection: FallbackChainSelection;
  readonly attempts: FallbackAttempt[];

  constructor(message: string, selection: FallbackChainSelection, attempts: FallbackAttempt[]) {
    super(message);
    this.name = 'ApifyFallbackChainError';
    this.selection = { route: selection.route, actors: [...selection.actors] };
    this.attempts = attempts.map((attempt) => ({ ...attempt }));
  }
}

export type ApifyFallbackRunner = (
  request: ScrapeFallbackRequest,
  config: ApifyFallbackConfig,
  localFailure: { statusCode: number; strategy?: string; error?: string },
) => Promise<ApifyRunResult>;

const MAX_DOMAIN_ROUTES = 64;
const MAX_ACTORS_PER_ROUTE = 3;
const MIN_ACTOR_RUN_CHARGE_USD = 0.01;

export function readApifyFallbackChainConfig(
  env: NodeJS.ProcessEnv,
  baseConfig: ApifyFallbackConfig,
): ApifyFallbackChainConfig {
  const maxAttempts = readInteger(env.APIFY_FALLBACK_MAX_ATTEMPTS, 2, 1, MAX_ACTORS_PER_ROUTE);
  const defaultActors = parseActorList(env.APIFY_FALLBACK_ACTORS, [baseConfig.actorId]).slice(0, maxAttempts);
  const maxChainTotalChargeUsd = readNumber(
    env.APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD,
    Math.min(100, baseConfig.maxTotalChargeUsd * maxAttempts),
    MIN_ACTOR_RUN_CHARGE_USD,
    100,
  );
  const domainRoutes = parseDomainRoutes(env.APIFY_DOMAIN_FALLBACKS_JSON, maxAttempts);
  const longestConfiguredChain = Math.max(
    defaultActors.length,
    1,
    ...domainRoutes.map((route) => route.actors.length),
  );
  if (maxChainTotalChargeUsd / longestConfiguredChain < MIN_ACTOR_RUN_CHARGE_USD) {
    throw new TypeError(
      `APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD must fund at least USD ${MIN_ACTOR_RUN_CHARGE_USD.toFixed(2)} per configured Actor attempt`,
    );
  }

  return {
    defaultActors,
    domainRoutes,
    maxAttempts,
    maxChainTotalChargeUsd,
  };
}

export function selectFallbackActors(
  rawUrl: string | undefined,
  config: ApifyFallbackChainConfig,
): FallbackChainSelection {
  if (!rawUrl) return { route: 'default', actors: config.defaultActors.slice(0, config.maxAttempts) };
  let hostname = '';
  try {
    hostname = new URL(rawUrl).hostname.toLowerCase().replace(/^\[/, '').replace(/\]$/, '');
  } catch {
    return { route: 'default', actors: config.defaultActors.slice(0, config.maxAttempts) };
  }

  const match = config.domainRoutes
    .filter((route) => domainPatternMatches(route.pattern, hostname))
    .sort((left, right) => routeSpecificity(right.pattern) - routeSpecificity(left.pattern))[0];

  if (!match) return { route: 'default', actors: config.defaultActors.slice(0, config.maxAttempts) };
  return { route: match.pattern, actors: match.actors.slice(0, config.maxAttempts) };
}

export async function runApifyFallbackChain(
  request: ScrapeFallbackRequest,
  baseConfig: ApifyFallbackConfig,
  localFailure: { statusCode: number; strategy?: string; error?: string },
  chainConfig: ApifyFallbackChainConfig,
  runner: ApifyFallbackRunner = runApifyActorFallback,
): Promise<ApifyFallbackChainResult> {
  const selection = selectFallbackActors(request.url, chainConfig);
  const actors = selection.actors.slice(0, chainConfig.maxAttempts);
  if (actors.length === 0) throw new ApifyFallbackChainError('Apify fallback chain has no configured Actor', selection, []);

  const localBudgetMs = clampInteger(
    request.timeoutMs ?? baseConfig.localDefaultTimeoutMs,
    1_000,
    baseConfig.localMaxTimeoutMs,
  );
  const providerWindowMs = baseConfig.maxTotalTimeoutMs - localBudgetMs - baseConfig.minDelayMs;
  if (providerWindowMs < 1_000) {
    throw new ApifyFallbackChainError(
      `Apify fallback chain total timeout budget is exhausted (remaining=${providerWindowMs}ms)`,
      selection,
      [],
    );
  }

  // Apify's maxTotalChargeUsd is a per-run cap. Divide the chain ceiling over
  // all possible attempts so the worst-case sum of provider run ceilings can
  // never exceed APIFY_FALLBACK_CHAIN_MAX_TOTAL_CHARGE_USD.
  const perAttemptChargeCap = Math.min(
    baseConfig.maxTotalChargeUsd,
    chainConfig.maxChainTotalChargeUsd / actors.length,
  );
  if (!Number.isFinite(perAttemptChargeCap) || perAttemptChargeCap < MIN_ACTOR_RUN_CHARGE_USD) {
    throw new ApifyFallbackChainError(
      'Apify fallback chain charge budget is too small for configured attempts',
      selection,
      [],
    );
  }

  const attempts: FallbackAttempt[] = [];
  const chainStartedAt = Date.now();
  let lastError: Error | null = null;

  for (let index = 0; index < actors.length; index += 1) {
    const actorId = actors[index];
    const elapsedMs = Date.now() - chainStartedAt;
    const remainingWindowMs = providerWindowMs - elapsedMs;
    const remainingAttempts = actors.length - index;
    if (remainingWindowMs < 1_000) {
      lastError = new Error('Apify fallback chain exhausted its total timeout budget');
      break;
    }

    const perAttemptTimeoutMs = Math.max(
      1_000,
      Math.min(baseConfig.timeoutMs, Math.floor(remainingWindowMs / remainingAttempts)),
    );
    const attemptConfig: ApifyFallbackConfig = {
      ...baseConfig,
      actorId,
      timeoutMs: perAttemptTimeoutMs,
      maxTotalTimeoutMs: localBudgetMs + baseConfig.minDelayMs + perAttemptTimeoutMs,
      maxTotalChargeUsd: perAttemptChargeCap,
    };

    const attemptStartedAt = Date.now();
    try {
      const result = await runner(request, attemptConfig, localFailure);
      attempts.push({
        actorId,
        outcome: 'success',
        durationMs: Math.max(0, Date.now() - attemptStartedAt),
      });
      annotateFallbackProvenance(
        result.response,
        selection,
        attempts,
        chainConfig.maxAttempts,
        chainConfig.maxChainTotalChargeUsd,
      );
      return { result, selection, attempts };
    } catch (error) {
      const normalized = asError(error);
      const errorClass = classifyActorFailure(normalized.message);
      attempts.push({
        actorId,
        outcome: 'error',
        durationMs: Math.max(0, Date.now() - attemptStartedAt),
        errorClass,
      });
      lastError = normalized;
      if (!isRetryableActorFailure(errorClass)) break;
    }
  }

  const summary = attempts.map((attempt) => `${attempt.actorId}:${attempt.errorClass ?? attempt.outcome}`).join(',');
  throw new ApifyFallbackChainError(
    `Apify fallback chain failed after ${attempts.length}/${actors.length} attempt(s)` +
      (summary ? ` [${summary}]` : '') +
      (lastError ? `: ${lastError.message}` : ''),
    selection,
    attempts,
  );
}

export function classifyActorFailure(message: string): FallbackAttemptErrorClass {
  const normalized = message.toLowerCase();
  if (/http\s+(?:401|403)/.test(normalized) || /unauthorized|invalid token|forbidden/.test(normalized)) {
    return 'provider-auth';
  }
  if (/http\s+400/.test(normalized) || /target is not eligible|not configured|invalid actor|invalid input/.test(normalized)) {
    return 'invalid-request';
  }
  if (/timed out|timeout|aborted/.test(normalized)) return 'timeout';
  if (/http\s+429|rate.?limit|too many requests/.test(normalized)) return 'rate-limit';
  if (/http\s+5\d\d/.test(normalized)) return 'provider-server-error';
  if (/request failed|fetch failed|network|econn|enotfound|eai_again|socket|tls|certificate/.test(normalized)) {
    return 'network';
  }
  if (/no dataset item|empty result/.test(normalized)) return 'empty-result';
  if (/invalid json|invalid response/.test(normalized)) return 'invalid-response';
  if (/response exceeds/.test(normalized)) return 'response-too-large';
  return 'unknown';
}

export function isRetryableActorFailure(errorClass: FallbackAttemptErrorClass): boolean {
  return [
    'timeout',
    'rate-limit',
    'provider-server-error',
    'network',
    'empty-result',
    'invalid-response',
    'response-too-large',
  ].includes(errorClass);
}

function parseDomainRoutes(value: string | undefined, maxAttempts: number): DomainFallbackRoute[] {
  if (value === undefined || value.trim() === '') return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new TypeError('APIFY_DOMAIN_FALLBACKS_JSON must be valid JSON');
  }
  if (!isRecord(parsed)) {
    throw new TypeError('APIFY_DOMAIN_FALLBACKS_JSON must be a JSON object mapping domains to Actor arrays');
  }
  const entries = Object.entries(parsed);
  if (entries.length > MAX_DOMAIN_ROUTES) {
    throw new TypeError(`APIFY_DOMAIN_FALLBACKS_JSON may contain at most ${MAX_DOMAIN_ROUTES} routes`);
  }
  const routes: DomainFallbackRoute[] = [];
  for (const [rawPattern, rawActors] of entries) {
    const pattern = normalizeDomainPattern(rawPattern);
    if (!Array.isArray(rawActors) || rawActors.some((actor) => typeof actor !== 'string')) {
      throw new TypeError(`fallback route ${pattern} must be an array of Actor IDs`);
    }
    if (rawActors.length < 1 || rawActors.length > MAX_ACTORS_PER_ROUTE) {
      throw new TypeError(`fallback route ${pattern} must contain 1..${MAX_ACTORS_PER_ROUTE} Actor IDs`);
    }
    const actors = dedupeActors(rawActors.map((actor) => normalizeActorId(actor))).slice(0, maxAttempts);
    if (actors.length === 0) {
      throw new TypeError(`fallback route ${pattern} must contain at least one distinct Actor ID`);
    }
    routes.push({ pattern, actors });
  }
  return routes;
}

function parseActorList(value: string | undefined, fallback: string[]): string[] {
  if (value === undefined || value.trim() === '') return dedupeActors(fallback.map(normalizeActorId));
  const actors = value.split(',').map((actor) => actor.trim()).filter(Boolean).map(normalizeActorId);
  if (actors.length === 0) throw new TypeError('APIFY_FALLBACK_ACTORS must contain at least one Actor ID');
  if (actors.length > MAX_ACTORS_PER_ROUTE) {
    throw new TypeError(`APIFY_FALLBACK_ACTORS may contain at most ${MAX_ACTORS_PER_ROUTE} Actor IDs`);
  }
  return dedupeActors(actors);
}

function normalizeActorId(actorId: string): string {
  const value = actorId.trim();
  if (!value || value.length > 200 || !/^[A-Za-z0-9._~-]+(?:\/[A-Za-z0-9._~-]+)?$/.test(value)) {
    throw new TypeError('Apify Actor ID is invalid');
  }
  return value;
}

function normalizeDomainPattern(rawPattern: string): string {
  const value = rawPattern.trim().toLowerCase().replace(/\.$/, '');
  const wildcard = value.startsWith('*.');
  const hostname = wildcard ? value.slice(2) : value;
  const labels = hostname.split('.');
  const labelsValid = labels.every(
    (label) =>
      label.length >= 1 &&
      label.length <= 63 &&
      /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label),
  );
  if (
    !hostname ||
    hostname.length > 253 ||
    isIP(hostname) !== 0 ||
    hostname === 'localhost' ||
    hostname.endsWith('.localhost') ||
    hostname.endsWith('.local') ||
    hostname.endsWith('.internal') ||
    !labelsValid
  ) {
    throw new TypeError(`invalid fallback domain pattern: ${rawPattern}`);
  }
  return wildcard ? `*.${hostname}` : hostname;
}

function domainPatternMatches(pattern: string, hostname: string): boolean {
  if (!pattern.startsWith('*.')) return hostname === pattern;
  const suffix = pattern.slice(1); // includes the leading dot
  return hostname.endsWith(suffix) && hostname.length > suffix.length;
}

function routeSpecificity(pattern: string): number {
  return pattern.startsWith('*.') ? pattern.length - 2 : pattern.length + 1_000;
}

function dedupeActors(actors: string[]): string[] {
  return [...new Set(actors)];
}

function annotateFallbackProvenance(
  response: Record<string, unknown>,
  selection: FallbackChainSelection,
  attempts: FallbackAttempt[],
  maxAttempts: number,
  maxChainTotalChargeUsd: number,
): void {
  if (!isRecord(response.fallback)) return;
  response.fallback.route = selection.route;
  response.fallback.attempts = attempts.map((attempt) => ({ ...attempt }));
  response.fallback.maxChainAttempts = maxAttempts;
  response.fallback.maxChainChargeUsd = maxChainTotalChargeUsd;
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

function isRecord(value: unknown): value is Record<string, any> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}
