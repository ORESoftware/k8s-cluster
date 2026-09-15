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
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const POLICY = 'contracts/browser-mcp-platform-jobs/instances/BrowserWorkflowPolicy/valid/platform-jobs.json';
const MCP_BASE = 'remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.deployment.yaml';
const MCP_PATCH = 'remote/deployments/browser-mcp-rs/k8s/ec2/dd-browser-mcp-rs.platform-jobs.patch.yaml';
const WORKER_BASE = 'remote/argocd/dd-next-runtime/dd-web-scraper.deployment.yaml';
const WORKER_PATCH = 'remote/argocd/dd-next-runtime/dd-web-scraper.platform-jobs.patch.yaml';

type PlatformJobsPolicy = {
  workflowId: string;
  allowedDomains: string[];
  blockedPlatformJobDomains: string[];
  globallyBlockedDomains: string[];
  blockedFieldClasses: string[];
  humanCompletionPoints: string[];
  requireExplicitApproval: boolean;
  requireRevisionBoundActionDigest: boolean;
  allowArbitraryCompanySites: boolean;
  allowMarketplaceNavigation: boolean;
  allowRedirectorNavigation: boolean;
  permitCaptchaAutomation: boolean;
};

function read(path: string): string {
  return readFileSync(resolve(repoRoot, path), 'utf8');
}

function envValue(manifest: string, name: string): string {
  const match = manifest.match(new RegExp(`- name:\\s*${name}\\s*\\n\\s*value:\\s*['\"]([^'\"]+)['\"]`, 'm'));
  assert.ok(match, `missing ${name}`);
  return match[1];
}

function workflowMap(manifest: string): Record<string, string[]> {
  const match = manifest.match(
    /- name:\s*BROWSER_MCP_WORKFLOW_ALLOWLISTS_JSON\s*\n\s*value:\s*>-\s*\n\s*(\{[^\n]+\})/,
  );
  assert.ok(match, 'missing Browser MCP workflow allowlist JSON');
  return JSON.parse(match[1]) as Record<string, string[]>;
}

function admitted(host: string, roots: string[]): boolean {
  return roots.some((root) => host === root || host.endsWith(`.${root}`));
}

test('Rust MCP and browser-worker manifests consume the reviewed platform-jobs contract', () => {
  const policy = JSON.parse(read(POLICY)) as PlatformJobsPolicy;
  const mcpBase = read(MCP_BASE);
  const mcpPatch = read(MCP_PATCH);
  const workerBase = read(WORKER_BASE);
  const workerPatch = read(WORKER_PATCH);

  assert.equal(policy.workflowId, 'platform-jobs');
  assert.equal(policy.requireExplicitApproval, true);
  assert.equal(policy.requireRevisionBoundActionDigest, true);
  assert.equal(policy.allowArbitraryCompanySites, false);
  assert.equal(policy.allowMarketplaceNavigation, false);
  assert.equal(policy.allowRedirectorNavigation, false);
  assert.equal(policy.permitCaptchaAutomation, false);
  assert.deepEqual(new Set(policy.humanCompletionPoints), new Set(['captcha', 'mfa']));

  const baseCeiling = envValue(mcpBase, 'BROWSER_MCP_ALLOWED_DOMAINS').split(',');
  const patchCeiling = envValue(mcpPatch, 'BROWSER_MCP_ALLOWED_DOMAINS').split(',');
  assert.deepEqual(envValue(workerBase, 'BROWSER_AGENT_ALLOWED_DOMAINS').split(','), baseCeiling);
  assert.deepEqual(envValue(workerPatch, 'BROWSER_AGENT_ALLOWED_DOMAINS').split(','), baseCeiling);
  assert.deepEqual(patchCeiling, baseCeiling);

  const baseWorkflows = workflowMap(mcpBase);
  const patchWorkflows = workflowMap(mcpPatch);
  assert.deepEqual(baseWorkflows['platform-jobs'], policy.allowedDomains);
  assert.deepEqual(patchWorkflows['platform-jobs'], policy.allowedDomains);
  assert.deepEqual(patchWorkflows['fiducia-applications'], baseWorkflows['fiducia-applications']);
  assert.deepEqual(patchWorkflows.appointments, baseWorkflows.appointments);

  for (const blocked of policy.blockedPlatformJobDomains) {
    assert.equal(admitted(blocked, policy.allowedDomains), false, `${blocked} must stay outside platform-jobs`);
  }
  for (const blocked of policy.globallyBlockedDomains) {
    assert.equal(admitted(blocked, baseCeiling), false, `${blocked} must stay outside the process ceiling`);
  }

  assert.ok(policy.blockedFieldClasses.includes('ssn'));
  assert.ok(policy.blockedFieldClasses.includes('credential'));
  assert.ok(policy.blockedFieldClasses.includes('legal_attestation'));
  assert.ok(policy.blockedFieldClasses.includes('compensation_commitment'));
});
