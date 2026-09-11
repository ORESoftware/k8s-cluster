import { spawn, type ChildProcess } from 'node:child_process';
import {
  createServer,
  request as httpRequest,
  type IncomingHttpHeaders,
  type IncomingMessage,
  type OutgoingHttpHeaders,
  type ServerResponse,
} from 'node:http';
import { dirname, join } from 'node:path';
import { pipeline } from 'node:stream';
import { fileURLToPath } from 'node:url';

import { tryConsumeProviderStartBudget, pruneProviderStarts } from './provider-start-budget.js';

import {
  classifyFallback,
  readApifyFallbackConfig,
  runApifyFallback,
  type FallbackDecisionReason,
  type ScrapeFallbackRequest,
} from './apify-fallback.js';

const externalHost = process.env.HOST ?? '0.0.0.0';
const externalPort = readIntegerEnv('PORT', 8097, 1, 65_535);
const coreHost = '127.0.0.1';
const corePort = readIntegerEnv('SCRAPER_CORE_PORT', 18_097, 1, 65_535);
const startupTimeoutMs = readIntegerEnv('SCRAPER_CORE_STARTUP_TIMEOUT_MS', 30_000, 1_000, 300_000);
const maxRequestBodyBytes = readIntegerEnv('SCRAPER_SUPERVISOR_MAX_BODY_BYTES', 1_048_576, 1_024, 10_000_000);
const maxCoreResponseBytes = readIntegerEnv(
  'SCRAPER_SUPERVISOR_MAX_CORE_RESPONSE_BYTES',
  4_194_304,
  65_536,
  25_000_000,
);
const apifyConfig = readApifyFallbackConfig();
const apifyMaxPerMinute = readIntegerEnv('APIFY_FALLBACK_MAX_PER_MINUTE', 20, 1, 600);

if (corePort === externalPort && (externalHost === coreHost || externalHost === '127.0.0.1')) {
  throw new Error('SCRAPER_CORE_PORT must differ from PORT');
}

type SupervisorSkipReason = FallbackDecisionReason | 'provider-rate-limit';

const fallbackMetrics = {
  attempts: 0,
  success: 0,
  failure: 0,
  skipped: new Map<SupervisorSkipReason, number>(),
};

let shuttingDown = false;
let child: ChildProcess | null = null;
let apifyInFlight = 0;
let apifyCooldownUntil = 0;
const apifyStarts: number[] = [];

const server = createServer((request, response) => {
  void routeRequest(request, response).catch((error) => {
    const message = safeErrorMessage(error);
    log('error', 'supervisor_request_failed', { error: message });
    if (!response.headersSent) {
      const statusCode = message.startsWith('request body exceeds') ? 413 : 502;
      respondJson(response, statusCode, {
        ok: false,
        error: statusCode === 413 ? message : 'scraper supervisor request failed',
      });
    } else {
      response.destroy();
    }
  });
});

async function main(): Promise<void> {
  child = startCore();
  await waitForCore();

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(externalPort, externalHost, () => {
      server.off('error', reject);
      resolve();
    });
  });

  log('info', 'supervisor_started', {
    host: externalHost,
    port: externalPort,
    corePort,
    apifyFallbackEnabled: apifyConfig.enabled,
    apifyFallbackConfigured: apifyConfig.enabled && Boolean(apifyConfig.token),
    apifyActorId: apifyConfig.actorId,
    apifyMaxPerMinute,
  });
}

function startCore(): ChildProcess {
  const currentDir = dirname(fileURLToPath(import.meta.url));
  const coreEntrypoint = join(currentDir, 'server.js');
  const proc = spawn(process.execPath, [coreEntrypoint], {
    env: {
      ...process.env,
      HOST: coreHost,
      PORT: String(corePort),
    },
    stdio: ['ignore', 'inherit', 'inherit'],
  });

  proc.once('error', (error) => {
    log('error', 'scraper_core_spawn_failed', { error: safeErrorMessage(error) });
    void shutdown('SIGTERM', 1);
  });
  proc.once('exit', (code, signal) => {
    if (shuttingDown) return;
    log('error', 'scraper_core_exited', { code: code ?? null, signal: signal ?? null });
    void shutdown('SIGTERM', code && code > 0 ? code : 1);
  });
  return proc;
}

