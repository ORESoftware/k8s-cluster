import { expect, test } from '@playwright/test';

const baseURL = process.env.PUSH_DOCS_BASE_URL ?? 'http://127.0.0.1:8121';

const publicPaths = new Set([
  '/healthz',
  '/readyz',
  '/v1/contact/readyz',
  '/openapi.json',
  '/api/docs.json',
  '/api/docs',
  '/docs/api',
]);

const privatePaths = [
  '/v1/push/jobs',
  '/v1/push/jobs/batch',
  '/v1/contact/jobs',
  '/v1/contact/jobs/batch',
  '/internal/openapi.json',
  '/internal/docs/api',
];

test('public Scalar reference renders without browser errors', async ({ page }) => {
  const consoleErrors = [];
  const pageErrors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });
  page.on('pageerror', (error) => pageErrors.push(error.message));

  const response = await page.goto(`${baseURL}/api/docs`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.locator('body')).toContainText(/push-notification-server API/i);
  // Scalar lazily expands operation groups, so assert its stable rendered
  // navigation rather than requiring collapsed operation paths in body text.
  await expect(page.locator('body')).toContainText(/documentation|operations/i);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
});

test('documentation alias renders the same executable contract', async ({ page }) => {
  const response = await page.goto(`${baseURL}/docs/api`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.locator('body')).toContainText(/push-notification-server API/i);
});

test('public OpenAPI JSON is fail closed', async ({ request }) => {
  const response = await request.get(`${baseURL}/openapi.json`);
  expect(response.status()).toBe(200);
  expect(response.headers()['content-type']).toContain('application/vnd.oai.openapi+json');

  const document = await response.json();
  expect(document.openapi).toBe('3.1.0');
  expect(document['x-dd-contract-scope']).toBe('public');
  expect(new Set(Object.keys(document.paths))).toEqual(publicPaths);
  for (const path of privatePaths) expect(document.paths[path]).toBeUndefined();
});

test('JSON alias is byte-identical and private docs fail closed', async ({ request }) => {
  const canonical = await request.get(`${baseURL}/openapi.json`);
  const alias = await request.get(`${baseURL}/api/docs.json`);
  expect(alias.status()).toBe(200);
  expect(await alias.body()).toEqual(await canonical.body());

  for (const path of ['/internal/openapi.json', '/internal/docs/api']) {
    const response = await request.get(`${baseURL}${path}`);
    expect(response.status()).toBe(401);
    expect(await response.text()).not.toContain('SERVER_AUTH_SECRET');
  }
});

test('unauthenticated mutation returns a bounded redacted error', async ({ request }) => {
  const secretCapability = 'fixture-device-token-that-must-never-be-echoed';
  const response = await request.post(`${baseURL}/v1/push/jobs`, {
    data: {
      version: 'v1',
      job_id: 'browser-job-1',
      tenant_id: 'tenant-1',
      application_id: 'app-1',
      idempotency_key: 'browser-event-1',
      provider: 'fcm',
      target: { type: 'fcm', token: secretCapability },
      notification: { title: 'Browser smoke' },
      options: {},
      trace: {},
    },
  });
  expect(response.status()).toBe(401);
  const text = await response.text();
  expect(text).not.toContain(secretCapability);
  expect(text.length).toBeLessThan(2048);
});
