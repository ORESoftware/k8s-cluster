import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const contractWorkflow = readFileSync(
  '.github/workflows/ai-agent-bridge-public-ingress-diagnosis-contract.yml',
  'utf8',
);
const opsWorkflow = readFileSync(
  '.github/workflows/ops-ai-agent-bridge-public-ingress-diagnosis.yml',
  'utf8',
);
const wrapper = readFileSync(
  'scripts/ci/run-ai-agent-bridge-public-ingress-diagnostic.sh',
  'utf8',
);
const classifier = readFileSync(
  'scripts/ci/classify-ai-agent-bridge-public-ingress.py',
  'utf8',
);
const runbook = readFileSync(
  'docs/operations/ai-agent-bridge-public-ingress-incident.md',
  'utf8',
);

const immutableAction = /^(?:\.\/|[^@\s]+@[0-9a-fA-F]{40})$/;

function actionReferences(workflow) {
  return [...workflow.matchAll(/(?m)^\s*-?\s*uses:\s*([^\s#]+)/g)].map(
    (match) => match[1],
  );
}

test('wrapper always classifies retained evidence without masking probe failure', () => {
  assert.match(wrapper, /set \+e[\s\S]*bash "\$PROBE"[\s\S]*probe_status=\$\?/);
  assert.match(
    wrapper,
    /python3 "\$CLASSIFIER" --input "\$EVIDENCE_PATH" --output "\$EVIDENCE_PATH"/,
  );
  assert.match(wrapper, /if \[\[ "\$probe_status" != "0" \]\]; then/);
  assert.match(wrapper, /exit "\$probe_status"/);
  assert.match(wrapper, /if \[\[ "\$classifier_status" != "0" \]\]; then/);
  assert.doesNotMatch(wrapper, /\|\|\s*true/);
});

test('classifier recognizes all-522 origin incidents using metadata only', () => {
  assert.match(classifier, /cloudflare_origin_unreachable/);
  assert.match(classifier, /set\(statuses\) == \{522\}/);
  assert.match(classifier, /edge_tls_reachable/);
  assert.match(classifier, /origin_application_reachable/);
  assert.match(classifier, /response_bodies_recorded/);
  assert.match(classifier, /write_atomic/);
  assert.doesNotMatch(classifier, /SLACK_SIGNING_SECRET/);
  assert.doesNotMatch(classifier, /CLOUDFLARE_API_TOKEN/);
  assert.doesNotMatch(classifier, /os\.environ\[["'](?:TOKEN|SECRET|PASSWORD)/);
});

test('pull-request contract is static, immutable, bounded, and credential-free', () => {
  assert.match(contractWorkflow, /pull_request:/);
  assert.match(contractWorkflow, /push:\n\s+branches: \[dev\]/);
  assert.match(contractWorkflow, /workflow_dispatch:/);
  assert.match(contractWorkflow, /permissions:\n\s+contents: read/);
  assert.match(contractWorkflow, /runs-on: ubuntu-24\.04/);
  assert.match(contractWorkflow, /timeout-minutes: 10/);
  assert.match(contractWorkflow, /cancel-in-progress: false/);
  assert.match(contractWorkflow, /\$\{\{ github\.sha \}\}/);
  assert.match(contractWorkflow, /python3 -m py_compile/);
  assert.match(contractWorkflow, /node --test/);
  assert.match(contractWorkflow, /--self-test/);
  assert.match(contractWorkflow, /bash -n scripts\/ci\/run-ai-agent-bridge-public-ingress-diagnostic\.sh/);
  assert.doesNotMatch(
    contractWorkflow,
    /run-ai-agent-bridge-public-ingress-diagnostic\.sh\s*$/m,
  );
  for (const action of actionReferences(contractWorkflow)) {
    assert.match(action, immutableAction);
  }
});

test('manual diagnostic preserves live failure and uploads metadata on every result', () => {
  assert.match(opsWorkflow, /workflow_dispatch:/);
  assert.doesNotMatch(opsWorkflow, /\n  pull_request:/);
  assert.doesNotMatch(opsWorkflow, /\n  push:/);
  assert.doesNotMatch(opsWorkflow, /\n  schedule:/);
  assert.match(opsWorkflow, /permissions:\n\s+contents: read/);
  assert.match(opsWorkflow, /ref: dev/);
  assert.match(
    opsWorkflow,
    /run: bash scripts\/ci\/run-ai-agent-bridge-public-ingress-diagnostic\.sh/,
  );
  assert.match(opsWorkflow, /if: always\(\)/);
  assert.match(opsWorkflow, /retention-days: 14/);
  for (const action of actionReferences(opsWorkflow)) {
    assert.match(action, immutableAction);
  }
});

test('both workflows expose no privileged or secret-bearing surface', () => {
  for (const workflow of [contractWorkflow, opsWorkflow]) {
    for (const forbidden of [
      'secrets.',
      'contents: write',
      'pull-requests: write',
      'id-token: write',
      'services:',
      'container:',
      'sudo ',
      'curl ',
      'wget ',
      'docker ',
      'kubectl ',
      'git push',
    ]) {
      assert.doesNotMatch(workflow, new RegExp(forbidden.replace('.', '\\.')));
    }
  }
});

test('runbook keeps activation closed and separates credential-free from operator checks', () => {
  assert.match(runbook, /Credential-free evidence/);
  assert.match(runbook, /Operator-only cluster checks/);
  assert.match(runbook, /SLACK_COMMAND_DRY_RUN=true/);
  assert.match(runbook, /provider runner replicas=0/i);
  assert.match(runbook, /do not.*credential.*pasted/i);
  assert.doesNotMatch(runbook, /ghp_[A-Za-z0-9]+/);
  assert.doesNotMatch(runbook, /cfat_[A-Za-z0-9]+/);
});
