#!/usr/bin/env node
import { Buffer } from 'node:buffer';
import { spawn } from 'node:child_process';
import { env, stdin, stderr, stdout } from 'node:process';

const maxFunctionBodyBytes = positiveInt(env.LAMBDA_FUNCTION_BODY_MAX_BYTES, 262_144);
const maxInputLineBytes = positiveInt(env.LAMBDA_CHILD_INPUT_MAX_BYTES, 6_291_456);
const maxResultBytes = positiveInt(env.LAMBDA_RESULT_MAX_BYTES, 1_048_576);

let buffer = '';

function positiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value || ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function resolveDefinition(envelope) {
  const definition = envelope.definition || (envelope.functionBody ? envelope : null);
  if (!definition || typeof definition !== 'object') {
    throw new Error('lambda definition with functionBody is required');
  }
  if (definition.status === 'paused' || definition.status === 'archived') {
    throw new Error(`lambda function is ${definition.status}`);
  }
  return definition;
}

function checkBash(functionBody) {
  return new Promise((resolve, reject) => {
    const child = spawn('/bin/bash', ['-n'], {
      env: {
        PATH: env.PATH || '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
      },
      stdio: ['pipe', 'ignore', 'pipe'],
    });

    let err = '';
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk) => {
      err += chunk;
    });
    child.on('error', reject);
    child.on('close', (status) => {
      if (status !== 0) {
        reject(new Error(`bash syntax check failed${err ? `: ${err.trim()}` : ''}`));
        return;
      }
      resolve();
    });
    child.stdin.end(functionBody);
  });
}

function runBash(functionBody, request, context) {
  return new Promise((resolve, reject) => {
    const child = spawn('/bin/bash', ['-s'], {
      env: {
        PATH: env.PATH || '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
        LAMBDA_REQUEST_JSON: JSON.stringify(request ?? {}),
        LAMBDA_CONTEXT_JSON: JSON.stringify(context ?? {}),
      },
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    let out = '';
    let err = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      out += chunk;
      if (Buffer.byteLength(out, 'utf8') > maxResultBytes) {
        child.kill('SIGKILL');
        reject(new Error('lambda result exceeds configured byte limit'));
      }
    });
    child.stderr.on('data', (chunk) => {
      err += chunk;
      stderr.write(`[lambda:bash] ${chunk}`);
    });
    child.on('error', reject);
    child.on('close', (status) => {
      if (status !== 0) {
        reject(new Error(`bash exited with status ${status}${err ? `: ${err.trim()}` : ''}`));
        return;
      }
      const text = out.trim();
      if (!text) {
        resolve(null);
        return;
      }
      try {
        resolve(JSON.parse(text));
      } catch {
        resolve(text);
      }
    });
    child.stdin.end(functionBody);
  });
}

async function invoke(line) {
  const envelope = JSON.parse(line);
  const definition = resolveDefinition(envelope);
  const functionBody = String(definition.functionBody || '');
  if (!functionBody.trim()) {
    throw new Error('functionBody is required');
  }
  if (Buffer.byteLength(functionBody, 'utf8') > maxFunctionBodyBytes) {
    throw new Error('functionBody exceeds configured byte limit');
  }
  if (envelope.checkOnly === true || envelope.mode === 'check') {
    await checkBash(functionBody);
    return {
      ok: true,
      check: {
        runtime: definition.runtime,
        slug: definition.slug || envelope.slug,
      },
    };
  }
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
  const result = await runBash(functionBody, request, context);
  return {
    ok: true,
    result,
    invocationId: context.invocationId,
  };
}

async function handleLine(line) {
  try {
    writeResult(await invoke(line));
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
