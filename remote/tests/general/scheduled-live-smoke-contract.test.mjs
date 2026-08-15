import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function repoRoot() {
  for (const candidate of [
    process.cwd(),
    resolve(process.cwd(), '..'),
    resolve(process.cwd(), '..', '..'),
  ]) {
    if (existsSync(resolve(candidate, '.github/workflows/athleto-ui-tests.yml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const athletoWorkflow = readFileSync(
  resolve(root, '.github/workflows/athleto-ui-tests.yml'),
  'utf8',
);
const browserMcpWorkflow = readFileSync(
  resolve(root, '.github/workflows/browser-mcp-external-smoke.yml'),
  'utf8',
);

test('Browser MCP schedule keeps the operator secret inside AWS', () => {
  for (const marker of [
    'schedule:',
    'id-token: write',
    'aws-actions/configure-aws-credentials@',
    'aws ssm send-command',
    'aws secretsmanager get-secret-value',
    'dd/remote-dev/browser-mcp-secrets',
    'BROWSER_MCP_OAUTH_OPERATOR_SECRET',
    'https://98.90.186.114/browser-mcp',
    'https://hello.95-217-171-250.sslip.io/browser-mcp',
  ]) {
    assert.ok(browserMcpWorkflow.includes(marker), `missing Browser MCP marker: ${marker}`);
  }

  assert.match(browserMcpWorkflow, /if: github\.event_name != 'pull_request'/);
  assert.doesNotMatch(browserMcpWorkflow, /secrets\.BROWSER_MCP_OAUTH_OPERATOR_SECRET/);
  assert.doesNotMatch(browserMcpWorkflow, /echo[^\n]*operator_secret/i);
  assert.doesNotMatch(browserMcpWorkflow, /set -x/);
});

test('AthletO schedule executes only AthletO browser tests', () => {
  for (const file of [
    'ui/athleto-app.playwright.test.mjs',
    'ui/athleto-marketing.playwright.test.mjs',
    'ui/athleto-marketing.puppeteer.test.mjs',
  ]) {
    assert.ok(athletoWorkflow.includes(file), `missing AthletO test file: ${file}`);
  }

  for (const unrelated of [
    'ui/*.test.mjs',
    'pnpm run test:ui:athleto',
    'gha-continuity.playwright.test.mjs',
    'slack-agent-command.playwright.test.mjs',
    'slack-agent-command-security.playwright.test.mjs',
  ]) {
    assert.ok(!athletoWorkflow.includes(unrelated), `unrelated UI scope leaked into AthletO: ${unrelated}`);
  }
});

test('AthletO live endpoints fail fast before browser installation', () => {
  const preflight = athletoWorkflow.indexOf('Preflight live AthletO surfaces');
  const playwrightInstall = athletoWorkflow.indexOf('Install Playwright chromium');

  assert.ok(preflight >= 0, 'missing AthletO endpoint preflight');
  assert.ok(playwrightInstall > preflight, 'endpoint preflight must run before browser installation');

  for (const marker of [
    '--connect-timeout 10',
    '--max-time 30',
    '--retry 2',
    '--retry-all-errors',
    'ATHLETO_MARKETING_URL',
    'ATHLETO_APP_URL',
  ]) {
    assert.ok(athletoWorkflow.includes(marker), `missing bounded preflight marker: ${marker}`);
  }
});
