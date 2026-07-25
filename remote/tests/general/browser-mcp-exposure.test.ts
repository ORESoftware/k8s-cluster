import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

// Ratchets on the service that is reachable from the public internet with a
// write-capable browser tool. Each assertion encodes a production defect found
// on 2026-07-25.
//
// ChatGPT custom MCP apps cannot supply an operator's arbitrary static bearer.
// This deployment is therefore anonymous until OAuth is implemented. The
// process-level domain ceiling, worker-level ceiling, SSRF controls, quotas, and
// gateway rate/connection limits are required compensating controls.
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
const GATEWAY = 'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml';
const AWS_APPS = 'remote/argocd/clusters/aws/applications.yaml';
const HETZNER_APPS = 'remote/argocd/clusters/hetzner/applications.yaml';
const CLI_FLAGS = 'remote/deployments/browser-mcp-rs/.cli-flags.toml';

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

test('anonymous browser-mcp has a narrow, hostname-only domain ceiling', () => {
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
    'false',
    'ChatGPT no-auth connectivity requires BROWSER_MCP_REQUIRE_AUTH=false until OAuth exists.',
  );

  const domains = (value as string).split(',').map((domain) => domain.trim());
  assert.deepEqual(
    domains,
    ['benefactor.cc'],
    'The anonymous production endpoint must start with only Benefactor itself allowed.',
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
});

test('browser-mcp bearer is sourced from a secret, never inlined', () => {
  const manifest = readDeployment();
  if (!manifest) return;

  assert.match(
    manifest,
    /BROWSER_MCP_AUTH_SECRET\s*\n\s*valueFrom:\s*\n\s*secretKeyRef:/,
    'BROWSER_MCP_AUTH_SECRET must come from a secretKeyRef, not a literal value.',
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

  assert.match(manifest, /image:\s*ghcr\.io\/oresoftware\/dd-browser-mcp-rs:dev/);
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

  assert.match(gateway, /limit_req_zone[\s\S]*zone=dd_browser_mcp:10m rate=60r\/m/);
  assert.match(gateway, /limit_conn_zone[\s\S]*zone=dd_browser_mcp_conn:10m/);
  assert.match(gateway, /location = \/browser-mcp[\s\S]*limit_req zone=dd_browser_mcp/);
  assert.match(gateway, /location = \/browser-mcp[\s\S]*limit_conn dd_browser_mcp_conn 10/);
  assert.match(gateway, /location = \/browser-mcp[\s\S]*proxy_set_header X-Real-IP \$remote_addr/);
  assert.match(
    gateway,
    /location = \/browser-mcp[\s\S]*proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for/,
  );
});

test('browser-mcp CLI contract has no credential defaults and matches production policy', () => {
  const flags = readFileSync(resolve(repoRoot, CLI_FLAGS), 'utf8');

  assert.doesNotMatch(flags, /dummy-.*credential/);
  assert.match(flags, /\[flags\.require_auth\][\s\S]*?default = "false"/);
  assert.match(flags, /\[flags\.allowed_domains\][\s\S]*?default = "benefactor\.cc"/);
  for (const section of ['worker_auth_secret', 'auth_secret']) {
    const body = flags.match(new RegExp(`\\[flags\\.${section}\\]([\\s\\S]*?)(?=\\n\\[|$)`))?.[1];
    assert.ok(body, `missing ${section} flag`);
    assert.doesNotMatch(body, /\ndefault\s*=/, `${section} must not have a public default`);
  }
});
