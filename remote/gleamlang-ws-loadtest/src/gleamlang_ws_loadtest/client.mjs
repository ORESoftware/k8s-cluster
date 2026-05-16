import WebSocket from "ws";

const DEFAULT_WS_URL = "ws://dd-gleamlang-server.default.svc.cluster.local:8081/ws";

function parsePositiveInt(name, fallback) {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

export function run() {
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
