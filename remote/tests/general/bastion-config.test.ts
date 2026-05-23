import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/bastion-rs/src/main.rs'))) {
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
  const source = await readRepoFile('remote/deployments/bastion-rs/src/main.rs');
  const cargo = await readRepoFile('remote/deployments/bastion-rs/Cargo.toml');
  const dockerfile = await readRepoFile('remote/deployments/bastion-rs/Dockerfile');
  const readme = await readRepoFile('remote/deployments/bastion-rs/readme.md');

  assert.match(cargo, /name = "dd-bastion"/);
  assert.match(cargo, /axum/);
  assert.match(cargo, /base64/);
  assert.match(dockerfile, /rust:1\.90-bookworm/);
  assert.doesNotMatch(dockerfile, /util-linux/);
  assert.match(dockerfile, /dd-bastion/);

  assert.match(source, /const SERVICE_NAME: &str = "dd-bastion"/);
  assert.match(source, /DEFAULT_PORT: u16 = 8111/);
  assert.match(source, /BASTION_AUTH_SECRET/);
  assert.match(source, /SERVER_AUTH_SECRET/);
  assert.match(source, /x-bastion-auth/);
  assert.match(source, /AUTHORIZATION/);
  assert.match(source, /constant_time_eq/);
  assert.match(source, /CACHE_CONTROL/);
  assert.match(source, /no-store/);
  assert.match(source, /x-content-type-options/);
  assert.match(source, /route\("\/profile"/);
  assert.match(source, /route\("\/config"/);
  assert.match(source, /route\("\/kubeconfig"/);
  assert.match(source, /route\("\/runtime\/deployments"/);
  assert.match(source, /route\("\/terminal"/);
  assert.match(source, /route\("\/terminal\/ws"/);
  assert.match(source, /const MANAGED_DEPLOYMENTS/);
  assert.match(source, /dd-lock-loadtest-trigger/);
  assert.match(source, /dd-container-pool/);
  assert.match(source, /dd-gleamlang-server/);
  assert.match(source, /dd-webrtc-signaling/);
  assert.match(source, /dd-fsharp-ws-server/);
  assert.match(source, /dd-ws-loadtest-rs/);
  assert.match(source, /dd-gleamlang-ws-loadtest/);
  assert.match(source, /WebSocketUpgrade/);
  assert.match(source, /kubectl/);
  assert.match(source, /BASTION_SCRIPT_BIN/);
  assert.match(source, /pty-script-kubectl/);
  assert.match(source, /BASTION_TERMINAL_ENABLED", false/);
  assert.match(source, /dd-vpn-readonly/);
  assert.match(source, /dd-bastion-readonly/);
  assert.match(cargo, /features = \["ws"\]/);
  assert.match(source, /certificate-authority-data/);
  assert.match(source, /read-only service account token/);
  assert.match(readme, /authenticated HTTP service/);
  assert.match(readme, /kubeconfig/);
  assert.match(readme, /runtime\/deployments/);
  assert.match(readme, /Rust WebRTC and Gleam WebSocket surfaces/);
  assert.match(readme, /disabled by default/);
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
  const ec2Readme = await readRepoFile('remote/ec2/README.md');

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
  assert.match(rbac, /kind:\s*ClusterRole[\s\S]*name:\s*dd-bastion-readonly/);
  assert.match(rbac, /resources:[\s\S]*-\s*pods[\s\S]*-\s*services[\s\S]*-\s*deployments/);
  assert.match(rbac, /verbs:[\s\S]*-\s*get[\s\S]*-\s*list[\s\S]*-\s*watch/);
  // Read-only access to metrics-server and kubectl logs for the live
  // container cards on the homepage.
  assert.match(rbac, /apiGroups:\s*\[metrics\.k8s\.io\][\s\S]*resources:[\s\S]*-\s*pods/);
  assert.match(rbac, /resources:[\s\S]*-\s*pods\/log[\s\S]*verbs:[\s\S]*-\s*get/);
  // Exec is allowed only via the dedicated dd-bastion-exec role; the
  // read-only role must not gain mutation verbs.
  assert.match(rbac, /kind:\s*ClusterRole[\s\S]*name:\s*dd-bastion-exec[\s\S]*pods\/exec[\s\S]*verbs:[\s\S]*-\s*create/);
  assert.match(rbac, /kind:\s*ClusterRoleBinding[\s\S]*name:\s*dd-bastion-exec[\s\S]*name:\s*dd-bastion-exec/);
  // Hardened defaults: no Secret access, no mutation verbs other than
  // `pods/exec` create above.
  assert.doesNotMatch(rbac, /-\s*secrets\b/);
  assert.doesNotMatch(rbac, /-\s*patch\b/);
  assert.doesNotMatch(rbac, /-\s*update\b/);
  assert.doesNotMatch(rbac, /-\s*delete\b/);

  assert.match(externalSecret, /name:\s*dd-bastion-secrets/);
  assert.match(externalSecret, /key:\s*dd\/remote-dev\/agent-secrets/);
  assert.match(externalSecret, /property:\s*SERVER_AUTH_SECRET/);
  assert.doesNotMatch(externalSecret, /dataFrom:/);

  assert.match(deployment, /name:\s*dd-bastion/);
  assert.match(deployment, /serviceAccountName:\s*dd-bastion/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/bastion-rs/);
  assert.match(deployment, /export PATH=\/usr\/local\/cargo\/bin/);
  assert.match(deployment, /PATH[\s\S]*\/usr\/local\/cargo\/bin/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8111'/);
  assert.match(deployment, /CARGO_HOME[\s\S]*\/tmp\/cargo-home/);
  assert.match(deployment, /CARGO_TARGET_DIR[\s\S]*\/tmp\/dd-bastion-target/);
  assert.match(deployment, /BASTION_PUBLIC_BASE_URL[\s\S]*dd-bastion\.vpn\.svc\.cluster\.local:8111/);
  assert.match(deployment, /BASTION_WIREGUARD_ENDPOINT[\s\S]*54\.91\.17\.58:51820/);
  assert.match(deployment, /BASTION_SERVICE_CIDR[\s\S]*10\.96\.0\.0\/12/);
  assert.match(deployment, /BASTION_POD_CIDR[\s\S]*10\.244\.0\.0\/16/);
  assert.match(deployment, /BASTION_KUBECONFIG_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /BASTION_KUBECTL_BIN[\s\S]*value:\s*\/usr\/bin\/kubectl/);
  assert.doesNotMatch(deployment, /BASTION_SCRIPT_BIN/);
  // The browser terminal is enabled by default in this deployment; the
  // matching pods/exec verb is granted only by ClusterRole dd-bastion-exec.
  assert.match(deployment, /BASTION_TERMINAL_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-bastion-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /readOnlyRootFilesystem:\s*true/);
  assert.match(deployment, /runAsNonRoot:\s*true/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /mountPath:\s*\/tmp[\s\S]*emptyDir:\s*\{\}/);
  assert.match(deployment, /mountPath:\s*\/usr\/bin\/kubectl[\s\S]*path:\s*\/usr\/bin\/kubectl/);
  assert.doesNotMatch(deployment, /mountPath:\s*\/usr\/bin\/script/);

  assert.match(service, /type:\s*ClusterIP/);
  assert.match(service, /name:\s*http[\s\S]*port:\s*8111/);
  assert.doesNotMatch(service, /NodePort|LoadBalancer|hostPort/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /port:\s*8111/);

  assert.match(vpnReadme, /Bastion\/access broker/);
  assert.match(vpnReadme, /runtime\/deployments/);
  assert.match(vpnReadme, /dd-bastion-exec/);
  assert.match(vpnReadme, /metrics-server/);
  assert.match(vpnReadme, /not a public MCP server that can mint AWS access/);
  assert.match(vpnReadme, /AWS credentials stay\s+in AWS Secrets Manager/);
  assert.match(ec2Readme, /inbound UDP `51820`/);
  assert.match(ec2Readme, /Do not use a public MCP endpoint as a password-to-SSH or password-to-AWS bridge/);
  assert.match(remoteReadme, /argocd\/vpn/);
  assert.match(remoteReadme, /Rust `dd-bastion` access/);
});
