import { createHash, randomUUID } from 'node:crypto';
import { Buffer } from 'node:buffer';
import { connect as connectTcp } from 'node:net';
import { env, stdin, stderr, stdout } from 'node:process';
import { compileFunction as compileVmFunction, createContext } from 'node:vm';

// Source-of-truth NATS subject constants. The runner's working directory is
// the service root in dev, CI, the pod's clean projection, and runtime images,
// so this relative path resolves identically on every execution path.
import {
  containerPoolLanguageRequestsSubject,
} from '../../../../../libs/nats/subject-defs/generated/javascript/index.mjs';
import { createLambdaContext } from './lambda-context.mjs';

const maxCompiledFunctions = positiveInt(env.LAMBDA_FUNCTION_CACHE_MAX, 128);
const maxFunctionBodyBytes = positiveInt(env.LAMBDA_FUNCTION_BODY_MAX_BYTES, 262_144);
const maxInputLineBytes = positiveInt(env.LAMBDA_CHILD_INPUT_MAX_BYTES, 6_291_456);
const maxResultBytes = positiveInt(env.LAMBDA_RESULT_MAX_BYTES, 1_048_576);
const maxActorStateBytes = positiveInt(env.LAMBDA_ACTOR_STATE_MAX_BYTES, 524_288);
const maxStreamBytes = positiveInt(env.LAMBDA_STREAM_MAX_BYTES, 16_777_216);
const maxStreamChunkBytes = Math.max(
  1_024,
  Math.min(
    positiveInt(env.LAMBDA_STREAM_CHUNK_BYTES, 65_536),
    262_144,
  ),
);
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

