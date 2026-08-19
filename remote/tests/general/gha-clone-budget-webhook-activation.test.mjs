import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(import.meta.dirname, '../../..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');

const cloneDeployment = read('remote/argocd/dd-next-runtime/dd-gha-clone-server.deployment.yaml');
const routerDeployment = read('remote/argocd/dd-next-runtime/dd-gha-executor-router.deployment.yaml');
const clonePolicy = read('remote/argocd/dd-next-runtime/dd-gha-clone-server.networkpolicy.yaml');
const ingress = read('remote/argocd/dd-next-runtime/dd-remote-gateway.ingress.yaml');
const gateway = read('remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml');
const gatewayDaemonSet = read('remote/argocd/dd-next-runtime/dd-remote-gateway.deployment.yaml');
const renewGatewayCert = read('remote/ec2/renew-letsencrypt-gateway-cert.sh');
const cloneConfig = read('remote/argocd/dd-next-runtime/dd-gha-clone-server.configmap.yaml');
const buildPatch = read('remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml');
const registerScript = read('scripts/ops/register_gha_clone_budget_webhook.sh');
const canary = read('scripts/ops/canary_gha_clone_budget_webhook.py');
const runbook = read('docs/operations/gha-budget-webhook-activation.md');

function envLiteral(yaml, name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = yaml.match(new RegExp(`- name: ${escaped}\\n\\s+value: ["']?([^"'\\n]+)["']?`));
  return match?.[1]?.trim();
}

function exactLocation(config, marker, nextMarker) {
  const start = config.indexOf(marker);
  assert.notEqual(start, -1, `missing location marker: ${marker}`);
  const end = config.indexOf(nextMarker, start);
  assert.notEqual(end, -1, `missing next marker after: ${marker}`);
  return config.slice(start, end);
}

test('clone server is one immutable bounded execution replica', () => {
  assert.match(cloneDeployment, /name: dd-gha-clone-server[\s\S]*?replicas: 1\n/);
  assert.match(cloneDeployment, /minReadySeconds: 10/);
  assert.match(
    cloneDeployment,
    /ghcr\.io\/oresoftware\/gha-clone-server@sha256:719a50b3d8cf105cd8c78bb66ce9d10dca072e4de28f6f7ba4fa79db446a2be8/,
  );
  assert.equal(envLiteral(cloneDeployment, 'GHA_CLONE_EXECUTION_ENABLED'), 'true');
  assert.equal(envLiteral(cloneDeployment, 'GHA_CLONE_WEBHOOK_EXECUTION_ENABLED'), 'true');
  assert.equal(envLiteral(cloneDeployment, 'GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS'), 'action_required');
  assert.equal(
    envLiteral(cloneDeployment, 'GHA_CLONE_BUILD_SERVER_URL'),
    'http://dd-gha-executor-router.default.svc.cluster.local:8126',
  );
  assert.doesNotMatch(cloneDeployment, /GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS[\s\S]{0,120}failure,/);
  assert.match(cloneDeployment, /automountServiceAccountToken: false/);
  assert.match(cloneDeployment, /readOnlyRootFilesystem: true/);
  assert.match(cloneDeployment, /drop: \["ALL"\]/);
});

test('executor router is one immutable AWS-only execution replica', () => {
  assert.match(routerDeployment, /name: dd-gha-executor-router[\s\S]*?replicas: 1\n/);
  assert.match(routerDeployment, /minReadySeconds: 10/);
  assert.match(
    routerDeployment,
    /ghcr\.io\/oresoftware\/gha-executor-router@sha256:e87bee0e28911fbdc096d2fec0c1a65811b7d2173594d81c377dc437ac658e8f/,
  );
  assert.equal(envLiteral(routerDeployment, 'GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED'), 'true');
  assert.equal(
    envLiteral(routerDeployment, 'GHA_EXECUTOR_ROUTER_AUTH_PATH'),
    '/var/run/secrets/gha-executor-router/inbound-auth',
  );
  assert.match(routerDeployment, /key: SERVER_AUTH_SECRET\n\s+path: aws-build-server-auth/);
  assert.match(routerDeployment, /automountServiceAccountToken: false/);
  assert.match(routerDeployment, /readOnlyRootFilesystem: true/);
});

