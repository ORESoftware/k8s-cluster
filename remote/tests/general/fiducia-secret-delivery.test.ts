import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/secrets/common/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function renderKustomization(relativePath: string): string {
  return execFileSync('kubectl', ['kustomize', relativePath], {
    cwd: repoRoot,
    encoding: 'utf8',
  });
}

function requiredSecretRefPattern(
  environmentName: string,
  secretName: string,
  secretKey: string,
): RegExp {
  // Kubernetes' YAML serializer may order secretKeyRef.{key,name,optional}
  // differently from the source manifest. Look ahead for all three fields in
  // the same small mapping instead of treating serialization order as a contract.
  return new RegExp(
    `name:\\s*${environmentName}[\\s\\S]{0,260}` +
      `secretKeyRef:` +
      `(?=[\\s\\S]{0,220}key:\\s*${secretKey})` +
      `(?=[\\s\\S]{0,220}name:\\s*${secretName})` +
      `(?=[\\s\\S]{0,220}optional:\\s*false)`,
  );
}

test('common secrets bundle renders a namespace-gated Fiducia store and admission guard', () => {
  const rendered = renderKustomization('remote/argocd/secrets/common');

  assert.match(rendered, /kind:\s*ClusterSecretStore[\s\S]*name:\s*dd-fiducia-kv/);
  assert.match(rendered, /dd\.dev\/secret-backend:\s*fiducia-kv/);
  assert.match(
    rendered,
    /conditions:[\s\S]{0,180}namespaceSelector:[\s\S]{0,180}dd\.dev\/fiducia-kv-secrets:\s*enabled/,
  );
  assert.match(
    rendered,
    /url:\s*http:\/\/fiducia-load-balance\.fiducia\.svc\.cluster\.local:8088\/v1\/kv\?key=\{\{[\s\n]*\.remoteRef\.key \}\}/,
  );
  assert.match(rendered, /jsonPath:\s*\$\.entry\.value/);
  assert.match(rendered, /Authorization:\s*Bearer \{\{ \.auth\.token \}\}/);
  assert.match(rendered, /name:\s*fiducia-eso-reader[\s\S]*namespace:\s*external-secrets/);
  assert.match(rendered, /key:\s*dd\/remote-dev\/fiducia-eso-reader/);
  assert.match(rendered, /property:\s*FIDUCIA_API_KEY/);
  assert.match(rendered, /external-secrets\.io\/type:\s*webhook/);
  assert.match(rendered, /kind:\s*ValidatingAdmissionPolicy\b/);
  assert.match(rendered, /kind:\s*ValidatingAdmissionPolicyBinding\b/);
  assert.match(rendered, /validationActions:[\s\S]{0,80}- Deny[\s\S]{0,80}- Audit/);
  assert.doesNotMatch(rendered, /urlquery/);
  assert.doesNotMatch(rendered, /fdc_(?:live|test)_[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/);
});

test('admission policy constrains authors, target lifecycle, and every Fiducia key path', async () => {
  const policy = await readRepoFile(
    'remote/argocd/secrets/common/fiducia-external-secret-policy.yaml',
  );

  assert.match(policy, /failurePolicy:\s*Fail/);
  assert.match(policy, /system:serviceaccounts:argocd/);
  assert.match(policy, /system:masters/);
  assert.match(policy, /dataFrom is forbidden/);
  assert.match(policy, /creationPolicy:\s*Owner|object\.spec\.target\.creationPolicy == 'Owner'/);
  assert.match(policy, /deletionPolicy:\s*Retain|object\.spec\.target\.deletionPolicy == 'Retain'/);
  assert.match(policy, /k8s\/<namespace>\/<annotated-workload>\/<ENV_VAR>/);
  assert.match(policy, /item\.remoteRef\.key\.startsWith\(variables\.expectedPrefix\)/);
  assert.match(policy, /item\.remoteRef\.key\.endsWith\('\/' \+ item\.secretKey\)/);
});

test('Fiducia runtime renders cloud bootstrap secrets and fail-closed credential references', () => {
  const rendered = renderKustomization('remote/argocd/fiducia');

  for (const secretName of [
    'fiducia-cluster-secrets',
    'fiducia-auth-secrets',
    'fiducia-backend-secrets',
    'fiducia-admin-secrets',
  ]) {
    assert.match(rendered, new RegExp(`kind:\\s*ExternalSecret[\\s\\S]*name:\\s*${secretName}`));
  }

  for (const property of [
    'FIDUCIA_INTERNAL_SECRET',
    'FIDUCIA_BRAIN_RAFT_SECRET',
    'FIDUCIA_JWT_SIGNING_KEY',
    'FIDUCIA_INTROSPECT_SECRET',
    'FIDUCIA_KEY_IDEMPOTENCY_SECRET',
    'CUSTOMER_API_KEY_PEPPER',
    'SUPABASE_SERVICE_ROLE_KEY',
  ]) {
    assert.match(rendered, new RegExp(`property:\\s*${property}`));
  }

  assert.match(rendered, requiredSecretRefPattern('FIDUCIA_INTERNAL_SECRET', 'fiducia-cluster-secrets', 'internal-secret'));
  assert.match(rendered, requiredSecretRefPattern('FIDUCIA_BRAIN_RAFT_SECRET', 'fiducia-cluster-secrets', 'brain-raft-secret'));
  assert.match(rendered, requiredSecretRefPattern('FIDUCIA_JWT_SIGNING_KEY', 'fiducia-auth-secrets', 'jwt-signing-key'));
  assert.match(rendered, requiredSecretRefPattern('CUSTOMER_API_KEY_PEPPER', 'fiducia-auth-secrets', 'api-key-pepper'));
  assert.match(rendered, requiredSecretRefPattern('SUPABASE_SERVICE_ROLE_KEY', 'fiducia-auth-secrets', 'supabase-service-role-key'));
  assert.match(rendered, /name:\s*FIDUCIA_AUTH_REQUIRED\s*\n\s*value:\s*["']?true["']?/);
  assert.match(rendered, /name:\s*CUSTOMER_API_KEY_ACCEPT_LEGACY_SHA256\s*\n\s*value:\s*["']?false["']?/);
  assert.doesNotMatch(rendered, /fdc_(?:live|test)_[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/);
});

test('Fiducia nodes require the cloud-bootstrapped versioned encryption keyring', async () => {
  const [bootstrap, node] = await Promise.all([
    readRepoFile('remote/argocd/fiducia/fiducia-kv-protection.externalsecret.yaml'),
    readRepoFile('remote/argocd/fiducia/fiducia-node.statefulset.yaml'),
  ]);

  assert.match(bootstrap, /key:\s*dd\/remote-dev\/fiducia-kv-protection/);
  assert.match(bootstrap, /property:\s*FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(bootstrap, /property:\s*FIDUCIA_KV_ENCRYPTION_ACTIVE_KEY_ID/);
  assert.match(bootstrap, /argocd\.argoproj\.io\/sync-wave:\s*"-1"/);
  assert.match(node, /name:\s*FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(node, /name:\s*fiducia-kv-protection\s*\n\s*key:\s*encryption-keys\s*\n\s*optional:\s*false/);
  assert.match(node, /name:\s*FIDUCIA_KV_ENCRYPTION_ACTIVE_KEY_ID/);
  assert.match(node, /name:\s*fiducia-kv-protection\s*\n\s*key:\s*active-key-id\s*\n\s*optional:\s*false/);
});

test('load-balancer ingress is an explicit workload allowlist, not a self-applied label', async () => {
  const policy = await readRepoFile(
    'remote/argocd/fiducia/fiducia-load-balance.networkpolicy.yaml',
  );

  for (const app of [
    'dd-contract-service',
    'dd-billing-server',
    'dd-build-server',
    'dd-fabrication-server',
  ]) {
    assert.match(policy, new RegExp(`- ${app}`));
  }
  assert.match(
    policy,
    /kubernetes\.io\/metadata\.name:\s*external-secrets[\s\S]{0,180}app\.kubernetes\.io\/name:\s*external-secrets/,
  );
  assert.match(policy, /protocol:\s*TCP\s*\n\s*port:\s*8088/);
  assert.doesNotMatch(policy, /^\s*dd\.dev\/fiducia-client:\s*/m);
});

test('only explicitly labelled namespaces can consume the cluster-wide Fiducia reader', async () => {
  const namespace = await readRepoFile('remote/argocd/dd-next-runtime/namespace.yaml');

  assert.match(namespace, /name:\s*default/);
  assert.match(namespace, /dd\.dev\/fiducia-kv-secrets:\s*enabled/);
});

test('runbook records the audited callers, guarded key grammar, and staged durability/TLS work', async () => {
  const runbook = await readRepoFile('docs/fiducia-secret-delivery.md');

  assert.match(runbook, /Audit inventory as of 2026-07-27/);
  assert.match(runbook, /main currently has no active[\s\S]*dd-fiducia-kv/i);
  assert.match(runbook, /dd-fabrication-server[\s\S]*legacy[\s\S]*overlay/i);
  assert.match(runbook, /name:\s*dd-fiducia-kv/);
  assert.match(runbook, /key:\s*k8s\/default\/example-api\/DATABASE_URL/);
  assert.match(runbook, /envFrom:[\s\S]*secretRef:[\s\S]*name:\s*example-api-secrets/);
  assert.match(runbook, /secret\.reloader\.stakater\.com\/reload:\s*example-api-secrets/);
  assert.match(runbook, /environment variables do not change until the pod restarts/i);
  assert.match(runbook, /never give ESO write scope/i);
  assert.match(runbook, /emptyDir/);
  assert.match(runbook, /transport-encrypted/i);
});

test('FID-SEC-1 phase 1: the KV-path TLS PKI renders and is inert (no workload consumes it yet)', () => {
  const rendered = renderKustomization('remote/argocd/fiducia');

  // A namespace CA is bootstrapped from the cluster `selfsigned` ClusterIssuer.
  assert.match(
    rendered,
    /kind:\s*Certificate[\s\S]{0,400}name:\s*fiducia-internal-ca[\s\S]{0,400}isCA:\s*true/,
    'expected an isCA fiducia-internal-ca Certificate',
  );
  assert.match(
    rendered,
    /kind:\s*Certificate[\s\S]{0,600}name:\s*fiducia-internal-ca[\s\S]{0,600}issuerRef:[\s\S]{0,160}name:\s*selfsigned/,
    'CA must chain to the selfsigned ClusterIssuer',
  );
  // A CA Issuer signs fiducia serving certs from that CA secret.
  assert.match(
    rendered,
    /kind:\s*Issuer[\s\S]{0,200}name:\s*fiducia-ca[\s\S]{0,200}secretName:\s*fiducia-internal-ca/,
    'expected a fiducia-ca Issuer backed by the internal CA secret',
  );
  // The LB serving certificate covers every in-cluster Service DNS form clients use.
  assert.match(rendered, /secretName:\s*fiducia-load-balance-tls/);
  assert.match(
    rendered,
    /dnsNames:[\s\S]{0,240}fiducia-load-balance\.fiducia\.svc\.cluster\.local/,
  );
  assert.match(rendered, /^\s*-\s*fiducia-load-balance\s*$/m, 'expected the bare Service name SAN');
  // kubectl kustomize sorts spec keys alphabetically, so issuerRef precedes
  // secretName; anchor on the cert's metadata name instead of field order.
  assert.match(
    rendered,
    /name:\s*fiducia-load-balance-tls[\s\S]{0,400}issuerRef:[\s\S]{0,120}kind:\s*Issuer[\s\S]{0,60}name:\s*fiducia-ca/,
    'the serving cert must be signed by the fiducia-ca Issuer',
  );

  // Inertness ratchet: phase 1 must NOT wire the LB's TLS listener. When the
  // phase-2 cutover (DEN-1240) lands FIDUCIA_TLS_CERT_PATH + the volume mount,
  // this assertion is flipped deliberately in the same change — so an accidental
  // early enablement (which 426s every plaintext client) fails the suite here.
  assert.doesNotMatch(
    rendered,
    /FIDUCIA_TLS_CERT_PATH/,
    'phase 1 is inert: no workload may consume the TLS cert until the coordinated cutover',
  );
});
