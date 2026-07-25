// ERP /api/v1 write + read path, exercised with a real B2B API key. Minting a
// key requires an AAL2 (2FA-verified) browser session, which the suite harness
// deliberately doesn't automate (twofa.test.mjs explains why), so this suite
// takes a pre-issued key from E2E_ERP_KEY and skips when it's absent. In the
// full-cluster e2e that key is seeded for an approved B2B account.
//
// Pure HTTP -- no browser needed. Covers the "850 in" order-create contract,
// its validation rejections, the orders read-back, and (as a tracked, skipped
// spec) the idempotency guard that athleto-app-rs#2 will add.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { BASE_URL } from './lib/harness.mjs';

const KEY = process.env.E2E_ERP_KEY || '';
const skip = KEY ? false : 'E2E_ERP_KEY not set (a pre-issued athk_ B2B key)';

function api(path, init = {}) {
  return fetch(`${BASE_URL}${path}`, {
    ...init,
    headers: {
      authorization: `Bearer ${KEY}`,
      'content-type': 'application/json',
      ...(init.headers || {}),
    },
  });
}

async function firstProductId() {
  const res = await api('/api/v1/products');
  assert.equal(res.status, 200, 'products list is readable with a valid key');
  const { products } = await res.json();
  assert.ok(Array.isArray(products) && products.length > 0, 'catalog is non-empty');
  const p = products[0];
  assert.ok(Number.isInteger(p.id), 'product has an integer id');
  assert.ok(p.unit_price_cents > 0, 'product has a price');
  return p.id;
}

test('[erp] GET /api/v1/products returns the catalog for a valid key', { skip }, async () => {
  await firstProductId(); // assertions inside
});

test('[erp] POST /api/v1/orders with no items is 422', { skip }, async () => {
  const res = await api('/api/v1/orders', { method: 'POST', body: JSON.stringify({ items: [] }) });
  assert.equal(res.status, 422, 'empty items rejected');
});

test('[erp] POST /api/v1/orders recurring without a frequency is 422', { skip }, async () => {
  const productId = await firstProductId();
  const res = await api('/api/v1/orders', {
    method: 'POST',
    body: JSON.stringify({ kind: 'recurring', items: [{ product_id: productId, qty: 1 }] }),
  });
  assert.equal(res.status, 422, 'recurring needs a frequency');
});

test('[erp] POST /api/v1/orders places a one-time order and it reads back', { skip }, async () => {
  const productId = await firstProductId();
  const po = `E2E-${Date.now().toString(36)}`;
  const res = await api('/api/v1/orders', {
    method: 'POST',
    body: JSON.stringify({
      kind: 'one_time',
      po_number: po,
      items: [{ product_id: productId, qty: 2 }],
    }),
  });
  assert.equal(res.status, 201, 'order created');
  const body = await res.json();
  assert.match(String(body.id), /[0-9a-f-]{36}/i, 'returns an order id');
  assert.ok(body.total_cents > 0, 'has a positive total');

  // It appears in the owner's order feed.
  const list = await api('/api/v1/orders');
  assert.equal(list.status, 200);
  const { orders } = await list.json();
  const found = orders.find((o) => o.id === body.id);
  assert.ok(found, 'created order is present in GET /api/v1/orders');
  assert.equal(found.po_number, po, 'PO number round-trips');
  assert.ok(String(found.status).length > 0, 'order carries a status label');
});

test('[erp] POST /api/v1/orders resolves items by slug as well as id', { skip }, async () => {
  const res = await api('/api/v1/products');
  const { products } = await res.json();
  const withSlug = products.find((p) => p.slug);
  assert.ok(withSlug, 'a product exposes a slug');
  const created = await api('/api/v1/orders', {
    method: 'POST',
    body: JSON.stringify({ items: [{ slug: withSlug.slug, qty: 1 }] }),
  });
  assert.equal(created.status, 201, 'slug-addressed order created');
});

// --- Tracked gap: idempotency (athleto-app-rs#2) -------------------------------
// The cart-less API path has no dedupe today: a retried POST creates a second
// order and decrements stock twice. When #2 lands (an Idempotency-Key header +
// a uniquely-constrained order_idempotency table), a retry with the same key
// must return the ORIGINAL order, not a new one. This is the executable spec;
// it stays skipped until the guard exists so CI is honest about the gap.
test(
  '[erp] a retried create with the same Idempotency-Key returns the original order',
  { skip: skip || 'idempotency guard not implemented yet — athleto-app-rs#2' },
  async () => {
    const productId = await firstProductId();
    const idem = `idem-${crypto.randomUUID()}`;
    const payload = JSON.stringify({ items: [{ product_id: productId, qty: 1 }] });

    const first = await api('/api/v1/orders', {
      method: 'POST',
      headers: { 'idempotency-key': idem },
      body: payload,
    });
    assert.equal(first.status, 201);
    const firstId = (await first.json()).id;

    // Same key, same body: the classic client-retry-after-timeout.
    const retry = await api('/api/v1/orders', {
      method: 'POST',
      headers: { 'idempotency-key': idem },
      body: payload,
    });
    // The retry must NOT mint a second order.
    assert.ok([200, 201].includes(retry.status), `got ${retry.status}`);
    assert.equal((await retry.json()).id, firstId, 'retry returns the original order id');
  },
);
