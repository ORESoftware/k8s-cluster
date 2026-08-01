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
  assert.match(workflow, /publish:[\s\S]*if: always\(\) && github\.event_name != 'pull_request'/);
  assert.match(workflow, /contract:[\s\S]*persist-credentials: false/);
  assert.match(workflow, /actionlint@sha256:[0-9a-f]{64}/);
  assert.match(workflow, /browser-mcp-public-e2e\.test\.ts/);
});

test('the live workflow uses OIDC and keeps the operator secret inside AWS', () => {
  const live = workflow.slice(workflow.indexOf('  live:'), workflow.indexOf('  publish:'));
  assert.match(live, /id-token: write/);
  assert.match(live, /contents: read/);
  assert.doesNotMatch(live, /contents: write/);
  assert.match(
    live,
    /aws-actions\/configure-aws-credentials@e6de054238d6b7531b4efff3b6587d9aade6a06c # v6/,
  );
  assert.doesNotMatch(live, /aws-actions\/configure-aws-credentials@v6/);
  assert.match(live, /id: aws[\s\S]*continue-on-error: true/);
  assert.match(live, /aws ssm send-command/);
  assert.match(live, /aws secretsmanager get-secret-value/);
  assert.match(live, /BROWSER_MCP_OAUTH_OPERATOR_SECRET/);
  assert.doesNotMatch(live, /secrets\.BROWSER_MCP_OAUTH_OPERATOR_SECRET/);
  assert.doesNotMatch(live, /set -x/);
  assert.doesNotMatch(live, /curl[^\n]*(?:--insecure|-k)\b/);
});

test('live verification classifies prerequisites without disclosing their values', () => {
  const live = workflow.slice(workflow.indexOf('  live:'), workflow.indexOf('  publish:'));
  assert.match(live, /name: Check live verification prerequisites/);
  assert.match(live, /id: preflight/);
  for (const phase of [
    'aws_role_not_configured',
    'aws_ssm_instance_not_configured',
    'preflight_internal_error',
    'aws_oidc_assume_role_failed',
    'ssm_or_live_verifier_failed',
  ]) {
    assert.ok(live.includes(phase), `missing bounded failure phase: ${phase}`);
  }
  assert.match(live, /if: steps\.preflight\.outputs\.ready == 'true'/);
  assert.match(
    live,
    /if: steps\.preflight\.outputs\.ready == 'true' && steps\.aws\.outcome == 'success'/,
  );
  assert.doesNotMatch(live, /echo[^\n]*(?:ROLE_TO_ASSUME|INSTANCE_ID)/);
  assert.doesNotMatch(live, /jq[^\n]*(?:ROLE_TO_ASSUME|INSTANCE_ID)/);
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

test('the live job always creates and uploads a sanitized result', () => {
  const live = workflow.slice(workflow.indexOf('  live:'), workflow.indexOf('  publish:'));
  assert.match(live, /Initialize sanitized result/);
  assert.match(live, /workflow_precondition_failed/);
  assert.match(live, /failure_phase/);
  assert.match(live, /aws_outcome/);
  assert.match(live, /smoke_outcome/);
  assert.match(
    live,
    /actions\/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7/,
  );
  assert.doesNotMatch(live, /actions\/upload-artifact@v7/);
  assert.match(live, /if: always\(\)/);
  assert.match(live, /retention-days: 14/);
});

test('credential-free publisher writes only the sanitized result branch', () => {
  const publish = workflow.slice(workflow.indexOf('  publish:'));
  assert.match(publish, /actions: read/);
  assert.match(publish, /contents: write/);
  assert.doesNotMatch(publish, /id-token: write/);
  assert.doesNotMatch(publish, /aws-actions\/configure-aws-credentials/);
  assert.doesNotMatch(publish, /secrets\./);
  assert.match(
    publish,
    /actions\/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8/,
  );
  assert.doesNotMatch(publish, /actions\/download-artifact@v8/);
  assert.match(publish, /automation\/browser-mcp-verification-results/);
  assert.match(publish, /browser-mcp\/latest\.json/);
  assert.match(publish, /\.failure_phase/);
  assert.match(publish, /Validate result contains no credentials/);
  assert.doesNotMatch(publish, /HEAD:main/);
});

test('sanitized artifacts redact credentials and authorization codes', () => {
  assert.match(workflow, /Bearer \[REDACTED\]/);
  assert.match(workflow, /access_token\|refresh_token\|operator_secret\|signing_secret/);
  assert.match(workflow, /\(\[\?&\]code=\)/);
  assert.match(workflow, /BROWSER_MCP_OAUTH_OPERATOR_SECRET=\)\[\^\\s\]\+/);
  assert.match(workflow, /BEGIN \(RSA \|EC \|OPENSSH \)\?PRIVATE KEY/);
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
