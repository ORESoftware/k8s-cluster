import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('node task workers publish every stream event to nats for websocket fanout', async () => {
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  const publisher = await readRepoFile('remote/deployments/dev-server/src/nats-publisher.ts');
  const wsFanout = await readRepoFile('remote/deployments/dev-server/src/ws-fanout.ts');
  const packageJson = await readRepoFile('remote/deployments/dev-server/package.json');
  const restApi = [
    await readRepoFile('remote/deployments/rest-api-rs/src/events.rs'),
    await readRepoFile('remote/deployments/rest-api-rs/src/dispatch.rs'),
    await readRepoFile('remote/deployments/rest-api-rs/src/shared.rs'),
    await readRepoFile('remote/deployments/rest-api-rs/src/threads.rs'),
  ].join('\n');
  const bootstrapDeployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-dev-server-home.deployment.yaml',
  );

  assert.match(server, /new NatsPublisher\(config\.natsUrl\)/);
  // dev-server now sources the default NATS event subject from the
  // generated `dd-nats-subject-defs` mirror module instead of inlining the
  // magic string, so the test asserts on the source-of-truth wiring.
  assert.match(
    server,
    /natsEventSubject: process\.env\.NATS_EVENT_SUBJECT \?\? RUNTIME_EVENTS_SUBJECT/,
  );
  assert.match(
    server,
    /import \{ RUNTIME_EVENTS_SUBJECT \} from '\.\/nats-subject-defs\.js';/,
  );
  // Local mirror equals the generated source of truth.
  const mirror = await readRepoFile(
    'remote/deployments/dev-server/src/nats-subject-defs.ts',
  );
  const generatedTs = await readRepoFile(
    'remote/libs/nats/subject-defs/generated/typescript/index.ts',
  );
  const mirrorValue = /RUNTIME_EVENTS_SUBJECT = '([^']+)'/.exec(mirror)?.[1];
  const generatedValue = /RUNTIME_EVENTS_SUBJECT = "([^"]+)"/.exec(generatedTs)?.[1];
  assert.ok(mirrorValue, 'mirror RUNTIME_EVENTS_SUBJECT missing');
  assert.equal(
    mirrorValue,
    generatedValue,
    'dev-server/src/nats-subject-defs.ts must mirror the generated lib value',
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
  const fullServerBridge = await readRepoFile(
    'remote/deployments/gleamlang-server/nats-bridge.mjs',
  );
  const fullServerNatsClient = await readRepoFile(
    'remote/deployments/gleamlang-server/nats-client.mjs',
  );
  const dockerfile = await readRepoFile('remote/deployments/gleamlang-ws-server/Dockerfile');
  const deployment = await readRepoFile(
    'remote/deployments/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
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
  // The NATS_PUBLISH_SUBJECT default now resolves through dd_nats_subject_consts
  // (auto-generated mirror inside the Gleam dd_nats_subject_defs dep) so a
  // subject rename in remote/libs/nats/subject-defs/schema/runtime-events.schema.json
  // is caught here at build time.
  assert.match(env, /dd_nats_subject_consts:websocket_events_subject\(\)/);
  assert.match(env, /gen_tcp:connect\(Host, Port/);
  assert.match(env, /"POST ", Path, " HTTP\/1\.1\\r\\n"/);
  assert.doesNotMatch(env, /httpc:request\(post/);
  assert.match(natsClient, /let singleton = null/);
  assert.match(natsClient, /export function getNatsClient/);
  assert.match(natsClient, /net\.createConnection/);
  assert.match(natsClient, /SUB \$\{subscription\.subject\} \$\{sid\}/);
  assert.match(natsClient, /PUB \$\{next\.subject\} \$\{next\.payload\.length\}/);
  assert.match(natsClient, /PONG\\r\\n/);
  assert.equal(
    natsClient,
    fullServerNatsClient,
    'both websocket deployments must use the same hardened raw NATS client',
  );
  assert.match(natsClient, /MAX_QUEUE_BYTES = 8 \* 1024 \* 1024/);
  assert.match(natsClient, /MAX_PAYLOAD_BYTES = 1024 \* 1024/);
  assert.match(natsClient, /MAX_INBOUND_BUFFER_BYTES = 2 \* 1024 \* 1024/);
  assert.match(natsClient, /queueBytes/);
  assert.match(natsClient, /waitingForDrain/);
  assert.match(natsClient, /socket\.once\('drain'/);
  assert.match(natsClient, /MAX_RECONNECT_MS/);
  assert.match(natsClient, /2 \*\* this\.reconnectAttempts/);
  assert.match(natsClient, /0\.8 \+ Math\.random\(\) \* 0\.4/);
  assert.match(natsClient, /inbound buffer limit exceeded/);
  assert.match(natsClient, /control line limit exceeded/);
  assert.match(natsClient, /invalid MSG terminator/);
  assert.match(natsClient, /auth_token/);
  assert.doesNotMatch(natsClient, /invalid NATS_URL: \$\{this\.url\}/);
  assert.match(dockerfile, /apk add --no-cache nodejs/);
  assert.match(bridge, /getNatsClient/);
  assert.match(bridge, /NATS_BRIDGE_DEDUPE_TTL_MS/);
  assert.match(bridge, /seenMessageIds/);
  assert.match(bridge, /dropDuplicate\(event\.messageId\)/);
  assert.match(bridge, /normalizeEvent/);
  assert.match(bridge, /NATS_READ_SUBJECT/);
  assert.match(bridge, /NATS_PUBLISH_SUBJECT/);
  // The nats-bridge .mjs files default the read/publish subjects from the
  // generated `dd-nats-subject-defs` mirror module so a schema rename
  // propagates through both directions of the websocket fan-out without
  // touching this bridge file.
  assert.match(
    bridge,
    /import \{\s*RUNTIME_EVENTS_SUBJECT,\s*WEBSOCKET_EVENTS_SUBJECT,?\s*\} from '\.\.\/\.\.\/libs\/nats\/subject-defs\/generated\/javascript\/index\.mjs';/,
  );
  assert.match(bridge, /createServer/);
  assert.match(bridge, /url\.pathname !== '\/publish'/);
  assert.match(bridge, /http:\/\/127\.0\.0\.1:8081\/broadcast/);
  assert.match(bridge, /requiredEnv\('GLEAM_BROADCAST_SECRET'\)/);
  assert.doesNotMatch(bridge, /GLEAM_BROADCAST_SECRET \?\?/);
  assert.match(bridge, /'x-dd-internal-auth': broadcastSecret/);
  assert.match(bridge, /nats\.publish\(subject, body\)/);
  for (const candidate of [bridge, fullServerBridge]) {
    assert.match(candidate, /timingSafeEqual/);
    assert.match(candidate, /NATS_BRIDGE_MAX_BROADCAST_CONCURRENCY/);
    assert.match(candidate, /NATS_BRIDGE_MAX_BROADCAST_QUEUE_BYTES/);
    assert.match(candidate, /NATS_BRIDGE_MAX_DEDUPE_ENTRIES/);
    assert.match(candidate, /AbortSignal\.timeout\(broadcastTimeoutMs\)/);
    assert.match(candidate, /subject !== publishSubject/);
    assert.match(candidate, /subject-not-allowed/);
    assert.match(candidate, /invalid-json-event/);
    assert.match(candidate, /server\.maxRequestsPerSocket = 100/);
    assert.match(candidate, /'x-content-type-options': 'nosniff'/);
    assert.match(candidate, /service: 'dd-nats-bridge'/);
    assert.match(candidate, /process\.once\('SIGTERM', shutdown\)/);
    assert.match(candidate, /nats\.destroy\(\)/);
  }
  assert.match(deployment, /name:\s*nats-bridge/);
  assert.match(
    deployment,
    /exec node \/opt\/dd-next-1\/remote\/deployments\/gleamlang-ws-server\/nats-bridge\.mjs/,
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
});

test('raw nats bridge client enforces byte, depth, payload, and subject bounds', async () => {
  const moduleUrl = pathToFileURL(
    resolve(repoRoot, 'remote/deployments/gleamlang-ws-server/nats-client.mjs'),
  ).href;
  const { NatsClient } = (await import(moduleUrl)) as {
    NatsClient: new (options: Record<string, unknown>) => {
      connect: () => void;
      destroy: () => void;
      publish: (subject: string, payload: string | Buffer) => boolean;
      queue: Array<{ payload: Buffer }>;
      queueBytes: number;
    };
  };
  const warnings: string[] = [];
  const client = new NatsClient({
    url: 'nats://127.0.0.1:4222',
    logger: { warn: (message: string) => warnings.push(message) },
    maxQueueDepth: 2,
    maxQueueBytes: 8,
    maxPayloadBytes: 4,
  });
  // Keep this unit test deterministic and offline; queue behavior does not
  // require opening a socket.
  client.connect = () => {};

  assert.equal(client.publish('dd.remote.events', 'aaaa'), true);
  assert.equal(client.publish('dd.remote.events', 'bbbb'), true);
  assert.equal(client.publish('dd.remote.events', 'cccc'), true);
  assert.equal(client.queue.length, 2);
  assert.equal(client.queueBytes, 8);
  assert.deepEqual(
    client.queue.map((entry) => entry.payload.toString('utf8')),
    ['bbbb', 'cccc'],
  );
  assert.ok(warnings.some((message) => message.includes('outbound queue full')));
  assert.throws(
    () => client.publish('dd.remote.events', 'oversized'),
    /payload exceeds 4 byte limit/,
  );
  assert.throws(
    () => client.publish('dd.remote.*', 'ok'),
    /invalid NATS publish subject/,
  );
  client.destroy();
});

test('rust task page opens websocket before dispatch and dedupes with sse fallback', async () => {
  const home = await readRepoFile('remote/deployments/web-home-rs/src/agents.rs');

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