test('only the exact HMAC webhook path is public and POST replay is disabled at the edge', () => {
  assert.match(ingress, /name: dd-gha-clone-webhook/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/rewrite-target: \/webhooks\/github/);
  assert.match(ingress, /path: \/gha-webhooks\/github\n\s+pathType: Exact/);
  assert.match(ingress, /name: dd-gha-clone-server\n\s+port:\n\s+number: 8125/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/backend-protocol: "HTTP"/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/ssl-redirect: "true"/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/proxy-body-size: "1m"/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/limit-rps: "5"/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/proxy-next-upstream: "off"/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/proxy-next-upstream-tries: "1"/);
  assert.match(ingress, /nginx\.ingress\.kubernetes\.io\/proxy-request-buffering: "off"/);

  const webhookLocation = exactLocation(
    gateway,
    '      location = /gha-webhooks/github {',
    '      # Inbound webhooks from GitHub',
  );
  assert.match(webhookLocation, /if \(\$request_method != POST\)/);
  assert.match(webhookLocation, /dd-gha-clone-server\.default\.svc\.cluster\.local:8125/);
  assert.match(webhookLocation, /proxy_pass http:\/\/\$dd_up_gha_clone\/webhooks\/github/);
  assert.match(webhookLocation, /limit_req zone=dd_gha_webhook burst=10 nodelay/);
  assert.match(webhookLocation, /proxy_buffering off;/);
  assert.doesNotMatch(webhookLocation, /X-Server-Auth|Auth \$/);

  assert.match(gatewayDaemonSet, /dd\.dev\/gateway-config-revision: '2026-08-19-gha-webhook-no-retry'/);
  assert.match(gatewayDaemonSet, /name: render-webhook-no-retry-config/);
  assert.match(gatewayDaemonSet, /test "\$\(grep -Fxc '      location = \/gha-webhooks\/github \{' "\$input"\)" -eq 1/);
  assert.match(gatewayDaemonSet, /proxy_request_buffering off;/);
  assert.match(gatewayDaemonSet, /proxy_next_upstream off;/);
  assert.match(gatewayDaemonSet, /partial or duplicate webhook no-retry directives; refusing to start/);
  assert.match(gatewayDaemonSet, /runAsNonRoot: true[\s\S]{0,100}runAsUser: 101/);
  assert.match(gatewayDaemonSet, /name: config-rendered[\s\S]{0,160}mountPath: \/etc\/nginx\/templates\/default\.conf\.template/);
  assert.match(renewGatewayCert, /GATEWAY_WORKLOAD="daemonset\/dd-remote-gateway"/);
  assert.match(renewGatewayCert, /kubectl rollout restart "\$\{GATEWAY_WORKLOAD\}"/);
});

test('NetworkPolicy permits only gateway or ingress controller input and router output', () => {
  assert.match(clonePolicy, /app: dd-remote-gateway/);
  assert.match(clonePolicy, /kubernetes\.io\/metadata\.name: ingress-nginx/);
  assert.match(clonePolicy, /app\.kubernetes\.io\/name: ingress-nginx/);
  assert.match(clonePolicy, /app\.kubernetes\.io\/component: controller/);
  assert.match(clonePolicy, /app: dd-gha-executor-router[\s\S]*?port: 8126/);
  assert.doesNotMatch(clonePolicy, /app: dd-build-server/);
});

test('pilot repository, path, and fixed build profile remain exact', () => {
  assert.match(
    cloneConfig,
    /"ORESoftware\/k8s-cluster": \["\.github\/workflows\/gha-clone-server-meta\.yml"\]/,
  );
  assert.match(
    buildPatch,
    /"repository":"https:\/\/github\.com\/ORESoftware\/k8s-cluster\.git","profiles":\["rust-verify"\]/,
  );
});

test('hook registration is unambiguous, workflow_run-only, and keeps the secret out of argv', () => {
  assert.match(registerScript, /events: \["workflow_run"\]/);
  assert.doesNotMatch(registerScript, /"push"|"pull_request"/);
  assert.match(registerScript, /--rawfile secret "\$normalized_secret_file"/);
  assert.match(registerScript, /multiple hooks already use the exact callback URL/);
  assert.match(registerScript, /! -L "\$secret_file"/);
  assert.match(registerScript, /single visible-ASCII line/);
  assert.match(registerScript, /--method PATCH/);
  assert.match(registerScript, /--method POST/);
  assert.match(registerScript, /insecure_ssl: "0"/);
  assert.doesNotMatch(registerScript, /GITHUB_WEBHOOK_SECRET|--arg secret|echo .*\$webhook_secret|printf .*\$webhook_secret/);
  assert.match(registerScript, /https:\/\/98\.90\.186\.114\/gha-webhooks\/github/);
});

test('canary rejects bad HMAC, forbids redirects, and proves one replay creates no new run', () => {
  assert.match(canary, /SHA_RE = re\.compile\(r"\^\[0-9a-f\]\{40\}\$"\)/);
  assert.match(canary, /class NoRedirect/);
  assert.match(canary, /invalid_status != 401/);
  assert.match(canary, /"conclusion": "action_required"/);
  assert.match(canary, /hmac\.new\(webhook_secret, body, hashlib\.sha256\)/);
  assert.match(canary, /"X-GitHub-Event": "workflow_run"/);
  assert.match(canary, /"X-GitHub-Delivery": delivery/);
  assert.match(canary, /"X-Hub-Signature-256": signature/);
  assert.match(canary, /len\(run_ids\) != 1/);
  assert.match(canary, /replay\.get\("accepted"\) is not False/);
  assert.match(canary, /"runIds" in replay/);
  assert.match(canary, /"duplicateDeliverySuppressed": True/);
  assert.match(canary, /run\.get\("workflowPath"\) != args\.workflow_path/);
  assert.match(canary, /state in \{"succeeded", "failed"\}/);
  assert.doesNotMatch(canary, /print\([^\n]*(webhook_secret|clone_auth)/);
  assert.match(canary, /https:\/\/98\.90\.186\.114\/gha-webhooks\/github/);
});

test('runbook states the compatibility limitation and one-change rollback', () => {
  assert.match(runbook, /does not emit a repository webhook named “budget exhausted.”/);
  assert.match(runbook, /compatibility signal rather than cryptographic proof of billing state/);
  assert.match(runbook, /public TLS ingress, HMAC verification, repository extraction/);
  assert.match(runbook, /set `GHA_CLONE_WEBHOOK_EXECUTION_ENABLED=false`/);
  assert.match(runbook, /scale clone server and router to `0`/);
});