async function waitForCore(): Promise<void> {
  const deadline = Date.now() + startupTimeoutMs;
  let lastError = 'core health check not attempted';
  while (Date.now() < deadline) {
    if (child?.exitCode !== null) {
      throw new Error(`scraper core exited before becoming healthy (exit=${child?.exitCode ?? 'unknown'})`);
    }
    try {
      const result = await callCoreBuffered('GET', '/healthz', {}, Buffer.alloc(0), 256_000);
      if (result.statusCode >= 200 && result.statusCode < 300) return;
      lastError = `health endpoint returned HTTP ${result.statusCode}`;
    } catch (error) {
      lastError = safeErrorMessage(error);
    }
    await delay(150);
  }
  throw new Error(`scraper core failed to become healthy: ${lastError}`);
}

async function routeRequest(request: IncomingMessage, response: ServerResponse): Promise<void> {
  const pathname = parsePathname(request.url);
  if (request.method === 'GET' && (pathname === '/fallback/status' || pathname === '/scrape/fallback/status')) {
    respondJson(response, 200, fallbackStatus());
    return;
  }

  if (request.method === 'GET' && (pathname === '/metrics' || pathname === '/scrape/metrics')) {
    await proxyMetrics(request, response);
    return;
  }

  if (request.method === 'GET' && (pathname === '/status' || pathname === '/scrape/status')) {
    await proxyStatus(request, response);
    return;
  }

  if (request.method === 'POST' && pathname === '/scrape') {
    await handleScrape(request, response);
    return;
  }

  proxyStreaming(request, response);
}

async function handleScrape(request: IncomingMessage, response: ServerResponse): Promise<void> {
  const startedAt = Date.now();
  const body = await readRequestBodyBounded(request, maxRequestBodyBytes);
  const local = await callCoreBuffered(
    request.method ?? 'POST',
    request.url ?? '/scrape',
    request.headers,
    body,
    maxCoreResponseBytes,
  );

  if (local.statusCode < 500 || local.statusCode > 599) {
    sendBufferedResponse(response, local);
    return;
  }

  const requestBody = parseJsonRecord(body.toString('utf8')) as ScrapeFallbackRequest | null;
  const localJson = parseJsonRecord(local.body.toString('utf8'));
  if (!requestBody) {
    incrementSkipped('non-retriable-error');
    sendBufferedResponse(response, local);
    return;
  }

  if (!requestBody.requestId && typeof localJson?.requestId === 'string') {
    requestBody.requestId = localJson.requestId;
  }
  const localError =
    typeof localJson?.error === 'string' ? localJson.error : local.body.toString('utf8').slice(0, 1_000);
  const decision = classifyFallback(apifyConfig, requestBody, local.statusCode, localError);
  if (!decision.eligible) {
    incrementSkipped(decision.reason);
    sendBufferedResponse(response, local);
    return;
  }

  if (apifyInFlight >= apifyConfig.maxConcurrent) {
    incrementSkipped('provider-concurrency-limit');
    sendBufferedResponse(response, local);
    return;
  }
  if (Date.now() < apifyCooldownUntil) {
    incrementSkipped('provider-cooldown');
    sendBufferedResponse(response, local);
    return;
  }

  apifyInFlight += 1;
  try {
    if (apifyConfig.minDelayMs > 0) await delay(apifyConfig.minDelayMs);
    if (!tryConsumeProviderStartBudget(apifyStarts, Date.now(), apifyMaxPerMinute)) {
      incrementSkipped('provider-rate-limit');
      sendBufferedResponse(response, local);
      return;
    }
    fallbackMetrics.attempts += 1;
    const fallback = await runApifyFallback(requestBody, apifyConfig, {
      statusCode: local.statusCode,
      strategy: typeof localJson?.strategy === 'string' ? localJson.strategy : undefined,
      error: localError,
    });
    fallbackMetrics.success += 1;
    const payload = fallback.response;
    payload.durationMs = Date.now() - startedAt;
    if (isRecord(payload.fallback)) {
      if (typeof localJson?.durationMs === 'number') payload.fallback.localDurationMs = localJson.durationMs;
      payload.fallback.totalDurationMs = Date.now() - startedAt;
    }
    respondJson(response, 200, payload);
  } catch (error) {
    fallbackMetrics.failure += 1;
    apifyCooldownUntil = Date.now() + apifyConfig.failureCooldownMs;
    const fallbackError = redactToken(safeErrorMessage(error), apifyConfig.token);
    log('warning', 'apify_fallback_failed', {
      error: fallbackError,
      localStatusCode: local.statusCode,
      requestId: typeof localJson?.requestId === 'string' ? localJson.requestId : requestBody.requestId ?? null,
    });
    const original = localJson ?? { ok: false, error: localError };
    original.fallback = {
      provider: 'apify',
      attempted: true,
      outcome: 'error',
      error: fallbackError,
    };
    respondJson(response, local.statusCode, original);
  } finally {
    apifyInFlight = Math.max(0, apifyInFlight - 1);
  }
}

