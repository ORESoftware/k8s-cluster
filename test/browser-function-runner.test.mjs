import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { test } from 'node:test';

function checkRuntime(runtime, browserEngine) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['child-runtimes/browser-function-runner.mjs'], {
      cwd: process.cwd(),
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      stdout += chunk;
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code !== 0) {
        reject(new Error(`browser runner exited ${code}: ${stderr}`));
        return;
      }
      resolve(JSON.parse(stdout.trim().split('\n').at(-1)));
    });
    child.stdin.end(
      `${JSON.stringify({
        slug: `check-${runtime}`,
        definition: {
          slug: `check-${runtime}`,
          runtime,
          functionBody: 'return { ok: true };',
        },
        browserEngine,
        request: {},
        checkOnly: true,
      })}\n`,
    );
  });
}

test('Playwright and Puppeteer definitions compile as first-class runtimes', async () => {
  const playwright = await checkRuntime('playwright');
  const puppeteer = await checkRuntime('puppeteer');
  assert.equal(playwright.ok, true);
  assert.equal(playwright.check.engine, 'playwright');
  assert.equal(puppeteer.ok, true);
  assert.equal(puppeteer.check.engine, 'puppeteer');
});

test('an explicit browser engine selects Puppeteer', async () => {
  const result = await checkRuntime('browser', 'puppeteer');
  assert.equal(result.ok, true);
  assert.equal(result.check.engine, 'puppeteer');
});
