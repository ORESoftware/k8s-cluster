import { createHash } from 'node:crypto';
import { Buffer } from 'node:buffer';
import { env, stdin, stderr, stdout } from 'node:process';

const maxCompiledFunctions = positiveInt(env.LAMBDA_FUNCTION_CACHE_MAX, 128);
const maxFunctionBodyBytes = positiveInt(env.LAMBDA_FUNCTION_BODY_MAX_BYTES, 262_144);
const maxInputLineBytes = positiveInt(env.LAMBDA_CHILD_INPUT_MAX_BYTES, 6_291_456);
const maxResultBytes = positiveInt(env.LAMBDA_RESULT_MAX_BYTES, 1_048_576);

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
Object.defineProperty(globalThis, 'process', {
  configurable: false,
  enumerable: false,
  value: undefined,
  writable: false,
});
Object.defineProperty(globalThis, 'Buffer', {
  configurable: false,
  enumerable: false,
  value: undefined,
  writable: false,
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

  const fn = new Function(
    'request',
    'context',
    'console',
    'process',
    'require',
    'Buffer',
    `"use strict"; return (async () => {\n${functionBody}\n})();`,
  );
  compiledFunctions.set(cacheKey, fn);
  while (compiledFunctions.size > maxCompiledFunctions) {
    const oldestKey = compiledFunctions.keys().next().value;
    compiledFunctions.delete(oldestKey);
  }
  return fn;
}

function assertSlug(slug) {
  const normalized = String(slug || '').trim().toLowerCase();
  if (!/^[a-z0-9][a-z0-9-]{1,118}[a-z0-9]$/.test(normalized)) {
    throw new Error('valid lambda slug is required');
  }
  return normalized;
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
  const context = {
    id: definition.id,
    invocationId: envelope.invocationId,
    slug: definition.slug || envelope.slug,
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
  const result = await fn(request, context, safeConsole, undefined, undefined, undefined);
  return {
    ok: true,
    result: result ?? null,
    invocationId: context.invocationId,
    cachedFunctions: compiledFunctions.size,
  };
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
