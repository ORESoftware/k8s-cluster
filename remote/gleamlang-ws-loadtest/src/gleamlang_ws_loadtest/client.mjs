import WebSocket from "ws";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";

const DEFAULT_WS_URL = "ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws";

function parsePositiveInt(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number.parseInt(raw, 10);
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

  let attempted = 0;
  let connected = 0;
  let failed = 0;
  let open = 0;
  let messages = 0;

  console.log(
    [
      "gleamlang-ws-loadtest starting",
      `target_ws_url=${targetWsUrl}`,
      `client_count=${clientCount}`,
      `hold_seconds=${holdSeconds}`,
      `connect_timeout_ms=${connectTimeoutMs}`,
      `reconnect_delay_ms=${reconnectDelayMs}`,
      `ramp_delay_ms=${rampDelayMs}`,
      `report_interval_seconds=${reportIntervalSeconds}`,
    ].join(" "),
  );

  const holdMs = holdSeconds * 1000;

  function connectClient(clientId) {
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
      setTimeout(() => connectClient(clientId), reconnectDelayMs);
    });
  }

  for (let clientId = 0; clientId < clientCount; clientId += 1) {
    setTimeout(() => connectClient(clientId), clientId * rampDelayMs);
  }

  setInterval(() => {
    console.log(
      `gleamlang-ws-loadtest report attempted=${attempted} connected=${connected} failed=${failed} open=${open} messages=${messages}`,
    );
  }, reportIntervalSeconds * 1000);
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
