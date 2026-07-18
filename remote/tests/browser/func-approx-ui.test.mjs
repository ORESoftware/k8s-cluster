// Browser E2E — the dd-func-approx approximator UI.
//
// Drives the real, self-contained UI at
// remote/deployments/func-approx-rs/ui.html with no backend: the page's fit
// call only fires on click, so page load, the dd-data-viz config badge, the
// client-side sample generators, and the custom-JSON validation are all
// exercised hermetically. A local static server supplies ui.html and a
// fixture ui/config.json. Every scenario runs under BOTH Puppeteer and
// Playwright.
//
// Run: node --test remote/tests/browser/func-approx-ui.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import path from "node:path";

import {
  ENGINES,
  repoRoot,
  assetExists,
  startStaticServer,
  withPage,
  pollUntil,
} from "./harness.mjs";

const UI_REL = "remote/deployments/func-approx-rs/ui.html";
const hasUi = assetExists(UI_REL);
const skip = !hasUi && "func-approx-rs/ui.html not present in this checkout";

function routes(configBody) {
  return {
    "/ui.html": { file: path.join(repoRoot, UI_REL) },
    "/ui/config.json": { body: configBody, type: "application/json; charset=utf-8" },
  };
}

// Load ui.html and wait for loadConfig() to resolve (the badge stops saying
// "checking…" only after the config fetch settles).
async function openUi(page, origin) {
  await page.goto(`${origin}/ui.html`);
  await page.waitForSelector("#vizBadge");
  await pollUntil(page, () => !/checking/.test(document.getElementById("vizBadge").textContent));
  return page;
}

for (const engine of ENGINES) {
  test(`[${engine}] func-approx UI renders its shell`, { skip }, async () => {
    const server = await startStaticServer(routes("{}"));
    try {
      await withPage(engine, async (page) => {
        await openUi(page, server.origin);

        assert.match(await page.title(), /dd-func-approx/i);
        assert.equal((await page.text("h1")).trim(), "dd-func-approx");

        // The four labelled panels are all present.
        const panelHeadings = await page.evaluate(() =>
          Array.from(document.querySelectorAll(".panel h2")).map((el) => el.textContent.trim()),
        );
        for (const heading of ["Dataset & method", "Result", "Visualization", "Metrics"]) {
          assert.ok(panelHeadings.includes(heading), `expected a "${heading}" panel`);
        }

        // The example dataset selector offers all five closed forms + custom.
        const options = await page.evaluate(() =>
          Array.from(document.querySelectorAll("#example option")).map((o) => o.value),
        );
        assert.deepEqual(options, ["quadratic", "cubic", "sine", "damped", "linear", "custom"]);
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] dd-data-viz badge reflects served config`, { skip }, async () => {
    // With a configured dataVizUrl the badge shows it and the renderer unlocks.
    const server = await startStaticServer(routes(JSON.stringify({ dataVizUrl: "http://viz.local:8088" })));
    try {
      await withPage(engine, async (page) => {
        await openUi(page, server.origin);
        assert.match(await page.text("#vizBadge"), /viz\.local:8088/);
        const disabled = await page.evaluate(() => document.getElementById("rDataviz").disabled);
        assert.equal(disabled, false, "dd-data-viz renderer should be enabled when configured");
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] badge reports "not configured" without a viz URL`, { skip }, async () => {
    const server = await startStaticServer(routes("{}"));
    try {
      await withPage(engine, async (page) => {
        await openUi(page, server.origin);
        assert.match(await page.text("#vizBadge"), /not configured/i);
        const disabled = await page.evaluate(() => document.getElementById("rDataviz").disabled);
        assert.equal(disabled, true, "renderer stays disabled with no viz URL");
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] client-side sample generator is deterministic without noise`, { skip }, async () => {
    const server = await startStaticServer(routes("{}"));
    try {
      await withPage(engine, async (page) => {
        await openUi(page, server.origin);
        // sampleExample() is a top-level function in the page's classic script,
        // so it is a global callable by name from evaluate(). noise=0 =>
        // exact y = 2x²-3.
        const sample = await page.evaluate(() => sampleExample("quadratic", 0));
        assert.equal(sample.length, 41, "x in [-3, 3] step 0.15 yields 41 points");
        assert.deepEqual(sample[0].x, [-3]);
        assert.ok(Math.abs(sample[0].y - 15) < 1e-9, "2·(-3)² − 3 = 15");
        const mid = sample.find((p) => Math.abs(p.x[0]) < 1e-9);
        assert.ok(mid && Math.abs(mid.y - -3) < 1e-9, "2·0² − 3 = -3");
      });
    } finally {
      await server.close();
    }
  });

  test(`[${engine}] custom dataset with an empty box surfaces a clear error`, { skip }, async () => {
    const server = await startStaticServer(routes("{}"));
    try {
      await withPage(engine, async (page) => {
        await openUi(page, server.origin);
        await page.select("#example", "custom");
        await page.fill("#custom", "   ");
        const error = await page.evaluate(() => {
          try {
            currentSamples();
            return null;
          } catch (e) {
            return e.message;
          }
        });
        assert.ok(error, "currentSamples() must throw for an empty custom box");
        assert.match(error, /empty/i);
      });
    } finally {
      await server.close();
    }
  });
}
