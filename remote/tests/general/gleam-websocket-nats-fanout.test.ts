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
  assert.match(server, /natsPublisher\.publish\(config\.natsEventSubject/);
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
  const bridge = await readRepoFile('remote/gleamlang-server/nats-bridge.mjs');
  const deployment = await readRepoFile(
    'remote/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
  );
  const minikubeDeployment = await readRepoFile(
    'remote/gleamlang-server/k8s/dd-gleamlang-server.deployment.yaml',
  );

  assert.match(broadcaster, /BroadcastJson\(payload: String\)/);
  assert.match(broadcaster, /dd_gleamlang_nats_messages_total|nats_messages/);
  assert.match(httpServer, /\["broadcast"\] -> broadcast\(req, broker_name\)/);
  assert.match(httpServer, /mist\.read_body\(req, 1_048_576\)/);
  assert.match(httpServer, /broadcaster\.BroadcastJson\(payload\)/);
  assert.match(httpServer, /os\.get_env\("GLEAM_BROADCAST_SECRET"\)/);
  assert.match(bridge, /net\.createConnection/);
  assert.match(bridge, /SUB \$\{subject\} 1/);
  assert.match(bridge, /http:\/\/127\.0\.0\.1:8081\/broadcast/);
  assert.match(bridge, /requiredEnv\('GLEAM_BROADCAST_SECRET'\)/);
  assert.doesNotMatch(bridge, /GLEAM_BROADCAST_SECRET \?\?/);
  assert.match(bridge, /'x-dd-internal-auth': broadcastSecret/);
  assert.match(deployment, /name:\s*nats-bridge/);
  assert.match(deployment, /NATS_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /GLEAM_BROADCAST_URL[\s\S]*127\.0\.0\.1:8081\/broadcast/);
  assert.match(
    deployment,
    /name:\s*GLEAM_BROADCAST_SECRET[\s\S]*valueFrom:[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-gleamlang-server-secrets[\s\S]*key:\s*GLEAM_BROADCAST_SECRET/,
  );
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
