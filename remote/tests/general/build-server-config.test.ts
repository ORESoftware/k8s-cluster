import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/build-server-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust build server queues controlled image builds and deploys', async () => {
  const cargoToml = await readRepoFile('remote/deployments/build-server-rs/Cargo.toml');
  const source = await readRepoFile('remote/deployments/build-server-rs/src/main.rs');
  const readme = await readRepoFile('remote/deployments/build-server-rs/readme.md');

  assert.match(cargoToml, /name = "dd-build-server"/);
  assert.match(cargoToml, /axum/);
  assert.match(cargoToml, /base64/);
  assert.match(cargoToml, /hmac/);
  assert.match(cargoToml, /reqwest/);
  assert.match(cargoToml, /sha2/);
  assert.match(cargoToml, /tokio/);
  assert.match(source, /const SERVICE_NAME: &str = "dd-build-server"/);
  assert.match(source, /POST \/builds/);
  assert.match(source, /GET \/builds\/<jobId>\/logs/);
  assert.match(source, /schemaVersion/);
  assert.match(source, /jobKind/);
  assert.match(source, /BUILD_SERVER_ALLOWED_REPO_PREFIXES/);
  assert.match(source, /BUILD_SERVER_ALLOWED_IMAGE_PREFIXES/);
  assert.match(source, /BUILD_SERVER_ALLOWED_NAMESPACES/);
  assert.match(source, /BUILD_SERVER_PUSH_ENABLED/);
  assert.match(source, /BUILD_SERVER_ECR_LOGIN_ENABLED/);
  assert.match(source, /BUILD_SERVER_DEPLOY_ENABLED/);
  assert.match(source, /BUILD_SERVER_MAX_CONCURRENT_BUILDS/);
  assert.match(source, /BUILD_SERVER_MAX_LOG_BYTES/);
  assert.match(source, /request_is_authorized/);
  assert.match(source, /x-server-auth/);
  assert.match(source, /repoUrl must use https:\/\/, ssh:\/\/, or git@/);
  assert.match(source, /image must include an explicit tag or digest/);
  assert.match(source, /push currently requires an Amazon ECR image/);
  assert.match(source, /AmazonEC2ContainerRegistry_V20150921\.GetAuthorizationToken/);
  assert.match(source, /x-amz-target/);
  assert.match(source, /--password-stdin/);
  assert.match(source, /redacted_build_args/);
  assert.match(source, /GIT_TERMINAL_PROMPT/);
  assert.match(source, /\.env_clear\(\)/);
  assert.match(source, /dd_build_server_ecr_logins_total/);
  assert.match(source, /deploy\.kind must be one of: kustomize, manifest, none/);
  assert.match(source, /validate_relative_path/);
  assert.match(source, /Component::ParentDir/);
  assert.match(source, /Command::new\(program\)/);
  assert.match(source, /"clone"/);
  assert.match(source, /"build"/);
  assert.match(source, /"apply"/);
  assert.match(source, /"rollout"/);
  assert.match(source, /dd_build_server_jobs_submitted_total/);
  assert.doesNotMatch(source, /\/bin\/bash/);
  assert.match(readme, /does not accept arbitrary shell commands/);
  assert.match(readme, /not a fully untrusted code sandbox/);
  assert.match(readme, /`deploy.kind`: `kustomize`, `manifest`, or `none`/);
  assert.match(readme, /ECR push support is enabled/);
  assert.match(readme, /`SERVER_AUTH_SECRET` must come from `dd-agent-secrets`/);
});

