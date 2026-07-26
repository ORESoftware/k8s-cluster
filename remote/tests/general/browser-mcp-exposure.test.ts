import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

// Ratchets on the service that is reachable from the public internet with a
// write-capable browser tool. Each assertion encodes a production defect found
// on 2026-07-25.
//
// ChatGPT custom MCP apps cannot supply an operator's arbitrary static bearer.
// This deployment therefore implements OAuth discovery, dynamic public-client
// registration, PKCE, scoped/audience-bound access tokens, and rotating refresh
// grants while retaining the domain ceiling and browser safety controls.
//
// These are cheap file assertions on purpose: they must fail in CI *before*
// anything reaches a cluster.

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/apps/dd-next-runtime.application.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const DEPLOYMENT = 'remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.deployment.yaml';
const WORKER_DEPLOYMENT = 'remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml';
const GATEWAY = 'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml';
const AWS_APPS = 'remote/argocd/clusters/aws/applications.yaml';
const HETZNER_APPS = 'remote/argocd/clusters/hetzner/applications.yaml';
const CLI_FLAGS = 'remote/deployments/browser-mcp-rs/.cli-flags.toml';
const FIDUCIA_PORTAL_DOMAINS = [
  'confluent.cloud',
  'confluent.io',
  'signoz.io',
  'tailscale.com',
  'planetscale.com',
  'clerk.com',
  'algolia.com',
  'app.posthog.com',
  'elevenlabs.io',
  'www.together.ai',
  'support.snyk.io',
  'us.ovhcloud.com',
  'www.pulumi.com',
  'tally.so',
  'allthingsopen.org',
  'allthingsopen.wufoo.com',
  'static.wufoo.com',
  'talks.devopsdays.org',
  'sessionize.com',
  'events.linuxfoundation.org',
  'cfp.awscommunitydaysoflo.com',
  'forms.gle',
  'docs.google.com',
  'www.gstatic.com',
  'ssl.gstatic.com',
  'fonts.googleapis.com',
  'fonts.gstatic.com',
];

function readDeployment(): string {
  const path = resolve(repoRoot, DEPLOYMENT);
  if (!existsSync(path)) {
    // The service lives in a submodule; skip rather than fail a checkout that
    // has not initialised it.
    return '';
  }
  return readFileSync(path, 'utf8');
}

