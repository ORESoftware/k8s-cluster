import WebSocket from "ws";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";

const DEFAULT_WS_URL = "ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws";
const LOAD_MODE_HOLD = "hold";
const LOAD_MODE_PIPELINE = "pipeline";
// gcs mode drives the chat.vibe Go server (via gcs-router) using its real chat
// protocol instead of the akka-style echo frames. See connectGcsClient below.
const LOAD_MODE_GCS = "gcs";

function parsePositiveInt(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function parsePositiveFloat(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

export function run() {
  if (process.env.CONTAINER_POOL_URL) {
    runContainerPoolSmoke().catch((error) => {
      console.error(`gleamlang-container-pool-smoke failed: ${error?.stack || error}`);
      process.exitCode = 1;
    });
    return;
  }

  const targetWsUrl = process.env.TARGET_WS_URL || DEFAULT_WS_URL;
  const clientCount = parsePositiveInt("CLIENT_COUNT", 5_000);
  const holdSeconds = parsePositiveInt("HOLD_SECONDS", 300);
  const connectTimeoutMs = parsePositiveInt("CONNECT_TIMEOUT_MS", 20_000);
  const reconnectDelayMs = parsePositiveInt("RECONNECT_DELAY_MS", 1_000);
  const rampDelayMs = parsePositiveInt("RAMP_DELAY_MS", 1);
  const reportIntervalSeconds = parsePositiveInt("REPORT_INTERVAL_SECONDS", 10);
  const loadMode = process.env.LOAD_MODE || LOAD_MODE_HOLD;
  const messagesPerSecondPerClient = parsePositiveFloat("MESSAGES_PER_SECOND_PER_CLIENT", 10.0);
  const messagePayload = process.env.MESSAGE_PAYLOAD || "a benchmark message body";
  const correlationTimeoutMs = parsePositiveInt("CORRELATION_TIMEOUT_MS", 10_000);
  // gcs-mode: clients per conversation (fan-out factor + conv-hash grouping)
  // and how many of them send (0/unset => all send).
  const gcsClientsPerConv = parsePositiveInt("GCS_CLIENTS_PER_CONV", 5);
  const gcsSendersPerConvRaw = Number.parseInt(process.env.GCS_SENDERS_PER_CONV || "0", 10);
  const gcsSendersPerConv =
    Number.isFinite(gcsSendersPerConvRaw) && gcsSendersPerConvRaw > 0
      ? Math.min(gcsSendersPerConvRaw, gcsClientsPerConv)
      : gcsClientsPerConv;

  let attempted = 0;
  let connected = 0;
  let failed = 0;
  let open = 0;
  let messages = 0;
  // Pipeline-mode counters.
  let sent = 0;
  let received = 0;
  let receiveErrors = 0;
  let correlationMisses = 0;
  let inFlightTotal = 0;
  /** @type {number[]} */
  const latenciesUs = [];

  console.log(
    [
      "gleamlang-ws-loadtest starting",
      `target_ws_url=${targetWsUrl}`,
      `client_count=${clientCount}`,
      `load_mode=${loadMode}`,
      `hold_seconds=${holdSeconds}`,
      `connect_timeout_ms=${connectTimeoutMs}`,
      `reconnect_delay_ms=${reconnectDelayMs}`,
      `ramp_delay_ms=${rampDelayMs}`,
      `report_interval_seconds=${reportIntervalSeconds}`,
      `messages_per_second_per_client=${messagesPerSecondPerClient}`,
      `correlation_timeout_ms=${correlationTimeoutMs}`,
    ].join(" "),
  );

  const holdMs = holdSeconds * 1000;
  const sendIntervalMs = 1000 / messagesPerSecondPerClient;

  function connectHoldClient(clientId) {
    attempted += 1;
    const socket = new WebSocket(targetWsUrl, {
      perMessageDeflate: false,
      handshakeTimeout: connectTimeoutMs,
    });

    let opened = false;
    let holdTimer = null;

    socket.on("open", () => {
      opened = true;
      connected += 1;
      open += 1;
      socket.send(`ping-gleam-${clientId}`);
      holdTimer = setTimeout(() => {
        try {
          socket.close();
        } catch {
          // no-op
        }
      }, holdMs);
    });

    socket.on("message", () => {
      messages += 1;
    });

    socket.on("error", (_error) => {
      failed += 1;
    });

    socket.on("close", () => {
      if (holdTimer) {
        clearTimeout(holdTimer);
      }
      if (opened) {
        open = Math.max(0, open - 1);
      }
      setTimeout(() => connectHoldClient(clientId), reconnectDelayMs);
    });
  }

  /**
   * Pipeline-mode client: sends shaped JSON messages at a fixed per-client rate and correlates
   * the server's responses back to per-id send timestamps so we can measure end-to-end
   * round-trip latency.
   */
  function connectPipelineClient(clientId) {
    attempted += 1;
    const socket = new WebSocket(targetWsUrl, {
      perMessageDeflate: false,
      handshakeTimeout: connectTimeoutMs,
    });

    let opened = false;
    /** @type {Map<string, number>} */
    const pending = new Map();
    let seq = 0;
    let sendTimer = null;
    let sweepTimer = null;

    socket.on("open", () => {
      opened = true;
      connected += 1;
      open += 1;

      sendTimer = setInterval(() => {
        seq += 1;
        const id = `c${clientId}-${seq}`;
        const escapedPayload = messagePayload.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
        const frame = `{"id":"${id}","payload":"${escapedPayload}"}`;
        pending.set(id, performance.now());
        inFlightTotal = Math.max(inFlightTotal, pending.size);
        try {
          socket.send(frame);
          sent += 1;
        } catch (_error) {
          receiveErrors += 1;
        }
      }, sendIntervalMs);

      // Drop very-old pending entries to bound memory if the server slows down.
      sweepTimer = setInterval(() => {
        const cutoff = performance.now() - correlationTimeoutMs;
        for (const [id, sentAt] of pending) {
          if (sentAt < cutoff) pending.delete(id);
        }
      }, Math.max(1000, correlationTimeoutMs / 2));
    });

    socket.on("message", (data) => {
      messages += 1;
      const text = typeof data === "string" ? data : data.toString();
      const id = extractId(text);
      if (id == null) {
        correlationMisses += 1;
        return;
      }
      const sentAt = pending.get(id);
      if (sentAt == null) {
        correlationMisses += 1;
        return;
      }
      pending.delete(id);
      const latencyUs = Math.max(0, Math.round((performance.now() - sentAt) * 1000));
      received += 1;
      latenciesUs.push(latencyUs);
    });

    socket.on("error", (_error) => {
      failed += 1;
    });

    socket.on("close", () => {
      if (sendTimer) clearInterval(sendTimer);
      if (sweepTimer) clearInterval(sweepTimer);
      if (opened) open = Math.max(0, open - 1);
      pending.clear();
      setTimeout(() => connectPipelineClient(clientId), reconnectDelayMs);
    });
  }

  // gcs-mode setup: deterministic conversations/members so each conversation's
  // clients hash to the same gcs pod (conv-hash) and fan out to one another.
  const convCount = Math.ceil(clientCount / gcsClientsPerConv);
  /** @type {string[]} */
  const convIds = [];
  /** @type {string[][]} */
  const convMembers = [];
  if (loadMode === LOAD_MODE_GCS) {
    for (let c = 0; c < convCount; c += 1) {
      convIds.push(objectId());
      convMembers.push([]);
    }
    for (let i = 0; i < clientCount; i += 1) {
      convMembers[Math.floor(i / gcsClientsPerConv)].push(objectId());
    }
    console.log(
      `gleamlang-ws-loadtest gcs-setup conversations=${convCount} ` +
        `clients_per_conv=${gcsClientsPerConv} senders_per_conv=${gcsSendersPerConv}`,
    );
  }

  function connectGcsClient(clientId) {
    attempted += 1;
    const c = Math.floor(clientId / gcsClientsPerConv);
    const idx = clientId % gcsClientsPerConv;
    const convId = convIds[c];
    const userId = convMembers[c][idx];
    const deviceId = objectId();
    const members = convMembers[c];
    const isSender = idx < gcsSendersPerConv;
    const url = gcsConnectUrl(targetWsUrl, userId, deviceId, convId);

    const socket = new WebSocket(url, {
      perMessageDeflate: false,
      handshakeTimeout: connectTimeoutMs,
    });

    let opened = false;
    let sendTimer = null;
    let seq = 0;

    socket.on("open", () => {
      opened = true;
      connected += 1;
      open += 1;
      if (isSender) {
        sendTimer = setInterval(() => {
          seq += 1;
          const marker = `gcsrt-${clientId}-${seq}-${Math.round(performance.now() * 1000)}`;
          const frame = buildGcsFrame(convId, userId, members, marker);
          try {
            socket.send(frame);
            sent += 1;
          } catch (_error) {
            receiveErrors += 1;
          }
        }, sendIntervalMs);
      }
    });

    socket.on("message", (data) => {
      messages += 1;
      const text = typeof data === "string" ? data : data.toString();
      const now = Math.round(performance.now() * 1000);
      for (const sendUs of parseGcsSendMicros(text)) {
        received += 1;
        latenciesUs.push(Math.max(0, now - sendUs));
      }
    });

    socket.on("error", (_error) => {
      failed += 1;
    });

    socket.on("close", () => {
      if (sendTimer) clearInterval(sendTimer);
      if (opened) open = Math.max(0, open - 1);
      setTimeout(() => connectGcsClient(clientId), reconnectDelayMs);
    });
  }

  const connect =
    loadMode === LOAD_MODE_PIPELINE
      ? connectPipelineClient
      : loadMode === LOAD_MODE_GCS
        ? connectGcsClient
        : connectHoldClient;
  for (let clientId = 0; clientId < clientCount; clientId += 1) {
    setTimeout(() => connect(clientId), clientId * rampDelayMs);
  }

  setInterval(() => {
    // pipeline and gcs both produce per-message latency samples; in gcs mode
    // received/sent approximates conversation fan-out.
    if (loadMode === LOAD_MODE_PIPELINE || loadMode === LOAD_MODE_GCS) {
      const p = percentiles(latenciesUs);
      console.log(
        `gleamlang-ws-loadtest ${loadMode}-report attempted=${attempted} connected=${connected} ` +
          `failed=${failed} open=${open} sent=${sent} received=${received} ` +
          `in_flight_peak=${inFlightTotal} correlation_misses=${correlationMisses} ` +
          `receive_errors=${receiveErrors} p50_us=${p.p50} p95_us=${p.p95} p99_us=${p.p99} ` +
          `max_us=${p.max} mean_us=${p.mean} sample=${latenciesUs.length}`,
      );
    } else {
      console.log(
        `gleamlang-ws-loadtest report attempted=${attempted} connected=${connected} failed=${failed} open=${open} messages=${messages}`,
      );
    }
  }, reportIntervalSeconds * 1000);
}

/** Cheap-and-cheerful id extraction; avoids JSON.parse on every frame on the hot path. */
function extractId(frame) {
  const needle = '"id":"';
  const start = frame.indexOf(needle);
  if (start < 0) return null;
  const begin = start + needle.length;
  const end = frame.indexOf('"', begin);
  if (end < 0) return null;
  return frame.slice(begin, end);
}

/**
 * Reports p50/p95/p99/max/mean over the latency buffer. Quadratic-free: we sort a single
 * pass per report. The Map<id, sentAt> already bounds memory; the latency array grows
 * linearly with sample size so for very long runs the harness may want a windowed buffer.
 */
function percentiles(samples) {
  if (samples.length === 0) {
    return { p50: 0, p95: 0, p99: 0, max: 0, mean: 0 };
  }
  const sorted = samples.slice().sort((a, b) => a - b);
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)] | 0;
  const sum = samples.reduce((acc, v) => acc + v, 0);
  return {
    p50: at(0.5),
    p95: at(0.95),
    p99: at(0.99),
    max: sorted[sorted.length - 1] | 0,
    mean: Math.round(sum / samples.length),
  };
}

