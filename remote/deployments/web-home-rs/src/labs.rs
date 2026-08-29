pub(crate) const WSS_TEST_CSS: &str = r###":root {
  color-scheme: dark;
  --bg: #0b1117;
  --panel: #111923;
  --panel-2: #0f1720;
  --field: #0e1520;
  --field-2: #0a1017;
  --line: rgba(148, 163, 184, 0.24);
  --text: #eef2f6;
  --muted: #a8b3c1;
  --accent: #5eead4;
  --warn: #fbbf24;
  --danger: #fb7185;
  --ok: #86efac;
}
* { box-sizing: border-box; }
[hidden] { display: none !important; }
body {
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
a { color: inherit; text-decoration: none; }
header {
  position: sticky;
  top: 0;
  z-index: 10;
  display: grid;
  gap: 10px;
  padding: 12px 16px;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
}
.topline {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
h1 { font-size: 18px; margin: 0 12px 0 0; }
label { display: grid; gap: 3px; color: var(--muted); font-size: 11px; }
input, select, textarea, button {
  background: var(--field);
  color: var(--text);
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 6px 8px;
  font: inherit;
}
input:focus, select:focus, textarea:focus, button:focus { outline: 1px solid var(--accent); }
button { cursor: pointer; background: var(--panel-2); }
button:hover { background: #182032; }
button:disabled {
  cursor: not-allowed;
  opacity: 0.46;
  color: var(--muted);
  border-color: var(--line);
}
button:disabled:hover { background: var(--panel-2); }
button.primary { border-color: var(--accent); color: var(--accent); }
button.danger { border-color: var(--danger); color: var(--danger); }
#base { width: min(36vw, 320px); }
#path { width: min(44vw, 460px); }
.pill {
  display: inline-flex;
  align-items: center;
  min-height: 24px;
  padding: 2px 8px;
  border-radius: 999px;
  color: var(--accent);
  border: 1px solid rgba(94, 234, 212, 0.35);
  background: rgba(94, 234, 212, 0.08);
  font-size: 12px;
}
.pill.warn { color: var(--warn); border-color: rgba(251, 191, 36, 0.35); background: rgba(251, 191, 36, 0.08); }
.pill.bad { color: var(--danger); border-color: rgba(251, 113, 133, 0.35); background: rgba(251, 113, 133, 0.08); }
.pill.ok { color: var(--ok); border-color: rgba(134, 239, 172, 0.35); background: rgba(134, 239, 172, 0.08); }
.stats {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.url-strip {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  align-items: center;
  gap: 8px;
  color: var(--muted);
  font-size: 11px;
}
#url-preview {
  display: block;
  min-width: 0;
  overflow-x: auto;
  white-space: nowrap;
}
#url-preview.bad { color: var(--danger); }
.metrics {
  display: grid;
  grid-template-columns: repeat(5, minmax(112px, 1fr));
  gap: 8px;
}
.metric {
  min-width: 0;
  display: grid;
  gap: 2px;
  padding: 7px 9px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: rgba(15, 23, 32, 0.72);
}
.metric strong {
  color: var(--muted);
  font-size: 10px;
  font-weight: 600;
  text-transform: uppercase;
}
.metric span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text);
}
.grid {
  display: grid;
  grid-template-columns: minmax(320px, 0.78fr) minmax(420px, 1.22fr);
  gap: 14px;
  padding: 16px;
  align-items: start;
}
.panel {
  min-width: 0;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  overflow: hidden;
}
.panel.full { grid-column: 1 / -1; }
.panel-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 12px;
  background: var(--panel-2);
  border-bottom: 1px solid var(--line);
}
.panel h2 {
  margin: 0;
  font-size: 13px;
}
.panel > h2 {
  padding: 10px 12px;
  background: var(--panel-2);
  border-bottom: 1px solid var(--line);
}
.panel-body { display: grid; gap: 12px; padding: 12px; }
.fields { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.field-wide { grid-column: 1 / -1; }
.actions { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
.quick-links code { color: #d7fbf4; }
.detail {
  display: block;
  min-height: 30px;
  padding: 7px 9px;
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--muted);
  background: var(--field-2);
  overflow-wrap: anywhere;
}
.payload-tools {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  flex-wrap: wrap;
}
.checkline {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.checkline input { width: auto; margin: 0; }
.log-tools {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: center;
  justify-content: flex-end;
}
.log-tools select { min-width: 108px; }
textarea {
  width: 100%;
  min-height: 248px;
  resize: vertical;
  line-height: 1.45;
}
.log {
  margin: 0;
  min-height: 432px;
  max-height: calc(100vh - 248px);
  overflow: auto;
  padding: 8px 10px 12px;
  background: #090f16;
  color: var(--text);
  white-space: pre-wrap;
  word-break: break-word;
}
.row {
  display: grid;
  grid-template-columns: 116px 42px minmax(0, 1fr);
  gap: 8px;
  padding: 3px 0;
  border-bottom: 1px solid rgba(148, 163, 184, 0.08);
}
.row.in { color: var(--ok); }
.row.out { color: var(--accent); }
.row.warn { color: var(--warn); }
.row.bad { color: var(--danger); }
.row.meta { color: var(--muted); }
.ts {
  color: var(--muted);
  white-space: nowrap;
}
.dir {
  color: var(--text);
  opacity: 0.72;
  text-transform: uppercase;
  white-space: nowrap;
}
.msg {
  min-width: 0;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}
code {
  display: inline-block;
  max-width: 100%;
  overflow-wrap: anywhere;
  border: 1px solid rgba(148, 163, 184, 0.2);
  border-radius: 6px;
  padding: 1px 5px;
  background: #0a1017;
  color: #d7fbf4;
}
@media (max-width: 860px) {
  .grid { grid-template-columns: 1fr; }
  .metrics { grid-template-columns: 1fr 1fr; }
  .fields { grid-template-columns: 1fr; }
  #base, #path { width: 100%; }
  .log { min-height: 320px; max-height: none; }
}
@media (max-width: 560px) {
  .metrics { grid-template-columns: 1fr; }
  .row { grid-template-columns: 104px 36px minmax(0, 1fr); }
}
"###;

pub(crate) const WSS_TEST_BODY: &str = r###"<header>
  <div class="topline">
    <h1>websocket lab</h1>
    <label>preset
      <select id="preset">
        <option value="gleam">Gleam fan-out</option>
        <option value="webrtc">Rust WebRTC signaling</option>
        <option value="gcs">gms/gcs/chat.vibe router</option>
        <option value="fsrx">F# Rx burst</option>
      </select>
    </label>
    <label>base
      <input id="base" placeholder="same origin" />
    </label>
    <label>path
      <input id="path" />
    </label>
    <span id="status" class="pill warn">idle</span>
    <span id="health-pill" class="pill warn">health unchecked</span>
    <span id="counter" class="pill">0 frames</span>
    <span id="sent-counter" class="pill">0 sent</span>
    <span id="recv-counter" class="pill">0 recv</span>
  </div>
  <div class="url-strip">
    <span>target</span>
    <code id="url-preview">ws://...</code>
  </div>
  <div class="metrics">
    <div class="metric"><strong>ready state</strong><span id="ready-state">closed</span></div>
    <div class="metric"><strong>latency</strong><span id="latency">-</span></div>
    <div class="metric"><strong>uptime</strong><span id="uptime">-</span></div>
    <div class="metric"><strong>interval</strong><span id="interval-state">stopped</span></div>
    <div class="metric"><strong>last event</strong><span id="last-event">idle</span></div>
  </div>
</header>

<main class="grid">
  <section class="panel">
    <h2>connection</h2>
    <div class="panel-body">
      <div class="fields">
        <label class="preset-field" data-presets="gleam">thread id<input id="thread-id" /></label>
        <label class="preset-field" data-presets="gleam">task id<input id="task-id" /></label>
        <label class="preset-field" data-presets="webrtc">room<input id="room-id" /></label>
        <label class="preset-field" data-presets="webrtc">peer<input id="peer-id" /></label>
        <label class="preset-field" data-presets="gcs fsrx">user id<input id="user-id" /></label>
        <label class="preset-field" data-presets="gcs fsrx">device id<input id="device-id" /></label>
        <label class="preset-field" data-presets="gcs fsrx">conversation id<input id="conv-id" /></label>
        <label>burst count<input id="burst-count" type="number" min="1" max="500" value="12" /></label>
        <label>interval ms<input id="interval-ms" type="number" min="50" max="60000" value="1000" /></label>
        <label class="preset-field" data-presets="gcs">gcs route
          <select id="gcs-route">
            <option value="conv">conv</option>
            <option value="user">user</option>
            <option value="device">device</option>
          </select>
        </label>
      </div>
      <div class="actions">
        <button id="connect" class="primary" type="button">connect</button>
        <button id="disconnect" class="danger" type="button">disconnect</button>
        <button id="copy-url" type="button">copy url</button>
        <button id="check-health" type="button">health</button>
        <button id="clear" type="button">clear</button>
      </div>
      <output id="health-detail" class="detail">health unchecked</output>
      <div class="actions quick-links">
        <a href="/presence-test?user=alice&amp;device=d1&amp;autoconnect=1"><code>/presence-test</code></a>
        <a href="/gleam/home"><code>/gleam/home</code></a>
        <a href="/webrtc/"><code>/webrtc/</code></a>
        <a href="/gcs/ws-health"><code>/gcs/ws-health</code></a>
        <a href="/wss-test?preset=fsrx"><code>/fsws/ws/rx-burst</code></a>
      </div>
    </div>
  </section>

  <section class="panel">
    <h2>frames</h2>
    <div class="panel-body">
      <textarea id="payload" spellcheck="false"></textarea>
      <div class="payload-tools">
        <div class="actions">
          <button id="format-payload" type="button">format JSON</button>
          <button id="compact-payload" type="button">compact JSON</button>
          <button id="copy-payload" type="button">copy payload</button>
        </div>
      </div>
      <div class="actions">
        <button id="send" class="primary" type="button">send</button>
        <button id="send-ping" type="button">ping</button>
        <button id="send-hello" type="button">hello</button>
        <button id="send-sample" type="button">sample</button>
        <button id="send-burst" type="button">burst</button>
        <button id="start-interval" type="button">start interval</button>
        <button id="stop-interval" type="button">stop interval</button>
      </div>
    </div>
  </section>

  <section class="panel full">
    <div class="panel-title">
      <h2>log</h2>
      <div class="log-tools">
        <label>filter
          <select id="log-filter">
            <option value="all">all</option>
            <option value="in">in</option>
            <option value="out">out</option>
            <option value="meta">meta</option>
            <option value="warn">warn</option>
            <option value="bad">bad</option>
          </select>
        </label>
        <label class="checkline"><input id="autoscroll" type="checkbox" checked />autoscroll</label>
        <button id="copy-log" type="button">copy log</button>
      </div>
    </div>
    <pre id="log" class="log"></pre>
  </section>
</main>"###;

pub(crate) const WSS_TEST_JS: &str = r###"const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
const defaults = {
  threadId: "00000000-0000-4000-8000-000000000001",
  taskId: "00000000-0000-4000-8000-000000000002",
  roomId: "browser-room",
  peerId: "peer-" + Math.random().toString(16).slice(2, 8),
  userId: "65c48f2f47d56fec05a41b38",
  deviceId: "65c48f2f47d56fec05a41b39",
  convId: "65c48f2f47d56fec05a41b3a",
};
const presets = ["gleam", "webrtc", "gcs", "fsrx"];
const sendControlIds = ["send", "send-ping", "send-hello", "send-sample", "send-burst", "start-interval"];
const state = {
  ws: null,
  frames: 0,
  sent: 0,
  received: 0,
  intervalTimer: null,
  uptimeTimer: null,
  openedAt: 0,
  connectStartedAt: 0,
  lastSentAt: 0,
  latencyMs: null,
  lastEvent: "idle",
};

function sameOriginWsBase() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${location.host}`;
}
function httpToWs(value) {
  return value.replace(/^http:\/\//i, "ws://").replace(/^https:\/\//i, "wss://");
}
function wsToHttp(value) {
  return value.replace(/^ws:\/\//i, "http://").replace(/^wss:\/\//i, "https://");
}
function trimSlash(value) {
  return value.replace(/\/+$/, "");
}
function ensureLeadingSlash(value) {
  return value.startsWith("/") ? value : "/" + value;
}
function normalizeWsBase(value) {
  let raw = value.trim() || sameOriginWsBase();
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(raw)) {
    raw = `${location.protocol === "https:" ? "wss" : "ws"}://${raw}`;
  }
  return trimSlash(httpToWs(raw));
}
function ts() {
  const d = new Date();
  return d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0");
}
function rowKindLabel(kind) {
  return ({ in: "in", out: "out", meta: "meta", warn: "warn", bad: "bad" })[kind] || kind;
}
function shouldShowRow(row) {
  const filter = $("log-filter").value;
  return filter === "all" || row.dataset.kind === filter;
}
function applyLogFilter() {
  for (const row of $("log").children) row.hidden = !shouldShowRow(row);
}
function setLastEvent(text) {
  state.lastEvent = String(text || "idle").replace(/\s+/g, " ").slice(0, 96);
  $("last-event").textContent = state.lastEvent;
}
function log(text, kind = "meta") {
  const row = document.createElement("div");
  row.className = "row " + kind;
  row.dataset.kind = kind;
  const stamp = document.createElement("span");
  stamp.className = "ts";
  stamp.textContent = ts();
  const dir = document.createElement("span");
  dir.className = "dir";
  dir.textContent = rowKindLabel(kind);
  const msg = document.createElement("span");
  msg.className = "msg";
  msg.textContent = String(text);
  row.dataset.copy = `${stamp.textContent} ${dir.textContent} ${msg.textContent}`;
  row.append(stamp, dir, msg);
  $("log").appendChild(row);
  while ($("log").childNodes.length > 600) $("log").removeChild($("log").firstChild);
  row.hidden = !shouldShowRow(row);
  if ($("autoscroll").checked) $("log").scrollTop = $("log").scrollHeight;
  setLastEvent(`${dir.textContent} ${msg.textContent}`);
}
function formatDuration(ms) {
  if (!ms || ms < 0) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  if (minutes < 60) return `${minutes}m ${remainder}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}
function readyStateText() {
  if (!state.ws) return "closed";
  return ["connecting", "open", "closing", "closed"][state.ws.readyState] || "unknown";
}
function refreshMetrics() {
  $("ready-state").textContent = readyStateText();
  $("latency").textContent = state.latencyMs === null ? "-" : `${state.latencyMs} ms`;
  $("uptime").textContent = state.openedAt ? formatDuration(Date.now() - state.openedAt) : "-";
  $("interval-state").textContent = state.intervalTimer === null ? "stopped" : "running";
  $("last-event").textContent = state.lastEvent;
}
function startUptimeTimer() {
  stopUptimeTimer();
  state.openedAt = Date.now();
  state.uptimeTimer = setInterval(refreshMetrics, 1000);
  refreshMetrics();
}
function stopUptimeTimer() {
  if (state.uptimeTimer !== null) clearInterval(state.uptimeTimer);
  state.uptimeTimer = null;
  state.openedAt = 0;
  refreshMetrics();
}
function setDisabled(id, disabled) {
  const el = $(id);
  if (el) el.disabled = disabled;
}
function updateControls() {
  const ready = state.ws ? state.ws.readyState : WebSocket.CLOSED;
  const isOpen = ready === WebSocket.OPEN;
  setDisabled("connect", ready === WebSocket.CONNECTING || isOpen);
  setDisabled("disconnect", !state.ws || ready === WebSocket.CLOSED);
  for (const id of sendControlIds) setDisabled(id, !isOpen);
  setDisabled("start-interval", !isOpen || state.intervalTimer !== null);
  setDisabled("stop-interval", state.intervalTimer === null);
  refreshMetrics();
}
function setStatus(text, cls = "warn") {
  $("status").textContent = text;
  $("status").className = "pill " + cls;
  setLastEvent(text);
  updateControls();
}
function setHealth(text, cls = "warn", detail = text) {
  $("health-pill").textContent = text;
  $("health-pill").className = "pill " + cls;
  $("health-detail").textContent = detail;
}
function updateCounters() {
  $("counter").textContent = `${state.frames} frames`;
  $("sent-counter").textContent = `${state.sent} sent`;
  $("recv-counter").textContent = `${state.received} recv`;
}
function countFrame(direction) {
  state.frames += 1;
  if (direction === "out") state.sent += 1;
  if (direction === "in") state.received += 1;
  updateCounters();
}
function pretty(raw) {
  if (typeof raw !== "string") return String(raw);
  try { return JSON.stringify(JSON.parse(raw), null, 2); } catch (_) { return raw; }
}
function updatePresetFields() {
  const preset = $("preset").value;
  for (const field of document.querySelectorAll("[data-presets]")) {
    field.hidden = !field.dataset.presets.split(/\s+/).includes(preset);
  }
}
function gcsRouteId() {
  const route = $("gcs-route").value;
  if (route === "user") return $("user-id").value;
  if (route === "device") return $("device-id").value;
  return $("conv-id").value;
}
function setGcsPath() {
  $("path").value = `/gcs/ws/${$("gcs-route").value}/${encodeURIComponent(gcsRouteId())}`;
}
function applyPreset() {
  const preset = $("preset").value;
  $("base").placeholder = sameOriginWsBase();
  updatePresetFields();
  if (preset === "gleam") {
    $("path").value = "/gleam/ws";
    $("payload").value = "ping";
  } else if (preset === "webrtc") {
    $("path").value = "/webrtc/signal";
    $("payload").value = JSON.stringify({
      type: "hello",
      metadata: { client: "web-home-rs/wss-test", at: new Date().toISOString() }
    }, null, 2);
  } else if (preset === "gcs") {
    setGcsPath();
    $("payload").value = JSON.stringify({
      Meta: {},
      List: [{
        "@vibe-meta": {},
        "@vibe-type": "PollForKafkaMessages",
        "@vibe-data": JSON.stringify({ TopicIds: [$("user-id").value] })
      }]
    }, null, 2);
  } else {
    $("path").value = "/fsws/ws/rx-burst";
    $("payload").value = JSON.stringify({
      id: "rx-" + Date.now().toString(36),
      payload: "sample from web-home-rs/wss-test"
    }, null, 2);
  }
  updateUrlPreview();
  updateControls();
}
function buildUrl() {
  const preset = $("preset").value;
  const base = normalizeWsBase($("base").value);
  if (preset === "gcs") setGcsPath();
  const path = ensureLeadingSlash($("path").value.trim() || "/");
  const url = new URL(base + path);

  if (preset === "gleam") {
    url.searchParams.set("threadId", $("thread-id").value.trim());
    url.searchParams.set("taskId", $("task-id").value.trim());
  } else if (preset === "webrtc") {
    url.searchParams.set("room", $("room-id").value.trim());
    url.searchParams.set("peer", $("peer-id").value.trim());
  } else {
    url.searchParams.set("userId", $("user-id").value.trim());
    url.searchParams.set("deviceId", $("device-id").value.trim());
    url.searchParams.set("conversationIds", JSON.stringify([$("conv-id").value.trim()]));
  }

  return url.toString();
}
function updateUrlPreview() {
  try {
    $("url-preview").textContent = buildUrl();
    $("url-preview").classList.remove("bad");
  } catch (error) {
    $("url-preview").textContent = "invalid target: " + String(error.message || error);
    $("url-preview").classList.add("bad");
  }
}
function healthPath() {
  const preset = $("preset").value;
  if (preset === "gleam") return "/gleam/healthz";
  if (preset === "webrtc") return "/webrtc/healthz";
  if (preset === "gcs") return "/gcs/ws-health";
  return "/fsws/healthz";
}
function httpBase() {
  return trimSlash(wsToHttp(normalizeWsBase($("base").value)));
}
async function checkHealth() {
  const url = httpBase() + healthPath();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 9000);
  setHealth("checking", "warn", url);
  log("GET " + url, "meta");
  try {
    const response = await fetch(url, { cache: "no-store", signal: controller.signal });
    const text = await response.text();
    const summary = text.slice(0, 600) || response.statusText;
    setHealth(`health ${response.status}`, response.ok ? "ok" : "bad", summary);
    log(`health ${response.status}: ${summary}`, response.ok ? "in" : "bad");
  } catch (error) {
    const message = "health error: " + String(error);
    setHealth("health error", "bad", message);
    log(message, "bad");
  } finally {
    clearTimeout(timeout);
  }
}
function connect() {
  let url;
  try {
    url = buildUrl();
  } catch (error) {
    log("invalid target: " + String(error.message || error), "bad");
    updateUrlPreview();
    return;
  }
  if (state.ws && state.ws.readyState !== WebSocket.CLOSED) disconnect();
  const ws = new WebSocket(url);
  state.ws = ws;
  state.connectStartedAt = performance.now();
  state.latencyMs = null;
  setStatus("connecting", "warn");
  log("open " + url, "meta");
  ws.onopen = () => {
    if (state.ws !== ws) return;
    state.latencyMs = Math.round(performance.now() - state.connectStartedAt);
    startUptimeTimer();
    setStatus("open", "ok");
    log(`connected in ${state.latencyMs}ms`, "meta");
    if ($("preset").value === "webrtc") sendHello();
  };
  ws.onmessage = (event) => {
    if (state.ws !== ws) return;
    if (state.lastSentAt) {
      state.latencyMs = Math.round(performance.now() - state.lastSentAt);
      state.lastSentAt = 0;
      refreshMetrics();
    }
    countFrame("in");
    log(pretty(event.data), "in");
  };
  ws.onerror = () => {
    if (state.ws !== ws) return;
    setStatus("error", "bad");
    log("websocket error", "bad");
  };
  ws.onclose = (event) => {
    if (state.ws !== ws) return;
    stopInterval();
    state.ws = null;
    stopUptimeTimer();
    setStatus(`closed ${event.code}`, event.code === 1000 ? "warn" : "bad");
    log(`closed code=${event.code} reason="${event.reason || ""}"`, event.code === 1000 ? "warn" : "bad");
    updateControls();
  };
  updateControls();
}
function disconnect() {
  stopInterval();
  if (!state.ws) {
    stopUptimeTimer();
    setStatus("idle", "warn");
    return;
  }
  if (state.ws.readyState === WebSocket.CLOSED) {
    state.ws = null;
    stopUptimeTimer();
    setStatus("idle", "warn");
    return;
  }
  stopUptimeTimer();
  setStatus("closing", "warn");
  try { state.ws.close(1000, "ui disconnect"); } catch (_) {}
  updateControls();
}
function isOpen() {
  return state.ws && state.ws.readyState === WebSocket.OPEN;
}
function sendRaw(raw) {
  if (!isOpen()) {
    log("not connected", "bad");
    updateControls();
    return false;
  }
  try {
    state.ws.send(raw);
  } catch (error) {
    log("send error: " + String(error), "bad");
    return false;
  }
  state.lastSentAt = performance.now();
  countFrame("out");
  log(pretty(raw), "out");
  updateControls();
  return true;
}
function sendPayload() {
  const raw = $("payload").value;
  if (raw.trim()) sendRaw(raw);
}
function sendPing() {
  if ($("preset").value === "webrtc") {
    sendRaw(JSON.stringify({ type: "ping" }));
  } else {
    sendRaw("ping");
  }
}
function sendHello() {
  if ($("preset").value === "webrtc") {
    sendRaw(JSON.stringify({
      type: "hello",
      metadata: { client: "web-home-rs/wss-test", peer: $("peer-id").value }
    }));
  } else {
    sendRaw("hello from web-home-rs/wss-test");
  }
}
function sampleFrame(index = null) {
  if ($("preset").value === "gleam") {
    return JSON.stringify({
      type: "task-event",
      threadId: $("thread-id").value,
      taskId: $("task-id").value,
      body: index === null ? "sample from wss-test" : `sample ${index} from wss-test`,
      at: new Date().toISOString()
    });
  }
  if ($("preset").value === "webrtc") {
    return JSON.stringify({
      type: "message",
      payload: {
        body: index === null ? "sample signaling message" : `sample signaling message ${index}`,
        at: new Date().toISOString()
      }
    });
  }
  if ($("preset").value === "gcs") {
    return JSON.stringify({
      Meta: {},
      List: [{
        "@vibe-meta": {},
        "@vibe-type": "PollForKafkaMessages",
        "@vibe-data": JSON.stringify({
          TopicIds: [$("user-id").value, $("conv-id").value],
          Sequence: index
        })
      }]
    });
  }
  return JSON.stringify({
    id: `rx-${Date.now().toString(36)}-${index === null ? "sample" : index}`,
    payload: index === null ? "sample from wss-test" : `burst payload ${index}`
  });
}
function sendSample() {
  sendRaw(sampleFrame());
}
function sendBurst() {
  if (!isOpen()) {
    log("not connected", "bad");
    return;
  }
  const count = Math.min(500, Math.max(1, Number.parseInt($("burst-count").value, 10) || 1));
  for (let i = 0; i < count; i += 1) sendRaw(sampleFrame(i + 1));
}
function stopInterval() {
  if (state.intervalTimer !== null) {
    clearInterval(state.intervalTimer);
    state.intervalTimer = null;
    log("interval stopped", "meta");
    updateControls();
  }
}
function startInterval() {
  if (!isOpen()) {
    log("not connected", "bad");
    return;
  }
  stopInterval();
  const ms = Math.min(60000, Math.max(50, Number.parseInt($("interval-ms").value, 10) || 1000));
  state.intervalTimer = setInterval(sendSample, ms);
  log(`interval started ${ms}ms`, "meta");
  updateControls();
}
function formatPayload(compact = false) {
  try {
    const parsed = JSON.parse($("payload").value);
    $("payload").value = JSON.stringify(parsed, null, compact ? 0 : 2);
    log(compact ? "payload compacted" : "payload formatted", "meta");
  } catch (error) {
    log("payload is not JSON: " + String(error.message || error), "bad");
  }
}
async function copyText(text, label) {
  try {
    await navigator.clipboard.writeText(text);
    log(`copied ${label}`, "meta");
  } catch (error) {
    log(`copy ${label} failed: ${String(error.message || error)}`, "bad");
  }
}

const requestedPreset = params.get("preset") || "gleam";
$("preset").value = presets.includes(requestedPreset) ? requestedPreset : "gleam";
$("base").value = params.get("base") || "";
$("thread-id").value = params.get("threadId") || defaults.threadId;
$("task-id").value = params.get("taskId") || defaults.taskId;
$("room-id").value = params.get("room") || defaults.roomId;
$("peer-id").value = params.get("peer") || defaults.peerId;
$("user-id").value = params.get("userId") || defaults.userId;
$("device-id").value = params.get("deviceId") || defaults.deviceId;
$("conv-id").value = params.get("convId") || defaults.convId;

$("preset").addEventListener("change", applyPreset);
$("gcs-route").addEventListener("change", () => {
  if ($("preset").value === "gcs") setGcsPath();
  updateUrlPreview();
});
for (const id of ["base", "path", "thread-id", "task-id", "room-id", "peer-id", "user-id", "device-id", "conv-id"]) {
  $(id).addEventListener("input", updateUrlPreview);
}
for (const id of ["burst-count", "interval-ms"]) {
  $(id).addEventListener("input", updateControls);
}
$("connect").onclick = connect;
$("disconnect").onclick = disconnect;
$("send").onclick = sendPayload;
$("send-ping").onclick = sendPing;
$("send-hello").onclick = sendHello;
$("send-sample").onclick = sendSample;
$("send-burst").onclick = sendBurst;
$("start-interval").onclick = startInterval;
$("stop-interval").onclick = stopInterval;
$("format-payload").onclick = () => formatPayload(false);
$("compact-payload").onclick = () => formatPayload(true);
$("copy-payload").onclick = () => copyText($("payload").value, "payload");
$("copy-log").onclick = () => copyText(Array.from($("log").children).map((row) => row.dataset.copy || row.textContent).join("\n"), "log");
$("check-health").onclick = () => { checkHealth().catch((error) => log("health error: " + String(error), "bad")); };
$("clear").onclick = () => {
  $("log").textContent = "";
  state.frames = 0;
  state.sent = 0;
  state.received = 0;
  updateCounters();
  setLastEvent("cleared");
};
$("copy-url").onclick = () => {
  try {
    copyText(buildUrl(), "url");
  } catch (error) {
    log("copy url failed: " + String(error.message || error), "bad");
  }
};
$("log-filter").addEventListener("change", applyLogFilter);
$("autoscroll").addEventListener("change", () => {
  if ($("autoscroll").checked) $("log").scrollTop = $("log").scrollHeight;
});
$("payload").addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") sendPayload();
});

applyPreset();
updateCounters();
setHealth("health unchecked", "warn");
log("ready", "meta");
window.addEventListener("beforeunload", disconnect);
if (params.get("autoconnect") === "1") setTimeout(connect, 50);
"###;
pub(crate) const PRESENCE_TEST_CSS: &str = r###":root {
  color-scheme: dark;
  --bg: #0b1117;
  --panel: #111923;
  --panel-2: #0f1720;
  --field: #0e1520;
  --line: rgba(148, 163, 184, 0.24);
  --text: #eef2f6;
  --muted: #a8b3c1;
  --accent: #5eead4;
  --warn: #fbbf24;
  --danger: #fb7185;
  --ok: #86efac;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
header {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 16px;
  align-items: center;
  padding: 12px 16px;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
  position: sticky;
  top: 0;
  z-index: 10;
}
header label { display: flex; flex-direction: column; gap: 2px; font-size: 11px; color: var(--muted); }
header input { width: 180px; }
input, button, select {
  background: var(--field);
  color: var(--text);
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 5px 8px;
  font: inherit;
}
input:focus, button:focus { outline: 1px solid var(--accent); }
button {
  cursor: pointer;
  background: var(--panel-2);
  transition: background .12s ease;
}
button:hover { background: #182032; }
button.primary { border-color: var(--accent); color: var(--accent); }
button.danger { border-color: var(--danger); color: var(--danger); }
.pill {
  display: inline-block;
  padding: 1px 7px;
  border-radius: 9px;
  font-size: 11px;
  background: rgba(94, 234, 212, 0.12);
  color: var(--accent);
  border: 1px solid rgba(94, 234, 212, 0.3);
}
.pill.warn { background: rgba(251,191,36,.12); color: var(--warn); border-color: rgba(251,191,36,.3); }
.pill.bad { background: rgba(251,113,133,.12); color: var(--danger); border-color: rgba(251,113,133,.3); }
.pill.ok { background: rgba(134,239,172,.12); color: var(--ok); border-color: rgba(134,239,172,.3); }
main { padding: 16px; display: grid; gap: 16px; grid-template-columns: 1fr; }
@media (min-width: 1080px) {
  main { grid-template-columns: 1fr 1fr; }
  .user-panel { grid-column: 1 / -1; }
}
.panel {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  min-height: 240px;
  overflow: hidden;
}
.panel-head {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
  padding: 8px 12px;
  border-bottom: 1px solid var(--line);
  background: var(--panel-2);
}
.panel-head .title { font-weight: 600; color: var(--text); }
.panel-head .meta { font-size: 11px; color: var(--muted); }
.panel-body { display: flex; flex-direction: column; flex: 1; min-height: 0; }
.controls { display: flex; gap: 6px; padding: 8px 12px; flex-wrap: wrap; align-items: center; }
.controls input[type="text"] { flex: 1; min-width: 140px; }
.log {
  flex: 1;
  margin: 0 12px 12px;
  padding: 8px;
  background: var(--panel-2);
  border: 1px solid var(--line);
  border-radius: 6px;
  overflow: auto;
  font-size: 12px;
  min-height: 160px;
  max-height: 320px;
}
.log .row { padding: 1px 0; white-space: pre-wrap; word-break: break-word; }
.log .row.system { color: var(--accent); }
.log .row.warn { color: var(--warn); }
.log .row.bad { color: var(--danger); }
.log .row.muted { color: var(--muted); }
.log .ts { color: var(--muted); }
.quick-bar {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  padding: 8px 16px;
  background: var(--panel-2);
  border-bottom: 1px solid var(--line);
}
footer { padding: 12px 16px; color: var(--muted); font-size: 12px; border-top: 1px solid var(--line); }
code { background: var(--panel-2); padding: 1px 6px; border-radius: 4px; }
"###;

pub(crate) const PRESENCE_TEST_BODY: &str = r###"<header>
  <label>user-id<input id="user" value="alice" /></label>
  <label>device-id<input id="device" value="d1" /></label>
  <label>presence base<input id="presence" value="/presence" style="width: 220px;" /></label>
  <label>conv ids (comma)<input id="convs" value="conv-1,conv-2,conv-3,conv-4,conv-5" style="width: 260px;" /></label>
  <div style="flex:1"></div>
  <button id="connect" class="primary" type="button">Connect all</button>
  <button id="disconnect" type="button">Disconnect all</button>
  <span id="status" class="pill warn">idle</span>
</header>

<div class="quick-bar">
  <span class="pill" id="self-info">no session</span>
  <span class="pill warn" id="ws-count">0 / 6 ws open</span>
  <span class="pill" id="hello-node">node: ?</span>
  <span class="muted" style="margin-left:auto">open this page in 3 tabs (alice/d1, bob/d2, carol/d3) to test cross-user fan-out</span>
</div>

<main id="grid">
  <section class="panel user-panel" id="user-panel">
    <div class="panel-head">
      <span class="title">user-ws</span>
      <span class="meta" id="user-meta">/ws?user=…&amp;device=…</span>
      <span id="user-status" class="pill bad" style="margin-left:auto">closed</span>
    </div>
    <div class="controls">
      <input id="user-broadcast-input" type="text" placeholder="send to /user/&lt;me&gt;/broadcast — every user-ws of me on every node" />
      <button id="user-broadcast-send" type="button">user-broadcast</button>
      <button id="user-logout" class="danger" type="button">logout this device</button>
      <button id="user-clear" type="button">clear log</button>
    </div>
    <div class="log" id="user-log"></div>
  </section>
  <!-- conv panels are injected here -->
</main>

<footer>
  <div>Quick links:</div>
  <div>
    <a href="?user=alice&amp;device=d1&amp;autoconnect=1">alice / d1</a> ·
    <a href="?user=bob&amp;device=d2&amp;autoconnect=1">bob / d2</a> ·
    <a href="?user=carol&amp;device=d3&amp;autoconnect=1">carol / d3</a>
  </div>
  <div style="margin-top:6px">
    Routes exercised:
    <code>GET /ws?user=…&amp;device=…</code>,
    <code>GET /ws?user=…&amp;conv=…&amp;device=…</code>,
    <code>POST /conv/&lt;id&gt;/members/&lt;user&gt;</code>,
    <code>DELETE /conv/&lt;id&gt;/members/&lt;user&gt;</code>,
    <code>POST /conv/&lt;id&gt;/broadcast</code>,
    <code>POST /user/&lt;id&gt;/broadcast</code>,
    <code>POST /user/&lt;u&gt;/devices/&lt;d&gt;/logout</code>.
  </div>
</footer>"###;

pub(crate) const PRESENCE_TEST_JS: &str = r###"const $ = (id) => document.getElementById(id);

// Apply ?user=, ?device=, ?presence=, ?convs=, ?autoconnect= from URL.
const params = new URLSearchParams(location.search);
for (const k of ["user", "device", "presence", "convs"]) {
  if (params.has(k)) $(k).value = params.get(k);
}

// ───────────────────────────────────────────────────────────────────
// state
const state = {
  userWs: null,
  convs: {},          // convId → { ws, panel, logEl, statusEl, membersEl }
  helloUserNode: null,
  presenceProbe: null,
};

function nowTs() {
  const d = new Date();
  return d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0");
}

function log(panelLog, text, cls = "") {
  const row = document.createElement("div");
  row.className = "row " + cls;
  const ts = document.createElement("span");
  ts.className = "ts";
  ts.textContent = nowTs() + " ";
  row.append(ts, document.createTextNode(text));
  panelLog.appendChild(row);
  // Keep ~400 lines max.
  while (panelLog.childNodes.length > 400) panelLog.removeChild(panelLog.firstChild);
  panelLog.scrollTop = panelLog.scrollHeight;
}

function setPill(el, text, cls) {
  el.textContent = text;
  el.className = "pill " + cls;
}

function compactBody(text) {
  const s = String(text || "").trim();
  return s.length > 180 ? s.slice(0, 180) + "..." : s;
}
function normalizedHost(hostname) {
  return String(hostname || "").toLowerCase().replace(/^\[|\]$/g, "");
}
function isLoopbackHost(hostname) {
  const h = normalizedHost(hostname);
  return h === "localhost" || h === "::1" || h === "0.0.0.0" || h.startsWith("127.");
}
function isLocalPage() {
  return isLoopbackHost(location.hostname);
}
function presenceBaseUrl() {
  const raw = $("presence").value.trim() || "/presence";
  const url = new URL(raw, location.origin);
  if (url.protocol === "ws:") url.protocol = "http:";
  if (url.protocol === "wss:") url.protocol = "https:";
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(`unsupported presence base protocol: ${url.protocol}`);
  }
  if (!isLocalPage() && isLoopbackHost(url.hostname)) {
    throw new Error(`refusing loopback presence base from remote page: ${url.hostname}`);
  }
  url.hash = "";
  url.search = "";
  return url;
}
function safePresenceBaseUrl(panelLog) {
  try {
    return presenceBaseUrl();
  } catch (e) {
    const targetLog = panelLog || $("user-log");
    log(targetLog, e && e.message ? e.message : String(e), "bad");
    setPill($("status"), "invalid base", "bad");
    return null;
  }
}
function stripTrailingSlash(value) {
  return value.replace(/\/$/, "");
}
function wsBase(panelLog) {
  const url = safePresenceBaseUrl(panelLog);
  if (!url) return null;
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return stripTrailingSlash(url.toString());
}
function httpBase(panelLog) {
  const url = safePresenceBaseUrl(panelLog);
  return url ? stripTrailingSlash(url.toString()) : null;
}
async function ensurePresenceReady(panelLog) {
  const base = httpBase(panelLog);
  if (!base) return false;
  const now = Date.now();
  const cached = state.presenceProbe;
  if (cached && cached.base === base && now - cached.at < 3000) return cached.ok;
  try {
    const r = await fetch(`${base}/healthz`, { credentials: "same-origin", cache: "no-store" });
    const body = compactBody(await r.text());
    const ok = r.ok;
    state.presenceProbe = { base, ok, at: now };
    if (!ok) {
      const auth = r.status === 401 ? "presence gateway auth required" : "presence health check failed";
      log(panelLog, `${auth}: HTTP ${r.status}${body ? " " + body : ""}`, "bad");
      setPill($("status"), r.status === 401 ? "auth required" : "health failed", "bad");
      return false;
    }
    return true;
  } catch (e) {
    state.presenceProbe = { base, ok: false, at: now };
    log(panelLog, `presence health check failed: ${e}`, "bad");
    setPill($("status"), "health failed", "bad");
    return false;
  }
}

function updateWsCount() {
  let open = 0;
  if (state.userWs && state.userWs.readyState === WebSocket.OPEN) open++;
  for (const k in state.convs) {
    const c = state.convs[k];
    if (c.ws && c.ws.readyState === WebSocket.OPEN) open++;
  }
  const total = 1 + Object.keys(state.convs).length;
  const el = $("ws-count");
  el.textContent = `${open} / ${total} ws open`;
  el.className = "pill " + (open === total ? "ok" : open === 0 ? "bad" : "warn");
}

function applySelfInfo() {
  $("self-info").textContent = `me: ${$("user").value || "?"}@${$("device").value || "?"}`;
}

// ───────────────────────────────────────────────────────────────────
// Conv panels — built once from the comma-separated list, never re-
// rendered. Each panel owns one conv-ws lifecycle.
function buildConvPanels() {
  const grid = $("grid");
  // Remove any existing conv panels (everything after user-panel).
  Array.from(grid.querySelectorAll(".panel.conv-panel")).forEach((n) => n.remove());
  state.convs = {};

  const convIds = $("convs").value.split(",").map((s) => s.trim()).filter(Boolean);
  for (const convId of convIds) {
    const panel = document.createElement("section");
    panel.className = "panel conv-panel";
    panel.dataset.conv = convId;
    panel.innerHTML = `
      <div class="panel-head">
        <span class="title">${convId}</span>
        <span class="meta">/ws?user=&hellip;&amp;conv=${convId}</span>
        <span class="pill" data-role="members">members: —</span>
        <span class="pill bad" data-role="status" style="margin-left:auto">closed</span>
      </div>
      <div class="controls">
        <button type="button" data-act="join">join (me)</button>
        <button type="button" data-act="leave" class="danger">leave (me)</button>
        <button type="button" data-act="open">open ws</button>
        <button type="button" data-act="close" class="danger">close ws</button>
        <button type="button" data-act="refresh">refresh members</button>
      </div>
      <div class="controls">
        <input type="text" data-role="broadcast-input" placeholder="broadcast to ${convId} — every conv-ws of every member" />
        <button type="button" data-act="broadcast">send</button>
        <button type="button" data-act="clear">clear log</button>
      </div>
      <div class="log" data-role="log"></div>
    `;
    grid.appendChild(panel);

    const logEl = panel.querySelector('[data-role="log"]');
    const statusEl = panel.querySelector('[data-role="status"]');
    const membersEl = panel.querySelector('[data-role="members"]');
    const broadcastInput = panel.querySelector('[data-role="broadcast-input"]');

    panel.querySelector('[data-act="join"]').onclick = () => joinConv(convId);
    panel.querySelector('[data-act="leave"]').onclick = () => leaveConv(convId);
    panel.querySelector('[data-act="open"]').onclick = () => openConvWs(convId);
    panel.querySelector('[data-act="close"]').onclick = () => closeConvWs(convId);
    panel.querySelector('[data-act="refresh"]').onclick = () => refreshConvMembers(convId);
    panel.querySelector('[data-act="broadcast"]').onclick = () => {
      const v = broadcastInput.value;
      if (!v) return;
      convBroadcast(convId, v);
      broadcastInput.value = "";
    };
    broadcastInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") panel.querySelector('[data-act="broadcast"]').click();
    });
    panel.querySelector('[data-act="clear"]').onclick = () => { logEl.textContent = ""; };

    state.convs[convId] = { ws: null, panel, logEl, statusEl, membersEl };
  }
}

// ───────────────────────────────────────────────────────────────────
// user-ws lifecycle
async function openUserWs(skipPreflight = false) {
  const logEl = $("user-log");
  if (state.userWs && state.userWs.readyState <= 1) return true;
  if (!skipPreflight && !(await ensurePresenceReady(logEl))) return false;
  const user = $("user").value.trim();
  const device = $("device").value.trim();
  if (!user) { log(logEl, "missing user-id", "bad"); return false; }
  const qs = new URLSearchParams({ user });
  if (device) qs.set("device", device);
  const base = wsBase(logEl);
  if (!base) return false;
  const url = `${base}/ws?${qs}`;
  $("user-meta").textContent = url;
  const ws = new WebSocket(url);
  state.userWs = ws;
  setPill($("user-status"), "connecting", "warn");
  log(logEl, `→ open ${url}`, "muted");
  ws.onopen = () => { setPill($("user-status"), "open", "ok"); updateWsCount(); };
  ws.onclose = (e) => {
    setPill($("user-status"), `closed (${e.code})`, "bad");
    log(logEl, `← close code=${e.code} reason="${e.reason || ""}"`, "warn");
    updateWsCount();
  };
  ws.onerror = () => log(logEl, "← error (see devtools)", "bad");
  ws.onmessage = (e) => handleUserFrame(e.data);
  return true;
}

function closeUserWs() {
  if (state.userWs) {
    try { state.userWs.close(); } catch (_) {}
    state.userWs = null;
  }
  setPill($("user-status"), "closed", "bad");
  updateWsCount();
}

function handleUserFrame(raw) {
  const sys = tryParseSystemFrame(raw);
  if (!sys) {
    log($("user-log"), `← payload: ${raw}`);
    return;
  }
  log($("user-log"), `← ${raw}`, "system");
  if (sys.type === "hello") {
    $("hello-node").textContent = `node: ${sys.node}`;
    state.helloUserNode = sys.node;
  } else if (sys.type === "membership-changed") {
    if (sys.change === "added" && state.convs[sys.conv]) {
      const c = state.convs[sys.conv];
      const members = Array.isArray(sys.members) ? sys.members : [];
      setPill(c.membersEl, `members: ${members.join(",") || "—"}`, "");
      log(c.logEl, `(user-ws) added; members=[${members.join(",")}]`, "system");
    } else if (sys.change === "removed" && state.convs[sys.conv]) {
      const c = state.convs[sys.conv];
      log(c.logEl, "(user-ws) removed from conv", "warn");
      setPill(c.membersEl, "members: (you left)", "warn");
    }
  } else if (sys.type === "kick") {
    log($("user-log"), `kick: ${sys.reason}`, "bad");
  }
}

// ───────────────────────────────────────────────────────────────────
// conv-ws lifecycle
async function openConvWs(convId, skipPreflight = false) {
  const c = state.convs[convId];
  if (!c) return false;
  if (c.ws && c.ws.readyState <= 1) return true;
  if (!skipPreflight && !(await ensurePresenceReady(c.logEl))) return false;
  const user = $("user").value.trim();
  const device = $("device").value.trim();
  const qs = new URLSearchParams({ user, conv: convId });
  if (device) qs.set("device", device);
  const base = wsBase(c.logEl);
  if (!base) return false;
  const url = `${base}/ws?${qs}`;
  const ws = new WebSocket(url);
  c.ws = ws;
  setPill(c.statusEl, "connecting", "warn");
  log(c.logEl, `→ open ${url}`, "muted");
  ws.onopen = () => { setPill(c.statusEl, "open", "ok"); updateWsCount(); };
  ws.onclose = (e) => {
    setPill(c.statusEl, `closed (${e.code})`, "bad");
    log(c.logEl, `← close code=${e.code} reason="${e.reason || ""}"`, "warn");
    updateWsCount();
  };
  ws.onerror = () => log(c.logEl, "← error", "bad");
  ws.onmessage = (e) => handleConvFrame(convId, e.data);
  return true;
}

function closeConvWs(convId) {
  const c = state.convs[convId];
  if (c && c.ws) {
    try { c.ws.close(); } catch (_) {}
    c.ws = null;
  }
  if (c) setPill(c.statusEl, "closed", "bad");
  updateWsCount();
}

function handleConvFrame(convId, raw) {
  const c = state.convs[convId];
  if (!c) return;
  const sys = tryParseSystemFrame(raw);
  if (!sys) {
    log(c.logEl, `← payload: ${raw}`);
    return;
  }
  log(c.logEl, `← ${raw}`, "system");
  if (sys.type === "kick") {
    setPill(c.statusEl, `kicked`, "bad");
  }
}

// ───────────────────────────────────────────────────────────────────
// HTTP API calls
async function joinConv(convId) {
  const user = $("user").value.trim();
  const c = state.convs[convId];
  const base = httpBase(c ? c.logEl : $("user-log"));
  if (!base) return false;
  const res = await postPlain(`${base}/conv/${enc(convId)}/members/${enc(user)}`);
  if (c) log(c.logEl, `POST /members/${user} → ${res}`, "system");
  // Refresh membership pill (the user-ws will also see the membership-
  // changed JSON if I'm registered).
  refreshConvMembers(convId);
  return !res.startsWith("HTTP 401");
}

async function leaveConv(convId) {
  const user = $("user").value.trim();
  const c = state.convs[convId];
  const base = httpBase(c ? c.logEl : $("user-log"));
  if (!base) return;
  const res = await deletePlain(`${base}/conv/${enc(convId)}/members/${enc(user)}`);
  if (c) log(c.logEl, `DELETE /members/${user} → ${res}`, "warn");
  refreshConvMembers(convId);
}

async function refreshConvMembers(convId) {
  const c = state.convs[convId];
  if (!c) return;
  const base = httpBase(c.logEl);
  if (!base) return;
  try {
    const r = await fetch(`${base}/conv/${enc(convId)}/members`, { credentials: "same-origin", cache: "no-store" });
    const body = (await r.text()).trim();
    if (!r.ok) {
      setPill(c.membersEl, `members: HTTP ${r.status}`, "bad");
      return;
    }
    const members = body ? body.split("\n") : [];
    setPill(c.membersEl, `members: ${members.join(",") || "—"}`, members.length ? "" : "warn");
  } catch (e) {
    setPill(c.membersEl, "members: ?", "bad");
  }
}

async function convBroadcast(convId, payload) {
  const c = state.convs[convId];
  const base = httpBase(c ? c.logEl : $("user-log"));
  if (!base) return;
  const res = await postPlain(`${base}/conv/${enc(convId)}/broadcast`, payload);
  if (c) log(c.logEl, `POST /broadcast (${payload.length}B) → ${res}`, "muted");
}

async function userBroadcast(payload) {
  const user = $("user").value.trim();
  const base = httpBase($("user-log"));
  if (!base) return;
  const res = await postPlain(`${base}/user/${enc(user)}/broadcast`, payload);
  log($("user-log"), `POST /user/${user}/broadcast → ${res}`, "muted");
}

async function deviceLogout() {
  const user = $("user").value.trim();
  const device = $("device").value.trim();
  if (!device) { log($("user-log"), "device-id required for logout", "bad"); return; }
  const base = httpBase($("user-log"));
  if (!base) return;
  const res = await postPlain(`${base}/user/${enc(user)}/devices/${enc(device)}/logout`, "ui-button");
  log($("user-log"), `POST /devices/${device}/logout → ${res}`, "warn");
}

// ───────────────────────────────────────────────────────────────────
// helpers
async function postPlain(url, body = "") {
  try {
    const r = await fetch(url, {
      method: "POST",
      body,
      headers: { "content-type": "text/plain" },
      credentials: "same-origin",
      cache: "no-store",
    });
    return `HTTP ${r.status} ${(await r.text()).trim()}`;
  } catch (e) { return `error: ${e}`; }
}
async function deletePlain(url) {
  try {
    const r = await fetch(url, { method: "DELETE", credentials: "same-origin", cache: "no-store" });
    return `HTTP ${r.status} ${(await r.text()).trim()}`;
  } catch (e) { return `error: ${e}`; }
}
function enc(s) { return encodeURIComponent(s); }

function tryParseSystemFrame(raw) {
  if (typeof raw !== "string") return null;
  const s = raw.trimStart();
  if (!s.startsWith("{")) return null;
  try {
    const o = JSON.parse(s);
    return typeof o === "object" && o && typeof o.type === "string" ? o : null;
  } catch (_) { return null; }
}

// ───────────────────────────────────────────────────────────────────
// top-level connect / disconnect
async function connectAll() {
  setPill($("status"), "connecting", "warn");
  if (!(await ensurePresenceReady($("user-log")))) return;
  await openUserWs(true);
  // Join every conv THEN open its ws. Membership is required for the
  // conv-ws upgrade to succeed.
  for (const convId of Object.keys(state.convs)) {
    const joined = await joinConv(convId);
    if (joined) await openConvWs(convId, true);
  }
  setPill($("status"), "connected", "ok");
}

function disconnectAll() {
  closeUserWs();
  for (const convId of Object.keys(state.convs)) closeConvWs(convId);
  setPill($("status"), "idle", "warn");
}

// ───────────────────────────────────────────────────────────────────
// wire up
$("user").addEventListener("input", applySelfInfo);
$("device").addEventListener("input", applySelfInfo);
$("convs").addEventListener("change", buildConvPanels);
$("presence").addEventListener("input", () => { state.presenceProbe = null; });

$("connect").onclick = connectAll;
$("disconnect").onclick = disconnectAll;
$("user-broadcast-send").onclick = () => {
  const v = $("user-broadcast-input").value;
  if (!v) return;
  userBroadcast(v);
  $("user-broadcast-input").value = "";
};
$("user-broadcast-input").addEventListener("keydown", (e) => {
  if (e.key === "Enter") $("user-broadcast-send").click();
});
$("user-logout").onclick = deviceLogout;
$("user-clear").onclick = () => { $("user-log").textContent = ""; };

applySelfInfo();
buildConvPanels();
updateWsCount();
// Periodic ws-count refresh in case readyState changes silently.
setInterval(updateWsCount, 1000);

if (params.get("autoconnect") === "1") {
  // Defer one tick so panels are in the DOM before the WSes try to
  // resolve. Then fire-and-forget.
  setTimeout(connectAll, 50);
}
"###;
