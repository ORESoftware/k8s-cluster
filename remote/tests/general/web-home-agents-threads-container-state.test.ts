import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';
import { chromium } from 'playwright';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/web-home-rs/Cargo.toml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readWebHomeSource(): Promise<string> {
  return readFile(resolve(repoRoot, 'remote/deployments/web-home-rs/src/main.rs'), 'utf8');
}

function rawStringConst(source: string, name: string): string {
  const startMatch = new RegExp(`const ${name}: &str = r(#+)"`).exec(source);
  assert.ok(startMatch, `expected ${name} raw string constant`);
  const start = startMatch.index + startMatch[0].length;
  const endToken = `"${startMatch[1]};`;
  const end = source.indexOf(endToken, start);
  assert.notEqual(end, -1, `expected ${name} raw string terminator`);
  return source.slice(start, end);
}

function extractClassifierBlock(js: string): string {
  const startMarker = '      const CONTAINER_FAIL_REASONS = new Set([';
  const endMarker = '      function pillClassFromKind(kind) {';
  const start = js.indexOf(startMarker);
  const end = js.indexOf(endMarker);
  assert.ok(start >= 0, 'expected CONTAINER_FAIL_REASONS declaration');
  assert.ok(end > start, 'expected pillClassFromKind declaration after classifier');
  return js.slice(start, end);
}

function extractContainerStateBlock(js: string): string {
  const startMarker = '      const CONTAINER_FAIL_REASONS = new Set([';
  const endMarker = '      async function loadRuntimeState(threadId, render = true) {';
  const start = js.indexOf(startMarker);
  const end = js.indexOf(endMarker);
  assert.ok(start >= 0, 'expected CONTAINER_FAIL_REASONS declaration');
  assert.ok(end > start, 'expected loadRuntimeState declaration after container-state block');
  return js.slice(start, end);
}

