import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function repoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..'), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, '.github/workflows/browser-mcp-public-e2e.yml'))) return candidate;
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const root = repoRoot();
const workflow = readFileSync(resolve(root, '.github/workflows/browser-mcp-public-e2e.yml'), 'utf8');
const verifier = readFileSync(resolve(root, 'scripts/verify-browser-mcp.sh'), 'utf8');

test('pull requests run only credential-free workflow contracts', () => {
  assert.match(workflow, /pull_request:/);
  assert.match(workflow, /live:[\s\S]*if: github\.event_name != 'pull_request'/);
  assert.match(workflow, /contract:[\s\S]*persist-credentials: false/);
  assert.match(workflow, /actionlint@sha256:[0-9a-f]{64}/);
  assert.match(workflow, /browser-mcp-public-e2e\.test\.ts/);
});

test('the live workflow uses OIDC and keeps the operator secret inside AWS', () => {
  assert.match(workflow, /live:[\s\S]*id-token: write/);
  assert.match(workflow, /aws-actions\/configure-aws-credentials@v6/);
  assert.match(workflow, /aws ssm send-command/);
  assert.match(workflow, /aws secretsmanager get-secret-value/);
  assert.match(workflow, /BROWSER_MCP_OAUTH_OPERATOR_SECRET/);
  assert.doesNotMatch(workflow, /secrets\.BROWSER_MCP_OAUTH_OPERATOR_SECRET/);
  assert.doesNotMatch(workflow, /set -x/);
  assert.doesNotMatch(workflow, /curl[^\n]*(?:--insecure|-k)\b/);
});

test('both canonical public edges receive the full repository verifier', () => {
  assert.match(workflow, /https:\/\/98\.90\.186\.114\/browser-mcp/);
  assert.match(workflow, /https:\/\/hello\.95-217-171-250\.sslip\.io\/browser-mcp/);
  assert.match(workflow, /git show "origin\/\$\{SOURCE_REVISION:-main\}:scripts\/verify-browser-mcp\.sh"/);
  assert.match(workflow, /bash \/tmp\/verify-browser-mcp\.sh "\$endpoint"/);
});

test('live certification checks the immutable GitOps image before browser actions', () => {
  assert.match(workflow, /remote\/argocd\/dd-next-runtime\/dd-web-scraper\.deployment\.yaml/);
  assert.match(workflow, /ghcr\.io\\\/oresoftware\\\/dd-web-scraper@sha256/);
  assert.match(workflow, /kubectl -n default rollout status deployment\/dd-web-scraper/);
  assert.match(workflow, /kubectl -n default rollout status deployment\/dd-browser-mcp-rs/);
  assert.match(workflow, /deployed worker image does not match the main GitOps pin/);
});

test('sanitized artifacts redact credentials and authorization codes', () => {
  assert.match(workflow, /browser-mcp-public-oauth-e2e-result/);
  assert.match(workflow, /Bearer \[REDACTED\]/);
  assert.match(workflow, /access_token\|refresh_token\|operator_secret\|signing_secret/);
  assert.match(workflow, /\(\[\?&\]code=\)/);
  assert.match(workflow, /retention-days: 14/);
});

test('the verifier covers OAuth, exact tools, harmless actions, and fail-closed boundaries', () => {
  for (const marker of [
    'checking unauthenticated 401 and OAuth discovery',
    'code_challenge_method=S256',
    'checking refresh-token rotation and replay rejection',
    '["browser_act", "browser_state"]',
    'checking harmless form fill',
    'checking explicit submit stops for approval',
    'checking that off-allowlist navigation is denied',
    'closing smoke-test session',
  ]) {
    assert.ok(verifier.includes(marker), `verifier is missing coverage marker: ${marker}`);
  }
});
