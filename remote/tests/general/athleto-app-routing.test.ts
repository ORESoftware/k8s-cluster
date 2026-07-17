import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/dd-next-runtime/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();
const runtimeDir = 'remote/argocd/dd-next-runtime';

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

// The Athleto app is fronted by the `jello-ws` Service alias, and its public
// Ingress must route both the B2C host (app.athleto.store) and the B2B host
// (biz.athleto.store) to that Service on port 8145. Regressing any of these
// takes the storefront offline, so pin them.
test('athleto ingress routes both hosts to the jello-ws service on 8145', async () => {
  const ingress = await readRepoFile(`${runtimeDir}/dd-athleto-app-rs.ingress.yaml`);

  for (const host of ['app.athleto.store', 'biz.athleto.store']) {
    assert.match(ingress, new RegExp(`host:\\s*${host.replace(/\./g, '\\.')}`), `missing host ${host}`);
  }

  const backendCount = (ingress.match(/name:\s*jello-ws/g) ?? []).length;
  assert.ok(backendCount >= 2, 'both host rules must point at the jello-ws service');
  assert.match(ingress, /number:\s*8145/, 'backend service port must be 8145');

  // Each public host needs its own TLS secret so cert-manager issues both.
  for (const secret of ['athleto-public-tls', 'athleto-biz-tls']) {
    assert.match(ingress, new RegExp(`secretName:\\s*${secret}`), `missing TLS secret ${secret}`);
  }
});

// ingress-nginx runs hostNetwork, so its traffic reaches the pod with a node
// identity (host / remote-node) that a plain NetworkPolicy podSelector can
// never match. A CiliumNetworkPolicy admitting those entities on 8145 is what
// stops the intermittent 504s; assert it stays registered and correct.
test('athleto has a CiliumNetworkPolicy admitting node-identity traffic on 8145', async () => {
  const kustomization = await readRepoFile(`${runtimeDir}/kustomization.yaml`);
  assert.match(
    kustomization,
    /dd-athleto-app-rs\.ciliumnetworkpolicy\.yaml/,
    'the CiliumNetworkPolicy must be registered in the kustomization',
  );

  const cnp = await readRepoFile(`${runtimeDir}/dd-athleto-app-rs.ciliumnetworkpolicy.yaml`);
  assert.match(cnp, /kind:\s*CiliumNetworkPolicy/);
  assert.match(cnp, /app:\s*dd-athleto-app-rs/, 'must select the athleto pods');
  for (const entity of ['host', 'remote-node']) {
    assert.match(cnp, new RegExp(`-\\s*${entity}\\b`), `fromEntities must include ${entity}`);
  }
  assert.match(cnp, /port:\s*"?8145"?/, 'must admit the app port 8145');
});
