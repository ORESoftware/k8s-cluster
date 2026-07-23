#!/usr/bin/env node
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { env } from 'node:process';

const docker = env.SCINTILLA_CONTAINER_CLI || 'docker';
const imagePrefix = env.SCINTILLA_RUNTIME_IMAGE_PREFIX || 'scintilla-runtime-';
const imageTag = env.SCINTILLA_RUNTIME_IMAGE_TAG || 'e2e';
const timeoutMs = Number.parseInt(env.SCINTILLA_RUNTIME_E2E_TIMEOUT_MS || '120000', 10);
const pullPolicy = env.SCINTILLA_RUNTIME_E2E_PULL || 'missing';

const cases = {
  nodejs: 'return request;',
  golang: `package main
func Handler(request map[string]any, _context map[string]any) (any, error) {
  return request, nil
}`,
  java: `public final class Handler {
  public static String handle(String requestJson, String contextJson) {
    return requestJson;
  }
}`,
  erlang: `-module(handler).
-export([handle/2]).
handle(Request, _Context) -> Request.`,
  elixir: `defmodule Handler do
  def handle(request, _context), do: request
end`,
  gleam: `pub fn handle(request_json: String, _context_json: String) -> String {
  request_json
}`,
  rust: `pub fn handle(request_json: &str, _context_json: &str) -> String {
  request_json.to_string()
}`,
};

function imageFor(runtime) {
  const key = `SCINTILLA_RUNTIME_${runtime.toUpperCase()}_IMAGE`;
  return env[key] || `${imagePrefix}${runtime}:${imageTag}`;
}

function invoke(runtime, functionBody) {
  const image = imageFor(runtime);
  const envelope = JSON.stringify({
    definition: {
      id: '00000000-0000-0000-0000-000000000001',
      slug: `${runtime}-docker-e2e`,
      runtime,
      functionBody,
      maxRunMs: timeoutMs - 5_000,
      status: 'active',
    },
    invocationId: `${runtime}-docker-e2e`,
    request: { value: 42 },
  });

  return new Promise((resolve, reject) => {
    const child = spawn(docker, [
      'run', '--rm', '-i', `--pull=${pullPolicy}`, '--read-only',
      '--network', 'none',
      '--user', '10001:10001',
      '--cap-drop', 'ALL',
      '--security-opt', 'no-new-privileges',
      '--pids-limit', '128',
      '--memory', '1g',
      '--cpus', '2',
      '--tmpfs', '/tmp:rw,noexec,nosuid,size=16m,mode=1777',
      '--tmpfs', '/work:rw,exec,nosuid,nodev,size=1g,mode=1777',
      image,
    ], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    let settled = false;
    const finish = (error, result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill('SIGTERM');
      if (error) reject(error);
      else resolve(result);
    };
    const timer = setTimeout(() => {
      finish(new Error(`${runtime} image timed out after ${timeoutMs}ms: ${stderr}`));
    }, timeoutMs);

    child.once('error', finish);
    child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
    child.stdout.setEncoding('utf8').on('data', (chunk) => {
      stdout += chunk;
      const newline = stdout.indexOf('\n');
      if (newline < 0) return;
      try {
        finish(null, JSON.parse(stdout.slice(0, newline)));
      } catch (error) {
        finish(new Error(`${runtime} returned invalid JSON: ${stdout}\n${stderr}`, { cause: error }));
      }
    });
    child.once('close', (status) => {
      if (!settled) {
        finish(new Error(`${runtime} image exited with ${status}: ${stdout}\n${stderr}`));
      }
    });
    child.stdin.end(`${envelope}\n`);
  });
}

for (const [runtime, functionBody] of Object.entries(cases)) {
  const result = await invoke(runtime, functionBody);
  assert.equal(result.ok, true, `${runtime}: ${result.error || 'invocation failed'}`);
  assert.deepEqual(result.result, { value: 42 }, `${runtime}: unexpected result`);
  process.stdout.write(`ok ${runtime} ${imageFor(runtime)}\n`);
}