test('agents threads page renders a clickable container-state pill in the workspace topbar', async () => {
  const server = await readWebHomeSource();
  assert.match(
    server,
    /span id="container-state" class="pill warn clickable" role="button" tabindex="0" aria-busy="false" aria-live="polite"[^{]*\{ "container: no thread" \}/,
  );
  assert.match(server, /title="[^"]*Click to probe now\./);
  assert.match(
    server,
    /a href="\/agents\/tasks" \{ "Diagnostics table" \}\s*\n\s*a href="\/home" \{ "Service directory" \}/,
  );
  const css = rawStringConst(server, 'AGENTS_THREADS_CSS');
  assert.match(css, /\.pill\.clickable \{[\s\S]*cursor: pointer/);
  assert.match(css, /\.pill\.clickable:hover \{/);
  assert.match(css, /\.pill\.clickable:focus-visible \{/);
  assert.match(css, /\.pill\.clickable\.probing \{[\s\S]*cursor: progress/);
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  assert.match(js, /function classifyContainerState\(data, opts = \{\}\) \{/);
  assert.match(js, /function startContainerStatePolling\(threadId\)/);
  assert.match(js, /function stopContainerStatePolling\(\)/);
  assert.match(js, /function refreshContainerStateNow\(\)/);
  assert.match(js, /function scheduleNextContainerStatePoll\(threadId\)/);
  assert.match(js, /function containerStatePollInterval\(\)/);
  assert.match(js, /function abortInflightContainerStateFetch\(\)/);
  assert.match(js, /function bindContainerStateVisibility\(\)/);
  assert.match(js, /const CONTAINER_STATE_FETCH_TIMEOUT_MS = \d+;/);
  assert.match(js, /const CONTAINER_STATE_MANUAL_DEBOUNCE_MS = \d+;/);
  assert.match(js, /const CONTAINER_STATE_BACKOFF_BASE_MS = \d+;/);
  assert.match(js, /const CONTAINER_STATE_BACKOFF_MAX_MS = \d+;/);
  assert.match(js, /typeof AbortController === "function"/);
  assert.match(js, /visibilitychange/);
  assert.match(js, /syncContainerStatePolling\(\);/);
  assert.match(
    js,
    /\$\("container-state"\)\.addEventListener\("click", refreshContainerStateNow\);/,
  );
  assert.match(
    js,
    /\$\("container-state"\)\.addEventListener\("keydown", \(event\) => \{[\s\S]*event\.key !== "Enter" && event\.key !== " "/,
  );
  assert.match(
    js,
    /\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/runtime/,
  );
});

test('classifyContainerState maps runtime payloads to lifecycle labels', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const classifierBlock = extractClassifierBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(`<!doctype html><html><body><script>
      ${classifierBlock}
      window.__classify = classifyContainerState;
    </script></body></html>`);

    const result = await page.evaluate(() => {
      const classify = (window as unknown as {
        __classify: (data: unknown, opts?: Record<string, unknown>) => {
          label: string;
          kind: string;
          title: string;
        };
      }).__classify;

      const cases = {
        neverLived: classify({
          ok: true,
          deployment: null,
          pods: [],
          summary: { phase: 'missing', desiredReplicas: 0 },
          errors: [],
        }),
        nonExistent: classify(
          {
            ok: true,
            deployment: null,
            pods: [],
            summary: { phase: 'missing', desiredReplicas: 0 },
            errors: [],
          },
          { threadExists: true },
        ),
        suspended: classify({
          ok: true,
          deployment: { name: 'dd-remote-thread-abc' },
          pods: [],
          summary: { phase: 'sleeping', desiredReplicas: 0 },
          errors: [],
        }),
        coldStart: classify({
          ok: true,
          deployment: { name: 'dd-remote-thread-abc' },
          pods: [],
          summary: { phase: 'creating', desiredReplicas: 1, podCount: 0, readyPodCount: 0 },
          errors: [],
        }),
        warming: classify({
          ok: true,
          deployment: { name: 'dd-remote-thread-abc' },
          pods: [
            {
              name: 'dd-remote-thread-abc-0',
              phase: 'Pending',
              containers: [
                {
                  name: 'worker',
                  ready: false,
                  restartCount: 0,
                  state: {
                    waiting: { reason: 'ContainerCreating', message: 'pulling image' },
                  },
                },
              ],
            },
          ],
          summary: {
            phase: 'starting',
            desiredReplicas: 1,
            podCount: 1,
            readyPodCount: 0,
          },
          errors: [],
        }),
        running: classify({
          ok: true,
          deployment: { name: 'dd-remote-thread-abc' },
          pods: [
            {
              name: 'dd-remote-thread-abc-0',
              phase: 'Running',
              containers: [
                {
                  name: 'worker',
                  ready: true,
                  restartCount: 0,
                  state: { running: { startedAt: '2026-05-21T12:00:00Z' } },
                },
              ],
            },
          ],
          summary: {
            phase: 'ready',
            desiredReplicas: 1,
            availableReplicas: 1,
            readyReplicas: 1,
            podCount: 1,
            readyPodCount: 1,
          },
          errors: [],
        }),
        dead: classify({
          ok: true,
          deployment: { name: 'dd-remote-thread-abc' },
          pods: [
            {
              name: 'dd-remote-thread-abc-0',
              phase: 'Running',
              containers: [
                {
                  name: 'worker',
                  ready: false,
                  restartCount: 8,
                  state: {
                    waiting: {
                      reason: 'CrashLoopBackOff',
                      message: 'back-off restarting failed container',
                    },
                  },
                },
              ],
            },
          ],
          summary: {
            phase: 'starting',
            desiredReplicas: 1,
            availableReplicas: 0,
            readyReplicas: 0,
            podCount: 1,
            readyPodCount: 0,
          },
          errors: [],
        }),
        pending: classify({
          ok: true,
          deployment: { name: 'dd-remote-thread-abc' },
          pods: [
            {
              name: 'dd-remote-thread-abc-0',
              phase: 'Pending',
              conditions: [
                {
                  type: 'PodScheduled',
                  status: 'False',
                  reason: 'Unschedulable',
                  message: '0/3 nodes are available: 3 Too many pods.',
                },
              ],
              containers: [],
            },
          ],
          summary: {
            phase: 'starting',
            desiredReplicas: 1,
            availableReplicas: 0,
            readyReplicas: 0,
            podCount: 1,
            readyPodCount: 0,
          },
          errors: [],
        }),
        runtimeError: classify({
          ok: false,
          deployment: null,
          pods: [],
          summary: { phase: 'missing', desiredReplicas: 0 },
          errors: ['kubernetes API unreachable: timeout'],
        }),
        idle: classify(null),
      };
      return cases;
    });

    assert.equal(result.neverLived.label, 'container: never-lived');
    assert.equal(result.neverLived.kind, 'warn');

    assert.equal(result.nonExistent.label, 'container: non-existent');
    assert.equal(result.nonExistent.kind, 'warn');

    assert.equal(result.suspended.label, 'container: suspended');
    assert.equal(result.suspended.kind, 'warn');

    assert.equal(result.coldStart.label, 'container: cold-start');
    assert.equal(result.coldStart.kind, 'warn');

    assert.equal(result.warming.label, 'container: warming (ContainerCreating)');
    assert.equal(result.warming.kind, 'warn');
    assert.ok(result.warming.title.includes('pulling image'));

    assert.equal(result.running.label, 'container: running');
    assert.equal(result.running.kind, 'ok');
    assert.ok(result.running.title.includes('1/1 pods ready'));

    assert.equal(result.dead.label, 'container: dead (CrashLoopBackOff)');
    assert.equal(result.dead.kind, 'bad');
    assert.ok(result.dead.title.includes('back-off restarting failed container'));

    assert.equal(result.pending.label, 'container: pending (Unschedulable)');
    assert.equal(result.pending.kind, 'warn');
    assert.ok(result.pending.title.includes('Too many pods'));

    assert.equal(result.runtimeError.label, 'container: runtime error');
    assert.equal(result.runtimeError.kind, 'bad');
    assert.ok(result.runtimeError.title.includes('timeout'));

    assert.equal(result.idle.label, 'container: idle');
    assert.equal(result.idle.kind, 'warn');
  } finally {
    await browser.close();
  }
});

test('agents threads page wires the container-state pill into selection changes', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const classifierBlock = extractClassifierBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    const pillHarness = `
      function pillClassFromKind(kind) {
        if (kind === "ok") return "pill";
        if (kind === "bad") return "pill bad";
        return "pill warn";
      }
      function containerStatePillClass(kind, probing) {
        const classes = [pillClassFromKind(kind), "clickable"];
        if (probing) classes.push("probing");
        return classes.join(" ");
      }
      const state = { selectedThreadId: null, containerStateLastKey: "" };
      function $(id) { return document.getElementById(id); }
      function existingThread(id) { return id === "thread-known" ? { id, title: "known" } : null; }
      function setContainerStatePill(info) {
        const node = $("container-state");
        if (!node) return;
        const next = info || { label: "container: no thread", kind: "warn", title: "" };
        const probing = Boolean(next.probing);
        const key = next.kind + "|" + next.label + "|" + (next.title || "") + "|" + (probing ? 1 : 0);
        if (state.containerStateLastKey === key) return;
        state.containerStateLastKey = key;
        node.textContent = next.label;
        node.className = containerStatePillClass(next.kind, probing);
        node.title = next.title || "";
      }
      function refreshContainerStatePill(data) {
        const threadId = state.selectedThreadId;
        if (!threadId) { setContainerStatePill(null); return; }
        setContainerStatePill(classifyContainerState(data, { threadExists: Boolean(existingThread(threadId)) }));
      }
      window.__setState = (next) => Object.assign(state, next);
      window.__refresh = refreshContainerStatePill;
      window.__setPill = setContainerStatePill;
    `;

    await page.setContent(`<!doctype html><html><body>
      <span id="container-state" class="pill warn clickable" role="button" tabindex="0">container: no thread</span>
      <script>
        ${classifierBlock}
        ${pillHarness}
      </script>
    </body></html>`);

    type PillSnapshot = { text: string; className: string; title: string };
    const observed = await page.evaluate<PillSnapshot[]>(`(() => {
      const win = window;
      const node = document.getElementById('container-state');
      const snapshot = () => ({ text: node.textContent, className: node.className, title: node.title });
      const log = [];
      log.push(snapshot());

      win.__setState({ selectedThreadId: 'thread-unknown' });
      win.__refresh({
        deployment: null,
        pods: [],
        summary: { phase: 'missing', desiredReplicas: 0 },
        errors: [],
      });
      log.push(snapshot());

      win.__refresh({
        deployment: { name: 'dd-remote-thread-x' },
        pods: [
          {
            name: 'dd-remote-thread-x-0',
            phase: 'Running',
            containers: [
              { name: 'worker', ready: true, state: { running: { startedAt: '2026-05-21T12:00:00Z' } } },
            ],
          },
        ],
        summary: {
          phase: 'ready',
          desiredReplicas: 1,
          availableReplicas: 1,
          readyReplicas: 1,
          podCount: 1,
          readyPodCount: 1,
        },
        errors: [],
      });
      log.push(snapshot());

      win.__setPill({ label: 'container: probing', kind: 'warn', title: 'manual probe', probing: true });
      log.push(snapshot());

      win.__setState({ selectedThreadId: null });
      win.__refresh({});
      log.push(snapshot());

      return log;
    })()`);

    assert.deepEqual(observed[0], {
      text: 'container: no thread',
      className: 'pill warn clickable',
      title: '',
    });
    assert.equal(observed[1].text, 'container: never-lived');
    assert.equal(observed[1].className, 'pill warn clickable');
    assert.equal(observed[2].text, 'container: running');
    assert.equal(observed[2].className, 'pill clickable');
    assert.equal(observed[3].text, 'container: probing');
    assert.equal(observed[3].className, 'pill warn clickable probing');
    assert.equal(observed[3].title, 'manual probe');
    assert.equal(observed[4].text, 'container: no thread');
    assert.equal(observed[4].className, 'pill warn clickable');
  } finally {
    await browser.close();
  }
});

test('clicking the container-state pill issues a one-off runtime probe', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const classifierBlock = extractClassifierBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    const harness = `
      function pillClassFromKind(kind) {
        if (kind === "ok") return "pill";
        if (kind === "bad") return "pill bad";
        return "pill warn";
      }
      function containerStatePillClass(kind, probing) {
        const classes = [pillClassFromKind(kind), "clickable"];
        if (probing) classes.push("probing");
        return classes.join(" ");
      }
      const state = { selectedThreadId: null, containerStateLastKey: "", lastRuntimeData: null };
      function $(id) { return document.getElementById(id); }
      function existingThread(id) { return null; }
      function warnAdminDetail() {}
      function setContainerStatePill(info) {
        const node = $("container-state");
        if (!node) return;
        const next = info || { label: "container: no thread", kind: "warn", title: "" };
        const probing = Boolean(next.probing);
        const key = next.kind + "|" + next.label + "|" + (next.title || "") + "|" + (probing ? 1 : 0);
        if (state.containerStateLastKey === key) return;
        state.containerStateLastKey = key;
        node.textContent = next.label;
        node.className = containerStatePillClass(next.kind, probing);
        node.title = next.title || "";
      }
      function refreshContainerStatePill(data) {
        const threadId = state.selectedThreadId;
        if (!threadId) { setContainerStatePill(null); return; }
        setContainerStatePill(classifyContainerState(data, { threadExists: Boolean(existingThread(threadId)) }));
      }
      async function loadContainerState(threadId, opts = {}) {
        const manual = Boolean(opts.manual);
        if (manual && state.selectedThreadId === threadId) {
          setContainerStatePill({
            label: "container: probing",
            kind: "warn",
            title: "Probing runtime state for " + threadId,
            probing: true,
          });
        }
        const response = await fetch("/api/agents/threads/" + encodeURIComponent(threadId) + "/runtime", { cache: "no-store" });
        if (!response.ok) {
          if (state.selectedThreadId === threadId) {
            setContainerStatePill({
              label: "container: probe failed (" + response.status + ")",
              kind: "bad",
              title: "Runtime probe HTTP " + response.status + ". Click to retry.",
            });
          }
          throw new Error("runtime " + response.status);
        }
        const data = await response.json();
        state.lastRuntimeData = data;
        if (state.selectedThreadId === threadId) refreshContainerStatePill(data);
        return data;
      }
      function refreshContainerStateNow() {
        const threadId = state.selectedThreadId;
        if (!threadId) { setContainerStatePill(null); return; }
        loadContainerState(threadId, { manual: true }).catch(() => {});
      }
      window.__fetchUrls = [];
      window.__fetchBody = {
        ok: true,
        deployment: { name: "dd-remote-thread-x" },
        pods: [
          {
            name: "dd-remote-thread-x-0",
            phase: "Running",
            containers: [
              { name: "worker", ready: true, state: { running: { startedAt: "2026-05-21T12:00:00Z" } } },
            ],
          },
        ],
        summary: {
          phase: "ready",
          desiredReplicas: 1,
          availableReplicas: 1,
          readyReplicas: 1,
          podCount: 1,
          readyPodCount: 1,
        },
        errors: [],
      };
      window.fetch = async (input) => {
        const url = typeof input === 'string' ? input : input.url;
        window.__fetchUrls.push(url);
        return {
          ok: true,
          status: 200,
          json: async () => window.__fetchBody,
          text: async () => JSON.stringify(window.__fetchBody),
        };
      };
      window.__setState = (next) => Object.assign(state, next);
      window.__refresh = refreshContainerStateNow;
      window.__state = state;

      document.getElementById('container-state').addEventListener('click', refreshContainerStateNow);
      document.getElementById('container-state').addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        refreshContainerStateNow();
      });
    `;

    await page.setContent(`<!doctype html><html><body>
      <span id="container-state" class="pill warn clickable" role="button" tabindex="0">container: no thread</span>
      <script>
        ${classifierBlock}
        ${harness}
      </script>
    </body></html>`);

    const observed = await page.evaluate<{ urls: string[]; before: string; after: string }>(`(async () => {
      const win = window;
      const node = document.getElementById('container-state');
      win.__setState({ selectedThreadId: 'thread-click-target' });
      win.__fetchUrls.length = 0;
      node.click();
      const probingClass = node.className;
      const deadline = Date.now() + 2000;
      while (node.textContent === 'container: probing' && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      return {
        urls: win.__fetchUrls.slice(),
        before: probingClass,
        after: node.className,
        text: node.textContent,
      };
    })()`);

    assert.equal(observed.urls.length, 1, 'click should issue exactly one runtime probe');
    assert.equal(
      observed.urls[0],
      '/api/agents/threads/thread-click-target/runtime',
      'probe URL should target the selected thread',
    );
    assert.equal(observed.before, 'pill warn clickable probing');
    assert.equal(observed.after, 'pill clickable');

    const keyboardObserved = await page.evaluate<{ count: number }>(`(async () => {
      const win = window;
      win.__fetchUrls.length = 0;
      const node = document.getElementById('container-state');
      const settle = async () => {
        const deadline = Date.now() + 1500;
        while (node.textContent === 'container: probing' && Date.now() < deadline) {
          await new Promise((resolve) => setTimeout(resolve, 20));
        }
      };
      const enter = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
      node.dispatchEvent(enter);
      await settle();
      const space = new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true });
      node.dispatchEvent(space);
      await settle();
      const other = new KeyboardEvent('keydown', { key: 'a', bubbles: true, cancelable: true });
      node.dispatchEvent(other);
      await new Promise((resolve) => setTimeout(resolve, 30));
      return { count: win.__fetchUrls.length };
    })()`);

    assert.equal(keyboardObserved.count, 2, 'Enter and Space should each trigger one probe, others should not');

    const idleObserved = await page.evaluate<{ count: number; text: string }>(`(async () => {
      const win = window;
      win.__setState({ selectedThreadId: null });
      win.__fetchUrls.length = 0;
      const node = document.getElementById('container-state');
      node.click();
      await new Promise((resolve) => setTimeout(resolve, 30));
      return { count: win.__fetchUrls.length, text: node.textContent };
    })()`);

    assert.equal(idleObserved.count, 0, 'clicking with no selected thread should not call the runtime API');
    assert.equal(idleObserved.text, 'container: no thread');
  } finally {
    await browser.close();
  }
});

function hardeningHarness(containerStateBlock: string): string {
  return `<!doctype html><html><body>
    <span id="container-state" class="pill warn clickable" role="button" tabindex="0" aria-busy="false" aria-live="polite">container: no thread</span>
    <script>
      window.__warnings = [];
      function warnAdminDetail(label, value) {
        window.__warnings.push({ label, value: value && value.message ? value.message : String(value) });
      }
      const state = {
        selectedThreadId: null,
        lastRuntimeData: null,
        containerStatePoll: null,
        containerStatePolledThread: null,
        containerStateLastKey: "",
        containerStateRequestToken: 0,
        containerStateAbortController: null,
        containerStateFailureCount: 0,
        containerStateLastFetchAt: 0,
        containerStateLastManualAt: 0,
        containerStateVisibilityBound: false,
      };
      function $(id) { return document.getElementById(id); }
      function existingThread(id) { return null; }
      ${containerStateBlock}
      window.__state = state;
      window.__loadContainerState = loadContainerState;
      window.__refreshContainerStateNow = refreshContainerStateNow;
      window.__stopContainerStatePolling = stopContainerStatePolling;
      window.__startContainerStatePolling = startContainerStatePolling;
      window.__pollInterval = containerStatePollInterval;
      window.__abortInflight = abortInflightContainerStateFetch;
      window.__bindVisibility = bindContainerStateVisibility;
      window.__setContainerStatePill = setContainerStatePill;
      window.__classifyContainerState = classifyContainerState;

      window.__fetchUrls = [];
      window.__fetchSignals = [];
      window.__fetchPlan = [];
      window.fetch = function (input, init) {
        const url = typeof input === 'string' ? input : input.url;
        window.__fetchUrls.push(url);
        const signal = init && init.signal ? init.signal : null;
        window.__fetchSignals.push(signal);
        const plan = window.__fetchPlan.shift() || { kind: 'ok', body: { ok: true, summary: { phase: 'sleeping', desiredReplicas: 0 }, deployment: { name: 'dd-remote-thread-x' }, pods: [], errors: [] } };
        return new Promise(function (resolve, reject) {
          const finalize = function () {
            if (plan.kind === 'ok') {
              resolve({
                ok: true,
                status: 200,
                json: function () { return Promise.resolve(plan.body); },
                text: function () { return Promise.resolve(JSON.stringify(plan.body)); },
              });
              return;
            }
            if (plan.kind === 'http-error') {
              resolve({
                ok: false,
                status: plan.status || 500,
                json: function () { return Promise.resolve({}); },
                text: function () { return Promise.resolve(plan.bodyText || ''); },
              });
              return;
            }
            if (plan.kind === 'bad-json') {
              resolve({
                ok: true,
                status: 200,
                json: function () { return Promise.reject(new SyntaxError('not json')); },
                text: function () { return Promise.resolve(plan.bodyText || ''); },
              });
              return;
            }
            if (plan.kind === 'network-error') {
              reject(new TypeError(plan.message || 'Failed to fetch'));
              return;
            }
            resolve({ ok: true, status: 200, json: function () { return Promise.resolve({}); } });
          };
          const fire = function () {
            if (signal && signal.aborted) {
              const error = new DOMException('Aborted', 'AbortError');
              reject(error);
              return;
            }
            if (signal) {
              signal.addEventListener('abort', function () {
                const error = new DOMException('Aborted', 'AbortError');
                reject(error);
              }, { once: true });
            }
            finalize();
          };
          if (plan.delayMs) {
            setTimeout(fire, plan.delayMs);
          } else {
            fire();
          }
        });
      };
    </script>
  </body></html>`;
}

test('manual clicks within the debounce window collapse to a single probe', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      fetchUrls: string[];
      tokenAfter: number;
      lastFetchAtChanged: boolean;
    }>(`(async () => {
      const win = window;
      win.__state.selectedThreadId = 'thread-debounce';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      win.__state.containerStateLastFetchAt = 0;
      win.__state.containerStatePolledThread = 'thread-debounce';
      win.__fetchUrls.length = 0;
      win.__fetchPlan.length = 0;
      for (let i = 0; i < 5; i += 1) {
        win.__fetchPlan.push({
          kind: 'ok',
          body: {
            ok: true,
            summary: { phase: 'ready', desiredReplicas: 1, availableReplicas: 1, readyReplicas: 1, podCount: 1, readyPodCount: 1 },
            deployment: { name: 'dd-remote-thread-x' },
            pods: [{ name: 'p', phase: 'Running', containers: [{ name: 'worker', ready: true, state: { running: { startedAt: '2026-05-21T12:00:00Z' } } }] }],
            errors: [],
          },
        });
      }
      for (let i = 0; i < 5; i += 1) {
        win.__refreshContainerStateNow();
      }
      const deadline = Date.now() + 1500;
      while (document.getElementById('container-state').textContent === 'container: probing' && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      return {
        fetchUrls: win.__fetchUrls.slice(),
        tokenAfter: win.__state.containerStateRequestToken,
        lastFetchAtChanged: win.__state.containerStateLastFetchAt > 0,
      };
    })()`);

    assert.equal(observed.fetchUrls.length, 1, 'debounce should collapse rapid clicks to a single fetch');
    assert.equal(observed.tokenAfter, 1, 'only one request token should be issued for the burst of clicks');
    assert.equal(observed.lastFetchAtChanged, true, 'the single successful probe should record a fetch timestamp');
  } finally {
    await browser.close();
  }
});

