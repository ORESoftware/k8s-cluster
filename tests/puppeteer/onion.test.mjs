// Puppeteer e2e: drive a real Chromium through the onion SOCKS proxy to surf a
// local origin, proving traffic traverses the overlay and that caches/DNS are
// busted on every navigation. Also exercises the web dashboard UI.
//
// Run: node --test tests/puppeteer/  (see tests/package.json scripts)

import { test, before, after, describe } from 'node:test';
import assert from 'node:assert/strict';
import puppeteer from 'puppeteer';
import { startOverlay, proxyArgs, bustUrl } from '../harness/overlay.mjs';

describe('puppeteer · onion surfing', () => {
  let overlay;
  let browser;

  before(async () => {
    overlay = await startOverlay({ hops: 3 });
  }, { timeout: 120000 });

  after(async () => {
    try { await browser?.close(); } catch {}
    await overlay?.stop();
  });

  test('surfs a site through the onion proxy and busts cache on each load', async () => {
    browser = await puppeteer.launch({ headless: true, args: proxyArgs(overlay.socksPort) });

    const before = (await overlay.status()).circuits_built;
    const beforeHits = overlay.hitsOf('/page');
    const navigations = 3;

    for (let i = 0; i < navigations; i++) {
      // Fresh, isolated context per navigation = no shared HTTP/DNS cache.
      const ctx = await browser.createBrowserContext();
      const page = await ctx.newPage();
      await page.setCacheEnabled(false);
      // Use the `localhost` hostname (not an IP) so the SOCKS proxy performs
      // remote DNS resolution at the exit — exercising DNS-through-proxy.
      const url = bustUrl(`http://localhost:${overlay.originPort}/page`, i);
      try {
        await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });
      } catch (error) {
        throw new Error(`${error.message}\n${overlay.dumpLogs()}`);
      }

      const marker = await page.$eval('#marker', (e) => e.textContent);
      assert.equal(marker, 'onion-origin', 'origin page loaded through the proxy');

      const query = await page.$eval('#query', (e) => e.textContent);
      assert.match(query, /cb=/, 'cache-buster query reached the origin');

      await ctx.close();
    }

    const after = (await overlay.status()).circuits_built;
    assert.ok(
      after >= before + navigations,
      `circuits built should grow by >= ${navigations} (was ${before}, now ${after}) — proves browser traffic went through the overlay`
    );
    assert.ok(
      overlay.hitsOf('/page') >= beforeHits + navigations,
      `origin should see ${navigations} fresh hits (cache busted): before ${beforeHits}, now ${overlay.hitsOf('/page')}`
    );
  });

  test('dashboard UI loads status and fetches through the onion network', async () => {
    // Direct (unproxied) browser to reach the dashboard itself.
    const direct = await puppeteer.launch({ headless: true, args: ['--no-sandbox'] });
    try {
      const page = await direct.newPage();
      await page.goto(`http://127.0.0.1:${overlay.uiPort}/`, { waitUntil: 'networkidle0', timeout: 30000 });

      // Status grid populates from /api/status.
      await page.waitForFunction(
        () => document.querySelector('[data-testid=stat-relays]')?.textContent !== '–',
        { timeout: 15000 }
      );
      const relays = await page.$eval('[data-testid=stat-relays]', (e) => e.textContent);
      assert.equal(relays, '3', 'dashboard shows 3 relays');

      // Drive the "browse through the onion network" form against the origin.
      const target = `http://127.0.0.1:${overlay.originPort}/ui-fetch`;
      await page.$eval('[data-testid=url-input]', (el, v) => { el.value = v; }, target);
      await page.click('[data-testid=fetch-btn]');
      await page.waitForFunction(
        () => /HTTP\//.test(document.querySelector('[data-testid=result-meta]')?.textContent || ''),
        { timeout: 30000 }
      );
      const meta = await page.$eval('[data-testid=result-meta]', (e) => e.textContent);
      assert.match(meta, /200/, 'dashboard fetch returned HTTP 200 through the onion path');
      assert.ok(overlay.hitsOf('/ui-fetch') >= 1, 'origin saw the dashboard-initiated request');
    } finally {
      await direct.close();
    }
  });
});
