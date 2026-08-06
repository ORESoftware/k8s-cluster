import assert from "node:assert/strict";
import { test } from "node:test";
import { chromium } from "playwright";
import { chromeExecutablePath, startServer } from "./app-browser-harness.mjs";

// Playwright half of the web-server browser e2e: page-shell wiring, the JSON
// health/info API, and the per-route-class security headers. Runs the same
// SQLite-backed `serve` process as the puppeteer spec.

const TEST_AUTH_EMAIL = "browser-e2e@canonical.invalid";
const TEST_AUTH_PASSWORD = "browser-e2e-only";

async function withPage(t) {
  const server = await startServer();
  t.after(() => server.stop());

  const browser = await chromium.launch({
    executablePath: chromeExecutablePath(),
    headless: true,
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });
  t.after(() => browser.close());

  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  return { server, page, pageErrors };
}

async function signIn(page, server) {
  await page.goto(`${server.url}/login`, { waitUntil: "networkidle" });
  await page.locator('input[name="email"]').fill(TEST_AUTH_EMAIL);
  await page.locator('input[name="password"]').fill(TEST_AUTH_PASSWORD);

  const loginResponsePromise = page.waitForResponse(
    (response) =>
      response.url() === `${server.url}/auth/login` &&
      response.request().method() === "POST",
  );
  await page.locator('form[action="/auth/login"] button[type="submit"]').click();
  const loginResponse = await loginResponsePromise;
  assert.equal(loginResponse.status(), 200);
  await page.waitForURL(`${server.url}/app`);
  await page.locator("main h1").waitFor({ state: "visible" });

  const requestHeaders = await loginResponse.request().allHeaders();
  assert.equal(requestHeaders["hx-request"], "true");
  assert.equal(requestHeaders.origin, server.url);

  const sessionCookie = (await page.context().cookies(server.url)).find(
    (cookie) => cookie.name === "canonical_session",
  );
  assert.ok(sessionCookie);
  assert.ok(sessionCookie.value.length >= 32);
  assert.equal(sessionCookie.httpOnly, true);
  assert.equal(sessionCookie.sameSite, "Lax");
  assert.equal(await page.evaluate(() => document.cookie.includes("canonical_session")), false);
}

function trackWebSockets(page) {
  const sockets = [];
  const messages = [];
  const events = [];
  let sequence = 0;
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (
      request.method() === "GET" &&
      url.pathname === "/api/v1/sync/changes"
    ) {
      events.push({ sequence: sequence++, kind: "pull", url: request.url() });
    }
  });
  page.on("websocket", (socket) => {
    sockets.push(socket);
    socket.on("framereceived", ({ payload }) => {
      try {
        const message = JSON.parse(String(payload));
        messages.push(message);
        events.push({
          sequence: sequence++,
          kind: "websocket",
          type: message.type,
        });
      } catch {
        // Ping/pong and malformed frames are not application messages.
      }
    });
  });
  return { sockets, messages, events };
}

