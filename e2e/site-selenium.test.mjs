import assert from "node:assert/strict";
import { test } from "node:test";
import { Builder, By, until } from "selenium-webdriver";
import { startServer } from "./harness.mjs";

// Selenium driver — verification against a REMOTE Selenium grid (the fleet runs
// a dd-selenium-server in k8s-cluster). Set T2V_SELENIUM_REMOTE_URL to the grid
// hub (e.g. http://127.0.0.1:4444/wd/hub). Skipped when unset so the default
// `node --test` run needs no grid, matching the other fleet e2e suites.
//
// When running against a remote grid, the server under test must be reachable
// from the grid — set T2V_WEB_TEST_URL to a URL the grid can hit (the harness
// then reuses it instead of booting a loopback-only binary).
const gridUrl = process.env.T2V_SELENIUM_REMOTE_URL;

test(
  "selenium (remote grid): dashboard renders and htmx executes",
  { skip: gridUrl ? false : "set T2V_SELENIUM_REMOTE_URL to run the remote Selenium check" },
  async (t) => {
    const server = await startServer();
    t.after(() => server.stop());

    const driver = await new Builder()
      .usingServer(gridUrl)
      .forBrowser("chrome")
      .build();
    t.after(() => driver.quit());

    await driver.get(`${server.url}/`);
    assert.match(await driver.getTitle(), /t2v/i);

    // Hero heading present.
    await driver.wait(until.elementLocated(By.css(".hero h1")), 10000);

    // Four unique stat cards.
    for (const id of ["stat-transcriptions", "stat-translations", "stat-syntheses", "stat-vapi"]) {
      const nodes = await driver.findElements(By.id(id));
      assert.equal(nodes.length, 1, `#${id} must be a single node`);
    }

    // Vendored htmx executed under the CSP.
    const htmxVersion = await driver.executeScript("return globalThis.htmx?.version");
    assert.ok(htmxVersion, "htmx global should be defined (script executed under CSP)");
  },
);