test('build server is deployed through Argo runtime manifests, gateway, and observability', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-build-server.service.yaml');
  const rbac = await readRepoFile('remote/argocd/dd-next-runtime/dd-build-server-rbac.yaml');
  const networkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-build-server.networkpolicy.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-build-server/);
  assert.match(deployment, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
  assert.match(deployment, /serviceAccountName:\s*dd-build-server/);
  assert.match(deployment, /allowPrivilegeEscalation:\s*false/);
  assert.match(deployment, /capabilities:[\s\S]*drop:[\s\S]*- ALL/);
  assert.match(deployment, /seccompProfile:[\s\S]*type:\s*RuntimeDefault/);
  assert.match(deployment, /cd "\$source_root\/remote\/deployments\/build-server-rs"/);
  assert.match(deployment, /CARGO_TARGET_DIR[\s\S]*\/tmp\/dd-build-server-target/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8100'/);
  assert.match(deployment, /BUILD_SERVER_WORK_ROOT[\s\S]*\/var\/lib\/dd-build-server\/jobs/);
  assert.match(deployment, /BUILD_SERVER_NERDCTL_BIN[\s\S]*\/usr\/local\/bin\/nerdctl/);
  assert.match(deployment, /BUILD_SERVER_KUBECTL_BIN[\s\S]*\/usr\/bin\/kubectl/);
  assert.match(deployment, /BUILD_SERVER_CONTAINERD_NAMESPACE[\s\S]*value:\s*k8s\.io/);
  assert.match(deployment, /BUILD_SERVER_ALLOWED_REPO_PREFIXES[\s\S]*https:\/\/github\.com\//);
  assert.match(
    deployment,
    /BUILD_SERVER_ALLOWED_IMAGE_PREFIXES[\s\S]*710156900967\.dkr\.ecr\.us-east-1\.amazonaws\.com\//,
  );
  assert.match(deployment, /BUILD_SERVER_ALLOWED_NAMESPACES[\s\S]*value:\s*default/);
  assert.match(deployment, /BUILD_SERVER_DEPLOY_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /BUILD_SERVER_PUSH_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /BUILD_SERVER_ECR_LOGIN_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /BUILD_SERVER_MAX_CONCURRENT_BUILDS[\s\S]*value:\s*'1'/);
  assert.match(deployment, /AWS_REGION[\s\S]*value:\s*us-east-1/);
  assert.match(deployment, /AWS_ACCESS_KEY_ID[\s\S]*optional:\s*true/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /mountPath:\s*\/run\/containerd\/containerd\.sock/);
  assert.match(deployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.match(deployment, /mountPath:\s*\/usr\/bin\/kubectl/);
  assert.match(deployment, /mountPath:\s*\/opt\/dd-next-1[\s\S]*readOnly:\s*true/);
  assert.match(deployment, /mountPath:\s*\/tmp/);
  assert.match(deployment, /path:\s*\/var\/lib\/dd-build-server/);
  assert.match(service, /name:\s*dd-build-server/);
  assert.match(service, /port:\s*8100/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(rbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-build-server/);
  assert.match(rbac, /name:\s*dd-build-server-deployer/);
  assert.match(rbac, /resources: \[configmaps, services\]/);
  assert.match(rbac, /resources: \[events\]/);
  assert.match(rbac, /resources: \[deployments\]/);
  assert.match(rbac, /resources: \[horizontalpodautoscalers\]/);
  assert.match(rbac, /resources: \[ingresses\]/);
  assert.doesNotMatch(rbac, /resources: \[secrets\]/);
  assert.doesNotMatch(rbac, /resources: \[pods/);
  assert.doesNotMatch(rbac, /serviceaccounts/);
  assert.doesNotMatch(rbac, /daemonsets/);
  assert.doesNotMatch(rbac, /statefulsets/);
  assert.doesNotMatch(rbac, /cronjobs/);
  assert.doesNotMatch(rbac, /networkpolicies/);
  assert.match(kustomization, /dd-build-server-rbac\.yaml/);
  assert.match(kustomization, /dd-build-server\.deployment\.yaml/);
  assert.match(kustomization, /dd-build-server\.service\.yaml/);
  assert.match(kustomization, /dd-build-server\.networkpolicy\.yaml/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /podSelector:[\s\S]*app:\s*dd-build-server/);
  assert.match(networkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*observability/);
  assert.match(networkPolicy, /port:\s*8100/);
  assert.match(
    gateway,
    /location = \/builds[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-build-server\.default\.svc\.cluster\.local:8100/,
  );
  assert.match(
    gateway,
    /location \/builds\/[\s\S]*proxy_read_timeout 1800[\s\S]*dd-build-server\.default\.svc\.cluster\.local:8100/,
  );
  assert.match(
    prometheus,
    /job_name:\s*dd-build-server[\s\S]*dd-build-server\.default\.svc\.cluster\.local:8100/,
  );
  assert.match(
    otel,
    /job_name:\s*dd-build-server[\s\S]*dd-build-server\.default\.svc\.cluster\.local:8100/,
  );
  assert.match(home, /dd-build-server:8100/);
  assert.match(home, /POST \/builds/);
  assert.match(home, /build-server\.v1/);
  assert.match(runtimeReadme, /`dd-build-server`/);
  assert.match(runtimeReadme, /`BUILD_SERVER_MAX_CONCURRENT_BUILDS=1`/);
  assert.match(runtimeReadme, /`BUILD_SERVER_ALLOWED_NAMESPACES`/);
  assert.match(runtimeReadme, /`BUILD_SERVER_ALLOWED_IMAGE_PREFIXES`/);
  assert.match(runtimeReadme, /`BUILD_SERVER_ECR_LOGIN_ENABLED=true`/);
});
