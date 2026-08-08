import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const workflowPath = '.github/workflows/ai-agent-bridge-public-ingress.yml';
const scriptPath = 'scripts/ci/test-ai-agent-bridge-public-ingress.sh';
const ingressPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.service.yaml';
const runnerPath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-runner.deployment.yaml';

const workflow = readFileSync(workflowPath, 'utf8');
const script = readFileSync(scriptPath, 'utf8');
const ingress = readFileSync(ingressPath, 'utf8');
const runner = readFileSync(runnerPath, 'utf8');

const host = 'api.fiducia.cloud';
const sourceSha = 'ec667946b1f8725b6baea8e67ae6a701d602dc04';
const exactPaths = [
  '/slack/commands/ores-claude',
  '/slack/commands/ores-chatgpt',
  '/slack/interactions',
];

test('public probe is bound to the reviewed host and source contract', () => {
  assert.match(script, new RegExp(`HOST='${host.replaceAll('.', '\\.')}'`));
  assert.match(script, new RegExp(sourceSha));
  assert.match(workflow, new RegExp(sourceSha));
  assert.match(ingress, new RegExp(`host: ${host.replaceAll('.', '\\.')}`));
  for (const path of exactPaths) {
    assert.match(script, new RegExp(path.replaceAll('/', '\\/')));
    assert.match(ingress, new RegExp(`path: ${path.replaceAll('/', '\\/')}`));
  }
});

test('probe remains credential-free and cannot manufacture a valid Slack request', () => {
  for (const content of [workflow, script]) {
    assert.doesNotMatch(content, /secrets\./);
    assert.doesNotMatch(content, /SLACK_SIGNING_SECRET/);
    assert.doesNotMatch(content, /SLACK_BOT_TOKEN/);
    assert.doesNotMatch(content, /x-slack-signature/i);
    assert.doesNotMatch(content, /x-slack-request-timestamp/i);
    assert.doesNotMatch(content, /authorization:/i);
    assert.doesNotMatch(content, /GH_PAT/);
    assert.doesNotMatch(content, /CLOUDFLARE/i);
    assert.doesNotMatch(content, /R2_/i);
  }
  assert.match(script, /probe=unsigned-den-845/);
  assert.match(workflow, /permissions:\n\s+contents: read/);
});

test('transport checks require trusted HTTPS without redirects', () => {
  assert.match(script, /--proto '=https'/);
  assert.match(script, /--tlsv1\.2/);
  assert.match(script, /--max-redirs 0/);
  assert.match(script, /-verify_return_error/);
  assert.match(script, /-verify_hostname "\$\{HOST\}"/);
  assert.match(script, /ssl_verify_result/);
  assert.match(script, /unexpected_effective_url/);
  assert.doesNotMatch(script, /curl\s+-[^\n]*L\b/);
  assert.doesNotMatch(script, /--location/);
});

test('fail-closed routing expectations match the reviewed Axum router', () => {
  for (const name of [
    'unsigned_chatgpt_command',
    'unsigned_claude_command',
    'unsigned_interaction',
  ]) {
    const start = script.indexOf(`'${name}'`);
    assert.notEqual(start, -1, `${name} probe is missing`);
    assert.match(script.slice(start, start + 240), /'POST'[\s\S]*?'401'/);
  }
  assert.match(
    script,
    /'wrong_method_on_exact_command_route'[\s\S]*?'GET'[\s\S]*?'405'/,
  );
  assert.match(
    script,
    /'unknown_slack_route'[\s\S]*?'GET'[\s\S]*?'404'/,
  );
  assert.match(script, /Request authentication failed\./);
});

test('evidence is metadata-only and uploaded even when the probe fails', () => {
  assert.match(script, /"response_body_recorded": False/);
  assert.match(script, /"response_bodies_recorded": False/);
  assert.match(script, /"body_sha256"/);
  assert.match(script, /"body_bytes"/);
  assert.doesNotMatch(script, /"response_body":/);
  assert.match(workflow, /if: always\(\)/);
  assert.match(workflow, /retention-days: 14/);
  assert.match(workflow, /if-no-files-found: warn/);
});

test('workflow dependencies, runner, and triggers are immutable and bounded', () => {
  assert.match(workflow, /runs-on: ubuntu-24\.04/);
  assert.match(workflow, /timeout-minutes: 10/);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /schedule:/);
  assert.doesNotMatch(workflow, /^\s*runs-on:\s*ubuntu-latest\s*$/m);
  for (const match of workflow.matchAll(/^\s*uses:\s*([^\s#]+)/gm)) {
    assert.match(match[1], /^(?:\.\/|[^@]+@[0-9a-fA-F]{40})$/);
  }
});

test('activation gates remain closed while the public edge is measured', () => {
  assert.match(ingress, /- name: SLACK_COMMAND_DRY_RUN\n\s+value: "true"/);
  assert.match(runner, /^\s*replicas:\s*0\s*$/m);
});
