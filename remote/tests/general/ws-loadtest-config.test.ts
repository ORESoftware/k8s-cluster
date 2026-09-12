import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

function parseNumericEnv(yamlText: string, key: string): number {
  const matcher = new RegExp(`name:\\s*${key}[\\s\\S]*?value:\\s*"?(\\d+)"?`, 'm');
  const match = yamlText.match(matcher);
  assert.ok(match, `missing env var ${key}`);
  return Number.parseInt(match[1]!, 10);
}

function yamlScalar(value: string): string {
  return `['"]?${value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}['"]?`;
}

function assertLoadtestMatrix(manifest: string, label: string): void {
  assert.match(
    manifest,
    /name:\s*MESSAGE_ENCODINGS[\s\S]*value:\s*"json,msgpack,protobuf,flatbuffers"/,
    `${label} should advertise JSON, MessagePack, protobuf, and FlatBuffers`,
  );
  assert.match(
    manifest,
    /name:\s*LOADTEST_TRANSPORTS[\s\S]*value:\s*"http,tcp,websocket"/,
    `${label} should advertise HTTP, TCP, and WebSocket transport coverage`,
  );
}

test('ws loadtest manifests configure rust + gleam clients against websocket endpoint', async () => {
  const rustDeployment = await readRepoFile(
    'remote/deployments/ws-loadtest-rs/k8s/ec2/dd-ws-loadtest-rs.deployment.yaml',
  );
  const gleamDeployment = await readRepoFile(
    'remote/deployments/gleamlang-ws-loadtest/k8s/ec2/dd-gleamlang-ws-loadtest.deployment.yaml',
  );
  const gleamServerDeployment = await readRepoFile(
    'remote/deployments/gleamlang-server/k8s/ec2/dd-gleamlang-server.deployment.yaml',
  );

  const rustCount = parseNumericEnv(rustDeployment, 'CLIENT_COUNT');
  const gleamCount = parseNumericEnv(gleamDeployment, 'CLIENT_COUNT');
  const total = rustCount + gleamCount;

  assert.equal(rustCount, 7_500, 'rust loadtest must open 7.5k clients');
  assert.equal(gleamCount, 5_000, 'gleam loadtest must open 5k clients');
  assert.equal(total, 12_500, 'combined loadtest target must be 12.5k clients');

  assert.match(
    rustDeployment,
    /ws:\/\/dd-gleamlang-server\.default\.svc\.cluster\.local:8081\/ws/,
    'rust deployment should target gleam websocket endpoint',
  );
  assert.match(
    gleamDeployment,
    /ws:\/\/dd-gleamlang-server\.default\.svc\.cluster\.local:8081\/ws/,
    'gleam deployment should target gleam websocket endpoint',
  );
  assert.match(
    gleamDeployment,
    /image:\s*ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-node-alpine/,
    'gleam loadtest deployment should use the node-enabled gleam runtime image',
  );
  assert.match(
    rustDeployment,
    /image:\s*docker\.io\/library\/rust:1\.90-bookworm/,
    'rust loadtest deployment should use the full rust toolchain image for cargo run',
  );
  assert.match(
    rustDeployment,
    /\/usr\/local\/cargo\/bin\/cargo run --release/,
    'rust deployment should call cargo from absolute path',
  );
  assert.match(
    gleamDeployment,
    /name:\s*LOAD_MODE[\s\S]*value:\s*"pipeline"/,
    'gleam loadtest should use rate-driven pipeline mode',
  );
  assertLoadtestMatrix(rustDeployment, 'rust websocket loadtest');
  assertLoadtestMatrix(gleamDeployment, 'gleam websocket loadtest');
  assert.match(
    gleamServerDeployment,
    /ghcr\.io\/gleam-lang\/gleam:v1\.16\.0-erlang-alpine/,
    'gleam server deployment should use gleam 1.16 erlang runtime',
  );
  assert.match(
    gleamServerDeployment,
    new RegExp(
      `requests:\\s*[\\s\\S]*cpu:\\s*${yamlScalar('250m')}[\\s\\S]*memory:\\s*${yamlScalar('2Gi')}`,
    ),
    'gleam server deployment should request enough CPU and memory for steady serving',
  );
  assert.match(
    gleamServerDeployment,
    new RegExp(
      `limits:\\s*[\\s\\S]*cpu:\\s*${yamlScalar('6')}[\\s\\S]*memory:\\s*${yamlScalar('8Gi')}`,
    ),
    'gleam server deployment should cap benchmark CPU while reserving 8Gi memory',
  );
});

test('gcs websocket loadtest deployments advertise the encoding and transport matrix', async () => {
  const rustGcs = await readRepoFile(
    'remote/deployments/ws-loadtest-rs/k8s/gcs/dd-ws-loadtest-rs-gcs.deployment.yaml',
  );
  const nodeGcs = await readRepoFile(
    'remote/deployments/gleamlang-ws-loadtest/k8s/gcs-node/dd-nodejs-ws-loadtest-gcs.deployment.yaml',
  );
  const gleamGcs = await readRepoFile(
    'remote/deployments/gleamlang-ws-loadtest/k8s/gcs/dd-gleamlang-ws-loadtest-gcs.deployment.yaml',
  );

  assertLoadtestMatrix(rustGcs, 'rust gcs loadtest');
  assertLoadtestMatrix(nodeGcs, 'nodejs gcs loadtest');
  assertLoadtestMatrix(gleamGcs, 'gleam gcs loadtest');
  for (const [label, manifest] of [
    ['rust gcs loadtest', rustGcs],
    ['nodejs gcs loadtest', nodeGcs],
    ['gleam gcs loadtest', gleamGcs],
  ] as const) {
    assert.match(
      manifest,
      /name:\s*GCS_MESSAGE_ENCODING[\s\S]*value:\s*"protobuf"/,
      `${label} should run the GCS hot path with protobuf frames`,
    );
  }
});

