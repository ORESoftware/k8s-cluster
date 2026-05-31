import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/browser-job-runner-rs/Cargo.toml'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

const CRATE = 'remote/deployments/browser-job-runner-rs';

const STEP_ACTIONS = [
  'goto',
  'click',
  'fill',
  'select',
  'press',
  'waitForSelector',
  'waitForUrl',
  'waitForTimeout',
  'extractText',
  'extractAttribute',
  'screenshot',
  'evaluate',
] as const;

test('browser-job-runner is a Rust axum spawner that launches one nerdctl worker per job', async () => {
  const cargo = await readRepoFile(`${CRATE}/Cargo.toml`);
  const main = await readRepoFile(`${CRATE}/src/main.rs`);
  const readme = await readRepoFile(`${CRATE}/readme.md`);

  // Lean axum/tokio crate that shells out to nerdctl (no bollard, no NATS dep).
  assert.match(cargo, /name = "dd-browser-job-runner"/);
  assert.match(cargo, /axum = /);
  assert.match(cargo, /tokio = .*process/);
  assert.match(cargo, /base64 = /);
  assert.match(cargo, /serde_json = /);

  // Each POST /run spawns one detached, self-removing, host-network worker.
  assert.match(main, /"run", "-d", "--rm"/);
  assert.match(main, /"--network", &config\.network/);
  assert.match(main, /dd\.browser-job\.managed=true/);
  assert.match(main, /dd\.browser-job\.deadline-ms=/);
  assert.match(main, /dd\.browser-job\.job-id=/);
  assert.match(main, /JOB_SPEC_B64=/);
  assert.match(main, /NATS_URL=/);
  assert.match(main, /BROWSER_JOB_RESULT_SUBJECT=/);
  assert.match(main, /--pull=/);

  // Both engines are selectable; the bounded DSL is validated server-side.
  assert.match(main, /const ENGINES: &\[&str\] = &\["playwright", "puppeteer"\]/);
  for (const action of STEP_ACTIONS) {
    assert.match(main, new RegExp(`"${action}"`), `missing step action: ${action}`);
  }

  // 9-minute hard ceiling is clamped in config and used as the container deadline.
  assert.match(main, /BROWSER_JOB_MAX_LIFETIME_SECONDS", 540\)\.clamp\(30, 540\)/);
  assert.match(main, /deadline_ms = started_ms \+ \(state\.config\.max_lifetime_seconds as u128\) \* 1000/);

  // Async model: results go to NATS, the HTTP response is just an accepted ticket.
  assert.match(main, /StatusCode::ACCEPTED/);
  assert.match(main, /"resultSubject": result_subject/);
  assert.match(main, /dd\.remote\.browser_jobs/);

  // Concurrency cap returns 429 rather than oversubscribing the node.
  assert.match(main, /StatusCode::TOO_MANY_REQUESTS/);
  assert.match(main, /jobs\.len\(\) >= state\.config\.max_concurrent/);

  // Constant-time auth on the same headers as the sibling browser services.
  assert.match(main, /constant_time_equals/);
  assert.match(main, /"x-server-auth"/);

  // Routes are mirrored under /browser-jobs/* for the gateway prefix.
  assert.match(main, /\.route\("\/run", post\(handle_run\)\)/);
  assert.match(main, /\.route\("\/browser-jobs\/run", post\(handle_run\)\)/);
  assert.match(main, /"\/healthz"/);
  assert.match(main, /"\/readyz"/);
  assert.match(main, /"\/metrics"/);
  assert.match(main, /"\/status"/);
  assert.match(main, /"\/jobs"/);

  // The in-server tracker enforces the deadline and prunes finished containers.
  assert.match(main, /run_tracker_loop/);
  assert.match(main, /force_remove/);

  assert.match(readme, /dd-browser-job-runner/);
  assert.match(readme, /nerdctl/);
  assert.match(readme, /9 minutes/);
  assert.match(readme, /NATS/);
});

test('browser-job worker runs one Playwright/Puppeteer scenario and publishes to NATS', async () => {
  const pkg = await readRepoFile(`${CRATE}/worker/package.json`);
  const worker = await readRepoFile(`${CRATE}/worker/src/worker.ts`);
  const dockerfile = await readRepoFile(`${CRATE}/worker/Dockerfile`);

  // Worker speaks NATS and both browser engines; it has no HTTP server.
  assert.match(pkg, /"name": "dd-browser-job-worker"/);
  assert.match(pkg, /"nats":/);
  assert.match(pkg, /"playwright":/);
  assert.match(pkg, /"puppeteer":/);
  assert.match(pkg, /"zod":/);
  assert.doesNotMatch(pkg, /"fastify":/);

  // Single-shot: read the job from env, run it, publish JSON to NATS, exit.
  assert.match(worker, /process\.env\.JOB_SPEC_B64/);
  assert.match(worker, /from 'nats'/);
  assert.match(worker, /JSONCodec/);
  assert.match(worker, /config\.resultSubject/);
  assert.match(worker, /config\.resultFanoutSubject/);
  assert.match(worker, /openPlaywrightDriver/);
  assert.match(worker, /openPuppeteerDriver/);
  assert.match(worker, /playwrightChromium\.executablePath\(\)/);
  assert.match(worker, /--no-sandbox/);
  assert.match(worker, /--disable-dev-shm-usage/);

  // Independent watchdog guarantees the process never outlives its budget.
  assert.match(worker, /BROWSER_JOB_MAX_MS/);
  assert.match(worker, /watchdog/);
  assert.match(worker, /process\.exit/);

  // Every DSL action is handled.
  for (const action of STEP_ACTIONS) {
    assert.match(worker, new RegExp(`case '${action}'`), `worker missing step action: ${action}`);
  }

  // Same Playwright Noble base as the other browser services.
  assert.match(dockerfile, /mcr\.microsoft\.com\/playwright:v\$\{PLAYWRIGHT_VERSION\}-noble/);
  assert.match(dockerfile, /CMD \["node", "dist\/worker\.js"\]/);
});

test('browser-job-runner deploys as a privileged host-network spawner through Argo and the gateway', async () => {
  const deployment = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-browser-job-runner.deployment.yaml',
  );
  const service = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-browser-job-runner.service.yaml',
  );
  const kustomization = await readRepoFile('remote/argocd/dd-next-runtime/kustomization.yaml');
  const gateway = await readRepoFile(
    'remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml',
  );
  const promtail = await readRepoFile('remote/argocd/observability/promtail.configmap.yaml');
  const runtimeReadme = await readRepoFile('remote/argocd/dd-next-runtime/readme.md');

  assert.match(deployment, /name:\s*dd-browser-job-runner/);

  // Privileged, host-network nerdctl driver self-building from the mounted repo.
  assert.match(deployment, /privileged:\s*true/);
  assert.match(deployment, /hostNetwork:\s*true/);
  assert.match(deployment, /dnsPolicy:\s*ClusterFirstWithHostNet/);
  assert.match(deployment, /image:\s*docker\.io\/library\/rust:1\.90-bookworm/);
  assert.match(deployment, /cd \/opt\/dd-next-1\/remote\/deployments\/browser-job-runner-rs/);
  assert.match(deployment, /cargo run --release/);

  // Port + NATS + namespace + worker image + the 9-minute ceiling.
  assert.match(deployment, /containerPort:\s*8106/);
  assert.match(deployment, /name:\s*PORT[\s\S]*value:\s*'8106'/);
  assert.match(deployment, /name:\s*NATS_URL[\s\S]*nats:\/\/dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /name:\s*BROWSER_JOB_CONTAINERD_NAMESPACE[\s\S]*value:\s*dd-browser-jobs/);
  assert.match(deployment, /name:\s*BROWSER_JOB_NETWORK[\s\S]*value:\s*host/);
  assert.match(deployment, /name:\s*BROWSER_JOB_IMAGE[\s\S]*dd-browser-job-worker:dev/);
  assert.match(deployment, /name:\s*BROWSER_JOB_PULL_POLICY[\s\S]*value:\s*never/);
  assert.match(deployment, /name:\s*BROWSER_JOB_MAX_LIFETIME_SECONDS[\s\S]*value:\s*'540'/);
  assert.match(deployment, /name:\s*BROWSER_JOB_ALLOW_EVALUATE[\s\S]*value:\s*'false'/);
  assert.match(
    deployment,
    /name:\s*SERVER_AUTH_SECRET[\s\S]*secretKeyRef:[\s\S]*name:\s*dd-agent-secrets[\s\S]*key:\s*SERVER_AUTH_SECRET/,
  );

  // It drives the node's containerd via the socket + nerdctl bind mounts.
  assert.match(deployment, /mountPath:\s*\/run\/containerd\/containerd\.sock/);
  assert.match(deployment, /mountPath:\s*\/usr\/local\/bin\/nerdctl/);

  // The Service exposes only the spawner API; no browser/grid port is published.
  assert.match(service, /name:\s*dd-browser-job-runner/);
  assert.match(service, /port:\s*8106/);
  assert.match(service, /targetPort:\s*http/);

  assert.match(kustomization, /dd-browser-job-runner\.deployment\.yaml/);
  assert.match(kustomization, /dd-browser-job-runner\.service\.yaml/);

  // Gateway routes share the auth shape used by /scrape, /browser-test, /selenium.
  assert.match(
    gateway,
    /location = \/browser-jobs[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-browser-job-runner\.default\.svc\.cluster\.local:8106/,
  );
  assert.match(
    gateway,
    /location \/browser-jobs\/[\s\S]*X-Server-Auth "\$\{DD_REMOTE_DEV_SERVER_AUTH_VALUE\}"[\s\S]*dd-browser-job-runner\.default\.svc\.cluster\.local:8106/,
  );

  // Logs flow through promtail's prod selector and the service is documented.
  assert.match(promtail, /dd-browser-job-runner/);
  assert.match(runtimeReadme, /`dd-browser-job-runner`/);
  assert.match(runtimeReadme, /dd\.remote\.browser_jobs/);
});