test('network errors and HTTP failures update the pill and bump the failure counter', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      pillAfterNetwork: { text: string; className: string; title: string; busy: string };
      pillAfterHttp: { text: string; className: string; title: string; busy: string };
      pillAfterBadJson: { text: string; className: string; title: string; busy: string };
      failuresPeak: number;
      failuresAfterRecovery: number;
      pillAfterRecovery: { text: string; className: string };
    }>(`(async () => {
      const win = window;
      const node = document.getElementById('container-state');
      const snapshot = () => ({ text: node.textContent, className: node.className, title: node.title, busy: node.getAttribute('aria-busy') });
      win.__state.selectedThreadId = 'thread-errors';
      win.__state.containerStatePolledThread = 'thread-errors';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      win.__state.containerStateFailureCount = 0;

      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({ kind: 'network-error', message: 'Failed to fetch' });
      try { await win.__loadContainerState('thread-errors'); } catch (_e) {}
      const pillAfterNetwork = snapshot();

      win.__state.containerStateLastManualAt = 0;
      win.__fetchPlan.push({ kind: 'http-error', status: 503 });
      try { await win.__loadContainerState('thread-errors', { manual: true }); } catch (_e) {}
      const pillAfterHttp = snapshot();

      win.__state.containerStateLastManualAt = 0;
      win.__fetchPlan.push({ kind: 'bad-json', bodyText: 'oops' });
      try { await win.__loadContainerState('thread-errors', { manual: true }); } catch (_e) {}
      const pillAfterBadJson = snapshot();

      const failuresPeak = win.__state.containerStateFailureCount;

      win.__state.containerStateLastManualAt = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        body: {
          ok: true,
          summary: { phase: 'ready', desiredReplicas: 1, availableReplicas: 1, readyReplicas: 1, podCount: 1, readyPodCount: 1 },
          deployment: { name: 'dd-remote-thread-x' },
          pods: [{ name: 'p', phase: 'Running', containers: [{ name: 'worker', ready: true, state: { running: { startedAt: 'now' } } }] }],
          errors: [],
        },
      });
      await win.__loadContainerState('thread-errors', { manual: true });
      const failuresAfterRecovery = win.__state.containerStateFailureCount;
      const pillAfterRecovery = { text: node.textContent, className: node.className };

      return { pillAfterNetwork, pillAfterHttp, pillAfterBadJson, failuresPeak, failuresAfterRecovery, pillAfterRecovery };
    })()`);

    assert.equal(observed.pillAfterNetwork.text, 'container: probe error');
    assert.match(observed.pillAfterNetwork.className, /pill bad clickable/);
    assert.match(observed.pillAfterNetwork.title, /Failed to fetch/);
    assert.match(observed.pillAfterNetwork.title, /Click to retry\./);
    assert.equal(observed.pillAfterNetwork.busy, 'false');

    assert.equal(observed.pillAfterHttp.text, 'container: probe failed (503)');
    assert.match(observed.pillAfterHttp.className, /pill bad clickable/);
    assert.match(observed.pillAfterHttp.title, /HTTP 503/);
    assert.match(observed.pillAfterHttp.title, /\(2 consecutive failures\)/);

    assert.equal(observed.pillAfterBadJson.text, 'container: invalid response');
    assert.match(observed.pillAfterBadJson.className, /pill bad clickable/);
    assert.match(observed.pillAfterBadJson.title, /non-JSON body/);
    assert.match(observed.pillAfterBadJson.title, /\(3 consecutive failures\)/);

    assert.equal(observed.failuresPeak, 3);
    assert.equal(observed.failuresAfterRecovery, 0, 'a successful probe should reset the failure counter');
    assert.equal(observed.pillAfterRecovery.text, 'container: running');
    assert.match(observed.pillAfterRecovery.className, /pill clickable/);
  } finally {
    await browser.close();
  }
});

