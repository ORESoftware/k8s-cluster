import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('node task workers publish every stream event to nats for websocket fanout', async () => {
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  const publisher = await readRepoFile('remote/deployments/dev-server/src/nats-publisher.ts');
  const wsFanout = await readRepoFile('remote/deployments/dev-server/src/ws-fanout.ts');
  const packageJson = await readRepoFile('remote/deployments/dev-server/package.json');
  const restApi = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');
  const bootstrapDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );

  assert.match(server, /new NatsPublisher\(config\.natsUrl\)/);
  assert.match(
    server,
    /natsEventSubject: process\.env\.NATS_EVENT_SUBJECT \?\? 'dd\.remote\.events'/,
  );
  assert.match(server, /type: 'task-event'/);
  assert.match(server, /messageId: randomUUID\(\)/);
  assert.match(server, /natsPublisher\.publish\(config\.natsEventSubject/);
  assert.match(server, /new WorkerFanoutWebSocket/);
  assert.match(server, /workerFanout\.publish\(fanoutPayload\)/);
  assert.match(wsFanout, /workerFanoutWsUrlFromEnv/);
  assert.match(wsFanout, /globalThis[\s\S]*WebSocket/);
  assert.match(wsFanout, /GLEAM_WORKER_WS_SECRET/);
  assert.match(wsFanout, /dd-gleamlang-server\.default\.svc\.cluster\.local:8081\/worker-ws/);
  assert.match(packageJson, /"@nats-io\/transport-node":\s*"\^3\.4\.0"/);
  assert.match(packageJson, /"@nats-io\/jetstream":\s*"\^3\.4\.0"/);
  assert.match(packageJson, /"ws":\s*"\^8\.20\.1"/);
  assert.match(publisher, /PUB \$\{next\.subject\} \$\{bytes\}/);
  assert.match(publisher, /PONG\\r\\n/);
  assert.match(restApi, /async_nats::connect\(nats_url\(\)\)/);
  assert.match(restApi, /async fn publish_task_dispatch_to_nats/);
  assert.match(restApi, /publish_task_event_to_websocket_fanout/);
  assert.match(restApi, /REST_STATUS_GLEAM_BROADCAST_URL/);
  assert.match(restApi, /async fn publish_thread_runtime_event_to_nats/);
  assert.match(
    restApi,
    /"name":\s*"NATS_URL",\s*"value":\s*"nats:\/\/dd-nats\.messaging\.svc\.cluster\.local:4222"/,
  );
  assert.match(
    bootstrapDeployment,
    /name:\s*NATS_URL[\s\S]*value:\s*nats:\/\/dd-nats\.messaging\.svc\.cluster\.local:4222/,
  );
  assert.match(
    bootstrapDeployment,
    /name:\s*NATS_EVENT_SUBJECT[\s\S]*value:\s*dd\.remote\.events/,
  );
});