test('idle-reaper backstops rogue browser-job containers in the dd-browser-jobs namespace', async () => {
  const reaper = await readRepoFile('remote/deployments/idle-reaper-rs/src/main.rs');
  const configmap = await readRepoFile('remote/argocd/dd-next-runtime/dd-idle-reaper.configmap.yaml');

  // A dedicated age/deadline reap loop wired into the existing reaper deployment.
  assert.match(reaper, /struct BrowserJobReapJob/);
  assert.match(reaper, /fn browser_job_reap_job_from_env/);
  assert.match(reaper, /async fn run_browser_job_reap_loop/);
  assert.match(reaper, /tokio::spawn\(run_browser_job_reap_loop/);
  assert.match(reaper, /BROWSER_JOB_REAP_ENABLED/);
  assert.match(reaper, /dd\.browser-job\.managed=true/);
  assert.match(reaper, /"rm"/);
  assert.match(reaper, /"-f"/);
  // Reaps past deadline + grace (or when the deadline label is missing).
  assert.match(reaper, /deadline_ms \+ grace_ms/);

  assert.match(configmap, /BROWSER_JOB_REAP_ENABLED:\s*'true'/);
  assert.match(configmap, /BROWSER_JOB_REAP_NAMESPACE:\s*'dd-browser-jobs'/);
  assert.match(configmap, /BROWSER_JOB_REAP_DEADLINE_LABEL:\s*'dd\.browser-job\.deadline-ms'/);
});
