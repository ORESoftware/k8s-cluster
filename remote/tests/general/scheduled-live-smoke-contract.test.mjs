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
const browserMcpPublicWorkflow = readFileSync(
  resolve(root, '.github/workflows/browser-mcp-public-e2e.yml'),
  'utf8',
);

test('Browser MCP schedule delegates to the protected verifier', () => {
  for (const marker of [
    'schedule:',
    'actions: write',
    'TARGET_WORKFLOW: browser-mcp-public-e2e.yml',
    'actions/workflows/${TARGET_WORKFLOW}/dispatches',
    'event=workflow_dispatch&branch=main',
    '.id > $previous_run_id',
    '.head_sha == $main_sha',
    'gh run watch "$RUN_ID"',
    '-f ref=main',
  ]) {
    assert.ok(
      browserMcpWorkflow.includes(marker),
      `missing Browser MCP caller marker: ${marker}`,
    );
  }

  assert.match(browserMcpWorkflow, /if: github\.event_name != 'pull_request'/);
  for (const duplicatedImplementation of [
    'aws ssm send-command',
    'aws secretsmanager get-secret-value',
    'BROWSER_MCP_OAUTH_OPERATOR_SECRET',
    'BROWSER_MCP_SECRET_ID',
  ]) {
    assert.ok(
      !browserMcpWorkflow.includes(duplicatedImplementation),
      `protected implementation leaked into schedule caller: ${duplicatedImplementation}`,
    );
  }

  for (const marker of [
    'id-token: write',
    'aws ssm send-command',
    'aws secretsmanager get-secret-value',
    'BROWSER_MCP_OAUTH_OPERATOR_SECRET',
    'https://98.90.186.114/browser-mcp',
    'https://hello.95-217-171-250.sslip.io/browser-mcp',
  ]) {
    assert.ok(
      browserMcpPublicWorkflow.includes(marker),
      `missing protected Browser MCP verifier marker: ${marker}`,
    );
  }

  assert.doesNotMatch(
    browserMcpPublicWorkflow,
    /secrets\.BROWSER_MCP_OAUTH_OPERATOR_SECRET/,
  );
  assert.doesNotMatch(browserMcpPublicWorkflow, /echo[^\n]*operator_secret/i);
  assert.doesNotMatch(browserMcpPublicWorkflow, /set -x/);
});

test('AthletO schedule executes only AthletO browser tests', () => {
  for (const file of [
    'ui/athleto-app.playwright.test.mjs',
    'ui/athleto-marketing.playwright.test.mjs',
    'ui/athleto-marketing.puppeteer.test.mjs',
  ]) {
    assert.ok(
      athletoWorkflow.includes(file),
      `missing AthletO test file: ${file}`,
    );
  }

  for (const unrelated of [
    'ui/*.test.mjs',
    'pnpm run test:ui:athleto',
    'gha-continuity.playwright.test.mjs',
    'slack-agent-command.playwright.test.mjs',
    'slack-agent-command-security.playwright.test.mjs',
  ]) {
    assert.ok(
      !athletoWorkflow.includes(unrelated),
      `unrelated UI scope leaked into AthletO: ${unrelated}`,
    );
  }
});

test('AthletO live endpoints fail fast before browser installation', () => {
  const preflight = athletoWorkflow.indexOf('Preflight live AthletO surfaces');
  const playwrightInstall = athletoWorkflow.indexOf(
    'Install Playwright chromium',
  );

  assert.ok(preflight >= 0, 'missing AthletO endpoint preflight');
  assert.ok(
    playwrightInstall > preflight,
    'endpoint preflight must run before browser installation',
  );

  for (const marker of [
    '--connect-timeout 10',
    '--max-time 30',
    '--retry 2',
    '--retry-all-errors',
    'ATHLETO_MARKETING_URL',
    'ATHLETO_APP_URL',
  ]) {
    assert.ok(
      athletoWorkflow.includes(marker),
      `missing bounded preflight marker: ${marker}`,
    );
  }
});