test('gleam websocket deployment bridges nats tcp events into browser websockets', async () => {
  const broadcaster = await readRepoFile(
    'remote/deployments/gleamlang-ws-server/src/gleamlang_ws_server/broadcaster.gleam',
  );
  const httpServer = await readRepoFile(
    'remote/deployments/gleamlang-ws-server/src/gleamlang_ws_server/http_server.gleam',
  );
  const main = await readRepoFile(
    'remote/deployments/gleamlang-ws-server/src/gleamlang_ws_server.gleam',
  );
  const pgListen = await readRepoFile(
    'remote/deployments/gleamlang-ws-server/src/gleamlang_ws_server/pg_listen.gleam',
  );
  const env = await readRepoFile(
    'remote/deployments/gleamlang-ws-server/src/gleamlang_ws_server_ffi.erl',
  );
  const bridge = await readRepoFile('remote/deployments/gleamlang-ws-server/nats-bridge.mjs');
  const natsClient = await readRepoFile('remote/deployments/gleamlang-ws-server/nats-client.mjs');
  const dockerfile = await readRepoFile('remote/deployments/gleamlang-ws-server/Dockerfile');
  const deployment = await readRepoFile(
    'remote/deployments/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/deployments/gleamlang-server/k8s/minikube/dd-gleamlang-server.deployment.yaml',
  );

  assert.match(broadcaster, /BroadcastJson\(payload: String\)/);
  assert.match(broadcaster, /const dedupe_ttl_ms = 300_000/);
  assert.match(broadcaster, /json_message_id\(payload\)/);
  assert.match(broadcaster, /SeenMessage\(id: message_id, expires_at_ms: now \+ dedupe_ttl_ms\)/);
  assert.match(broadcaster, /dd_gleamlang_nats_messages_total|nats_messages/);
  assert.match(httpServer, /Post,\s*\["broadcast"\] -> broadcast\(deps, req\)/);
  assert.match(httpServer, /Get,\s*\["worker-ws", secret\] -> worker_websocket\(deps, req, secret\)/);
  assert.match(httpServer, /mist\.read_body\(req, 1_048_576\)/);
  assert.match(httpServer, /broadcaster\.BroadcastJson\(payload\)/);
  assert.match(httpServer, /env_get\("GLEAM_BROADCAST_SECRET"\)/);
  assert.match(httpServer, /env_get\("GLEAM_WORKER_WS_SECRET"\)/);
  assert.match(httpServer, /nats_publish_via_sidecar\(payload\)/);
  assert.match(main, /PG_DATABASE_URL/);
  assert.match(main, /PRESENCE_NOTIFY_SHARDS/);
  assert.match(main, /PRESENCE_WAL_ENABLED/);
  assert.match(pgListen, /channel_accepts_event\(channel, event\)/);
  assert.match(pgListen, /valid_shard\(fields\.conv_shard, n_shards\)/);
  assert.match(pgListen, /parse_op\(fields\.op\)/);
  assert.match(env, /publish_nats\/1/);
  assert.match(env, /json_message_id\/1/);
  assert.match(env, /now_ms\/0/);
  assert.match(env, /GLEAM_NATS_PUBLISH_URL/);
  assert.match(env, /NATS_PUBLISH_SUBJECT/);
  assert.match(env, /gen_tcp:connect\(Host, Port/);
  assert.match(env, /"POST ", Path, " HTTP\/1\.1\\r\\n"/);
  assert.doesNotMatch(env, /httpc:request\(post/);
  assert.match(natsClient, /let singleton = null/);
  assert.match(natsClient, /export function getNatsClient/);
  assert.match(natsClient, /net\.createConnection/);
  assert.match(natsClient, /SUB \$\{subscription\.subject\} \$\{sid\}/);
  assert.match(natsClient, /PUB \$\{next\.subject\} \$\{next\.payload\.length\}/);
  assert.match(natsClient, /PONG\\r\\n/);
  assert.match(dockerfile, /apk add --no-cache nodejs/);
  assert.match(bridge, /getNatsClient/);
  assert.match(bridge, /NATS_BRIDGE_DEDUPE_TTL_MS/);
  assert.match(bridge, /seenMessageIds/);
  assert.match(bridge, /dropDuplicate\(payload\)/);
  assert.match(bridge, /extractMessageId/);
  assert.match(bridge, /NATS_READ_SUBJECT/);
  assert.match(bridge, /NATS_PUBLISH_SUBJECT/);
  assert.match(bridge, /createServer/);
  assert.match(bridge, /url\.pathname !== '\/publish'/);
  assert.match(bridge, /http:\/\/127\.0\.0\.1:8081\/broadcast/);
  assert.match(bridge, /requiredEnv\('GLEAM_BROADCAST_SECRET'\)/);
  assert.doesNotMatch(bridge, /GLEAM_BROADCAST_SECRET \?\?/);
  assert.match(bridge, /'x-dd-internal-auth': broadcastSecret/);
  assert.match(bridge, /nats\.publish\(subject, body\)/);
  assert.match(deployment, /name:\s*nats-bridge/);
  assert.match(
    deployment,
    /cd \/opt\/dd-next-1\/remote\/deployments\/gleamlang-ws-server/,
  );
  assert.match(deployment, /GLEAM_NATS_PUBLISH_URL[\s\S]*127\.0\.0\.1:8083\/publish/);
  assert.match(deployment, /NATS_READ_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /NATS_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /NATS_PUBLISH_SUBJECT[\s\S]*dd\.remote\.websocket\.events/);
  assert.match(deployment, /name:\s*PG_DATABASE_URL[\s\S]*key:\s*RDS_DATABASE_URL/);
  assert.match(deployment, /name:\s*PRESENCE_NOTIFY_SHARDS[\s\S]*value:\s*"256"/);
  assert.match(deployment, /name:\s*PRESENCE_WAL_ENABLED[\s\S]*value:\s*"true"/);
  assert.match(deployment, /GLEAM_BROADCAST_URL[\s\S]*127\.0\.0\.1:8081\/broadcast/);
  assert.match(
    deployment,
    /name:\s*GLEAM_BROADCAST_SECRET[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-gleamlang-server-secrets[\s\S]*key:\s*GLEAM_BROADCAST_SECRET/,
  );
  assert.match(minikubeDeployment, /name:\s*nats-bridge/);
  assert.match(minikubeDeployment, /image:\s*dd-gleamlang-ws-server:dev/);
  assert.match(minikubeDeployment, /exec node \/app\/remote\/deployments\/gleamlang-ws-server\/nats-bridge\.mjs/);
  assert.match(minikubeDeployment, /NATS_READ_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(minikubeDeployment, /NATS_PUBLISH_SUBJECT[\s\S]*dd\.remote\.websocket\.events/);
  assert.match(minikubeDeployment, /name:\s*PG_DATABASE_URL[\s\S]*key:\s*RDS_DATABASE_URL/);
  assert.match(minikubeDeployment, /name:\s*PRESENCE_NOTIFY_SHARDS[\s\S]*value:\s*"256"/);
  assert.match(minikubeDeployment, /name:\s*PRESENCE_WAL_ENABLED[\s\S]*value:\s*"true"/);
  assert.match(
    minikubeDeployment,
    /name:\s*GLEAM_BROADCAST_SECRET[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-gleamlang-server-secrets[\s\S]*key:\s*GLEAM_BROADCAST_SECRET/,
  );
});

test('rust task page opens websocket before dispatch and dedupes with sse fallback', async () => {
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  assert.match(home, /new WebSocket\(wsUrl\)/);
  assert.match(home, /\/gleam\/ws\?threadId=/);
  assert.match(home, /select id="dispatch-mode"/);
  assert.match(home, /option value="queued" selected \{ "queued NATS" \}/);
  assert.match(home, /const usesQueuedDispatch = dispatchMode === "queued" \|\| dispatchMode === "queued-pool"/);
  assert.match(home, /const dispatchStatus = usesQueuedDispatch \? "queued via NATS" : "waking worker"/);
  assert.match(home, /clearStream\(dispatchStatus\)/);
  assert.match(home, /openGleamLiveSocket\(threadId, taskId\);[\s\S]*fetch\(`\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/tasks`/);
  assert.match(home, /if \(!usesQueuedDispatch\) openLiveStream\(threadId, taskId\)/);
  assert.match(home, /new EventSource\(`\/api\/agents\/threads\/\$\{encodeURIComponent\(threadId\)\}\/stream\/\$\{encodeURIComponent\(taskId\)\}`\)/);
  assert.match(home, /messageId/);
  assert.match(home, /state\.renderedEvents\.has\(key\)/);
  assert.match(home, /adminPreview\("dispatch response body", body\)/);
  assert.match(home, /task-event/);
});
