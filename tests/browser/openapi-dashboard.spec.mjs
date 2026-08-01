import { expect, test } from '@playwright/test';

const baseURL = process.env.TOR_DOCS_BASE_URL ?? 'http://127.0.0.1:19060';
const token = process.env.TOR_DOCS_TOKEN ?? 'browser-smoke-token-0123456789abcdef';

const publicPaths = new Set([
  '/healthz',
  '/vendor/{file}',
  '/docs',
  '/docs/{name}',
  '/proxy.pac',
  '/openapi.json',
  '/api/docs.json',
  '/api/docs',
  '/docs/api',
]);

const sensitivePaths = [
  '/',
  '/api/status',
  '/api/fetch',
  '/ws/stats',
  '/internal/openapi.json',
  '/internal/docs/api',
];

test('public Scalar API reference renders in Chromium', async ({ page }) => {
  const response = await page.goto(`${baseURL}/api/docs`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.locator('body')).toContainText(/tor-server dashboard API/i);
  await expect(page.locator('body')).toContainText(/healthz|proxy\.pac/i);
});

test('docs alias and existing markdown docs remain navigable', async ({ page }) => {
  let response = await page.goto(`${baseURL}/docs/api`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.locator('body')).toContainText(/tor-server dashboard API/i);

  response = await page.goto(`${baseURL}/docs`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.locator('body')).toContainText('Documentation');
});

test('public document is a fail-closed projection', async ({ request }) => {
  const response = await request.get(`${baseURL}/openapi.json`);
  expect(response.status()).toBe(200);
  expect(response.headers()['content-type']).toContain('application/vnd.oai.openapi+json');
  const document = await response.json();
  expect(document.openapi).toBe('3.1.0');
  expect(document['x-dd-contract-scope']).toBe('public');
  expect(new Set(Object.keys(document.paths))).toEqual(publicPaths);
  for (const path of sensitivePaths) expect(document.paths[path]).toBeUndefined();
  expect(document.components?.securitySchemes).toBeUndefined();
});

test('private status and internal docs require the same UI token', async ({ request }) => {
  for (const path of ['/api/status', '/internal/openapi.json', '/internal/docs/api']) {
    const denied = await request.get(`${baseURL}${path}`);
    expect(denied.status()).toBe(401);
  }

  const status = await request.get(`${baseURL}/api/status`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(status.status()).toBe(200);
  const statusBody = await status.json();
  expect(statusBody.backend).toBe('overlay');
  expect(statusBody.relay_count).toBe(1);
  expect(JSON.stringify(statusBody)).not.toContain(token);

  const internal = await request.get(`${baseURL}/internal/openapi.json`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(internal.status()).toBe(200);
  const document = await internal.json();
  expect(document['x-dd-contract-scope']).toBe('internal');
  for (const path of sensitivePaths) expect(document.paths[path]).toBeDefined();
  expect(document.components.securitySchemes.ui_token).toBeDefined();
});

test('dashboard and WebSocket render through query-token compatibility', async ({ page }) => {
  const response = await page.goto(`${baseURL}/?token=${encodeURIComponent(token)}`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('backend-badge')).toContainText('overlay');
  await expect(page.getByTestId('relay-pill')).toHaveCount(1);
  await expect(page.getByTestId('stat-relays')).toHaveText('1');
});

test('fetch proxy rejects anonymous callers before any outbound connection', async ({ request }) => {
  const response = await request.get(`${baseURL}/api/fetch?url=http://example.com/`);
  expect(response.status()).toBe(401);
  const text = await response.text();
  expect(text).not.toContain(token);
  expect(text.length).toBeLessThan(2048);
});
