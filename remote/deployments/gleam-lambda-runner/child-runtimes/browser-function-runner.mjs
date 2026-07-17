// Browser-automation child runner for the gleam-lambda-runner.
//
// Same line-delimited-JSON stdio contract as js-function-runner.mjs: read one
// {"slug","definition","request"} envelope per line from stdin, run the stored
// functionBody, write one JSON result line to stdout. The difference is the
// execution context: this runner exposes Playwright and Puppeteer plus a warm,
// reused browser, so a lambda can drive a real Chromium instance for scraping,
// screenshotting, PDF rendering, and end-to-end checks.
//
// Chromium needs filesystem and child-process access, so the Node process is
// isolated by the hardened container. Function bodies still execute in a
// code-generation-disabled vm context without process, Buffer, import, or raw
// browser launch handles.
//
// Reuse is what makes this fast: the parent engine keeps this process alive
// between invocations (ETS worker pool), so the browser launched on the first
// invoke stays warm for later ones. Each invoke gets a fresh, isolated
// BrowserContext so cookies/storage never leak between lambdas.

import { createHash, randomUUID } from 'node:crypto';
import { lookup } from 'node:dns/promises';
import { connect as connectTcp, isIP } from 'node:net';
import { env, stdin, stderr, stdout } from 'node:process';
import { createContext, Script } from 'node:vm';

const maxCompiledFunctions = positiveInt(env.LAMBDA_FUNCTION_CACHE_MAX, 128);
const maxFunctionBodyBytes = positiveInt(env.LAMBDA_FUNCTION_BODY_MAX_BYTES, 262_144);
const maxInputLineBytes = positiveInt(env.LAMBDA_CHILD_INPUT_MAX_BYTES, 6_291_456);
const maxResultBytes = positiveInt(env.LAMBDA_RESULT_MAX_BYTES, 4_194_304);

// Which library backs the shared `context.browser`. Both are always available
// via `context.playwright` / `context.puppeteer`; this only picks the default.
const defaultEngine = normalizeEngine(env.LAMBDA_BROWSER_ENGINE || 'playwright');
// Polite-by-default scraping knobs. A conservative identifying User-Agent and a
// minimum per-origin delay make robots-respecting, rate-limited scraping the
// path of least resistance rather than something each lambda must reinvent.
const defaultUserAgent =
  env.LAMBDA_SCRAPING_USER_AGENT ||
  'dd-lambda-browser/1.0 (+https://github.com/ORESoftware; respects robots.txt)';
const minPerOriginDelayMs = positiveInt(env.LAMBDA_SCRAPING_MIN_DELAY_MS, 1_000);
const robotsCacheTtlMs = positiveInt(env.LAMBDA_SCRAPING_ROBOTS_TTL_MS, 3_600_000);
const navigationTimeoutMs = positiveInt(env.LAMBDA_SCRAPING_NAV_TIMEOUT_MS, 30_000);
const allowRobotsOverride = env.LAMBDA_SCRAPING_ALLOW_ROBOTS_OVERRIDE === 'true';
const allowPrivateNetworks = env.LAMBDA_BROWSER_ALLOW_PRIVATE_NETWORKS === 'true';
const allowedHosts = new Set(
  String(env.LAMBDA_BROWSER_ALLOWED_HOSTS || '')
    .split(',')
    .map((host) => host.trim().toLowerCase())
    .filter(Boolean),
);

const containerPoolNatsUrl = env.CONTAINER_POOL_NATS_URL || env.NATS_URL || '';
const containerPoolSubjectPrefix =
  env.CONTAINER_POOL_NATS_SUBJECT_PREFIX || 'dd.remote.container_pool';
const containerPoolNatsTimeoutMs = positiveInt(env.CONTAINER_POOL_NATS_TIMEOUT_MS, 30_000);

const compiledFunctions = new Map();
const robotsCache = new Map(); // origin -> { parser, unavailable, fetchedAt }
const lastRequestAtByOrigin = new Map(); // origin -> epoch ms
const originRateTails = new Map();
const loadedLibraries = new Map(); // name -> module
let sharedBrowser = null; // { engine, browser } launched lazily, reused warm
let buffer = '';
let inputEnded = false;
let inFlight = 0;
let closing = false;

function positiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value ?? ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function normalizeEngine(value) {
  const engine = String(value || '').trim().toLowerCase();
  return engine === 'puppeteer' ? 'puppeteer' : 'playwright';
}

// Console is redirected to stderr so a lambda's logs never corrupt the
// stdout result stream the parent parses line-by-line.
const safeConsole = Object.freeze(
  Object.fromEntries(
    ['debug', 'error', 'info', 'log', 'warn'].map((level) => [
      level,
      (...args) => {
        const rendered = args
          .map((arg) => (typeof arg === 'string' ? arg : safeStringify(arg)))
          .join(' ');
        stderr.write(`[lambda:${level}] ${rendered}\n`);
      },
    ]),
  ),
);
globalThis.console = safeConsole;

function safeStringify(value) {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function hashBody(body) {
  return createHash('sha256').update(body).digest('hex');
}

function compileFunction(functionBody) {
  const cacheKey = hashBody(functionBody);
  const cached = compiledFunctions.get(cacheKey);
  if (cached) {
    return cached;
  }
  const script = new Script(
    `"use strict"; (async (request, context, console) => {\n${functionBody}\n})(request, context, console);`,
    { filename: `lambda-browser-${cacheKey.slice(0, 12)}.mjs` },
  );
  compiledFunctions.set(cacheKey, script);
  while (compiledFunctions.size > maxCompiledFunctions) {
    const oldestKey = compiledFunctions.keys().next().value;
    compiledFunctions.delete(oldestKey);
  }
  return script;
}

function assertSlug(slug) {
  const normalized = String(slug || '').trim().toLowerCase();
  if (!/^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$/.test(normalized)) {
    throw new Error('valid lambda slug is required');
  }
  return normalized;
}

// Lazily import a browser library. Missing libraries surface a clear, actionable
// error instead of a raw module-resolution stack trace.
async function loadLibrary(name) {
  if (loadedLibraries.has(name)) {
    return loadedLibraries.get(name);
  }
  let mod;
  try {
    mod = await import(name);
  } catch (error) {
    throw new Error(
      `browser runtime could not load "${name}" (is it installed in the image?): ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
  }
  const resolved = mod?.default ?? mod;
  loadedLibraries.set(name, resolved);
  return resolved;
}

// Chromium launch flags that make it run inside the hardened, non-root,
// read-only container. `--no-sandbox` is required because the container drops
// the capabilities Chromium's own sandbox needs; the container is the sandbox.
function chromiumLaunchArgs() {
  return [
    '--no-sandbox',
    '--disable-setuid-sandbox',
    '--disable-gpu',
    '--headless=new',
  ];
}

async function launchBrowser(engine) {
  if (engine === 'puppeteer') {
    const puppeteer = await loadLibrary('puppeteer-core');
    const { chromium } = await loadLibrary('playwright');
    return puppeteer.launch({
      headless: true,
      executablePath: chromium.executablePath(),
      args: chromiumLaunchArgs(),
    });
  }
  const { chromium } = await loadLibrary('playwright');
  return chromium.launch({ headless: true, args: chromiumLaunchArgs() });
}

// One warm browser per process, reused across invocations. If it has crashed or
// disconnected we relaunch transparently.
async function getSharedBrowser(engine) {
  if (sharedBrowser && sharedBrowser.engine === engine && isBrowserConnected(sharedBrowser.browser)) {
    return sharedBrowser.browser;
  }
  if (sharedBrowser) {
    await closeBrowserQuietly(sharedBrowser.browser);
    sharedBrowser = null;
  }
  const browser = await launchBrowser(engine);
  sharedBrowser = { engine, browser };
  return browser;
}

function isBrowserConnected(browser) {
  try {
    // Playwright: isConnected(); Puppeteer: connected getter or isConnected().
    if (typeof browser.isConnected === 'function') {
      return browser.isConnected();
    }
    if (typeof browser.connected === 'boolean') {
      return browser.connected;
    }
  } catch {
    return false;
  }
  return true;
}

async function closeBrowserQuietly(browser) {
  try {
    await browser.close();
  } catch {
    // A browser we are discarding failing to close is not actionable.
  }
}

// ---- Responsible-scraping helpers ---------------------------------------------
// These make SSRF-resistant, robots-aware, paced scraping the default.

function originOf(url) {
  return new URL(url).origin;
}

function isPrivateIpAddress(host) {
  if (isIP(host) === 4) {
    const [a, b] = host.split('.').map(Number);
    return (
      a === 0 ||
      a === 10 ||
      a === 127 ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 100 && b >= 64 && b <= 127) ||
      a >= 224
    );
  }
  if (isIP(host) === 6) {
    const normalized = host.toLowerCase();
    if (normalized.startsWith('::ffff:')) {
      return isPrivateIpAddress(normalized.slice('::ffff:'.length));
    }
    return (
      normalized === '::' ||
      normalized === '::1' ||
      normalized.startsWith('fc') ||
      normalized.startsWith('fd') ||
      /^fe[89ab]/.test(normalized)
    );
  }
  return false;
}

async function assertSafeHttpUrl(rawUrl) {
  const url = new URL(rawUrl);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`browser request scheme ${url.protocol} is not allowed`);
  }
  if (url.username || url.password) {
    throw new Error('browser request URL credentials are not allowed');
  }
  const host = url.hostname.toLowerCase().replace(/^\[/, '').replace(/\]$/, '');
  if (allowedHosts.has(host) || allowPrivateNetworks) {
    return url;
  }
  if (
    host === 'localhost' ||
    host.endsWith('.localhost') ||
    host.endsWith('.local') ||
    host.endsWith('.internal') ||
    isPrivateIpAddress(host)
  ) {
    throw new Error(`browser request target ${host} is private or local`);
  }
  const addresses = await lookup(host, { all: true, verbatim: true });
  if (addresses.length === 0 || addresses.some(({ address }) => isPrivateIpAddress(address))) {
    throw new Error(`browser request target ${host} resolves to a private or local address`);
  }
  return url;
}

async function assertSafeBrowserRequest(rawUrl) {
  const protocol = new URL(rawUrl).protocol;
  if (protocol === 'about:' || protocol === 'blob:' || protocol === 'data:') {
    return;
  }
  await assertSafeHttpUrl(rawUrl);
}

async function safeFetch(rawUrl, options) {
  let url = await assertSafeHttpUrl(rawUrl);
  for (let redirects = 0; redirects <= 5; redirects += 1) {
    const response = await fetch(url, { ...options, redirect: 'manual' });
    if (response.status < 300 || response.status >= 400) {
      return response;
    }
    const location = response.headers.get('location');
    if (!location) {
      throw new Error('robots.txt redirect omitted its Location header');
    }
    url = await assertSafeHttpUrl(new URL(location, url).href);
  }
  throw new Error('robots.txt redirect limit exceeded');
}

function setBounded(map, key, value, maxEntries = 256) {
  map.delete(key);
  map.set(key, value);
  while (map.size > maxEntries) {
    map.delete(map.keys().next().value);
  }
}

async function fetchRobots(origin) {
  const cached = robotsCache.get(origin);
  const now = Date.now();
  if (cached && now - cached.fetchedAt < robotsCacheTtlMs) {
    return cached;
  }
  try {
    const robotsUrl = `${origin}/robots.txt`;
    const response = await safeFetch(robotsUrl, {
      headers: { 'user-agent': defaultUserAgent },
      signal: AbortSignal.timeout(Math.min(navigationTimeoutMs, 15_000)),
    });
    if (response.status >= 500) {
      throw new Error(`robots.txt returned ${response.status}`);
    }
    const createRobotsParser = await loadLibrary('robots-parser');
    const parser = createRobotsParser(robotsUrl, response.ok ? await response.text() : '');
    const record = { parser, unavailable: false, fetchedAt: now };
    setBounded(robotsCache, origin, record);
    return record;
  } catch (error) {
    const record = {
      parser: null,
      unavailable: true,
      fetchedAt: now,
      error: error instanceof Error ? error.message : String(error),
    };
    setBounded(robotsCache, origin, record);
    return record;
  }
}

async function isAllowed(url, userAgent = defaultUserAgent) {
  await assertSafeHttpUrl(url);
  const robots = await fetchRobots(originOf(url));
  return !robots.unavailable && robots.parser.isAllowed(url, userAgent) !== false;
}

async function assertAllowed(url, userAgent = defaultUserAgent) {
  if (!(await isAllowed(url, userAgent))) {
    throw new Error(`robots.txt is unavailable or disallows ${url} for this user-agent`);
  }
}

async function respectRateLimit(origin) {
  const prior = originRateTails.get(origin) || Promise.resolve();
  const turn = prior.then(async () => {
    const last = lastRequestAtByOrigin.get(origin);
    const now = Date.now();
    if (last !== undefined) {
      const wait = minPerOriginDelayMs - (now - last);
      if (wait > 0) {
        await new Promise((resolve) => setTimeout(resolve, wait));
      }
    }
    setBounded(lastRequestAtByOrigin, origin, Date.now(), 1024);
  });
  const tail = turn.catch(() => {});
  originRateTails.set(origin, tail);
  await turn;
  if (originRateTails.get(origin) === tail) {
    originRateTails.delete(origin);
  }
}

// robots-check + per-origin rate-limit + navigate. `respectRobots` defaults to
// true; a caller must explicitly pass false to bypass the check (e.g. for a
// site they own and have authorization to crawl aggressively).
async function politeGoto(page, url, options = {}) {
  const { respectRobots = true, userAgent = defaultUserAgent, ...gotoOptions } = options;
  if (respectRobots) {
    await assertAllowed(url, userAgent);
  } else if (!allowRobotsOverride) {
    throw new Error('robots.txt override is disabled by operator policy');
  }
  await respectRateLimit(originOf(url));
  return page.goto(url, { timeout: navigationTimeoutMs, ...gotoOptions });
}

function buildScrapingHelpers() {
  return Object.freeze({
    userAgent: defaultUserAgent,
    minPerOriginDelayMs,
    isAllowed,
    assertAllowed,
    politeGoto,
  });
}

// ---- Container-pool dispatch (self-contained, image-safe) ----------------------
// A minimal NATS request/reply so a browser lambda can fan work out to a pool,
// mirroring js-function-runner.mjs but without importing repo-relative modules
// (this file is copied standalone into the runtime image).

function connectPayload(parsed) {
  const payload = { verbose: false, pedantic: false, lang: 'nodejs', name: 'dd-gleam-lambda-runner-browser' };
  if (parsed.username && parsed.password) {
    payload.user = decodeURIComponent(parsed.username);
    payload.pass = decodeURIComponent(parsed.password);
  } else if (parsed.username) {
    payload.auth_token = decodeURIComponent(parsed.username);
  }
  return JSON.stringify(payload);
}

function parseNatsFrame(buf) {
  let offset = 0;
  while (offset < buf.length) {
    const lineEnd = buf.indexOf('\r\n', offset, 'utf8');
    if (lineEnd < 0) return { buffer: buf.subarray(offset) };
    const line = buf.subarray(offset, lineEnd).toString('utf8');
    offset = lineEnd + 2;
    if (!line || line === '+OK' || line.startsWith('INFO') || line === 'PONG') continue;
    if (line === 'PING') return { ping: true, buffer: buf.subarray(offset) };
    if (line.startsWith('-ERR')) throw new Error(`NATS error: ${line}`);
    if (line.startsWith('MSG ')) {
      const parts = line.split(' ');
      const byteCount = Number.parseInt(parts.at(-1) || '', 10);
      if (!Number.isFinite(byteCount) || byteCount < 0) throw new Error(`invalid NATS MSG frame: ${line}`);
      if (buf.length < offset + byteCount + 2) return { buffer: buf.subarray(lineEnd - line.length) };
      const payload = buf.subarray(offset, offset + byteCount);
      return { payload, buffer: buf.subarray(offset + byteCount + 2) };
    }
  }
  return { buffer: buf.subarray(0, 0) };
}

function natsRequest(subject, payload, timeoutMs = containerPoolNatsTimeoutMs) {
  if (!containerPoolNatsUrl) {
    return Promise.reject(new Error('NATS_URL or CONTAINER_POOL_NATS_URL is required'));
  }
  const parsed = new URL(containerPoolNatsUrl);
  if (parsed.protocol !== 'nats:' || !parsed.hostname) {
    return Promise.reject(new Error('container pool NATS URL must use nats://'));
  }
  const inbox = `_INBOX.${randomUUID().replaceAll('-', '')}`;
  const encoded = Buffer.from(JSON.stringify(payload), 'utf8');
  return new Promise((resolve, reject) => {
    let settled = false;
    let buf = Buffer.alloc(0);
    const socket = connectTcp({ host: parsed.hostname, port: parsed.port ? Number(parsed.port) : 4222 });
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      error ? reject(error) : resolve(value);
    };
    const timer = setTimeout(
      () => finish(new Error(`container pool NATS request timed out after ${timeoutMs}ms`)),
      Math.max(1_000, timeoutMs),
    );
    socket.setTimeout(Math.max(1_000, timeoutMs));
    socket.on('connect', () => {
      socket.write(`CONNECT ${connectPayload(parsed)}\r\n`);
      socket.write(`SUB ${inbox} 1\r\n`);
      socket.write(`PUB ${subject} ${inbox} ${encoded.length}\r\n`);
      socket.write(encoded);
      socket.write('\r\nPING\r\n');
    });
    socket.on('data', (chunk) => {
      try {
        buf = Buffer.concat([buf, chunk]);
        while (buf.length > 0) {
          const frame = parseNatsFrame(buf);
          buf = frame.buffer;
          if (frame.ping) {
            socket.write('PONG\r\n');
            continue;
          }
          if (frame.payload) {
            const text = frame.payload.toString('utf8');
            try {
              finish(null, JSON.parse(text));
            } catch {
              finish(null, text);
            }
            return;
          }
          break;
        }
      } catch (error) {
        finish(error);
      }
    });
    socket.on('timeout', () => finish(new Error(`container pool NATS request timed out after ${timeoutMs}ms`)));
    socket.on('error', finish);
    socket.on('close', () => {
      if (!settled) finish(new Error('container pool NATS connection closed before a reply was received'));
    });
  });
}

async function dispatchContainerPool(pool, payload = {}, options = {}) {
  const poolSlug = assertSlug(pool);
  const subject = options.subject || `${containerPoolSubjectPrefix}.${poolSlug}.requests`;
  const request = {
    requestId: options.requestId || randomUUID(),
    poolSlug,
    payload,
    ...(options.path ? { path: options.path } : {}),
    ...(options.headers ? { headers: options.headers } : {}),
  };
  return natsRequest(subject, request, positiveInt(options.timeoutMs, containerPoolNatsTimeoutMs));
}

// ---- Invocation ---------------------------------------------------------------

function resolveDefinition(envelope) {
  const definition = envelope.definition || (envelope.functionBody ? envelope : null);
  if (!definition || typeof definition !== 'object') {
    throw new Error('lambda definition with functionBody is required');
  }
  definition.slug = assertSlug(definition.slug || envelope.slug);
  if (definition.status === 'paused' || definition.status === 'archived') {
    throw new Error(`lambda function is ${definition.status}`);
  }
  return definition;
}

function pickEngine(envelope, definition) {
  const runtimeEngine = definition?.runtime === 'puppeteer' ? 'puppeteer' : definition?.runtime;
  return normalizeEngine(
    envelope.browserEngine ||
      definition?.browserEngine ||
      definition?.metaData?.browserEngine ||
      runtimeEngine ||
      defaultEngine,
  );
}

async function invoke(line) {
  const envelope = JSON.parse(line);
  const definition = resolveDefinition(envelope);
  const functionBody = String(definition.functionBody || '');
  const request = envelope.request || {};
  const engine = pickEngine(envelope, definition);

  if (!functionBody.trim()) {
    throw new Error('functionBody is required');
  }
  if (Buffer.byteLength(functionBody, 'utf8') > maxFunctionBodyBytes) {
    throw new Error('functionBody exceeds configured byte limit');
  }

  const script = compileFunction(functionBody);

  // A check request validates + compiles without launching a browser, so the
  // control plane can lint a definition cheaply.
  if (envelope.checkOnly === true || envelope.mode === 'check') {
    return {
      ok: true,
      check: { runtime: definition.runtime, slug: definition.slug, engine },
      cachedFunctions: compiledFunctions.size,
    };
  }

  const browser = await getSharedBrowser(engine);

  // Fresh isolated context per invoke so cookies/storage/permissions from one
  // lambda never leak into the next, while the browser process stays warm.
  const isolate = await createIsolatedContext(engine, browser);
  try {
    const context = Object.freeze({
      id: definition.id,
      invocationId: envelope.invocationId,
      slug: definition.slug,
      engine,
      page: await isolate.newPage(),
      newPage: () => isolate.newPage(),
      scraping: buildScrapingHelpers(),
      containerPool: Object.freeze({ dispatch: dispatchContainerPool, request: dispatchContainerPool }),
      meta: Object.freeze({
        runtime: definition.runtime,
        labels: definition.labels,
        metaData: definition.metaData,
        ...(envelope.meta || {}),
      }),
    });

    const sandbox = createContext(
      { request, context, console: safeConsole },
      {
        name: `lambda-browser:${definition.slug}`,
        codeGeneration: { strings: false, wasm: false },
      },
    );
    const result = await script.runInContext(sandbox);
    return {
      ok: true,
      result: result ?? null,
      invocationId: context.invocationId,
      engine,
      cachedFunctions: compiledFunctions.size,
    };
  } finally {
    await isolate.close();
  }
}

// Build an isolated browsing context + page factory that works for either
// library, so lambda code and the newPage() helper behave the same regardless
// of engine.
async function createIsolatedContext(engine, browser) {
  if (engine === 'puppeteer') {
    const createIncognito =
      browser.createBrowserContext?.bind(browser) || browser.createIncognitoBrowserContext?.bind(browser);
    if (!createIncognito) {
      throw new Error('Puppeteer runtime cannot create an isolated browser context');
    }
    const ctx = await createIncognito();
    return {
      newPage: async () => {
        const page = await ctx.newPage();
        await page.setUserAgent(defaultUserAgent);
        await page.setRequestInterception(true);
        page.on('request', (request) => {
          void assertSafeBrowserRequest(request.url()).then(
            () => request.continue(),
            (error) => {
              safeConsole.warn(error instanceof Error ? error.message : String(error));
              return request.abort('blockedbyclient');
            },
          );
        });
        return page;
      },
      close: () => ctx.close().catch(() => {}),
    };
  }
  const ctx = await browser.newContext({ userAgent: defaultUserAgent });
  await ctx.route('**/*', async (route) => {
    try {
      await assertSafeBrowserRequest(route.request().url());
      await route.continue();
    } catch (error) {
      safeConsole.warn(error instanceof Error ? error.message : String(error));
      await route.abort('blockedbyclient');
    }
  });
  return {
    newPage: () => ctx.newPage(),
    close: () => ctx.close().catch(() => {}),
  };
}

async function handleLine(line) {
  try {
    const result = await invoke(line);
    writeResult(result);
  } catch (error) {
    writeResult({ ok: false, error: error instanceof Error ? error.message : String(error) });
  }
}

function dispatchLine(line) {
  inFlight += 1;
  void handleLine(line).finally(() => {
    inFlight -= 1;
    finishWhenIdle();
  });
}

function finishWhenIdle() {
  if (!inputEnded || inFlight !== 0 || closing) {
    return;
  }
  closing = true;
  const close = sharedBrowser ? closeBrowserQuietly(sharedBrowser.browser) : Promise.resolve();
  void close.finally(() => process.exit(0));
}

function writeResult(result) {
  let encoded = JSON.stringify(result);
  if (Buffer.byteLength(encoded, 'utf8') > maxResultBytes) {
    encoded = JSON.stringify({ ok: false, error: 'lambda result exceeds configured byte limit' });
  }
  stdout.write(`${encoded}\n`);
}

stdin.setEncoding('utf8');
stdin.on('data', (chunk) => {
  buffer += chunk;
  if (Buffer.byteLength(buffer, 'utf8') > maxInputLineBytes) {
    buffer = '';
    writeResult({ ok: false, error: 'lambda input exceeds configured byte limit' });
    return;
  }
  let newlineIndex = buffer.indexOf('\n');
  while (newlineIndex >= 0) {
    const line = buffer.slice(0, newlineIndex).trim();
    buffer = buffer.slice(newlineIndex + 1);
    if (line) {
      dispatchLine(line);
    }
    newlineIndex = buffer.indexOf('\n');
  }
});

stdin.on('end', () => {
  const finalLine = buffer.trim();
  buffer = '';
  if (finalLine) {
    dispatchLine(finalLine);
  }
  inputEnded = true;
  finishWhenIdle();
});

// Best-effort browser teardown so a rolling restart doesn't leak a Chromium.
for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => {
    if (sharedBrowser) {
      void closeBrowserQuietly(sharedBrowser.browser).finally(() => process.exit(0));
    } else {
      process.exit(0);
    }
  });
}
