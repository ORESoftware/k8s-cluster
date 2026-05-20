import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/gleam-lambda-runner/gleam.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const runnerCwd = resolve(repoRoot, 'remote/deployments/gleam-lambda-runner');

type RuntimeCase = {
  name: string;
  command: string;
  args: string[];
  runtime: string;
  validBody: string;
  invalidBody: string;
};

function runChildCheck(runtimeCase: RuntimeCase, functionBody: string): Promise<{
  status: number | null;
  stdout: string;
  stderr: string;
  body: { ok?: boolean; error?: string; check?: Record<string, unknown> };
}> {
  const payload = {
    slug: `check-${runtimeCase.runtime}`,
    definition: {
      slug: `check-${runtimeCase.runtime}`,
      runtime: runtimeCase.runtime,
      functionBody,
    },
    request: {},
    checkOnly: true,
  };

  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(runtimeCase.command, runtimeCase.args, {
      cwd: runnerCwd,
      env: {
        ...process.env,
        NODE_NO_WARNINGS: '1',
      },
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      rejectRun(new Error(`${runtimeCase.name} check timed out\nstdout:\n${stdout}\nstderr:\n${stderr}`));
    }, 10_000);

    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', (error) => {
      clearTimeout(timer);
      rejectRun(error);
    });
    child.on('close', (status) => {
      clearTimeout(timer);
      const line = stdout.trim().split('\n').filter(Boolean).at(-1) || '{}';
      let body: { ok?: boolean; error?: string; check?: Record<string, unknown> };
      try {
        body = JSON.parse(line);
      } catch (error) {
        rejectRun(
          new Error(
            `${runtimeCase.name} emitted non-JSON check output: ${error}\nstdout:\n${stdout}\nstderr:\n${stderr}`,
          ),
        );
        return;
      }
      resolveRun({ status, stdout, stderr, body });
    });

    child.stdin.end(`${JSON.stringify(payload)}\n`);
  });
}

const runtimeCases: RuntimeCase[] = [
  {
    name: 'Node.js',
    command: 'node',
    args: ['child-runtimes/js-function-runner.mjs'],
    runtime: 'nodejs',
    validBody: 'return { status: 200, body: { ok: true } };',
    invalidBody: 'return {',
  },
  {
    name: 'Python',
    command: 'python3',
    args: ['child-runtimes/python-function-runner.py'],
    runtime: 'python3',
    validBody: 'result = { "status": 200, "body": { "ok": True } }',
    invalidBody: 'if :\n  result = 1',
  },
  {
    name: 'Ruby',
    command: 'ruby',
    args: ['child-runtimes/ruby-function-runner.rb'],
    runtime: 'ruby',
    validBody: '{ status: 200, body: { ok: true } }',
    invalidBody: 'def broken(',
  },
  {
    name: 'Bash',
    command: 'node',
    args: ['child-runtimes/bash-function-runner.mjs'],
    runtime: 'bash',
    validBody: 'printf \'%s\\n\' \'{"status":200,"body":{"ok":true}}\'',
    invalidBody: 'if true; then echo ok',
  },
];

test('lambda child runtimes support checkOnly compile checks', { timeout: 45_000 }, async () => {
  for (const runtimeCase of runtimeCases) {
    const result = await runChildCheck(runtimeCase, runtimeCase.validBody);
    assert.equal(result.status, 0, `${runtimeCase.name} process should exit cleanly`);
    assert.equal(result.body.ok, true, `${runtimeCase.name} should accept valid source`);
    assert.equal(
      result.body.check?.runtime,
      runtimeCase.runtime,
      `${runtimeCase.name} should report the checked runtime`,
    );
  }
});

test('lambda child runtimes reject invalid source during checkOnly', { timeout: 45_000 }, async () => {
  for (const runtimeCase of runtimeCases) {
    const result = await runChildCheck(runtimeCase, runtimeCase.invalidBody);
    assert.equal(result.status, 0, `${runtimeCase.name} process should return JSON errors`);
    assert.equal(result.body.ok, false, `${runtimeCase.name} should reject invalid source`);
    assert.equal(typeof result.body.error, 'string', `${runtimeCase.name} should include an error`);
    assert.notEqual(result.body.error?.trim(), '', `${runtimeCase.name} error should not be empty`);
  }
});
