import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/multicloud-data-plane/README.md'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

const providers = {
  aws: {
    region: 'us-east-1',
    peerRegions: ['gcp', 'azure'],
    storageProvisioner: 'ebs.csi.aws.com',
    internalAnnotation: 'service.beta.kubernetes.io/aws-load-balancer-scheme: internal',
  },
  gcp: {
    region: 'us-central1',
    peerRegions: ['aws', 'azure'],
    storageProvisioner: 'pd.csi.storage.gke.io',
    internalAnnotation: 'networking.gke.io/load-balancer-type: Internal',
  },
  azure: {
    region: 'eastus',
    peerRegions: ['aws', 'gcp'],
    storageProvisioner: 'disk.csi.azure.com',
    internalAnnotation: 'service.beta.kubernetes.io/azure-load-balancer-internal: "true"',
  },
} as const;

test('provider roots register six manual data-plane Applications', async () => {
  for (const [provider, contract] of Object.entries(providers)) {
    const applications = await readRepoFile(
      `remote/argocd/multicloud-data-plane/clusters/${provider}/applications.yaml`,
    );

    assert.equal((applications.match(/^kind: Application$/gm) ?? []).length, 6, provider);
    assert.doesNotMatch(applications, /^\s+automated:/m, provider);
    assert.match(applications, /chart: cert-manager\n\s+targetRevision: v1\.21\.1/, provider);
    assert.match(applications, /chart: trust-manager\n\s+targetRevision: v0\.24\.0/, provider);
    assert.match(applications, /chart: cockroachdb-operator-chart\n\s+targetRevision: 1\.0\.0/, provider);
    assert.match(applications, /chart: cockroachdb-chart\n\s+targetRevision: 26\.3\.1/, provider);
    assert.match(applications, /chart: nats\n\s+targetRevision: 2\.14\.6/, provider);
    assert.match(applications, new RegExp(`cloudRegion: ${contract.region}`), provider);
    assert.match(
      applications,
      new RegExp(`cockroachdb/values/${provider}\\.yaml`),
      provider,
    );
    assert.match(applications, new RegExp(`nats/values/${provider}\\.yaml`), provider);
    assert.match(applications, /targetRevision: dev\n\s+ref: values/, provider);

    const renderedRegistration = execFileSync(
      'kubectl',
      ['kustomize', `remote/argocd/multicloud-data-plane/clusters/${provider}`],
      { cwd: repoRoot, encoding: 'utf8' },
    );
    assert.match(renderedRegistration, new RegExp(`name: dd-multicloud-cockroachdb-${provider}`));
    assert.match(
      renderedRegistration,
      new RegExp(`name: dd-multicloud-nats-supercluster-${provider}`),
    );

    const renderedRoot = execFileSync(
      'kubectl',
      ['kustomize', `remote/argocd/clusters/${provider}`],
      { cwd: repoRoot, encoding: 'utf8' },
    );
    assert.match(renderedRoot, new RegExp(`name: dd-root-${provider}`), provider);
    assert.match(renderedRoot, new RegExp(`name: dd-multicloud-data-prerequisites-${provider}`));
    assert.match(renderedRoot, new RegExp(`provisioner: ${contract.storageProvisioner.replaceAll('.', '\\.')}`));
  }
});

