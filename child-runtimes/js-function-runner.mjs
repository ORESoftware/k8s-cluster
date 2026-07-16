import { createHash, randomUUID } from 'node:crypto';
import { Buffer } from 'node:buffer';
import { connect as connectTcp } from 'node:net';
import { env, stdin, stderr, stdout } from 'node:process';
import { compileFunction as compileVmFunction, createContext } from 'node:vm';

// Source-of-truth NATS subject constants. The repo is mounted at
// /opt/dd-next-1 inside the pod so this relative path resolves identically
// in dev, CI, and prod containers.
import {
  containerPoolLanguageRequestsSubject,
} from '../../../libs/nats/subject-defs/generated/javascript/index.mjs';

const maxCompiledFunctions = positiveInt(env.LAMBDA_FUNCTION_CACHE_MAX, 128);
const maxFunctionBodyBytes = positiveInt(env.LAMBDA_FUNCTION_BODY_MAX_BYTES, 262_144);
const maxInputLineBytes = positiveInt(env.LAMBDA_CHILD_INPUT_MAX_BYTES, 6_291_456);
const maxResultBytes = positiveInt(env.LAMBDA_RESULT_MAX_BYTES, 1_048_576);
const containerPoolNatsUrl = env.CONTAINER_POOL_NATS_URL || env.NATS_URL || '';
// Optional override; when unset, every per-pool subject is built from the
// generated containerPoolLanguageRequestsSubject() formatter so the dot
// layout always tracks the source-of-truth schema.
const containerPoolSubjectPrefix = env.CONTAINER_POOL_NATS_SUBJECT_PREFIX || '';
const containerPoolNatsTimeoutMs = positiveInt(env.CONTAINER_POOL_NATS_TIMEOUT_MS, 30_000);
const browserExecutablePath = env.LAMBDA_BROWSER_EXECUTABLE_PATH || '/usr/bin/chromium-browser';
const browserLaunchArgs = Object.freeze([
  '--disable-crash-reporter',
  '--disable-crashpad',
  '--disable-dev-shm-usage',
  '--disable-gpu',
  '--disable-setuid-sandbox',
  '--no-sandbox',
]);

const compiledFunctions = new Map();
let buffer = '';

function positiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value || ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

const safeConsole = Object.freeze(
  Object.fromEntries(
    ['debug', 'error', 'info', 'log', 'warn'].map((level) => [
      level,
      (...args) => {
        const rendered = args
          .map((arg) => (typeof arg === 'string' ? arg : JSON.stringify(arg)))
          .join(' ');
        stderr.write(`[lambda:${level}] ${rendered}\n`);
      },
    ]),
  ),
);

globalThis.console = safeConsole;

// User functions execute in a context with an explicit safe-global surface.
// This keeps process/require/Buffer and host module loading unavailable without
// mutating globals that Playwright, Puppeteer, and their dependencies require.
const lambdaGlobals = Object.assign(Object.create(null), {
  AbortController,
  AbortSignal,
  Headers,
  Request,
  Response,
  TextDecoder,
  TextEncoder,
  URL,
  URLSearchParams,
  clearInterval,
  clearTimeout,
  console: safeConsole,
  fetch,
  queueMicrotask,
  setInterval,
  setTimeout,
  structuredClone,
});
const lambdaContext = createContext(lambdaGlobals, {
  name: 'dd-lambda-user-code',
  codeGeneration: { strings: false, wasm: false },
});

function hashBody(body) {
  return createHash('sha256').update(body).digest('hex');
}

function compileFunction(functionBody) {
  const cacheKey = hashBody(functionBody);
  const cached = compiledFunctions.get(cacheKey);
  if (cached) {
    return cached;
  }

  const fn = compileVmFunction(
    `"use strict"; return (async () => {\n${functionBody}\n})();`,
    ['request', 'context', 'console', 'process', 'require', 'Buffer'],
    { parsingContext: lambdaContext },
  );
  compiledFunctions.set(cacheKey, fn);
  while (compiledFunctions.size > maxCompiledFunctions) {
    const oldestKey = compiledFunctions.keys().next().value;
    compiledFunctions.delete(oldestKey);
  }
  return fn;
}

