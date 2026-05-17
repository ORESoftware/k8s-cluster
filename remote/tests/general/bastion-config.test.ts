import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/bastion-rs/src/main.rs'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust bastion serves authenticated vpn cluster access profile and kubeconfig', async () => {
  const source = await readRepoFile('remote/bastion-rs/src/main.rs');
  const cargo = await readRepoFile('remote/bastion-rs/Cargo.toml');
  const dockerfile = await readRepoFile('remote/bastion-rs/Dockerfile');
  const readme = await readRepoFile('remote/bastion-rs/readme.md');

  assert.match(cargo, /name = "dd-bastion"/);
  assert.match(cargo, /axum/);
  assert.match(cargo, /base64/);
  assert.match(dockerfile, /rust:1\.90-bookworm/);
  assert.match(dockerfile, /util-linux/);
  assert.match(dockerfile, /dd-bastion/);

  assert.match(source, /const SERVICE_NAME: &str = "dd-bastion"/);
  assert.match(source, /DEFAULT_PORT: u16 = 8111/);
  assert.match(source, /BASTION_AUTH_SECRET/);
  assert.match(source, /SERVER_AUTH_SECRET/);
  assert.match(source, /x-bastion-auth/);
  assert.match(source, /AUTHORIZATION/);
  assert.match(source, /route\("\/profile"/);
  assert.match(source, /route\("\/config"/);
  assert.match(source, /route\("\/kubeconfig"/);
  assert.match(source, /route\("\/runtime\/deployments"/);
  assert.match(source, /route\("\/terminal"/);
  assert.match(source, /route\("\/terminal\/ws"/);
  assert.match(source, /const MANAGED_DEPLOYMENTS/);
  assert.match(source, /dd-lock-loadtest-trigger/);
  assert.match(source, /dd-container-pool/);
  assert.match(source, /WebSocketUpgrade/);
  assert.match(source, /kubectl/);
  assert.match(source, /BASTION_SCRIPT_BIN/);
  assert.match(source, /pty-script-kubectl/);
  assert.match(cargo, /features = \["ws"\]/);
  assert.match(source, /certificate-authority-data/);
  assert.match(source, /access-broker service account token/);
  assert.match(readme, /authenticated HTTP service/);
  assert.match(readme, /kubeconfig/);
  assert.match(readme, /runtime\/deployments/);
  assert.match(readme, /browser terminal/);
});

test('vpn bundle deploys bastion as cluster-only access broker and terminal jump host', async () => {
  const kustomization = await readRepoFile('remote/argocd/vpn/kustomization.yaml');
  const rbac = await readRepoFile('remote/argocd/vpn/dd-bastion-rbac.yaml');
  const externalSecret = await readRepoFile(
    'remote/argocd/vpn/dd-bastion-secrets.externalsecret.yaml',
  );
  const deployment = await readRepoFile('remote/argocd/vpn/dd-bastion.deployment.yaml');
  const service = await readRepoFile('remote/argocd/vpn/dd-bastion.service.yaml');
  const networkPolicy = await readRepoFile('remote/argocd/vpn/dd-bastion.networkpolicy.yaml');
  const vpnReadme = await readRepoFile('remote/argocd/vpn/readme.md');
  const remoteReadme = await readRepoFile('remote/readme.md');

  for (const resource of [
    'dd-bastion-rbac.yaml',
    'dd-bastion-secrets.externalsecret.yaml',
    'dd-bastion.deployment.yaml',
    'dd-bastion.service.yaml',
    'dd-bastion.networkpolicy.yaml',
  ]) {
    assert.match(kustomization, new RegExp(resource.replaceAll('.', '\\.')));
  }

  assert.match(rbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-bastion/);
  assert.match(rbac, /kind:\s*ClusterRole[\s\S]*name:\s*dd-bastion-access-broker/);
  assert.match(rbac, /resources:[\s\S]*-\s*pods[\s\S]*-\s*services[\s\S]*-\s*deployments/);
  assert.match(rbac, /resources:[\s\S]*-\s*pods\/exec[\s\S]*verbs:[\s\S]*-\s*create/);
  assert.match(rbac, /verbs:[\s\S]*-\s*get[\s\S]*-\s*list[\s\S]*-\s*watch/);
  assert.doesNotMatch(rbac, /-\s*secrets/);
  assert.doesNotMatch(rbac, /-\s*patch/);
  assert.doesNotMatch(rbac, /-\s*update/);
  assert.doesNotMatch(rbac, /-\s*delete/);

  assert.match(externalSecret, /name:\s*dd-bastion-secrets/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/agent-secrets/);

  assert.match(deployment, /name:\s*dd-bastion/);
  assert.match(deployment, /serviceAccountName:\s*dd-bastion/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/bastion-rs/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8111'/);
  assert.match(deployment, /CARGO_HOME[\s\S]*\/tmp\/cargo-home/);
  assert.match(deployment, /CARGO_TARGET_DIR[\s\S]*\/tmp\/dd-bastion-target/);
  assert.match(deployment, /BASTION_PUBLIC_BASE_URL[\s\S]*dd-bastion\.vpn\.svc\.cluster\.local:8111/);
  assert.match(deployment, /BASTION_WIREGUARD_ENDPOINT[\s\S]*54\.91\.17\.58:51820/);
  assert.match(deployment, /BASTION_SERVICE_CIDR[\s\S]*10\.96\.0\.0\/12/);
  assert.match(deployment, /BASTION_POD_CIDR[\s\S]*10\.244\.0\.0\/16/);
  assert.match(deployment, /BASTION_KUBECONFIG_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /BASTION_KUBECTL_BIN[\s\S]*value:\s*\/usr\/bin\/kubectl/);
  assert.match(deployment, /BASTION_SCRIPT_BIN[\s\S]*value:\s*\/usr\/bin\/script/);
  assert.match(deployment, /BASTION_TERMINAL_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-bastion-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /mountPath:\s*\/tmp[\s\S]*emptyDir:\s*\{\}/);
  assert.match(deployment, /mountPath:\s*\/usr\/bin\/kubectl[\s\S]*path:\s*\/usr\/bin\/kubectl/);
  assert.match(deployment, /mountPath:\s*\/usr\/bin\/script[\s\S]*path:\s*\/usr\/bin\/script/);

  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /name:\s*http[\s\S]*port:\s*8111/);
  assert.doesNotMatch(service, /NodePort|LoadBalancer|hostPort/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /port:\s*8111/);

  assert.match(vpnReadme, /Bastion\/access broker/);
  assert.match(vpnReadme, /runtime\/deployments/);
  assert.match(vpnReadme, /allowlisted browser terminal/);
  assert.match(remoteReadme, /argocd\/vpn/);
  assert.match(remoteReadme, /Rust `dd-bastion` access/);
});