test('CockroachDB is one retained, TLS-only, three-region R3-per-region cluster', async () => {
  const common = await readRepoFile(
    'remote/argocd/multicloud-data-plane/cockroachdb/values/common.yaml',
  );

  assert.match(common, /enabled: true\n\s+certificates:\n\s+caConfigMapName: dd-cockroachdb-ca/);
  assert.match(common, /selfSigner:\n\s+enabled: false/);
  assert.match(common, /certManager:\n\s+enabled: false/);
  assert.equal((common.match(/\n\s+nodes: 3/g) ?? []).length, 3);
  assert.match(common, /code: us-east-1[\s\S]*cloudProvider: aws/);
  assert.match(common, /code: us-central1[\s\S]*cloudProvider: gcp/);
  assert.match(common, /code: eastus[\s\S]*cloudProvider: azure/);
  assert.match(common, /storageClassName: dd-block/);
  assert.match(common, /storage: 100Gi/);
  assert.match(common, /persistentVolumeClaimRetentionPolicy:\n\s+whenDeleted: Retain/);
  assert.match(common, /topologyKey: topology\.kubernetes\.io\/zone/);
  assert.match(common, /whenUnsatisfiable: DoNotSchedule/);
  assert.match(common, /localityLabel: region/);
  assert.match(common, /localityLabel: zone/);
  assert.doesNotMatch(common, /postInitSQL/);

  for (const [provider, contract] of Object.entries(providers)) {
    const overlay = await readRepoFile(
      `remote/argocd/multicloud-data-plane/cockroachdb/values/${provider}.yaml`,
    );
    assert.match(overlay, /type: LoadBalancer/, provider);
    assert.match(overlay, new RegExp(contract.internalAnnotation.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});

test('CockroachDB private PKI is secret-backed and publishes only its public trust bundle', async () => {
  const externalSecrets = await readRepoFile(
    'remote/argocd/multicloud-data-plane/prerequisites/external-secrets.yaml',
  );
  const pki = await readRepoFile(
    'remote/argocd/multicloud-data-plane/prerequisites/cockroachdb-pki.yaml',
  );

  assert.match(externalSecrets, /name: dd-cockroachdb-ca[\s\S]*refreshPolicy: CreatedOnce/);
  assert.match(externalSecrets, /key: dd\/multicloud\/cockroachdb-ca/);
  assert.match(externalSecrets, /name: dd-cockroachdb-ca-trust-source/);
  assert.match(pki, /kind: Issuer[\s\S]*secretName: dd-cockroachdb-ca/);
  assert.match(pki, /kind: Bundle[\s\S]*key: tls\.crt[\s\S]*configMap:[\s\S]*key: ca\.crt/);
  assert.equal((pki.match(/^kind: Certificate$/gm) ?? []).length, 3);
  assert.match(pki, /commonName: node/);
  assert.match(pki, /commonName: root/);
  assert.match(pki, /dd-cockroachdb\.cockroachdb\.svc\.aws\.crdb\.k8s\.ores\.internal/);
  assert.match(pki, /dd-cockroachdb\.cockroachdb\.svc\.gcp\.crdb\.k8s\.ores\.internal/);
  assert.match(pki, /dd-cockroachdb\.cockroachdb\.svc\.azure\.crdb\.k8s\.ores\.internal/);
  assert.doesNotMatch(externalSecrets + pki, /BEGIN (?:RSA )?PRIVATE KEY/);
  assert.doesNotMatch(externalSecrets + pki, /^kind: Secret$/m);
});

test('NATS is one authenticated supercluster with explicit regional R3 stream placement', async () => {
  const common = await readRepoFile(
    'remote/argocd/multicloud-data-plane/nats/values/common.yaml',
  );

  assert.match(common, /cluster:\n\s+enabled: true[\s\S]*replicas: 3/);
  assert.match(common, /jetstream:\n\s+enabled: true\n\s+merge:\n\s+domain: ORES_MULTICLOUD/);
  assert.match(common, /storageClassName: dd-block/);
  assert.match(common, /size: 50Gi/);
  assert.match(common, /gateway:\n\s+enabled: true\n\s+port: 7222/);
  assert.match(common, /secretName: dd-nats-gateway-tls/);
  assert.match(common, /secretName: dd-nats-route-tls/);
  assert.match(common, /secretName: dd-nats-client-tls/);
  assert.match(common, /secretName: dd-nats-ca/);
  assert.match(common, /accounts:\n\s+DD_APP:\n\s+jetstream: enabled/);
  assert.match(common, /SYS:[\s\S]*system_account: SYS/);
  assert.match(common, /user: << \$NATS_APP_USER >>/);
  assert.match(common, /password: << \$NATS_APP_PASSWORD >>/);
  assert.match(common, /user: << \$NATS_SYSTEM_USER >>/);
  assert.match(common, /password: << \$NATS_SYSTEM_PASSWORD >>/);
  assert.match(common, /name: dd-nats-client-auth\n\s+key: username/);
  assert.match(common, /name: dd-nats-client-auth\n\s+key: password/);
  assert.match(common, /name: dd-nats-system-auth\n\s+key: username/);
  assert.match(common, /name: dd-nats-system-auth\n\s+key: password/);
  assert.doesNotMatch(common, /authorization:\n\s+token:/);
  assert.match(common, /persistentVolumeClaimRetentionPolicy:[\s\S]*whenDeleted: Retain/);
  assert.match(common, /maxUnavailable: 1/);
  assert.match(common, /topology\.kubernetes\.io\/zone:[\s\S]*DoNotSchedule/);
  assert.match(common, /leafnodes:\n\s+enabled: false/);
  assert.match(common, /natsBox:\n\s+enabled: false/);

  for (const [provider, contract] of Object.entries(providers)) {
    const overlay = await readRepoFile(
      `remote/argocd/multicloud-data-plane/nats/values/${provider}.yaml`,
    );

    assert.ok((overlay.match(new RegExp(`name: ores-${provider}`, 'g')) ?? []).length >= 2, provider);
    assert.doesNotMatch(overlay, /domain:/, provider);
    assert.match(overlay, /reject_unknown_cluster: true/, provider);
    assert.match(overlay, new RegExp(contract.internalAnnotation.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
    assert.match(overlay, /kind: NetworkPolicy/);
    assert.match(overlay, /dd\.dev\/nats-client: "true"/);
    assert.match(overlay, /loadBalancerSourceRanges:/);
    assert.doesNotMatch(overlay, new RegExp(`nats-gateway\\.${provider}\\.`), provider);
    assert.doesNotMatch(overlay, /fiducia/i, provider);

    for (const peer of contract.peerRegions) {
      assert.match(
        overlay,
        new RegExp(`nats://nats-gateway\\.${peer}\\.k8s\\.ores\\.internal:7222`),
        `${provider} -> ${peer}`,
      );
    }
  }
});

test('NATS secret and placement contracts are fail-closed', async () => {
  const externalSecrets = await readRepoFile(
    'remote/argocd/multicloud-data-plane/prerequisites/external-secrets.yaml',
  );
  const runbook = await readRepoFile('docs/multicloud-cockroachdb-nats.md');

  assert.match(externalSecrets, /name: dd-nats-client-auth[\s\S]*key: dd\/multicloud\/nats-client-auth/);
  assert.match(externalSecrets, /name: dd-nats-system-auth[\s\S]*key: dd\/multicloud\/nats-system-auth/);
  assert.match(runbook, /--js-domain ORES_MULTICLOUD/);
  assert.match(runbook, /--cluster ores-(?:aws|gcp|azure)/);
  assert.match(runbook, /explicit stream placement/i);
  assert.match(runbook, /nine-member JetStream metadata quorum/i);
});

test('the pinned NATS chart renders the shared domain, accounts, and secret references', () => {
  for (const provider of Object.keys(providers)) {
    const rendered = execFileSync(
      'helm',
      [
        'template',
        'dd-nats-supercluster',
        'nats',
        '--repo',
        'https://nats-io.github.io/k8s/helm/charts/',
        '--version',
        '2.14.6',
        '--namespace',
        'nats-system',
        '-f',
        'remote/argocd/multicloud-data-plane/nats/values/common.yaml',
        '-f',
        `remote/argocd/multicloud-data-plane/nats/values/${provider}.yaml`,
      ],
      { cwd: repoRoot, encoding: 'utf8', maxBuffer: 10 * 1024 * 1024 },
    );

    assert.match(rendered, /"domain": "ORES_MULTICLOUD"/, provider);
    assert.match(rendered, /"DD_APP": \{[\s\S]*"jetstream": "enabled"/, provider);
    assert.match(rendered, /"SYS": \{[\s\S]*"system_account": "SYS"/, provider);
    assert.match(rendered, /"password": \$NATS_APP_PASSWORD/, provider);
    assert.match(rendered, /"password": \$NATS_SYSTEM_PASSWORD/, provider);
    assert.match(rendered, new RegExp(`"name": "ores-${provider}"`), provider);
    assert.match(rendered, /name: dd-nats-client-auth[\s\S]*name: dd-nats-system-auth/, provider);
    assert.doesNotMatch(rendered, /NATS_AUTH_TOKEN/, provider);
  }
});

test('the new supercluster does not replace the existing bootstrap NATS plane', async () => {
  const bootstrap = await readRepoFile('remote/argocd/messaging/nats.deployment.yaml');
  const awsRoot = await readRepoFile('remote/argocd/clusters/aws/applications.yaml');
  const runbook = await readRepoFile('docs/multicloud-cockroachdb-nats.md');

  assert.match(bootstrap, /name: dd-nats/);
  assert.match(awsRoot, /name: dd-messaging/);
  assert.match(runbook, /existing `messaging\/dd-nats` bootstrap remains authoritative/);
  assert.match(runbook, /Fiducia is not a fourth NATS gateway member/);
  assert.match(runbook, /Gateways connect the three regional NATS clusters/);
  assert.match(runbook, /does not[\s\S]*replicate a stream into another region automatically/i);
});
