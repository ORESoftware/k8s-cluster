import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const bundlePath =
  'remote/argocd/dd-next-runtime/dd-ai-agent-bridge.networkpolicy.yaml';
const runbookPath = 'docs/ai-agent-bridge-immutable-rollout-2026-08-07.md';
const bundle = readFileSync(bundlePath, 'utf8');
const runbook = readFileSync(runbookPath, 'utf8');
const documents = bundle.split(/\n---\n/u);

const sourceRevision = 'ef5358d54faac1c035e0754f5e7421e10664a75f';
const workflowRun = '31269442497';
const evidenceArtifact = 'image-digest-ores-client-31269442497-1';
const image =
  'ghcr.io/oresoftware/ores-ai-agent-bridge-client@sha256:f6576d1fc2fbadad454c77cc078078695421b138731374347f44f98c67f8269e';

function documentNamed(name: string): string {
  const document = documents.find((candidate) =>
    new RegExp(`^  name: ${name}$`, 'mu').test(candidate),
  );
  assert.ok(document, `${bundlePath} must contain ${name}`);
  return document;
}

function occurrences(text: string, expression: RegExp): number {
  return Array.from(text.matchAll(expression)).length;
}

test('probe and smoke are suspended, exact-digest, manually activated jobs', () => {
  const probe = documentNamed('dd-ai-agent-bridge-client-probe');
  const smoke = documentNamed('dd-ai-agent-bridge-client-smoke');

  assert.equal(occurrences(bundle, /^kind: CronJob$/gmu), 2);
  for (const [name, document, mode] of [
    ['probe', probe, 'probe'],
    ['smoke', smoke, 'smoke'],
  ] as const) {
    assert.match(document, /^  suspend: true$/mu, `${name} must remain suspended`);
    assert.match(document, /^  concurrencyPolicy: Forbid$/mu);
    assert.match(document, /^      backoffLimit: 0$/mu);
    assert.match(document, /^          restartPolicy: Never$/mu);
    assert.match(
      document,
      /^          serviceAccountName: dd-ai-agent-bridge-client-smoke$/mu,
    );
    assert.match(document, /^          automountServiceAccountToken: false$/mu);
    assert.match(document, new RegExp(`^              image: ${image}$`, 'mu'));
    assert.match(document, new RegExp(`^                - ${mode}$`, 'mu'));
    assert.match(document, new RegExp(`dd.dev/source-revision: ${sourceRevision}`, 'u'));
    assert.match(document, new RegExp(`dd.dev/image-workflow-run: "${workflowRun}"`, 'u'));
    assert.match(document, new RegExp(`dd.dev/image-evidence-artifact: ${evidenceArtifact}`, 'u'));
    assert.match(document, /dd\.dev\/activation-mode: suspended-manual/u);
    assert.doesNotMatch(document, /^              command:/mu);
    assert.doesNotMatch(document, /image:\s+[^\n]+:(?:latest|main)\b/u);
  }

  assert.equal(occurrences(bundle, new RegExp(image, 'gu')), 2);
});

test('bearer delivery is secret-backed and never embedded in command, URL, or manifest', () => {
  assert.equal(
    occurrences(bundle, /^                - name: ORES_AI_AGENT_BRIDGE_BEARER$/gmu),
    2,
  );
  assert.equal(
    occurrences(bundle, /^                    secretKeyRef:$/gmu),
    2,
  );
  assert.equal(
    occurrences(bundle, /^                      name: dd-ai-agent-bridge-secrets$/gmu),
    2,
  );
  assert.equal(occurrences(bundle, /^                      key: inbox_token$/gmu), 2);
  assert.equal(
    occurrences(
      bundle,
      /^                  value: http:\/\/dd-ai-agent-bridge\.default\.svc\.cluster\.local:8142$/gmu,
    ),
    2,
  );
  assert.doesNotMatch(bundle, /--bearer/u);
  assert.doesNotMatch(bundle, /https?:\/\/[^\s:@]+:[^\s@]+@/u);
  assert.doesNotMatch(bundle, /ORES_AI_AGENT_BRIDGE_BEARER\s*\n\s*value:/u);
});

test('client pods are non-root, read-only, tokenless, and isolated to DNS plus bridge ports', () => {
  assert.equal(occurrences(bundle, /^            runAsNonRoot: true$/gmu), 2);
  assert.equal(occurrences(bundle, /^            runAsUser: 65532$/gmu), 2);
  assert.equal(occurrences(bundle, /^            runAsGroup: 65532$/gmu), 2);
  assert.equal(occurrences(bundle, /^                allowPrivilegeEscalation: false$/gmu), 2);
  assert.equal(occurrences(bundle, /^                readOnlyRootFilesystem: true$/gmu), 2);
  assert.equal(occurrences(bundle, /^                    - ALL$/gmu), 2);
  assert.equal(occurrences(bundle, /^              type: RuntimeDefault$/gmu), 2);
  assert.doesNotMatch(bundle, /hostNetwork:\s*true/u);
  assert.doesNotMatch(bundle, /hostPID:\s*true/u);
  assert.doesNotMatch(bundle, /hostPath:/u);
  assert.doesNotMatch(bundle, /privileged:\s*true/u);

  const clientPolicy = documents.find(
    (document) =>
      /^kind: NetworkPolicy$/mu.test(document) &&
      /^  name: dd-ai-agent-bridge-client-smoke$/mu.test(document),
  );
  assert.ok(clientPolicy, 'client deny-by-default NetworkPolicy must exist');
  assert.match(clientPolicy, /^  ingress: \[\]$/mu);
  assert.match(clientPolicy, /kubernetes\.io\/metadata\.name: kube-system/u);
  assert.match(clientPolicy, /k8s-app: kube-dns/u);
  assert.match(clientPolicy, /app: dd-ai-agent-bridge/u);
  assert.match(clientPolicy, /operator: DoesNotExist/u);
  assert.match(clientPolicy, /^          port: 53$/gmu);
  assert.match(clientPolicy, /^          port: 8142$/mu);
  assert.match(clientPolicy, /^          port: 8143$/mu);
  assert.doesNotMatch(clientPolicy, /cidr:\s+0\.0\.0\.0\/0/u);

  const serverPolicy = documents[0];
  assert.match(serverPolicy, /app: dd-ai-agent-bridge-client-smoke/u);
  assert.match(serverPolicy, /^          port: 8142$/mu);
  assert.match(serverPolicy, /^          port: 8143$/mu);
});

test('the runbook pins the same evidence and keeps activation manual', () => {
  assert.match(runbook, new RegExp(image, 'u'));
  assert.match(runbook, new RegExp(sourceRevision, 'u'));
  assert.match(runbook, new RegExp(workflowRun, 'u'));
  assert.match(runbook, new RegExp(evidenceArtifact, 'u'));
  assert.match(
    runbook,
    /kubectl -n default create job --from=cronjob\/dd-ai-agent-bridge-client-probe/u,
  );
  assert.match(
    runbook,
    /kubectl -n default create job --from=cronjob\/dd-ai-agent-bridge-client-smoke/u,
  );
  assert.match(runbook, /Both CronJobs remain `suspend: true`/u);
  assert.match(runbook, /Do not unsuspend either CronJob/u);
});
