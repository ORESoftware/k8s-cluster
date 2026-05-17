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

test('vpn app deploys wg-easy wireguard with private admin UI', async () => {
  const app = await readRepoFile('remote/argocd/apps/dd-vpn.application.yaml');
  const kustomization = await readRepoFile('remote/argocd/vpn/kustomization.yaml');
  const config = await readRepoFile('remote/argocd/vpn/dd-vpn.configmap.yaml');
  const externalSecret = await readRepoFile(
    'remote/argocd/vpn/dd-vpn-secrets.externalsecret.yaml',
  );
  const deployment = await readRepoFile('remote/argocd/vpn/dd-vpn.deployment.yaml');
  const service = await readRepoFile('remote/argocd/vpn/dd-vpn-ui.service.yaml');
  const networkPolicy = await readRepoFile('remote/argocd/vpn/dd-vpn.networkpolicy.yaml');
  const readme = await readRepoFile('remote/argocd/vpn/readme.md');

  assert.match(app, /name:\s*dd-vpn/);
  assert.match(app, /path:\s*remote\/argocd\/vpn/);
  assert.match(app, /namespace:\s*vpn/);
  assert.match(app, /ServerSideApply=true/);

  for (const resource of [
    'namespace.yaml',
    'dd-vpn.serviceaccount.yaml',
    'dd-vpn.configmap.yaml',
    'dd-vpn-secrets.externalsecret.yaml',
    'dd-vpn.pvc.yaml',
    'dd-vpn.deployment.yaml',
    'dd-vpn-ui.service.yaml',
    'dd-vpn.networkpolicy.yaml',
  ]) {
    assert.match(kustomization, new RegExp(resource.replaceAll('.', '\\.')));
  }

  assert.match(config, /INIT_HOST:\s*"54\.91\.17\.58"/);
  assert.match(config, /INIT_PORT:\s*"51820"/);
  assert.match(config, /INIT_DNS:\s*"10\.96\.0\.10,1\.1\.1\.1"/);
  assert.match(config, /INIT_ALLOWED_IPS:\s*"10\.8\.0\.0\/24,10\.96\.0\.0\/12,10\.244\.0\.0\/16"/);
  assert.match(config, /INSECURE:\s*"true"/);
  assert.match(config, /DISABLE_IPV6:\s*"true"/);

  assert.match(externalSecret, /kind:\s*ExternalSecret/);
  assert.match(externalSecret, /name:\s*dd-vpn-secrets/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/vpn-secrets/);

  assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  assert.match(deployment, /image:\s*ghcr\.io\/wg-easy\/wg-easy:15/);
  assert.match(deployment, /image:\s*busybox:1\.37/);
  assert.match(deployment, /sysctl -w net\.ipv4\.ip_forward=1/);
  assert.match(deployment, /privileged:\s*true/);
  assert.match(deployment, /NET_ADMIN/);
  assert.match(deployment, /SYS_MODULE/);
  assert.match(deployment, /hostPort:\s*51820/);
  assert.match(deployment, /protocol:\s*UDP/);
  assert.match(deployment, /mountPath:\s*\/etc\/wireguard/);
  assert.match(deployment, /path:\s*\/lib\/modules/);
  assert.match(deployment, /INIT_USERNAME[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-vpn-secrets/);
  assert.match(deployment, /INIT_PASSWORD[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-vpn-secrets/);
  assert.match(deployment, /startupProbe:[\s\S]*port:\s*http/);

  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /name:\s*http[\s\S]*port:\s*51821/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*- Ingress/);
  assert.match(networkPolicy, /protocol:\s*UDP[\s\S]*port:\s*51820/);
  assert.match(networkPolicy, /protocol:\s*TCP[\s\S]*port:\s*51821/);

  assert.match(readme, /WireGuard VPN endpoint/);
  assert.match(readme, /Open UDP `51820`/);
  assert.match(readme, /VPC-like overlay/);
  assert.match(readme, /does not create or manage AWS VPC resources/);
});
