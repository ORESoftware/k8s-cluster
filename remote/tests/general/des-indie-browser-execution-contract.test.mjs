import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const script = fs.readFileSync('scripts/ops/run_des_indie_browser_workflows.sh', 'utf8');
const workflow = fs.readFileSync('.github/workflows/ops-verify-des-indie-plans.yml', 'utf8');
const policy = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
  'utf8',
);

const PLAYWRIGHT_SHA = '104a02c72ca827ec678de0ed6e71d40b6fccbb93';
const PUPPETEER_SHA = '85f58bfa7fd7209c05c691cf6c700b412f11e7d7';

test('DES indie harness uses planner-valid reviewed workflow paths', () => {
  assert.match(script, /PLAYWRIGHT_PATH='\.github\/workflows\/gha-indie-worker\.yml'/);
  assert.match(script, /PUPPETEER_PATH='\.github\/workflows\/gha-indie-worker\.yml'/);
  assert.doesNotMatch(script, /\.gha\/workflows\/(?:playwright|puppeteer)\.yml/);
  assert.match(script, /workflow_path" == \.github\/workflows\/\*\.yml/);
});

test('DES indie harness preserves immutable repository provenance', () => {
  assert.match(script, new RegExp(`PLAYWRIGHT_SHA='${PLAYWRIGHT_SHA}'`));
  assert.match(script, new RegExp(`PUPPETEER_SHA='${PUPPETEER_SHA}'`));
  assert.match(
    script,
    /https:\/\/raw\.githubusercontent\.com\/\$repository\/\$revision\/\$workflow_path/,
  );
  assert.match(script, /--rawfile workflowYaml/);
  assert.match(script, /immutableRevision == true/);
});

test('each explicit retry has a fresh deterministic plan identity', () => {
  assert.match(script, /DES_REQUEST_SUFFIX must use 1-128 GitHub-safe characters/);
  assert.match(script, /\^\[A-Za-z0-9\._-\]\+\$/);
  assert.match(script, /\$\{#REQUEST_SUFFIX\} -gt 128/);
  assert.match(
    script,
    /# gha-indie retry request: %s\\n' "\$REQUEST_SUFFIX"/,
  );
  assert.match(
    script,
    /YAML comments do not alter workflow semantics/,
  );
  assert.match(
    script,
    /--arg requestId "des-browser-\$slug-\$revision-\$REQUEST_SUFFIX"/,
  );
});

test('DES indie harness keeps failures diagnosable and terminal', () => {
  assert.match(script, /--write-out '%\{http_code\}'/);
  assert.match(script, /HTTP \$code from \$url/);
  assert.match(script, /DES_INDIE_PLAN_MISMATCH/);
  assert.match(script, /DES_INDIE_RUN_MISMATCH/);
  assert.match(script, /DES_INDIE_BUILD_LOG_BEGIN/);
  assert.match(script, /\/builds\/\$build_id\/logs/);
  assert.match(script, /tail -c 16000/);
  assert.match(script, /status == "succeeded"/);
  assert.match(script, /\.buildId \| type == "string" and length > 0/);
});

test('DES indie harness coordinates through the in-cluster agent bridge when available', () => {
  assert.match(script, /service\/dd-ai-agent-bridge 18142:8142/);
  assert.match(script, /dd-ai-agent-bridge-secrets/);
  assert.match(script, /bridge-auth\.header/);
  assert.match(script, /chmod 600 "\$bridge_auth_header"/);
  assert.match(script, /\/agents\/register/);
  assert.match(script, /\/channels\/resolve/);
  assert.match(script, /\/channels\/\$bridge_channel\/messages/);
  assert.match(script, /AI_AGENT_BRIDGE_COORDINATION=active/);
});

test('DES indie execution remains narrowly allowlisted and evidence-retained', () => {
  assert.match(
    policy,
    /discrete-event-systems-test\/des-web-playwright-e2e/,
  );
  assert.match(
    policy,
    /discrete-event-systems-test\/des-web-puppeteer-e2e/,
  );
  assert.match(policy, /BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED/);
  assert.match(policy, /value: 'true'/);
  assert.match(workflow, /scripts\/ops\/run_des_indie_browser_workflows\.sh/);
  assert.match(workflow, /retention-days: 30/);
});
