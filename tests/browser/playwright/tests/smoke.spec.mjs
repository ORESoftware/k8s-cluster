// Browser smoke contract for dd-fabrication-web-server (see ../../README.md).
import { expect, test } from '@playwright/test';

test('healthz reports the service as live', async ({ page }) => {
  const response = await page.goto('/healthz');
  expect(response.status()).toBe(200);
  const payload = JSON.parse(await response.text());
  expect(payload.ok).toBe(true);
  expect(payload.service).toBe('dd-fabrication-web-server');
});

test('readyz responds with a well-formed readiness body', async ({ page }) => {
  const response = await page.goto('/readyz');
  expect([200, 503]).toContain(response.status());
  const payload = JSON.parse(await response.text());
  expect(typeof payload.ok).toBe('boolean');
});

test('the operator surface denies anonymous browsers', async ({ page }) => {
  const response = await page.goto('/');
  expect([401, 403]).toContain(response.status());
});
