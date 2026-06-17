import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/idle-reaper-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust reaper includes the inline cluster doctor prompt', async () => {
  const reaper = await readRepoFile('remote/deployments/idle-reaper-rs/src/main.rs');
  const cargo = await readRepoFile('remote/deployments/idle-reaper-rs/Cargo.toml');

  assert.match(cargo, /async-nats\s*=\s*"=0\.38\.0"/);
  assert.match(cargo, /chrono-tz/);
  assert.match(cargo, /futures-util/);
  assert.match(cargo, /serde_json/);
  assert.match(reaper, /const CLUSTER_DOCTOR_PROMPT/);
  assert.match(reaper, /scheduled cluster doctor/);
  assert.match(reaper, /dd-prometheus\.observability\.svc\.cluster\.local:9090/);
  assert.match(reaper, /dd-loki\.observability\.svc\.cluster\.local:3100/);
  assert.match(reaper, /dd-grafana\.observability\.svc\.cluster\.local:3000/);
  assert.match(reaper, /dd-nats\.messaging\.svc\.cluster\.local:8222/);
  assert.match(reaper, /dd-otel-collector\.observability\.svc\.cluster\.local:8889/);
  assert.match(
    reaper,
    /remote-dev server will commit changed files, push the branch, and open or/,
  );
  assert.match(reaper, /CLUSTER_DOCTOR_INTERVAL_SECONDS", 90 \* 60/);
  assert.match(reaper, /post\(&job\.task_url\)/);
  assert.match(reaper, /header\("x-server-auth", &job\.server_auth_secret\)/);
  assert.match(reaper, /struct WorkerImageBuildJob/);
  assert.match(reaper, /WORKER_IMAGE_BUILD_TIMEZONE/);
  assert.match(reaper, /America\/New_York/);
  assert.match(reaper, /next_worker_image_build_delay/);
  assert.match(reaper, /nerdctl/);
  assert.match(reaper, /DD_REPO_CACHE_BUST/);
  assert.match(reaper, /docker\.io\/library\/dd-dev-server:dev/);
});