test('stale responses from a previous selection do not overwrite the current pill', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      pillText: string;
      lastRuntimeRunning: boolean;
      abortedFirst: boolean;
      requestTokenFinal: number;
    }>(`(async () => {
      const win = window;
      win.__state.selectedThreadId = 'thread-old';
      win.__state.containerStatePolledThread = 'thread-old';
      win.__state.containerStateRequestToken = 0;
      win.__state.lastRuntimeData = null;
      win.__fetchPlan.length = 0;
      win.__fetchUrls.length = 0;

      // First fetch: slow, would resolve as 'running' for thread-old.
      win.__fetchPlan.push({
        kind: 'ok',
        delayMs: 200,
        body: {
          ok: true,
          summary: { phase: 'ready', desiredReplicas: 1, availableReplicas: 1, readyReplicas: 1, podCount: 1, readyPodCount: 1 },
          deployment: { name: 'dd-remote-thread-old' },
          pods: [{ name: 'pod-old', phase: 'Running', containers: [{ name: 'worker', ready: true, state: { running: { startedAt: 'now' } } }] }],
          errors: [],
        },
      });
      const firstPromise = win.__loadContainerState('thread-old').catch(() => null);
      await new Promise((resolve) => setTimeout(resolve, 20));

      // Switch selection mid-flight; second fetch resolves immediately as 'suspended'.
      win.__state.selectedThreadId = 'thread-new';
      win.__state.containerStatePolledThread = 'thread-new';
      win.__state.containerStateLastManualAt = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        body: {
          ok: true,
          summary: { phase: 'sleeping', desiredReplicas: 0 },
          deployment: { name: 'dd-remote-thread-new' },
          pods: [],
          errors: [],
        },
      });
      const secondPromise = win.__loadContainerState('thread-new');

      const firstResult = await firstPromise;
      const secondResult = await secondPromise;

      const node = document.getElementById('container-state');
      return {
        pillText: node.textContent,
        lastRuntimeRunning: Boolean(win.__state.lastRuntimeData && win.__state.lastRuntimeData.deployment && win.__state.lastRuntimeData.deployment.name === 'dd-remote-thread-old'),
        abortedFirst: firstResult === null,
        requestTokenFinal: win.__state.containerStateRequestToken,
      };
    })()`);

    assert.equal(observed.pillText, 'container: suspended');
    assert.equal(observed.lastRuntimeRunning, false, 'stale fetch must not stamp lastRuntimeData with the prior thread');
    assert.equal(observed.abortedFirst, true, 'stale fetch should resolve to null (token bumped before completion)');
    assert.equal(observed.requestTokenFinal, 2);
  } finally {
    await browser.close();
  }
});

