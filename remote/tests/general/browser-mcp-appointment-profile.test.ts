import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/apps/dd-next-runtime.application.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const MCP_DEPLOYMENT =
  'remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.deployment.yaml';
const WORKER_DEPLOYMENT = 'remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml';
const APPOINTMENT_DOMAINS = ['cal.com', 'calendly.com'] as const;

function envValue(manifest: string, name: string): string {
  const match = manifest.match(new RegExp(`- name:\\s*${name}\\s*\\n\\s*value:\\s*(.*)`, 'm'));
  assert.ok(match, `${name} must be present`);
  return match[1]!.trim().replace(/^['"]|['"]$/g, '');
}

test('Gmail-derived appointment workflow is narrow and aligned with the browser worker', () => {
  const mcp = readFileSync(resolve(repoRoot, MCP_DEPLOYMENT), 'utf8');
  const worker = readFileSync(resolve(repoRoot, WORKER_DEPLOYMENT), 'utf8');

  const ceiling = envValue(mcp, 'BROWSER_MCP_ALLOWED_DOMAINS');
  const workerCeiling = envValue(worker, 'BROWSER_AGENT_ALLOWED_DOMAINS');
  assert.equal(
    workerCeiling,
    ceiling,
    'Browser MCP and Playwright/Selenium hostname ceilings must remain byte-for-byte identical.',
  );

  const workflowJson = mcp.match(
    /- name:\s*BROWSER_MCP_WORKFLOW_ALLOWLISTS_JSON\s*\n\s*value:\s*>-\s*\n\s*(\{[^\n]+\})/,
  )?.[1];
  assert.ok(workflowJson, 'missing folded workflow allowlist JSON');
  const workflows = JSON.parse(workflowJson) as Record<string, string[]>;
  assert.deepEqual(
    workflows.appointments,
    [...APPOINTMENT_DOMAINS],
    'appointments must contain exactly the two Gmail-evidenced booking roots.',
  );

  const domains = ceiling.split(',');
  for (const domain of APPOINTMENT_DOMAINS) assert.ok(domains.includes(domain));
  for (const forbidden of [
    'gmail.com',
    'mail.google.com',
    'accounts.google.com',
    'outlook.com',
    'login.microsoftonline.com',
    'linkedin.com',
    'bit.ly',
    't.co',
    'tinyurl.com',
  ]) {
    assert.ok(!domains.includes(forbidden), `${forbidden} must remain outside the navigation ceiling`);
  }
});
