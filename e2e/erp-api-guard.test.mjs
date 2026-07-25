// ERP API (/api/v1) authentication guard, driven as an unauthenticated caller.
// No Supabase needed: every case here asserts that a missing, malformed, or
// unknown API key NEVER yields a 2xx from an ERP endpoint. That invariant holds
// whether or not a database is wired up:
//   * with a DB (the e2e CI Postgres): the guard rejects with 401.
//   * degraded (no DATABASE_URL): `authenticate` short-circuits with 503 before
//     it even reads the token.
// So we assert the security property directly -- "no 2xx without a valid key",
// status in {401, 403, 503} -- which runs green in every lane, including PRs
// with no deployment secrets. The precise 401/403 codes and the happy path are
// pinned in api-keys.test.mjs, which has a real DB + an approved B2B account.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { Driver } from './lib/driver.mjs';
import { BASE_URL } from './lib/harness.mjs';

// A rejection is any non-2xx the guard can legitimately return. The point of
// the suite is the negative space: an ERP endpoint must never serve data to an
// unauthenticated caller, so a 2xx is always a failure.
const REJECTIONS = new Set([401, 403, 415, 422, 503]);

let driver;
before(async () => {
  driver = await Driver.launch();
});
after(async () => {
  await driver?.close();
});

/** Fetch from inside the page (same-origin) and return {status, body}. */
async function apiFetch(page, path, init = {}) {
  return page.evaluate(
    async ({ base, p, i }) => {
      const r = await fetch(`${base}${p}`, { ...i, credentials: 'same-origin' });
      let body = '';
      try {
        body = await r.text();
      } catch {
        /* ignore */
      }
      return { status: r.status, body };
    },
    { base: BASE_URL, p: path, i: init },
  );
}

const GET_ENDPOINTS = ['/api/v1/products', '/api/v1/orders'];

for (const path of GET_ENDPOINTS) {
  test(`[${Driver.engine()}] GET ${path} without a bearer never returns 2xx`, async () => {
    const page = await driver.newPage();
    try {
      await page.goto(`${BASE_URL}/`);
      const { status } = await apiFetch(page, path);
      assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
      assert.ok(REJECTIONS.has(status), `expected an auth/unavailable rejection, got ${status}`);
    } finally {
      await page.close();
    }
  });

  test(`[${Driver.engine()}] GET ${path} with a non-athk bearer never returns 2xx`, async () => {
    const page = await driver.newPage();
    try {
      await page.goto(`${BASE_URL}/`);
      const { status } = await apiFetch(page, path, {
        headers: { authorization: 'Bearer not-an-athleto-key' },
      });
      assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
      assert.ok(REJECTIONS.has(status), `got ${status}`);
    } finally {
      await page.close();
    }
  });

  test(`[${Driver.engine()}] GET ${path} with a well-formed but unknown athk key never returns 2xx`, async () => {
    const page = await driver.newPage();
    try {
      await page.goto(`${BASE_URL}/`);
      // Shaped like a real key ("athk_" + 64 hex) but never issued: this reaches
      // the DB lookup (when a DB exists) and must come back unknown/revoked.
      const { status } = await apiFetch(page, path, {
        headers: { authorization: `Bearer athk_${'0'.repeat(64)}` },
      });
      assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
      assert.ok(REJECTIONS.has(status), `got ${status}`);
    } finally {
      await page.close();
    }
  });
}

test(`[${Driver.engine()}] POST /api/v1/orders without a bearer never places an order`, async () => {
  const page = await driver.newPage();
  try {
    await page.goto(`${BASE_URL}/`);
    const { status } = await apiFetch(page, '/api/v1/orders', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ items: [{ product_id: 1, qty: 1 }] }),
    });
    // The auth guard runs before the body is ever considered, so an unauthorized
    // create must never reach 201.
    assert.notEqual(status, 201, 'unauthenticated create must not succeed');
    assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
    assert.ok(REJECTIONS.has(status), `got ${status}`);
  } finally {
    await page.close();
  }
});

test(`[${Driver.engine()}] POST /api/v1/orders/{id}/fulfillment without the ops key never returns 2xx`, async () => {
  const page = await driver.newPage();
  try {
    await page.goto(`${BASE_URL}/`);
    const { status } = await apiFetch(page, `/api/v1/orders/${crypto.randomUUID()}/fulfillment`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ carrier: 'ups', tracking: 'X' }),
    });
    assert.ok(status < 200 || status >= 300, `expected non-2xx, got ${status}`);
    assert.ok(REJECTIONS.has(status), `got ${status}`);
  } finally {
    await page.close();
  }
});
