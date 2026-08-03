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

test('vpn app deploys wg-easy wireguard with private admin UI and mobile Argo CD access', async () => {
  const app = await readRepoFile('remote/argocd/apps/dd-vpn.application.yaml');
  const kustomization = await readRepoFile('remote/argocd/vpn/kustomization.yaml');
  const config = await readRepoFile('remote/argocd/vpn/dd-vpn.configmap.yaml');
  const mobileConfig = await readRepoFile(
    'remote/argocd/vpn/dd-argocd-mobile.configmap.yaml',
  );
  const mobileCertificate = await readRepoFile(
    'remote/argocd/vpn/dd-argocd-mobile.certificate.yaml',
  );
  const mobileArgoConfig = await readRepoFile(
    'remote/argocd/vpn/dd-argocd-mobile.argocd-config.yaml',
  );
  const mobilePrereqs = await readRepoFile(
    'remote/argocd/vpn/dd-argocd-mobile-prereqs.application.yaml',
  );
  const externalSecret = await readRepoFile(
    'remote/argocd/vpn/dd-vpn-secrets.externalsecret.yaml',
  );
  const persistentVolume = await readRepoFile('remote/argocd/vpn/dd-vpn.pv.yaml');
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
    'dd-vpn.pv.yaml',
    'dd-vpn.pvc.yaml',
    'dd-argocd-mobile-prereqs.application.yaml',
    'dd-argocd-mobile.configmap.yaml',
    'dd-argocd-mobile.certificate.yaml',
    'dd-argocd-mobile.argocd-config.yaml',
    'dd-vpn.deployment.yaml',
    'dd-vpn-ui.service.yaml',
    'dd-vpn.networkpolicy.yaml',
  ]) {
    assert.match(kustomization, new RegExp(resource.replaceAll('.', '\\.')));
  }

  assert.match(config, /INIT_HOST:\s*"[A-Za-z0-9.-]+"/);
  assert.match(config, /INIT_PORT:\s*"51820"/);
  assert.match(config, /INIT_DNS:\s*"10\.8\.0\.1"/);
  assert.doesNotMatch(config, /INIT_DNS:[^\n]*1\.1\.1\.1/);
  assert.match(
    config,
    /INIT_ALLOWED_IPS:\s*"10\.8\.0\.0\/24,10\.96\.0\.0\/12,10\.244\.0\.0\/16"/,
  );
  assert.match(config, /INSECURE:\s*"true"/);
  assert.match(config, /DISABLE_IPV6:\s*"true"/);

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

  assert.match(mobileCertificate, /kind:\s*Certificate/);
  assert.match(mobileCertificate, /name:\s*argocd-mobile-tls/);
  assert.match(mobileCertificate, /namespace:\s*vpn/);
  assert.match(mobileCertificate, /secretName:\s*argocd-mobile-tls/);
  assert.match(mobileCertificate, /name:\s*letsencrypt-prod-dns01/);
  assert.match(mobileCertificate, /- argocd-vpn\.fiducia\.cloud/);
  assert.match(mobileCertificate, /rotationPolicy:\s*Always/);

  assert.match(mobilePrereqs, /name:\s*argocd-mobile-dns01/);
  assert.match(
    mobilePrereqs,
    /path:\s*remote\/argocd\/cert-manager\/dns01-cloudflare/,
  );
  assert.match(
    mobilePrereqs,
    /include:\s*["']\{cloudflare-api-token\.externalsecret\.yaml,clusterissuer\.yaml\}["']/,
  );
  assert.match(
    mobilePrereqs,
    /exclude:\s*["']\{kustomization\.yaml,gateway-certificate\.yaml\}["']/,
  );
  assert.match(mobilePrereqs, /prune:\s*false/);
  assert.match(mobilePrereqs, /ServerSideApply=true/);

  assert.match(mobileArgoConfig, /name:\s*argocd-cm/);
  assert.match(mobileArgoConfig, /accounts\.argocd-mobile:\s*login/);
  assert.match(mobileArgoConfig, /name:\s*argocd-rbac-cm/);
  assert.match(mobileArgoConfig, /policy\.mobile\.csv:/);
  assert.match(
    mobileArgoConfig,
    /p, argocd-mobile, applications, sync, \*\/\*, allow/,
  );
  assert.match(mobileArgoConfig, /p, argocd-mobile, logs, get, \*\/\*, allow/);
  assert.doesNotMatch(mobileArgoConfig, /argocd-mobile[^\n]*delete/);
  assert.doesNotMatch(mobileArgoConfig, /argocd-mobile[^\n]*exec/);
  assert.doesNotMatch(mobileArgoConfig, /argocd-mobile, applications, update/);
  assert.doesNotMatch(mobileArgoConfig, /argocd-mobile[^\n]*action/);

  assert.match(externalSecret, /kind:\s*ExternalSecret/);
  assert.match(externalSecret, /name:\s*dd-vpn-secrets/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/vpn-secrets/);
  assert.match(externalSecret, /deletionPolicy:\s*Retain/);
  assert.match(externalSecret, /conversionStrategy:\s*Default/);
  assert.match(externalSecret, /decodingStrategy:\s*None/);
  assert.match(externalSecret, /metadataPolicy:\s*None/);

  assert.match(persistentVolume, /kind:\s*PersistentVolume/);
  assert.match(persistentVolume, /name:\s*dd-vpn-config/);
  assert.match(persistentVolume, /persistentVolumeReclaimPolicy:\s*Retain/);
  assert.match(
    persistentVolume,
    /claimRef:[\s\S]*namespace:\s*vpn[\s\S]*name:\s*dd-vpn-config/,
  );
  assert.match(persistentVolume, /path:\s*\/home\/ec2-user\/dd-vpn-config/);

  assert.match(deployment, /strategy:[\s\S]*type:\s*Recreate/);
  assert.match(deployment, /image:\s*ghcr\.io\/wg-easy\/wg-easy:15/);
  assert.match(deployment, /image:\s*busybox:1\.37/);
  assert.match(
    deployment,
    /image:\s*registry\.k8s\.io\/coredns\/coredns:v1\.14\.2/,
  );
  assert.match(
    deployment,
    /image:\s*docker\.io\/library\/nginx:1\.29\.7-alpine/,
  );
  assert.match(deployment, /sysctl -w net\.ipv4\.ip_forward=1/);
  assert.match(deployment, /privileged:\s*true/);
  assert.match(deployment, /NET_ADMIN/);
  assert.match(deployment, /SYS_MODULE/);
  assert.match(deployment, /NET_BIND_SERVICE/);
  assert.match(deployment, /runAsUser:\s*65532/);
  assert.match(deployment, /runAsUser:\s*101/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /hostPort:\s*51820/);
  assert.doesNotMatch(deployment, /hostPort:\s*8443/);
  assert.match(deployment, /name:\s*dns-udp[\s\S]*containerPort:\s*53/);
  assert.match(deployment, /name:\s*dns-tcp[\s\S]*containerPort:\s*53/);
  assert.match(
    deployment,
    /name:\s*argocd-mobile[\s\S]*containerPort:\s*8443/,
  );
  assert.match(
    deployment,
    /secretName:\s*argocd-mobile-tls[\s\S]*optional:\s*true/,
  );
  assert.match(deployment, /sha256sum[\s\S]*nginx -s reload/);
  assert.match(deployment, /mountPath:\s*\/etc\/wireguard/);
  assert.match(deployment, /path:\s*\/lib\/modules/);
  assert.match(
    deployment,
    /INIT_USERNAME[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-vpn-secrets/,
  );
  assert.match(
    deployment,
    /INIT_PASSWORD[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-vpn-secrets/,
  );
  assert.match(deployment, /startupProbe:[\s\S]*port:\s*http/);

  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /name:\s*http[\s\S]*port:\s*51821/);
  assert.match(networkPolicy, /policyTypes:[\s\S]*- Ingress/);
  assert.match(networkPolicy, /protocol:\s*UDP[\s\S]*port:\s*51820/);
  assert.match(networkPolicy, /protocol:\s*TCP[\s\S]*port:\s*51821/);
  assert.match(networkPolicy, /protocol:\s*UDP[\s\S]*port:\s*53/);
  assert.match(networkPolicy, /protocol:\s*TCP[\s\S]*port:\s*53/);
  assert.match(networkPolicy, /protocol:\s*TCP[\s\S]*port:\s*8443/);

  assert.match(readme, /WireGuard VPN endpoint/);
  assert.match(readme, /Open UDP `51820`/);
  assert.match(readme, /Phone setup for Argo CD/);
  assert.match(readme, /Server:\s+https:\/\/argocd-vpn\.fiducia\.cloud:8443/);
  assert.match(readme, /Username:\s+argocd-mobile/);
  assert.match(readme, /does not grant application-spec updates/);
  assert.match(readme, /no whole-pod restart/);
  assert.match(readme, /VPC-like overlay/);
  assert.match(readme, /does not create or manage AWS VPC resources/);
});
