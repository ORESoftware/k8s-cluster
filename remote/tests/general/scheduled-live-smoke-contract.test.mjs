import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

import {
  allowedLiveOrigins,
  normalizeLiveTarget,
} from '../ui/lib/live-targets.mjs';

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
const athletoHarness = readFileSync(
  resolve(root, 'remote/tests/ui/lib/harness.mjs'),
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
    '.head_branch == "main"',
    '.event == "workflow_dispatch"',
    '.created_at >= $dispatch_started_at',
    'candidate_count',
    'ambiguous protected verifier runs',
    'post_dispatch_main_sha',
    'Validate child-run identity',
    'gh run watch "$RUN_ID"',
    '-f ref=main',
  ]) {
    assert.ok(
      browserMcpWorkflow.includes(marker),
      `missing Browser MCP caller marker: ${marker}`,
    );
  }

  assert.match(browserMcpWorkflow, /if: github\.event_name != 'pull_request'/);
  assert.ok(
    browserMcpWorkflow.includes(
      "browser-mcp-external-smoke-${{ github.event_name == 'pull_request' && github.ref || 'live' }}",
    ),
    'live Browser MCP callers must share one concurrency lock',
  );
  assert.doesNotMatch(browserMcpWorkflow, /first\s*\|\s*\.id/);

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

test('AthletO pull requests run only the dependency-free contract', () => {
  for (const marker of [
    'pull_request:',
    'live-smoke security contract',
    "if: github.event_name != 'pull_request'",
    "athleto-ui-${{ github.event_name == 'pull_request' && github.ref || 'live' }}",
    'cancel-in-progress: false',
  ]) {
    assert.ok(
      athletoWorkflow.includes(marker),
      `missing AthletO PR/live separation marker: ${marker}`,
    );
  }
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

test('AthletO live endpoints are fixed, HTTPS-only, and fail fast', () => {
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
    'ATHLETO_MARKETING_URL: https://athleto.store',
    'ATHLETO_APP_URL: https://app.athleto.store',
    '--proto \'=https\'',
    '--proto-redir \'=https\'',
    '--tlsv1.2',
    '--max-redirs 5',
    '--connect-timeout 10',
    '--max-time 30',
    '--retry 2',
    '--retry-max-time 75',
    '--retry-all-errors',
  ]) {
    assert.ok(
      athletoWorkflow.includes(marker),
      `missing bounded HTTPS preflight marker: ${marker}`,
    );
  }

  assert.doesNotMatch(athletoWorkflow, /github\.event\.inputs/);
  assert.doesNotMatch(athletoWorkflow, /^\s+marketing_url:/m);
  assert.doesNotMatch(athletoWorkflow, /^\s+app_url:/m);
});

test('AthletO browsers enforce certificate validation', () => {
  assert.doesNotMatch(athletoHarness, /--ignore-certificate-errors/);
  assert.doesNotMatch(athletoHarness, /ignoreHTTPSErrors/);
  assert.match(athletoHarness, /normalizeLiveTarget/);
});

test('AthletO target parser rejects unsafe or unexpected origins', () => {
  assert.equal(
    normalizeLiveTarget('marketing', 'https://athleto.store/'),
    'https://athleto.store',
  );
  assert.equal(
    normalizeLiveTarget('app', 'https://app.athleto.store'),
    'https://app.athleto.store',
  );

  for (const value of [
    'http://athleto.store',
    'https://user:secret@athleto.store',
    'https://127.0.0.1',
    'https://localhost',
    'https://athleto.store/private',
    'https://athleto.store/?token=secret',
    'https://athleto.store/#fragment',
    'https://athleto.store:8443',
    'https://example.com',
  ]) {
    assert.throws(
      () => normalizeLiveTarget('target', value),
      TypeError,
      `unsafe target should be rejected: ${value}`,
    );
  }

  const allowedOrigins = allowedLiveOrigins('https://preview.athleto.example');
  assert.equal(
    normalizeLiveTarget('preview', 'https://preview.athleto.example', {
      allowedOrigins,
    }),
    'https://preview.athleto.example',
  );
});
