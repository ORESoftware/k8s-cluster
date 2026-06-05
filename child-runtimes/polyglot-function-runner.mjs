#!/usr/bin/env node
import { Buffer } from 'node:buffer';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { env, stdin, stderr, stdout } from 'node:process';

const maxFunctionBodyBytes = positiveInt(env.LAMBDA_FUNCTION_BODY_MAX_BYTES, 262_144);
const maxInputLineBytes = positiveInt(env.LAMBDA_CHILD_INPUT_MAX_BYTES, 6_291_456);
const maxResultBytes = positiveInt(env.LAMBDA_RESULT_MAX_BYTES, 1_048_576);
const defaultTimeoutMs = positiveInt(env.LAMBDA_POLYGLOT_CHILD_TIMEOUT_MS, 30_000);
const targetRuntime = normalizeRuntime(env.LAMBDA_TARGET_RUNTIME || '');

let buffer = '';

function positiveInt(value, fallback) {
  const parsed = Number.parseInt(String(value || ''), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function normalizeRuntime(value) {
  const runtime = String(value || '').trim().toLowerCase();
  if (runtime === 'go') return 'golang';
  if (runtime === 'erl') return 'erlang';
  if (runtime === 'ex') return 'elixir';
  if (runtime === 'jvm') return 'java';
  return runtime;
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

function resultFromStdout(text) {
  const trimmed = String(text || '').trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return trimmed;
  }
}

function asJson(value) {
  return JSON.stringify(value ?? {});
}

function runnerTimeoutMs(definition) {
  return positiveInt(definition.maxRunMs, defaultTimeoutMs);
}

function run(command, args, options = {}) {
  const timeoutMs = Math.max(1_000, positiveInt(options.timeoutMs, defaultTimeoutMs));
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: {
        PATH: env.PATH || '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
        HOME: env.HOME || '/tmp',
        ...(options.env || {}),
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let out = '';
    let err = '';
    let settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolve(value);
    };
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      finish(new Error(`${command} timed out after ${timeoutMs}ms`));
    }, timeoutMs);

    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      out += chunk;
      if (Buffer.byteLength(out, 'utf8') > maxResultBytes) {
        child.kill('SIGKILL');
        finish(new Error('lambda result exceeds configured byte limit'));
      }
    });
    child.stderr.on('data', (chunk) => {
      err += chunk;
      stderr.write(`[lambda:${targetRuntime || 'polyglot'}] ${chunk}`);
    });
    child.on('error', finish);
    child.on('close', (status) => {
      if (settled) return;
      if (status !== 0) {
        finish(new Error(`${command} exited with status ${status}${err ? `: ${err.trim()}` : ''}`));
        return;
      }
      finish(null, out);
    });
  });
}

async function workDir() {
  const dir = join(tmpdir(), `dd-lambda-${targetRuntime || 'polyglot'}-${randomUUID()}`);
  await mkdir(dir, { recursive: true, mode: 0o700 });
  return dir;
}

async function runGolang(definition, request, context, checkOnly) {
  const dir = await workDir();
  await writeFile(join(dir, 'handler.go'), definition.functionBody, { mode: 0o600 });
  await writeFile(
    join(dir, 'runner.go'),
    `package main

import (
  "encoding/json"
  "fmt"
  "os"
)

func main() {
  var request map[string]any
  var context map[string]any
  if err := json.Unmarshal([]byte(os.Getenv("DD_LAMBDA_REQUEST_JSON")), &request); err != nil {
    panic(err)
  }
  if err := json.Unmarshal([]byte(os.Getenv("DD_LAMBDA_CONTEXT_JSON")), &context); err != nil {
    panic(err)
  }
  result, err := Handler(request, context)
  if err != nil {
    panic(err)
  }
  encoded, err := json.Marshal(result)
  if err != nil {
    panic(err)
  }
  fmt.Print(string(encoded))
}
`,
    { mode: 0o600 },
  );
  const timeoutMs = runnerTimeoutMs(definition);
  await run('go', ['test'], { cwd: dir, timeoutMs, env: { GO111MODULE: 'off', GOCACHE: join(dir, 'gocache') } });
  if (checkOnly) return { mode: 'compile' };
  const out = await run('go', ['run', '.'], {
    cwd: dir,
    timeoutMs,
    env: {
      DD_LAMBDA_REQUEST_JSON: asJson(request),
      DD_LAMBDA_CONTEXT_JSON: asJson(context),
      GO111MODULE: 'off',
      GOCACHE: join(dir, 'gocache'),
    },
  });
  return { result: resultFromStdout(out) };
}

async function runDart(definition, request, context, checkOnly) {
  const dir = await workDir();
  await writeFile(join(dir, 'handler.dart'), definition.functionBody, { mode: 0o600 });
  await writeFile(
    join(dir, 'runner.dart'),
    `import 'dart:convert';
import 'dart:io';
import 'handler.dart' as user;

void main() {
  final request = jsonDecode(Platform.environment['DD_LAMBDA_REQUEST_JSON'] ?? '{}') as Map<String, dynamic>;
  final context = jsonDecode(Platform.environment['DD_LAMBDA_CONTEXT_JSON'] ?? '{}') as Map<String, dynamic>;
  final result = user.handler(request, context);
  stdout.write(jsonEncode(result));
}
`,
    { mode: 0o600 },
  );
  const timeoutMs = runnerTimeoutMs(definition);
  await run('dart', ['compile', 'exe', 'runner.dart', '-o', 'runner'], { cwd: dir, timeoutMs });
  if (checkOnly) return { mode: 'compile' };
  const out = await run(join(dir, 'runner'), [], {
    cwd: dir,
    timeoutMs,
    env: {
      DD_LAMBDA_REQUEST_JSON: asJson(request),
      DD_LAMBDA_CONTEXT_JSON: asJson(context),
    },
  });
  return { result: resultFromStdout(out) };
}

