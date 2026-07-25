// Browser smoke contract for dd-fabrication-web-server (see ../../README.md).
// Selenium Manager provisions a driver for the locally installed Chrome.
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { Browser, Builder } from 'selenium-webdriver';
import chrome from 'selenium-webdriver/chrome.js';

const baseUrl = (process.env.DD_FAB_E2E_BASE_URL ?? 'http://127.0.0.1:8115').replace(/\/+$/, '');
const ANONYMOUS_REJECTION_STATUSES = [401, 403, 503];

let driver;

before(async () => {
  const options = new chrome.Options().addArguments('--headless=new', '--disable-gpu');
  driver = await new Builder().forBrowser(Browser.CHROME).setChromeOptions(options).build();
});

after(async () => {
  if (driver) await driver.quit();
});

// Same-origin fetch from the page context: Selenium cannot observe navigation
// status codes, so status assertions go through the browser's own fetch.
async function fetchStatusAndBody(path) {
  return driver.executeAsyncScript(
    `const [path, done] = [arguments[0], arguments[arguments.length - 1]];
     fetch(path, { redirect: 'follow' })
       .then(async (response) => done({ status: response.status, body: await response.text() }))
       .catch((error) => done({ status: 0, body: String(error) }));`,
    path,
  );
}

test('healthz reports the service as live', async () => {
  await driver.get(`${baseUrl}/healthz`);
  const { status, body } = await fetchStatusAndBody('/healthz');
  assert.equal(status, 200, `healthz should be 200, got ${status}: ${body}`);
  const payload = JSON.parse(body);
  assert.equal(payload.ok, true);
  assert.equal(payload.service, 'dd-fabrication-web-server');
});

test('readyz responds with a well-formed readiness body', async () => {
  await driver.get(`${baseUrl}/healthz`);
  const { status, body } = await fetchStatusAndBody('/readyz');
  assert.ok([200, 503].includes(status), `readyz should be 200 or 503, got ${status}: ${body}`);
  const payload = JSON.parse(body);
  assert.equal(typeof payload.ok, 'boolean');
});

test('the operator surface denies anonymous browsers', async () => {
  await driver.get(`${baseUrl}/healthz`);
  const { status, body } = await fetchStatusAndBody('/');
  // Fail-closed either way: 401/403 when shared-auth is configured and the
  // browser has no bearer token, 503 when shared-auth is absent (the server
  // refuses to serve authenticated routes rather than serving them open).
  // What must never happen is a 2xx — that would be operator content leaking.
  assert.ok(
    ANONYMOUS_REJECTION_STATUSES.includes(status),
    `anonymous "/" must be rejected with 401/403/503, got ${status}: ${body}`,
  );
});
