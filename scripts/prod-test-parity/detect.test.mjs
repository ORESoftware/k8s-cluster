import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  EXPECTED_PATH,
  ROOT,
  alignedObservation,
  detect,
  formatMarkdown,
  ghApiClient,
  inferRole,
  loadExpected,
  observeFromGitHub,
  stableStringify,
} from "./detect.mjs";

test("expected snapshot covers at least ten pairs and names a policy version", () => {
  const expected = loadExpected(EXPECTED_PATH);
  assert.equal(expected.schemaVersion, 1);
  assert.equal(expected.policyVersion, "den-3444.v1");
  assert.ok(expected.pairs.length >= 10);
  assert.ok(expected.exceptions.some((item) => item.kind === "orphan-test-org"));
  assert.match(stableStringify(expected), /embedded-alerts-test/);
  assert.doesNotMatch(stableStringify(expected), /ghp_|lin_api_|github_pat_/);
});

test("aligned fixture is byte-stable and drift-free", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  const first = detect({ expected, observed, observedAt: observed.observedAt });
  const second = detect({ expected, observed, observedAt: observed.observedAt });
  assert.equal(first.drift.length, 0);
  assert.equal(first.unknown.length, 0);
  assert.equal(stableStringify(first), stableStringify(second));
  assert.equal(first.observedAt, "2026-08-26T00:00:00Z");
});

test("missing GitHub access is unknown, never an empty organization", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  observed.orgs["embedded-alerts"] = { status: "unknown" };
  delete observed.orgs["embedded-alerts-test"];
  const report = detect({ expected, observed, observedAt: observed.observedAt });
  assert.ok(report.unknown.some((item) => item.org === "embedded-alerts"));
  assert.ok(report.unknown.some((item) => item.org === "embedded-alerts-test"));
  assert.ok(!report.drift.some((item) => item.productionOrg === "embedded-alerts"));
});

test("orphan test repositories and setting mismatches are classified", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  observed.orgs["embedded-alerts-test"].repos.push({
    archived: false,
    defaultBranch: "main",
    name: "unclaimed-canary",
    visibility: "private",
  });
  const prodRepo = observed.orgs["embedded-alerts"].repos.find((repo) => repo.name === "eal-interfaces");
  const testRepo = observed.orgs["embedded-alerts-test"].repos.find((repo) => repo.name === "eal-e2e");
  testRepo.defaultBranch = "dev";
  testRepo.visibility = "public";
  testRepo.archived = true;
  observed.orgs["fiducia-cloud-test"].repos = observed.orgs["fiducia-cloud-test"].repos.filter(
    (repo) => repo.name !== "fiducia-cli",
  );
  prodRepo.hasZpkg = false;
  observed.orgs["file-tunnel-tesr"] = { status: "ok", repos: [] };
  const report = detect({ expected, observed, observedAt: observed.observedAt });
  const classes = new Set(report.drift.map((item) => item.class));
  assert.ok(classes.has("orphan-test-repository"));
  assert.ok(classes.has("mismatch-default-branch"));
  assert.ok(classes.has("mismatch-visibility"));
  assert.ok(classes.has("mismatch-archived"));
  assert.ok(classes.has("production-without-test-strategy"));
  assert.ok(classes.has("missing-zpkg-toml"));
  assert.ok(!classes.has("orphan-test-org"), "file-tunnel-tesr is an explicit exception");
});

test("explicit orphan exception covers file-tunnel-tesr", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  observed.orgs["file-tunnel-tesr"] = { status: "ok", repos: [] };
  const report = detect({ expected, observed, observedAt: observed.observedAt });
  assert.ok(!report.drift.some((item) => item.testOrg === "file-tunnel-tesr"));
});

test("live credential copies in a test org are drift", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  observed.orgs["zed-pkg-test"].repos[0].copiesLiveCredentials = true;
  const report = detect({ expected, observed, observedAt: observed.observedAt });
  assert.ok(report.drift.some((item) => item.class === "live-credential-copy"));
});

test("markdown output is deterministic and secret-free", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  observed.orgs["embedded-alerts-test"].repos.push({ name: "ghost", defaultBranch: "main" });
  const report = detect({ expected, observed, observedAt: "2026-08-26T00:00:00Z" });
  const markdown = formatMarkdown(report);
  assert.equal(markdown, formatMarkdown(report));
  assert.match(markdown, /orphan-test-repository/);
  assert.doesNotMatch(markdown, /ghp_|lin_api_/);
});

test("role inference covers the canonical topology suffixes", () => {
  assert.equal(inferRole("eal-interfaces"), "interfaces");
  assert.equal(inferRole("eal-lib-core"), "lib");
  assert.equal(inferRole("eal-clients"), "clients");
  assert.equal(inferRole("eal-cli"), "cli");
  assert.equal(inferRole("eal-web-server.rs"), "web");
  assert.equal(inferRole("eal-api-server.rs"), "api");
  assert.equal(inferRole("eal-desktop-app.rs"), "desktop");
  assert.equal(inferRole("eal-e2e"), "e2e");
  assert.equal(inferRole("eal-infra"), "infra");
  assert.equal(inferRole("eal-monorepo"), "monorepo");
  assert.equal(inferRole("README"), "other");
});

test("observeFromGitHub never infers an empty org from 403/404", async () => {
  const responses = {
    "/orgs/embedded-alerts": { status: 403, body: null },
    "/orgs/embedded-alerts-test": { status: 404, body: null },
  };
  const observed = await observeFromGitHub({
    expected: {
      pairs: [{ productionOrg: "embedded-alerts", testOrg: "embedded-alerts-test" }],
    },
    ghApi: async (path) => responses[path] ?? { status: 404, body: null },
  });
  assert.equal(observed.orgs["embedded-alerts"].status, "unknown");
  assert.equal(observed.orgs["embedded-alerts-test"].status, "missing");
  assert.equal(observed.orgs["embedded-alerts"].repos, undefined);
});

test("ghApiClient maps 404/403 from gh without throwing", async () => {
  const client = ghApiClient({
    runner: () => {
      const error = new Error("gh failed");
      error.stderr = "gh: Not Found (HTTP 404)";
      throw error;
    },
  });
  assert.deepEqual(await client("/orgs/missing"), { status: 404, body: null });
});

test("aligned fixture file matches the expected snapshot", () => {
  const expected = loadExpected();
  const generated = alignedObservation(expected);
  const committed = JSON.parse(readFileSync(join(ROOT, "fixtures/aligned.json"), "utf8"));
  assert.equal(stableStringify(generated), stableStringify(committed));
});

test("report write path is byte-stable", () => {
  const expected = loadExpected();
  const observed = alignedObservation(expected);
  const report = detect({ expected, observed, observedAt: observed.observedAt });
  const directory = mkdtempSync(join(tmpdir(), "parity-"));
  const path = join(directory, "report.json");
  writeFileSync(path, stableStringify(report));
  assert.equal(readFileSync(path, "utf8"), stableStringify(report));
});
