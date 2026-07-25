// Cross-user order isolation, end to end through the browser. Order reads are
// scoped to the owner in SQL (db::get_order(pool, user_id, order_id)); this
// proves the property from the outside: user B, fully logged in, cannot open
// user A's receipt by guessing its id. Complements the DB-level test
// tests/order_ownership_db.rs (which is #[ignore]d without a database).
//
// B2C only, so no 2FA is involved -- needs just SUPABASE_URL + SUPABASE_SERVICE_KEY.
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

/** Log in a personal (B2C) user, save the default profile, place one order, and
 * return the order's detail path (/orders/{uuid}). */
async function placeOrderAs(email) {
  const page = await driver.newPage();
  try {
    await loginBrowser(page, email); // new user -> /account/setup
    await page.goto(`${BASE_URL}/account/setup`);
    await page.waitFor('form[action="/account/setup"] button[type="submit"]');
    await page.click('form[action="/account/setup"] button[type="submit"]');
    await page.waitFor('.hero, .product-grid, .product-card', { timeout: 10000 });

    await page.goto(`${BASE_URL}/`);
    await page.waitFor('.product-card button.buy');
    await page.click('.product-card button.buy');
    await page.waitFor('.card-status .added', { timeout: 10000 });

    await page.goto(`${BASE_URL}/cart`);
    await page.waitFor('.checkout-form button[type="submit"]');
    await page.click('.checkout-form button[type="submit"]');
    await page.waitFor('.order-card', { timeout: 10000 });

    const href = await page.attr('.order-card a.button', 'href');
    assert.ok(href, "owner's order card links to its receipt");
    // Normalize to a path (href may be absolute or relative).
    const path = href.startsWith('http') ? new URL(href).pathname : href;
    assert.match(path, /^\/orders\/[0-9a-f-]{36}$/i, 'a real order detail path');
    return path;
  } finally {
    await page.close();
  }
}

test(`[${Driver.engine()}] a second user cannot open the first user's receipt`, { skip }, async () => {
  const alice = testEmail('iso-a');
  const bob = testEmail('iso-b');
  created.push(alice, bob);

  const orderPath = await placeOrderAs(alice);
  const shortId = orderPath.split('/').pop().slice(0, 8);

  // Alice can open her own receipt.
  const aPage = await driver.newPage();
  try {
    await loginBrowser(aPage, alice);
    await aPage.goto(`${BASE_URL}${orderPath}`);
    await aPage.waitFor('.receipt', { timeout: 10000 });
    assert.match(await aPage.content(), /Subtotal/i, 'owner sees the receipt');
    assert.match(await aPage.url(), new RegExp(orderPath.replace(/[/]/g, '\\/')), 'stayed on the receipt');
  } finally {
    await aPage.close();
  }

  // Bob -- a different, fully-logged-in user -- cannot.
  const bPage = await driver.newPage();
  try {
    await loginBrowser(bPage, bob); // new user -> /account/setup; save personal
    await bPage.goto(`${BASE_URL}/account/setup`);
    await bPage.waitFor('form[action="/account/setup"] button[type="submit"]');
    await bPage.click('form[action="/account/setup"] button[type="submit"]');
    await bPage.waitAwayFrom('/account/setup', { timeout: 10000 });

    // Attempt to open Alice's order by its exact id.
    await bPage.goto(`${BASE_URL}${orderPath}`);
    // Ownership is enforced in SQL: a non-owner read returns None -> redirect to
    // the caller's own /orders list. Bob must never see Alice's receipt.
    assert.doesNotMatch(await bPage.url(), new RegExp(`${orderPath}$`), 'not left on the receipt URL');
    const html = await bPage.content();
    assert.doesNotMatch(html, /class="receipt"/, "no receipt rendered for a non-owner");
    assert.doesNotMatch(html, new RegExp(shortId, 'i'), "Alice's order id is not disclosed");

    // And Bob's own order list is empty -- he has placed nothing.
    await bPage.goto(`${BASE_URL}/orders`);
    await bPage.waitFor('body');
    assert.equal(await bPage.count('.order-card'), 0, 'a fresh user has no orders');
  } finally {
    await bPage.close();
  }
});

test(`[${Driver.engine()}] a guest is bounced from the orders area to login`, { skip }, async () => {
  const page = await driver.newPage();
  try {
    // No session at all: /orders requires auth and must not render a list.
    const { status } = await page.navigate(`${BASE_URL}/orders`);
    // require_full redirects an anonymous caller to /login (200 after the
    // redirect chain); the key property is that no order data is shown.
    assert.ok(status < 400, `followed redirect chain, got ${status}`);
    const url = await page.url();
    const html = await page.content();
    assert.ok(
      /\/login/.test(url) || !/class="order-card"/.test(html),
      'anonymous caller sees login or an empty area, never orders',
    );
  } finally {
    await page.close();
  }
});
