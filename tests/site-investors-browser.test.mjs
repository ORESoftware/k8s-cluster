import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import puppeteer from 'puppeteer';

const root = new URL('../dist/', import.meta.url);
const artifacts = new URL('../test-results/', import.meta.url);
const chrome = process.env.CHROME_BIN;

function mime(path) {
  return ({ '.html': 'text/html', '.json': 'application/json', '.css': 'text/css', '.js': 'text/javascript', '.svg': 'image/svg+xml' })[extname(path)] ?? 'application/octet-stream';
}

async function serve() {
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://127.0.0.1');
      const stripped = url.pathname.replace(/^\/fiducia(?=\/|$)/, '') || '/';
      let path = normalize(stripped).replace(/^\/+/, '');
      if (!path || path.endsWith('/')) path += 'index.html';
      const candidate = join(root.pathname, path);
      const info = await stat(candidate).catch(() => null);
      const file = info?.isDirectory() ? join(candidate, 'index.html') : candidate;
      const body = await readFile(file);
      response.writeHead(200, { 'content-type': mime(file), 'cache-control': 'no-store' });
      response.end(body);
    } catch {
      response.writeHead(404).end('not found');
    }
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, origin: `http://127.0.0.1:${server.address().port}` };
}

async function auditPage(page, origin, prefix, driver) {
  const errors = [];
  page.on('console', (message) => { if (message.type() === 'error') errors.push(`console: ${message.text()}`); });
  page.on('pageerror', (error) => errors.push(`page: ${error.message}`));
  const response = await page.goto(`${origin}${prefix}/investors/`, { waitUntil: 'networkidle' });
  assert.equal(response.status(), 200);
  assert.match(await page.title(), /Fiducia for investors/i);
  assert.equal(await page.locator('a[href="mailto:hello@fiducia.cloud"]').count() >= 2, true);
  assert.equal(await page.locator('a[href="https://github.com/fiducia-cloud"]').count() >= 2, true);
  assert.equal(await page.locator('a[href="https://linkedin.com/in/alexanderdmills"]').count() >= 1, true);
  assert.match(await page.locator('main').innerText(), /Bootstrapped, active public implementation/);
  assert.match(await page.locator('main').innerText(), /crash-fault tolerant|Raft/i);

  const factsHref = await page.locator('a[href$="company-facts.json"]').getAttribute('href');
  const facts = await page.evaluate(async (href) => (await fetch(href)).json(), factsHref);
  assert.equal(facts.company, 'Fiducia Cloud');
  assert.equal(facts.official_contact, 'hello@fiducia.cloud');
  assert.equal(facts.website, 'https://fiducia.cloud');
  assert.equal(facts.github_organization, 'https://github.com/fiducia-cloud');
  assert.equal(facts.founder.linkedin, 'https://linkedin.com/in/alexanderdmills');
  assert.match(facts.disclaimer, /excludes private financial, customer, account, credential, and application data/i);

  await page.setViewportSize?.({ width: 390, height: 844 });
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
  assert.equal(overflow, false, `${driver}: mobile layout must not overflow horizontally`);
  await page.screenshot({ path: new URL(`${driver}-${prefix ? 'prefixed' : 'root'}.png`, artifacts).pathname, fullPage: true });
  assert.deepEqual(errors, []);
}

test('investor evidence works in Playwright and Puppeteer at root and /fiducia', async (t) => {
  const { server, origin } = await serve();
  t.after(() => server.close());

  const playwright = await chromium.launch({ executablePath: chrome || undefined, headless: true });
  t.after(() => playwright.close());
  for (const prefix of ['', '/fiducia']) {
    const page = await playwright.newPage();
    await auditPage(page, origin, prefix, 'playwright');
    await page.close();
  }

  const puppet = await puppeteer.launch({ executablePath: chrome || undefined, headless: true, args: ['--no-sandbox'] });
  t.after(() => puppet.close());
  for (const prefix of ['', '/fiducia']) {
    const page = await puppet.newPage();
    page.locator = undefined;
    const errors = [];
    page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
    page.on('pageerror', (error) => errors.push(error.message));
    const response = await page.goto(`${origin}${prefix}/investors/`, { waitUntil: 'networkidle0' });
    assert.equal(response.status(), 200);
    assert.match(await page.title(), /Fiducia for investors/i);
    const snapshot = await page.evaluate(async () => {
      const links = [...document.querySelectorAll('a')].map((a) => a.getAttribute('href'));
      const factsHref = links.find((href) => href?.endsWith('company-facts.json'));
      return { links, text: document.body.innerText, facts: await (await fetch(factsHref)).json() };
    });
    assert(snapshot.links.includes('mailto:hello@fiducia.cloud'));
    assert(snapshot.links.includes('https://github.com/fiducia-cloud'));
    assert(snapshot.links.includes('https://linkedin.com/in/alexanderdmills'));
    assert.match(snapshot.text, /Bootstrapped, active public implementation/);
    assert.equal(snapshot.facts.official_contact, 'hello@fiducia.cloud');
    await page.setViewport({ width: 390, height: 844 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false);
    await page.screenshot({ path: new URL(`puppeteer-${prefix ? 'prefixed' : 'root'}.png`, artifacts).pathname, fullPage: true });
    assert.deepEqual(errors, []);
    await page.close();
  }
});