async function waitUntil(predicate, description, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for ${description}`);
}

test("playwright: login shell wires htmx, the CSRF meta, and the client bundle without page errors", async (t) => {
  const { server, page, pageErrors } = await withPage(t);

  await page.goto(`${server.url}/login`, { waitUntil: "networkidle" });

  // Page-shell metadata rendered by the maud layout.
  assert.equal(
    await page.locator('meta[name="color-scheme"]').getAttribute("content"),
    "light dark",
  );
  // The login form carries a CSRF token in a hidden field (the layout only
  // emits a csrf <meta> for authenticated pages, so assert the field here).
  assert.ok(
    (await page.locator('form input[name="csrf"]').getAttribute("value"))?.length > 0,
  );

  // The shell references the ES-module client bundle and the /app-assets mount
  // exists (a missing asset 404s instead of falling through to marketing HTML).
  assert.equal(
    await page.locator('script[type="module"]').getAttribute("src"),
    "/app-assets/app.js",
  );
  const missingAsset = await page.request.get(`${server.url}/app-assets/not-a-real-asset.js`);
  assert.equal(missingAsset.status(), 404);

  // htmx target slot for the async login result.
  await page.locator("#login-result").waitFor({ state: "attached" });
  assert.deepEqual(pageErrors, []);
});

test("playwright: the versioned JSON API reports health and the service info contract", async (t) => {
  const { server, page } = await withPage(t);

  const healthz = await page.request.get(`${server.url}/healthz`);
  assert.equal(healthz.status(), 200);

  const readyz = await page.request.get(`${server.url}/readyz`);
  assert.equal(readyz.status(), 200); // SQLite ping succeeds

  const health = await page.request.get(`${server.url}/api/v1/health`);
  assert.equal(health.status(), 200);
  assert.deepEqual(await health.json(), { status: "ok", service: "canonical-web-server" });

  const info = await page.request.get(`${server.url}/api/v1/info`);
  assert.equal(info.status(), 200);
  const body = await info.json();
  assert.equal(body.service, "canonical-web-server");
  assert.equal(body.domain, "canonical.cloud");
  assert.deepEqual(body.stack, ["supabase", "maud", "axum", "seaorm", "htmx"]);

  // Unknown API paths stay JSON and never fall into the marketing site.
  const missing = await page.request.get(`${server.url}/api/v1/missing`);
  assert.equal(missing.status(), 404);
  assert.equal((await missing.json()).error.code, "not_found");
});

test("playwright: application and marketing routes carry their tailored security headers", async (t) => {
  const { server, page } = await withPage(t);

  const appResponse = await page.request.get(`${server.url}/login`);
  const appHeaders = appResponse.headers();
  const marketingResponse = await page.request.get(`${server.url}/`);
  const marketingHeaders = marketingResponse.headers();

  // Global hardening applied to every response.
  for (const headers of [appHeaders, marketingHeaders]) {
    assert.equal(headers["x-content-type-options"], "nosniff");
    assert.equal(headers["x-frame-options"], "DENY");
    assert.match(headers["referrer-policy"], /strict-origin/);
  }

  // Both surfaces use same-origin external scripts. The marketing build has a
  // contract test preventing Astro regressions back to inline executable code.
  assert.match(appHeaders["content-security-policy"], /script-src 'self';/);
  assert.match(appHeaders["content-security-policy"], /connect-src 'self'/);
  assert.match(marketingHeaders["content-security-policy"], /script-src 'self';/);
  assert.doesNotMatch(appHeaders["content-security-policy"], /script-src 'self' 'unsafe-inline'/);
  assert.doesNotMatch(marketingHeaders["content-security-policy"], /script-src 'self' 'unsafe-inline'/);
});

test("playwright: authenticated session and CSRF drive the engagement lifecycle through htmx", async (t) => {
  const { server, page, pageErrors } = await withPage(t);
  await signIn(page, server);

  assert.equal(await page.locator("main h1").textContent(), "Application");
  assert.match(await page.locator("main").innerText(), new RegExp(TEST_AUTH_EMAIL));
  await page.locator("#session-fragment").getByText("HTMX connected").waitFor();

  // A cookie-authenticated unsafe request with an invalid CSRF header must be
  // rejected before any mutation is applied.
  const rejectedRecordId = "11111111-1111-4111-8111-111111111111";
  const invalidCsrfStatus = await page.evaluate(async (recordId) => {
    const response = await fetch("/api/v1/sync/mutations", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-csrf-token": "deliberately-invalid",
      },
      body: JSON.stringify({
        protocolVersion: 1,
        clientId: "22222222-2222-4222-8222-222222222222",
        operations: [
          {
            mutationId: "33333333-3333-4333-8333-333333333333",
            key: { kind: "draft_note", id: recordId },
            action: "put",
            baseVersion: null,
            schemaVersion: 1,
            value: { title: "must not be written", body: "" },
          },
        ],
      }),
    });
    return response.status;
  }, rejectedRecordId);
  assert.equal(invalidCsrfStatus, 403);
  const rejectedPull = await page.evaluate(async () => {
    const response = await fetch("/api/v1/sync/changes?limit=500");
    return { status: response.status, body: await response.json() };
  });
  assert.equal(rejectedPull.status, 200);
  assert.equal(
    rejectedPull.body.changes.some((change) => change.key.id === rejectedRecordId),
    false,
  );
  await page.waitForFunction(() => window.canonicalSync !== undefined);
  assert.equal(
    await page.evaluate(
      (recordId) => window.canonicalSync.store.getRecord(recordId),
      rejectedRecordId,
    ),
    undefined,
  );

  await page.goto(`${server.url}/app/engagements`, { waitUntil: "networkidle" });
  const csrf = await page.locator('meta[name="csrf-token"]').getAttribute("content");
  assert.ok(csrf?.length > 20);

  await page.locator('input[name="company"]').fill("Browser Audit Co");
  await page.locator('select[name="framework"]').selectOption("soc2");
  await page.locator('input[name="target_report_date"]').fill("2026-12-31");
  const createResponsePromise = page.waitForResponse(
    (response) =>
      response.url() === `${server.url}/app/engagements` &&
      response.request().method() === "POST",
  );
  await page.locator('form[action="/app/engagements"] button[type="submit"]').click();
  const createResponse = await createResponsePromise;
  assert.equal(createResponse.status(), 200);
  const createHeaders = await createResponse.request().allHeaders();
  assert.equal(createHeaders["hx-request"], "true");
  assert.equal(createHeaders["x-csrf-token"], csrf);
  await page.locator("#engagement-list").getByText("Browser Audit Co").waitFor();

  await page.locator("#engagement-list a", { hasText: "Browser Audit Co" }).click();
  await page.waitForURL(/\/app\/engagements\/[0-9a-f-]+$/);
  assert.equal(await page.locator("#engagement-status h2").textContent(), "Status: Scoping");
  assert.match(await page.locator("#note-list").innerText(), /No notes yet/);

  await page.locator('#engagement-status select[name="status"]').selectOption("in_audit");
  const statusResponsePromise = page.waitForResponse(
    (response) =>
      response.url().endsWith("/status") && response.request().method() === "POST",
  );
  await page.locator('#engagement-status button[type="submit"]').click();
  assert.equal((await statusResponsePromise).status(), 200);
  await page.locator("#engagement-status h2").getByText("Status: In audit").waitFor();

  await page.locator('textarea[name="body"]').fill("Browser-confirmed kickoff note");
  const noteResponsePromise = page.waitForResponse(
    (response) =>
      response.url().endsWith("/notes") && response.request().method() === "POST",
  );
  await page.locator('form[hx-target="#note-list"] button[type="submit"]').click();
  assert.equal((await noteResponsePromise).status(), 200);
  await page.locator("#note-list").getByText("Browser-confirmed kickoff note").waitFor();

  assert.deepEqual(pageErrors, []);
});

test("playwright: IndexedDB is optimistic and the authenticated WebSocket wakes and reconnects", async (t) => {
  const { server, page, pageErrors } = await withPage(t);
  const websocket = trackWebSockets(page);
  await page.addInitScript(() => {
    const NativeWebSocket = window.WebSocket;
    const state = { created: 0, opened: 0, closed: 0, last: null };
    class TrackedWebSocket extends NativeWebSocket {
      constructor(...args) {
        super(...args);
        state.created += 1;
        state.last = this;
        this.addEventListener("open", () => {
          state.opened += 1;
        });
        this.addEventListener("close", () => {
          state.closed += 1;
        });
      }
    }
    window.WebSocket = TrackedWebSocket;
    window.__canonicalE2eSocketState = state;
    window.__canonicalE2eCloseSocket = () => state.last?.close(4000, "e2e reconnect");
  });

  await signIn(page, server);
  await page.waitForFunction(() => window.canonicalSync !== undefined);
  await waitUntil(
    () => websocket.messages.some((message) => message.type === "hello"),
    "the authenticated WebSocket hello",
  );

  // Save while offline: the UI and IndexedDB update before the network can.
  await page.context().setOffline(true);
  await page.locator('form[data-sync-form="draft_note"] input[name="title"]').fill("Offline-first draft");
  await page.locator('form[data-sync-form="draft_note"] textarea[name="body"]').fill("Queued in IndexedDB");
  await page.locator('form[data-sync-form="draft_note"] button[type="submit"]').click();
  await page.locator('[data-sync-list="draft_note"] h3').getByText("Offline-first draft").waitFor();
  await page
    .locator('[data-sync-list="draft_note"] small')
    .getByText("Saved locally; sync pending")
    .waitFor();

  const optimistic = await page.evaluate(async () => {
    const outbox = await window.canonicalSync.store.listOutbox();
    const notes = await window.canonicalSync.store.listEffectiveDraftNotes();
    return {
      outbox: outbox.length,
      pending: notes.find((note) => note.value.title === "Offline-first draft")?.pending,
    };
  });
  assert.deepEqual(optimistic, { outbox: 1, pending: true });

  const firstPushPromise = page.waitForResponse(
    (response) =>
      response.url() === `${server.url}/api/v1/sync/mutations` &&
      response.request().method() === "POST",
  );
  await page.context().setOffline(false);
  await page.evaluate(() => window.canonicalSync.syncNow());
  assert.equal((await firstPushPromise).status(), 200);
  await page.waitForFunction(async () => {
    const outbox = await window.canonicalSync.store.listOutbox();
    const notes = await window.canonicalSync.store.listEffectiveDraftNotes();
    return (
      outbox.length === 0 &&
      notes.some(
        (note) =>
          note.value.title === "Offline-first draft" &&
          note.pending === false &&
          note.version === "1",
      )
    );
  });

  await page.waitForFunction(
    () =>
      window.__canonicalE2eSocketState.last?.readyState === window.WebSocket.OPEN,
  );
  await page.evaluate(() => window.canonicalSync.syncNow());
  const invalidationsBeforeWake = websocket.events.filter(
    (event) => event.kind === "websocket" && event.type === "sync.invalidated",
  ).length;
  const externalRecordId = "44444444-4444-4444-8444-444444444444";
  const csrf = await page.locator('meta[name="csrf-token"]').getAttribute("content");
  assert.ok(csrf?.length > 20);
  const externalMutationStatus = await page.evaluate(
    async ({ csrfToken, recordId }) => {
      const response = await fetch("/api/v1/sync/mutations", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-csrf-token": csrfToken,
        },
        body: JSON.stringify({
          protocolVersion: 1,
          clientId: "55555555-5555-4555-8555-555555555555",
          operations: [
            {
              mutationId: "66666666-6666-4666-8666-666666666666",
              key: { kind: "draft_note", id: recordId },
              action: "put",
              baseVersion: null,
              schemaVersion: 1,
              value: {
                title: "Externally committed wake",
                body: "Only the WebSocket can wake the browser pull here.",
              },
            },
          ],
        }),
      });
      return response.status;
    },
    { csrfToken: csrf, recordId: externalRecordId },
  );
  assert.equal(externalMutationStatus, 200);
  await waitUntil(
    () =>
      websocket.events.filter(
        (event) =>
          event.kind === "websocket" && event.type === "sync.invalidated",
      ).length > invalidationsBeforeWake,
    "the WebSocket sync invalidation wake",
  );
  const wakeEvent = websocket.events.findLast(
    (event) =>
      event.kind === "websocket" && event.type === "sync.invalidated",
  );
  assert.ok(wakeEvent);
  await waitUntil(
    () =>
      websocket.events.some(
        (event) =>
          event.kind === "pull" && event.sequence > wakeEvent.sequence,
      ),
    "a REST pull triggered after the WebSocket invalidation",
    5_000,
  );
  await page.waitForFunction(
    async (recordId) => {
      const record = await window.canonicalSync.store.getRecord(recordId);
      return (
        record?.state === "synced" &&
        record.confirmed?.value?.title === "Externally committed wake"
      );
    },
    externalRecordId,
  );

  // Close the authenticated connection with a restart-style code. The htmx
  // websocket extension must establish a fresh cookie-authenticated socket.
  const socketsBeforeReconnect = websocket.sockets.length;
  await page.evaluate(() => window.__canonicalE2eCloseSocket());
  await waitUntil(
    () => websocket.sockets.length > socketsBeforeReconnect,
    "the WebSocket reconnect",
  );
  await waitUntil(
    () =>
      websocket.messages.filter((message) => message.type === "hello").length >= 2,
    "the reconnected WebSocket hello",
  );
  const socketState = await page.evaluate(() => window.__canonicalE2eSocketState);
  assert.ok(socketState.created >= 2);
  assert.ok(socketState.opened >= 2);
  assert.ok(socketState.closed >= 1);

  assert.deepEqual(pageErrors, []);
});
