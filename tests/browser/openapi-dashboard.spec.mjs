import { expect, test } from '@playwright/test';
import WebSocket from 'ws';

const baseURL = process.env.TOR_DOCS_BASE_URL ?? 'http://127.0.0.1:19060';
const token = process.env.TOR_DOCS_TOKEN ?? 'browser+smoke/token:0123456789abcdef';
const wsURL = `${baseURL.replace(/^http/, 'ws')}/ws/stats?token=${encodeURIComponent(token)}`;

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

function firstWebSocketMessage(origin) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(wsURL, { headers: { Origin: origin } });
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error('timed out waiting for WebSocket status frame'));
    }, 10_000);
    socket.once('message', (data) => {
      clearTimeout(timer);
      resolve({ socket, text: data.toString() });
    });
    socket.once('unexpected-response', (_request, response) => {
      clearTimeout(timer);
      response.resume();
      reject(new Error(`unexpected WebSocket HTTP ${response.statusCode}`));
    });
    socket.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function rejectedWebSocketStatus(origin) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const socket = new WebSocket(wsURL, { headers: { Origin: origin } });
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error('timed out waiting for cross-origin WebSocket rejection'));
    }, 10_000);
    socket.once('unexpected-response', (_request, response) => {
      settled = true;
      clearTimeout(timer);
      const status = response.statusCode;
      response.resume();
      resolve(status);
    });
    socket.once('open', () => {
      clearTimeout(timer);
      socket.terminate();
      reject(new Error('cross-origin WebSocket unexpectedly opened'));
    });
    socket.once('error', (error) => {
      if (!settled) {
        clearTimeout(timer);
        reject(error);
      }
    });
  });
}

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

test('public OpenAPI aliases are byte-identical fail-closed projections', async ({ request }) => {
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
  for (const path of sensitivePaths) expect(document.paths[path]).toBeUndefined();
  expect(document.components?.securitySchemes).toBeUndefined();
});

test('private status and internal docs require the same UI token', async ({ request }) => {
  for (const path of ['/api/status', '/internal/openapi.json', '/internal/docs/api']) {
    const denied = await request.get(`${baseURL}${path}`);
    expect(denied.status()).toBe(401);
    expect(await denied.text()).not.toContain(token);
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

  const internalDocs = await request.get(`${baseURL}/internal/docs/api`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(internalDocs.status()).toBe(200);
  expect(await internalDocs.text()).not.toContain(token);
});

test('query-token compatibility decodes exact values and rejects malformed auth', async ({ request }) => {
  const encoded = await request.get(`${baseURL}/api/status?token=${encodeURIComponent(token)}`);
  expect(encoded.status()).toBe(200);

  const unescapedPlus = await request.get(`${baseURL}/api/status?token=${token}`);
  expect(unescapedPlus.status()).toBe(401);

  const wrongScheme = await request.get(`${baseURL}/api/status`, {
    headers: { Authorization: `bearer ${token}` },
  });
  expect(wrongScheme.status()).toBe(401);

  const wrongToken = await request.get(`${baseURL}/api/status`, {
    headers: { Authorization: 'Bearer definitely-not-the-token' },
  });
  expect(wrongToken.status()).toBe(401);
  expect(await wrongToken.text()).not.toContain(token);
});

test('dashboard and live WebSocket render through query-token compatibility', async ({ page }) => {
  const response = await page.goto(`${baseURL}/?token=${encodeURIComponent(token)}`, {
    waitUntil: 'domcontentloaded',
  });
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('backend-badge')).toContainText('overlay');
  await expect(page.getByTestId('relay-pill')).toHaveCount(1);
  await expect(page.getByTestId('stat-relays')).toHaveText('1');

  const { socket, text } = await firstWebSocketMessage(baseURL);
  const frame = JSON.parse(text);
  expect(frame.backend).toBe('overlay');
  expect(frame.relay_count).toBe(1);
  expect(text).not.toContain(token);
  socket.close();
});

test('cross-origin WebSocket hijacking is rejected after valid authentication', async () => {
  expect(await rejectedWebSocketStatus('https://evil.example')).toBe(403);
});

test('fetch proxy rejects anonymous and request-smuggling inputs before useful egress', async ({ request }) => {
  const anonymous = await request.get(`${baseURL}/api/fetch?url=http://example.com/`);
  expect(anonymous.status()).toBe(401);
  const anonymousText = await anonymous.text();
  expect(anonymousText).not.toContain(token);
  expect(anonymousText.length).toBeLessThan(2048);

  const injectedURL = encodeURIComponent('http://example.com/path\r\nX-Injected: true');
  const injected = await request.get(
    `${baseURL}/api/fetch?token=${encodeURIComponent(token)}&url=${injectedURL}`,
  );
  expect(injected.status()).toBe(200);
  const injectedText = await injected.text();
  expect(injectedText).toMatch(/invalid|control|space/i);
  expect(injectedText).not.toContain(token);
});

test('documentation route rejects encoded traversal attempts', async ({ request }) => {
  const response = await request.get(`${baseURL}/docs/%2e%2e%2fCargo`);
  expect([400, 404]).toContain(response.status());
  expect(await response.text()).not.toContain(token);
});