test('aria-busy toggles during probing and clears after the response settles', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      ariaBeforeProbe: string;
      ariaDuringProbe: string;
      ariaAfterProbe: string;
    }>(`(async () => {
      const win = window;
      const node = document.getElementById('container-state');
      win.__state.selectedThreadId = 'thread-aria';
      win.__state.containerStatePolledThread = 'thread-aria';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        delayMs: 80,
        body: {
          ok: true,
          summary: { phase: 'ready', desiredReplicas: 1, availableReplicas: 1, readyReplicas: 1, podCount: 1, readyPodCount: 1 },
          deployment: { name: 'dd-remote-thread-x' },
          pods: [{ name: 'pod', phase: 'Running', containers: [{ name: 'worker', ready: true, state: { running: { startedAt: 'now' } } }] }],
          errors: [],
        },
      });
      const ariaBefore = node.getAttribute('aria-busy');
      const probePromise = win.__loadContainerState('thread-aria', { manual: true });
      await new Promise((resolve) => setTimeout(resolve, 10));
      const ariaDuring = node.getAttribute('aria-busy');
      await probePromise;
      const ariaAfter = node.getAttribute('aria-busy');
      return { ariaBeforeProbe: ariaBefore, ariaDuringProbe: ariaDuring, ariaAfterProbe: ariaAfter };
    })()`);

    assert.equal(observed.ariaBeforeProbe, 'false');
    assert.equal(observed.ariaDuringProbe, 'true');
    assert.equal(observed.ariaAfterProbe, 'false');
  } finally {
    await browser.close();
  }
});

