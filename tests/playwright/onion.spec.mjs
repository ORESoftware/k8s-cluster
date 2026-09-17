// Playwright e2e: same guarantees as the Puppeteer suite, driven through
// Playwright's Chromium. Traffic goes through the onion SOCKS proxy; caches and
// DNS are busted per navigation; the dashboard UI is exercised.
//
// Run: npx playwright test  (from tests/, see package.json)

import { test, expect } from '@playwright/test';
import { chromium } from 'playwright';
import { startOverlay, proxyArgs, bustUrl } from '../harness/overlay.mjs';

test.describe('playwright · onion surfing', () => {
  test.describe.configure({ mode: 'serial' });

  let overlay;
  let browser;

  test.beforeAll(async () => {
    test.setTimeout(120000);
    overlay = await startOverlay({ hops: 3 });
  });

  test.afterAll(async () => {
    try { await browser?.close(); } catch {}
    await overlay?.stop();
  });

  test('surfs through the onion proxy and busts cache on each load', async () => {
    browser = await chromium.launch({ headless: true, args: proxyArgs(overlay.socksPort) });

    const before = (await overlay.status()).circuits_built;
    const beforeHits = overlay.hitsOf('/page');
    const navigations = 3;

    for (let i = 0; i < navigations; i++) {
      const ctx = await browser.newContext();       // isolated cache/cookies
      const page = await ctx.newPage();
      const session = await ctx.newCDPSession(page);
      await session.send('Network.setCacheDisabled', { cacheDisabled: true });

      const url = bustUrl(`http://localhost:${overlay.originPort}/page`, i);
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });

      await expect(page.locator('#marker')).toHaveText('onion-origin');
      await expect(page.locator('#query')).toContainText('cb=');

      await ctx.close();
    }

    const after = (await overlay.status()).circuits_built;
    expect(after, 'circuits built grew — browser traffic went through the overlay')
      .toBeGreaterThanOrEqual(before + navigations);
    expect(overlay.hitsOf('/page'), 'origin saw fresh hits — cache busted')
      .toBeGreaterThanOrEqual(beforeHits + navigations);
  });

  test('dashboard UI loads status and fetches through the onion network', async () => {
    const direct = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
    try {
      const page = await direct.newPage();
      await page.goto(`http://127.0.0.1:${overlay.uiPort}/`, { waitUntil: 'networkidle', timeout: 30000 });

      await expect(page.locator('[data-testid=stat-relays]')).toHaveText('3', { timeout: 15000 });

      const target = `http://127.0.0.1:${overlay.originPort}/ui-fetch`;
      await page.locator('[data-testid=url-input]').fill(target);
      await page.locator('[data-testid=fetch-btn]').click();
      await expect(page.locator('[data-testid=result-meta]')).toContainText('200', { timeout: 30000 });
      expect(overlay.hitsOf('/ui-fetch')).toBeGreaterThanOrEqual(1);
    } finally {
      await direct.close();
    }
  });

  test('serves rendered markdown docs', async () => {
    const direct = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
    try {
      const page = await direct.newPage();
      await page.goto(`http://127.0.0.1:${overlay.uiPort}/docs`, { waitUntil: 'domcontentloaded' });
      await expect(page.locator('a', { hasText: 'overview' })).toBeVisible();
      await page.goto(`http://127.0.0.1:${overlay.uiPort}/docs/tor-interop`, { waitUntil: 'domcontentloaded' });
      await expect(page.locator('h1')).toContainText('Interoperability');
    } finally {
      await direct.close();
    }
  });
});
