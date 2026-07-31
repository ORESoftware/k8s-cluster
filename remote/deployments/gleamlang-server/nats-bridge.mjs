import { timingSafeEqual } from 'node:crypto';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';

import { getNatsClient } from './nats-client.mjs';
// Source-of-truth NATS subject constants. The repo is mounted at
// /opt/dd-next-1 inside the pod so this relative path resolves identically
// in dev, CI, and prod containers.
import {
  RUNTIME_EVENTS_SUBJECT,
  WEBSOCKET_EVENTS_SUBJECT,
} from '../../libs/nats/subject-defs/generated/javascript/index.mjs';

const natsUrl = process.env.NATS_URL ?? 'nats://dd-nats.messaging.svc.cluster.local:4222';
const readSubject =
  process.env.NATS_READ_SUBJECT ?? process.env.NATS_EVENT_SUBJECT ?? RUNTIME_EVENTS_SUBJECT;
const publishSubject = process.env.NATS_PUBLISH_SUBJECT ?? WEBSOCKET_EVENTS_SUBJECT;
const broadcastUrl = process.env.GLEAM_BROADCAST_URL ?? 'http://127.0.0.1:8081/broadcast';
const broadcastSecret = requiredEnv('GLEAM_BROADCAST_SECRET');
const bridgePort = numberEnv('NATS_BRIDGE_HTTP_PORT', 8083, 65_535);
const maxBodyBytes = numberEnv('NATS_BRIDGE_MAX_BODY_BYTES', 1_048_576, 1_048_576);
const dedupeTtlMs = numberEnv('NATS_BRIDGE_DEDUPE_TTL_MS', 5 * 60 * 1000, 86_400_000);
const maxDedupeEntries = numberEnv('NATS_BRIDGE_MAX_DEDUPE_ENTRIES', 10_000, 100_000);
const maxBroadcastConcurrency = numberEnv('NATS_BRIDGE_MAX_BROADCAST_CONCURRENCY', 16, 128);
const maxBroadcastQueueDepth = numberEnv('NATS_BRIDGE_MAX_BROADCAST_QUEUE_DEPTH', 256, 4096);
const maxBroadcastQueueBytes = numberEnv(
  'NATS_BRIDGE_MAX_BROADCAST_QUEUE_BYTES',
  8 * 1024 * 1024,
  64 * 1024 * 1024,
);
const broadcastTimeoutMs = numberEnv('NATS_BRIDGE_BROADCAST_TIMEOUT_MS', 5000, 60_000);
const seenMessageIds = new Map();
const broadcastQueue = [];
let broadcastQueueBytes = 0;
let activeBroadcasts = 0;
const counters = {
  received: 0,
  published: 0,
  broadcastSucceeded: 0,
  broadcastFailed: 0,
  droppedDuplicate: 0,
  droppedInvalid: 0,
  droppedOverload: 0,
};
const apiDocsHtml = readFileSync(new URL('./generated/api-docs.nats-bridge.html', import.meta.url), 'utf8');
const apiDocsJson = readFileSync(new URL('./generated/api-docs.nats-bridge.json', import.meta.url), 'utf8');

const nats = getNatsClient({ url: natsUrl, logger: console });
nats.subscribe(readSubject, (payload) => {
  counters.received += 1;
  const event = normalizeEvent(payload);
  if (!event) {
    counters.droppedInvalid += 1;
    log('warn', 'invalid_event_dropped');
    return;
  }
  if (dropDuplicate(event.messageId)) {
    counters.droppedDuplicate += 1;
    return;
  }
  enqueueBroadcast(event.body);
});
log('info', 'subscribed', { subject: readSubject });

const publishServer = startPublishServer();
process.once('SIGTERM', shutdown);
process.once('SIGINT', shutdown);

function requiredEnv(name) {
  const value = process.env[name];
  if (!value?.trim()) {
    throw new Error(`${name} must be configured`);
  }
  return value;
}

function numberEnv(name, fallback, max) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number(raw);
  return Number.isSafeInteger(parsed) && parsed > 0 ? Math.min(parsed, max) : fallback;
}

