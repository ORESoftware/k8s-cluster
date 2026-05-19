import { createServer } from 'node:http';

import { getNatsClient } from './nats-client.mjs';

const natsUrl = process.env.NATS_URL ?? 'nats://dd-nats.messaging.svc.cluster.local:4222';
const readSubject =
  process.env.NATS_READ_SUBJECT ?? process.env.NATS_EVENT_SUBJECT ?? 'dd.remote.events';
const publishSubject = process.env.NATS_PUBLISH_SUBJECT ?? 'dd.remote.websocket.events';
const broadcastUrl = process.env.GLEAM_BROADCAST_URL ?? 'http://127.0.0.1:8081/broadcast';
const broadcastSecret = requiredEnv('GLEAM_BROADCAST_SECRET');
const bridgePort = numberEnv('NATS_BRIDGE_HTTP_PORT', 8083);
const maxBodyBytes = numberEnv('NATS_BRIDGE_MAX_BODY_BYTES', 1_048_576);
const dedupeTtlMs = numberEnv('NATS_BRIDGE_DEDUPE_TTL_MS', 5 * 60 * 1000);
const seenMessageIds = new Map();

const nats = getNatsClient({ url: natsUrl, logger: console });
nats.subscribe(readSubject, (payload) => {
  if (dropDuplicate(payload)) {
    return;
  }
  void broadcast(payload);
});
console.info(`[nats-bridge] subscribed ${readSubject}`);

startPublishServer();

function requiredEnv(name) {
  const value = process.env[name];
  if (!value?.trim()) {
    throw new Error(`${name} must be configured`);
  }
  return value;
}

function numberEnv(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number(raw);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

async function broadcast(payload) {
  try {
    const response = await fetch(broadcastUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-dd-internal-auth': broadcastSecret,
      },
      body: payload,
    });
    if (!response.ok) {
      console.warn(`[nats-bridge] broadcast failed ${response.status}`);
    }
  } catch (error) {
    console.warn(
      `[nats-bridge] broadcast error: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

function dropDuplicate(payload) {
  const messageId = extractMessageId(payload);
  if (!messageId) return false;
  const now = Date.now();
  pruneSeenMessageIds(now);
  const expiresAt = seenMessageIds.get(messageId);
  if (expiresAt && expiresAt > now) {
    return true;
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

function extractMessageId(payload) {
  try {
    const parsed = JSON.parse(Buffer.isBuffer(payload) ? payload.toString('utf8') : String(payload));
    const candidate = parsed?.messageId ?? parsed?.message_id ?? parsed?.id;
    return typeof candidate === 'string' && candidate.length <= 128 ? candidate : null;
  } catch {
    return null;
  }
}

function startPublishServer() {
  const server = createServer((request, response) => {
    void handlePublishRequest(request, response);
  });
  server.listen(bridgePort, '127.0.0.1', () => {
    console.info(
      `[nats-bridge] publish endpoint listening on 127.0.0.1:${bridgePort}/publish`,
    );
  });
}

async function handlePublishRequest(request, response) {
  const url = new URL(request.url ?? '/', 'http://127.0.0.1');

  if (request.method === 'GET' && url.pathname === '/healthz') {
    respond(response, 200, { ok: true, readSubject, publishSubject });
    return;
  }

  if (request.method !== 'POST' || url.pathname !== '/publish') {
    respond(response, 404, { error: 'not-found' });
    return;
  }

  if (headerValue(request.headers['x-dd-internal-auth']) !== broadcastSecret) {
    respond(response, 401, { error: 'unauthorized' });
    return;
  }

  const subject =
    headerValue(request.headers['x-nats-subject']) ??
    url.searchParams.get('subject') ??
    publishSubject;
  if (!validSubject(subject)) {
    respond(response, 400, { error: 'invalid-subject' });
    return;
  }

  try {
    const body = await readBody(request, maxBodyBytes);
    nats.publish(subject, body);
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
    const chunks = [];
    let size = 0;

    request.on('data', (chunk) => {
      size += chunk.length;
      if (size > limit) {
        reject({ status: 413 });
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on('end', () => {
      resolve(Buffer.concat(chunks));
    });
    request.on('error', () => {
      reject({ status: 400 });
    });
  });
}

function respond(response, status, body) {
  response.writeHead(status, { 'content-type': 'application/json' });
  response.end(JSON.stringify(body));
}

function headerValue(value) {
  if (Array.isArray(value)) return value[0];
  return value;
}

function validSubject(subject) {
  return typeof subject === 'string' && /^[^\s]+$/.test(subject);
}
