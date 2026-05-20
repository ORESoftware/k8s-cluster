import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust webrtc signaling service is a signaling-only deployment', async () => {
  const server = await readRepoFile('remote/deployments/webrtc-signaling-rs/src/main.rs');
  const readme = await readRepoFile('remote/deployments/webrtc-signaling-rs/readme.md');
  const cargo = await readRepoFile('remote/deployments/webrtc-signaling-rs/Cargo.toml');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-webrtc-signaling.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-webrtc-signaling.service.yaml',
  );
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const runtimeKustomization = await readRepoFile(
    'remote/argocd/dd-next-runtime/kustomization.yaml',
  );
  const otelCollector = await readRepoFile(
    'remote/argocd/observability/otel-collector.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');

  assert.match(cargo, /name = "dd-webrtc-signaling"/);
  assert.match(cargo, /axum = \{ version = "0\.7", features = \["macros", "ws"\] \}/);
  assert.match(server, /struct RoomState/);
  assert.match(server, /struct PeerConnection/);
  assert.match(server, /\.route\("\/webrtc", get\(root\)\)/);
  assert.match(server, /\.route\("\/webrtc\/", get\(root\)\)/);
  assert.match(server, /\.route\("\/webrtc\/healthz", get\(healthz\)\)/);
  assert.match(server, /\.route\("\/webrtc\/metrics", get\(metrics\)\)/);
  assert.match(server, /\.route\("\/webrtc\/signal", get\(signal_ws\)\)/);
  assert.match(server, /\.route\("\/signal", get\(signal_ws\)\)/);
  assert.match(server, /"supportedSignalTypes": \["hello", "ping", "offer", "answer", "ice", "candidate", "renegotiate", "message", "bye"\]/);
  assert.match(server, /"type": "peer-joined"/);
  assert.match(server, /"type": "peer-left"/);
  assert.match(server, /"type": "signal"/);
  assert.match(server, /"signalType": message_type/);
  assert.match(server, /dd_webrtc_active_connections/);
  assert.match(server, /dd_webrtc_active_rooms/);
  assert.match(server, /dd_webrtc_signal_messages_total/);
  assert.match(readme, /Why Signaling Instead Of A Media Relay/);
  assert.match(readme, /It intentionally does not relay audio, video, or WebRTC data-channel payloads\./);
  assert.match(readme, /Browser to browser, browser to mobile, and mobile to mobile all use the same/);
  assert.match(readme, /Add a\s+TURN server such as coturn/);
  assert.match(deployment, /name:\s*dd-webrtc-signaling/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/webrtc-signaling-rs/);
  assert.match(deployment, /containerPort:\s*8095/);
  assert.match(service, /name:\s*dd-webrtc-signaling/);
  assert.match(service, /port:\s*8095/);
  assert.match(runtimeKustomization, /dd-webrtc-signaling\.deployment\.yaml/);
  assert.match(runtimeKustomization, /dd-webrtc-signaling\.service\.yaml/);
  assert.match(
    gateway,
    /location\s+\/webrtc\/[\s\S]*proxy_set_header Upgrade \$http_upgrade[\s\S]*dd-webrtc-signaling\.default\.svc\.cluster\.local:8095/,
  );
  assert.match(otelCollector, /job_name: dd-webrtc-signaling/);
  assert.match(otelCollector, /dd-webrtc-signaling\.default\.svc\.cluster\.local:8095/);
  assert.match(prometheus, /job_name: dd-webrtc-signaling/);
  assert.match(prometheus, /dd-webrtc-signaling\.default\.svc\.cluster\.local:8095/);
});
