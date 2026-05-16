import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/web-home-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust homepage lists public task paths and protected ops paths', async () => {
  const home = await readRepoFile('remote/web-home-rs/src/main.rs');

  assert.match(home, /dd remote service directory/);
  assert.match(
    home,
    /<code>\/<\/code>, <code>\/home<\/code>, <code>\/agents\/tasks<\/code>, <code>\/agents\/threads<\/code>, <code>\/api\/agents\/tasks<\/code>, and <code>\/webrtc\/<\/code> are open\. Authenticated entries include <code>\/lambdas\/functions<\/code>, <code>\/lambdas\/invoke\/&lt;function-id&gt;<\/code>, and <code>\/scrape<\/code>; ops paths stay behind internal gateway access\./,
  );
  assert.match(home, /<h2>Deployments<\/h2>/);
  assert.match(home, /<code>dd-web-scraper<\/code>/);
  assert.match(home, /<code>dd-web-scraper:8097<\/code>/);
  assert.match(home, /SCRAPER_PARSER_WORKERS=2/);
  assert.match(home, /<code>dd-gleam-lambda-runner<\/code>/);
  assert.match(home, /<code>dd-gleam-lambda-runner:8083<\/code>/);
  assert.match(home, /dd-gleam-lambda-runner-secrets/);
  assert.doesNotMatch(home, /Auth: [^<"]+/);
  assert.doesNotMatch(home, /Auth header/);
  assert.match(home, /Node\.js Coding Agent Task Manager/);
  assert.doesNotMatch(home, /Node control-plane API/);
  assert.match(
    home,
    /href="\/tasks"><code>\/tasks<\/code><\/a><a href="\/status"><code>\/status<\/code><\/a><a href="\/stream\/example-task-id"><code>\/stream\/&lt;uuid&gt;<\/code><\/a>/,
  );
  assert.match(
    home,
    /href="\/"><code>\/<\/code><\/a><a href="\/home"><code>\/home<\/code><\/a><a href="\/agents\/tasks"><code>\/agents\/tasks<\/code><\/a><a href="\/agents\/threads"><code>\/agents\/threads<\/code><\/a>/,
  );
  assert.match(home, /href="\/"><code>\/<\/code><\/a><a href="\/home"><code>\/home<\/code><\/a>/);
  assert.match(home, /rejects requests for the wrong pinned thread/);
  assert.match(home, /Kubernetes Ingress selects the UUID-bound worker Service/);
  assert.match(home, /Kubernetes per-thread Ingress/);
  assert.match(home, /\/dd-thread\/&lt;short&gt;/);
  assert.match(home, /href="\/dd-thread\/example"/);
  assert.match(home, /\/dd-thread\/&lt;short&gt;\/tasks/);
  assert.match(home, /href="\/dd-thread\/example\/tasks"/);
  assert.match(home, /Ingress selects the UUID-bound worker Service/);
  assert.doesNotMatch(home, /routes by thread UUID\/taskId/);
  assert.match(home, /href="\/agents\/tasks"/);
  assert.match(home, /href="\/agents\/threads"/);
  assert.match(home, /Rust web homepage deployment/);
  assert.match(home, /Service directory plus cluster-served task\/thread\/PR UI/);
  assert.match(home, /stored events/);
  assert.match(home, /href="\/api\/agents\/tasks"/);
  assert.match(home, /href="\/api\/agents\/threads\/example-thread-id\/context"/);
  assert.match(home, /href="\/lambdas\/functions"/);
  assert.match(home, /href="\/api\/lambdas\/functions"/);
  assert.match(home, /href="\/lambdas\/invoke\/00000000-0000-0000-0000-000000000000"/);
  assert.match(home, /dd-gleam-lambda-runner deployment \+ Rust REST API/);
  assert.match(home, /Gleam child-process runner/);
  assert.match(home, /href="\/auth\?return=\/home"/);
  assert.match(home, /Rust PIN auth service/);
  assert.match(home, /dd_auth/);
  assert.match(home, /option value="echo" \{ "echo" \}/);
  assert.match(home, /Queue Consumer/);
  assert.match(home, /Rust NATS shadow preparer \(dd-remote-queue-consumer\)/);
  assert.match(home, /Rust NATS Queue Consumer/);
  assert.match(home, /dd\.remote\.thread\.\*\.tasks/);
  assert.match(home, /href="\/api\/agents\/threads\/example-thread-id\/prepare"/);
  assert.match(home, /dd-remote-thread-preparer/);
  assert.match(home, /It does not execute prompts/);
  assert.match(home, /href="\/gleam\/home"/);
  assert.match(home, /href="\/gleam\/healthz"/);
  assert.match(home, /href="\/gleam\/metrics"/);
  assert.match(home, /href="\/gleam\/ws"/);
  assert.match(home, /wss:\/\/54\.91\.17\.58\/gleam\/ws/);
  assert.doesNotMatch(home, /ws:\/\/54\.91\.17\.58\/gleam\/ws/);
  assert.match(home, /href="\/mcp"/);
  assert.match(home, /href="\/mcp\/home"/);
  assert.match(home, /href="\/mcp\/healthz"/);
  assert.match(home, /href="\/mcp\/metrics"/);
  assert.match(home, /Gleam MCP service/);
  assert.match(home, /href="\/webrtc\/"/);
  assert.match(home, /href="\/webrtc\/healthz"/);
  assert.match(home, /href="\/webrtc\/metrics"/);
  assert.match(home, /wss:\/\/54\.91\.17\.58\/webrtc\/signal/);
  assert.match(home, /Rust WebRTC signaling service/);
  assert.match(home, /Media and data channels stay peer-to-peer/);
  assert.match(home, /href="\/scrape"/);
  assert.match(home, /href="\/scrape\/healthz"/);
  assert.match(home, /Playwright, Puppeteer, and Browserless scraping/);
  assert.doesNotMatch(home, /href="\/scraper/);
  assert.match(home, /href="\/telemetry\/"/);
  assert.match(home, /href="\/prometheus\/"/);
  assert.match(home, /href="\/nats\/"/);
  assert.match(home, /href="\/nats-metrics\/metrics"/);
  assert.match(home, /href="\/reaper\/"/);
  assert.match(home, /href="\/cron\/"/);
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
  const authServer = await readRepoFile('remote/auth-server-rs/src/main.rs');
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
  const lambdaDeployment = await readRepoFile(
    'remote/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.deployment.yaml',
  );
  const lambdaService = await readRepoFile(
    'remote/gleam-lambda-runner/k8s/ec2/dd-gleam-lambda-runner.service.yaml',
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
    /location\s+\/api\/agents\/[\s\S]*dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
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
  assert.match(webrtcDeployment, /name:\s*dd-webrtc-signaling/);
  assert.match(webrtcDeployment, /cd \/opt\/dd-next-1\/remote\/webrtc-signaling-rs/);
  assert.match(webrtcDeployment, /containerPort:\s*8095/);
  assert.match(webrtcService, /name:\s*dd-webrtc-signaling/);
  assert.match(webrtcService, /port:\s*8095/);
  assert.match(scraperDeployment, /name:\s*dd-web-scraper/);
  assert.match(scraperDeployment, /cd \/opt\/dd-next-1\/remote\/web-scraper-service/);
  assert.match(scraperDeployment, /containerPort:\s*8097/);
  assert.match(scraperService, /name:\s*dd-web-scraper/);
  assert.match(scraperService, /port:\s*8097/);
  assert.match(lambdaDeployment, /name:\s*dd-gleam-lambda-runner/);
  assert.match(lambdaDeployment, /cd \/opt\/dd-next-1\/remote\/gleam-lambda-runner/);
  assert.match(lambdaDeployment, /containerPort:\s*8083/);
  assert.match(lambdaService, /name:\s*dd-gleam-lambda-runner/);
  assert.match(lambdaService, /port:\s*8083/);
  assert.match(lambdaApp, /path:\s*remote\/gleam-lambda-runner\/k8s\/ec2/);
  assert.match(authDeployment, /name:\s*dd-remote-auth/);
  assert.match(
    authDeployment,
    /name:\s*DD_AUTH_PIN[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-remote-auth-secrets[\s\S]*key:\s*DD_AUTH_PIN/,
  );
  assert.match(
    authDeployment,
    /name:\s*DD_AUTH_COOKIE_VALUE[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-remote-auth-secrets[\s\S]*key:\s*DD_AUTH_COOKIE_VALUE/,
  );
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
  assert.match(runtimeReadme, /HTTP remains enabled for[\s\S]*bootstrap access/);
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
  const server = await readRepoFile('remote/web-home-rs/src/main.rs');
  const cargo = await readRepoFile('remote/web-home-rs/Cargo.toml');
  const readme = await readRepoFile('remote/web-home-rs/readme.md');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-web-home.deployment.yaml',
  );
  const dockerfile = await readRepoFile('remote/web-home-rs/Dockerfile');
  const deployWorkflow = await readRepoFile('.github/workflows/deploy-remote-web-home-ssm.yml');
  const restDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.deployment.yaml',
  );
  const restService = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.service.yaml',
  );
  const restRbac = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api-rbac.yaml',
  );
  const restReadme = await readRepoFile('remote/rest-api-rs/readme.md');
  const restServer = await readRepoFile('remote/rest-api-rs/src/main.rs');

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
  assert.match(deployWorkflow, /name: Deploy remote web-home image runtime/);
  assert.match(deployWorkflow, /workflow_dispatch:/);
  assert.match(deployWorkflow, /group: deploy-remote-web-home-ssm/);
  assert.match(deployWorkflow, /ref: dev/);
  assert.match(deployWorkflow, /role-to-assume: \$\{\{ secrets\.AWS_ROLE_TO_ASSUME \}\}/);
  assert.match(
    deployWorkflow,
    /nerdctl -n k8s\.io build --progress=plain[\s\S]*-t docker\.io\/library\/dd-remote-web-home:dev remote\/web-home-rs/,
  );
  assert.match(
    deployWorkflow,
    /kubectl apply -f remote\/argocd\/dd-next-runtime\/dd-idle-reaper\.configmap\.yaml/,
  );
  assert.match(
    deployWorkflow,
    /kubectl apply -f remote\/argocd\/dd-next-runtime\/dd-remote-web-home\.deployment\.yaml/,
  );
  assert.match(
    deployWorkflow,
    /kubectl rollout status deployment\/dd-remote-web-home -n default --timeout=300s/,
  );
  assert.match(deployWorkflow, /aws ssm send-command/);
  assert.match(deployWorkflow, /aws ssm get-command-invocation/);
  assert.doesNotMatch(deployment, /REMOTE_REST_API_URL/);
  assert.doesNotMatch(deployment, /dd-remote-web-home-secrets/);
  assert.match(restDeployment, /name:\s*dd-agent-secrets[\s\S]*optional:\s*true/);
  assert.match(restDeployment, /name:\s*dd-remote-rest-api-secrets[\s\S]*optional:\s*true/);
  assert.match(restDeployment, /name:\s*THREAD_RUNTIME_NAMESPACE[\s\S]*value:\s*default/);
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
    /insert into agent_remote_dev_tasks[\s\S]*\(id, thread_id, user_id, docker_task_id, prompt, status, branch, last_event_seq, is_soft_deleted, started_at, created_at, updated_at, created_by, updated_by\)/,
  );
  assert.match(restServer, /updated_by = excluded\.updated_by/);
  assert.match(restServer, /fn public_data_source_error\(source: &str\) -> String \{/);
  assert.match(restServer, /fn thread_runtime_namespace\(\) -> String \{/);
  assert.match(
    restServer,
    /env::var\("THREAD_RUNTIME_NAMESPACE"\)\.unwrap_or_else\(\|_\| "default"\.to_string\(\)\)/,
  );
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
  const server = await readRepoFile('remote/web-home-rs/src/main.rs');

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
  const server = await readRepoFile('remote/web-home-rs/src/main.rs');
  const readme = await readRepoFile('remote/web-home-rs/readme.md');
  const restReadme = await readRepoFile('remote/rest-api-rs/readme.md');
  const threadsJs = server.slice(
    server.indexOf('const AGENTS_THREADS_JS'),
    server.indexOf('const AGENTS_TASKS_CSS'),
  );

  assert.match(server, /async fn agents_threads_page\(\) -> impl IntoResponse/);
  assert.match(server, /use maud::\{html, Markup, DOCTYPE\}/);
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
    /\.prompt-panel \{[\s\S]*flex: var\(--control-share\) 1 0;[\s\S]*overflow: hidden auto;/,
  );
  assert.match(server, /\.main\.control-wide \{[\s\S]*--control-share: 1\.2;[\s\S]*--lower-share: 0\.8;/);
  assert.match(server, /\.main\.lower-wide \{[\s\S]*--control-share: 0\.8;[\s\S]*--lower-share: 1\.2;/);
  assert.match(server, /\.prompt-panel label,[\s\S]*\.field-wide \{[\s\S]*min-width: 0;/);
  assert.match(server, /\.prompt-actions,[\s\S]*\.status-line \{[\s\S]*margin-top: 12px;/);
  assert.match(server, /div id="task-stream-grid" class="grid task-stream-grid"/);
  assert.match(server, /\.task-stream-grid \{[\s\S]*margin-top: 6px;/);
  assert.match(server, /\.task-stream-grid\.tasks-wide \{[\s\S]*grid-template-columns: minmax\(0, 1\.02fr\) minmax\(0, 0\.98fr\);/);
  assert.match(server, /\.task-stream-grid\.stream-wide \{[\s\S]*grid-template-columns: minmax\(0, 0\.62fr\) minmax\(0, 1\.38fr\);/);
  assert.match(server, /function setWorkspaceLayout\(mode\) \{/);
  assert.match(server, /\$\("thread-control-panel"\)\.addEventListener\("click", \(\) => setWorkspaceLayout\("control"\)\)/);
  assert.match(server, /function setTaskStreamLayout\(mode\) \{/);
  assert.match(server, /\$\("previous-tasks-panel"\)\.addEventListener\("click", \(\) => setTaskStreamLayout\("tasks"\)\)/);
  assert.match(server, /\$\("response-stream-panel"\)\.addEventListener\("click", \(\) => setTaskStreamLayout\("stream"\)\)/);
  assert.doesNotMatch(server, /div class="grid" style="margin-top: 14px"/);
  assert.match(server, /\.route\("\/agents\/threads", get\(agents_threads_page\)\)/);
  assert.match(server, /\.route\("\/agents\/threads\/", get\(agents_threads_page\)\)/);
  assert.match(server, /Agent threads/);
  assert.match(server, /id="thread-list"/);
  assert.match(server, /select id="repo-url"/);
  assert.match(server, /input id="repo-url-new"/);
  assert.match(server, /New repo URL\.\.\./);
  assert.match(server, /id="task-list"/);
  assert.match(server, /id="stream"/);
  assert.match(server, /Response stream/);
  assert.match(server, /Previous tasks/);
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
  assert.match(server, /Delete \(Delete Container\)/);
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
  assert.match(server, /if \(terminalTargetUrl\) openInlineTerminal\(terminalTargetUrl\);/);
  assert.doesNotMatch(threadsJs, /window\.open\(/);
  assert.doesNotMatch(threadsJs, /terminalWindow/);
  assert.match(server, /sendFeedback\(seq, vote, button\)/);
  assert.match(server, /collectText\(raw\)/);
  assert.match(server, /Creating or waking the UUID-bound worker/);
  assert.match(server, /dispatch still waiting after/);
  assert.match(server, /dispatch accepted/);
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
    /\.prompt-panel \{[\s\S]*flex: var\(--control-share\) 1 0;[\s\S]*overflow: hidden auto;[\s\S]*z-index: 1;/,
  );
  assert.match(server, /\.prompt-panel label,[\s\S]*\.field-wide \{[\s\S]*min-width: 0;/);
  assert.match(server, /\.prompt-actions,[\s\S]*\.status-line \{[\s\S]*margin-top: 12px;/);
  assert.match(server, /div id="task-stream-grid" class="grid task-stream-grid"/);
  assert.match(server, /\.task-stream-grid \{[\s\S]*margin-top: 6px;/);
  assert.match(server, /\.main > \.grid \{[\s\S]*flex: var\(--lower-share\) 1 0;/);
  assert.match(
    server,
    /\.grid > \.panel > \.task-list,[\s\S]*\.grid > \.panel > \.stream,[\s\S]*\.grid > \.panel > \.terminal-inline \{[\s\S]*flex: 1 1 auto;/,
  );
  assert.match(
    server,
    /@media \(max-width: 980px\) \{[\s\S]*\.app \{[\s\S]*grid-template-rows: minmax\(150px, 28dvh\) minmax\(0, 1fr\);/,
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
  const server = await readRepoFile('remote/web-home-rs/src/main.rs');
  const restServer = await readRepoFile('remote/rest-api-rs/src/main.rs');

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
  assert.match(restServer, /failed to persist remote task before worker wake/);
  assert.match(
    restServer,
    /remember_runtime_task\(&request, None\);[\s\S]*persist_runtime_task_to_postgres\(&request, None\)\.await[\s\S]*ensure_thread_worker\(&thread_id, &repo_config\.repo, &repo_config\.base_branch\)\.await/,
  );
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
  const server = await readRepoFile('remote/web-home-rs/src/main.rs');
  const restServer = await readRepoFile('remote/rest-api-rs/src/main.rs');

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
    /appendStreamLine\(`dispatch accepted \$\{textBody\.slice\(0, 500\)\}`\);[\s\S]*openTaskStream\(threadId, taskId\);[\s\S]*openWorkerWebSocket\(threadId, taskId\);/,
  );
  assert.match(server, /setThreadRuntimeState\(threadId, "sleeping"/);
  assert.match(server, /button\.ok/);
  assert.match(server, /sleepingStatuses = new Set\(\["sleeping", "archived", "suspended"\]\)/);
  assert.match(server, /body: JSON\.stringify\(payload\)/);
  assert.match(restServer, /struct ThreadControlRequest/);
  assert.match(restServer, /validate_thread_control_signal/);
  assert.match(restServer, /"control payload kind must be thread-control"/);
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
    /appendStreamLine\(`\$\{config\.label\} failed \$\{response\.status\}: \$\{textBody\.slice\(0, 500\)\}`\);/,
  );
  assert.match(
    server,
    /appendStreamLine\(`\$\{config\.label\} accepted \$\{textBody\.slice\(0, 500\)\}`\);/,
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
    'remote/gleamlang-server/src/gleamlang_server/http_server.gleam',
  );

  assert.match(server, /startsWith\('\/gleam\/'\)/);
  assert.match(server, /wsPath=prefix\+'\/ws'/);
});

test('claude sdk runner pins a runnable native executable before dispatch', async () => {
  const runner = await readRepoFile('remote/dev-server/src/agents/claude-sdk.ts');
  const selector = await readRepoFile('remote/dev-server/src/agents/index.ts');

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
  const dockerfile = await readRepoFile('remote/dev-server/Dockerfile');
  const bootstrapDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );
  const restServer = await readRepoFile('remote/rest-api-rs/src/main.rs');

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
  const server = await readRepoFile('remote/dev-server/src/server.ts');
  const restServer = await readRepoFile('remote/rest-api-rs/src/main.rs');
  const webHome = await readRepoFile('remote/web-home-rs/src/main.rs');

  assert.match(server, /POST \/thread\/open-pr/);
  assert.match(server, /fastify\.post\('\/thread\/open-pr'/);
  assert.match(server, /const OpenPullRequestSchema = z\.object/);
  assert.match(server, /async function openPullRequestForThread/);
  assert.match(server, /async function ensurePullRequestForSession/);
  assert.match(server, /'--draft'/);
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
  const server = await readRepoFile('remote/dev-server/src/server.ts');

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
  assert.match(server, /type: 'terminal-output'/);
  assert.match(server, /spawn\(shell, \['-i'\]/);
});
