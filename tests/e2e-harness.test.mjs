// Cheap contract test (runs in base `npm test`, no browser/live server needed):
// the browser e2e harness stays wired and self-consistent. Catches an
// accidentally deleted spec or a renamed script before CI would.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";

const E2E = "tests/e2e";

test("both Playwright and Puppeteer suites are present", () => {
  assert.ok(existsSync(`${E2E}/auth.spec.mjs`), "Playwright spec missing");
  assert.ok(
    existsSync(`${E2E}/puppeteer/auth.puppeteer.test.mjs`),
    "Puppeteer spec missing",
  );
  assert.ok(existsSync(`${E2E}/playwright.config.mjs`), "Playwright config missing");
});

test("e2e package declares both browser tools and their scripts", () => {
  const pkg = JSON.parse(readFileSync(`${E2E}/package.json`, "utf8"));
  assert.ok(pkg.devDependencies["@playwright/test"], "playwright devDependency missing");
  assert.ok(pkg.devDependencies.puppeteer, "puppeteer devDependency missing");
  assert.match(pkg.scripts["test:playwright"], /playwright/);
  assert.match(pkg.scripts["test:puppeteer"], /node --test/);
});

test("both suites gate on the same base-URL env var", () => {
  // If the specs and docs drift on the env var name, the suite silently never
  // runs. Pin the contract: both reference DAEDALUS_WEB_BASE_URL.
  const playwright = readFileSync(`${E2E}/auth.spec.mjs`, "utf8");
  const puppeteer = readFileSync(`${E2E}/puppeteer/auth.puppeteer.test.mjs`, "utf8");
  for (const [name, source] of [["playwright", playwright], ["puppeteer", puppeteer]]) {
    assert.match(source, /DAEDALUS_WEB_BASE_URL/, `${name} spec must gate on DAEDALUS_WEB_BASE_URL`);
    // Both must assert the auth gate: anonymous access is never a 200.
    assert.match(source, /\b200\b/, `${name} spec must assert on the 200 boundary`);
  }
});