test('poll interval adapts to document visibility and failure backoff', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      idle: number;
      oneFailure: number;
      twoFailures: number;
      manyFailures: number;
      hidden: number;
    }>(`(async () => {
      const win = window;
      win.__state.containerStateFailureCount = 0;
      const idle = win.__pollInterval();
      win.__state.containerStateFailureCount = 1;
      const oneFailure = win.__pollInterval();
      win.__state.containerStateFailureCount = 2;
      const twoFailures = win.__pollInterval();
      win.__state.containerStateFailureCount = 99;
      const manyFailures = win.__pollInterval();
      win.__state.containerStateFailureCount = 0;
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
      const hidden = win.__pollInterval();
      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
      return { idle, oneFailure, twoFailures, manyFailures, hidden };
    })()`);

    assert.equal(observed.idle, 10000);
    assert.equal(observed.oneFailure, 5000);
    assert.equal(observed.twoFailures, 10000);
    assert.equal(observed.manyFailures, 60000, 'failure backoff should saturate at the configured cap');
    assert.equal(observed.hidden, 60000, 'hidden tabs should poll at the slow cadence');
  } finally {
    await browser.close();
  }
});

test('auto-polls do not flash the probing visual, but manual probes still do', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      autoPollIntermediate: string;
      autoPollFinal: string;
      manualIntermediate: string;
      manualFinal: string;
    }>(`(async () => {
      const win = window;
      const node = document.getElementById('container-state');
      win.__state.selectedThreadId = 'thread-quiet';
      win.__state.containerStatePolledThread = 'thread-quiet';
      win.__state.containerStateLastKey = 'ok|container: running|prev|0|0';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      node.textContent = 'container: running';
      node.className = 'pill clickable';
      node.setAttribute('aria-busy', 'false');

      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        delayMs: 60,
        body: {
          ok: true,
          summary: { phase: 'sleeping', desiredReplicas: 0 },
          deployment: { name: 'dd-remote-thread-x' },
          pods: [],
          errors: [],
        },
      });
      const autoPromise = win.__loadContainerState('thread-quiet');
      await new Promise((resolve) => setTimeout(resolve, 20));
      const autoPollIntermediate = node.textContent;
      await autoPromise;
      const autoPollFinal = node.textContent;

      win.__state.containerStateLastManualAt = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        delayMs: 60,
        body: {
          ok: true,
          summary: { phase: 'ready', desiredReplicas: 1, availableReplicas: 1, readyReplicas: 1, podCount: 1, readyPodCount: 1 },
          deployment: { name: 'dd-remote-thread-x' },
          pods: [{ name: 'pod', phase: 'Running', containers: [{ name: 'worker', ready: true, state: { running: { startedAt: 'now' } } }] }],
          errors: [],
        },
      });
      const manualPromise = win.__loadContainerState('thread-quiet', { manual: true });
      await new Promise((resolve) => setTimeout(resolve, 20));
      const manualIntermediate = node.textContent;
      await manualPromise;
      const manualFinal = node.textContent;

      return { autoPollIntermediate, autoPollFinal, manualIntermediate, manualFinal };
    })()`);

    assert.equal(
      observed.autoPollIntermediate,
      'container: running',
      'auto-poll must not replace the existing label with "container: probing"',
    );
    assert.equal(observed.autoPollFinal, 'container: suspended');
    assert.equal(
      observed.manualIntermediate,
      'container: probing',
      'manual probe must show the probing visual immediately',
    );
    assert.equal(observed.manualFinal, 'container: running');
  } finally {
    await browser.close();
  }
});

