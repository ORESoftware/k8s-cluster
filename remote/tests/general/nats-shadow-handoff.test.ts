import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/rest-api-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rest api publishes queued handoffs while preserving direct worker dispatch', async () => {
  const cargo = await readRepoFile('remote/rest-api-rs/Cargo.toml');
  const server = await readRepoFile('remote/rest-api-rs/src/main.rs');

  assert.match(cargo, /async-nats\s*=\s*"=0\.38\.0"/);
  assert.match(server, /struct NatsTaskMessage/);
  assert.match(server, /fn nats_task_subject/);
  assert.match(server, /dd\.remote\.thread\.\{thread_id\}\.tasks/);
  assert.match(server, /fn nats_task_stream_name/);
  assert.match(server, /DD_REMOTE_TASKS/);
  assert.match(server, /ensure_nats_task_stream/);
  assert.match(server, /jetstream_publish_task/);
  assert.match(server, /RetentionPolicy::WorkQueue/);
  assert.match(server, /fn nats_wakeup_subject/);
  assert.match(server, /dd\.remote\.orchestrator\.wakeup/);
  assert.match(server, /fn nats_event_subject/);
  assert.match(server, /dd\.remote\.events/);
  assert.match(server, /persist_task_status_event/);
  assert.match(server, /publish_task_event_to_nats/);
  assert.match(server, /queued-dispatch-accepted/);
  assert.match(server, /"stage": "nats-published"/);
  assert.match(server, /on conflict \(task_id, seq\) do update set/);
  assert.match(server, /"task\.shadow"/);
  assert.match(server, /publish_task_dispatch_to_nats/);
  assert.match(server, /publish_task_to_nats\(request, branch, "task\.dispatch", false, true\)/);
  assert.match(server, /"task\.dispatch"/);
  assert.match(server, /dispatch_mode/);
  assert.match(server, /"stage": "nats-publish-failed"/);
  assert.match(server, /continuing with synchronous worker dispatch/);
  assert.ok(
    server.indexOf('publish_task_dispatch_to_nats(&request, None).await')
      < server.indexOf('ensure_thread_worker(&thread_id'),
    'queued NATS publish must happen before the synchronous worker wake path',
  );
  assert.match(server, /shadow: bool/);
  assert.match(server, /direct_dispatch: bool/);
  assert.match(server, /publish_task_shadow_to_nats\(&request, branch\.as_deref\(\)\)/);
  assert.match(server, /Duration::from_secs\(2\)/);
});

