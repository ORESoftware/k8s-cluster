import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const workflow = readFileSync(
  '.github/workflows/ai-agent-bridge-public-ingress.yml',
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

test('classifier recognizes the observed all-522 incident with metadata only', () => {
  assert.match(classifier, /cloudflare_origin_unreachable/);
  assert.match(classifier, /set\(statuses\) == \{522\}/);
  assert.match(classifier, /edge_tls_reachable/);
  assert.match(classifier, /origin_application_reachable/);
  assert.match(classifier, /response_bodies_recorded/);
  assert.match(classifier, /write_atomic/);
  assert.doesNotMatch(classifier, /SLACK_SIGNING_SECRET/);
  assert.doesNotMatch(classifier, /CLOUDFLARE_API_TOKEN/);
  assert.doesNotMatch(classifier, /authorization:\s*bearer/i);
});

test('workflow executes static contracts and classifier self-test before live probe', () => {
  const staticPosition = workflow.indexOf(
    'ai-agent-bridge-public-ingress-diagnosis-contract.test.mjs',
  );
  const selfTestPosition = workflow.indexOf(
    'classify-ai-agent-bridge-public-ingress.py --self-test',
  );
  const probePosition = workflow.indexOf(
    'run-ai-agent-bridge-public-ingress-diagnostic.sh',
  );
  assert.ok(staticPosition >= 0);
  assert.ok(selfTestPosition > staticPosition);
  assert.ok(probePosition > selfTestPosition);
  assert.match(workflow, /if: always\(\)/);
  assert.match(workflow, /permissions:\n\s+contents: read/);
});

test('runbook keeps activation closed and separates credential-free from operator checks', () => {
  assert.match(runbook, /Credential-free evidence/);
  assert.match(runbook, /Operator-only cluster checks/);
  assert.match(runbook, /SLACK_COMMAND_DRY_RUN=true/);
  assert.match(runbook, /provider runner.*zero/i);
  assert.match(runbook, /do not.*pasted.*credential/i);
  assert.doesNotMatch(runbook, /ghp_[A-Za-z0-9]+/);
  assert.doesNotMatch(runbook, /cfat_[A-Za-z0-9]+/);
});