async function proxyStatus(request: IncomingMessage, response: ServerResponse): Promise<void> {
  const local = await callCoreBuffered('GET', request.url ?? '/status', request.headers, Buffer.alloc(0), 1_000_000);
  if (local.statusCode < 200 || local.statusCode >= 300) {
    sendBufferedResponse(response, local);
    return;
  }
  const json = parseJsonRecord(local.body.toString('utf8'));
  if (!json) {
    sendBufferedResponse(response, local);
    return;
  }
  json.externalFallback = fallbackStatus().fallback;
  respondJson(response, 200, json);
}

async function proxyMetrics(request: IncomingMessage, response: ServerResponse): Promise<void> {
  const local = await callCoreBuffered('GET', request.url ?? '/metrics', request.headers, Buffer.alloc(0), 4_000_000);
  if (local.statusCode < 200 || local.statusCode >= 300) {
    sendBufferedResponse(response, local);
    return;
  }
  const suffix = renderFallbackMetrics();
  const text = `${local.body.toString('utf8').replace(/\s*$/, '\n')}${suffix}`;
  response.statusCode = 200;
  response.setHeader('content-type', 'text/plain; version=0.0.4; charset=utf-8');
  response.setHeader('content-length', Buffer.byteLength(text));
  response.end(text);
}

function proxyStreaming(request: IncomingMessage, response: ServerResponse): void {
  const headers = sanitizeRequestHeaders(request.headers);
  const upstream = httpRequest(
    {
      host: coreHost,
      port: corePort,
      path: request.url ?? '/',
      method: request.method,
      headers,
    },
    (upstreamResponse) => {
      response.writeHead(upstreamResponse.statusCode ?? 502, sanitizeResponseHeaders(upstreamResponse.headers));
      pipeline(upstreamResponse, response, (error) => {
        if (error && !response.destroyed) response.destroy(error);
      });
    },
  );

  upstream.on('error', (error) => {
    log('error', 'scraper_core_proxy_failed', { error: safeErrorMessage(error) });
    if (!response.headersSent) {
      respondJson(response, 502, { ok: false, error: 'local scraper core unavailable' });
    } else {
      response.destroy();
    }
  });
  request.on('aborted', () => upstream.destroy());
  pipeline(request, upstream, (error) => {
    if (error) upstream.destroy(error);
  });
}

type BufferedCoreResponse = {
  statusCode: number;
  headers: IncomingHttpHeaders;
  body: Buffer;
};

