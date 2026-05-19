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
  const server = await readRepoFile('remote/dev-server/src/server.ts');
  const publisher = await readRepoFile('remote/dev-server/src/nats-publisher.ts');
  const wsFanout = await readRepoFile('remote/dev-server/src/ws-fanout.ts');
  const restApi = await readRepoFile('remote/rest-api-rs/src/main.rs');
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
  assert.match(publisher, /PUB \$\{next\.subject\} \$\{bytes\}/);
  assert.match(publisher, /PONG\\r\\n/);
  assert.match(restApi, /async_nats::connect\(nats_url\(\)\)/);
  assert.match(restApi, /async fn publish_task_shadow_to_nats/);
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
    'remote/gleamlang-server/src/gleamlang_server/broadcaster.gleam',
  );
  const httpServer = await readRepoFile(
    'remote/gleamlang-server/src/gleamlang_server/http_server.gleam',
  );
  const env = await readRepoFile('remote/gleamlang-server/src/gleamlang_server_env.erl');
  const bridge = await readRepoFile('remote/gleamlang-server/nats-bridge.mjs');
  const natsClient = await readRepoFile('remote/gleamlang-server/nats-client.mjs');
  const dockerfile = await readRepoFile('remote/gleamlang-server/Dockerfile');
  const deployment = await readRepoFile(
    'remote/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/gleamlang-server/k8s/minikube/dd-gleamlang-server.deployment.yaml',
  );

  assert.match(broadcaster, /BroadcastJson\(payload: String\)/);
  assert.match(broadcaster, /const dedupe_ttl_ms = 300_000/);
  assert.match(broadcaster, /json_message_id\(payload\)/);
  assert.match(broadcaster, /SeenMessage\(id: message_id, expires_at_ms: now \+ dedupe_ttl_ms\)/);
  assert.match(broadcaster, /dd_gleamlang_nats_messages_total|nats_messages/);
  assert.match(httpServer, /\["broadcast"\] -> broadcast\(req, broker_name\)/);
  assert.match(httpServer, /\["worker-ws", secret\] -> worker_websocket\(req, broker_name, secret\)/);
  assert.match(httpServer, /mist\.read_body\(req, 1_048_576\)/);
  assert.match(httpServer, /broadcaster\.BroadcastJson\(payload\)/);
  assert.match(httpServer, /env_get\("GLEAM_BROADCAST_SECRET"\)/);
  assert.match(httpServer, /env_get\("GLEAM_WORKER_WS_SECRET"\)/);
  assert.match(httpServer, /nats_publish\(payload\)/);
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
  assert.match(deployment, /GLEAM_NATS_PUBLISH_URL[\s\S]*127\.0\.0\.1:8083\/publish/);
  assert.match(deployment, /NATS_READ_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /NATS_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /NATS_PUBLISH_SUBJECT[\s\S]*dd\.remote\.websocket\.events/);
  assert.match(deployment, /GLEAM_BROADCAST_URL[\s\S]*127\.0\.0\.1:8081\/broadcast/);
  assert.match(
    deployment,
    /name:\s*GLEAM_BROADCAST_SECRET[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-gleamlang-server-secrets[\s\S]*key:\s*GLEAM_BROADCAST_SECRET/,
  );
  assert.match(minikubeDeployment, /name:\s*nats-bridge/);
  assert.match(minikubeDeployment, /exec node \/app\/remote\/gleamlang-server\/nats-bridge\.mjs/);
  assert.match(minikubeDeployment, /NATS_READ_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(minikubeDeployment, /NATS_PUBLISH_SUBJECT[\s\S]*dd\.remote\.websocket\.events/);
  assert.match(
    minikubeDeployment,
    /name:\s*GLEAM_BROADCAST_SECRET[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-gleamlang-server-secrets[\s\S]*key:\s*GLEAM_BROADCAST_SECRET/,
  );
});

test('rust task page opens websocket before dispatch and dedupes with sse fallback', async () => {
  const home = await readRepoFile('remote/web-home-rs/src/main.rs');

  assert.match(home, /new WebSocket\(wsUrl\)/);
  assert.match(home, /\/gleam\/ws\?threadId=/);
  assert.match(home, /openTaskWebSocket\(threadId, taskId\);[\s\S]*POST \$\{route\}/);
  assert.match(home, /new EventSource\(streamUrl\)/);
  assert.match(home, /seenStreamEvents/);
  assert.match(home, /messageId/);
  assert.match(home, /const resetRealtimeState = \(threadId, taskId\) =>/);
  assert.match(home, /activeTaskKey = `\$\{threadId\}:\$\{taskId\}`;/);
  assert.match(
    home,
    /if \(activeTaskKey && `\$\{threadId \|\| ""\}:\$\{taskId \|\| ""\}` !== activeTaskKey\) return false;/,
  );
  assert.match(home, /appendStreamLine\("websocket connected"\)/);
  assert.match(home, /worker websocket connected/);
  assert.match(home, /renderStreamEvent\("message", event\.data, "worker-ws"\)/);
  assert.match(home, /appendStreamLine\(`dispatch failed \$\{response\.status\}:/);
  assert.match(home, /task-event/);
});
