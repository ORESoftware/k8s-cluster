import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/container-pool-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust container pool reads Postgres config and dispatches over HTTP or NATS', async () => {
  const cargoToml = await readRepoFile('remote/container-pool-rs/Cargo.toml');
  const source = await readRepoFile('remote/container-pool-rs/src/main.rs');
  const readme = await readRepoFile('remote/container-pool-rs/readme.md');
  const appConfigTableSql = await readRepoFile('remote/databases/pg/tables/app-config-table.sql');
  const appConfigSeedSql = await readRepoFile(
    'remote/databases/pg/seeds/container-pool-app-config.sql',
  );
  const tableSql = await readRepoFile(
    'remote/databases/pg/tables/container-pool-configs-table.sql',
  );

  assert.match(cargoToml, /name = "dd-container-pool"/);
  assert.match(cargoToml, /async-nats/);
  assert.match(cargoToml, /tokio-postgres/);
  assert.match(cargoToml, /reqwest/);
  assert.match(source, /const SERVICE_NAME: &str = "dd-container-pool"/);
  assert.match(source, /from app_config/);
  assert.match(source, /CONTAINER_POOL_APP_CONFIG_KEY/);
  assert.match(source, /container-pool\.runtime-pools\.v1/);
  assert.match(source, /from container_pool_configs/);
  assert.match(source, /CONTAINER_POOL_DATABASE_URL/);
  assert.match(source, /AGENT_TASKS_RDS_DATABASE_URL/);
  assert.match(source, /CONTAINER_POOL_CONFIG_JSON/);
  assert.match(source, /CONTAINER_POOL_NATS_SUBJECT/);
  assert.match(source, /CONTAINER_POOL_START_TIMEOUT_SECONDS/);
  assert.match(source, /dd\.remote\.container_pool\.requests/);
  assert.match(source, /dd\.remote\.container_pool\.results/);
  assert.match(source, /route\("\/pools\/:pool\/dispatch", post\(dispatch_pool\)\)/);
  assert.match(source, /route\("\/pools\/:pool\/warm", post\(warm_pool\)\)/);
  assert.match(source, /request_is_authorized/);
  assert.match(source, /x-container-pool-auth/);
  assert.match(source, /Command::new\(program\)\.args\(args\)\.output\(\)/);
  assert.match(source, /"run"\.to_string\(\)/);
  assert.match(source, /wait_container_ready/);
  assert.match(source, /"--network"\.to_string\(\)/);
  assert.match(source, /"--label"\.to_string\(\)/);
  assert.match(source, /dd\.container-pool\.managed=true/);
  assert.match(source, /max_concurrency_per_container/);
  assert.match(source, /dd_container_pool_dispatch_total/);
  assert.doesNotMatch(source, /\/bin\/bash/);
  assert.match(readme, /reads active pool definitions from Postgres/);
  assert.match(readme, /app_config/);
  assert.match(readme, /NATS requests on `CONTAINER_POOL_NATS_SUBJECT`/);
  assert.match(readme, /never\s+accepts arbitrary commands from dispatch requests/);
  assert.match(appConfigTableSql, /create table if not exists app_config/);
  assert.match(appConfigTableSql, /key varchar\(200\) not null/);
  assert.match(appConfigTableSql, /value jsonb not null/);
  assert.match(appConfigTableSql, /app_config_scope_key_uq/);
  assert.match(appConfigSeedSql, /container-pool\.runtime-pools\.v1/);
  assert.match(appConfigSeedSql, /"baseImages": \[/);
  assert.match(appConfigSeedSql, /"dockerfile": "remote\/container-pool-rs\/runtime-images\/nodejs\.Dockerfile"/);
  assert.match(appConfigSeedSql, /dd-container-pool-nodejs-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-rust-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-golang-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-python3-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-dart-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-gleamlang-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-erlang-runtime:dev/);
  assert.match(appConfigSeedSql, /on conflict \(scope, key\) do update/);
  assert.match(tableSql, /create table if not exists container_pool_configs/);
  assert.match(tableSql, /slug varchar\(120\) not null/);
  assert.match(tableSql, /image text not null/);
  assert.match(tableSql, /command jsonb not null default '\[\]'::jsonb/);
  assert.match(tableSql, /env jsonb not null default '\{\}'::jsonb/);
  assert.match(tableSql, /min_warm integer not null default 1/);
  assert.match(tableSql, /max_warm integer not null default 2/);
  assert.match(tableSql, /nats_subject text/);
});