// One sandbox (vm context + its own compiled-function cache) per function
// identity. Worker processes are pooled by runtime — `pool:host:nodejs` is
// shared by every function that does not set reuseKey/maxConcurrency — so a
// single module-level context would put unrelated functions in one mutable
// global object, where one function can leave a getter or Proxy behind and read
// the next function's request, context, and result. Keying the sandbox by
// function identity keeps that state inside one tenant.
const sandboxes = new Map();
const maxSandboxes = positiveInt(env.LAMBDA_SANDBOX_CACHE_MAX, 64);
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
//
// NOTE ON THE TRUST BOUNDARY: node:vm is not a security sandbox, and this one is
// not either. The bridged values below (fetch, Headers, Response, URL, …) are
// host-realm functions, so `Headers.constructor` is the host realm's Function
// constructor and reaches code generation that this context's
// `codeGeneration: { strings: false }` does not govern. The enforced isolation
// boundary for untrusted code is therefore the container
// (`containerized: true`), not this context. What the context does buy is
// keeping ordinary function code away from process/require and keeping one
// tenant's globals out of another's — see `sandboxes` above.
function createLambdaGlobals() {
  // A fresh object per context: sharing one globals object across contexts
  // would re-introduce the cross-function channel this split exists to close.
  return Object.assign(Object.create(null), {
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
}

// Stable per-tenant sandbox identity. Prefer the immutable revision, then the
// function id, then the slug; an unidentifiable definition gets its own
// single-use sandbox rather than sharing the fallback with every other one.
function sandboxIdentity(definition, envelope) {
  return (
    definition.revisionId ||
    definition.id ||
    definition.slug ||
    envelope.slug ||
    `anonymous:${randomUUID()}`
  );
}

function getSandbox(identity) {
  const existing = sandboxes.get(identity);
  if (existing) {
    // Refresh LRU position.
    sandboxes.delete(identity);
    sandboxes.set(identity, existing);
    return existing;
  }
  const sandbox = {
    context: createContext(createLambdaGlobals(), {
      name: `dd-lambda-user-code:${identity}`,
      codeGeneration: { strings: false, wasm: false },
    }),
    compiled: new Map(),
  };
  sandboxes.set(identity, sandbox);
  while (sandboxes.size > maxSandboxes) {
    const oldestKey = sandboxes.keys().next().value;
    sandboxes.delete(oldestKey);
  }
  return sandbox;
}

function hashBody(body) {
  return createHash('sha256').update(body).digest('hex');
}

function countCompiledFunctions() {
  let total = 0;
  for (const sandbox of sandboxes.values()) {
    total += sandbox.compiled.size;
  }
  return total;
}

function compileFunction(sandbox, functionBody) {
  const cacheKey = hashBody(functionBody);
  const cached = sandbox.compiled.get(cacheKey);
  if (cached) {
    return cached;
  }

  const fn = compileVmFunction(
    `"use strict"; return (async () => {\n${functionBody}\n})();`,
    ['request', 'context', 'console', 'process', 'require', 'Buffer'],
    { parsingContext: sandbox.context },
  );
  sandbox.compiled.set(cacheKey, fn);
  while (sandbox.compiled.size > maxCompiledFunctions) {
    const oldestKey = sandbox.compiled.keys().next().value;
    sandbox.compiled.delete(oldestKey);
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

function actorStorageKey(value) {
  const key = String(value ?? '');
  const bytes = Buffer.byteLength(key, 'utf8');
  if (bytes < 1 || bytes > 512) {
    throw new Error('actor storage key must be between 1 and 512 bytes');
  }
  return key;
}

function jsonActorValue(value, label = 'actor storage value') {
  if (value === undefined) {
    throw new Error(`${label} must be JSON serializable`);
  }
  let encoded;
  try {
    encoded = JSON.stringify(value);
  } catch {
    throw new Error(`${label} must be JSON serializable`);
  }
  if (encoded === undefined) {
    throw new Error(`${label} must be JSON serializable`);
  }
  return JSON.parse(encoded);
}

function normalizedAlarmAt(value) {
  if (value === null || value === undefined) {
    return null;
  }
  const timestamp = value instanceof Date ? value.getTime() : new Date(value).getTime();
  if (!Number.isFinite(timestamp)) {
    throw new Error('actor alarm must be a valid Date, epoch value, or timestamp string');
  }
  return new Date(timestamp).toISOString();
}

function createActorSession(actor) {
  if (!actor || typeof actor !== 'object' || Array.isArray(actor)) {
    throw new Error('actor context is required for actor invocation');
  }
  const actorId = String(actor.id || '');
  const key = String(actor.key || '');
  if (!actorId || !key) {
    throw new Error('actor id and key are required');
  }
  const initialState =
    actor.state && typeof actor.state === 'object' && !Array.isArray(actor.state)
      ? jsonActorValue(actor.state, 'actor state')
      : {};
  const values = new Map(Object.entries(initialState));
  let alarmAt = normalizedAlarmAt(actor.alarmAt);

  const storage = Object.freeze({
    get: async (storageKey) => {
      const value = values.get(actorStorageKey(storageKey));
      return value === undefined ? undefined : structuredClone(value);
    },
    put: async (storageKey, value) => {
      values.set(actorStorageKey(storageKey), jsonActorValue(value));
    },
    delete: async (storageKey) => values.delete(actorStorageKey(storageKey)),
    list: async (options = {}) => {
      const prefix = String(options?.prefix || '');
      const limit = Math.max(1, Math.min(positiveInt(options?.limit, 1_000), 1_000));
      return Object.fromEntries(
        [...values.entries()]
          .filter(([entryKey]) => entryKey.startsWith(prefix))
          .sort(([left], [right]) => left.localeCompare(right))
          .slice(0, limit)
          .map(([entryKey, value]) => [entryKey, structuredClone(value)]),
      );
    },
  });
  const alarm = Object.freeze({
    get: async () => alarmAt,
    set: async (value) => {
      alarmAt = normalizedAlarmAt(value);
      return alarmAt;
    },
    delete: async () => {
      alarmAt = null;
    },
  });

  return {
    api: Object.freeze({
      id: actorId,
      key,
      version: Number.isSafeInteger(actor.version) ? actor.version : 0,
      storage,
      alarm,
    }),
    snapshot: () => {
      const state = Object.fromEntries(values);
      const encoded = JSON.stringify(state);
      if (Buffer.byteLength(encoded, 'utf8') > maxActorStateBytes) {
        throw new Error('actor state exceeds configured byte limit');
      }
      return { state, alarmAt };
    },
  };
}

async function invoke(line) {
  const envelope = JSON.parse(line);
  const definition = resolveDefinition(envelope);
  const functionBody = String(definition.functionBody || '');
  const request = envelope.request || {};
  const browserAutomation = browserAutomationEnabled(definition);
  const actorSession = envelope.mode === 'actor' ? createActorSession(envelope.actor) : null;
  const context = createLambdaContext({
  definition,
  envelope,
  browserAutomation,
  actorSession,
  dispatchContainerPool,
});
  if (actorSession) {
    context.actor = actorSession.api;
  }

  if (!functionBody.trim()) {
    throw new Error('functionBody is required');
  }
  if (Buffer.byteLength(functionBody, 'utf8') > maxFunctionBodyBytes) {
    throw new Error('functionBody exceeds configured byte limit');
  }

  const fn = compileFunction(getSandbox(sandboxIdentity(definition, envelope)), functionBody);
  if (envelope.checkOnly === true || envelope.mode === 'check') {
    return {
      ok: true,
      check: {
        runtime: definition.runtime,
        slug: definition.slug || envelope.slug,
        browserAutomation,
        browserEngines: browserAutomation ? ['playwright', 'puppeteer'] : [],
      },
      cachedFunctions: countCompiledFunctions(),
    };
  }

  const browserSession = browserAutomation ? await createBrowserSession() : null;
  context.browser = browserSession?.api;
  try {
    const result = await fn(request, context, safeConsole, undefined, undefined, undefined);
    if (envelope.mode === 'stream') {
      await writeStreamingResult(result ?? null);
      return { streamHandled: true };
    }
    if (actorSession) {
      return {
        ok: true,
        result: result ?? null,
        actor: actorSession.snapshot(),
        invocationId: context.invocationId,
        cachedFunctions: countCompiledFunctions(),
      };
    }
    return {
      ok: true,
      result: result ?? null,
      invocationId: context.invocationId,
      cachedFunctions: countCompiledFunctions(),
    };
  } finally {
    await browserSession?.closeAll();
  }
}

async function handleLine(line) {
  let streamMode = false;
  try {
    streamMode = JSON.parse(line)?.mode === 'stream';
    const result = await invoke(line);
    if (result?.streamHandled !== true) {
      writeResult(result);
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (streamMode) {
      try {
        await writeStreamFrame({
          stream: true,
          event: 'error',
          error: message.slice(0, 4096),
        });
      } catch (writeError) {
        stderr.write(
          `[lambda:error] failed to write stream error: ${
            writeError instanceof Error ? writeError.message : String(writeError)
          }\n`,
        );
      }
    } else {
      writeResult({ ok: false, error: message });
    }
  }
}

async function writeStreamingResult(result) {
  const response = result instanceof Response ? result : null;
  await writeStreamFrame({
    stream: true,
    event: 'start',
    contentType: response?.headers.get('content-type') || 'application/octet-stream',
    status: response?.status || 200,
  });

  let totalBytes = 0;
  for await (const value of streamValues(result)) {
    const bytes = encodeStreamValue(value);
    for (let offset = 0; offset < bytes.length; offset += maxStreamChunkBytes) {
      const chunk = bytes.subarray(offset, offset + maxStreamChunkBytes);
      if (totalBytes + chunk.length > maxStreamBytes) {
        throw new Error('lambda stream exceeds configured byte limit');
      }
      totalBytes += chunk.length;
      await writeStreamFrame({
        stream: true,
        event: 'chunk',
        encoding: 'base64',
        data: chunk.toString('base64'),
      });
    }
  }

  await writeStreamFrame({
    stream: true,
    event: 'end',
    bytes: totalBytes,
  });
}

async function* streamValues(result) {
  if (result instanceof Response) {
    if (result.body) {
      for await (const chunk of result.body) {
        yield chunk;
      }
    }
    return;
  }

  if (
    result &&
    typeof result !== 'string' &&
    typeof result[Symbol.asyncIterator] === 'function'
  ) {
    for await (const chunk of result) {
      yield chunk;
    }
    return;
  }

  yield result;
}

function encodeStreamValue(value) {
  if (typeof value === 'string') {
    return Buffer.from(value, 'utf8');
  }
  if (value instanceof ArrayBuffer) {
    return Buffer.from(value);
  }
  if (ArrayBuffer.isView(value)) {
    return Buffer.from(value.buffer, value.byteOffset, value.byteLength);
  }
  const json = JSON.stringify(value ?? null);
  return Buffer.from(`${json}\n`, 'utf8');
}

async function writeStreamFrame(frame) {
  const encoded = `${JSON.stringify(frame)}\n`;
  if (stdout.write(encoded)) {
    return;
  }
  await new Promise((resolve, reject) => {
    const onDrain = () => {
      cleanup();
      resolve();
    };
    const onError = (error) => {
      cleanup();
      reject(error);
    };
    const cleanup = () => {
      stdout.off('drain', onDrain);
      stdout.off('error', onError);
    };
    stdout.once('drain', onDrain);
    stdout.once('error', onError);
  });
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