test('refreshContainerStateNow cancels the scheduled auto-poll before issuing the manual probe', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      timerClearedBeforeFetch: boolean;
      fetchUrls: string[];
    }>(`(async () => {
      const win = window;
      win.__state.selectedThreadId = 'thread-reset';
      win.__state.containerStatePolledThread = 'thread-reset';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      const sentinelTimerId = setTimeout(function () {}, 60000);
      win.__state.containerStatePoll = sentinelTimerId;

      win.__fetchUrls.length = 0;
      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        delayMs: 30,
        body: { ok: true, summary: { phase: 'sleeping', desiredReplicas: 0 }, deployment: { name: 'x' }, pods: [], errors: [] },
      });

      const observePollAtFetchStart = new Promise((resolve) => {
        const originalFetch = win.fetch;
        win.fetch = function (input, init) {
          win.fetch = originalFetch;
          resolve(win.__state.containerStatePoll);
          return originalFetch(input, init);
        };
      });

      win.__refreshContainerStateNow();
      const pollAtFetchStart = await observePollAtFetchStart;
      const deadline = Date.now() + 1500;
      while (document.getElementById('container-state').textContent === 'container: probing' && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      return {
        timerClearedBeforeFetch: pollAtFetchStart === null,
        fetchUrls: win.__fetchUrls.slice(),
      };
    })()`);

    assert.equal(
      observed.timerClearedBeforeFetch,
      true,
      'manual probe should clear the scheduled auto-poll BEFORE issuing the fetch',
    );
    assert.equal(observed.fetchUrls.length, 1);
  } finally {
    await browser.close();
  }
});

test('error tooltips are capped to a sane length and normalised whitespace', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      title: string;
      titleLength: number;
      endsWithEllipsis: boolean;
      collapsedWhitespace: boolean;
    }>(`(async () => {
      const win = window;
      win.__state.selectedThreadId = 'thread-cap';
      win.__state.containerStatePolledThread = 'thread-cap';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      win.__state.containerStateFailureCount = 0;
      const longMessage = 'A'.repeat(800) + '\\n\\n\\t   B';
      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({ kind: 'network-error', message: longMessage });
      try { await win.__loadContainerState('thread-cap', { manual: true }); } catch (_e) {}
      const node = document.getElementById('container-state');
      const title = node.title;
      return {
        title,
        titleLength: title.length,
        endsWithEllipsis: title.includes('…'),
        collapsedWhitespace: !title.includes('\\n') && !title.includes('\\t'),
      };
    })()`);

    assert.ok(observed.titleLength <= 240, `tooltip length should be capped, got ${observed.titleLength}`);
    assert.equal(observed.endsWithEllipsis, true, 'capped tooltip should end with an ellipsis');
    assert.equal(observed.collapsedWhitespace, true, 'tooltip should collapse newlines and tabs into single spaces');
  } finally {
    await browser.close();
  }
});