// Pull the literal that follows an env var name, e.g.
//   - name: BROWSER_MCP_ALLOWED_DOMAINS
//     value: 'a,b'
function envValue(manifest: string, name: string): string | null {
  const re = new RegExp(`- name:\\s*${name}\\s*\\n\\s*value:\\s*(.*)`, 'm');
  const m = manifest.match(re);
  if (!m) return null;
  return m[1].trim().replace(/^['"]|['"]$/g, '');
}

function nginxLocation(source: string, declaration: string): string {
  const marker = `      location ${declaration} {`;
  const start = source.indexOf(marker);
  assert.notEqual(start, -1, `missing nginx location: ${declaration}`);
  const end = source.indexOf('\n      }', start);
  assert.notEqual(end, -1, `unterminated nginx location: ${declaration}`);
  return source.slice(start, end);
}

test('OAuth browser-mcp has a reviewed, hostname-only domain ceiling', () => {
  const manifest = readDeployment();
  if (!manifest) return;

  const value = envValue(manifest, 'BROWSER_MCP_ALLOWED_DOMAINS');
  assert.notEqual(value, null, `${DEPLOYMENT} must declare BROWSER_MCP_ALLOWED_DOMAINS`);
  assert.notEqual(
    value,
    '',
    'BROWSER_MCP_ALLOWED_DOMAINS is empty — the server treats that as "no allowlist" and will ' +
      'navigate to ANY public https host. Name the hosts explicitly.',
  );
  assert.ok(
    (value as string).split(',').every((d) => d.trim().length > 0),
    `BROWSER_MCP_ALLOWED_DOMAINS has an empty entry: ${value}`,
  );
  assert.equal(
    envValue(manifest, 'BROWSER_MCP_REQUIRE_AUTH'),
    'true',
    'The public write-capable browser MCP must require OAuth.',
  );

  const domains = (value as string).split(',').map((domain) => domain.trim());
  assert.deepEqual(
    domains,
    FIDUCIA_PORTAL_DOMAINS,
    'The OAuth production endpoint must contain only the reviewed Fiducia portal profile.',
  );
  assert.ok(
    domains.every(
      (domain) =>
        domain !== '*' &&
        !domain.includes('/') &&
        !domain.includes(':') &&
        /^[a-z0-9.-]+$/.test(domain),
    ),
    `BROWSER_MCP_ALLOWED_DOMAINS must contain hostnames only: ${value}`,
  );
  assert.ok(
    ['irs.gov', 'sos.state.co.us', 'dnb.com'].every((domain) => !domains.includes(domain)),
    'Filing and identity-registration sites must not be exposed by the Fiducia portal profile.',
  );
  assert.ok(
    [
      'gmail.com',
      'mail.google.com',
      'accounts.google.com',
      'outlook.com',
      'login.microsoftonline.com',
    ].every((domain) => !domains.includes(domain)),
    'Webmail and identity-provider login hosts must never be exposed by this browser profile.',
  );

  const worker = readFileSync(resolve(repoRoot, WORKER_DEPLOYMENT), 'utf8');
  const workerValue = envValue(worker, 'BROWSER_AGENT_ALLOWED_DOMAINS');
  assert.equal(
    workerValue,
    value,
    'The MCP and Playwright worker hostname ceilings must stay byte-for-byte aligned.',
  );
});

test('browser-mcp OAuth and worker credentials are sourced from Secrets', () => {
  const manifest = readDeployment();
  if (!manifest) return;

  for (const secret of [
    'BROWSER_MCP_OAUTH_SIGNING_SECRET',
    'BROWSER_MCP_OAUTH_OPERATOR_SECRET',
  ]) {
    assert.match(
      manifest,
      new RegExp(`${secret}\\s*\\n\\s*valueFrom:\\s*\\n\\s*secretKeyRef:`),
      `${secret} must come from a secretKeyRef, not a literal value.`,
    );
  }
  assert.doesNotMatch(
    manifest,
    /BROWSER_MCP_AUTH_SECRET/,
    'The obsolete static MCP bearer must not remain in the OAuth deployment.',
  );
  assert.match(
    manifest,
    /SERVER_AUTH_SECRET\s*\n\s*valueFrom:\s*\n\s*secretKeyRef:\s*\n\s*name:\s*dd-agent-secrets\s*\n\s*key:\s*SERVER_AUTH_SECRET/,
    'The private worker credential must come from the shared Kubernetes Secret.',
  );
  assert.doesNotMatch(
    manifest,
    /SERVER_AUTH_SECRET\s*\n\s*valueFrom:[\s\S]{0,180}optional:\s*true/,
    'The private worker credential is required; making it optional creates false-ready pods.',
  );
});

test('browser-mcp is a prebuilt, non-root container without shared source mounts', () => {
  const manifest = readDeployment();
  if (!manifest) return;

  assert.match(
    manifest,
    /image:\s*ghcr\.io\/oresoftware\/dd-browser-mcp-rs@sha256:[a-f0-9]{64}/,
    'The public MCP must run an immutable dedicated image, not a mutable branch tag.',
  );
  assert.match(manifest, /replicas:\s*2/);
  assert.match(manifest, /readOnlyRootFilesystem:\s*true/);
  assert.match(manifest, /automountServiceAccountToken:\s*false/);
  assert.match(manifest, /startupProbe:[\s\S]*path:\s*\/healthz/);
  assert.match(manifest, /readinessProbe:[\s\S]*path:\s*\/readyz/);
  assert.doesNotMatch(manifest, /hostPath:/);
  assert.doesNotMatch(manifest, /cargo run/);
  assert.doesNotMatch(manifest, /rust:[0-9]/);
});

test('browser-mcp is registered in both cluster profiles', () => {
  const aws = readFileSync(resolve(repoRoot, AWS_APPS), 'utf8');
  const hetzner = readFileSync(resolve(repoRoot, HETZNER_APPS), 'utf8');

  for (const [name, source] of [
    [AWS_APPS, aws],
    [HETZNER_APPS, hetzner],
  ] as const) {
    assert.match(source, /name:\s*dd-browser-mcp-rs/);
    assert.match(
      source,
      /path:\s*remote\/deployments\/browser-mcp-rs\/k8s\/ec2/,
      `${name} must reconcile the browser MCP deployment.`,
    );
  }
});

test('public browser-mcp gateway has dedicated abuse limits and trusted client forwarding', () => {
  const gateway = readFileSync(resolve(repoRoot, GATEWAY), 'utf8');

  assert.match(
    gateway,
    /limit_req_zone \$dd_browser_mcp_client_ip zone=dd_browser_mcp:10m rate=60r\/m/,
  );
  assert.match(
    gateway,
    /limit_conn_zone \$dd_browser_mcp_client_ip zone=dd_browser_mcp_conn:10m/,
  );
  assert.match(
    gateway,
    /limit_req_zone \$dd_browser_mcp_oauth_write_limit_key zone=dd_browser_mcp_oauth:10m rate=10r\/m/,
  );

  const mcp = nginxLocation(gateway, '= /browser-mcp');
  assert.match(mcp, /limit_req zone=dd_browser_mcp burst=15 nodelay/);
  assert.match(mcp, /limit_conn dd_browser_mcp_conn 10/);
  assert.match(mcp, /client_max_body_size 1m/);
  assert.match(mcp, /client_body_timeout 10s/);
  assert.match(mcp, /proxy_buffering off/);
  assert.match(mcp, /proxy_set_header X-Real-IP \$dd_browser_mcp_client_ip/);
  assert.match(mcp, /proxy_set_header X-Forwarded-For \$dd_browser_mcp_client_ip/);
  assert.doesNotMatch(mcp, /proxy_set_header Authorization ""/);
  assert.match(mcp, /proxy_set_header Cookie ""/);

  const health = nginxLocation(gateway, '= /browser-mcp/healthz');
  assert.match(health, /proxy_pass http:\/\/\$dd_browser_mcp_upstream\/healthz/);
  assert.match(gateway, /location ~ \^\/browser-mcp\/.*oauth.*limit_req zone=dd_browser_mcp_oauth/s);
  for (const declaration of [
    '= /.well-known/oauth-protected-resource/browser-mcp',
    '= /.well-known/oauth-authorization-server/browser-mcp',
  ]) {
    const metadata = nginxLocation(gateway, declaration);
    assert.match(metadata, /limit_req zone=dd_browser_mcp burst=15 nodelay/);
    assert.match(metadata, /limit_conn dd_browser_mcp_conn 10/);
    assert.match(metadata, /client_max_body_size 16k/);
    assert.match(metadata, /proxy_set_header X-Real-IP \$dd_browser_mcp_client_ip/);
    assert.match(metadata, /proxy_set_header Authorization ""/);
    assert.match(metadata, /proxy_set_header Cookie ""/);
  }
  assert.match(nginxLocation(gateway, '/browser-mcp/'), /return 404/);

  const accessLog = gateway
    .split('\n')
    .find((line) => line.includes('log_format dd_gateway_json'));
  assert.ok(accessLog, 'missing redacted JSON gateway access log');
  assert.match(accessLog, /"uri":"\$uri"/);
  assert.doesNotMatch(
    accessLog,
    /\$request_uri|\$args|\$http_authorization|\$http_cookie|\$request_body/,
    'Gateway access logs must not contain queries, credentials, cookies, or request bodies.',
  );
});

test('browser-mcp CLI contract has no credential or implicit navigation defaults', () => {
  const flags = readFileSync(resolve(repoRoot, CLI_FLAGS), 'utf8');

  assert.doesNotMatch(flags, /dummy-.*credential/);
  assert.match(flags, /\[flags\.require_auth\][\s\S]*?default = "true"/);
  for (const section of [
    'worker_auth_secret',
    'oauth_signing_secret',
    'oauth_operator_secret',
    'allowed_domains',
  ]) {
    const body = flags.match(new RegExp(`\\[flags\\.${section}\\]([\\s\\S]*?)(?=\\n\\[|$)`))?.[1];
    assert.ok(body, `missing ${section} flag`);
    assert.doesNotMatch(body, /\ndefault\s*=/, `${section} must not have an implicit default`);
  }
});