// --- gcs (chat.vibe) helpers -------------------------------------------------

/** 24-hex-char Mongo ObjectId, matching the working chat.vibe test scripts. */
function objectId() {
  const ts = Math.floor(Date.now() / 1000)
    .toString(16)
    .padStart(8, "0");
  const r1 = Math.floor(Math.random() * 0xffffff)
    .toString(16)
    .padStart(6, "0");
  const r2 = Math.floor(Math.random() * 0xffff)
    .toString(16)
    .padStart(4, "0");
  const r3 = Math.floor(Math.random() * 0xffffff)
    .toString(16)
    .padStart(6, "0");
  return ts + r1 + r2 + r3;
}

/** chat.vibe connect URL: query params carry the subscription (no auth in-cluster). */
function gcsConnectUrl(base, userId, deviceId, convId) {
  const b = base.replace(/\/$/, "");
  const convIds = encodeURIComponent(JSON.stringify([convId]));
  return `${b}/gcs/ws/?userId=${userId}&deviceId=${deviceId}&conversationIds=${convIds}`;
}

/** Outer envelope holding one MongoChatMessage; @vibe-data is a JSON string. */
function buildGcsFrame(convId, userId, members, marker) {
  const now = new Date().toISOString();
  const inner = JSON.stringify({
    _id: objectId(),
    IsGroupChat: members.length > 2,
    PriorityUserIds: members,
    CreatedByUserId: userId,
    CreatedBy: userId,
    CreatedAt: now,
    ChatId: convId,
    Messages: [marker],
    DateCreatedOnDevice: now,
    DateFirstOnServer: now,
  });
  return JSON.stringify({
    Meta: {},
    List: [{ "@vibe-meta": {}, "@vibe-type": "MongoChatMessage", "@vibe-data": inner }],
  });
}

