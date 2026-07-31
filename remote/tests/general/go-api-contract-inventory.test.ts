import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile, readdir } from 'node:fs/promises';
import { relative, resolve } from 'node:path';
import test from 'node:test';

interface ModuleEntry {
  id: string;
  moduleRoot: string;
  entrypoint: string;
  classification: string;
  productHttpApi: boolean;
  primaryProtocol: string;
  operationalHttpRoutes: string[];
  openapiDisposition: string;
  evidence: string[];
  nextAction: string;
}

interface NonModuleEntry {
  id: string;
  path: string;
  classification: string;
  productHttpApi: boolean;
  openapiDisposition: string;
  evidence: string[];
  nextAction: string;
}

interface GoInventory {
  schemaVersion: number;
  scopeRoot: string;
  classifications: string[];
  modules: ModuleEntry[];
  nonModuleSources: NonModuleEntry[];
}

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/config/go-api-contract-inventory.json'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

async function walkForGoModules(directory: string): Promise<string[]> {
  const modules: string[] = [];
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries) {
    if (entry.name === '.git' || entry.name === 'node_modules' || entry.name === 'target') {
      continue;
    }
    const absolute = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      modules.push(...(await walkForGoModules(absolute)));
    } else if (entry.isFile() && entry.name === 'go.mod') {
      modules.push(relative(repoRoot, resolve(directory)).replaceAll('\\', '/'));
    }
  }
  return modules;
}

async function inventory(): Promise<GoInventory> {
  return JSON.parse(
    await readRepoFile('remote/config/go-api-contract-inventory.json'),
  ) as GoInventory;
}

test('every standalone Go deployment module has exactly one explicit protocol classification', async () => {
  const document = await inventory();
  assert.equal(document.schemaVersion, 1);
  assert.equal(document.scopeRoot, 'remote/deployments');
  assert.deepEqual(document.classifications, [
    'product-http-api',
    'protocol-only',
    'controller-worker',
    'metrics-exporter',
    'embedded-runtime-fixture',
    'retired',
  ]);

  const discovered = (await walkForGoModules(resolve(repoRoot, document.scopeRoot))).sort();
  const declared = document.modules.map((entry) => entry.moduleRoot).sort();
  assert.deepEqual(
    declared,
    discovered,
    `Go module inventory drift: declared=${declared.join(',')} discovered=${discovered.join(',')}`,
  );

  assert.equal(new Set(document.modules.map((entry) => entry.id)).size, document.modules.length);
  assert.equal(
    new Set(document.modules.map((entry) => entry.moduleRoot)).size,
    document.modules.length,
  );

  for (const entry of document.modules) {
    assert.ok(document.classifications.includes(entry.classification));
    assert.ok(existsSync(resolve(repoRoot, entry.moduleRoot, 'go.mod')));
    assert.ok(existsSync(resolve(repoRoot, entry.entrypoint)));
    assert.ok(entry.primaryProtocol.length > 0);
    assert.ok(entry.evidence.length >= 2);
    assert.ok(entry.evidence.every((line) => line.trim().length >= 20));
    assert.ok(entry.nextAction.trim().length >= 20);
    assert.deepEqual(entry.operationalHttpRoutes, [...entry.operationalHttpRoutes].sort());
    assert.equal(new Set(entry.operationalHttpRoutes).size, entry.operationalHttpRoutes.length);

    if (entry.productHttpApi) {
      assert.equal(entry.classification, 'product-http-api');
      assert.equal(entry.openapiDisposition, 'required-executable-contract');
    } else {
      assert.notEqual(entry.classification, 'product-http-api');
      assert.match(entry.openapiDisposition, /^not-applicable-/);
    }
  }
});

