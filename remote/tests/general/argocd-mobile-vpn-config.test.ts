import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/argocd/vpn/kustomization.yaml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('Argo CD mobile stays VPN-only and gates certificate/account activation', async () => {
  const vpnKustomization = await readRepoFile('remote/argocd/vpn/kustomization.yaml');
  const mobileConfig = await readRepoFile(
    'remote/argocd/vpn/dd-argocd-mobile.configmap.yaml',
  );
  const deployment = await readRepoFile('remote/argocd/vpn/dd-vpn.deployment.yaml');
  const networkPolicy = await readRepoFile('remote/argocd/vpn/dd-vpn.networkpolicy.yaml');
  const prereqApp = await readRepoFile(
    'remote/argocd/apps/dd-argocd-mobile-prereqs.application.yaml',
  );
  const prereqKustomization = await readRepoFile(
    'remote/argocd/vpn/argocd-mobile-prereqs/kustomization.yaml',
  );
  const cloudflareSecret = await readRepoFile(
    'remote/argocd/vpn/argocd-mobile-prereqs/cloudflare-token.externalsecret.yaml',
  );
  const issuer = await readRepoFile(
    'remote/argocd/vpn/argocd-mobile-prereqs/clusterissuer.yaml',
  );
  const certificate = await readRepoFile(
    'remote/argocd/vpn/argocd-mobile-prereqs/certificate.yaml',
  );
  const argoConfig = await readRepoFile(
    'remote/argocd/vpn/argocd-mobile-prereqs/argocd-config.yaml',
  );
  const runbook = await readRepoFile(
    'remote/argocd/vpn/argocd-mobile-prereqs/readme.md',
  );

  assert.match(vpnKustomization, /dd-argocd-mobile\.configmap\.yaml/);
  assert.doesNotMatch(vpnKustomization, /argocd-mobile-prereqs/);
  assert.doesNotMatch(vpnKustomization, /ClusterIssuer/);
  assert.doesNotMatch(vpnKustomization, /Certificate/);

  assert.match(mobileConfig, /10\.8\.0\.1 argocd-vpn\.fiducia\.cloud/);
  assert.match(mobileConfig, /forward \. 10\.96\.0\.10/);
  assert.match(mobileConfig, /listen 8443 ssl/);
  assert.match(mobileConfig, /allow 10\.8\.0\.0\/24/);
  assert.match(
    mobileConfig,
    /proxy_pass https:\/\/argocd-server\.argocd\.svc\.cluster\.local:443/,
  );
  assert.match(mobileConfig, /proxy_ssl_verify off/);
  assert.match(mobileConfig, /proxy_buffering off/);

  assert.match(
    deployment,
    /image:\s*registry\.k8s\.io\/coredns\/coredns:v1\.14\.2/,
  );
  assert.match(
    deployment,
    /image:\s*docker\.io\/library\/nginx:1\.29\.7-alpine/,
  );
  assert.match(deployment, /NET_BIND_SERVICE/);
  assert.match(deployment, /runAsUser:\s*65532/);
  assert.match(deployment, /runAsUser:\s*101/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /name:\s*dns-udp[\s\S]*containerPort:\s*53/);
  assert.match(deployment, /name:\s*dns-tcp[\s\S]*containerPort:\s*53/);
  assert.match(
    deployment,
    /name:\s*argocd-mobile[\s\S]*containerPort:\s*8443/,
  );
  assert.doesNotMatch(deployment, /hostPort:\s*8443/);
  assert.match(
    deployment,
    /secretName:\s*argocd-mobile-tls[\s\S]*optional:\s*true/,
  );
  assert.match(deployment, /sha256sum[\s\S]*nginx -s reload/);

  assert.match(networkPolicy, /protocol:\s*UDP[\s\S]*port:\s*53/);
  assert.match(networkPolicy, /protocol:\s*TCP[\s\S]*port:\s*53/);
  assert.match(networkPolicy, /protocol:\s*TCP[\s\S]*port:\s*8443/);

  assert.match(prereqApp, /name:\s*dd-argocd-mobile-prereqs/);
  assert.match(
    prereqApp,
    /path:\s*remote\/argocd\/vpn\/argocd-mobile-prereqs/,
  );
  assert.match(prereqApp, /prune:\s*false/);
  assert.match(prereqApp, /ServerSideApply=true/);

  for (const resource of [
    'cloudflare-token.externalsecret.yaml',
    'clusterissuer.yaml',
    'certificate.yaml',
    'argocd-config.yaml',
  ]) {
    assert.match(
      prereqKustomization,
      new RegExp(resource.replaceAll('.', '\\.')),
    );
  }

  assert.match(cloudflareSecret, /kind:\s*ExternalSecret/);
  assert.match(cloudflareSecret, /namespace:\s*cert-manager/);
  assert.match(cloudflareSecret, /name:\s*dd-cluster-secrets/);
  assert.match(cloudflareSecret, /key:\s*dd\/remote-dev\/cloudflare/);
  assert.match(cloudflareSecret, /property:\s*CLOUDFLARE_DNS_API_TOKEN/);
  assert.match(cloudflareSecret, /deletionPolicy:\s*Retain/);

  assert.match(issuer, /kind:\s*ClusterIssuer/);
  assert.match(issuer, /name:\s*letsencrypt-prod-dns01-argocd-mobile/);
  assert.match(issuer, /dnsZones:[\s\S]*- fiducia\.cloud/);
  assert.match(
    issuer,
    /name:\s*argocd-mobile-cloudflare-dns-api-token[\s\S]*key:\s*api-token/,
  );

  assert.match(certificate, /kind:\s*Certificate/);
  assert.match(certificate, /namespace:\s*vpn/);
  assert.match(certificate, /secretName:\s*argocd-mobile-tls/);
  assert.match(certificate, /- argocd-vpn\.fiducia\.cloud/);
  assert.match(certificate, /rotationPolicy:\s*Always/);

  assert.match(argoConfig, /accounts\.argocd-mobile:\s*login/);
  assert.match(argoConfig, /accounts\.argocd-mobile\.enabled:\s*"true"/);
  assert.match(argoConfig, /policy\.mobile\.csv:/);
  assert.match(argoConfig, /p, argocd-mobile, logs, get, \*\/\*, allow/);
  assert.doesNotMatch(argoConfig, /argocd-mobile[^\n]*sync/);
  assert.doesNotMatch(argoConfig, /argocd-mobile[^\n]*delete/);
  assert.doesNotMatch(argoConfig, /argocd-mobile[^\n]*exec/);
  assert.doesNotMatch(argoConfig, /argocd-mobile, applications, update/);
  assert.doesNotMatch(argoConfig, /argocd-mobile[^\n]*action/);

  assert.match(runbook, /intentionally \*\*not\*\* referenced/);
  assert.match(runbook, /Server:\s+https:\/\/argocd-vpn\.fiducia\.cloud:8443/);
  assert.match(runbook, /cannot sync, update, delete, execute in pods/);
  assert.match(runbook, /Optional project-scoped sync/);
  assert.match(runbook, /Never replace `<project>` with `\*`/);
  assert.match(runbook, /Never delete cert-manager CRDs/);
});