async function runJava(definition, request, context, checkOnly) {
  const dir = await workDir();
  await writeFile(join(dir, 'Handler.java'), definition.functionBody, { mode: 0o600 });
  await writeFile(
    join(dir, 'Runner.java'),
    `import java.nio.charset.StandardCharsets;
import java.util.Base64;

public final class Runner {
  public static void main(String[] args) throws Exception {
    String requestJson = new String(Base64.getDecoder().decode(System.getenv("DD_LAMBDA_REQUEST_B64")), StandardCharsets.UTF_8);
    String contextJson = new String(Base64.getDecoder().decode(System.getenv("DD_LAMBDA_CONTEXT_B64")), StandardCharsets.UTF_8);
    String result = Handler.handle(requestJson, contextJson);
    System.out.print(result == null ? "null" : result);
  }
}
`,
    { mode: 0o600 },
  );
  const timeoutMs = runnerTimeoutMs(definition);
  await run('javac', ['Handler.java', 'Runner.java'], { cwd: dir, timeoutMs });
  if (checkOnly) return { mode: 'compile' };
  const out = await run('java', ['-cp', dir, 'Runner'], {
    cwd: dir,
    timeoutMs,
    env: {
      DD_LAMBDA_REQUEST_B64: Buffer.from(asJson(request)).toString('base64'),
      DD_LAMBDA_CONTEXT_B64: Buffer.from(asJson(context)).toString('base64'),
    },
  });
  return { result: resultFromStdout(out) };
}

async function runErlang(definition, request, context, checkOnly) {
  const dir = await workDir();
  await writeFile(join(dir, 'handler.erl'), definition.functionBody, { mode: 0o600 });
  const timeoutMs = runnerTimeoutMs(definition);
  await run('erlc', ['handler.erl'], { cwd: dir, timeoutMs });
  if (checkOnly) return { mode: 'compile' };
  const out = await run(
    'erl',
    [
      '-noshell',
      '-pa',
      dir,
      '-eval',
      'Request = base64:decode(os:getenv("DD_LAMBDA_REQUEST_B64")), Context = base64:decode(os:getenv("DD_LAMBDA_CONTEXT_B64")), Result = handler:handle(Request, Context), io:format("~s", [Result]), halt(0).',
    ],
    {
      cwd: dir,
      timeoutMs,
      env: {
        DD_LAMBDA_REQUEST_B64: Buffer.from(asJson(request)).toString('base64'),
        DD_LAMBDA_CONTEXT_B64: Buffer.from(asJson(context)).toString('base64'),
      },
    },
  );
  return { result: resultFromStdout(out) };
}

async function runElixir(definition, request, context, checkOnly) {
  const dir = await workDir();
  const ebin = join(dir, 'ebin');
  await mkdir(ebin, { recursive: true, mode: 0o700 });
  await writeFile(join(dir, 'handler.ex'), definition.functionBody, { mode: 0o600 });
  const timeoutMs = runnerTimeoutMs(definition);
  await run('elixirc', ['handler.ex', '-o', ebin], { cwd: dir, timeoutMs });
  if (checkOnly) return { mode: 'compile' };
  const out = await run(
    'elixir',
    [
      '-pa',
      ebin,
      '-e',
      'request = System.get_env("DD_LAMBDA_REQUEST_B64") |> Base.decode64!(); context = System.get_env("DD_LAMBDA_CONTEXT_B64") |> Base.decode64!(); IO.write(Handler.handle(request, context))',
    ],
    {
      cwd: dir,
      timeoutMs,
      env: {
        DD_LAMBDA_REQUEST_B64: Buffer.from(asJson(request)).toString('base64'),
        DD_LAMBDA_CONTEXT_B64: Buffer.from(asJson(context)).toString('base64'),
      },
    },
  );
  return { result: resultFromStdout(out) };
}

async function invoke(line) {
  const envelope = JSON.parse(line);
  const definition = resolveDefinition(envelope);
  const runtime = normalizeRuntime(definition.runtime || targetRuntime);
  const functionBody = String(definition.functionBody || '');
  if (!functionBody.trim()) {
    throw new Error('functionBody is required');
  }
  if (Buffer.byteLength(functionBody, 'utf8') > maxFunctionBodyBytes) {
    throw new Error('functionBody exceeds configured byte limit');
  }
  if (runtime !== targetRuntime) {
    throw new Error(`runtime/image mismatch: ${runtime} definition cannot run in ${targetRuntime || 'unknown'} container`);
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
  const checkOnly = envelope.checkOnly === true || envelope.mode === 'check';
  const runResult = await {
    golang: runGolang,
    dart: runDart,
    java: runJava,
    erlang: runErlang,
    elixir: runElixir,
  }[runtime]?.(definition, request, context, checkOnly);
  if (!runResult) {
    throw new Error(`unsupported polyglot runtime: ${runtime}`);
  }
  if (checkOnly) {
    return {
      ok: true,
      check: {
        runtime: definition.runtime,
        slug: definition.slug || envelope.slug,
        mode: runResult.mode || 'compile',
      },
    };
  }
  return {
    ok: true,
    result: runResult.result ?? null,
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