function browserAutomationEnabled(definition) {
  return (
    definition.containerized === true &&
    (definition.browserAutomation === true || definition.metaData?.browserAutomation === true)
  );
}

async function createBrowserSession() {
  const [playwright, puppeteerModule] = await Promise.all([
    import('playwright-core'),
    import('puppeteer-core'),
  ]);
  const puppeteer = puppeteerModule.default ?? puppeteerModule;
  const launched = new Set();
  const track = async (promise) => {
    const browser = await promise;
    launched.add(browser);
    return browser;
  };

  return {
    api: Object.freeze({
      engines: Object.freeze(['playwright', 'puppeteer']),
      executablePath: browserExecutablePath,
      launchPlaywright: () =>
        track(
          playwright.chromium.launch({
            args: [...browserLaunchArgs],
            executablePath: browserExecutablePath,
            headless: true,
          }),
        ),
      launchPuppeteer: () =>
        track(
          puppeteer.launch({
            args: [...browserLaunchArgs],
            executablePath: browserExecutablePath,
            headless: true,
          }),
        ),
    }),
    closeAll: async () => {
      const browsers = [...launched];
      launched.clear();
      await Promise.allSettled(browsers.map((browser) => browser.close()));
    },
  };
}

function assertSlug(slug) {
  const normalized = String(slug || '').trim().toLowerCase();
  if (!/^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$/.test(normalized)) {
    throw new Error('valid lambda slug is required');
  }
  return normalized;
}

function connectPayload(parsed) {
  const payload = {
    verbose: false,
    pedantic: false,
    lang: 'nodejs',
    name: 'dd-gleam-lambda-runner',
  };
  if (parsed.username && parsed.password) {
    payload.user = decodeURIComponent(parsed.username);
    payload.pass = decodeURIComponent(parsed.password);
  } else if (parsed.username) {
    payload.auth_token = decodeURIComponent(parsed.username);
  }
  return JSON.stringify(payload);
}

function parseNatsFrame(buffer) {
  let offset = 0;
  while (offset < buffer.length) {
    const lineEnd = buffer.indexOf('\r\n', offset, 'utf8');
    if (lineEnd < 0) {
      return { buffer: buffer.subarray(offset) };
    }
    const line = buffer.subarray(offset, lineEnd).toString('utf8');
    offset = lineEnd + 2;
    if (!line || line === '+OK' || line.startsWith('INFO') || line === 'PONG') {
      continue;
    }
    if (line === 'PING') {
      return { ping: true, buffer: buffer.subarray(offset) };
    }
    if (line.startsWith('-ERR')) {
      throw new Error(`NATS error: ${line}`);
    }
    if (line.startsWith('MSG ')) {
      const parts = line.split(' ');
      const byteCount = Number.parseInt(parts.at(-1) || '', 10);
      if (!Number.isFinite(byteCount) || byteCount < 0) {
        throw new Error(`invalid NATS MSG frame: ${line}`);
      }
      if (buffer.length < offset + byteCount + 2) {
        return { buffer: buffer.subarray(lineEnd - line.length) };
      }
      const payload = buffer.subarray(offset, offset + byteCount);
      return { payload, buffer: buffer.subarray(offset + byteCount + 2) };
    }
  }
  return { buffer: Buffer.alloc(0) };
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
    let buffer = Buffer.alloc(0);
    const socket = connectTcp({
      host: parsed.hostname,
      port: parsed.port ? Number(parsed.port) : 4222,
    });
    const finish = (error, value) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      if (error) {
        reject(error);
      } else {
        resolve(value);
      }
    };
    const timer = setTimeout(() => {
      finish(new Error(`container pool NATS request timed out after ${timeoutMs}ms`));
    }, Math.max(1_000, timeoutMs));

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
        buffer = Buffer.concat([buffer, chunk]);
        while (buffer.length > 0) {
          const frame = parseNatsFrame(buffer);
          buffer = frame.buffer;
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
    socket.on('timeout', () => {
      finish(new Error(`container pool NATS request timed out after ${timeoutMs}ms`));
    });
    socket.on('error', finish);
    socket.on('close', () => {
      if (!settled) {
        finish(new Error('container pool NATS connection closed before a reply was received'));
      }
    });
  });
}