test('aria-disabled tracks whether a thread is selected', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      initial: string | null;
      afterSelection: string | null;
      afterClear: string | null;
    }>(`(async () => {
      const win = window;
      const node = document.getElementById('container-state');
      win.__state.selectedThreadId = null;
      win.__state.containerStateLastKey = "";
      win.__setContainerStatePill(null);
      const initial = node.getAttribute('aria-disabled');

      win.__state.selectedThreadId = 'thread-aria-disabled';
      win.__state.containerStateLastKey = '';
      win.__setContainerStatePill({ label: 'container: running', kind: 'ok', title: 'ready' });
      const afterSelection = node.getAttribute('aria-disabled');

      win.__state.selectedThreadId = null;
      win.__state.containerStateLastKey = '';
      win.__setContainerStatePill(null);
      const afterClear = node.getAttribute('aria-disabled');

      return { initial, afterSelection, afterClear };
    })()`);

    assert.equal(observed.initial, 'true', 'pill should advertise aria-disabled when no thread is selected');
    assert.equal(observed.afterSelection, 'false', 'pill should clear aria-disabled once a thread is selected');
    assert.equal(observed.afterClear, 'true', 'pill should re-assert aria-disabled when the selection is cleared');
  } finally {
    await browser.close();
  }
});

test('classifier tolerates null pod / null condition / null container entries from the API', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const classifierBlock = extractClassifierBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(`<!doctype html><html><body><script>
      ${classifierBlock}
      window.__classify = classifyContainerState;
    </script></body></html>`);

    const result = await page.evaluate<{ label: string; kind: string }>(`(() => {
      const malformed = {
        ok: true,
        deployment: { name: 'dd-remote-thread-x' },
        summary: { phase: 'ready', desiredReplicas: 1, availableReplicas: 1, readyReplicas: 1, podCount: 1, readyPodCount: 1 },
        pods: [
          null,
          'oops-not-an-object',
          {
            name: 'pod-good',
            phase: 'Running',
            conditions: [null, undefined, { type: 'Ready', status: 'True' }],
            initContainers: [null, undefined],
            containers: [null, { name: 'worker', ready: true, state: { running: { startedAt: 'now' } } }, undefined],
          },
        ],
        errors: [],
      };
      return window.__classify(malformed);
    })()`);

    assert.equal(result.label, 'container: running');
    assert.equal(result.kind, 'ok');
  } finally {
    await browser.close();
  }
});

test('visibility re-probe runs when the tab comes back into focus', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{ fetchUrls: string[]; bound: boolean }>(`(async () => {
      const win = window;
      win.__state.selectedThreadId = 'thread-visibility';
      win.__state.containerStatePolledThread = 'thread-visibility';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateRequestToken = 0;
      win.__state.containerStateVisibilityBound = false;
      win.__bindVisibility();

      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        body: { ok: true, summary: { phase: 'sleeping', desiredReplicas: 0 }, deployment: { name: 'x' }, pods: [], errors: [] },
      });
      win.__fetchUrls.length = 0;

      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });
      document.dispatchEvent(new Event('visibilitychange'));
      await new Promise((resolve) => setTimeout(resolve, 20));
      const hiddenFetchCount = win.__fetchUrls.length;

      Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
      document.dispatchEvent(new Event('visibilitychange'));
      await new Promise((resolve) => setTimeout(resolve, 50));
      const visibleFetchCount = win.__fetchUrls.length;

      return {
        fetchUrls: win.__fetchUrls.slice(),
        bound: win.__state.containerStateVisibilityBound,
        hiddenFetchCount,
        visibleFetchCount,
      };
    })()`);

    assert.equal(observed.bound, true);
    assert.equal(observed.fetchUrls.length, 1, 'only the visibility return should trigger a probe; the hide event should not');
    assert.equal(observed.fetchUrls[0], '/api/agents/threads/thread-visibility/runtime');
  } finally {
    await browser.close();
  }
});

test('stopContainerStatePolling clears the timer, aborts in-flight, and resets backoff', async () => {
  const server = await readWebHomeSource();
  const js = rawStringConst(server, 'AGENTS_THREADS_JS');
  const containerStateBlock = extractContainerStateBlock(js);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  try {
    await page.setContent(hardeningHarness(containerStateBlock));

    const observed = await page.evaluate<{
      signalAborted: boolean;
      pollCleared: boolean;
      polledThread: string | null;
      failureCount: number;
      fetchUrls: number;
    }>(`(async () => {
      const win = window;
      win.__state.selectedThreadId = 'thread-cleanup';
      win.__state.containerStatePolledThread = 'thread-cleanup';
      win.__state.containerStateLastManualAt = 0;
      win.__state.containerStateFailureCount = 4;
      win.__state.containerStatePoll = setTimeout(() => {}, 60000);
      win.__fetchPlan.length = 0;
      win.__fetchPlan.push({
        kind: 'ok',
        delayMs: 500,
        body: { ok: true, summary: { phase: 'ready', desiredReplicas: 1 }, deployment: { name: 'x' }, pods: [], errors: [] },
      });
      const probePromise = win.__loadContainerState('thread-cleanup').catch(() => null);
      await new Promise((resolve) => setTimeout(resolve, 30));
      const signal = win.__fetchSignals[win.__fetchSignals.length - 1];
      win.__stopContainerStatePolling();
      await probePromise;
      return {
        signalAborted: signal ? signal.aborted : false,
        pollCleared: win.__state.containerStatePoll === null,
        polledThread: win.__state.containerStatePolledThread,
        failureCount: win.__state.containerStateFailureCount,
        fetchUrls: win.__fetchUrls.length,
      };
    })()`);

    assert.equal(observed.signalAborted, true, 'in-flight fetch should be aborted via AbortController');
    assert.equal(observed.pollCleared, true);
    assert.equal(observed.polledThread, null);
    assert.equal(observed.failureCount, 0, 'stop should reset the failure counter');
    assert.equal(observed.fetchUrls, 1);
  } finally {
    await browser.close();
  }
});
