import { expect, test } from '@playwright/test';

const baseURL = process.env.PUSH_DOCS_BASE_URL ?? 'http://127.0.0.1:8121';
const deviceCapability = 'fixture-device-token-that-must-never-be-echoed';
const privateRecipient = 'private-recipient@example.com';

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

function pushPayload() {
  return {
    version: 'v1',
    job_id: 'browser-job-1',
    tenant_id: 'tenant-1',
    application_id: 'app-1',
    idempotency_key: 'browser-event-1',
    provider: 'fcm',
    target: { type: 'fcm', token: deviceCapability },
    notification: { title: 'Browser smoke' },
    options: {},
    trace: {},
  };
}

function contactPayload() {
  return {
    version: 'v1',
    job_id: 'browser-contact-1',
    tenant_id: 'tenant-1',
    application_id: 'app-1',
    idempotency_key: 'browser-contact-event-1',
    provider: 'sendgrid',
    target: { type: 'email', address: privateRecipient, name: 'Private Recipient' },
    content: { type: 'email', subject: 'Browser smoke', text: 'body' },
    trace: {},
  };
}

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

test('public OpenAPI JSON is a byte-stable fail-closed projection', async ({ request }) => {
  const canonical = await request.get(`${baseURL}/openapi.json`);
  const alias = await request.get(`${baseURL}/api/docs.json`);
  expect(canonical.status()).toBe(200);
  expect(alias.status()).toBe(200);
  expect(canonical.headers()['content-type']).toContain('application/vnd.oai.openapi+json');
  expect(alias.headers()['content-type']).toContain('application/vnd.oai.openapi+json');
  expect((await alias.body()).equals(await canonical.body())).toBe(true);

  const document = await canonical.json();
  expect(document.openapi).toBe('3.1.0');
  expect(document['x-dd-contract-scope']).toBe('public');
  expect(new Set(Object.keys(document.paths))).toEqual(publicPaths);
  for (const path of privatePaths) expect(document.paths[path]).toBeUndefined();
  expect(document.components?.securitySchemes).toBeUndefined();
});

test('readiness reports fail-closed startup without credentials or providers', async ({ request }) => {
  const health = await request.get(`${baseURL}/healthz`);
  expect(health.status()).toBe(200);
  expect(await health.json()).toEqual({ ok: true, service: 'push-notification-server' });

  const push = await request.get(`${baseURL}/readyz`);
  expect(push.status()).toBe(503);
  const pushBody = await push.json();
  expect(pushBody.ok).toBe(false);
  expect(pushBody.authentication).toEqual({ configured: false, mode: 'disabled' });
  expect(JSON.stringify(pushBody)).not.toContain('SECRET');

  const contact = await request.get(`${baseURL}/v1/contact/readyz`);
  expect(contact.status()).toBe(503);
  const contactBody = await contact.json();
  expect(contactBody.ok).toBe(false);
  expect(contactBody.authentication_configured).toBe(false);
  expect(contactBody.authentication_mode).toBe('disabled');
  expect(JSON.stringify(contactBody)).not.toContain('SECRET');
});

test('private docs reject absent and malformed bearer credentials', async ({ request }) => {
  const authVariants = [
    undefined,
    'bearer not-a-secret',
    'Bearer not-a-secret',
    'Bearer  not-a-secret',
    'Basic bm90LWEtc2VjcmV0',
  ];

  for (const path of ['/internal/openapi.json', '/internal/docs/api']) {
    for (const authorization of authVariants) {
      const response = await request.get(`${baseURL}${path}`, {
        headers: authorization ? { Authorization: authorization } : {},
      });
      expect(response.status()).toBe(401);
      const text = await response.text();
      expect(text).not.toContain('SERVER_AUTH_SECRET');
      expect(text).not.toContain('not-a-secret');
      expect(text.length).toBeLessThan(2048);
    }
  }
});

test('unauthenticated push mutation returns a bounded redacted JSON error', async ({ request }) => {
  const response = await request.post(`${baseURL}/v1/push/jobs`, {
    data: pushPayload(),
  });
  expect(response.status()).toBe(401);
  expect(response.headers()['content-type']).toContain('application/json');
  const body = await response.json();
  expect(body).toEqual({
    error: {
      code: 'unauthorized',
      safe_detail: 'request authentication failed',
    },
  });
  const text = JSON.stringify(body);
  expect(text).not.toContain(deviceCapability);
  expect(text.length).toBeLessThan(2048);
});

test('unauthenticated contact mutation never echoes recipient data', async ({ request }) => {
  const response = await request.post(`${baseURL}/v1/contact/jobs`, {
    data: contactPayload(),
  });
  expect(response.status()).toBe(401);
  const text = await response.text();
  expect(text).not.toContain(privateRecipient);
  expect(text).not.toContain('Private Recipient');
  expect(text.length).toBeLessThan(2048);
});

test('HTTP body limits reject oversized push and contact requests', async ({ request }) => {
  const push = await request.post(`${baseURL}/v1/push/jobs`, {
    headers: { 'Content-Type': 'application/json' },
    data: 'x'.repeat(512 * 1024 + 1),
  });
  expect(push.status()).toBe(413);

  const contact = await request.post(`${baseURL}/v1/contact/jobs`, {
    headers: { 'Content-Type': 'application/json' },
    data: 'x'.repeat(768 * 1024 + 1),
  });
  expect(contact.status()).toBe(413);
});

test('documentation endpoints reject unsupported methods without disclosure', async ({ request }) => {
  for (const path of ['/openapi.json', '/api/docs.json', '/api/docs', '/docs/api']) {
    const response = await request.post(`${baseURL}${path}`, { data: {} });
    expect(response.status()).toBe(405);
    const text = await response.text();
    expect(text).not.toContain(deviceCapability);
    expect(text).not.toContain(privateRecipient);
  }
});
