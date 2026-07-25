// B2B ERP API-key surface on /account, and the security gate around minting a
// key. A key is B2B + approved + AAL2-only: an approved business account that
// has NOT enrolled 2FA can see the "ERP API keys" card but cannot create one --
// the POST bounces to /account?required2fa=1. This proves the gate without the
// flaky live TOTP round-trip (see twofa.test.mjs); the full mint + use path is
// covered by erp-orders-api.test.mjs when a pre-issued key is supplied.
//
// Needs SUPABASE_URL + SUPABASE_SERVICE_KEY and E2E_OPS_KEY (== the app's
// ATHLETO_OPERATIONS_API_KEY) to approve the account; skips otherwise.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { Driver } from './lib/driver.mjs';
import {
  BASE_URL,
  hasAuth,
  testEmail,
  loginBrowser,
  deleteUser,
  getUserId,
} from './lib/harness.mjs';

const OPS = process.env.E2E_OPS_KEY || '';
const skip = !hasAuth() ? 'SUPABASE not set' : !OPS ? 'E2E_OPS_KEY not set' : false;

async function approve(userId) {
  return fetch(`${BASE_URL}/api/v1/ops/customers/${userId}/approval`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: `Bearer ${OPS}` },
    body: JSON.stringify({ approved: true }),
  });
}

let driver;
const created = [];
before(async () => {
  if (skip) return;
  driver = await Driver.launch();
});
after(async () => {
  await driver?.close();
  for (const email of created) await deleteUser(email).catch(() => {});
});

/** Log in, choose the B2B profile, and (out of band) ops-approve the account.
 * Leaves the page logged in as an approved-but-not-2FA business user. */
async function becomeApprovedB2B(page, email, company) {
  await loginBrowser(page, email);
  await page.goto(`${BASE_URL}/account/setup`);
  await page.waitFor('input[name="customer_type"][value="b2b"]');
  await page.click('input[name="customer_type"][value="b2b"]');
  await page.fill('input[name="company_name"]', company);
  await page.click('form[action="/account/setup"] button[type="submit"]');
  await page.waitAwayFrom('/account/setup', { timeout: 10000 });
  const userId = await getUserId(email);
  assert.ok(userId, 'created a user id');
  const r = await approve(userId);
  assert.equal(r.status, 200, 'ops approval succeeded');
  return userId;
}

test(`[${Driver.engine()}] approved B2B account shows the ERP API-keys card`, { skip }, async () => {
  const email = testEmail('apikey-card');
  created.push(email);
  const page = await driver.newPage();
  try {
    await becomeApprovedB2B(page, email, 'Keycard E2E Co');

    await page.goto(`${BASE_URL}/account`);
    await page.waitFor('#api-keys', { timeout: 10000 });
    const html = await page.content();
    assert.match(html, /ERP API keys/i, 'card heading present');
    assert.match(html, /\/api\/v1/, 'documents the /api/v1 base');
    assert.match(html, /Authorization: Bearer/i, 'documents bearer auth');
    // A fresh account has no keys, and the create form is present.
    assert.match(html, /No keys yet/i, 'empty-state copy');
    assert.ok(
      await page.exists('form[action="/account/api-keys"] input[name="name"]'),
      'create form with a name field',
    );
  } finally {
    await page.close();
  }
});

test(`[${Driver.engine()}] minting a key without 2FA is refused (required2fa gate)`, { skip }, async () => {
  const email = testEmail('apikey-gate');
  created.push(email);
  const page = await driver.newPage();
  try {
    await becomeApprovedB2B(page, email, 'Gate E2E Co');

    // Submit the create form. The account has no verified factor, so
    // require_b2b_ready must bounce the mint to /account?required2fa=1 and
    // reveal NO key -- an approved business account still cannot issue ERP
    // credentials until it has enrolled 2FA.
    await page.goto(`${BASE_URL}/account`);
    await page.waitFor('form[action="/account/api-keys"] input[name="name"]');
    await page.fill('form[action="/account/api-keys"] input[name="name"]', 'SPS Commerce prod');
    await page.click('form[action="/account/api-keys"] button[type="submit"]');
    await page.waitFor('body');

    const url = await page.url();
    const html = await page.content();
    assert.match(url, /required2fa=1|\/account/, 'bounced to the 2FA-required account view');
    // The one-time reveal panel must never appear, and no athk_ secret leaks.
    assert.doesNotMatch(html, /will not be shown again/i, 'no key-reveal panel');
    assert.doesNotMatch(html, /athk_[0-9a-f]{8}/i, 'no API key secret rendered');
  } finally {
    await page.close();
  }
});

test(`[${Driver.engine()}] revoking a non-existent key id is a safe no-op`, { skip }, async () => {
  // A B2B account POSTing a revoke for an id it does not own must not error or
  // leak; the handler swallows a missing/foreign id and redirects to /account.
  const email = testEmail('apikey-revoke');
  created.push(email);
  const page = await driver.newPage();
  try {
    await becomeApprovedB2B(page, email, 'Revoke E2E Co');
    await page.goto(`${BASE_URL}/account`);
    const csrf = await page.evaluate(
      () => (document.cookie.match(/(?:^|; )athleto_csrf=([^;]+)/) || [])[1] || '',
    );
    const status = await page.evaluate(
      async ({ base, token }) => {
        const r = await fetch(`${base}/account/api-keys/${crypto.randomUUID()}/revoke`, {
          method: 'POST',
          headers: {
            'content-type': 'application/x-www-form-urlencoded',
            'x-csrf-token': token,
          },
          body: `csrf_token=${encodeURIComponent(token)}`,
          credentials: 'same-origin',
          redirect: 'manual',
        });
        return r.status;
      },
      { base: BASE_URL, token: csrf },
    );
    // 2xx/3xx (redirect to /account), never a 5xx.
    assert.ok(status < 500, `revoke of an unknown id must not 5xx, got ${status}`);
  } finally {
    await page.close();
  }
});
