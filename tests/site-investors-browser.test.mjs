import { createServer } from 'node:http';
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { extname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import puppeteer from 'puppeteer';

const distRoot = fileURLToPath(new URL('../dist/', import.meta.url));
const artifactsDir = fileURLToPath(new URL('../test-results/', import.meta.url));
const chrome = process.env.CHROME_BIN;
const publicBase = normalizePublicBase(process.env.PUBLIC_BASE ?? '/fiducia');
const mode = publicBase ? 'prefixed' : 'root';

const mimeTypes = new Map([
  ['.css', 'text/css; charset=utf-8'],
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.svg', 'image/svg+xml; charset=utf-8'],
]);

const forbiddenRenderedClaims = [
  /\b(?:ARR|MRR)\b/i,
  /\b(?:annual|monthly) recurring revenue\b/i,
  /\brevenue\b[^.\n]{0,48}(?:\$|USD|\d|million|thousand)/i,
  /(?:\$|USD)\s?\d[^.\n]{0,48}\b(?:revenue|valuation|funding|credits?)\b/i,
  /\b\d[\d,]*(?:\+)?\s+(?:paying\s+)?customers?\b/i,
  /\b(?:raised|closed|secured)\b[^.\n]{0,48}\b(?:pre-seed|seed|series|funding|capital)\b/i,
  /\b(?:pre-seed|seed|series [a-z])\s+(?:round|funding|backed)\b/i,
  /\b(?:incorporated|headquartered|registered|domiciled|located)\s+in\b/i,
  /\b(?:LLC|C-?Corp(?:oration)?|S-?Corp)\b/i,
  /\b(?:awarded|approved for|received|secured)\b[^.\n]{0,48}\bcredits?\b/i,
  /\b\d[\d,]*(?:\+)?\s+(?:fundraising|computing|AI|conference|speaking)?\s*applications?\b/i,
];

function normalizePublicBase(value) {
  const trimmed = String(value).trim();
  if (trimmed === '' || trimmed === '/') return '';
  const normalized = `/${trimmed.replace(/^\/+|\/+$/g, '')}`;
  assert.match(
    normalized,
    /^\/[A-Za-z0-9/_-]+$/,
    `PUBLIC_BASE contains unsupported characters: ${value}`,
  );
  return normalized;
}

function mountedPath(pathname) {
  if (!publicBase) return pathname;
  if (pathname === publicBase) return '/';
  if (!pathname.startsWith(`${publicBase}/`)) return null;
  return pathname.slice(publicBase.length) || '/';
}

function isInsideDist(candidate) {
  const rel = relative(distRoot, candidate);
  return rel === '' || (!rel.startsWith(`..${sep}`) && rel !== '..' && !isAbsolute(rel));
}

async function serve() {
  const requests = [];
  const server = createServer(async (request, response) => {
    const startedAt = Date.now();
    let statusCode = 500;
    try {
      if (!['GET', 'HEAD'].includes(request.method ?? 'GET')) {
        statusCode = 405;
        response.writeHead(statusCode, { allow: 'GET, HEAD' }).end();
        return;
      }

      const url = new URL(request.url ?? '/', 'http://127.0.0.1');
      const pathAtMount = mountedPath(decodeURIComponent(url.pathname));
      if (pathAtMount === null || pathAtMount.includes('\0')) {
        statusCode = 404;
        response.writeHead(statusCode).end('not found');
        return;
      }

      let relativePath = pathAtMount.replace(/^\/+/, '');
      if (!relativePath || relativePath.endsWith('/')) relativePath += 'index.html';
      const candidate = resolve(distRoot, relativePath);
      if (!isInsideDist(candidate)) {
        statusCode = 403;
        response.writeHead(statusCode).end('forbidden');
        return;
      }

      const info = await stat(candidate).catch(() => null);
      const file = info?.isDirectory() ? resolve(candidate, 'index.html') : candidate;
      if (!isInsideDist(file)) {
        statusCode = 403;
        response.writeHead(statusCode).end('forbidden');
        return;
      }
      const body = await readFile(file);
      statusCode = 200;
      response.writeHead(statusCode, {
        'cache-control': 'no-store',
        'content-type': mimeTypes.get(extname(file)) ?? 'application/octet-stream',
        'x-content-type-options': 'nosniff',
      });
      response.end(request.method === 'HEAD' ? undefined : body);
    } catch {
      statusCode = 404;
      response.writeHead(statusCode).end('not found');
    } finally {
      requests.push({
        method: request.method ?? 'GET',
        path: request.url ?? '/',
        status: statusCode,
        duration_ms: Date.now() - startedAt,
      });
    }
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  assert(address && typeof address === 'object');
  return {
    origin: `http://127.0.0.1:${address.port}`,
    requests,
    close: () => new Promise((resolveClose, rejectClose) => {
      server.close((error) => (error ? rejectClose(error) : resolveClose()));
    }),
  };
}

function pagePath() {
  return `${publicBase}/investors/` || '/investors/';
}

function factsPath() {
  return `${publicBase}/company-facts.json` || '/company-facts.json';
}

function wrongMountPath() {
  return publicBase ? '/investors/' : '/fiducia/investors/';
}

async function setViewport(page, driver, width, height) {
  if (driver === 'playwright') await page.setViewportSize({ width, height });
  else await page.setViewport({ width, height });
}

function observePage(page, origin) {
  const diagnostics = { console_errors: [], page_errors: [], request_failures: [], bad_responses: [] };
  page.on('console', (message) => {
    if (message.type() === 'error') diagnostics.console_errors.push(message.text());
  });
  page.on('pageerror', (error) => diagnostics.page_errors.push(error.message));
  page.on('requestfailed', (request) => {
    diagnostics.request_failures.push(`${request.method()} ${request.url()} ${request.failure()?.errorText ?? ''}`.trim());
  });
  page.on('response', (response) => {
    const url = new URL(response.url());
    if (url.origin === origin && response.status() >= 400) {
      diagnostics.bad_responses.push(`${response.status()} ${url.pathname}`);
    }
  });
  return diagnostics;
}

async function snapshotPage(page) {
  return page.evaluate(async () => {
    const links = [...document.querySelectorAll('a')].map((link) => ({
      href: link.getAttribute('href'),
      text: (link.textContent ?? '').trim(),
      target: link.getAttribute('target'),
      rel: link.getAttribute('rel'),
    }));
    const duplicateIds = [...document.querySelectorAll('[id]')]
      .map((element) => element.id)
      .filter((id, index, all) => all.indexOf(id) !== index);
    const alternateFacts = document.querySelector('link[rel="alternate"][type="application/json"]');
    const factsAnchor = [...document.querySelectorAll('a')].find((link) =>
      link.getAttribute('href')?.endsWith('company-facts.json'),
    );
    const factsResponse = factsAnchor ? await fetch(factsAnchor.href, { cache: 'no-store' }) : null;
    const facts = factsResponse?.ok ? await factsResponse.json() : null;
    const main = document.querySelector('main');
    const h1 = document.querySelector('h1');
    return {
      alternate_json: alternateFacts?.href ?? null,
      canonical: document.querySelector('link[rel="canonical"]')?.href ?? null,
      duplicate_ids: duplicateIds,
      facts,
      facts_content_type: factsResponse?.headers.get('content-type') ?? null,
      facts_url: factsAnchor?.href ?? null,
      fault_model: document.querySelector('meta[name="fiducia:consensus-model"]')?.getAttribute('content') ?? null,
      facts_status: factsResponse?.status ?? null,
      h1_count: document.querySelectorAll('h1').length,
      h1_text: (h1?.textContent ?? '').trim(),
      html_lang: document.documentElement.lang,
      links,
      main_text: (main?.innerText ?? '').trim(),
      nav_count: document.querySelectorAll('nav').length,
      og_url: document.querySelector('meta[property="og:url"]')?.getAttribute('content') ?? null,
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
      page_title: document.title,
      page_url: location.href,
      rendered_background: getComputedStyle(document.body).backgroundColor,
      visible_h1: Boolean(h1 && h1.getBoundingClientRect().width > 0 && h1.getBoundingClientRect().height > 0),
    };
  });
}

function assertSafePublicClaims(text, label) {
  for (const pattern of forbiddenRenderedClaims) {
    assert.doesNotMatch(text, pattern, `${label}: unverified mutable claim matched ${pattern}`);
  }
}

function assertSnapshot(snapshot, origin) {
  const expectedPage = `${origin}${pagePath()}`;
  const expectedCanonical = `https://fiducia.cloud${pagePath()}`;
  const expectedFacts = `https://fiducia.cloud${factsPath()}`;
  assert.equal(snapshot.page_url, expectedPage);
  assert.match(snapshot.page_title, /Fiducia for investors/i);
  assert.equal(snapshot.html_lang, 'en');
  assert.equal(snapshot.h1_count, 1);
  assert.match(snapshot.h1_text, /coordination layer for autonomous systems/i);
  assert.equal(snapshot.visible_h1, true);
  assert.equal(snapshot.nav_count, 1);
  assert.deepEqual(snapshot.duplicate_ids, []);
  assert.equal(snapshot.canonical, expectedCanonical);
  assert.equal(snapshot.og_url, expectedCanonical);
  assert.equal(snapshot.alternate_json, expectedFacts);
  assert.equal(snapshot.facts_url, `${origin}${factsPath()}`);
  assert.equal(snapshot.facts_status, 200);
  assert.match(snapshot.facts_content_type ?? '', /^application\/json\b/i);
  assert.equal(snapshot.facts.company, 'Fiducia Cloud');
  assert.equal(snapshot.facts.official_contact, 'hello@fiducia.cloud');
  assert.equal(snapshot.facts.website, 'https://fiducia.cloud');
  assert.equal(snapshot.facts.github_organization, 'https://github.com/fiducia-cloud');
  assert.equal(snapshot.facts.founder.linkedin, 'https://linkedin.com/in/alexanderdmills');
  assert.match(snapshot.facts.disclaimer, /excludes private financial, customer, account, credential, and application data/i);
  assert.match(snapshot.main_text, /Bootstrapped, active public implementation/);
  assert.equal(snapshot.fault_model, 'crash-fault tolerant (CFT), not Byzantine');
  assert.equal(
    snapshot.facts.architecture.includes('crash-fault-tolerance-cft-not-byzantine'),
    true,
  );
  assert.notEqual(snapshot.rendered_background, 'rgba(0, 0, 0, 0)');

  const hrefs = snapshot.links.map(({ href }) => href);
  assert.equal(hrefs.filter((href) => href === 'mailto:hello@fiducia.cloud').length >= 2, true);
  assert.equal(hrefs.filter((href) => href === 'https://github.com/fiducia-cloud').length >= 2, true);
  assert.equal(hrefs.includes('https://linkedin.com/in/alexanderdmills'), true);
  assert.equal(hrefs.includes(factsPath()), true);
  for (const link of snapshot.links) {
    assert.ok(link.text || link.href?.startsWith('mailto:'), `link has no accessible text: ${link.href}`);
    assert.doesNotMatch(link.href ?? '', /^javascript:/i);
    if (link.target === '_blank') assert.match(link.rel ?? '', /\bnoopener\b/);
  }

  assertSafePublicClaims(snapshot.main_text, 'rendered investor page');
  assertSafePublicClaims(JSON.stringify(snapshot.facts), 'rendered company facts');
}

async function writeEvidence(page, driver, diagnostics, snapshot, requests) {
  await mkdir(artifactsDir, { recursive: true });
  const prefix = `${driver}-${mode}`;
  await Promise.all([
    page.screenshot({ path: resolve(artifactsDir, `${prefix}-mobile.png`), fullPage: true }),
    page.content().then((html) => writeFile(resolve(artifactsDir, `${prefix}.html`), html)),
    writeFile(
      resolve(artifactsDir, `${prefix}.json`),
      `${JSON.stringify({ diagnostics, mode, public_base: publicBase || '/', requests, snapshot }, null, 2)}\n`,
    ),
  ]);
}

async function auditBrowser(driver, launch, origin, requests) {
  const browser = await launch();
  let context;
  let page;
  try {
    if (driver === 'playwright') {
      context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      page = await context.newPage();
    } else {
      page = await browser.newPage();
      await setViewport(page, driver, 1280, 900);
    }

    const diagnostics = observePage(page, origin);
    let snapshot;
    try {
      const response = await page.goto(`${origin}${pagePath()}`, {
        waitUntil: driver === 'playwright' ? 'networkidle' : 'networkidle0',
      });
      assert(response, `${driver}: navigation produced no response`);
      assert.equal(response.status(), 200);
      snapshot = await snapshotPage(page);
      assertSnapshot(snapshot, origin);

      await page.screenshot({ path: resolve(artifactsDir, `${driver}-${mode}-desktop.png`), fullPage: true });
      await setViewport(page, driver, 390, 844);
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
      snapshot = await snapshotPage(page);
      assert.equal(snapshot.overflow, false, `${driver}: mobile layout must not overflow horizontally`);
      assert.deepEqual(diagnostics.console_errors, []);
      assert.deepEqual(diagnostics.page_errors, []);
      assert.deepEqual(diagnostics.request_failures, []);
      assert.deepEqual(diagnostics.bad_responses, []);
    } finally {
      if (page) {
        await writeEvidence(page, driver, diagnostics, snapshot ?? null, requests).catch((error) => {
          console.error(`failed to write ${driver} evidence: ${error.message}`);
        });
      }
    }
  } finally {
    await page?.close().catch(() => {});
    await context?.close().catch(() => {});
    await browser.close();
  }
}

test(`investor evidence is hardened in real browsers (${mode})`, async (t) => {
  await mkdir(artifactsDir, { recursive: true });
  const server = await serve();
  t.after(() => server.close());

  const wrongMount = await fetch(`${server.origin}${wrongMountPath()}`);
  assert.equal(wrongMount.status, 404, 'the test server must expose only the selected deployment base');

  const launchArgs = ['--disable-dev-shm-usage', '--no-sandbox'];
  await t.test('Playwright', async () => {
    await auditBrowser(
      'playwright',
      () => chromium.launch({ executablePath: chrome || undefined, headless: true, args: launchArgs }),
      server.origin,
      server.requests,
    );
  });
  await t.test('Puppeteer', async () => {
    await auditBrowser(
      'puppeteer',
      () => puppeteer.launch({ executablePath: chrome || undefined, headless: true, args: launchArgs }),
      server.origin,
      server.requests,
    );
  });
});
