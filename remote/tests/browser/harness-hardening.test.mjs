import assert from "node:assert/strict";
import { mkdtemp, readdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { ENGINES, startStaticServer, withPage } from "./harness.mjs";

const eventFixture = `<!doctype html>
<html>
  <head><meta charset="utf-8"><title>harness fixture</title></head>
  <body>
    <input id="value" value="initial">
    <script>
      window.events = [];
      for (const type of ["input", "change"]) {
        document.getElementById("value").addEventListener(type, () => window.events.push(type));
      }
    </script>
  </body>
</html>`;

for (const engine of ENGINES) {
  test(`[${engine}] fill mirrors real user input/change semantics`, async () => {
    const server = await startStaticServer({
      "/": { body: eventFixture, type: "text/html; charset=utf-8" },
    });
    try {
      await withPage(engine, async (page) => {
        await page.goto(`${server.origin}/`);
        await page.fill("#value", "updated");
        const state = await page.evaluate(() => ({
          events: window.events,
          value: document.getElementById("value").value,
        }));
        assert.equal(state.value, "updated");
        assert.deepEqual(state.events, ["input", "change"]);
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] hermetic mode blocks and records external requests`, async () => {
    const server = await startStaticServer({
      "/": { body: "<!doctype html><title>network fixture</title>", type: "text/html; charset=utf-8" },
    });
    try {
      await withPage(engine, async (page) => {
        await page.goto(`${server.origin}/`);
        const result = await page.evaluate(async () => {
          try {
            await fetch("https://example.com/browser-e2e-should-not-leave-loopback");
            return "resolved";
          } catch {
            return "rejected";
          }
        });
        assert.equal(result, "rejected");
        assert.ok(
          page.diagnostics.blockedRequests.some((url) => url.startsWith("https://example.com/")),
          "the harness must record the blocked external URL",
        );
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] failures retain screenshot and diagnostic artifacts`, async () => {
    const artifactDir = await mkdtemp(path.join(os.tmpdir(), "dd-browser-artifacts-"));
    const server = await startStaticServer({
      "/": { body: "<!doctype html><title>artifact fixture</title>", type: "text/html; charset=utf-8" },
    });
    try {
      await assert.rejects(
        withPage(
          engine,
          async (page) => {
            await page.goto(`${server.origin}/`);
            throw new Error("intentional artifact capture failure");
          },
          { artifactDir, artifactName: "harness-hardening" },
        ),
        /intentional artifact capture failure/,
      );
      const files = await readdir(artifactDir);
      assert.ok(files.some((file) => file.endsWith(".png")), "expected a failure screenshot");
      assert.ok(files.some((file) => file.endsWith(".json")), "expected browser diagnostics JSON");
      if (engine === "playwright") {
        assert.ok(files.some((file) => file.endsWith(".zip")), "expected a Playwright trace ZIP");
      }
    } finally {
      await server.close();
      await rm(artifactDir, { force: true, recursive: true });
    }
  });
}

test("static server accepts HEAD and rejects state-changing methods", async () => {
  const server = await startStaticServer({
    "/health": { body: "ok", type: "text/plain; charset=utf-8" },
  });
  try {
    const head = await fetch(`${server.origin}/health`, { method: "HEAD" });
    assert.equal(head.status, 200);
    assert.equal(await head.text(), "");
    assert.equal(head.headers.get("x-content-type-options"), "nosniff");

    const post = await fetch(`${server.origin}/health`, { method: "POST", body: "mutation" });
    assert.equal(post.status, 405);
    assert.equal(post.headers.get("allow"), "GET, HEAD");
  } finally {
    await server.close();
  }
});