test('Go WebSocket benchmark is protocol-only and exposes only operational HTTP routes', async () => {
  const document = await inventory();
  const entry = document.modules.find((candidate) => candidate.id === 'go-wss-server-go');
  assert.ok(entry);
  assert.equal(entry.classification, 'protocol-only');
  assert.equal(entry.primaryProtocol, 'websocket');
  assert.equal(entry.productHttpApi, false);
  assert.deepEqual(entry.operationalHttpRoutes, ['/healthz', '/metrics', '/readyz']);

  const source = await readRepoFile(entry.entrypoint);
  assert.match(source, /websocket\.Upgrader/);
  assert.match(source, /Upgrade\(w, r, nil\)/);
  assert.match(source, /mux\.Handle\("\/metrics"/);
  assert.match(source, /mux\.HandleFunc\("\/healthz"/);
  assert.match(source, /mux\.HandleFunc\("\/readyz"/);
  assert.doesNotMatch(source, /\/openapi\.json|\/api\/docs|\/docs\/api/);
});

test('thread operator is a CRD controller with metrics and probes, not a REST API', async () => {
  const document = await inventory();
  const entry = document.modules.find((candidate) => candidate.id === 'thread-operator-go');
  assert.ok(entry);
  assert.equal(entry.classification, 'controller-worker');
  assert.equal(entry.primaryProtocol, 'kubernetes-controller-runtime');
  assert.equal(entry.productHttpApi, false);
  assert.deepEqual(entry.operationalHttpRoutes, ['/healthz', '/metrics', '/readyz']);

  const source = await readRepoFile(entry.entrypoint);
  assert.match(source, /ctrl\.NewManager/);
  assert.match(source, /ThreadReconciler/);
  assert.match(source, /metricsserver\.Options/);
  assert.match(source, /AddHealthzCheck\("healthz"/);
  assert.match(source, /AddReadyzCheck\("readyz"/);
  assert.doesNotMatch(source, /http\.NewServeMux|http\.HandleFunc|\/openapi\.json/);
});

test('thread fleet process is a Prometheus exporter, not a product API', async () => {
  const document = await inventory();
  const entry = document.modules.find(
    (candidate) => candidate.id === 'thread-fleet-exporter-go',
  );
  assert.ok(entry);
  assert.equal(entry.classification, 'metrics-exporter');
  assert.equal(entry.primaryProtocol, 'prometheus-exposition');
  assert.equal(entry.productHttpApi, false);
  assert.deepEqual(entry.operationalHttpRoutes, ['/healthz', '/metrics']);

  const source = await readRepoFile(entry.entrypoint);
  assert.match(source, /promhttp\.HandlerFor/);
  assert.match(source, /mux\.Handle\("\/metrics"/);
  assert.match(source, /mux\.HandleFunc\("\/healthz"/);
  assert.match(source, /kubernetes\.NewForConfig/);
  assert.doesNotMatch(source, /\/openapi\.json|\/api\/docs|\/docs\/api/);
});

test('embedded Go handler is classified separately from standalone modules', async () => {
  const document = await inventory();
  assert.equal(document.nonModuleSources.length, 1);
  const entry = document.nonModuleSources[0];
  assert.equal(entry.id, 'container-pool-golang-handler');
  assert.equal(entry.classification, 'embedded-runtime-fixture');
  assert.equal(entry.productHttpApi, false);
  assert.equal(entry.openapiDisposition, 'not-applicable-embedded-fixture');
  assert.ok(existsSync(resolve(repoRoot, entry.path)));
  assert.ok(entry.evidence.length >= 2);
  assert.ok(entry.nextAction.length >= 20);
  assert.ok(!document.modules.some((module) => module.entrypoint === entry.path));
});

test('non-product Go processes are absent from the REST legacy scanner allowlist', async () => {
  const document = await inventory();
  const apiContracts = JSON.parse(
    await readRepoFile('remote/config/api-contracts.json'),
  ) as { legacySourceScannerAllowlist: string[] };
  const allowlist = new Set(apiContracts.legacySourceScannerAllowlist);

  for (const entry of document.modules.filter((candidate) => !candidate.productHttpApi)) {
    assert.equal(
      allowlist.has(entry.id),
      false,
      `${entry.id} is not a product HTTP API and must not be source-scanned into OpenAPI`,
    );
  }
});
