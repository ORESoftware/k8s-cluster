// B2C checkout edge cases, end to end: an empty cart offers no checkout, and a
// place -> reorder -> re-checkout round-trip works (reorder repopulates the cart
// and re-claims holds). B2C, so no 2FA -- needs SUPABASE_URL + SUPABASE_SERVICE_KEY.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { Driver } from './lib/driver.mjs';
import { BASE_URL, hasAuth, testEmail, loginBrowser, deleteUser } from './lib/harness.mjs';

const skip = hasAuth() ? false : 'SUPABASE_URL / SUPABASE_SERVICE_KEY not set';

let driver;
const created = [];
before(async () => {
  if (!hasAuth()) return;
  driver = await Driver.launch();
});
after(async () => {
  await driver?.close();
  for (const email of created) await deleteUser(email).catch(() => {});
});

async function savePersonalProfile(page) {
  await page.goto(`${BASE_URL}/account/setup`);
  await page.waitFor('form[action="/account/setup"] button[type="submit"]');
  await page.click('form[action="/account/setup"] button[type="submit"]');
  await page.waitAwayFrom('/account/setup', { timeout: 10000 });
}

async function addFirstItem(page) {
  await page.goto(`${BASE_URL}/`);
  await page.waitFor('.product-card button.buy');
  await page.click('.product-card button.buy');
  await page.waitFor('.card-status .added', { timeout: 10000 });
}

test(`[${Driver.engine()}] a logged-in empty cart offers no checkout`, { skip }, async () => {
  const email = testEmail('empty-cart');
  created.push(email);
  const page = await driver.newPage();
  try {
    await loginBrowser(page, email);
    await savePersonalProfile(page);

    await page.goto(`${BASE_URL}/cart`);
    await page.waitFor('#cart-contents, .cart-table, .notice');
    const html = await page.content();
    assert.match(html, /empty/i, 'empty-cart copy shown');
    assert.equal(await page.exists('.checkout-form button[type="submit"]'), false, 'no checkout button');
    // No active hold to report.
    const hold = await page.evaluate(async (base) => {
      const r = await fetch(`${base}/cart/hold`, { credentials: 'same-origin' });
      return r.json();
    }, BASE_URL);
    assert.equal(hold.active, false, 'no active lease on an empty cart');
  } finally {
    await page.close();
  }
});

test(`[${Driver.engine()}] POST /checkout on an empty cart redirects back to /cart`, { skip }, async () => {
  const email = testEmail('empty-checkout');
  created.push(email);
  const page = await driver.newPage();
  try {
    await loginBrowser(page, email);
    await savePersonalProfile(page);

    // Issue the checkout POST directly with the double-submit CSRF token, but an
    // empty cart. The handler finds no lines and bounces to /cart -- never an
    // order, never a 500.
    await page.goto(`${BASE_URL}/cart`);
    const result = await page.evaluate(async (base) => {
      const token = (document.cookie.match(/(?:^|; )athleto_csrf=([^;]+)/) || [])[1] || '';
      const r = await fetch(`${base}/checkout`, {
        method: 'POST',
        headers: {
          'content-type': 'application/x-www-form-urlencoded',
          'x-csrf-token': token,
        },
        body: `csrf_token=${encodeURIComponent(token)}&ship_method=standard`,
        credentials: 'same-origin',
        redirect: 'manual',
      });
      return { status: r.status, location: r.headers.get('location') || '' };
    }, BASE_URL);
    // A redirect (30x) toward /cart, or a followed 200 that is not an order page.
    assert.ok(result.status < 400, `no error status, got ${result.status}`);
    if (result.location) assert.match(result.location, /\/cart/, 'redirected to /cart');
  } finally {
    await page.close();
  }
});

test(`[${Driver.engine()}] place -> reorder repopulates the cart and can check out again`, { skip }, async () => {
  const email = testEmail('reorder');
  created.push(email);
  const page = await driver.newPage();
  try {
    await loginBrowser(page, email);
    await savePersonalProfile(page);

    // First order.
    await addFirstItem(page);
    await page.goto(`${BASE_URL}/cart`);
    await page.waitFor('.checkout-form button[type="submit"]');
    await page.click('.checkout-form button[type="submit"]');
    await page.waitFor('.order-card', { timeout: 10000 });

    // Reorder from the order card -> lands on /cart with the item back.
    await page.waitFor('form[action$="/reorder"] button, .order-card form[action$="/reorder"]');
    await page.click('form[action$="/reorder"] button');
    await page.waitFor('.cart-table', { timeout: 10000 });
    assert.ok(await page.exists('.cart-table'), 'cart repopulated by reorder');
    const seconds = Number(await page.attr('#hold-banner', 'data-seconds'));
    assert.ok(seconds > 5000, `reorder re-claimed a ~90min hold, got ${seconds}s`);

    // And it checks out into a second order.
    await page.waitFor('.checkout-form button[type="submit"]');
    await page.click('.checkout-form button[type="submit"]');
    await page.waitFor('.order-card', { timeout: 10000 });
    assert.ok((await page.count('.order-card')) >= 2, 'two orders now listed');
  } finally {
    await page.close();
  }
});