test('queue consumer is deployed and prepares deterministic thread workers', async () => {
  const cargo = await readRepoFile('remote/queue-consumer-rs/Cargo.toml');
  const consumer = await readRepoFile('remote/queue-consumer-rs/src/main.rs');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-queue-consumer.deployment.yaml',
  );
  const scaledObject = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-queue-consumer.scaledobject.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');

  assert.match(cargo, /name\s*=\s*"dd-remote-queue-consumer"/);
  assert.match(consumer, /build_jetstream_consumer/);
  assert.match(consumer, /get_or_create_stream/);
  assert.match(consumer, /get_or_create_consumer/);
  assert.match(consumer, /RetentionPolicy::WorkQueue/);
  assert.match(consumer, /NATS_TASK_NAK_DELAY_SECONDS/);
  assert.match(consumer, /AckKind::Nak\(Some/);
  assert.match(consumer, /api\/agents\/threads\/\{thread_id\}\/prepare/);
  assert.match(consumer, /X-Agent-Auth/);
  assert.match(consumer, /dd\.remote\.thread\.\*\.tasks/);
  assert.match(consumer, /DD_REMOTE_TASKS/);
  assert.match(consumer, /QUEUE_CONSUMER_RECEIPTS_DIR/);
  assert.match(consumer, /CONTAINER_POOL_BASE_URL/);
  assert.match(consumer, /dispatch_to_container_pool/);
  assert.match(consumer, /repo_pool_slug/);
  assert.match(consumer, /nodejs-chat-claude-/);
  assert.match(consumer, /"affinityKey": &task\.thread_id/);
  assert.match(consumer, /QUEUE_CONSUMER_FALLBACK_REST_DISPATCH/);
  assert.match(consumer, /HashSet/);
  assert.match(consumer, /has_task_receipt/);
  assert.match(consumer, /write_task_receipt/);
  assert.match(consumer, /queue task skipped duplicate/);
  assert.match(consumer, /emit_queue_status_event/);
  assert.match(consumer, /persist_queue_status_event/);
  assert.match(consumer, /publish_queue_status_event/);
  assert.match(consumer, /dd\.remote\.events/);
  assert.match(consumer, /queue-received/);
  assert.match(consumer, /direct-dispatch-prepare/);
  assert.match(consumer, /direct-dispatch-prepare-failed/);
  assert.match(consumer, /shadow-prepare-failed/);
  assert.match(consumer, /non-executing handoff/);
  assert.match(consumer, /if shadow \|\| direct_dispatch \{[\s\S]*prepare_thread\(&http, &rest_api_url, &secret, &task\.thread_id\)\.await[\s\S]*\} else \{/);
  assert.match(consumer, /container-pool-dispatch/);
  assert.match(consumer, /let pool = task[\s\S]*repo_pool_slug\(repo, task\.base_branch\.as_deref\(\)\.unwrap_or\("dev"\)\)/);
  assert.match(consumer, /match dispatch_to_container_pool\(&http, &container_pool_url, &secret, &task\)\.await/);
  assert.match(consumer, /container-pool-failed/);
  assert.match(consumer, /"affinityKey": &task\.thread_id/);
  assert.match(consumer, /queue-handoff-ok/);
  assert.match(consumer, /rest-fallback-skipped/);
  assert.match(consumer, /rest-fallback-accepted/);
  assert.match(consumer, /queue-acked/);
  assert.doesNotMatch(consumer, /Command::new|tokio::process|std::process/);
  assert.match(deployment, /name:\s*dd-remote-queue-consumer/);
  assert.match(deployment, /NATS_QUEUE_GROUP[\s\S]*dd-remote-thread-preparer/);
  assert.match(deployment, /NATS_TASK_STREAM[\s\S]*DD_REMOTE_TASKS/);
  assert.match(deployment, /NATS_TASK_CONSUMER[\s\S]*dd-remote-thread-preparer/);
  assert.match(deployment, /NATS_TASK_NAK_DELAY_SECONDS[\s\S]*'15'/);
  assert.match(deployment, /QUEUE_CONSUMER_FALLBACK_REST_DISPATCH[\s\S]*value:\s*'false'/);
  assert.match(deployment, /resources:[\s\S]*requests:[\s\S]*cpu:\s*100m[\s\S]*memory:\s*128Mi/);
  assert.match(
    deployment,
    /resources:[\s\S]*limits:[\s\S]*cpu:\s*['"]?1['"]?[\s\S]*memory:\s*1Gi/,
  );
  assert.match(
    deployment,
    /REMOTE_REST_API_URL[\s\S]*dd-remote-rest-api\.default\.svc\.cluster\.local:8082/,
  );
  assert.match(
    deployment,
    /CONTAINER_POOL_BASE_URL[\s\S]*dd-container-pool\.default\.svc\.cluster\.local:8102/,
  );
  assert.match(
    deployment,
    /QUEUE_CONSUMER_RECEIPTS_DIR[\s\S]*\/tmp\/dd-remote-queue-consumer\/tasks/,
  );
  assert.match(scaledObject, /kind:\s*ScaledObject/);
  assert.match(scaledObject, /type:\s*nats-jetstream/);
  assert.match(scaledObject, /minReplicaCount:\s*1/);
  assert.match(scaledObject, /maxReplicaCount:\s*8/);
  assert.match(scaledObject, /stream:\s*DD_REMOTE_TASKS/);
  assert.match(scaledObject, /consumer:\s*dd-remote-thread-preparer/);
  assert.match(scaledObject, /lagThreshold:\s*'25'/);
  assert.match(kustomization, /dd-remote-queue-consumer\.deployment\.yaml/);
  assert.match(kustomization, /dd-remote-queue-consumer\.scaledobject\.yaml/);
});

test('keda is declared for event-driven queue consumer scaling', async () => {
  const application = await readRepoFile('remote/argocd/apps/keda.application.yaml');

  assert.match(application, /name:\s*keda/);
  assert.match(application, /repoURL:\s*https:\/\/kedacore\.github\.io\/charts/);
  assert.match(application, /chart:\s*keda/);
  assert.match(application, /targetRevision:\s*2\.19\.0/);
  assert.match(application, /CreateNamespace=true/);
});

test('rest api exposes an internal prepare route for queue warmup', async () => {
  const server = await readRepoFile('remote/rest-api-rs/src/main.rs');
  const readme = await readRepoFile('remote/rest-api-rs/readme.md');

  assert.match(server, /async fn prepare_thread/);
  assert.match(server, /authorized_internal_request/);
  assert.match(server, /prepare_thread_worker\(&thread_id\)/);
  assert.match(server, /"\/api\/agents\/threads\/:thread_id\/prepare"/);
  assert.match(readme, /POST \/api\/agents\/threads\/:threadId\/prepare/);
  assert.match(readme, /shadow task\s+message/);
});

test('runtime deployments avoid routing to half-started rust services', async () => {
  const restDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-rest-api.deployment.yaml',
  );
  const consumerDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-queue-consumer.deployment.yaml',
  );

  assert.match(
    restDeployment,
    /resources:[\s\S]*requests:[\s\S]*cpu:\s*100m[\s\S]*memory:\s*128Mi/,
  );
  assert.match(
    restDeployment,
    /resources:[\s\S]*limits:[\s\S]*cpu:\s*['"]?1['"]?[\s\S]*memory:\s*1Gi/,
  );
  assert.match(restDeployment, /startupProbe:[\s\S]*path: \/healthz/);
  assert.match(restDeployment, /readinessProbe:[\s\S]*path: \/healthz/);
  assert.match(restDeployment, /livenessProbe:[\s\S]*path: \/healthz/);
  assert.match(restDeployment, /NATS_TASK_STREAM[\s\S]*DD_REMOTE_TASKS/);
  assert.match(
    consumerDeployment,
    /resources:[\s\S]*requests:[\s\S]*cpu:\s*100m[\s\S]*memory:\s*128Mi/,
  );
  assert.match(consumerDeployment, /limits:[\s\S]*cpu:\s*'1'/);
  assert.match(consumerDeployment, /limits:[\s\S]*memory:\s*1Gi/);
});