const GCS_MARKER_RE = /gcsrt-\d+-\d+-(\d+)/g;

/** Extract the embedded send-time (µs) from every `gcsrt-` marker in a frame. */
function parseGcsSendMicros(text) {
  const out = [];
  GCS_MARKER_RE.lastIndex = 0;
  let match;
  while ((match = GCS_MARKER_RE.exec(text)) !== null) {
    const value = Number.parseInt(match[1], 10);
    if (Number.isFinite(value)) out.push(value);
  }
  return out;
}

function containerPoolDispatchUrl() {
  const baseUrl = process.env.CONTAINER_POOL_URL.replace(/\/+$/, "");
  const routePrefix = process.env.CONTAINER_POOL_ROUTE_PREFIX || "/pools";
  const pool = process.env.CONTAINER_POOL_POOL || "gleamlang";
  return `${baseUrl}${routePrefix}/${encodeURIComponent(pool)}/dispatch`;
}

async function runContainerPoolSmoke() {
  const timeoutMs = parsePositiveInt("CONTAINER_POOL_TIMEOUT_MS", 30_000);
  const echoKey = process.env.CONTAINER_POOL_ECHO_KEY || randomUUID();
  const url = containerPoolDispatchUrl();
  const headers = {
    "content-type": "application/json",
  };
  if (process.env.CONTAINER_POOL_AUTH_SECRET) {
    headers["x-server-auth"] = process.env.CONTAINER_POOL_AUTH_SECRET;
  }

  console.log(
    `gleamlang-container-pool-smoke starting url=${url} echo_key=${echoKey} timeout_ms=${timeoutMs}`,
  );
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      method: "POST",
      headers,
      signal: controller.signal,
      body: JSON.stringify({
        requestId: echoKey,
        payload: {
          echoKey,
          client: "gleamlang-ws-loadtest",
        },
      }),
    });
    const body = await response.json();
    const returnedKey = body?.body?.echoKey ?? body?.body?.request?.echoKey;
    if (!response.ok || returnedKey !== echoKey) {
      throw new Error(
        `unexpected container-pool response status=${response.status} returned_key=${returnedKey} body=${JSON.stringify(body)}`,
      );
    }
    console.log(
      `gleamlang-container-pool-smoke ok pool=${body.poolSlug} container=${body.containerName} echo_key=${returnedKey}`,
    );
  } finally {
    clearTimeout(timer);
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  run();
}