function callCoreBuffered(
  method: string,
  path: string,
  sourceHeaders: IncomingHttpHeaders | OutgoingHttpHeaders,
  body: Buffer,
  maxBytes: number,
): Promise<BufferedCoreResponse> {
  return new Promise((resolve, reject) => {
    const headers = sanitizeRequestHeaders(sourceHeaders);
    if (body.byteLength > 0) headers['content-length'] = String(body.byteLength);
    else delete headers['content-length'];

    const upstream = httpRequest(
      {
        host: coreHost,
        port: corePort,
        path,
        method,
        headers,
      },
      (upstreamResponse) => {
        const declaredLength = Number(upstreamResponse.headers['content-length']);
        if (Number.isFinite(declaredLength) && declaredLength > maxBytes) {
          upstreamResponse.destroy();
          reject(new Error(`local scraper response exceeds ${maxBytes} bytes`));
          return;
        }
        const chunks: Buffer[] = [];
        let total = 0;
        upstreamResponse.on('data', (chunk: Buffer | string) => {
          const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
          total += buffer.byteLength;
          if (total > maxBytes) {
            upstreamResponse.destroy(new Error(`local scraper response exceeds ${maxBytes} bytes`));
            return;
          }
          chunks.push(buffer);
        });
        upstreamResponse.once('error', reject);
        upstreamResponse.once('end', () => {
          resolve({
            statusCode: upstreamResponse.statusCode ?? 502,
            headers: upstreamResponse.headers,
            body: Buffer.concat(chunks, total),
          });
        });
      },
    );
    upstream.once('error', reject);
    if (body.byteLength > 0) upstream.end(body);
    else upstream.end();
  });
}

function readRequestBodyBounded(request: IncomingMessage, maxBytes: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const declaredLength = Number(request.headers['content-length']);
    if (Number.isFinite(declaredLength) && declaredLength > maxBytes) {
      reject(new Error(`request body exceeds ${maxBytes} bytes`));
      request.resume();
      return;
    }
    const chunks: Buffer[] = [];
    let total = 0;
    request.on('data', (chunk: Buffer | string) => {
      const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      total += buffer.byteLength;
      if (total > maxBytes) {
        reject(new Error(`request body exceeds ${maxBytes} bytes`));
        request.resume();
        return;
      }
      chunks.push(buffer);
    });
    request.once('error', reject);
    request.once('end', () => resolve(Buffer.concat(chunks, total)));
  });
}

function sendBufferedResponse(response: ServerResponse, local: BufferedCoreResponse): void {
  const headers = sanitizeResponseHeaders(local.headers);
  headers['content-length'] = String(local.body.byteLength);
  response.writeHead(local.statusCode, headers);
  response.end(local.body);
}

function sanitizeRequestHeaders(headers: IncomingHttpHeaders | OutgoingHttpHeaders): OutgoingHttpHeaders {
  const output: OutgoingHttpHeaders = { ...headers };
  delete output.host;
  delete output.connection;
  delete output['proxy-connection'];
  delete output['keep-alive'];
  delete output['transfer-encoding'];
  delete output.upgrade;
  return output;
}

function sanitizeResponseHeaders(headers: IncomingHttpHeaders): OutgoingHttpHeaders {
  const output: OutgoingHttpHeaders = { ...headers };
  delete output.connection;
  delete output['keep-alive'];
  delete output['transfer-encoding'];
  delete output.upgrade;
  return output;
}

function fallbackStatus(): Record<string, unknown> {
  return {
    ok: true,
    fallback: {
      provider: 'apify',
      enabled: apifyConfig.enabled,
      configured: apifyConfig.enabled && Boolean(apifyConfig.token),
      actorId: apifyConfig.actorId,
      timeoutMs: apifyConfig.timeoutMs,
      maxRequestRetries: apifyConfig.maxRequestRetries,
      maxTotalChargeUsd: apifyConfig.maxTotalChargeUsd,
      maxConcurrent: apifyConfig.maxConcurrent,
      maxPerMinute: apifyMaxPerMinute,
      startsInRollingMinute: pruneProviderStarts(apifyStarts, Date.now()),
      minDelayMs: apifyConfig.minDelayMs,
      failureCooldownMs: apifyConfig.failureCooldownMs,
      inFlight: apifyInFlight,
      cooldownUntil: apifyCooldownUntil > Date.now() ? new Date(apifyCooldownUntil).toISOString() : null,
      explicitStrategyFallback: apifyConfig.forExplicitStrategy,
    },
  };
}

