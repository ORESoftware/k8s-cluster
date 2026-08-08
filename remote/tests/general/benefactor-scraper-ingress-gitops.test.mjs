import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot() {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/dd-next-runtime/dd-web-scraper.networkpolicy.yaml'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repository root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(path) {
  return readFile(resolve(repoRoot, path), 'utf8');
}

function count(source, literal) {
  return source.split(literal).length - 1;
}

const allowedWorkloads = [
  {
    name: 'benefactor-contact-batch',
    component: 'lead-discovery',
    source: 'remote/argocd/benefactor-contact-pipeline/cronjob.yaml',
  },
  {
    name: 'benefactor-provider-probe-v2',
    component: 'credentialed-dry-run',
    source: 'scripts/ops/run_benefactor_provider_diagnostics_direct_20260808.sh',
  },
  {
    name: 'benefactor-search-scraper-contract-probe',
    component: 'credentialed-dry-run',
    source: 'scripts/ops/run_benefactor_search_scraper_contract_probe_20260808.sh',
  },
];

function exactSelector(name, component) {
  return [
    '- podSelector:',
    '            matchLabels:',
    `              app.kubernetes.io/name: ${name}`,
    `              app.kubernetes.io/component: ${component}`,
    '              app.kubernetes.io/part-of: benefactor-cc',
  ].join('\n');
}

test('private scraper ingress permits only exact Benefactor discovery and diagnostic workloads', async () => {
  const policy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-web-scraper.networkpolicy.yaml',
  );

  for (const workload of allowedWorkloads) {
    assert.ok(
      policy.includes(exactSelector(workload.name, workload.component)),
      `missing exact scraper ingress selector for ${workload.name}`,
    );
    assert.equal(
      count(policy, `app.kubernetes.io/name: ${workload.name}`),
      1,
      `${workload.name} must have one policy selector`,
    );
  }

  assert.equal(
    count(policy, 'app.kubernetes.io/part-of: benefactor-cc'),
    allowedWorkloads.length,
    'Benefactor access must not use a broad family-only selector',
  );
  assert.match(policy, /ports:\s*\n\s*- protocol:\s*TCP\s*\n\s*port:\s*8097/);
  assert.doesNotMatch(
    policy,
    /namespaceSelector:\s*\n\s*matchLabels:\s*\n\s*kubernetes\.io\/metadata\.name:\s*default/,
    'the default namespace must not receive blanket scraper access',
  );
});

test('every allowed selector is backed by the same labels on its controlled workload', async () => {
  for (const workload of allowedWorkloads) {
    const source = await readRepoFile(workload.source);
    assert.match(
      source,
      new RegExp(`app\\.kubernetes\\.io/name:\\s*${workload.name}`),
      `${workload.source} is missing its workload name label`,
    );
    assert.match(
      source,
      new RegExp(`app\\.kubernetes\\.io/component:\\s*${workload.component}`),
      `${workload.source} is missing its workload component label`,
    );
    assert.match(
      source,
      /app\.kubernetes\.io\/part-of:\s*benefactor-cc/,
      `${workload.source} is missing the Benefactor ownership label`,
    );
  }
});

test('the scraper remains deny-by-default and keeps its SSRF egress exclusions', async () => {
  const policy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-web-scraper.networkpolicy.yaml',
  );

  assert.match(policy, /policyTypes:\s*\n\s*- Ingress\s*\n\s*- Egress/);
  assert.match(policy, /cidr:\s*0\.0\.0\.0\/0/);
  for (const blockedRange of [
    '10.0.0.0/8',
    '127.0.0.0/8',
    '169.254.0.0/16',
    '172.16.0.0/12',
    '192.168.0.0/16',
  ]) {
    assert.ok(policy.includes(`- ${blockedRange}`), `missing SSRF exclusion ${blockedRange}`);
  }
});
