// Browser E2E — the offline draft cache service worker.
//
// Exercises remote/libs/browser/service-worker.js (the `dd-browser-drafts`
// worker used by the lambda draft UI) through its real postMessage protocol:
// save / load / delete plus the two error paths (missing key, unknown message
// type). Every scenario runs under BOTH Puppeteer and Playwright.
//
// Run: node --test remote/tests/browser/service-worker.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";

import {
  ENGINES,
  repoRoot,
  fixturesDir,
  assetExists,
  startStaticServer,
  withPage,
} from "./harness.mjs";

const SW_REL = "remote/libs/browser/service-worker.js";
const hasWorker = assetExists(SW_REL);

// The worker lives in the remote/libs submodule; if it isn't checked out we
// skip loudly rather than fail, so a shallow checkout doesn't red-X the suite.
const swRoutes = {
  "/sw-host.html": { file: path.join(fixturesDir, "sw-host.html") },
  "/service-worker.js": { file: path.join(repoRoot, SW_REL), type: "text/javascript; charset=utf-8" },
};

// Load the host page and wait for the worker to reach `active`.
async function openHost(page, origin) {
  await page.goto(`${origin}/sw-host.html`);
  const active = await page.evaluate(() => window.__swReady);
  assert.equal(active, true, "service worker should reach an active state");
  return page;
}

const call = (page, type, payload) =>
  page.evaluate(([t, p]) => window.ddDraftCall(t, p), [type, payload ?? {}]);

for (const engine of ENGINES) {
  test(`[${engine}] service worker registers and activates`, { skip: !hasWorker && "remote/libs submodule not checked out" }, async () => {
    const server = await startStaticServer(swRoutes);
    try {
      await withPage(engine, async (page) => {
        await openHost(page, server.origin);
        assert.equal(await page.text("#status"), "sw-ready");
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] save then load round-trips a draft record`, { skip: !hasWorker && "remote/libs submodule not checked out" }, async () => {
    const server = await startStaticServer(swRoutes);
    try {
      await withPage(engine, async (page) => {
        await openHost(page, server.origin);
        const record = { title: "draft-1", body: "hello", tags: ["a", "b"], n: 42 };
        const saved = await call(page, "dd-lambda-draft-save", { key: "task/42", record });
        assert.deepEqual(saved, { ok: true });

        const loaded = await call(page, "dd-lambda-draft-load", { key: "task/42" });
        assert.equal(loaded.ok, true);
        assert.deepEqual(loaded.record, record, "loaded record must equal what was saved");
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] loading an unknown key yields a null record`, { skip: !hasWorker && "remote/libs submodule not checked out" }, async () => {
    const server = await startStaticServer(swRoutes);
    try {
      await withPage(engine, async (page) => {
        await openHost(page, server.origin);
        const loaded = await call(page, "dd-lambda-draft-load", { key: "does/not/exist" });
        assert.deepEqual(loaded, { ok: true, record: null });
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] delete removes a saved draft`, { skip: !hasWorker && "remote/libs submodule not checked out" }, async () => {
    const server = await startStaticServer(swRoutes);
    try {
      await withPage(engine, async (page) => {
        await openHost(page, server.origin);
        await call(page, "dd-lambda-draft-save", { key: "task/del", record: { keep: false } });

        const deleted = await call(page, "dd-lambda-draft-delete", { key: "task/del" });
        assert.deepEqual(deleted, { ok: true });

        const afterDelete = await call(page, "dd-lambda-draft-load", { key: "task/del" });
        assert.deepEqual(afterDelete, { ok: true, record: null }, "deleted draft must be gone");
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] a missing key is rejected with an error`, { skip: !hasWorker && "remote/libs submodule not checked out" }, async () => {
    const server = await startStaticServer(swRoutes);
    try {
      await withPage(engine, async (page) => {
        await openHost(page, server.origin);
        const result = await call(page, "dd-lambda-draft-save", { key: "", record: { x: 1 } });
        assert.equal(result.ok, false);
        assert.match(result.error, /key is required/i);
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] an unsupported message type is rejected`, { skip: !hasWorker && "remote/libs submodule not checked out" }, async () => {
    const server = await startStaticServer(swRoutes);
    try {
      await withPage(engine, async (page) => {
        await openHost(page, server.origin);
        const result = await call(page, "dd-lambda-draft-bogus", { key: "task/1" });
        assert.equal(result.ok, false);
        assert.match(result.error, /unsupported/i);
      });
    } finally {
      await server.close();
    }
  });
}