test('argo applications point to both websocket loadtest deployments', async () => {
  const rustApp = await readRepoFile('remote/argocd/apps/dd-ws-loadtest-rs.application.yaml');
  const gleamApp = await readRepoFile('remote/argocd/apps/dd-ws-loadtest-gleam.application.yaml');

  assert.match(
    rustApp,
    /path:\s*remote\/deployments\/ws-loadtest-rs\/k8s\/ec2/,
    'rust app should source rust loadtest kustomization',
  );
  assert.match(
    gleamApp,
    /path:\s*remote\/deployments\/gleamlang-ws-loadtest\/k8s\/ec2/,
    'gleam app should source gleam loadtest kustomization',
  );
});

test('websocket loadtest clients include container-pool smoke mode', async () => {
  const rustCargo = await readRepoFile('remote/deployments/ws-loadtest-rs/Cargo.toml');
  const rustSource = await readRepoFile('remote/deployments/ws-loadtest-rs/src/main.rs');
  const rustReadme = await readRepoFile('remote/deployments/ws-loadtest-rs/README.md');
  const gleamClient = await readRepoFile(
    'remote/deployments/gleamlang-ws-loadtest/src/gleamlang_ws_loadtest/client.mjs',
  );
  const gleamReadme = await readRepoFile('remote/deployments/gleamlang-ws-loadtest/README.md');

  assert.match(rustCargo, /reqwest/);
  assert.match(rustCargo, /serde_json/);
  assert.match(rustSource, /CONTAINER_POOL_URL/);
  assert.match(rustSource, /enum MessageEncoding/);
  assert.match(rustSource, /MessageEncoding::MessagePack/);
  assert.match(rustSource, /MessageEncoding::Protobuf/);
  assert.match(rustSource, /MessageEncoding::FlatBuffers/);
  assert.match(rustSource, /encode_pipeline_message/);
  assert.match(rustSource, /GCS_MESSAGE_ENCODING/);
  assert.match(rustSource, /encode_gcs_protobuf_chat_frame/);
  assert.match(rustSource, /extract_id_from_binary/);
  assert.match(rustSource, /DEFAULT_LOADTEST_TRANSPORTS/);
  assert.match(rustSource, /run_container_pool_smoke/);
  assert.match(rustSource, /CONTAINER_POOL_POOL"\)\.unwrap_or_else\(\|_\| "rust"/);
  assert.match(rustSource, /"echoKey": echo_key/);
  assert.match(rustSource, /pointer\("\/body\/echoKey"\)/);
  assert.match(rustReadme, /Container pool smoke mode/);
  assert.match(gleamClient, /runContainerPoolSmoke/);
  assert.match(gleamClient, /SUPPORTED_MESSAGE_ENCODINGS/);
  assert.match(gleamClient, /encodeMsgpackPipelineMessage/);
  assert.match(gleamClient, /encodeProtobufPipelineMessage/);
  assert.match(gleamClient, /GCS_MESSAGE_ENCODING/);
  assert.match(gleamClient, /encodeGcsProtobufFrame/);
  assert.match(gleamClient, /encodeFlatbuffersPipelineMessage/);
  assert.match(gleamClient, /DEFAULT_LOADTEST_TRANSPORTS/);
  assert.match(gleamClient, /CONTAINER_POOL_POOL \|\| "gleamlang"/);
  assert.match(gleamClient, /crypto|randomUUID/);
  assert.match(gleamClient, /echoKey/);
  assert.match(gleamReadme, /Container pool smoke mode/);
});

test('gcs loadtest background jobs do not inherit parent cleanup trap', async () => {
  const workflow = await readRepoFile('.github/workflows/remote-k8s-maintenance.yml');

  assert.match(
    workflow,
    /\(\s*\n\s+trap - EXIT\s*\n\s+set \+e\s*\n\s+while true;/,
    'capacity guard background loop must not inherit the parent cleanup trap',
  );
  assert.match(
    workflow,
    /\( trap - EXIT; sleep "\$\{LOADTEST_PPROF_DELAY_SECONDS:-45\}"; collect_gcs_pprof "\$loader_name" \) &/,
    'serial pprof collector must not inherit the parent cleanup trap',
  );
  assert.match(
    workflow,
    /\( trap - EXIT; sleep "\$\{LOADTEST_PPROF_DELAY_SECONDS:-45\}"; collect_gcs_pprof "parallel-rust-node-gleam" \) &/,
    'parallel pprof collector must not inherit the parent cleanup trap',
  );

  for (const loader of [
    'run_loader dd-ws-loadtest-rs-gcs "Rust ws-loadtest-rs"',
    'run_loader dd-nodejs-ws-loadtest-gcs "Node.js ws-loadtest"',
    'run_loader dd-gleamlang-ws-loadtest-gcs "Gleam ws-loadtest"',
  ]) {
    assert.match(
      workflow,
      new RegExp(`\\(\\s*\\n\\s+trap - EXIT\\s*\\n[\\s\\S]{0,160}${loader.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`),
      `${loader} background job must not inherit the parent cleanup trap`,
    );
  }
});
