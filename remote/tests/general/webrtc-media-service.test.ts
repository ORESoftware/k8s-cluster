import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

const repoRoot = resolve(process.cwd(), '..', '..');

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust webrtc media config service stays separate from the signaling data path', async () => {
  const server = await readRepoFile('remote/deployments/webrtc-media-rs/src/main.rs');
  const readme = await readRepoFile('remote/deployments/webrtc-media-rs/readme.md');
  const cargo = await readRepoFile('remote/deployments/webrtc-media-rs/Cargo.toml');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-webrtc-media.deployment.yaml',
  );
  const service = await readRepoFile('remote/argocd/dd-next-runtime/dd-webrtc-media.service.yaml');
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

  assert.match(cargo, /name = "dd-webrtc-media"/);
  assert.match(cargo, /dd-runtime-config-client/);
  assert.match(server, /const DEFAULT_PORT: u16 = 8125/);
  assert.match(server, /WEBRTC_MEDIA_MODE/);
  assert.match(server, /WEBRTC_TURN_URLS/);
  assert.match(server, /WEBRTC_SFU_ENDPOINT/);
  assert.match(server, /\.route\("\/webrtc-media\/config", get\(media_config\)\)/);
  assert.match(server, /\.route\("\/webrtc-media\/ice", get\(ice\)\)/);
  assert.match(server, /\.route\("\/webrtc-media\/metrics", get\(metrics\)\)/);
  assert.match(server, /dd_webrtc_media_ready/);
  assert.match(server, /dd_webrtc_media_capability_enabled/);
  assert.match(server, /dd_runtime_config_client::router/);
  assert.match(readme, /does not relay audio, video, data-channel packets, or TURN UDP packets/);
  assert.match(readme, /WEBRTC_MEDIA_MODE/);
  assert.match(readme, /backing data-plane service must exist/);
  assert.match(deployment, /name:\s*dd-webrtc-media/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/webrtc-media-rs/);
  assert.match(deployment, /WEBRTC_MEDIA_MODE[\s\S]*value:\s*disabled/);
  assert.match(deployment, /WEBRTC_TURN_CREDENTIAL[\s\S]*optional:\s*true/);
  assert.match(deployment, /containerPort:\s*8125/);
  assert.match(service, /name:\s*dd-webrtc-media/);
  assert.match(service, /port:\s*8125/);
  assert.match(runtimeKustomization, /dd-webrtc-media\.deployment\.yaml/);
  assert.match(runtimeKustomization, /dd-webrtc-media\.service\.yaml/);
  assert.match(gateway, /location = \/webrtc-media[\s\S]*return 302 \/webrtc-media\//);
  assert.match(
    gateway,
    /location\s+\/webrtc-media\/[\s\S]*dd-webrtc-media\.default\.svc\.cluster\.local:8125/,
  );
  assert.match(otelCollector, /job_name: dd-webrtc-media/);
  assert.match(otelCollector, /dd-webrtc-media\.default\.svc\.cluster\.local:8125/);
  assert.match(prometheus, /job_name: dd-webrtc-media/);
  assert.match(prometheus, /dd-webrtc-media\.default\.svc\.cluster\.local:8125/);
});
