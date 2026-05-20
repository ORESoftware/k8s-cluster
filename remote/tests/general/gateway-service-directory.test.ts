import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/web-home-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function regexEscape(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function assertHomeCode(source: string, value: string): void {
  assert.match(source, new RegExp(`code \\{ "${regexEscape(value)}" \\}`));
}

function assertDeploymentRow(source: string, deployment: string, service?: string): void {
  const deploymentPattern =
    `DeploymentRow \\{ deployments: &\\[[^\\]]*"${regexEscape(deployment)}"[^\\]]*\\]`;
  assert.match(source, new RegExp(deploymentPattern));

  if (service) {
    const rowPattern =
      `${deploymentPattern}, service: &\\[[^\\]]*"${regexEscape(service)}"[^\\]]*\\]`;
    assert.match(source, new RegExp(rowPattern));
  }
}

function assertPathEntry(source: string, label: string, href?: string): void {
  const hrefPattern = href === undefined ? '[^}]+' : `Some\\("${regexEscape(href)}"\\)`;
  assert.match(
    source,
    new RegExp(`PathEntry \\{ label: "${regexEscape(label)}", href: ${hrefPattern} \\}`),
  );
}

test('rust homepage lists public task paths and protected ops paths', async () => {
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  assert.match(home, /dd remote service directory/);
  assert.match(home, /Public entrypoint for the EC2 Kubernetes runtime\. Open paths:/);
  for (const path of [
    '/',
    '/home',
    '/auth',
    '/agents/tasks',
    '/agents/threads',
    '/api/agents/tasks',
    '/presence-test',
    '/wss-test',
    '/webrtc/',
    '/fsws/',
    '/mdp/',
    '/des/',
  ]) {
    assertHomeCode(home, path);
  }
  assert.match(home, /Server-auth paths:/);
  for (const path of [
    '/lambdas/functions',
    '/lambdas/invoke/<function-id>',
    '/api/lambdas/',
    '/api/agent-worker/',
    '/container-pools',
    '/bastion/',
    '/scrape',
    '/trading/',
    '/contracts/',
    '/ml/',
    '/builds',
    '/gleam/',
    '/mcp',
    '/gcs/',
  ]) {
    assertHomeCode(home, path);
  }
  assert.match(home, /Internal-access ops:/);
  for (const path of [
    '/headlamp/',
    '/telemetry/',
    '/prometheus/',
    '/nats/',
    '/nats-metrics/',
    '/reaper/',
    '/cron/',
  ]) {
    assertHomeCode(home, path);
  }
  assert.match(home, /h2 \{ "Deployments" \}/);
  assertDeploymentRow(home, 'dd-web-scraper', 'dd-web-scraper:8097');
  assert.match(home, /scraper parser workers/);
  assertDeploymentRow(home, 'dd-build-server', 'dd-build-server:8100');
  assert.match(home, /Rust CI\/CD server/);
  assertDeploymentRow(home, 'dd-vpn');
  assertDeploymentRow(home, 'dd-live-mutex');
  assertDeploymentRow(home, 'dd-bastion');
  assertDeploymentRow(home, 'dd-redis-cache');
  assertDeploymentRow(home, 'dd-lock-loadtest-trigger');
  assertDeploymentRow(home, 'dd-container-pool');
  assert.match(home, /h2 \{ "Live containers" \}/);
  assert.match(home, /\/bastion\/runtime\/deployments/);
  assert.match(home, /const runtimeReloadIntervalMs = 30000/);
  assert.match(home, /HTTP poll plus websocket-triggered refresh/);
  assert.match(home, /openRuntimeSocket\("gleam", `\/admin\/gleam\/ws\?channel=k8s-runtime-admin&client=home-\$\{clientId\}`\)/);
  assert.match(home, /openRuntimeSocket\("rust", `\/admin\/webrtc\/runtime\/ws\?client=home-\$\{clientId\}`\)/);
  assert.match(home, /startTimedReload\(\)/);
  assert.match(home, /home-terminal-frame/);
  assert.match(home, /Open bastion exec terminal/);
  assert.match(home, /const safeBastionTerminalUrl = \(value\) =>/);
  assert.match(home, /url\.pathname !== "\/bastion\/terminal"/);
  assert.match(home, /ignored unsafe bastion terminal URL/);
  assert.match(home, /const safeTerminalUrl = safeBastionTerminalUrl\(container\.terminalUrl\)/);
  assertPathEntry(home, 'POST /builds', '/builds');
  assertPathEntry(home, '/builds/<jobId>/logs', '/builds/example-job/logs');
  assertDeploymentRow(home, 'dd-gleam-lambda-runner', 'dd-gleam-lambda-runner:8083');
  assert.match(home, /dd-gleam-lambda-runner-secrets/);
  assert.doesNotMatch(home, /Auth: [^<"]+/);
  assert.match(home, /legacy Auth header/);
  assert.match(home, /Node\.js Coding Agent Task Manager/);
  assert.doesNotMatch(home, /Node control-plane API/);
  assertPathEntry(home, '/tasks', '/tasks');
  assertPathEntry(home, '/status', '/status');
  assertPathEntry(home, '/stream/<uuid>', '/stream/example-task-id');
  assertPathEntry(home, '/', '/');
  assertPathEntry(home, '/home', '/home');
  assertPathEntry(home, '/agents/tasks', '/agents/tasks');
  assertPathEntry(home, '/agents/threads', '/agents/threads');
  assert.match(home, /rejects requests for the wrong pinned thread/);
  assert.match(home, /Kubernetes Ingress selects the UUID-bound worker Service/);
  assert.match(home, /Kubernetes per-thread Ingress/);
  assertPathEntry(home, '/dd-thread/<short>', '/dd-thread/example');
  assertPathEntry(home, '/dd-thread/<short>/tasks', '/dd-thread/example/tasks');
  assert.match(home, /Ingress selects the UUID-bound worker Service/);
  assert.doesNotMatch(home, /routes by thread UUID\/taskId/);
  assertPathEntry(home, '/agents/tasks', '/agents/tasks');
  assertPathEntry(home, '/agents/threads', '/agents/threads');
  assert.match(home, /Rust web homepage deployment/);
  assert.match(home, /Service directory plus cluster-served task\/thread\/PR UI/);
  assert.match(home, /stored events/);
  assertPathEntry(home, '/api/agents/tasks', '/api/agents/tasks');
  assertPathEntry(
    home,
    '/api/agents/threads/<uuid>/context',
    '/api/agents/threads/example-thread-id/context',
  );
  assertPathEntry(home, '/lambdas/functions', '/lambdas/functions');
  assertPathEntry(home, '/api/lambdas/functions', '/api/lambdas/functions');
  assertPathEntry(
    home,
    'POST /lambdas/invoke/<function-id>',
    '/lambdas/invoke/00000000-0000-0000-0000-000000000000',
  );
  assert.match(home, /dd-gleam-lambda-runner deployment \+ Rust REST API/);
  assert.match(home, /Gleam child-process runner/);
  assertPathEntry(home, '/auth', '/auth?return=/home');
  assert.match(home, /Rust PIN auth service/);
  assert.match(home, /dd_auth/);
  assertPathEntry(home, '/bastion/runtime/deployments', '/bastion/runtime/deployments');
  assert.match(home, /Rust bastion\/jumphost access broker/);
  assert.match(home, /allowlisted browser exec terminals/);
  assert.match(home, /option value="openai-sdk" selected \{ "openai-sdk" \}/);
  assert.match(home, /option value="generic-ai-sdk" \{ "generic-ai-sdk" \}/);
  assert.match(home, /option value="opencode-ai-sdk" \{ "opencode-ai-sdk" \}/);
  assert.doesNotMatch(home, /option value="echo" \{ "echo" \}/);
  assert.match(home, /Queue Consumer/);
  assert.match(home, /Rust NATS shadow preparer \(dd-remote-queue-consumer\)/);
  assert.match(home, /Rust NATS Queue Consumer/);
  assert.match(home, /dd\.remote\.thread\.\*\.tasks/);
  assertPathEntry(
    home,
    'POST /api/agents/threads/<uuid>/prepare',
    '/api/agents/threads/example-thread-id/prepare',
  );
  assertDeploymentRow(home, 'dd-remote-queue-consumer');
  assert.match(home, /It does not execute prompts/);
  assertPathEntry(home, '/gleam/home', '/gleam/home');
  assertPathEntry(home, '/gleam/healthz', '/gleam/healthz');
  assertPathEntry(home, '/gleam/metrics', '/gleam/metrics');
  assertPathEntry(home, '?preset=gleam', '/wss-test?preset=gleam');
  assertPathEntry(home, '?preset=webrtc', '/wss-test?preset=webrtc');
  assertPathEntry(home, '?preset=gcs', '/wss-test?preset=gcs');
  assertPathEntry(home, '?preset=fsrx', '/wss-test?preset=fsrx');
  assert.match(home, /\/fsws\/ws\/rx-burst/);
  assert.match(home, /id="send-burst"/);
  assert.match(home, /id="start-interval"/);
  assert.match(home, /id="stop-interval"/);
  assert.match(home, /wss:\/\/<host>\/gleam\/ws/);
  assert.doesNotMatch(home, /ws:\/\/54\.91\.17\.58\/gleam\/ws/);
  assertPathEntry(home, '/mcp', '/mcp');
  assertPathEntry(home, '/mcp/home', '/mcp/home');
  assertPathEntry(home, '/mcp/healthz', '/mcp/healthz');
  assertPathEntry(home, '/mcp/metrics', '/mcp/metrics');
  assert.match(home, /Gleam MCP service/);
  assertPathEntry(home, '/webrtc/', '/webrtc/');
  assertPathEntry(home, '/webrtc/healthz', '/webrtc/healthz');
  assertPathEntry(home, '/webrtc/metrics', '/webrtc/metrics');
  assertPathEntry(home, '/webrtc/signal test', '/wss-test?preset=webrtc');
  assert.match(home, /\/webrtc\/signal/);
  assert.match(home, /Rust WebRTC signaling service/);
  assert.match(home, /Media and data channels stay peer-to-peer/);
  assertPathEntry(home, 'POST /scrape', '/scrape');
  assertPathEntry(home, '/scrape/healthz', '/scrape/healthz');
  assert.match(home, /Playwright, Puppeteer, and Browserless scraping/);
  assert.doesNotMatch(home, /href="\/scraper/);
  assertPathEntry(home, '/telemetry/', '/telemetry/');
  assertPathEntry(home, '/prometheus/', '/prometheus/');
  assertPathEntry(home, '/nats/', '/nats/');
  assertPathEntry(home, '/nats-metrics/metrics', '/nats-metrics/metrics');
  assertPathEntry(home, '/reaper/', '/reaper/');
  assertPathEntry(home, '/cron/', '/cron/');
  assert.match(
    home,
    /Today: the public gateway keeps ops paths behind temporary internal access while bootstrap work is still in flight\./,
  );
});

test('gateway exposes public task paths and protects ops paths behind temporary Auth header', async () => {
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const gatewayDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.deployment.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const authDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-auth.deployment.yaml',
  );
  const authService = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-auth.service.yaml',
  );
  const authServer = await readRepoFile('remote/deployments/auth-server-rs/src/main.rs');
  const webrtcDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-webrtc-signaling.deployment.yaml',
  );
  const webrtcService = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-webrtc-signaling.service.yaml',
  );
  const scraperDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml',
  );
  const scraperService = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-web-scraper.service.yaml',
  );
  const buildServerDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml',
  );
  const buildServerService = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server.service.yaml',
  );
  const buildServerRbac = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server-rbac.yaml',
  );
  const buildServerNetworkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server.networkpolicy.yaml',
  );
  const lambdaDeployment = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.deployment.yaml',
  );
  const lambdaService = await readRepoFile(
    'remote/deployments/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.service.yaml',
  );
  const lambdaApp = await readRepoFile(
    'remote/argocd/apps/dd-gleam-lambda-runner.application.yaml',
  );

  assert.match(gateway, /map \$http_auth \$dd_gateway_header_auth_ok/);
  assert.match(gateway, /map \$cookie_dd_auth \$dd_gateway_cookie_auth_ok/);
  assert.match(
    gateway,
    /map "\$dd_gateway_header_auth_ok:\$dd_gateway_cookie_auth_ok" \$dd_gateway_auth_ok/,
  );
  assert.match(gateway, /map \$dd_gateway_auth_ok \$dd_gateway_auth_header/);
  assert.match(gateway, /map \$dd_gateway_auth_ok \$dd_dev_server_auth_header/);
  assert.match(gateway, /map \$http_accept \$dd_gateway_accepts_html/);
  assert.match(gateway, /default\s+0/);
  assert.match(gateway, /default\.conf\.template/);
  assert.match(gateway, /"\$\{DD_REMOTE_GATEWAY_AUTH_VALUE\}"\s+1/);
  assert.match(
    gateway,
    /location = \/auth[\s\S]*dd-remote-auth\.default\.svc\.cluster\.local:8083/,
  );
  assert.match(gateway, /location @auth_required/);
  assert.match(gateway, /return 302 \/auth\?return=\$request_uri/);
  assert.doesNotMatch(gateway, /location @auth_required_html/);
  assert.match(gateway, /proxy_set_header Auth \$dd_gateway_auth_header/);
  assert.match(gateway, /resolver kube-dns\.kube-system\.svc\.cluster\.local/);
  assert.match(
    gateway,
    /location ~ "\^\/dd-thread\/\(\?<thread_short>\[a-z0-9\]\{12\}\)\(\?<thread_path>\/\.\*\)\?\$"/,
  );
  assert.match(
    gateway,
    /location ~ "\^\/dd-thread\/\(\?<thread_short>\[a-z0-9\]\{12\}\)\/ws\$"[\s\S]*proxy_pass http:\/\/\$dd_thread_service:8080\/ws\$is_args\$args/,
  );
  assert.match(
    gateway,
    /set \$dd_thread_service dd-thread-\$thread_short\.default\.svc\.cluster\.local/,
  );
  assert.match(gateway, /set \$dd_thread_proxy_path \$thread_path/);
  assert.match(
    gateway,
    /if \(\$dd_thread_proxy_path = ""\) {\s*set \$dd_thread_proxy_path \/;\s*}/,
  );
  assert.match(gateway, /proxy_set_header X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"/);
  assert.match(
    gateway,
    /location ~ "\^\/dd-thread\/\(\?<thread_short>\[a-z0-9\]\{12\}\)\(\?<thread_path>\/\.\*\)\?\$"[\s\S]*proxy_set_header Upgrade \$http_upgrade[\s\S]*proxy_set_header Connection \$connection_upgrade/,
  );
  assert.match(gateway, /proxy_pass http:\/\/\$dd_thread_service:8080\$dd_thread_proxy_path/);
  assert.doesNotMatch(gateway, /location ~ "\^\/dd-thread\/\(\[a-z0-9\]\{12\}\)\(\/\.\*\)\$"/);
  assert.doesNotMatch(
    gateway,
    /location ~ "\^\/dd-thread\/\(\?<thread_short>\[a-z0-9\]\{12\}\)\(\?<thread_path>\/\.\*\)\$"/,
  );
  assert.doesNotMatch(gateway, /rewrite "\^\/dd-thread\/\[a-z0-9\]\{12\}\(\/\.\*\)\$" \$1 break/);
  assert.doesNotMatch(
    gateway,
    /set \$dd_thread_service dd-thread-\$1\.default\.svc\.cluster\.local/,
  );
  assert.doesNotMatch(gateway, /proxy_pass http:\/\/\$dd_thread_service:8080\$thread_path/);
  assert.doesNotMatch(gateway, /proxy_set_header X-Server-Auth "dd-k8s-home"/);
  assert.match(
    gateway,
    /location\s+\/agents\/tasks[\s\S]*dd-remote-web-home\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gateway,
    /location\s+\/agents\/threads[\s\S]*dd-remote-web-home\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gateway,
    /location\s+\/api\/agents\/[\s\S]*proxy_set_header X-Server-Auth \$dd_dev_server_auth_header[\s\S]*dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
  );
  assert.match(gateway, /location = \/bastion[\s\S]*return 302 \/bastion\/profile/);
  assert.match(
    gateway,
    /location\s+\/bastion\/[\s\S]*proxy_set_header Upgrade \$http_upgrade[\s\S]*proxy_set_header X-Bastion-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*set \$dd_bastion_upstream dd-bastion\.vpn\.svc\.cluster\.local:8111[\s\S]*rewrite \^\/bastion\/\?\(\.\*\)\$ \/\$1 break[\s\S]*proxy_pass http:\/\/\$dd_bastion_upstream/,
  );
  assert.match(
    gateway,
    /location\s+\/tasks[\s\S]*proxy_set_header X-Server-Auth \$dd_dev_server_auth_header[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gateway,
    /location\s+\/status[\s\S]*proxy_set_header X-Server-Auth \$dd_dev_server_auth_header[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gateway,
    /location\s+\/stream\/[\s\S]*proxy_set_header X-Server-Auth \$dd_dev_server_auth_header[\s\S]*dd-dev-server-api\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gateway,
    /location\s+\/lambdas\/functions[\s\S]*if \(\$dd_gateway_auth_ok = 0\)[\s\S]*dd-remote-web-home\.default\.svc\.cluster\.local:8080/,
  );
  assert.match(
    gateway,
    /location\s+\/api\/lambdas\/[\s\S]*if \(\$dd_gateway_auth_ok = 0\)[\s\S]*proxy_set_header X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
  );
  assert.match(
    gateway,
    /location\s+\/lambdas\/invoke\/[\s\S]*if \(\$dd_gateway_auth_ok = 0\)[\s\S]*proxy_set_header X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-gleam-lambda-runner\.default\.svc\.cluster\.local:8083\/invoke\//,
  );
  assert.match(gateway, /location = \/telemetry[\s\S]*return 302 \/telemetry\//);
  assert.match(
    gateway,
    /location\s+\/telemetry\/[\s\S]*dd-grafana\.observability\.svc\.cluster\.local:3000/,
  );
  assert.match(gateway, /location = \/prometheus[\s\S]*return 302 \/prometheus\//);
  assert.match(
    gateway,
    /location\s+\/prometheus\/[\s\S]*dd-prometheus\.observability\.svc\.cluster\.local:9090\//,
  );
  assert.match(gateway, /location = \/nats[\s\S]*return 302 \/nats\//);
  assert.match(
    gateway,
    /location\s+\/nats\/[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:8222\//,
  );
  assert.match(gateway, /location = \/nats-metrics[\s\S]*return 302 \/nats-metrics\/metrics/);
  assert.match(
    gateway,
    /location\s+\/nats-metrics\/[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:7777\//,
  );
  assert.match(gateway, /location = \/gleam[\s\S]*return 302 \/gleam\/home/);
  assert.match(
    gateway,
    /location\s+\/gleam\/[\s\S]*dd-gleamlang-server\.default\.svc\.cluster\.local:8081\//,
  );
  assert.match(
    gateway,
    /location = \/mcp[\s\S]*dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090\/mcp/,
  );
  assert.match(
    gateway,
    /location\s+\/mcp\/[\s\S]*dd-gleam-mcp-server\.default\.svc\.cluster\.local:8090\//,
  );
  assert.match(gateway, /location = \/webrtc[\s\S]*return 302 \/webrtc\//);
  assert.match(
    gateway,
    /location\s+\/webrtc\/[\s\S]*proxy_set_header Upgrade \$http_upgrade[\s\S]*dd-webrtc-signaling\.default\.svc\.cluster\.local:8095/,
  );
  assert.match(
    gateway,
    /location = \/scrape[\s\S]*dd-web-scraper\.default\.svc\.cluster\.local:8097/,
  );
  assert.match(
    gateway,
    /location \/scrape\/[\s\S]*dd-web-scraper\.default\.svc\.cluster\.local:8097/,
  );
  assert.match(
    gateway,
    /location = \/builds[\s\S]*proxy_set_header X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-build-server\.default\.svc\.cluster\.local:8100/,
  );
  assert.match(
    gateway,
    /location \/builds\/[\s\S]*proxy_set_header X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-build-server\.default\.svc\.cluster\.local:8100/,
  );
  assert.match(gateway, /location = \/reaper[\s\S]*return 302 \/reaper\//);
  assert.match(gateway, /location\s+\/reaper\//);
  assert.match(gateway, /location = \/cron[\s\S]*return 302 \/cron\//);
  assert.match(gateway, /location\s+\/cron\//);
  assert.match(gateway, /location @auth_required/);
  assert.doesNotMatch(gateway, /requiredHeader":"Auth: all-dogs-go-to-heaven"/);
  assert.match(gateway, /"errMessage":"missing required dd header"/);
  assert.match(gateway, /errMessage":"missing required dd header"/);
  assert.doesNotMatch(gateway, /requiredHeader/);
  assert.match(
    gatewayDeployment,
    /name:\s*DD_REMOTE_GATEWAY_AUTH_VALUE[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-remote-auth-secrets[\s\S]*key:\s*DD_AUTH_COOKIE_VALUE/,
  );
  assert.match(
    gatewayDeployment,
    /name:\s*DD_REMOTE_DEV_SERVER_AUTH_VALUE[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-agent-secrets[\s\S]*key:\s*SERVER_AUTH_SECRET/,
  );
  assert.match(
    gatewayDeployment,
    /mountPath:\s*\/etc\/nginx\/templates\/default\.conf\.template[\s\S]*subPath:\s*default\.conf\.template/,
  );
  assert.doesNotMatch(gatewayDeployment, /\/etc\/nginx\/conf\.d\/default\.conf/);
  assert.match(kustomization, /dd-remote-auth\.deployment\.yaml/);
  assert.match(kustomization, /dd-remote-auth\.service\.yaml/);
  assert.match(kustomization, /dd-webrtc-signaling\.deployment\.yaml/);
  assert.match(kustomization, /dd-webrtc-signaling\.service\.yaml/);
  assert.match(kustomization, /dd-web-scraper\.deployment\.yaml/);
  assert.match(kustomization, /dd-web-scraper\.service\.yaml/);
  assert.match(kustomization, /dd-build-server-rbac\.yaml/);
  assert.match(kustomization, /dd-build-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-build-server\.service\.yaml/);
  assert.match(kustomization, /dd-build-server\.networkpolicy\.yaml/);
  assert.match(webrtcDeployment, /name:\s*dd-webrtc-signaling/);
  assert.match(webrtcDeployment, /cd \/opt\/dd-next-1\/remote\/deployments\/webrtc-signaling-rs/);
  assert.match(webrtcDeployment, /containerPort:\s*8095/);
  assert.match(webrtcService, /name:\s*dd-webrtc-signaling/);
  assert.match(webrtcService, /port:\s*8095/);
  assert.match(scraperDeployment, /name:\s*dd-web-scraper/);
  assert.match(scraperDeployment, /cd \/opt\/dd-next-1\/remote\/deployments\/web-scraper-service/);
  assert.match(scraperDeployment, /containerPort:\s*8097/);
  assert.match(scraperService, /name:\s*dd-web-scraper/);
  assert.match(scraperService, /port:\s*8097/);
  assert.match(buildServerDeployment, /name:\s*dd-build-server/);
  assert.match(buildServerDeployment, /serviceAccountName:\s*dd-build-server/);
  assert.match(buildServerDeployment, /cd \/opt\/dd-next-1\/remote\/deployments\/build-server-rs/);
  assert.match(buildServerDeployment, /containerPort:\s*8100/);
  assert.match(
    buildServerDeployment,
    /BUILD_SERVER_ALLOWED_IMAGE_PREFIXES[\s\S]*710156900967\.dkr\.ecr\.us-east-1\.amazonaws\.com\//,
  );
  assert.match(buildServerDeployment, /BUILD_SERVER_ALLOWED_NAMESPACES[\s\S]*value:\s*default/);
  assert.match(buildServerDeployment, /BUILD_SERVER_PUSH_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(buildServerDeployment, /BUILD_SERVER_ECR_LOGIN_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(buildServerDeployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(buildServerDeployment, /mountPath:\s*\/run\/containerd\/containerd\.sock/);
  assert.match(buildServerDeployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.match(buildServerDeployment, /mountPath:\s*\/usr\/bin\/kubectl/);
  assert.match(buildServerService, /name:\s*dd-build-server/);
  assert.match(buildServerService, /port:\s*8100/);
  assert.match(buildServerRbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-build-server/);
  assert.match(buildServerRbac, /resources: \[deployments\]/);
  assert.match(buildServerRbac, /resources: \[ingresses\]/);
  assert.doesNotMatch(buildServerRbac, /resources: \[secrets\]/);
  assert.doesNotMatch(buildServerRbac, /serviceaccounts/);
  assert.doesNotMatch(buildServerRbac, /daemonsets/);
  assert.match(buildServerNetworkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(buildServerNetworkPolicy, /app:\s*dd-build-server/);
  assert.match(buildServerNetworkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(lambdaDeployment, /name:\s*dd-gleam-lambda-runner/);
  assert.match(lambdaDeployment, /cd \/opt\/dd-next-1\/remote\/deployments\/gleam-lambda-runner/);
  assert.match(lambdaDeployment, /containerPort:\s*8083/);
  assert.match(lambdaService, /name:\s*dd-gleam-lambda-runner/);
  assert.match(lambdaService, /port:\s*8083/);
  assert.match(lambdaApp, /path:\s*remote\/deployments\/gleam-lambda-runner\/k8s\/ec2/);
  assert.match(authDeployment, /name:\s*dd-remote-auth/);
  assert.match(
    authDeployment,
    /name:\s*DD_AUTH_PIN[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-remote-auth-secrets[\s\S]*key:\s*DD_AUTH_PIN/,
  );
  assert.match(
    authDeployment,
    /name:\s*DD_AUTH_COOKIE_VALUE[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-remote-auth-secrets[\s\S]*key:\s*DD_AUTH_COOKIE_VALUE/,
  );
  assert.match(authDeployment, /name:\s*DD_AUTH_COOKIE_MAX_AGE_SECONDS[\s\S]*value:\s*'3600'/);
  assert.match(
    authDeployment,
    /name:\s*DD_AUTH_TOTP_SECRET_BASE32[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-remote-auth-secrets[\s\S]*key:\s*DD_AUTH_TOTP_SECRET_BASE32[\s\S]*optional:\s*true/,
  );
  assert.match(authDeployment, /name:\s*DD_AUTH_TOTP_WINDOW_STEPS[\s\S]*value:\s*'1'/);
  const authPinEnvBlock =
    authDeployment.match(/- name: DD_AUTH_PIN[\s\S]*?(?=\n\s*- name:|\n\s*ports:)/)?.[0] ?? '';
  const authCookieValueEnvBlock =
    authDeployment.match(
      /- name: DD_AUTH_COOKIE_VALUE[\s\S]*?(?=\n\s*- name:|\n\s*ports:)/,
    )?.[0] ?? '';
  assert.doesNotMatch(authPinEnvBlock, /\n\s*value:\s*/);
  assert.doesNotMatch(authCookieValueEnvBlock, /\n\s*value:\s*/);
  assert.match(authDeployment, /name:\s*http[\s\S]*containerPort:\s*8083/);
  assert.match(authDeployment, /requests:[\s\S]*cpu:\s*50m[\s\S]*memory:\s*96Mi/);
  assert.match(authDeployment, /limits:[\s\S]*cpu:\s*['"]?1['"]?[\s\S]*memory:\s*1Gi/);
  assert.match(authDeployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(authDeployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(authDeployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(authService, /name:\s*dd-remote-auth/);
  assert.match(authService, /name:\s*http[\s\S]*port:\s*8083[\s\S]*targetPort:\s*8083/);
  assert.match(authServer, /fn auth_pin\(\) -> String/);
  assert.match(authServer, /fn cookie_name\(\) -> String/);
  assert.match(authServer, /fn cookie_value\(\) -> String/);
  assert.match(authServer, /fn valid_totp_code/);
  assert.match(authServer, /DD_AUTH_TOTP_SECRET_BASE32/);
  assert.match(authServer, /constant_time_eq/);
  assert.match(authServer, /fn validate_required_config\(\)/);
  assert.match(authServer, /validate_required_config\(\);/);
  assert.doesNotMatch(authServer, /DD_AUTH_PIN"\)\.unwrap_or_else/);
  assert.doesNotMatch(authServer, /DD_AUTH_COOKIE_VALUE"\)\.unwrap_or_else/);
  assert.match(authServer, /header::SET_COOKIE/);
  assert.match(authServer, /HttpOnly/);
  assert.match(authServer, /SameSite=Lax/);
  assert.match(authServer, /Secure/);
  assert.match(authServer, /safe_return_to/);
  assert.match(authServer, /if value\.starts_with\('\/'\) && !value\.starts_with\("\/\/"\)/);
  assert.match(authServer, /"\/home"\.to_string\(\)/);
  assert.match(authServer, /StatusCode::SEE_OTHER/);
  assert.match(authServer, /header::LOCATION/);
});

test('gateway terminates self-signed TLS on host port 443', async () => {
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.deployment.yaml',
  );
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const acmeLocationBlock =
    gateway.match(/location \/\.well-known\/acme-challenge\/ \{[\s\S]*?\n      \}/)?.[0] ?? '';

  assert.match(gateway, /listen 80 default_server/);
  assert.match(gateway, /listen 443 ssl default_server/);
  assert.match(gateway, /return 308 https:\/\/\$host\$request_uri/);
  assert.match(gateway, /ssl_certificate \/etc\/nginx\/tls\/tls\.crt/);
  assert.match(gateway, /ssl_certificate_key \/etc\/nginx\/tls\/tls\.key/);
  assert.match(gateway, /ssl_protocols TLSv1\.2 TLSv1\.3/);
  assert.match(gateway, /Strict-Transport-Security "max-age=3600" always/);
  assert.match(gateway, /Content-Security-Policy "upgrade-insecure-requests" always/);
  assert.match(gateway, /location \/\.well-known\/acme-challenge\//);
  assert.match(gateway, /root \/var\/www\/acme/);
  assert.match(acmeLocationBlock, /default_type text\/plain/);
  assert.match(acmeLocationBlock, /try_files \$uri =404/);
  assert.doesNotMatch(acmeLocationBlock, /\$dd_gateway_auth_ok/);
  assert.doesNotMatch(acmeLocationBlock, /proxy_pass/);
  assert.match(deployment, /name:\s*http[\s\S]*containerPort:\s*80[\s\S]*hostPort:\s*80/);
  assert.match(deployment, /name:\s*https[\s\S]*containerPort:\s*443[\s\S]*hostPort:\s*443/);
  assert.match(deployment, /mountPath:\s*\/etc\/nginx\/tls[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /mountPath:\s*\/var\/www\/acme[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /secretName:\s*dd-remote-gateway-tls/);
  assert.match(deployment, /path:\s*\/home\/ec2-user\/dd-acme-webroot/);
  assert.match(deployment, /type:\s*DirectoryOrCreate/);
  assert.match(runtimeReadme, /HTTP remains enabled for ACME HTTP-01[\s\S]*redirects browser traffic to HTTPS/);
  assert.match(runtimeReadme, /curl -k https:\/\/54\.91\.17\.58\/home/);
  assert.match(runtimeReadme, /\/home\/ec2-user\/dd-acme-webroot/);
  assert.match(runtimeReadme, /--webroot-path \/home\/ec2-user\/dd-acme-webroot/);
});

test("gateway Let's Encrypt renewal script stays aligned with the ACME webroot flow", async () => {
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const ec2Readme = await readRepoFile('remote/ec2/README.md');
  const renewScript = await readRepoFile('remote/ec2/renew-letsencrypt-gateway-cert.sh');
  const renewService = await readRepoFile('remote/ec2/dd-letsencrypt-renew.service');
  const renewTimer = await readRepoFile('remote/ec2/dd-letsencrypt-renew.timer');

  assert.match(renewScript, /^#!\/usr\/bin\/env bash/m);
  assert.match(renewScript, /set -euo pipefail/);
  assert.match(
    renewScript,
    /CERTBOT_BIN="\$\{CERTBOT_BIN:-\/home\/ec2-user\/certbot-venv-312\/bin\/certbot\}"/,
  );
  assert.match(renewScript, /K8S_SECRET_NAME="\$\{K8S_SECRET_NAME:-dd-remote-gateway-tls\}"/);
  assert.match(renewScript, /kubectl create secret tls "\$\{K8S_SECRET_NAME\}"/);
  assert.match(
    renewScript,
    /--cert="\$\{CERTBOT_CONFIG_DIR\}\/live\/\$\{CERT_NAME\}\/fullchain\.pem"/,
  );
  assert.match(
    renewScript,
    /--key="\$\{CERTBOT_CONFIG_DIR\}\/live\/\$\{CERT_NAME\}\/privkey\.pem"/,
  );
  assert.match(renewScript, /kubectl rollout restart "deployment\/\$\{K8S_GATEWAY_DEPLOYMENT\}"/);
  assert.match(renewScript, /kubectl rollout status "deployment\/\$\{K8S_GATEWAY_DEPLOYMENT\}"/);
  assert.match(renewScript, /"\$\{CERTBOT_BIN\}" renew/);
  assert.match(renewScript, /--deploy-hook "\$0 deploy"/);
  assert.match(ec2Readme, /certbot-venv-312/);
  assert.match(ec2Readme, /remote\/ec2\/renew-letsencrypt-gateway-cert\.sh deploy/);
  assert.match(ec2Readme, /remote\/ec2\/renew-letsencrypt-gateway-cert\.sh renew/);
  assert.match(ec2Readme, /dd-letsencrypt-renew\.service/);
  assert.match(ec2Readme, /dd-letsencrypt-renew\.timer/);
  assert.match(ec2Readme, /systemctl enable --now dd-letsencrypt-renew\.timer/);
  assert.match(renewService, /\[Service\]/);
  assert.match(renewService, /Type=oneshot/);
  assert.match(renewService, /User=ec2-user/);
  assert.match(renewService, /WorkingDirectory=\/home\/ec2-user\/codes\/dd\/dd-next-1/);
  assert.match(
    renewService,
    /ExecStart=\/home\/ec2-user\/codes\/dd\/dd-next-1\/remote\/ec2\/renew-letsencrypt-gateway-cert\.sh renew/,
  );
  assert.match(renewTimer, /\[Timer\]/);
  assert.match(renewTimer, /OnBootSec=15min/);
  assert.match(renewTimer, /OnUnitActiveSec=6h/);
  assert.match(renewTimer, /RandomizedDelaySec=30min/);
  assert.match(renewTimer, /Persistent=true/);
  assert.match(renewTimer, /Unit=dd-letsencrypt-renew\.service/);
  assert.match(runtimeReadme, /dd-remote-gateway-tls/);
  assert.match(runtimeReadme, /remote\/ec2\/renew-letsencrypt-gateway-cert\.sh deploy/);
  assert.match(runtimeReadme, /dd-letsencrypt-renew\.timer/);
});

test('rust agent tasks page fetches the REST API directly', async () => {
  const server = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const cargo = await readRepoFile('remote/deployments/web-home-rs/Cargo.toml');
  const readme = await readRepoFile('remote/deployments/web-home-rs/readme.md');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-web-home.deployment.yaml',
  );
  const dockerfile = await readRepoFile('remote/deployments/web-home-rs/Dockerfile');
  const refreshWorkflow = await readRepoFile(
    '.github/workflows/refresh-remote-web-home-local-image.yml',
  );
  const maintenanceWorkflow = await readRepoFile('.github/workflows/remote-k8s-maintenance.yml');
  const restDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.deployment.yaml',
  );
  const restService = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.service.yaml',
  );
  const restRbac = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api-rbac.yaml',
  );
  const restReadme = await readRepoFile('remote/deployments/rest-api-rs/readme.md');
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');

  assert.match(server, /\/agents\/tasks/);
  assert.match(server, /Thread chat/);
  assert.match(server, /threadIngressPrefix/);
  assert.match(server, /\/dd-thread\/\$\{threadShort\(threadId\)\}/);
  assert.match(server, /\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/tasks/);
  assert.match(
    server,
    /\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/stream\/\$\{encodeURIComponent\(taskId\)\}/,
  );
  assert.match(server, /Node\.js Coding Agent Task Manager/);
  assert.doesNotMatch(server, /Rust web UI \+ Rust REST API/);
  assert.doesNotMatch(server, /routes by thread UUID\/taskId/);
  assert.match(server, /fetch\(`\/api\/agents\/tasks\?limit=\$\{encodeURIComponent\(limit\)\}`/);
  assert.match(restReadme, /GET \/api\/agents\/tasks\?limit=/);
  assert.match(restReadme, /AGENT_TASKS_ADMIN_USER_ID/);
  assert.match(restReadme, /REMOTE_DEV_ADMIN_USER_ID/);
  assert.doesNotMatch(server, /\/agents\/tasks\/data/);
  assert.doesNotMatch(server, /REMOTE_REST_API_URL/);
  assert.match(readme, /Node\.js is not the UUID router/);
  assert.match(readme, /\/dd-thread\/<thread-short>\/tasks/);
  assert.doesNotMatch(cargo, /reqwest/);
  assert.match(deployment, /image:\s*docker\.io\/library\/dd-remote-web-home:dev/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /runAsUser:\s*10001/);
  assert.match(deployment, /runAsGroup:\s*10001/);
  assert.doesNotMatch(deployment, /\/opt\/dd-next-1/);
  assert.doesNotMatch(deployment, /hostPath:/);
  assert.doesNotMatch(deployment, /cargo run --release/);
  assert.match(deployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(dockerfile, /COPY --from=build \/app\/target\/release\/dd-remote-web-home/);
  assert.match(dockerfile, /USER 10001:10001/);
  assert.match(dockerfile, /CMD \["\/usr\/local\/bin\/dd-remote-web-home"\]/);
  assert.match(refreshWorkflow, /name: Refresh remote web-home local image/);
  assert.match(refreshWorkflow, /push:[\s\S]*branches:[\s\S]*-\s*dev/);
  assert.match(refreshWorkflow, /remote\/deployments\/web-home-rs\/\*\*/);
  assert.match(refreshWorkflow, /workflow_dispatch:/);
  assert.match(refreshWorkflow, /group: refresh-remote-web-home-local-image/);
  assert.match(refreshWorkflow, /ref: dev/);
  assert.match(
    refreshWorkflow,
    /role-to-assume: \$\{\{ secrets\.AWS_ROLE_TO_ASSUME \|\| secrets\.REMOTE_DEV_AWS_ROLE_TO_ASSUME \|\| secrets\.AWS_OIDC_ROLE_ARN \|\| secrets\.AWS_ECR_ROLE_ARN \}\}/,
  );
  assert.match(refreshWorkflow, /aws-region: \$\{\{ vars\.AWS_REGION \|\| secrets\.AWS_REGION \|\| 'us-east-1' \}\}/);
  assert.match(
    refreshWorkflow,
    /nerdctl -n k8s\.io build --progress=plain[\s\S]*-t docker\.io\/library\/dd-remote-web-home:dev remote\/deployments\/web-home-rs/,
  );
  assert.match(
    refreshWorkflow,
    /sudo -u ec2-user -H bash -lc '\\''cd \/home\/ec2-user\/codes\/dd\/dd-next-1 && kubectl apply -f remote\/argocd\/apps\/dd-next-runtime\.application\.yaml/,
  );
  assert.match(
    refreshWorkflow,
    /sudo -u ec2-user -H bash -lc '\\''kubectl -n argocd get application\/dd-next-runtime/,
  );
  assert.match(
    refreshWorkflow,
    /sudo -u ec2-user -H bash -lc '\\''kubectl -n argocd annotate application\/dd-next-runtime argocd\.argoproj\.io\/refresh=hard --overwrite/,
  );
  assert.match(
    refreshWorkflow,
    /kubectl delete pod -n default -l app=dd-remote-web-home --wait=true --timeout=120s/,
  );
  assert.match(
    refreshWorkflow,
    /kubectl wait --for=condition=Available deployment\/dd-remote-web-home -n default --timeout=300s/,
  );
  assert.doesNotMatch(refreshWorkflow, /kubectl apply -f remote\/argocd\/dd-next-runtime/);
  assert.match(refreshWorkflow, /aws ssm send-command/);
  assert.match(refreshWorkflow, /aws ssm get-command-invocation/);
  assert.match(maintenanceWorkflow, /-\s*verify-gleam-mcp-server/);
  assert.match(maintenanceWorkflow, /verify-gleam-mcp-server\)/);
  assert.match(maintenanceWorkflow, /remote\/ec2\/verify-gleam-mcp-server\.sh/);
  assert.doesNotMatch(deployment, /REMOTE_REST_API_URL/);
  assert.doesNotMatch(deployment, /dd-remote-web-home-secrets/);
  assert.match(restDeployment, /name:\s*dd-agent-secrets[\s\S]*optional:\s*true/);
  assert.match(restDeployment, /name:\s*dd-remote-rest-api-secrets[\s\S]*optional:\s*true/);
  assert.match(restDeployment, /name:\s*THREAD_RUNTIME_NAMESPACE[\s\S]*value:\s*default/);
  assert.match(restDeployment, /name:\s*THREAD_RUNTIME_CAPACITY_PRUNE_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(restDeployment, /name:\s*THREAD_RUNTIME_MAX_AWAKE_DEPLOYMENTS[\s\S]*value:\s*'4'/);
  assert.match(restDeployment, /name:\s*http[\s\S]*containerPort:\s*8082/);
  assert.match(restDeployment, /startupProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(restDeployment, /readinessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(restDeployment, /livenessProbe:[\s\S]*path:\s*\/healthz[\s\S]*port:\s*http/);
  assert.match(restService, /name:\s*http[\s\S]*port:\s*8082[\s\S]*targetPort:\s*8082/);
  assert.match(restService, /port:\s*8082/);
  assert.match(restRbac, /kind:\s*ServiceAccount[\s\S]*namespace:\s*default/);
  assert.match(
    restRbac,
    /kind:\s*RoleBinding[\s\S]*name:\s*dd-remote-rest-api-control-plane[\s\S]*namespace:\s*default/,
  );
  assert.match(
    restRbac,
    /kind:\s*RoleBinding[\s\S]*name:\s*dd-remote-rest-api-control-plane[\s\S]*namespace:\s*dd-dev/,
  );
  assert.match(restServer, /AGENT_TASKS_RDS_DATABASE_URL/);
  assert.match(restServer, /RDS_DATABASE_URL/);
  assert.match(restServer, /known_git_repos/);
  assert.match(restServer, /struct KnownGitRepoRow/);
  assert.match(restServer, /async fn fetch_known_git_repos_from_postgres/);
  assert.match(restServer, /async fn upsert_known_git_repo_to_postgres/);
  assert.match(restServer, /agent_remote_dev_threads/);
  assert.match(restServer, /agent_remote_dev_tasks/);
  assert.match(restServer, /agent_remote_dev_events/);
  assert.match(restServer, /struct AgentEventRow/);
  assert.match(restServer, /struct AgentTaskEventsResponse/);
  assert.match(restServer, /struct AgentFeedbackRequest/);
  assert.match(restServer, /async fn fetch_agent_events_from_postgres/);
  assert.match(restServer, /async fn persist_feedback_event_to_postgres/);
  assert.match(restServer, /GET", "\/api\/agents\/tasks\/:taskId\/events"/);
  assert.match(restServer, /POST", "\/api\/agents\/tasks\/:taskId\/feedback"/);
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/tasks\/:task_id\/events",\s*get\(agent_task_events\)\s*\)/,
  );
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/tasks\/:task_id\/feedback",\s*post\(agent_task_feedback\),\s*\)/,
  );
  assert.match(restServer, /"source": "agents-threads-ui"/);
  assert.match(
    restServer,
    /fn agent_tasks_admin_user_id\(\) -> Option<String> \{\s*first_env\(&\["AGENT_TASKS_ADMIN_USER_ID", "REMOTE_DEV_ADMIN_USER_ID"\]\)/,
  );
  assert.match(restServer, /SUPABASE_SERVICE_ROLE_KEY/);
  assert.match(
    restServer,
    /AGENT_TASKS_ADMIN_USER_ID or REMOTE_DEV_ADMIN_USER_ID is not configured/,
  );
  assert.match(
    restServer,
    /insert into agent_remote_dev_threads[\s\S]*\(id, user_id, known_git_repo_id, title, repo, base_branch, is_soft_deleted, created_at, updated_at, created_by, updated_by\)/,
  );
  assert.match(
    restServer,
    /insert into agent_remote_dev_tasks[\s\S]*\(id, thread_id, user_id, docker_task_id, prompt, status, branch, last_event_seq, meta, is_soft_deleted, started_at, created_at, updated_at, created_by, updated_by\)/,
  );
  assert.match(restServer, /meta = agent_remote_dev_tasks\.meta \|\| excluded\.meta/);
  assert.match(restServer, /struct AgentContextCandidatesRequest/);
  assert.match(restServer, /async fn fetch_agent_context_candidates_from_postgres/);
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/context-candidates",\s*post\(thread_context_candidates\),\s*\)/,
  );
  assert.match(restServer, /updated_by = excluded\.updated_by/);
  assert.match(restServer, /fn public_data_source_error\(source: &str\) -> String \{/);
  assert.match(restServer, /fn thread_runtime_namespace\(\) -> String \{/);
  assert.match(
    restServer,
    /env::var\("THREAD_RUNTIME_NAMESPACE"\)\.unwrap_or_else\(\|_\| "default"\.to_string\(\)\)/,
  );
  assert.match(maintenanceWorkflow, /free-thread-pod-slots/);
  assert.match(maintenanceWorkflow, /sync-agent-gh-pat/);
  assert.match(maintenanceWorkflow, /sync-agent-model-keys/);
  assert.match(maintenanceWorkflow, /data\["XAI_MODELS"\]\s*=\s*"grok-4\.3"/);
  assert.match(maintenanceWorkflow, /REMOTE_DEV_GH_PAT/);
  assert.match(maintenanceWorkflow, /dd\/remote-dev\/agent-secrets/);
  assert.match(maintenanceWorkflow, /THREAD_RUNTIME_MAX_AWAKE_DEPLOYMENTS/);
  assert.match(restServer, /source unavailable; check remote REST API server logs/);
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/git-repos",\s*get\(known_git_repos\)\.post\(save_known_git_repo\),\s*\)/,
  );
  assert.match(
    restServer,
    /agent tasks data source is not configured; showing runtime memory only/,
  );
  assert.doesNotMatch(
    restServer,
    /agent tasks data source is not configured; check remote REST API deployment secrets/,
  );
  assert.match(
    restServer,
    /This REST API is the database boundary\. Point AGENT_TASKS_RDS_DATABASE_URL at RDS today, then swap to an in-cluster Postgres service later\./,
  );
  assert.doesNotMatch(restServer, /errors\.push\(format!\("postgres: \{error\}"\)\)/);
  assert.doesNotMatch(restServer, /errors\.push\(format!\("supabase: \{error\}"\)\)/);
  assert.doesNotMatch(restServer, /unwrap_or_else\(\|_\| "dd-dev"\.to_string\(\)\)/);
  assert.doesNotMatch(restServer, /no data source configured; set AGENT_TASKS_RDS_DATABASE_URL/);
});

test('otel collector scrapes the rust REST API', async () => {
  const collector = await readRepoFile(
    'remote/argocd/observability/otel-collector.configmap.yaml',
  );

  assert.match(collector, /job_name: dd-remote-rest-api/);
  assert.match(collector, /dd-remote-rest-api\.default\.svc\.cluster\.local:8082/);
});

test('rust agent tasks page keeps the direct REST fetch error contract', async () => {
  const server = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  assert.doesNotMatch(server, /agent_remote_dev_threads/);
  assert.doesNotMatch(server, /SUPABASE_SERVICE_ROLE_KEY/);
  assert.doesNotMatch(server, /rest-api-unavailable/);
  assert.doesNotMatch(server, /fn limit_from_query\(query: &AgentsQuery\) -> i64 \{/);
  assert.doesNotMatch(
    server,
    /Json\(fetch_agents_snapshot\(limit_from_query\(&query\)\)\.await\)/,
  );
  assert.match(server, /const clearSnapshot = \(\) => \{/);
  assert.match(server, /setStat\("thread-count", 0\)/);
  assert.match(server, /renderTasks\(\[\]\)/);
  assert.match(server, /renderThreads\(\[\]\)/);
  assert.match(server, /const publicLoadError = \(error\) => \{/);
  assert.match(
    server,
    /agent tasks are temporarily unavailable; check remote web-home server logs/,
  );
  assert.match(
    server,
    /fetch\(`\/api\/agents\/tasks\?limit=\$\{encodeURIComponent\(limit\)\}`,\s*\{ cache: "no-store" \}\)/,
  );
  assert.match(server, /if \(!response\.ok\) \{/);
  assert.match(
    server,
    /throw new Error\(`agent tasks request failed \(\$\{response\.status\}\)`\)/,
  );
  assert.match(server, /clearSnapshot\(\);[\s\S]*errors\.textContent = publicLoadError\(error\);/);
  assert.doesNotMatch(server, /errors\.textContent = String\(error\)/);
  assert.doesNotMatch(server, /\.route\("\/agents\/tasks\/data"/);
});

test('rust agent threads page renders stored response events and feedback controls', async () => {
  const server = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const readme = await readRepoFile('remote/deployments/web-home-rs/readme.md');
  const restReadme = await readRepoFile('remote/deployments/rest-api-rs/readme.md');
  const threadsJs = server.slice(
    server.indexOf('const AGENTS_THREADS_JS'),
    server.indexOf('const AGENTS_TASKS_CSS'),
  );

  assert.match(server, /async fn agents_threads_page\(\) -> impl IntoResponse/);
  assert.match(server, /use maud::\{html, Markup, PreEscaped, DOCTYPE\}/);
  assert.match(server, /const AGENTS_THREADS_CSS: &str/);
  assert.match(server, /const AGENTS_THREADS_JS: &str/);
  assert.doesNotMatch(server, /const AGENTS_THREADS_HTML: &str/);
  assert.match(server, /body \{[\s\S]*overflow: hidden;/);
  assert.match(server, /\.app \{[\s\S]*height: 100dvh;[\s\S]*overflow: hidden;/);
  assert.match(
    server,
    /\.sidebar \{[\s\S]*overflow: hidden auto;[\s\S]*overscroll-behavior: contain;/,
  );
  assert.match(
    server,
    /\.thread-list \{[\s\S]*overflow: auto;[\s\S]*overscroll-behavior: contain;/,
  );
  assert.match(server, /\.stream \{[\s\S]*overflow: auto;[\s\S]*overscroll-behavior: contain;/);
  assert.match(server, /\.thread-meta > span \{[\s\S]*text-overflow: ellipsis;/);
  assert.match(
    server,
    /section id="thread-control-panel" class="panel prompt-panel" tabindex="0" aria-label="Thread control panel"/,
  );
  assert.match(server, /h2 \{ "Thread Control" \}/);
  assert.match(server, /span id="thread-mode" class="pill warn" \{ "select thread" \}/);
  assert.match(server, /button, select, input, textarea \{[\s\S]*max-width: 100%;/);
  assert.match(
    server,
    /\.workspace-flow \{[\s\S]*display: flex;[\s\S]*flex-direction: column;[\s\S]*overflow: hidden;/,
  );
  assert.match(
    server,
    /\.prompt-panel \{[\s\S]*flex: 0 0 auto;[\s\S]*min-height: 154px;[\s\S]*max-height: none;[\s\S]*overflow: visible;/,
  );
  assert.match(server, /\.main\.control-top #thread-control-panel \{[\s\S]*order: 1;/);
  assert.match(server, /\.main\.control-bottom #thread-control-panel \{[\s\S]*order: 2;/);
  assert.match(server, /\.main\.control-sliding-down #thread-control-panel \{[\s\S]*animation: control-slide-down 260ms ease;/);
  assert.match(server, /\.prompt-panel label,[\s\S]*\.field-wide \{[\s\S]*min-width: 0;/);
  assert.match(server, /\.prompt-actions,[\s\S]*\.status-line \{[\s\S]*margin-top: 12px;/);
  assert.match(server, /section id="response-stream-panel" class="panel stream-panel" tabindex="0" aria-label="Response stream panel"/);
  assert.match(server, /aside id="previous-tasks-panel" class="tasks-sidebar" tabindex="0" aria-label="Thread tasks sidebar"/);
  assert.match(server, /\.tasks-sidebar \{[\s\S]*display: flex;[\s\S]*flex-direction: column;[\s\S]*overflow: hidden;/);
  assert.match(server, /\.stream-panel > \.stream,[\s\S]*\.stream-panel > \.terminal-inline \{[\s\S]*flex: 1 1 auto;/);
  assert.match(server, /function setWorkspaceLayout\(mode\) \{/);
  assert.match(server, /function setControlPosition\(position, options = \{\}\) \{/);
  assert.match(server, /function setThreadUiMode\(modeName\) \{/);
  assert.match(server, /const UUID_PATTERN = \/\^\[0-9a-f\]\{8\}-\[0-9a-f\]\{4\}-\[0-9a-f\]\{4\}-\[0-9a-f\]\{4\}-\[0-9a-f\]\{12\}\$\/i;/);
  assert.match(server, /function readUuidInput\(id, label, options = \{\}\) \{/);
  assert.match(server, /const requestedThread = queryUuid\(params, "thread"\)/);
  assert.match(server, /const threadId = readUuidInput\("thread-id", "thread UUID", \{ generate: true \}\)/);
  assert.match(server, /class="main mode-empty control-top"/);
  assert.match(server, /\.main\.mode-new #terminal-thread[\s\S]*display: none;/);
  assert.match(server, /\$\("send"\)\.textContent = modeName === "new" \? "Create thread & send"/);
  assert.match(server, /\$\("thread-control-panel"\)\.addEventListener\("click", handleControlPanelClick\)/);
  assert.match(server, /function setTaskStreamLayout\(mode\) \{/);
  assert.match(server, /function handleLowerPanelClick\(event, mode\) \{/);
  assert.match(server, /\$\("previous-tasks-panel"\)\.addEventListener\("click", \(event\) => handleLowerPanelClick\(event, "tasks"\)\)/);
  assert.match(server, /\$\("response-stream-panel"\)\.addEventListener\("click", \(event\) => handleLowerPanelClick\(event, "stream"\)\)/);
  assert.doesNotMatch(server, /div class="grid" style="margin-top: 14px"/);
  assert.match(server, /\.route\("\/agents\/threads", get\(agents_threads_page\)\)/);
  assert.match(server, /\.route\("\/agents\/threads\/", get\(agents_threads_page\)\)/);
  assert.match(server, /script defer="defer" src="https:\/\/cdn\.jsdelivr\.net\/npm\/rxjs@7\.8\.1\/dist\/bundles\/rxjs\.umd\.min\.js" crossorigin="anonymous"/);
  assert.match(server, /Agent threads/);
  assert.match(server, /id="thread-list"/);
  assert.match(server, /select id="repo-url"/);
  assert.match(server, /input id="repo-url-new"/);
  assert.match(server, /input id="zero-context" type="checkbox"/);
  assert.match(server, /div id="context-candidates" class="context-candidates"/);
  assert.match(server, /\.context-candidates \{[\s\S]*overflow: auto;/);
  assert.match(server, /async function loadContextCandidates\(threadId, prompt, repo, baseBranch, promptKey\) \{/);
  assert.match(
    server,
    /fetch\(`\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/context-candidates`,/,
  );
  assert.match(server, /contextMode: contextDispatch\.contextMode/);
  assert.match(server, /contextIds: contextDispatch\.contextIds/);
  assert.match(server, /placeholder="git@github\.com:org\/repo\.git or org\/repo"/);
  assert.match(server, /New repo URL\.\.\./);
  assert.match(server, /const BUILTIN_GIT_REPOS = \[/);
  assert.match(server, /https:\/\/github\.com\/ORESoftware\/live-mutex\.git/);
  assert.match(server, /https:\/\/github\.com\/benefactor-cc\/benefactor-cc\.github\.io\.git/);
  assert.match(server, /https:\/\/github\.com\/ORESoftware\/k8s-cluster\.git/);
  assert.match(server, /https:\/\/github\.com\/ORESoftware\/us-anti-corruption-court-project\.git/);
  assert.match(server, /https:\/\/github\.com\/dancing-dragons\/dd-next-1\.git/);
  assert.match(server, /const \{ combineLatest, from, of \} = window\.rxjs/);
  assert.match(server, /from\(fetchPgKnownRepos\(\)\)\.pipe\(catchError\(\(\) => of\(\[\]\)\)\)/);
  assert.match(server, /mergeKnownRepos\(BUILTIN_GIT_REPOS, storedRepos\)/);
  assert.match(server, /const REPO_URL_HELP = "repo must start with git@, ssh:\/\/, or https:\/\/; GitHub owner\/repo shorthand is also accepted"/);
  assert.match(server, /const GITHUB_REPO_SHORTHAND_PATTERN = \/\^\(\[A-Za-z0-9\]\[A-Za-z0-9_\.-\]\*\)\\\/\(\[A-Za-z0-9\]\[A-Za-z0-9_\.-\]\*\?\)\(\?:\\\.git\)\?\$\/;/);
  assert.match(server, /return `https:\/\/github\.com\/\$\{shorthand\[1\]\}\/\$\{shorthand\[2\]\}\.git`;/);
  assert.match(server, /function validateCurrentRepoUrl\(\) \{/);
  assert.match(server, /\$\("repo-url-new"\)\.addEventListener\("blur", validateRepoUrlField\)/);
  assert.match(server, /const validateCurrentChatRepoUrl = \(\) => \{/);
  assert.match(server, /\$\("chat-repo-url-new"\)\.addEventListener\("blur", validateChatRepoUrlField\)/);
  assert.match(server, /id="task-list"/);
  assert.match(server, /id="stream"/);
  assert.match(server, /Response stream/);
  assert.match(server, /h2 \{ "Tasks" \}/);
  assert.match(
    server,
    /fetch\(`\/api\/agents\/tasks\/\$\{encodeURIComponent\(taskId\)\}\/events\?limit=250`, \{ cache: "no-store" \}\)/,
  );
  assert.match(
    server,
    /fetch\(`\/api\/agents\/tasks\/\$\{encodeURIComponent\(state\.selectedTaskId\)\}\/feedback`,/,
  );
  assert.match(
    server,
    /fetch\(`\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/tasks`,/,
  );
  assert.match(server, /fetch\("\/api\/agents\/git-repos\?limit=100"/);
  assert.match(server, /repo,\s*baseBranch,/);
  assert.match(
    server,
    /new EventSource\(`\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/stream\/\$\{encodeURIComponent\(taskId\)\}`\)/,
  );
  assert.match(
    server,
    /button id="save-repo" type="button" title="Save this repo URL and default branch to the known repo list" \{ "Save repo URL" \}/,
  );
  assert.match(
    server,
    /button id="sleep-thread" type="button" title="Reduce resources by scaling the thread container to zero" \{ "Pause\/Sleep" \}/,
  );
  assert.match(
    server,
    /button id="archive-thread" class="warn" type="button" title="Deep sleep: suspend the thread container" \{ "Archive" \}/,
  );
  assert.match(server, /Delete runtime/);
  assert.match(server, /Merge with upstream/);
  assert.match(
    server,
    /button id="commit-thread" type="button" title="Commit current worker changes and push the thread branch" \{ "Make commit" \}/,
  );
  assert.match(server, /Open draft PR/);
  assert.match(
    server,
    /button id="terminal-thread" type="button" title="Open a shell in the thread's Node\.js worker container" \{ "Terminal" \}/,
  );
  assert.match(server, /div id="terminal-inline" class="terminal-inline" hidden="hidden"/);
  assert.match(server, /iframe id="terminal-frame" title="Thread worker terminal"/);
  assert.match(server, /function openInlineTerminal\(targetUrl\) \{/);
  assert.match(server, /\$\("terminal-frame"\)\.src = targetUrl;/);
  assert.match(server, /function trustedTerminalUrl\(threadId, candidate\) \{/);
  assert.match(server, /parsed\.origin !== window\.location\.origin \|\| parsed\.pathname !== expectedPath \|\| returnedThreadId !== normalizeUuid\(threadId\)/);
  assert.match(server, /ignored unsafe terminal URL from control response/);
  assert.match(server, /terminalTargetUrl = terminalUrlFromControlResponse\(threadId, body\)/);
  assert.match(server, /clearStream\("waking terminal"\)/);
  assert.match(server, /Waking the selected worker and opening its shell inside the response panel/);
  assert.match(server, /if \(terminalTargetUrl\) openInlineTerminal\(terminalTargetUrl\);/);
  assert.doesNotMatch(threadsJs, /window\.open\(/);
  assert.doesNotMatch(threadsJs, /terminalWindow/);
  assert.match(server, /sendFeedback\(seq, vote, button\)/);
  assert.match(server, /collectText\(raw\)/);
  assert.match(server, /let sawTextKey = false/);
  assert.match(server, /if \(!out\.length && !sawTextKey\) \{/);
  assert.match(server, /model stream \$\{String\(finishReason\)\.toLowerCase\(\)\}/);
  assert.match(server, /const AGENT_TEXT_JOIN_DELAY_MS = 1200/);
  assert.match(server, /const AGENT_TEXT_MAX_BUFFER_MS = 3000/);
  assert.match(server, /function shouldCoalesceAgentText\(row, text\) \{/);
  assert.match(server, /if \(\/\^model stream\\b\/i\.test\(text\.trim\(\)\)\) return false/);
  assert.match(server, /function flushAgentTextBuffer\(\) \{/);
  assert.match(server, /seqLabel: pending\.firstSeq === pending\.lastSeq \? `seq \$\{pending\.firstSeq\}` : `seq \$\{pending\.firstSeq\}-\$\{pending\.lastSeq\}`/);
  assert.match(server, /for \(const event of data\.events\) renderEventRow\(event\);[\s\S]*flushAgentTextBuffer\(\);/);
  assert.match(server, /Creating or waking the UUID-bound worker/);
  assert.match(server, /NATS container pool/);
  assert.match(server, /queueing container-pool task/);
  assert.match(server, /thread UUID as the affinity key/);
  assert.match(server, /lastRuntimeData: null/);
  assert.match(server, /async function readableFetchError\(response, label\) \{/);
  assert.match(server, /gateway returned HTML; retrying/);
  assert.match(server, /function workerRuntimeWaitDetails\(data\) \{/);
  assert.match(server, /runtime phase=\$\{summary\.phase \|\| "unknown"\}/);
  assert.match(server, /node pod-slot limit full/);
  assert.match(server, /dispatch waiting \$\{elapsed\}s · \$\{runtimeSummary\}/);
  assert.match(server, /workerRuntimeWaitDetails\(state\.lastRuntimeData\)/);
  assert.match(server, /dispatch accepted/);
  assert.match(server, /streamTaskId: null/);
  assert.match(server, /async function loadTaskEvents\(taskId, options = \{\}\) \{/);
  assert.match(server, /if \(options\.preserveCurrentOnEmpty && state\.streamTaskId === taskId && \$\("stream"\)\.childElementCount > 0\) \{/);
  assert.match(server, /setStreamState\("showing live status", "ok"\)/);
  assert.match(server, /if \(options\.appendOnly\) \{[\s\S]*state\.streamTaskId = taskId;[\s\S]*\} else \{[\s\S]*clearStream\("loading events", taskId\);[\s\S]*\}/);
  assert.match(server, /async function loadSnapshot\(options = \{\}\) \{/);
  assert.match(server, /snapshotFailures: 0/);
  assert.match(server, /snapshotRetryTimer: null/);
  assert.match(server, /function scheduleSnapshotRetry\(options = \{\}\) \{/);
  assert.match(server, /function handleSnapshotError\(error, options = \{\}\) \{/);
  assert.match(server, /snapshot unavailable · retrying/);
  assert.match(server, /snapshot temporarily unavailable; retrying/);
  assert.match(server, /if \(options\.preserveStreamForTask !== state\.selectedTaskId\) \{[\s\S]*await loadTaskEvents\(state\.selectedTaskId, \{[\s\S]*preserveCurrentOnEmpty: state\.streamTaskId === state\.selectedTaskId,[\s\S]*\}\);[\s\S]*\}/);
  assert.match(server, /loadSnapshot\(\{ preserveStreamForTask: taskId \}\)/);
  assert.match(server, /handleSnapshotError\(error, \{ preserveStreamForTask: taskId \}\)/);
  assert.match(server, /setInterval\(\(\) => \{[\s\S]*loadSnapshot\(\{ preserveStreamForTask: state\.selectedTaskId \}\)[\s\S]*appendOnly: true,[\s\S]*\}, 10000\);/);
  assert.match(
    server,
    /fetch\(`\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/runtime`, \{ cache: "no-store" \}\)/,
  );
  assert.match(server, /workerRuntimeSummary/);
  assert.match(server, /worker deployment not created yet/);
  assert.match(server, /html \{[\s\S]*height: 100%;[\s\S]*overflow: hidden;/);
  assert.match(server, /body \{[\s\S]*height: 100%;[\s\S]*overflow: hidden;/);
  assert.match(server, /\.app \{[\s\S]*height: 100dvh;[\s\S]*overflow: hidden;/);
  assert.match(
    server,
    /\.sidebar \{[\s\S]*overflow: hidden auto;[\s\S]*overscroll-behavior: contain;/,
  );
  assert.match(
    server,
    /\.thread-list \{[\s\S]*overflow: auto;[\s\S]*overscroll-behavior: contain;/,
  );
  assert.match(server, /\.main \{[\s\S]*min-height: 0;[\s\S]*overflow: hidden;/);
  assert.match(server, /\.stream \{[\s\S]*overflow: auto;[\s\S]*overscroll-behavior: contain;/);
  assert.match(server, /\.thread-meta span \{[\s\S]*text-overflow: ellipsis;/);
  assert.match(
    server,
    /section id="thread-control-panel" class="panel prompt-panel" tabindex="0" aria-label="Thread control panel"/,
  );
  assert.match(server, /Thread Control/);
  assert.match(server, /viewing existing/);
  assert.match(server, /creating new/);
  assert.match(server, /button, select, input, textarea \{[\s\S]*max-width: 100%;/);
  assert.match(
    server,
    /\.prompt-panel \{[\s\S]*flex: 0 0 auto;[\s\S]*min-height: 154px;[\s\S]*max-height: none;[\s\S]*overflow: visible;[\s\S]*z-index: 1;/,
  );
  assert.match(server, /\.prompt-panel label,[\s\S]*\.field-wide \{[\s\S]*min-width: 0;/);
  assert.match(server, /\.prompt-actions,[\s\S]*\.status-line \{[\s\S]*margin-top: 12px;/);
  assert.match(server, /section id="response-stream-panel" class="panel stream-panel" tabindex="0" aria-label="Response stream panel"/);
  assert.match(server, /aside id="previous-tasks-panel" class="tasks-sidebar" tabindex="0" aria-label="Thread tasks sidebar"/);
  assert.match(server, /\.workspace-flow \{[\s\S]*display: flex;[\s\S]*flex-direction: column;[\s\S]*overflow: hidden;/);
  assert.match(
    server,
    /\.stream-panel > \.stream,[\s\S]*\.stream-panel > \.terminal-inline \{[\s\S]*flex: 1 1 auto;/,
  );
  assert.match(
    server,
    /@media \(max-width: 980px\) \{[\s\S]*\.app \{[\s\S]*grid-template-rows: minmax\(132px, 24dvh\) minmax\(0, 1fr\) minmax\(132px, 28dvh\);/,
  );
  assert.match(
    server,
    /@media \(max-width: 980px\) \{[\s\S]*\.main \{[\s\S]*overflow: hidden auto;[\s\S]*overscroll-behavior: contain;/,
  );
  assert.doesNotMatch(server, /html, body \{ overflow: auto; \}/);
  assert.doesNotMatch(server, /height: auto; overflow: visible/);
  assert.doesNotMatch(server, /div class="grid" style="margin-top: 14px"/);
  assert.match(readme, /serves `GET \/agents\/threads` as the thread-first chat\/task UI/);
  assert.match(restReadme, /GET \/api\/agents\/tasks\/:taskId\/events\?limit=250/);
  assert.match(restReadme, /POST \/api\/agents\/tasks\/:taskId\/feedback/);
});

test('rust thread chat dispatch keeps worker proxy transport errors server-side', async () => {
  const server = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');

  assert.match(
    server,
    /const route = `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/tasks`;/,
  );
  assert.match(restServer, /fn public_thread_worker_proxy_error\(action: &str\) -> String \{/);
  assert.match(restServer, /thread worker \{action\} failed; check remote REST API server logs/);
  assert.match(
    restServer,
    /async fn dispatch_thread_task\(\s*Path\(thread_id\): Path<String>,\s*Json\(request\): Json<DispatchTaskRequest>,\s*\) -> Response \{/s,
  );
  assert.match(restServer, /threadId path\/body mismatch/);
  assert.match(restServer, /THREAD_RUNTIME_CAPACITY_PRUNE_ENABLED/);
  assert.match(restServer, /THREAD_RUNTIME_MAX_AWAKE_DEPLOYMENTS/);
  assert.match(restServer, /async fn prune_awake_thread_workers_for_capacity/);
  assert.match(restServer, /labelSelector=app\.kubernetes\.io%2Fcomponent%3Dthread-pod/);
  assert.match(restServer, /"spec": \{ "replicas": 0 \}/);
  assert.match(
    restServer,
    /prune_awake_thread_workers_for_capacity\(&namespace, &name\)\.await/,
  );
  assert.match(restServer, /failed to persist remote task before worker wake/);
  assert.match(
    restServer,
    /remember_runtime_task\(&request, None\);[\s\S]*persist_runtime_task_to_postgres\(\s*&request,\s*None,\s*if queued_dispatch \{ "queued" \} else \{ "running" \},\s*\)\s*\.await[\s\S]*if queued_dispatch \{[\s\S]*publish_task_dispatch_to_nats\(&request, None, !container_pool_dispatch\)\.await[\s\S]*ensure_thread_worker\(&thread_id, &repo_config\.repo, &repo_config\.base_branch\)\.await/,
  );
  assert.match(restServer, /fn is_container_pool_dispatch_mode\(mode: &str\) -> bool/);
  assert.match(restServer, /"queued-pool" \| "nats-pool" \| "container-pool" \| "pool"/);
  assert.match(restServer, /StatusCode::ACCEPTED/);
  assert.match(restServer, /"directDispatch": false/);
  assert.match(restServer, /repo: String,/);
  assert.match(restServer, /base_branch: Option<String>,/);
  assert.match(restServer, /status: "running"\.to_string\(\),/);
  assert.match(restServer, /last_event_seq: -1,/);
  assert.match(restServer, /event_count: 0,/);
  assert.match(restServer, /latest_event_kind: Some\("dispatch"\.to_string\(\)\),/);
  assert.match(restServer, /latest_payload: None,/);
  assert.match(restServer, /failed to create or wake thread worker/);
  assert.match(restServer, /thread worker dispatch proxy failed: \{error\}/);
  assert.match(
    restServer,
    /Json\(json!\(\{ "error": public_thread_worker_proxy_error\("dispatch"\) \}\)\)/,
  );
  assert.doesNotMatch(restServer, /Json\(json!\(\{ "error": error\.to_string\(\) \}\)\)/);
  assert.match(restServer, /thread worker stream proxy failed: \{error\}/);
  assert.match(restServer, /public_thread_worker_proxy_error\("stream"\)/);
  assert.doesNotMatch(restServer, /format!\("stream proxy failed: \{error\}"\)/);
});

test('rust agent tasks page exposes runtime thread controls without collapsing admin archival semantics', async () => {
  const server = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');

  assert.match(
    server,
    /button id="thread-sleep" type="button" title="Reduce resources by scaling the thread container to zero" \{ "Pause\/Sleep" \}/,
  );
  assert.match(
    server,
    /button id="thread-archive" class="warn" type="button" title="Deep sleep: suspend the thread container" \{ "Archive" \}/,
  );
  assert.match(server, /button id="thread-delete" class="danger" type="button" \{ "Delete/);
  assert.match(server, /button id="thread-merge" type="button" \{ "Merge with upstream/);
  assert.match(
    server,
    /button id="thread-commit" type="button" title="Commit current worker changes and push the thread branch" \{ "Make commit" \}/,
  );
  assert.match(
    server,
    /button id="thread-terminal" type="button" title="Open a shell in the thread's Node\.js worker container" \{ "Terminal" \}/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/sleep`/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/archive`/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/hard-delete`/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/merge-upstream`/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/open-pr`/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/make-commit`/,
  );
  assert.match(
    server,
    /route: `\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/terminal`/,
  );
  assert.match(server, /const threadTerminalUrl = \(threadId\) => `\$\{threadIngressPrefix\(threadId\)\}\/terminal\?threadId=\$\{encodeURIComponent\(threadId\)\}`/);
  assert.match(server, /const trustedThreadTerminalUrl = \(threadId, candidate\) => \{/);
  assert.match(server, /parsed\.origin !== window\.location\.origin \|\| parsed\.pathname !== expectedPath \|\| returnedThreadId !== normalizeThreadId\(threadId\)/);
  assert.match(server, /ignored unsafe terminal URL from control response/);
  assert.match(server, /const threadTerminalUrlFromControlResponse = \(threadId, body\) => \{/);
  assert.match(server, /const targetUrl = threadTerminalUrlFromControlResponse\(threadId, textBody\)/);
  assert.match(server, /kind: "thread-control"/);
  assert.match(server, /action: config\.action/);
  assert.match(server, /openTaskWebSocket\(threadId, taskId\)/);
  assert.match(server, /const workerSockets = new Map\(\)/);
  assert.match(server, /const openWorkerWebSocket = \(threadId, taskId, attempt = 0\) =>/);
  assert.match(
    server,
    /\/ws\?threadId=\$\{encodeURIComponent\(threadId\)\}&taskId=\$\{encodeURIComponent\(taskId\)\}/,
  );
  assert.match(server, /renderStreamEvent\("message", event\.data, "worker-ws"\)/);
  assert.match(server, /openWorkerWebSocket\(threadId, taskId\)/);
  assert.match(
    server,
    /appendStreamLine\(`dispatch accepted \$\{adminPreview\("dispatch accepted response body", textBody\)\}`\);[\s\S]*openTaskStream\(threadId, taskId\);[\s\S]*openWorkerWebSocket\(threadId, taskId\);/,
  );
  assert.match(server, /setThreadRuntimeState\(threadId, "sleeping"/);
  assert.match(server, /button\.ok/);
  assert.match(server, /sleepingStatuses = new Set\(\["sleeping", "archived", "suspended"\]\)/);
  assert.match(server, /body: JSON\.stringify\(payload\)/);
  assert.match(restServer, /struct ThreadControlRequest/);
  assert.match(restServer, /validate_thread_control_signal/);
  assert.match(restServer, /"control payload kind must be thread-control"/);
  assert.match(restServer, /"threadId must be a UUID"/);
  assert.match(restServer, /"taskId must be a UUID"/);
  assert.match(server, /confirm: "Scale this thread runtime to zero replicas\?"/);
  assert.match(server, /confirm: "Archive this thread runtime\?"/);
  assert.match(server, /confirm: "Commit current worker changes and push this thread branch\?"/);
  assert.match(server, /confirm: "Open a terminal to this thread worker container\?"/);
  assert.match(
    server,
    /confirm: "Delete the Kubernetes runtime resources for this thread\? GitHub PRs are not deleted\."/,
  );
  assert.match(
    server,
    /appendStreamLine\(`\$\{config\.label\} failed \$\{response\.status\}: \$\{adminPreview\(`\$\{config\.label\} response body`, textBody\)\}`\);/,
  );
  assert.match(
    server,
    /appendStreamLine\(`\$\{config\.label\} accepted \$\{adminPreview\(`\$\{config\.label\} response body`, textBody\)\}`\);/,
  );
  assert.match(
    server,
    /\$\("thread-archive"\)\.addEventListener\("click", \(\) => \{\s*runThreadControl\("archive"\)/,
  );
  assert.doesNotMatch(
    server,
    /\/api\/admin\/remote-dev\/threads\/\$\{encodeURIComponent\(threadId\)\}\/end/,
  );

  assert.match(
    restServer,
    /async fn sleep_thread\(\s*Path\(thread_id\): Path<String>,\s*Json\(request\): Json<ThreadControlRequest>,\s*\) -> Response \{/,
  );
  assert.match(restServer, /validate_thread_control_signal\(&thread_id, "sleep", &request\)/);
  assert.match(
    restServer,
    /scale_thread_runtime\(thread_id, "sleep", 0, request\.task_id\.clone\(\)\)\.await/,
  );
  assert.match(
    restServer,
    /async fn archive_thread\(\s*Path\(thread_id\): Path<String>,\s*Json\(request\): Json<ThreadControlRequest>,\s*\) -> Response \{/,
  );
  assert.match(restServer, /validate_thread_control_signal\(&thread_id, "archive", &request\)/);
  assert.match(
    restServer,
    /scale_thread_runtime\(thread_id, "archive", 0, request\.task_id\.clone\(\)\)\.await/,
  );
  assert.match(
    restServer,
    /validate_thread_control_signal\(&thread_id, "hard-delete", &request\)/,
  );
  assert.match(
    restServer,
    /validate_thread_control_signal\(&thread_id, "merge-upstream", &request\)/,
  );
  assert.match(restServer, /validate_thread_control_signal\(&thread_id, "make-commit", &request\)/);
  assert.match(restServer, /validate_thread_control_signal\(&thread_id, "terminal", &request\)/);
  assert.match(restServer, /validate_thread_control_signal\(&thread_id, "open-pr", &request\)/);
  assert.match(restServer, /"action": "merge-upstream"/);
  assert.match(restServer, /"action": "make-commit"/);
  assert.match(restServer, /"action": "open-pr"/);
  assert.match(restServer, /thread_terminal_url/);
  assert.match(restServer, /"terminalUrl": terminal_url/);
  assert.match(restServer, /publish_thread_runtime_event_to_nats/);
  assert.match(restServer, /"kind": "thread-runtime"/);
  assert.match(restServer, /"status": status/);
  assert.match(restServer, /"waking"/);
  assert.match(restServer, /"awake"/);
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/archive",\s*post\(archive_thread\),?\s*\)/,
  );
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/runtime",\s*get\(thread_runtime\),?\s*\)/,
  );
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/open-pr",\s*post\(open_pr_thread\),?\s*\)/,
  );
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/make-commit",\s*post\(make_commit_thread\),?\s*\)/,
  );
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/terminal",\s*post\(terminal_thread\),?\s*\)/,
  );
});

test('prometheus is configured for gateway subpath hosting', async () => {
  const deployment = await readRepoFile('remote/argocd/observability/prometheus.deployment.yaml');

  assert.match(deployment, /--web\.external-url=http:\/\/54\.91\.17\.58\/prometheus\//);
  assert.match(deployment, /--web\.route-prefix=\//);
});

test('gleam websocket homepage honors the gateway prefix', async () => {
  const server = await readRepoFile(
    'remote/deployments/gleamlang-server/src/gleamlang_server/http_server.gleam',
  );

  assert.match(server, /startsWith\('\/gleam\/'\)/);
  assert.match(server, /wsPath=prefix\+'\/ws'/);
});

test('claude sdk runner pins a runnable native executable before dispatch', async () => {
  const runner = await readRepoFile('remote/deployments/dev-server/src/agents/claude-sdk.ts');
  const selector = await readRepoFile('remote/deployments/dev-server/src/agents/index.ts');

  assert.match(runner, /resolveClaudeCodeExecutable/);
  assert.match(runner, /function isRunnableExecutable\(executable: string\): boolean/);
  assert.match(runner, /accessSync\(executable, constants\.X_OK\)/);
  assert.match(
    runner,
    /return isRunnableExecutable\(process\.env\.CLAUDE_CODE_EXECUTABLE\)[\s\S]*\? process\.env\.CLAUDE_CODE_EXECUTABLE[\s\S]*: undefined;/,
  );
  assert.match(runner, /'@anthropic-ai\/claude-agent-sdk-linux-x64'/);
  assert.match(runner, /'@anthropic-ai\/claude-agent-sdk-linux-x64-musl'/);
  assert.match(runner, /pathToClaudeCodeExecutable: claudeExecutable/);
  assert.match(runner, /kind: 'stderr'/);
  assert.match(runner, /process\.getuid/);
  assert.match(runner, /\? 'default'\s*: 'bypassPermissions'/);
  assert.match(runner, /model: opts\.env\.ANTHROPIC_MODEL/);
  assert.match(selector, /hasClaudeSdkExecutable/);
  assert.match(selector, /Claude SDK native executable not found or not executable/);
});

test('node worker image is baked with git/ssh and runs as the node user', async () => {
  const dockerfile = await readRepoFile('remote/deployments/dev-server/Dockerfile');
  const bootstrapDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');

  assert.match(dockerfile, /apt-get install -y --no-install-recommends[\s\S]*git openssh-client/);
  assert.match(dockerfile, /USER node/);
  assert.match(dockerfile, /ENV HOME=\/home\/node/);
  assert.match(dockerfile, /WORKSPACE_REPO=\/home\/node\/workspace\/repo/);
  assert.match(dockerfile, /GH_DEPLOY_KEY_PATH=\/home\/node\/\.ssh\/id_ed25519/);
  assert.match(dockerfile, /git clone --depth 50 --branch "\$DD_REPO_REF"/);
  assert.match(bootstrapDeployment, /image: docker\.io\/library\/dd-dev-server:dev/);
  assert.match(bootstrapDeployment, /EVENT_INGEST_URL[\s\S]*\/api\/agents\/events/);
  assert.match(bootstrapDeployment, /EVENT_INGEST_SECRET[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(bootstrapDeployment, /runAsUser: 1000/);
  assert.match(bootstrapDeployment, /initContainers:/);
  assert.doesNotMatch(bootstrapDeployment, /apt-get update/);
  assert.doesNotMatch(bootstrapDeployment, /node:22-bookworm-slim/);
  assert.doesNotMatch(bootstrapDeployment, /\/opt\/dd-next-1/);
  assert.match(restServer, /fn thread_runtime_image\(\) -> String/);
  assert.match(restServer, /docker\.io\/library\/dd-dev-server:dev/);
  assert.match(restServer, /"initContainers"/);
  assert.match(restServer, /"runAsUser": 1000/);
  assert.match(restServer, /"IDLE_TIMEOUT_MS", "value": "0"/);
  assert.match(
    restServer,
    /"EVENT_INGEST_URL", "value": "http:\/\/dd-remote-rest-api\.default\.svc\.cluster\.local:8082\/api\/agents\/events"/,
  );
  assert.match(restServer, /struct AgentEventIngestRequest/);
  assert.match(restServer, /\.route\("\/api\/agents\/events", post\(ingest_agent_event\)\)/);
  assert.match(restServer, /record_request\("POST", "\/api\/agents\/events", StatusCode::OK\)/);
  assert.match(restServer, /if !authorized_internal_request\(&headers\) \{/);
  assert.match(restServer, /Json\(json!\(\{ "error": "event\.kind is required" \}\)\)/);
  assert.match(restServer, /persist_agent_event_to_postgres/);
  assert.doesNotMatch(restServer, /apt-get update/);
  assert.doesNotMatch(restServer, /node:22-bookworm-slim/);
  assert.doesNotMatch(restServer, /\/root\/\.ssh/);
  assert.match(restServer, /Some\(json!\(\{ "spec": deployment\["spec"\]\.clone\(\) \}\)\)/);
});

test('node worker opens draft PRs only through explicit control action', async () => {
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');
  const webHome = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  assert.match(server, /POST \/thread\/open-pr/);
  assert.match(server, /fastify\.post\('\/thread\/open-pr'/);
  assert.match(server, /const OpenPullRequestSchema = z\.object/);
  assert.match(server, /async function openPullRequestForThread/);
  assert.match(server, /async function ensurePullRequestForSession/);
  assert.match(server, /'--draft'/);
  assert.match(server, /\['rev-list', '--left-right', '--count', `origin\/\$\{config\.baseBranch\}\.\.\.HEAD`\]/);
  assert.match(server, /'--allow-empty'/);
  assert.match(server, /kind: 'open-pr-marker-commit'/);
  assert.match(
    server,
    /const title = rawTitle\.startsWith\('WIP - '\) \? rawTitle : `WIP - \$\{rawTitle\}`/,
  );
  assert.match(server, /'WIP'/);
  assert.match(server, /kind: 'pr_open'/);
  assert.doesNotMatch(server, /status: 'opening-pr'/);
  assert.doesNotMatch(server, /const prUrl = await ensurePullRequest/);
  assert.match(restServer, /validate_thread_control_signal\(&thread_id, "open-pr", &request\)/);
  assert.match(restServer, /open_thread_pr\(thread_id, request\)\.await/);
  assert.match(webHome, /Open draft PR/);
  assert.match(webHome, /\/open-pr`/);
});

test('node worker exposes manual commit and terminal controls for pinned threads', async () => {
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  const dockerfile = await readRepoFile('remote/deployments/dev-server/Dockerfile');

  assert.match(server, /POST \/thread\/make-commit/);
  assert.match(server, /GET  \/terminal/);
  assert.match(server, /fastify\.post\('\/thread\/make-commit'/);
  assert.match(server, /fastify\.get\('\/terminal'/);
  assert.match(server, /requestUrl\.pathname !== '\/ws' && requestUrl\.pathname !== '\/terminal\/ws'/);
  assert.match(server, /class TerminalWebSocketClient/);
  assert.match(server, /terminalPageHtml/);
  assert.match(server, /async function makeCommitForThread/);
  assert.match(server, /\['commit', '--no-verify', '-m', manualCommitMessage/);
  assert.match(server, /\['push', '--no-verify', '--set-upstream', 'origin', session\.branch\]/);
  assert.match(server, /@xterm\/xterm/);
  assert.match(server, /TERMINAL_SCRIPT_BIN/);
  assert.match(server, /transport: usePty \? 'pty-script' : 'pipe-fallback'/);
  assert.match(server, /type: 'terminal-output'/);
  assert.match(server, /spawn\(scriptBin, \['-q', '-f', '-e', '-c', terminalShellCommand\(shell\), '\/dev\/null'\]/);
  assert.match(dockerfile, /util-linux/);
});
