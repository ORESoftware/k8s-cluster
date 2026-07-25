import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

// Ratchets on the one service that is reachable from the public internet with a
// write-capable browser tool. Each of these encodes a real defect found on
// 2026-07-25, not a hypothetical:
//
//   * `origin/dev` carried BROWSER_MCP_ALLOWED_DOMAINS: '' together with
//     REQUIRE_AUTH: 'false'. An empty allowlist is not "no policy" — the server
//     only injects `allowed_domains` into the worker call when the list is
//     non-empty, so empty means the browser may navigate to ANY public https
//     host. Combined with no auth that is an open browser-automation relay
//     running from our EC2 address. It was never live only by luck: the running
//     pod had been hand-applied from an older manifest.
//
//   * The gateway `/browser-mcp` location has no auth gate of its own — it just
//     proxies — so the in-pod bearer is the only control. Turning it off makes
//     `browser_act` anonymous to the internet.
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

test('browser-mcp domain allowlist is never empty', () => {
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
});

test('browser-mcp keeps its bearer gate on, since the gateway has none', () => {
  const manifest = readDeployment();
  if (!manifest) return;

  assert.equal(
    envValue(manifest, 'BROWSER_MCP_REQUIRE_AUTH'),
    'true',
    'BROWSER_MCP_REQUIRE_AUTH must be "true": the gateway /browser-mcp location does not ' +
      'authenticate, so this is the only thing keeping the write-capable browser_act tool off ' +
      'the public internet.',
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
});
