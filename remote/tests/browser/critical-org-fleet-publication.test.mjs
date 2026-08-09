import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import test from "node:test";
import { chromium } from "playwright";

const SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789";
const MANIFEST_PATH = "/repository-fleets/hypesiege-streempilot.json";
const MANIFEST_URL =
  `https://raw.githubusercontent.com/ORESoftware/ai-agent-coordinator.rs/${SOURCE_SHA}` +
  MANIFEST_PATH;
const SHA = /^[0-9a-f]{40}$/;
const REPOSITORY_NAME = /^[A-Za-z0-9._-]+$/;
const EXPECTED_ORGANIZATIONS = Object.freeze({
  hypesiege: 15,
  streempilot: 17,
});
const live = process.env.CRITICAL_ORG_FLEET_LIVE === "1";
const artifactDir = path.resolve(
  process.env.FLEET_BROWSER_ARTIFACT_DIR ??
    "browser-artifacts/critical-org-fleet",
);

function validateManifest(manifest) {
  assert.ok(manifest && typeof manifest === "object");
  assert.equal(manifest.schema_version, 2);
  assert.equal(
    manifest.generator_sha256,
    "a57b00961ee57ae09bf3bb2e2d09afbdd1ddbbbde832b027802f82a1fc5dfa84",
  );
  assert.equal(manifest.repository_count, 32);
  assert.equal(manifest.total_tracked_files, 888);
  assert.equal(manifest.total_gitlinks, 30);
  assert.deepEqual(manifest.organizations, EXPECTED_ORGANIZATIONS);
  assert.ok(Array.isArray(manifest.repositories));
  assert.equal(manifest.repositories.length, 32);

  const observedOrganizations = { hypesiege: 0, streempilot: 0 };
  const identities = new Set();
  for (const record of manifest.repositories) {
    assert.ok(record && typeof record === "object");
    assert.ok(
      Object.hasOwn(EXPECTED_ORGANIZATIONS, record.org),
      `unapproved organization: ${record.org}`,
    );
    assert.equal(typeof record.name, "string");
    assert.match(record.name, REPOSITORY_NAME);
    assert.equal(typeof record.full_name, "string");
    assert.equal(
      record.full_name.toLowerCase(),
      `${record.org}/${record.name}`.toLowerCase(),
    );
    const identity = record.full_name.toLowerCase();
    assert.ok(!identities.has(identity), `duplicate repository: ${record.full_name}`);
    identities.add(identity);
    observedOrganizations[record.org] += 1;

    assert.equal(record.visibility, "public");
    assert.equal(record.default_branch, "main");
    assert.equal(typeof record.commit, "string");
    assert.match(record.commit, SHA);
    if (record.remote !== undefined) {
      assert.equal(
        record.remote.toLowerCase(),
        `https://github.com/${record.full_name}.git`.toLowerCase(),
      );
    }
  }
  assert.equal(identities.size, 32);
  assert.deepEqual(observedOrganizations, EXPECTED_ORGANIZATIONS);
  return manifest.repositories;
}

async function fetchLiveRecords() {
  const response = await fetch(MANIFEST_URL, {
    headers: {
      "user-agent": "oresoftware-critical-org-browser-canary",
    },
    redirect: "follow",
    signal: AbortSignal.timeout(30_000),
  });
  assert.equal(
    response.status,
    200,
    `pinned manifest returned HTTP ${response.status}`,
  );
  const responseUrl = new URL(response.url);
  assert.equal(responseUrl.protocol, "https:");
  assert.equal(responseUrl.hostname, "raw.githubusercontent.com");
  assert.equal(
    responseUrl.pathname,
    `/ORESoftware/ai-agent-coordinator.rs/${SOURCE_SHA}${MANIFEST_PATH}`,
  );
  return validateManifest(await response.json());
}

