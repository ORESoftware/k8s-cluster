import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/container-pool-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function parseContainerPoolAppConfigSeed(seedSql: string): {
  runtimeContract: {
    defaultRequestPath: string;
    defaultHealthPath: string;
    defaultContainerPort: number;
    managerInjectedEnv: string[];
    natsEventPattern: string;
    natsHeartbeatPattern: string;
  };
  baseImages: Array<{
    runtime: string;
    image: string;
    dockerfile: string;
    buildContext: string;
  }>;
  pools: Array<{
    slug: string;
    image: string;
    env: Record<string, string>;
    requestPath: string;
    healthPath: string;
    containerPort: number;
    readOnly?: boolean;
    user?: string;
    minWarm: number;
    maxWarm: number;
    maxConcurrencyPerContainer: number;
    requestTimeoutMs: number;
    idleTtlSeconds: number;
    natsSubject: string;
  }>;
} {
  const match = seedSql.match(
    /'container-pool\.runtime-pools\.v1',\s*'([\s\S]*?)'::jsonb,\s*1,\s*'active'/,
  );
  assert.ok(match, 'container pool app_config seed should include a JSONB value');
  return JSON.parse(match[1]);
}

test('rust container pool reads Postgres config and dispatches over HTTP or NATS', async () => {
  const cargoToml = await readRepoFile('remote/deployments/container-pool-rs/Cargo.toml');
  const source = await readRepoFile('remote/deployments/container-pool-rs/src/main.rs');
  const readme = await readRepoFile('remote/deployments/container-pool-rs/readme.md');
  // schema/schema.sql is the single source of truth for every shared table
  // (app_config + container_pool_configs + lambda_functions + agent_remote_dev_*),
  // and drives every adapter under remote/libs/pg-defs/generated/. Per-table
  // DDL files were retired in favor of this single contract.
  const schemaSql = await readRepoFile('remote/libs/pg-defs/schema/schema.sql');
  const appConfigSeedSql = await readRepoFile(
    'remote/databases/pg/seeds/container-pool-app-config.sql',
  );

  assert.match(cargoToml, /name = "dd-container-pool"/);
  assert.match(cargoToml, /async-nats/);
  assert.match(cargoToml, /tokio-postgres/);
  assert.match(cargoToml, /rustls-pemfile/);
  assert.match(cargoToml, /reqwest/);
  assert.match(source, /const SERVICE_NAME: &str = "dd-container-pool"/);
  assert.match(source, /from app_config/);
  assert.match(source, /CONTAINER_POOL_APP_CONFIG_KEY/);
  assert.match(source, /container-pool\.runtime-pools\.v1/);
  assert.match(source, /rds-us-east-1-bundle\.pem/);
  assert.match(source, /add_rds_root_certificates\(&mut root_store\)\?/);
  assert.match(source, /from container_pool_configs/);
  assert.match(source, /CONTAINER_POOL_DATABASE_URL/);
  assert.match(source, /AGENT_TASKS_RDS_DATABASE_URL/);
  assert.match(source, /CONTAINER_POOL_CONFIG_JSON/);
  assert.match(source, /CONTAINER_POOL_NATS_SUBJECT/);
  assert.match(source, /CONTAINER_POOL_NATS_MAX_PAYLOAD_BYTES/);
  assert.match(source, /CONTAINER_POOL_WORKER_RESPONSE_MAX_BYTES/);
  assert.match(source, /CONTAINER_POOL_START_TIMEOUT_SECONDS/);
  assert.match(source, /CONTAINER_POOL_CONTAINER_MEMORY/);
  assert.match(source, /CONTAINER_POOL_CONTAINER_CPUS/);
  assert.match(source, /CONTAINER_POOL_PIDS_LIMIT/);
  assert.match(source, /CONTAINER_POOL_NOFILE_LIMIT/);
  assert.match(source, /CONTAINER_POOL_HEALTH_CHECK_SECONDS/);
  assert.match(source, /CONTAINER_POOL_UNHEALTHY_FAILURE_THRESHOLD/);
  assert.match(source, /dd\.remote\.container_pool\.requests/);
  assert.match(source, /dd\.remote\.container_pool\.results/);
  assert.match(source, /route\("\/pools\/:pool\/dispatch", post\(dispatch_pool\)\)/);
  assert.match(source, /route\("\/pools\/:pool\/warm", post\(warm_pool\)\)/);
  assert.match(source, /request_is_authorized/);
  assert.match(source, /x-container-pool-auth/);
  assert.match(source, /Command::new\(program\)\.args\(args\)\.output\(\)/);
  assert.match(source, /"run"\.to_string\(\)/);
  assert.match(source, /container_run_timeout = state\.config\.command_timeout\.min\(Duration::from_secs\(30\)\)/);
  assert.match(source, /wait_container_ready/);
  assert.match(source, /inspect_container_running/);
  assert.match(source, /state\.config\.command_timeout\.min\(Duration::from_secs\(5\)\)/);
  assert.match(source, /container \{\} stopped before readiness at \{url\}/);
  assert.match(source, /retire_stale_starting_containers/);
  assert.match(source, /container\.status == ContainerStatus::Starting/);
  assert.match(source, /retire_stale_starting_containers\(state, Some\(pool_id\)\)\.await/);
  assert.match(source, /retire_stale_starting_containers\(state, None\)\.await/);
  assert.match(source, /"starting container is not running"/);
  assert.match(source, /probe_container_health/);
  assert.match(source, /retire_container/);
  assert.match(source, /prune_unhealthy_containers/);
  assert.match(source, /safe_container_image/);
  assert.match(source, /safe_local_path/);
  assert.match(source, /safe_nats_subject/);
  assert.match(source, /"--network"\.to_string\(\)/);
  assert.match(source, /"--label"\.to_string\(\)/);
  assert.match(source, /"--read-only"\.to_string\(\)/);
  assert.match(source, /"\/tmp:rw,noexec,nosuid,size=64m"\.to_string\(\)/);
  assert.match(source, /"--user"\.to_string\(\)/);
  assert.match(source, /"10001:10001"\.to_string\(\)/);
  assert.match(source, /"--cap-drop"\.to_string\(\)/);
  assert.match(source, /"ALL"\.to_string\(\)/);
  assert.match(source, /"--security-opt"\.to_string\(\)/);
  assert.match(source, /"no-new-privileges"\.to_string\(\)/);
  assert.match(source, /"--pids-limit"\.to_string\(\)/);
  assert.match(source, /"--ulimit"\.to_string\(\)/);
  assert.match(source, /DD_POOL_MAX_BODY_BYTES/);
  assert.match(source, /DD_POOL_HANDLER_TIMEOUT_SECONDS/);
  assert.match(source, /read_limited_response_body/);
  assert.match(source, /content_length\(\)/);
  assert.match(source, /x-container-pool-auth/);
  assert.match(source, /transfer-encoding/);
  assert.match(source, /proxy-/);
  assert.match(source, /dd\.container-pool\.managed=true/);
  assert.match(source, /max_concurrency_per_container/);
  assert.match(source, /available_capacity/);
  assert.match(source, /affinity: HashMap<String, String>/);
  assert.match(source, /affinity_key: Option<String>/);
  assert.match(source, /normalized_affinity_key/);
  assert.match(source, /affinity_map_key/);
  assert.match(source, /remove_affinity_for_container/);
  assert.match(source, /container_can_accept/);
  assert.match(source, /lease_container\([\s\S]*affinity_key: Option<&str>/);
  assert.match(source, /dd_container_pool_dispatch_total/);
  assert.match(source, /dd_container_pool_container_health_checks_total/);
  assert.doesNotMatch(source, /\/bin\/bash/);
  assert.match(readme, /reads active pool definitions from Postgres/);
  assert.match(readme, /keeps at least `min_warm` available request slots/);
  assert.match(readme, /Warm workers are health checked/);
  assert.match(readme, /Worker contract/);
  assert.match(readme, /app_config/);
  assert.match(readme, /NATS requests on `CONTAINER_POOL_NATS_SUBJECT`/);
  assert.match(readme, /never\s+accepts arbitrary commands from dispatch requests/);
  assert.match(schemaSql, /create table if not exists app_config/);
  assert.match(schemaSql, /key varchar\(200\) not null/);
  assert.match(schemaSql, /value jsonb not null/);
  assert.match(schemaSql, /app_config_scope_key_uq/);
  assert.match(appConfigSeedSql, /container-pool\.runtime-pools\.v1/);
  assert.match(appConfigSeedSql, /"runtimeContract": \{/);
  assert.match(appConfigSeedSql, /"defaultHealthPath": "\/healthz"/);
  assert.match(appConfigSeedSql, /"baseImages": \[/);
  assert.match(appConfigSeedSql, /"dockerfile": "remote\/deployments\/container-pool-rs\/runtime-images\/nodejs\.Dockerfile"/);
  assert.match(appConfigSeedSql, /dd-container-pool-nodejs-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-rust-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-golang-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-python3-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-dart-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-gleamlang-runtime:dev/);
  assert.match(appConfigSeedSql, /dd-container-pool-erlang-runtime:dev/);
  assert.match(appConfigSeedSql, /on conflict \(scope, key\) do update/);
  assert.match(schemaSql, /create table if not exists container_pool_configs/);
  // schema.sql uses the `default X not null` ordering; per-table dupes used
  // `not null default X`. Both are semantically identical; the regexes below
  // match the canonical schema.sql form.
  assert.match(schemaSql, /slug varchar\(120\) not null/);
  assert.match(schemaSql, /image text not null/);
  assert.match(schemaSql, /command jsonb default '\[\]'::jsonb not null/);
  assert.match(schemaSql, /env jsonb default '\{\}'::jsonb not null/);
  assert.match(schemaSql, /min_warm integer default 1 not null/);
  assert.match(schemaSql, /max_warm integer default 2 not null/);
  assert.match(schemaSql, /health_path varchar\(256\) default '\/healthz' not null/);
  assert.match(schemaSql, /nats_subject text/);
});

test('container pool app_config seed is a complete runtime contract', async () => {
  const appConfigSeedSql = await readRepoFile(
    'remote/databases/pg/seeds/container-pool-app-config.sql',
  );
  const parsed = parseContainerPoolAppConfigSeed(appConfigSeedSql);
  const expectedRuntimes = [
    'nodejs',
    'nodejs-chat-openai',
    'rust',
    'golang',
    'python3',
    'dart',
    'gleamlang',
    'erlang',
  ];

  assert.equal(parsed.runtimeContract.defaultRequestPath, '/invoke');
  assert.equal(parsed.runtimeContract.defaultHealthPath, '/healthz');
  assert.equal(parsed.runtimeContract.defaultContainerPort, 8080);
  assert.deepEqual(
    parsed.runtimeContract.managerInjectedEnv.sort(),
    [
      'DD_POOL_CONTAINER_NAME',
      'DD_POOL_HEALTH_PATH',
      'DD_POOL_ID',
      'DD_POOL_NATS_EVENT_SUBJECT',
      'DD_POOL_NATS_HEARTBEAT_SUBJECT',
      'DD_POOL_REQUEST_PATH',
      'DD_POOL_SLUG',
      'NATS_URL',
      'PORT',
    ].sort(),
  );
  assert.equal(
    parsed.runtimeContract.natsEventPattern,
    'dd.remote.container_pool.<poolSlug>.events',
  );
  assert.equal(
    parsed.runtimeContract.natsHeartbeatPattern,
    'dd.remote.container_pool.<poolSlug>.heartbeats',
  );
  assert.deepEqual(
    parsed.baseImages.map((entry) => entry.runtime).sort(),
    [...expectedRuntimes].sort(),
  );
  assert.deepEqual(
    parsed.pools.map((entry) => entry.slug).sort(),
    [
      'nodejs',
      'nodejs-chat-openai-k8s-cluster-dev',
      'nodejs-chat-openai-live-mutex-dev',
      'rust',
      'golang',
      'python3',
      'dart',
      'gleamlang',
      'erlang',
    ].sort(),
  );

  const baseImageByRuntime = new Map(parsed.baseImages.map((entry) => [entry.runtime, entry]));
  for (const pool of parsed.pools) {
    const baseImage =
      baseImageByRuntime.get(pool.slug) ??
      (pool.slug.startsWith('nodejs-chat-openai-')
        ? baseImageByRuntime.get('nodejs-chat-openai')
        : undefined);
    assert.ok(baseImage, `pool ${pool.slug} should have a matching base image`);
    assert.equal(pool.image, baseImage.image);
    if (pool.slug.startsWith('nodejs-chat-openai-')) {
      assert.equal(baseImage.dockerfile, 'remote/deployments/dev-server/Dockerfile');
      assert.equal(baseImage.buildContext, 'remote/deployments/dev-server');
      assert.equal(pool.requestPath, '/tasks');
      assert.equal(pool.env.WORKER_BIND_MODE, 'repo');
      const expectedRepoBySlug = new Map([
        ['nodejs-chat-openai-live-mutex-dev', 'git@github.com:ORESoftware/live-mutex.git'],
        ['nodejs-chat-openai-k8s-cluster-dev', 'git@github.com:ORESoftware/k8s-cluster.git'],
      ]);
      assert.equal(pool.env.DD_REPO_URL, expectedRepoBySlug.get(pool.slug));
      assert.equal(pool.env.AGENT_PROVIDER, 'openai-sdk');
      assert.equal(pool.readOnly, false);
      assert.equal(pool.user, '1000:1000');
    } else {
      assert.equal(
        baseImage.dockerfile,
        `remote/deployments/container-pool-rs/runtime-images/${pool.slug}.Dockerfile`,
      );
      assert.equal(baseImage.buildContext, 'remote/deployments/container-pool-rs');
      assert.equal(pool.requestPath, parsed.runtimeContract.defaultRequestPath);
      assert.equal(pool.env.DD_POOL_RUNTIME, pool.slug);
      assert.ok(pool.env.DD_POOL_HANDLER.length > 0, `pool ${pool.slug} should define a handler`);
    }
    assert.equal(pool.healthPath, parsed.runtimeContract.defaultHealthPath);
    assert.equal(pool.containerPort, parsed.runtimeContract.defaultContainerPort);
    if (pool.slug === 'nodejs-chat-openai-live-mutex-dev') {
      assert.ok(pool.minWarm >= 1, `pool ${pool.slug} should keep at least one warm worker`);
    } else {
      assert.ok(pool.minWarm >= 0, `pool ${pool.slug} should allow disabled prewarm`);
    }
    assert.ok(pool.maxWarm >= pool.minWarm, `pool ${pool.slug} maxWarm should cover minWarm`);
    assert.equal(pool.maxConcurrencyPerContainer, 1);
    assert.ok(pool.requestTimeoutMs >= 30_000);
    assert.ok(pool.idleTtlSeconds >= 900);
    assert.equal(pool.natsSubject, `dd.remote.container_pool.${pool.slug}.requests`);
  }
});

test('container pool is deployed through Argo, gateway, and metrics scraping', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-container-pool.deployment.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-container-pool.service.yaml');
  const rbac = await readRepoFile('remote/argocd/dd-next-runtime/dd-container-pool-rbac.yaml');
  const networkPolicy = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-container-pool.networkpolicy.yaml',
  );
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
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/container-pool-rs/);
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
  assert.match(deployment, /CONTAINER_POOL_PULL_POLICY[\s\S]*value:\s*never/);
  assert.match(deployment, /CONTAINER_POOL_PORT_START[\s\S]*value:\s*'12000'/);
  assert.match(deployment, /CONTAINER_POOL_NATS_MAX_PAYLOAD_BYTES[\s\S]*value:\s*'2097152'/);
  assert.match(deployment, /CONTAINER_POOL_WORKER_RESPONSE_MAX_BYTES[\s\S]*value:\s*'2097152'/);
  assert.match(deployment, /CONTAINER_POOL_COMMAND_TIMEOUT_SECONDS[\s\S]*value:\s*'300'/);
  assert.match(deployment, /CONTAINER_POOL_START_TIMEOUT_SECONDS[\s\S]*value:\s*'300'/);
  assert.match(deployment, /CONTAINER_POOL_CONTAINER_MEMORY[\s\S]*value:\s*512m/);
  assert.match(deployment, /CONTAINER_POOL_CONTAINER_CPUS[\s\S]*value:\s*'1'/);
  assert.match(deployment, /CONTAINER_POOL_PIDS_LIMIT[\s\S]*value:\s*'128'/);
  assert.match(deployment, /CONTAINER_POOL_NOFILE_LIMIT[\s\S]*value:\s*'128'/);
  assert.match(deployment, /CONTAINER_POOL_HEALTH_CHECK_SECONDS[\s\S]*value:\s*'10'/);
  assert.match(deployment, /CONTAINER_POOL_HEALTH_TIMEOUT_MS[\s\S]*value:\s*'1000'/);
  assert.match(deployment, /CONTAINER_POOL_UNHEALTHY_FAILURE_THRESHOLD[\s\S]*value:\s*'2'/);
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
  assert.match(kustomization, /dd-container-pool\.networkpolicy\.yaml/);
  assert.match(kustomization, /dd-container-pool\.service\.yaml/);
  assert.match(networkPolicy, /kind:\s*NetworkPolicy/);
  assert.match(networkPolicy, /app:\s*dd-container-pool/);
  assert.match(networkPolicy, /app:\s*dd-remote-gateway/);
  assert.match(networkPolicy, /kubernetes\.io\/metadata\.name:\s*messaging/);
  assert.match(networkPolicy, /port:\s*4222/);
  assert.match(networkPolicy, /port:\s*5432/);
  assert.match(networkPolicy, /port:\s*8102/);
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
  const worker = await readRepoFile('remote/deployments/container-pool-rs/runtime-images/common/worker.py');
  const shim = await readRepoFile('remote/deployments/container-pool-rs/scripts/nerdctl-process-shim.py');
  const runtimeReadme = await readRepoFile('remote/deployments/container-pool-rs/runtime-images/readme.md');
  const rustHandler = await readRepoFile('remote/deployments/container-pool-rs/runtime-images/common/rust-handler.rs');
  const golangHandler = await readRepoFile('remote/deployments/container-pool-rs/runtime-images/common/golang-handler.go');
  const erlangHandler = await readRepoFile('remote/deployments/container-pool-rs/runtime-images/common/erlang-handler.escript');
  const dockerfiles = new Map(
    await Promise.all(
      ['nodejs', 'rust', 'golang', 'python3', 'dart', 'gleamlang', 'erlang'].map(
        async (runtime) =>
          [
            runtime,
            await readRepoFile(`remote/deployments/container-pool-rs/runtime-images/${runtime}.Dockerfile`),
          ] as const,
      ),
    ),
  );

  assert.match(worker, /ThreadingHTTPServer/);
  assert.match(worker, /DD_POOL_HANDLER/);
  assert.match(worker, /DD_POOL_REQUEST_PATH/);
  assert.match(worker, /DD_POOL_HEALTH_PATH/);
  assert.match(worker, /DD_POOL_NATS_HEARTBEAT_SUBJECT/);
  assert.match(worker, /publish_nats/);
  assert.match(worker, /echo_key = request_payload\.get\("echoKey"\)/);
  assert.match(worker, /def do_POST\(self\):/);
  assert.match(worker, /path != REQUEST_PATH/);
  assert.match(worker, /subprocess\.run/);
  assert.match(shim, /Development-only nerdctl shim/);
  assert.match(shim, /"--publish"/);
  assert.match(shim, /handle_inspect/);
  assert.match(shim, /"Running": running/);
  assert.match(shim, /published_host_port/);
  assert.match(runtimeReadme, /nodejs/);
  assert.match(runtimeReadme, /gleamlang/);
  assert.match(runtimeReadme, /DD_POOL_NATS_HEARTBEAT_SUBJECT/);
  assert.match(rustHandler, /evaluate_expression/);
  assert.match(rustHandler, /answer/);
  assert.match(golangHandler, /evaluateExpression/);
  assert.match(golangHandler, /"answer"/);
  assert.match(erlangHandler, /eval_expr/);
  assert.match(erlangHandler, /answer/);

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
  assert.match(dockerfiles.get('rust') ?? '', /mkdir -p \/out/);
  assert.match(dockerfiles.get('golang') ?? '', /golang:1\.25-alpine AS build/);
  assert.match(dockerfiles.get('golang') ?? '', /mkdir -p \/out/);
  assert.match(dockerfiles.get('python3') ?? '', /python:3\.13-alpine/);
  assert.match(dockerfiles.get('dart') ?? '', /dart compile exe/);
  assert.match(dockerfiles.get('dart') ?? '', /mkdir -p \/out/);
  assert.match(dockerfiles.get('gleamlang') ?? '', /gleam:v1\.16\.0-erlang-alpine/);
  assert.match(dockerfiles.get('erlang') ?? '', /erlang:28-alpine/);
});