function enqueueBroadcast(body) {
  if (activeBroadcasts < maxBroadcastConcurrency) {
    startBroadcast(body);
    return;
  }
  while (
    broadcastQueue.length >= maxBroadcastQueueDepth ||
    broadcastQueueBytes + body.length > maxBroadcastQueueBytes
  ) {
    const dropped = broadcastQueue.shift();
    if (!dropped) break;
    broadcastQueueBytes -= dropped.length;
    counters.droppedOverload += 1;
  }
  if (body.length > maxBroadcastQueueBytes) {
    counters.droppedOverload += 1;
    return;
  }
  broadcastQueue.push(body);
  broadcastQueueBytes += body.length;
}

function startBroadcast(body) {
  activeBroadcasts += 1;
  void broadcast(body).finally(() => {
    activeBroadcasts -= 1;
    const next = broadcastQueue.shift();
    if (!next) return;
    broadcastQueueBytes -= next.length;
    startBroadcast(next);
  });
}

async function broadcast(body) {
  try {
    const response = await fetch(broadcastUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-dd-internal-auth': broadcastSecret,
      },
      body,
      redirect: 'error',
      signal: AbortSignal.timeout(broadcastTimeoutMs),
    });
    if (!response.ok) {
      counters.broadcastFailed += 1;
      log('warn', 'broadcast_failed', { status: response.status });
    } else {
      counters.broadcastSucceeded += 1;
    }
    await response.body?.cancel();
  } catch (error) {
    counters.broadcastFailed += 1;
    log('warn', 'broadcast_error', {
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function dropDuplicate(messageId) {
  if (!messageId) return false;
  const now = Date.now();
  pruneSeenMessageIds(now);
  const expiresAt = seenMessageIds.get(messageId);
  if (expiresAt && expiresAt > now) {
    return true;
  }
  while (seenMessageIds.size >= maxDedupeEntries) {
    const oldest = seenMessageIds.keys().next().value;
    if (oldest === undefined) break;
    seenMessageIds.delete(oldest);
  }
  seenMessageIds.set(messageId, now + dedupeTtlMs);
  return false;
}

function pruneSeenMessageIds(now = Date.now()) {
  for (const [messageId, expiresAt] of seenMessageIds.entries()) {
    if (expiresAt <= now) {
      seenMessageIds.delete(messageId);
    }
  }
}

function normalizeEvent(payload) {
  const body = Buffer.isBuffer(payload) ? payload : Buffer.from(String(payload), 'utf8');
  if (body.length === 0 || body.length > maxBodyBytes) return null;
  try {
    const parsed = JSON.parse(body.toString('utf8'));
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
    const candidate = parsed?.messageId ?? parsed?.message_id ?? parsed?.id;
    return {
      body,
      messageId:
        typeof candidate === 'string' && candidate.length <= 128 ? candidate : null,
    };
  } catch {
    return null;
  }
}

function startPublishServer() {
  const server = createServer((request, response) => {
    void handlePublishRequest(request, response);
  });
  server.keepAliveTimeout = 5000;
  server.headersTimeout = 10_000;
  server.requestTimeout = 10_000;
  server.maxRequestsPerSocket = 100;
  server.listen(bridgePort, '127.0.0.1', () => {
    log('info', 'http_listening', { address: '127.0.0.1', port: bridgePort });
  });
  return server;
}

async function handlePublishRequest(request, response) {
  const url = new URL(request.url ?? '/', 'http://127.0.0.1');

  if (request.method === 'GET' && url.pathname === '/healthz') {
    respond(response, 200, {
      ok: true,
      readSubject,
      publishSubject,
      activeBroadcasts,
      broadcastQueueDepth: broadcastQueue.length,
      broadcastQueueBytes,
      counters,
    });
    return;
  }

  if (request.method === 'GET' && (url.pathname === '/docs/api' || url.pathname === '/api/docs')) {
    response.writeHead(200, {
      'cache-control': 'no-store',
      'content-type': 'text/html; charset=utf-8',
      'x-content-type-options': 'nosniff',
    });
    response.end(apiDocsHtml);
    return;
  }

  if (request.method === 'GET' && url.pathname === '/api/docs.json') {
    response.writeHead(200, {
      'cache-control': 'no-store',
      'content-type': 'application/json; charset=utf-8',
      'x-content-type-options': 'nosniff',
    });
    response.end(apiDocsJson);
    return;
  }

  if (request.method !== 'POST' || url.pathname !== '/publish') {
    respond(response, 404, { error: 'not-found' });
    return;
  }

  if (!secretsEqual(headerValue(request.headers['x-dd-internal-auth']), broadcastSecret)) {
    respond(response, 401, { error: 'unauthorized' });
    return;
  }

  const requestedSubject =
    headerValue(request.headers['x-nats-subject']) ?? url.searchParams.get('subject');
  const subject = requestedSubject ?? publishSubject;
  if (!validSubject(subject)) {
    respond(response, 400, { error: 'invalid-subject' });
    return;
  }
  if (subject !== publishSubject) {
    respond(response, 403, { error: 'subject-not-allowed' });
    return;
  }

  try {
    const body = await readBody(request, maxBodyBytes);
    if (!normalizeEvent(body)) {
      respond(response, 400, { error: 'invalid-json-event' });
      return;
    }
    if (!nats.publish(subject, body)) {
      respond(response, 503, { error: 'nats-unavailable' });
      return;
    }
    counters.published += 1;
    respond(response, 202, { ok: true, subject });
  } catch (error) {
    const status =
      typeof error === 'object' && error && 'status' in error ? error.status : 400;
    respond(response, status, {
      error: status === 413 ? 'body-too-large' : 'invalid-body',
    });
  }
}

function readBody(request, limit) {
  return new Promise((resolve, reject) => {
    const contentLength = Number(headerValue(request.headers['content-length']) ?? 0);
    if (Number.isFinite(contentLength) && contentLength > limit) {
      reject({ status: 413 });
      request.resume();
      return;
    }
    const chunks = [];
    let size = 0;
    let settled = false;

    request.on('data', (chunk) => {
      if (settled) return;
      size += chunk.length;
      if (size > limit) {
        settled = true;
        reject({ status: 413 });
        request.resume();
        return;
      }
      chunks.push(chunk);
    });
    request.on('end', () => {
      if (settled) return;
      settled = true;
      resolve(Buffer.concat(chunks));
    });
    request.on('aborted', () => {
      if (settled) return;
      settled = true;
      reject({ status: 400 });
    });
    request.on('error', () => {
      if (settled) return;
      settled = true;
      reject({ status: 400 });
    });
  });
}

function respond(response, status, body) {
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    'cache-control': 'no-store',
    'content-type': 'application/json; charset=utf-8',
    'content-length': Buffer.byteLength(payload),
    'x-content-type-options': 'nosniff',
  });
  response.end(payload);
}

function headerValue(value) {
  if (Array.isArray(value)) return value[0];
  return value;
}

function validSubject(subject) {
  return (
    typeof subject === 'string' &&
    subject.length > 0 &&
    subject.length <= 255 &&
    !/[\s\u0000*>]/.test(subject) &&
    !subject.startsWith('.') &&
    !subject.endsWith('.') &&
    !subject.includes('..')
  );
}

function secretsEqual(presented, expected) {
  if (typeof presented !== 'string') return false;
  const left = Buffer.from(presented, 'utf8');
  const right = Buffer.from(expected, 'utf8');
  return left.length === right.length && timingSafeEqual(left, right);
}

function log(level, event, fields = {}) {
  const method = typeof console[level] === 'function' ? level : 'log';
  console[method](
    JSON.stringify({
      timestamp: new Date().toISOString(),
      service: 'dd-nats-bridge',
      event,
      ...fields,
    }),
  );
}

function shutdown() {
  log('info', 'shutdown_started');
  nats.destroy();
  publishServer.close((error) => {
    if (error) {
      log('error', 'shutdown_failed', { error: error.message });
      process.exitCode = 1;
    }
  });
}
