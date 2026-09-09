import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, '../../..');

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function renderKustomization(relativePath: string): string {
  return execFileSync('kubectl', ['kustomize', path.join(repoRoot, relativePath)], {
    encoding: 'utf8',
  });
}

function manifestDocument(rendered: string, kind: string, name: string): string {
  const document = rendered
    .split(/\n---\n/)
    .find(
      (candidate) =>
        new RegExp(`^kind:\\s*${kind}\\s*$`, 'm').test(candidate) &&
        new RegExp(`^\\s*name:\\s*${name}\\s*$`, 'm').test(candidate),
    );
  assert.ok(document, `missing ${kind}/${name}`);
  return document;
}

function resourceBlock(rendered: string, kind: string, name: string): string {
  return manifestDocument(rendered, kind, name);
}

function deploymentBlock(rendered: string, name: string): string {
  return manifestDocument(rendered, 'Deployment', name);
}

function statefulSetBlock(rendered: string, name: string): string {
  return manifestDocument(rendered, 'StatefulSet', name);
}

test('common secrets bundle renders a namespace-gated Fiducia store and admission guard', () => {
  const rendered = renderKustomization('remote/argocd/secrets/common');

  assert.match(rendered, /kind:\s*ClusterSecretStore/);
  assert.match(rendered, /name:\s*dd-fiducia-kv/);
  assert.match(rendered, /dd\.dev\/fiducia-kv-secrets:\s*enabled/);
  assert.match(
    rendered,
    /url:\s*['"]?https:\/\/fiducia-load-balance\.fiducia\.svc\.cluster\.local:8443\/v1\/kv\?key=\{\{[\s\n]*\.remoteRef\.key \}\}['"]?/,
  );
  assert.match(rendered, /caProvider:[\s\S]{0,180}name:\s*fiducia-load-balance-tls/);
  assert.match(rendered, /caProvider:[\s\S]{0,180}key:\s*ca\.crt/);
  assert.match(rendered, /jsonPath:\s*\$\.entry\.value/);
  assert.match(rendered, /Authorization:\s*Bearer \{\{ \.auth\.token \}\}/);
  assert.match(rendered, /name:\s*fiducia-eso-reader[\s\S]*namespace:\s*external-secrets/);
  assert.match(rendered, /kind:\s*ValidatingAdmissionPolicy/);
  assert.match(rendered, /name:\s*dd-fiducia-kv-external-secret-guard/);
  assert.match(rendered, /kind:\s*ValidatingAdmissionPolicyBinding/);
  assert.match(rendered, /name:\s*dd-fiducia-kv-external-secret-guard-binding/);
});

test('admission policy constrains authors, target lifecycle, and every Fiducia key path', () => {
  const rendered = renderKustomization('remote/argocd/secrets/common');
  const policy = resourceBlock(
    rendered,
    'ValidatingAdmissionPolicy',
    'dd-fiducia-kv-external-secret-guard',
  );

  assert.match(policy, /system:serviceaccounts:argocd/);
  assert.match(policy, /system:masters/);
  assert.match(policy, /creationPolicy/);
  assert.match(policy, /Owner/);
  assert.match(policy, /deletionPolicy/);
  assert.match(policy, /Retain/);
  assert.match(policy, /dataFrom/);
  assert.match(policy, /size\(object\.spec\.dataFrom\) == 0/);
  assert.match(policy, /object\.spec\.data\.all/);
  assert.match(policy, /dd\.dev\/fiducia-workload/);
  assert.match(policy, /expectedPrefix/);
  assert.match(policy, /k8s\//);
  assert.match(policy, /secretKey/);
});

test('Fiducia runtime renders cloud bootstrap secrets and fail-closed credential references', () => {
  const rendered = renderKustomization('remote/argocd/fiducia');
  const node = statefulSetBlock(rendered, 'fiducia-node');
  const loadBalancer = deploymentBlock(rendered, 'fiducia-load-balance');
  const admin = deploymentBlock(rendered, 'fiducia-admin');
  const auth = deploymentBlock(rendered, 'fiducia-auth');
  const brain = deploymentBlock(rendered, 'fiducia-brain');

  for (const block of [node, loadBalancer, admin, auth, brain]) {
    assert.match(block, /FIDUCIA_CLUSTER_BOOTSTRAP_TOKEN/);
    assert.match(block, /name:\s*fiducia-cloud-bootstrap/);
  }

  assert.match(node, /FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(node, /FIDUCIA_KV_CURRENT_KEY_ID/);
  assert.match(node, /name:\s*fiducia-kv-keyring/);
  assert.match(loadBalancer, /FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(loadBalancer, /FIDUCIA_KV_CURRENT_KEY_ID/);
  assert.match(loadBalancer, /name:\s*fiducia-kv-keyring/);
});

test('Fiducia nodes require the cloud-bootstrapped versioned encryption keyring', () => {
  const rendered = renderKustomization('remote/argocd/fiducia');
  const node = statefulSetBlock(rendered, 'fiducia-node');
  assert.match(node, /FIDUCIA_KV_ENCRYPTION_KEYS/);
  assert.match(node, /FIDUCIA_KV_CURRENT_KEY_ID/);
});

test('load-balancer ingress is an explicit workload allowlist, not a self-applied label', () => {
  const rendered = renderKustomization('remote/argocd/fiducia');
  const loadBalancer = deploymentBlock(rendered, 'fiducia-load-balance');
  assert.doesNotMatch(loadBalancer, /fiducia-load-balance-client/);
});

test('only explicitly labelled namespaces can consume the cluster-wide Fiducia reader', () => {
  const rendered = renderKustomization('remote/argocd/secrets/common');
  const store = resourceBlock(rendered, 'ClusterSecretStore', 'dd-fiducia-kv');
  const binding = resourceBlock(
    rendered,
    'ValidatingAdmissionPolicyBinding',
    'dd-fiducia-kv-external-secret-guard-binding',
  );
  assert.match(store, /namespaceSelector/);
  assert.match(store, /dd\.dev\/fiducia-kv-secrets:\s*enabled/);
  assert.match(binding, /namespaceSelector/);
  assert.match(binding, /dd\.dev\/fiducia-kv-secrets:\s*enabled/);
});

test('runbook records the audited callers, guarded key grammar, and staged durability/TLS work', () => {
  const runbook = read('remote/argocd/fiducia/README.md');
  assert.match(runbook, /Fiducia/i);
  assert.match(runbook, /ExternalSecret/i);
  assert.match(runbook, /k8s\/<namespace>\/<workload>\/<ENV_VAR>/);
  assert.match(runbook, /transport-encrypted/i);
});

test('FID-SEC-1 staged rollout: the KV-path TLS PKI and dual listener render', () => {
  const rendered = renderKustomization('remote/argocd/fiducia');

  // A namespace CA is bootstrapped from the cluster `selfsigned` ClusterIssuer.
  assert.match(rendered, /kind:\s*Issuer[\s\S]*name:\s*fiducia-selfsigned/);
  assert.match(rendered, /kind:\s*Certificate[\s\S]*name:\s*fiducia-ca/);
  assert.match(rendered, /secretName:\s*fiducia-ca/);
  assert.match(rendered, /isCA:\s*true/);

  const servingCertificate = manifestDocument(
    rendered,
    'Certificate',
    'fiducia-load-balance-tls',
  );
  assert.match(servingCertificate, /secretName:\s*fiducia-load-balance-tls/);
  assert.match(servingCertificate, /fiducia-load-balance\.fiducia\.svc\.cluster\.local/);
  assert.match(servingCertificate, /kind:\s*Issuer[\s\S]*name:\s*fiducia-ca/);

  // ESO has migrated to verified HTTPS, while application clients retain the
  // plaintext listener during the staged DEN-438 migration. The old listener
  // must remain until every direct caller has moved and the downgrade gate is
  // deliberately closed.
  assert.match(rendered, /name:\s*FIDUCIA_TLS_CERT_PATH[\s\S]{0,100}value:\s*\/etc\/fiducia\/tls\/tls\.crt/);
  assert.match(rendered, /name:\s*FIDUCIA_TLS_KEY_PATH[\s\S]{0,100}value:\s*\/etc\/fiducia\/tls\/tls\.key/);
  assert.match(rendered, /name:\s*TLS_PORT[\s\S]{0,80}value:\s*['"]?8443['"]?/);
  assert.match(rendered, /containerPort:\s*8443[\s\S]{0,40}name:\s*https/);
  assert.match(rendered, /containerPort:\s*8088[\s\S]{0,40}name:\s*http/);
});
