import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/mdp-optimizer-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('rust mdp optimizer is deployed, scraped, and connected to nats', async () => {
  const cargo = await readRepoFile('remote/deployments/mdp-optimizer-rs/Cargo.toml');
  const source = await readRepoFile('remote/deployments/mdp-optimizer-rs/src/main.rs');
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-mdp-optimizer.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-mdp-optimizer.service.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const otel = await readRepoFile('remote/argocd/observability/otel-collector.configmap.yaml');
  const prometheus = await readRepoFile('remote/argocd/observability/prometheus.configmap.yaml');
  const home = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  assert.match(cargo, /name\s*=\s*"dd-mdp-optimizer"/);
  assert.match(cargo, /async-nats\s*=\s*"=0\.38\.0"/);
  // Source-of-truth NATS subject + queue group constants come from the
  // generated @dd/nats-subject-defs crate.
  assert.match(cargo, /dd-nats-subject-defs\s*=\s*\{\s*path/);
  assert.match(
    source,
    /use dd_nats_subject_defs::\{[\s\S]*?MDP_OPTIMIZE_QUEUE_GROUP[\s\S]*?MDP_OPTIMIZE_SUBJECT[\s\S]*?MDP_RESULTS_SUBJECT[\s\S]*?RUNTIME_EVENTS_SUBJECT[\s\S]*?TELEMETRY_MDP_SUBJECT[\s\S]*?\};/,
  );
  assert.match(source, /fn optimize\(request: OptimizationRequest\)/);
  assert.match(source, /spawn_blocking\(move \|\| optimize\(request\)\)/);
  assert.match(source, /tokio::spawn\(async move/);
  assert.match(source, /gamma must be finite and in \[0, 1\)/);
  assert.match(source, /value iteration|mdp\.value-iteration/);
  assert.match(source, /BeliefSummary/);
  assert.match(source, /queue_subscribe\(subject, queue_group\)/);
  assert.match(source, /TelemetryLearningRequest/);
  assert.match(source, /optimize_telemetry_in_background/);
  assert.match(source, /spawn_blocking\(move \|\| optimize_telemetry\(request\)\)/);
  assert.match(source, /MAX_TELEMETRY_SIGNALS/);
  assert.match(source, /DefaultBodyLimit::max\(MAX_HTTP_BODY_BYTES\)/);
  assert.match(source, /payload\.len\(\) > MAX_NATS_PAYLOAD_BYTES/);
  assert.match(source, /bounded_impact_delta/);
  assert.match(source, /MDP_OPTIMIZE_SUBJECT/);
  assert.match(source, /TELEMETRY_MDP_SUBJECT/);
  assert.match(source, /MDP_RESULTS_SUBJECT/);
  assert.match(source, /RUNTIME_EVENTS_SUBJECT/);
  assert.match(source, /MDP_OPTIMIZE_QUEUE_GROUP/);
  assert.match(source, /dd_mdp_optimizer_optimizations_total/);
  assert.match(source, /dd_mdp_optimizer_telemetry_requests_total/);
  assert.match(source, /dd_mdp_optimizer_nats_telemetry_messages_total/);
  assert.match(source, /\.route\("\/healthz", get\(healthz\)\)/);
  assert.match(source, /\.route\("\/metrics", get\(metrics\)\)/);
  assert.match(source, /\.route\("\/telemetry\/learn", post\(telemetry_learning_http\)\)/);

  assert.match(deployment, /name:\s*dd-mdp-optimizer/);
  assert.match(deployment, /PORT[\s\S]*value:\s*'8096'/);
  assert.match(deployment, /NATS_URL[\s\S]*dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /MDP_OPTIMIZE_SUBJECT[\s\S]*dd\.remote\.mdp\.optimize/);
  assert.match(deployment, /MDP_TELEMETRY_SUBJECT[\s\S]*dd\.remote\.telemetry\.mdp/);
  assert.match(deployment, /MDP_TELEMETRY_QUEUE_GROUP[\s\S]*dd-mdp-telemetry-learner/);
  assert.match(deployment, /MDP_RESULT_SUBJECT[\s\S]*dd\.remote\.mdp\.results/);
  assert.match(deployment, /MDP_EVENT_SUBJECT[\s\S]*dd\.remote\.events/);
  assert.match(deployment, /startupProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /readinessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(deployment, /livenessProbe:[\s\S]*path: \/healthz[\s\S]*port: http/);
  assert.match(service, /port:\s*8096/);
  assert.match(service, /targetPort:\s*http/);
  assert.match(kustomization, /dd-mdp-optimizer\.deployment\.yaml/);
  assert.match(kustomization, /dd-mdp-optimizer\.service\.yaml/);
  assert.match(gateway, /location = \/mdp[\s\S]*return 302 \/mdp\//);
  assert.match(gateway, /dd-mdp-optimizer\.default\.svc\.cluster\.local:8096/);
  assert.match(otel, /job_name:\s*dd-mdp-optimizer/);
  assert.match(prometheus, /job_name:\s*dd-mdp-optimizer/);
  assert.match(home, /Rust MDP\/POMDP optimizer/);
  assert.match(home, /\/mdp\/telemetry\/learn/);
  assert.match(home, /dd\.remote\.telemetry\.mdp/);
});