test('container pool is deployed through Argo, gateway, and metrics scraping', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-container-pool.deployment.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-container-pool.service.yaml');
  const rbac = await readRepoFile('remote/argocd/dd-next-runtime/dd-container-pool-rbac.yaml');
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');

  assert.match(deployment, /name:\s*dd-container-pool/);
  assert.match(deployment, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
  assert.match(deployment, /serviceAccountName:\s*dd-container-pool/);
  assert.match(deployment, /hostNetwork:\s*true/);
  assert.match(deployment, /dnsPolicy:\s*ClusterFirstWithHostNet/);
  assert.match(deployment, /securityContext:\s*\n\s*privileged:\s*true/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/container-pool-rs/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8102'/);
  assert.match(
    deployment,
    /CONTAINER_POOL_NATS_SUBJECT[\s\S]*dd\.remote\.container_pool\.\*\.requests/,
  );
  assert.match(deployment, /CONTAINER_POOL_NERDCTL_BIN[\s\S]*\/usr\/local\/bin\/nerdctl/);
  assert.match(deployment, /CONTAINER_POOL_CONTAINERD_NAMESPACE[\s\S]*value:\s*k8s\.io/);
  assert.match(deployment, /CONTAINER_POOL_APP_CONFIG_SCOPE[\s\S]*value:\s*default/);
  assert.match(
    deployment,
    /CONTAINER_POOL_APP_CONFIG_KEY[\s\S]*value:\s*container-pool\.runtime-pools\.v1/,
  );
  assert.match(deployment, /CONTAINER_POOL_NETWORK[\s\S]*value:\s*host/);
  assert.match(deployment, /CONTAINER_POOL_PORT_START[\s\S]*value:\s*'12000'/);
  assert.match(deployment, /SERVER_AUTH_SECRET[\s\S]*dd-agent-secrets[\s\S]*SERVER_AUTH_SECRET/);
  assert.match(deployment, /dd-remote-rest-api-secrets/);
  assert.match(deployment, /dd-container-pool-secrets/);
  assert.match(deployment, /mountPath:\s*\/run\/containerd\/containerd\.sock/);
  assert.match(deployment, /mountPath:\s*\/var\/lib\/containerd/);
  assert.match(deployment, /mountPropagation:\s*Bidirectional/);
  assert.match(deployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.match(service, /name:\s*dd-container-pool/);
  assert.match(service, /port:\s*8102/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(rbac, /kind:\s*ServiceAccount[\s\S]*name:\s*dd-container-pool/);
  assert.match(kustomization, /dd-container-pool-rbac\.yaml/);
  assert.match(kustomization, /dd-container-pool\.deployment\.yaml/);
  assert.match(kustomization, /dd-container-pool\.service\.yaml/);
  assert.match(
    gateway,
    /location = \/container-pools[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-container-pool\.default\.svc\.cluster\.local:8102\/pools/,
  );
  assert.match(
    gateway,
    /location \/container-pools\/[\s\S]*rewrite \^\/container-pools\/\?\(\.\*\)\$ \/pools\/\$1 break[\s\S]*dd-container-pool\.default\.svc\.cluster\.local:8102/,
  );
  assert.match(
    prometheus,
    /job_name:\s*dd-container-pool[\s\S]*dd-container-pool\.default\.svc\.cluster\.local:8102/,
  );
  assert.match(
    otel,
    /job_name:\s*dd-container-pool[\s\S]*dd-container-pool\.default\.svc\.cluster\.local:8102/,
  );
  assert.match(runtimeReadme, /`dd-container-pool`/);
  assert.match(runtimeReadme, /`app_config`/);
  assert.match(runtimeReadme, /`container_pool_configs`/);
  assert.match(runtimeReadme, /dd\.remote\.container_pool\.\*\.requests/);
});

test('container pool runtime base images cover the supported language pools', async () => {
  const worker = await readRepoFile('remote/container-pool-rs/runtime-images/common/worker.py');
  const shim = await readRepoFile('remote/container-pool-rs/scripts/nerdctl-process-shim.py');
  const runtimeReadme = await readRepoFile('remote/container-pool-rs/runtime-images/readme.md');
  const dockerfiles = new Map(
    await Promise.all(
      ['nodejs', 'rust', 'golang', 'python3', 'dart', 'gleamlang', 'erlang'].map(
        async (runtime) =>
          [
            runtime,
            await readRepoFile(`remote/container-pool-rs/runtime-images/${runtime}.Dockerfile`),
          ] as const,
      ),
    ),
  );

  assert.match(worker, /ThreadingHTTPServer/);
  assert.match(worker, /DD_POOL_HANDLER/);
  assert.match(worker, /echo_key = request_payload\.get\("echoKey"\)/);
  assert.match(worker, /def do_POST\(self\):/);
  assert.match(worker, /self\.path != "\/invoke"/);
  assert.match(worker, /subprocess\.run/);
  assert.match(shim, /Development-only nerdctl shim/);
  assert.match(shim, /"--publish"/);
  assert.match(shim, /published_host_port/);
  assert.match(runtimeReadme, /nodejs/);
  assert.match(runtimeReadme, /gleamlang/);

  for (const [runtime, dockerfile] of dockerfiles) {
    assert.match(dockerfile, / AS /, `${runtime} image should be multi-stage`);
    assert.match(dockerfile, /worker\.py/, `${runtime} image should include the common worker`);
    assert.match(
      dockerfile,
      /ENTRYPOINT \["python3", "\/opt\/dd-container-pool\/worker\.py"\]/,
      `${runtime} image should start the HTTP worker`,
    );
    assert.match(dockerfile, new RegExp(`DD_POOL_RUNTIME=${runtime}`));
    assert.match(
      runtimeReadme,
      new RegExp(`dd-container-pool-${runtime}-runtime:dev`),
      `${runtime} image tag should be documented`,
    );
  }

  assert.match(dockerfiles.get('nodejs') ?? '', /nodejs-current/);
  assert.match(dockerfiles.get('rust') ?? '', /rust:1\.90-alpine AS build/);
  assert.match(dockerfiles.get('golang') ?? '', /golang:1\.25-alpine AS build/);
  assert.match(dockerfiles.get('python3') ?? '', /python:3\.13-alpine/);
  assert.match(dockerfiles.get('dart') ?? '', /dart compile exe/);
  assert.match(dockerfiles.get('gleamlang') ?? '', /gleam:v1\.16\.0-erlang-alpine/);
  assert.match(dockerfiles.get('erlang') ?? '', /erlang:28-alpine/);
});
