import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/des-simulator-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust discrete event simulator declares, validates, and runs async DES jobs', async () => {
  const cargo = await readRepoFile('remote/des-simulator-rs/Cargo.toml');
  const source = await readRepoFile('remote/des-simulator-rs/src/main.rs');
  const readme = await readRepoFile('remote/des-simulator-rs/readme.md');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-des-simulator.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-des-simulator.service.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const home = await readRepoFile('remote/web-home-rs/src/main.rs');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(cargo, /name = "dd-des-simulator"/);
  assert.match(cargo, /async-nats = "=0\.38\.0"/);
  assert.match(source, /const MODEL_SCHEMA_VERSION: &str = "des\.v1"/);
  assert.match(source, /struct SimulationRequest/);
  assert.match(source, /struct SimulationModel/);
  assert.match(source, /enum DelaySpec/);
  assert.match(source, /fn model_schema\(\) -> Value/);
  assert.match(source, /fn validate_simulation_request\(request: &SimulationRequest\)/);
  assert.match(source, /fn simulate\(request: SimulationRequest, job_id: String\)/);
  assert.match(source, /tokio::task::spawn_blocking/);
  assert.match(source, /tokio::spawn\(async move/);
  assert.match(source, /BinaryHeap/);
  assert.match(source, /ResourceState/);
  assert.match(source, /DefaultBodyLimit::max\(MAX_HTTP_BODY_BYTES\)/);
  assert.match(source, /payload\.len\(\) > MAX_NATS_PAYLOAD_BYTES/);
  assert.match(source, /\.route\("\/model\/schema", get\(schema_http\)\)/);
  assert.match(source, /\.route\("\/validate", post\(validate_with_metrics\)\)/);
  assert.match(source, /\.route\("\/simulate", post\(simulate_http\)\)/);
  assert.match(source, /\.route\("\/simulations\/:job_id", get\(job_status\)\)/);
  assert.match(source, /dd\.remote\.des\.simulate/);
  assert.match(source, /dd\.remote\.des\.results/);
  assert.match(source, /dd_des_simulator_jobs_started_total/);
  assert.match(source, /dd_des_simulator_validation_errors_total/);
  assert.match(source, /model\.schemaVersion must be \{MODEL_SCHEMA_VERSION\}/);
  assert.match(readme, /GET \/model\/schema/);
  assert.match(readme, /POST \/validate/);
  assert.match(readme, /POST \/simulate/);
  assert.match(readme, /schemaVersion": "des\.v1"/);

  assert.match(deployment, /name:\s*dd-des-simulator/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/des-simulator-rs/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8099'/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /DES_SIMULATE_SUBJECT[\s\S]*dd\.remote\.des\.simulate/);
  assert.match(deployment, /DES_QUEUE_GROUP[\s\S]*dd-des-simulator/);
  assert.match(deployment, /DES_RESULT_SUBJECT[\s\S]*dd\.remote\.des\.results/);
  assert.match(deployment, /DES_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /name:\s*dd-des-simulator/);
  assert.match(service, /port:\s*8099/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(kustomization, /dd-des-simulator\.deployment\.yaml/);
  assert.match(kustomization, /dd-des-simulator\.service\.yaml/);
  assert.match(gateway, /location = \/des[\s\S]*return 302 \/des\//);
  assert.match(
    gateway,
    /location \/des\/[\s\S]*dd-des-simulator\.default\.svc\.cluster\.local:8099\//,
  );
  assert.match(prometheus, /job_name:\s*dd-des-simulator/);
  assert.match(prometheus, /dd-des-simulator\.default\.svc\.cluster\.local:8099/);
  assert.match(otel, /job_name:\s*dd-des-simulator/);
  assert.match(otel, /dd-des-simulator\.default\.svc\.cluster\.local:8099/);
  assert.match(home, /Rust discrete event simulator/);
  assert.match(home, /\/des\/model\/schema/);
  assert.match(home, /dd\.remote\.des\.simulate/);
  assert.match(runtimeReadme, /`dd-des-simulator`/);
  assert.match(runtimeReadme, /\/des\/model\/schema/);
});