async function startFixture(records) {
  const approved = new Map(
    records.map((record) => [
      `/${record.full_name}/tree/${record.commit}`.toLowerCase(),
      record,
    ]),
  );
  const server = createServer((request, response) => {
    const requestPath = new URL(
      request.url ?? "/",
      "http://fixture.invalid",
    ).pathname;
    const record = approved.get(requestPath.toLowerCase());
    if (!record) {
      response.writeHead(404, {
        "content-type": "text/html; charset=utf-8",
      });
      response.end(
        "<!doctype html><title>Page not found</title><h1>Page not found</h1>",
      );
      return;
    }
    response.writeHead(200, {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
    });
    response.end(`<!doctype html>
      <html><head>
        <title>GitHub - ${record.full_name} at ${record.commit}</title>
        <meta name="octolytics-dimension-repository_nwo" content="${record.full_name}">
      </head><body>
        <main><h1>${record.full_name}</h1><code>${record.commit}</code></main>
      </body></html>`);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert.ok(address && typeof address === "object");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

async function inspectRepository(page, baseUrl, record) {
  const target = `${baseUrl}/${record.full_name}/tree/${record.commit}`;
  const response = await page.goto(target, {
    waitUntil: "domcontentloaded",
    timeout: 60_000,
  });
  assert.ok(response, `${record.full_name}: browser returned no response`);
  assert.equal(
    response.status(),
    200,
    `${record.full_name}: HTTP ${response.status()}`,
  );

  const finalUrl = new URL(page.url());
  assert.equal(finalUrl.origin, new URL(baseUrl).origin);
  assert.equal(
    finalUrl.pathname.replace(/\/$/, "").toLowerCase(),
    `/${record.full_name}/tree/${record.commit}`.toLowerCase(),
    `${record.full_name}: browser did not remain on the approved commit`,
  );
  const repositoryNwo = await page
    .locator('meta[name="octolytics-dimension-repository_nwo"]')
    .getAttribute("content");
  assert.equal(repositoryNwo?.toLowerCase(), record.full_name.toLowerCase());
  assert.ok(
    (await page.title()).toLowerCase().includes(record.name.toLowerCase()),
    `${record.full_name}: page title does not identify the repository`,
  );
  const body = (await page.locator("body").innerText()).toLowerCase();
  assert.doesNotMatch(body, /page not found|repository unavailable/);
  return {
    repository: record.full_name,
    commit: record.commit,
    status: response.status(),
    final_url: finalUrl.href,
  };
}

const fixtureManifest = {
  schema_version: 2,
  generator_sha256:
    "a57b00961ee57ae09bf3bb2e2d09afbdd1ddbbbde832b027802f82a1fc5dfa84",
  repository_count: 32,
  total_tracked_files: 888,
  total_gitlinks: 30,
  organizations: { hypesiege: 15, streempilot: 17 },
  repositories: [
    ...Array.from({ length: 15 }, (_, index) => ({
      org: "hypesiege",
      name: `fixture-${index + 1}`,
      full_name: `hypesiege/fixture-${index + 1}`,
      visibility: "public",
      default_branch: "main",
      commit: `${index + 1}`.padStart(40, "0"),
      remote: `https://github.com/hypesiege/fixture-${index + 1}.git`,
    })),
    ...Array.from({ length: 17 }, (_, index) => ({
      org: "streempilot",
      name: `fixture-${index + 1}`,
      full_name: `streempilot/fixture-${index + 1}`,
      visibility: "public",
      default_branch: "main",
      commit: `${index + 16}`.padStart(40, "0"),
      remote: `https://github.com/streempilot/fixture-${index + 1}.git`,
    })),
  ],
};
const fixtureRecords = validateManifest(fixtureManifest);

test(
  "browser canary verifies the approved fleet at exact commits",
  { timeout: 900_000 },
  async () => {
    await mkdir(artifactDir, { recursive: true, mode: 0o700 });
    const records = live ? await fetchLiveRecords() : fixtureRecords;
    const fixture = live ? null : await startFixture(records);
    const baseUrl = live ? "https://github.com" : fixture.origin;
    const browser = await chromium.launch({
      headless: true,
      args: ["--no-sandbox", "--disable-dev-shm-usage"],
    });
    const results = [];
    try {
      const context = await browser.newContext({
        ignoreHTTPSErrors: false,
        userAgent: "oresoftware-critical-org-browser-canary/1.0",
      });
      const page = await context.newPage();
      for (const record of records) {
        results.push(await inspectRepository(page, baseUrl, record));
      }
      await page.screenshot({
        path: path.join(
          artifactDir,
          live ? "live-last-repository.png" : "fixture-last-repository.png",
        ),
        fullPage: true,
      });
      await context.close();
    } finally {
      await browser.close();
      if (fixture) await fixture.close();
    }
    assert.equal(results.length, 32);
    await writeFile(
      path.join(
        artifactDir,
        live ? "live-results.json" : "fixture-results.json",
      ),
      `${JSON.stringify(
        {
          live,
          source_sha: SOURCE_SHA,
          repositories: results,
        },
        null,
        2,
      )}\n`,
      { mode: 0o600 },
    );
  },
);

test(
  "manifest validation rejects visibility, inventory, and name drift",
  { skip: live },
  () => {
    const privateMutation = structuredClone(fixtureManifest);
    privateMutation.repositories[0].visibility = "private";
    assert.throws(() => validateManifest(privateMutation));

    const inventoryMutation = structuredClone(fixtureManifest);
    inventoryMutation.repositories[0].org = "streempilot";
    inventoryMutation.repositories[0].name = "moved-from-hypesiege";
    inventoryMutation.repositories[0].full_name =
      "streempilot/moved-from-hypesiege";
    inventoryMutation.repositories[0].remote =
      "https://github.com/streempilot/moved-from-hypesiege.git";
    assert.throws(() => validateManifest(inventoryMutation));

    const nameMutation = structuredClone(fixtureManifest);
    nameMutation.repositories[0].name = "nested/repository";
    nameMutation.repositories[0].full_name = "hypesiege/nested/repository";
    assert.throws(() => validateManifest(nameMutation));
  },
);

test(
  "browser canary fails closed on an unapproved or missing commit",
  { skip: live },
  async () => {
    const fixture = await startFixture(fixtureRecords.slice(0, 1));
    const browser = await chromium.launch({
      headless: true,
      args: ["--no-sandbox"],
    });
    try {
      const page = await browser.newPage();
      await assert.rejects(
        inspectRepository(page, fixture.origin, fixtureRecords[1]),
        /HTTP 404/,
      );
    } finally {
      await browser.close();
      await fixture.close();
    }
  },
);