test('argocd reaper deployment runs the rust scheduler with a 90-minute doctor loop', async () => {
  const config = await readRepoFile('remote/argocd/dd-next-runtime/dd-idle-reaper.configmap.yaml');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-idle-reaper.deployment.yaml',
  );
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(config, /CLUSTER_DOCTOR_ENABLED:\s*['"]true['"]/);
  assert.match(config, /CLUSTER_DOCTOR_INTERVAL_SECONDS:\s*['"]5400['"]/);
  assert.match(
    config,
    /CLUSTER_DOCTOR_TASK_URL:\s*['"]http:\/\/dd-dev-server-api\.default\.svc\.cluster\.local:8080\/tasks['"]/,
  );
  assert.match(config, /CLUSTER_DOCTOR_THREAD_ID:\s*['"]00000000-0000-4000-8000-000000000001['"]/);
  assert.match(config, /CLUSTER_DOCTOR_PROVIDER:\s*['"]claude-sdk['"]/);
  assert.match(config, /WORKER_IMAGE_BUILD_ENABLED:\s*['"]true['"]/);
  assert.match(config, /WORKER_IMAGE_BUILD_TIMEZONE:\s*['"]America\/New_York['"]/);
  assert.match(config, /WORKER_IMAGE_BUILD_HOUR:\s*['"]4['"]/);
  assert.match(config, /WORKER_IMAGE_BUILD_IMAGE:\s*['"]docker\.io\/library\/dd-dev-server:dev['"]/);
  assert.match(config, /NATS_WATCH_ENABLED:\s*['"]true['"]/);
  assert.match(config, /NATS_WATCH_ACTIVE_INTERVAL_SECONDS:\s*['"]5['"]/);
  assert.match(config, /NATS_WATCH_IDLE_INTERVAL_SECONDS:\s*['"]15['"]/);
  assert.match(config, /NATS_WATCH_TASK_SUBJECT:\s*['"]dd\.remote\.thread\.\*\.tasks['"]/);
  assert.match(config, /NATS_WATCH_EVENT_SUBJECT:\s*['"]dd\.remote\.events['"]/);
  assert.match(
    config,
    /NATS_WATCH_GLEAM_BROADCAST_URL:\s*['"]http:\/\/dd-gleamlang-server\.default\.svc\.cluster\.local:8081\/broadcast['"]/,
  );
  assert.match(config, /RUNTIME_FLOOR_ENABLED:\s*['"]true['"]/);
  assert.match(config, /RUNTIME_FLOOR_INTERVAL_SECONDS:\s*['"]20['"]/);
  assert.match(config, /RUNTIME_FLOOR_NATS_TASK_STREAM:\s*['"]DD_REMOTE_TASKS['"]/);
  assert.match(
    config,
    /RUNTIME_FLOOR_NATS_TASK_CONSUMER:\s*['"]dd-remote-thread-preparer['"]/,
  );
  assert.match(
    config,
    /RUNTIME_FLOOR_CONTAINER_POOL_URL:\s*['"]http:\/\/dd-container-pool\.default\.svc\.cluster\.local:8102['"]/,
  );
  assert.match(
    config,
    /RUNTIME_FLOOR_QUEUE_CONSUMER_DEPLOYMENT:\s*['"]dd-remote-queue-consumer['"]/,
  );
  assert.match(config, /RUNTIME_FLOOR_QUEUE_CONSUMER_MIN_READY:\s*['"]1['"]/);
  assert.doesNotMatch(config, /NATS_WATCH_GLEAM_BROADCAST_SECRET:/);
  assert.doesNotMatch(config, /CLUSTER_DOCTOR_SERVER_AUTH_SECRET:/);
  assert.match(deployment, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/idle-reaper-rs/);
  assert.match(deployment, /cargo run --release/);
  assert.match(deployment, /name:\s*dd-idle-reaper-config/);
  assert.match(deployment, /name:\s*CLUSTER_DOCTOR_SERVER_AUTH_SECRET/);
  assert.match(deployment, /name:\s*NATS_WATCH_GLEAM_BROADCAST_SECRET/);
  assert.match(deployment, /name:\s*dd-idle-reaper-secret/);
  assert.match(deployment, /key:\s*CLUSTER_DOCTOR_SERVER_AUTH_SECRET/);
  assert.match(deployment, /key:\s*NATS_WATCH_GLEAM_BROADCAST_SECRET/);
  assert.match(deployment, /name:\s*WORKER_IMAGE_BUILD_GITHUB_DEPLOY_KEY/);
  assert.match(deployment, /key:\s*GH_DEPLOY_KEY/);
  assert.match(deployment, /mountPath:\s*\/run\/containerd\/containerd\.sock/);
  assert.match(deployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);
  assert.match(deployment, /mountPath:\s*\/opt\/dd-next-1/);
  assert.match(deployment, /dd\.dev\/telemetry-revision:\s*['"]2026-05-20-runtime-floor['"]/);
  assert.match(runtimeReadme, /dd-idle-reaper-secret` key `CLUSTER_DOCTOR_SERVER_AUTH_SECRET/);
  assert.match(runtimeReadme, /dd-idle-reaper-secret` key `NATS_WATCH_GLEAM_BROADCAST_SECRET/);
  assert.match(runtimeReadme, /adaptive NATS watchdog/);
  assert.match(runtimeReadme, /runtime floor reconciler every 20 seconds/);
  assert.match(runtimeReadme, /Every day at\s+4am America\/New_York/);
  assert.match(runtimeReadme, /docker\.io\/library\/dd-dev-server:dev/);
});

test('argocd runtime exposes a native headlamp cron sentinel', async () => {
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const cron = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-headlamp-cron-sentinel.cronjob.yaml',
  );
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(kustomization, /dd-headlamp-cron-sentinel\.cronjob\.yaml/);
  assert.match(cron, /kind:\s*CronJob/);
  assert.match(cron, /name:\s*dd-headlamp-cron-sentinel/);
  assert.match(cron, /namespace:\s*default/);
  assert.match(cron, /schedule:\s*['"]\* \* \* \* \*['"]/);
  assert.match(cron, /concurrencyPolicy:\s*Forbid/);
  assert.match(cron, /automountServiceAccountToken:\s*false/);
  assert.match(cron, /while true; do\s+sleep 3600\s+done/);
  assert.match(cron, /cpu:\s*1m/);
  assert.match(cron, /memory:\s*8Mi/);
  assert.match(runtimeReadme, /dd-headlamp-cron-sentinel/);
  assert.match(runtimeReadme, /Headlamp's Jobs and Cron Jobs workload cards/);
});

test('argocd runtime schedules the soccer tournament without compile or worker OOM spikes', async () => {
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const cron = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-soccer-tournament-nightly.cronjob.yaml',
  );
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(kustomization, /dd-soccer-tournament-nightly\.cronjob\.yaml/);
  assert.match(cron, /kind:\s*CronJob/);
  assert.match(cron, /name:\s*dd-soccer-tournament-nightly/);
  assert.match(cron, /namespace:\s*default/);
  assert.match(cron, /schedule:\s*["']0 2 \* \* \*["']/);
  assert.match(cron, /timeZone:\s*America\/Chicago/);
  assert.match(cron, /concurrencyPolicy:\s*Forbid/);
  assert.match(cron, /activeDeadlineSeconds:\s*28800/);
  assert.match(cron, /automountServiceAccountToken:\s*false/);
  assert.match(cron, /enableServiceLinks:\s*false/);
  assert.match(cron, /CARGO_BUILD_JOBS[\s\S]*value:\s*["']1["']/);
  assert.match(cron, /SOCCER_TOURNAMENT_THREADS[\s\S]*value:\s*["']2["']/);
  assert.match(cron, /SOCCER_TOURNAMENT_SOFT_DEADLINE_SECONDS[\s\S]*value:\s*["']25200["']/);
  assert.match(cron, /SOCCER_TOURNAMENT_LOCK_KEY[\s\S]*value:\s*soccer-nightly-tournament/);
  assert.match(cron, /limits:[\s\S]*cpu:\s*["']2["'][\s\S]*memory:\s*14Gi/);
  assert.match(cron, /emptyDir:[\s\S]*sizeLimit:\s*20Gi/);
  assert.match(cron, /git -C "\$\{dir\}" switch --detach FETCH_HEAD/);
  assert.doesNotMatch(cron, /\bgit checkout\b/);
  assert.doesNotMatch(cron, /rm -rf/);
  assert.match(runtimeReadme, /dd-soccer-tournament-nightly/);
  assert.match(runtimeReadme, /AWS\/Hetzner runner/);
});

test('reaper nats watchdog backstops worker prepare and websocket fanout', async () => {
  const reaper = await readRepoFile('remote/deployments/idle-reaper-rs/src/main.rs');
  const cargo = await readRepoFile('remote/deployments/idle-reaper-rs/Cargo.toml');
  const readme = await readRepoFile('remote/deployments/idle-reaper-rs/README.md');

  assert.match(reaper, /struct NatsWatchJob/);
  assert.match(reaper, /NATS_WATCH_ACTIVE_INTERVAL_SECONDS", 5/);
  assert.match(reaper, /NATS_WATCH_IDLE_INTERVAL_SECONDS", 15/);
  // Subject defaults are pulled from the @dd/nats-subject-defs generated
  // crate so a rename in remote/libs/nats/subject-defs/schema/ breaks the
  // build here instead of silently watching the wrong subject.
  assert.match(cargo, /dd-nats-subject-defs\s*=\s*\{\s*path/);
  assert.match(
    reaper,
    /use dd_nats_subject_defs::\{[\s\S]*?DD_REMOTE_TASKS_STREAM_NAME[\s\S]*?RUNTIME_EVENTS_SUBJECT[\s\S]*?THREAD_PREPARER_QUEUE_GROUP[\s\S]*?THREAD_TASKS_WILDCARD[\s\S]*?\};/,
  );
  assert.match(reaper, /THREAD_TASKS_WILDCARD\.to_string\(\)/);
  assert.match(reaper, /RUNTIME_EVENTS_SUBJECT\.to_string\(\)/);
  assert.match(reaper, /DD_REMOTE_TASKS_STREAM_NAME\.to_string\(\)/);
  assert.match(reaper, /THREAD_PREPARER_QUEUE_GROUP\.to_string\(\)/);
  assert.match(reaper, /prepare_thread_from_nats/);
  assert.match(reaper, /broadcast_event_from_nats/);
  assert.match(reaper, /struct RuntimeFloorJob/);
  assert.match(reaper, /RUNTIME_FLOOR_INTERVAL_SECONDS", 20/);
  assert.match(reaper, /ensure_runtime_floor_nats/);
  assert.match(reaper, /reconcile_queue_consumer_floor/);
  assert.match(reaper, /reconcile_container_pool_floor/);
  assert.match(reaper, /broadcaster|GLEAM_BROADCAST_URL|gleam_broadcast_url/);
  assert.match(reaper, /NATS_WATCH_GLEAM_BROADCAST_SECRET/);
  assert.match(reaper, /nats watchdog disabled: NATS_WATCH_GLEAM_BROADCAST_SECRET missing/);
  assert.doesNotMatch(reaper, /dd-k8s-home/);
  assert.match(readme, /NATS watchdog/);
  assert.match(readme, /backstop worker/);
  assert.match(readme, /runtime floor/);
  assert.match(readme, /`NATS_WATCH_GLEAM_BROADCAST_SECRET` \| yes, when enabled \| — \|/);
});

test('dev-server can receive provider and github secrets for scheduled PR work', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');
  const restApi = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');

  assert.match(deployment, /name:\s*dd-agent-secrets[\s\S]*optional:\s*true/);
  assert.match(deployment, /envFrom:[\s\S]*secretRef:[\s\S]*name:\s*dd-agent-secrets/);
  assert.match(deployment, /name:\s*NATS_URL[\s\S]*value:\s*nats:\/\/dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /name:\s*NATS_EVENT_SUBJECT[\s\S]*value:\s*dd\.remote\.events/);
  assert.doesNotMatch(deployment, /dd-k8s-home/);
  assert.doesNotMatch(restApi, /dd-k8s-home/);
  assert.match(restApi, /REMOTE_DEV_SERVER_SECRET or SERVER_AUTH_SECRET is not set/);
  assert.match(
    deployment,
    /name:\s*REMOTE_DEV_THREAD_ID[\s\S]*value:\s*"00000000-0000-4000-8000-000000000001"/,
  );
  assert.match(runtimeReadme, /Use `dd-agent-secrets` for shared remote runtime values:/);
  assert.match(runtimeReadme, /- `SERVER_AUTH_SECRET`/);
  assert.match(
    runtimeReadme,
    /- model-provider keys like `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `OPENAI_API_KEY`,\s+`OPENCODE_API_KEY`, `DEEPSEEK_API_KEY`, `DASHSCOPE_API_KEY`, and `XAI_API_KEY`/,
  );
  assert.match(
    runtimeReadme,
    /- GitHub credentials used by the remote dev worker entrypoint and PR creation path/,
  );
});

test('external secrets rollout stays aligned with runtime secret consumers', async () => {
  const secretsApp = await readRepoFile('remote/argocd/apps/dd-secrets.application.yaml');
  const operatorApp = await readRepoFile(
    'remote/argocd/apps/external-secrets-operator.application.yaml',
  );
  const secretStore = await readRepoFile('remote/argocd/secrets/providers/aws/secret-store.yaml');
  const secrets = await readRepoFile('remote/argocd/secrets/common/external-secrets.yaml');
  const kustomization = await readRepoFile('remote/argocd/secrets/kustomization.yaml');
  const secretsReadme = await readRepoFile('remote/argocd/secrets/readme.md');
  const restDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.deployment.yaml',
  );

  assert.match(secretsApp, /name:\s*dd-secrets/);
  assert.match(secretsApp, /path:\s*remote\/argocd\/secrets/);
  assert.match(operatorApp, /name:\s*external-secrets-operator/);
  assert.match(operatorApp, /repoURL:\s*https:\/\/charts\.external-secrets\.io/);
  assert.match(operatorApp, /chart:\s*external-secrets/);
  assert.match(operatorApp, /installCRDs:\s*true/);
  assert.match(operatorApp, /hostNetwork:\s*true/);
  assert.match(operatorApp, /dnsPolicy:\s*ClusterFirstWithHostNet/);
  assert.match(secretStore, /kind:\s*ClusterSecretStore/);
  assert.match(secretStore, /name:\s*dd-cluster-secrets/);
  assert.match(secretStore, /argocd\.argoproj\.io\/sync-options:\s*Replace=true/);
  assert.match(secretStore, /service:\s*SecretsManager/);
  assert.match(secretStore, /region:\s*us-east-1/);
  assert.doesNotMatch(secretStore, /accessKeyIDSecretRef/);
  assert.match(kustomization, /- providers\/aws/);
  assert.match(kustomization, /- common/);
  assert.match(secrets, /name:\s*dd-agent-secrets/);
  assert.match(secrets, /key:\s*dd\/remote-dev\/agent-secrets/);
  assert.match(secrets, /name:\s*dd-remote-rest-api-secrets/);
  assert.match(secrets, /key:\s*dd\/remote-dev\/rest-api-secrets/);
  assert.match(secrets, /name:\s*dd-idle-reaper-secret/);
  assert.match(secrets, /key:\s*dd\/remote-dev\/idle-reaper-secret/);
  assert.match(restDeployment, /name:\s*dd-agent-secrets[\s\S]*optional:\s*true/);
  assert.match(restDeployment, /name:\s*dd-remote-rest-api-secrets[\s\S]*optional:\s*true/);
  assert.match(secretsReadme, /dd\/remote-dev\/agent-secrets/);
  assert.match(secretsReadme, /dd\/remote-dev\/rest-api-secrets/);
  assert.match(secretsReadme, /dd\/remote-dev\/idle-reaper-secret/);
  assert.match(secretsReadme, /default AWS\s+credential chain/);
});
