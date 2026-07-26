// Browser smoke contract for dd-fabrication-web-server (see ../../README.md).
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import puppeteer from 'puppeteer';

const baseUrl = (process.env.DD_FAB_E2E_BASE_URL ?? 'http://127.0.0.1:8115').replace(/\/+$/, '');

let browser;
let page;

before(async () => {
  browser = await puppeteer.launch({ headless: true });
  page = await browser.newPage();
});

after(async () => {
  if (browser) await browser.close();
});

test('healthz reports the service as live', async () => {
  const response = await page.goto(`${baseUrl}/healthz`);
  assert.equal(response.status(), 200);
  const payload = JSON.parse(await response.text());
  assert.equal(payload.ok, true);
  assert.equal(payload.service, 'dd-fabrication-web-server');
});

test('readyz responds with a well-formed readiness body', async () => {
  const response = await page.goto(`${baseUrl}/readyz`);
  assert.ok(
    [200, 503].includes(response.status()),
    `readyz should be 200 or 503, got ${response.status()}`,
  );
  const payload = JSON.parse(await response.text());
  assert.equal(typeof payload.ok, 'boolean');
});

test('the operator surface denies anonymous browsers', async () => {
  const response = await page.goto(`${baseUrl}/`);
  // Fail-closed either way: 401/403 when shared-auth is configured and the
  // browser has no bearer token, 503 when shared-auth is absent (the server
  // refuses to serve authenticated routes rather than serving them open).
  // What must never happen is a 2xx — that would be operator content leaking.
  assert.ok(
    [401, 403, 503].includes(response.status()),
    `anonymous "/" must be rejected with 401/403/503, got ${response.status()}`,
  );
});