async function dispatchContainerPool(pool, payload = {}, options = {}) {
  const poolSlug = assertSlug(pool);
  const subject =
    options.subject ||
    (containerPoolSubjectPrefix
      ? `${containerPoolSubjectPrefix}.${poolSlug}.requests`
      : containerPoolLanguageRequestsSubject(poolSlug));
  const request = {
    requestId: options.requestId || randomUUID(),
    poolSlug,
    payload,
    ...(options.path ? { path: options.path } : {}),
    ...(options.headers ? { headers: options.headers } : {}),
  };
  return await natsRequest(subject, request, positiveInt(options.timeoutMs, containerPoolNatsTimeoutMs));
}

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

async function invoke(line) {
  const envelope = JSON.parse(line);
  const definition = resolveDefinition(envelope);
  const functionBody = String(definition.functionBody || '');
  const request = envelope.request || {};
  const browserAutomation = browserAutomationEnabled(definition);
  const context = {
    id: definition.id,
    invocationId: envelope.invocationId,
    slug: definition.slug || envelope.slug,
    containerPool: Object.freeze({
      dispatch: dispatchContainerPool,
      request: dispatchContainerPool,
    }),
    capabilities: Object.freeze({
      browserAutomation,
      browserEngines: browserAutomation ? Object.freeze(['playwright', 'puppeteer']) : Object.freeze([]),
    }),
    meta: {
      runtime: definition.runtime,
      labels: definition.labels,
      metaData: definition.metaData,
      ...(envelope.meta || {}),
    },
  };

  if (!functionBody.trim()) {
    throw new Error('functionBody is required');
  }
  if (Buffer.byteLength(functionBody, 'utf8') > maxFunctionBodyBytes) {
    throw new Error('functionBody exceeds configured byte limit');
  }

  const fn = compileFunction(functionBody);
  if (envelope.checkOnly === true || envelope.mode === 'check') {
    return {
      ok: true,
      check: {
        runtime: definition.runtime,
        slug: definition.slug || envelope.slug,
        browserAutomation,
        browserEngines: browserAutomation ? ['playwright', 'puppeteer'] : [],
      },
      cachedFunctions: compiledFunctions.size,
    };
  }

  const browserSession = browserAutomation ? await createBrowserSession() : null;
  context.browser = browserSession?.api;
  try {
    const result = await fn(request, context, safeConsole, undefined, undefined, undefined);
    return {
      ok: true,
      result: result ?? null,
      invocationId: context.invocationId,
      cachedFunctions: compiledFunctions.size,
    };
  } finally {
    await browserSession?.closeAll();
  }
}

async function handleLine(line) {
  try {
    const result = await invoke(line);
    writeResult(result);
  } catch (error) {
    writeResult({
        ok: false,
        error: error instanceof Error ? error.message : String(error),
    });
  }
}

function writeResult(result) {
  let encoded = JSON.stringify(result);
  if (Buffer.byteLength(encoded, 'utf8') > maxResultBytes) {
    encoded = JSON.stringify({
      ok: false,
      error: 'lambda result exceeds configured byte limit',
    });
  }
  stdout.write(`${encoded}\n`);
}

stdin.setEncoding('utf8');
stdin.on('data', (chunk) => {
  buffer += chunk;
  if (Buffer.byteLength(buffer, 'utf8') > maxInputLineBytes) {
    buffer = '';
    writeResult({
      ok: false,
      error: 'lambda input exceeds configured byte limit',
    });
    return;
  }
  let newlineIndex = buffer.indexOf('\n');
  while (newlineIndex >= 0) {
    const line = buffer.slice(0, newlineIndex).trim();
    buffer = buffer.slice(newlineIndex + 1);
    if (line) {
      void handleLine(line);
    }
    newlineIndex = buffer.indexOf('\n');
  }
});