function renderFallbackMetrics(): string {
  const lines = [
    '# HELP dd_web_scraper_external_fallback_attempts_total External fallback attempts after a retriable local scrape failure.',
    '# TYPE dd_web_scraper_external_fallback_attempts_total counter',
    `dd_web_scraper_external_fallback_attempts_total{provider="apify"} ${fallbackMetrics.attempts}`,
    '# HELP dd_web_scraper_external_fallback_success_total Successful external fallback scrapes.',
    '# TYPE dd_web_scraper_external_fallback_success_total counter',
    `dd_web_scraper_external_fallback_success_total{provider="apify"} ${fallbackMetrics.success}`,
    '# HELP dd_web_scraper_external_fallback_failure_total Failed external fallback scrapes.',
    '# TYPE dd_web_scraper_external_fallback_failure_total counter',
    `dd_web_scraper_external_fallback_failure_total{provider="apify"} ${fallbackMetrics.failure}`,
    '# HELP dd_web_scraper_external_fallback_in_flight Current external fallback calls.',
    '# TYPE dd_web_scraper_external_fallback_in_flight gauge',
    `dd_web_scraper_external_fallback_in_flight{provider="apify"} ${apifyInFlight}`,
    '# HELP dd_web_scraper_external_fallback_skipped_total Local failures not sent to an external fallback.',
    '# TYPE dd_web_scraper_external_fallback_skipped_total counter',
  ];
  for (const [reason, count] of fallbackMetrics.skipped.entries()) {
    lines.push(`dd_web_scraper_external_fallback_skipped_total{provider="apify",reason="${reason}"} ${count}`);
  }
  return `${lines.join('\n')}\n`;
}

function incrementSkipped(reason: SupervisorSkipReason): void {
  fallbackMetrics.skipped.set(reason, (fallbackMetrics.skipped.get(reason) ?? 0) + 1);
}

function respondJson(response: ServerResponse, statusCode: number, value: unknown): void {
  const body = JSON.stringify(value);
  response.statusCode = statusCode;
  response.setHeader('content-type', 'application/json; charset=utf-8');
  response.setHeader('content-length', Buffer.byteLength(body));
  response.end(body);
}

function parsePathname(rawUrl: string | undefined): string {
  try {
    return new URL(rawUrl ?? '/', 'http://scraper.local').pathname;
  } catch {
    return '/';
  }
}

function parseJsonRecord(value: string): Record<string, any> | null {
  try {
    const parsed = JSON.parse(value) as unknown;
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, any> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function redactToken(message: string, token: string | null): string {
  if (!token) return message;
  return message.split(token).join('[REDACTED]');
}

function safeErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readIntegerEnv(name: string, fallback: number, minimum: number, maximum: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === '') return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new TypeError(`${name} must be an integer in [${minimum}, ${maximum}]`);
  }
  return value;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function log(level: 'info' | 'warning' | 'error', event: string, fields: Record<string, unknown>): void {
  const line = JSON.stringify({
    timestamp: new Date().toISOString(),
    level,
    service: 'dd-web-scraper-supervisor',
    event,
    ...fields,
  });
  if (level === 'error') console.error(line);
  else if (level === 'warning') console.warn(line);
  else console.log(line);
}

async function shutdown(signal: NodeJS.Signals, exitCode = 0): Promise<void> {
  if (shuttingDown) return;
  shuttingDown = true;
  log('info', 'supervisor_shutdown', { signal });
  if (server.listening) {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
  if (child && child.exitCode === null) child.kill(signal);
  const timer = setTimeout(() => {
    if (child && child.exitCode === null) child.kill('SIGKILL');
  }, 5_000);
  timer.unref();
  if (child && child.exitCode === null) {
    await new Promise<void>((resolve) => child?.once('exit', () => resolve()));
  }
  clearTimeout(timer);
  process.exitCode = exitCode;
}

process.once('SIGTERM', () => void shutdown('SIGTERM'));
process.once('SIGINT', () => void shutdown('SIGINT'));

main().catch((error) => {
  log('error', 'supervisor_start_failed', { error: safeErrorMessage(error) });
  void shutdown('SIGTERM', 1);
});
