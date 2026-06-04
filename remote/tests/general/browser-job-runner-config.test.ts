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

test('browser-job-runner is a pool-first Rust axum orchestrator with a nerdctl fallback', async () => {
  const cargo = await readRepoFile(`${CRATE}/Cargo.toml`);
  const main = await readRepoFile(`${CRATE}/src/main.rs`);
  const readme = await readRepoFile(`${CRATE}/readme.md`);

  // axum/tokio crate that now also speaks NATS (request/reply to the pool +
  // republishing results) and still shells out to nerdctl for the fallback.
  assert.match(cargo, /name = "dd-browser-job-runner"/);
  assert.match(cargo, /axum = /);
  assert.match(cargo, /tokio = .*process/);
  assert.match(cargo, /base64 = /);
  assert.match(cargo, /serde_json = /);
  assert.match(cargo, /async-nats = "=0\.38\.0"/);

  // Primary path: NATS request/reply to the container-pool subject, then
  // republish the worker's RunResult to the per-job subject + fanout.
  assert.match(main, /fn dispatch_via_pool/);
  assert.match(main, /send_request/);
  assert.match(main, /fn connect_nats_loop/);
  assert.match(main, /fn publish_run_result/);
  assert.match(main, /fn process_job/);
  assert.match(main, /BROWSER_JOB_POOL_ENABLED/);
  assert.match(main, /BROWSER_JOB_POOL_SUBJECT/);
  assert.match(main, /dd\.remote\.container_pool\.browser-jobs\.requests/);
  // A DispatchResponse "body" is the result; a 409 means we raced a used worker
  // and must fall back rather than publish a bogus result.
  assert.match(main, /value\.get\("body"\)/);
  assert.match(main, /409/);

  // Fallback path: spawn one detached, self-removing, host-network worker.
  assert.match(main, /fn fallback_spawn/);
  assert.match(main, /"run", "-d", "--rm"/);
  assert.match(main, /"--network", &config\.network/);
  assert.match(main, /dd\.browser-job\.managed=true/);
  assert.match(main, /dd\.browser-job\.deadline-ms=/);
  assert.match(main, /JOB_SPEC_B64=/);
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

  // Async model: POST /run is just an accepted ticket; results go to NATS.
  assert.match(main, /StatusCode::ACCEPTED/);
  assert.match(main, /tokio::spawn\(process_job/);
  assert.match(main, /"resultSubject": result_subject/);
  assert.match(main, /"poolSubject": state\.config\.pool_subject/);
  assert.match(main, /dd\.remote\.browser_jobs/);

  // The fallback path owns the concurrency cap (not the synchronous handler).
  assert.match(main, /jobs\.len\(\) >= state\.config\.max_concurrent/);
  assert.match(main, /rejected_total/);

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

  // The in-server tracker enforces the fallback deadline and prunes containers.
  assert.match(main, /run_tracker_loop/);
  assert.match(main, /force_remove/);

  // Pool metrics so the path split is observable.
  assert.match(main, /browser_job_pool_dispatched_total/);
  assert.match(main, /browser_job_fallback_total/);

  assert.match(readme, /dd-browser-job-runner/);
  assert.match(readme, /dd-container-pool/);
  assert.match(readme, /nerdctl/);
  assert.match(readme, /9 minutes/);
  assert.match(readme, /NATS/);
});

test('browser-job worker is dual-mode: HTTP serve for the pool + one-shot for the fallback', async () => {
  const pkg = await readRepoFile(`${CRATE}/worker/package.json`);
  const worker = await readRepoFile(`${CRATE}/worker/src/worker.ts`);
  const dockerfile = await readRepoFile(`${CRATE}/worker/Dockerfile`);

  // Worker speaks NATS and both browser engines; HTTP server is node:http, not fastify.
  assert.match(pkg, /"name": "dd-browser-job-worker"/);
  assert.match(pkg, /"nats":/);
  assert.match(pkg, /"playwright":/);
  assert.match(pkg, /"puppeteer":/);
  assert.match(pkg, /"zod":/);
  assert.doesNotMatch(pkg, /"fastify":/);

  // Mode is chosen at startup by whether JOB_SPEC_B64 is set.
  assert.match(worker, /process\.env\.JOB_SPEC_B64/);
  assert.match(worker, /async function runOneShot/);
  assert.match(worker, /async function runServe/);

  // serve mode: a tiny node:http server with /healthz + /run that exits after one job.
  assert.match(worker, /from 'node:http'/);
  assert.match(worker, /createServer/);
  assert.match(worker, /'\/healthz'/);
  assert.match(worker, /'\/run'/);
  assert.match(worker, /already consumed/);
  assert.match(worker, /scheduleExit/);
  assert.match(worker, /closeAllConnections/);

  // one-shot mode: read the job from env, run it, publish JSON to NATS, exit.
  assert.match(worker, /from 'nats'/);
  assert.match(worker, /JSONCodec/);
  assert.match(worker, /config\.resultSubject/);
  assert.match(worker, /config\.resultFanoutSubject/);
  assert.match(worker, /openPlaywrightDriver/);
  assert.match(worker, /openPuppeteerDriver/);
  assert.match(worker, /playwrightChromium\.executablePath\(\)/);
  assert.match(worker, /--no-sandbox/);
  assert.match(worker, /--disable-dev-shm-usage/);

  // Independent watchdog guarantees a running job never outlives its budget.
  assert.match(worker, /BROWSER_JOB_MAX_MS/);
  assert.match(worker, /armWatchdog/);
  assert.match(worker, /process\.exit/);

  // Every DSL action is handled.
  for (const action of STEP_ACTIONS) {
    assert.match(worker, new RegExp(`case '${action}'`), `worker missing step action: ${action}`);
  }

  // Same Playwright Noble base as the other browser services.
  assert.match(dockerfile, /mcr\.microsoft\.com\/playwright:v\$\{PLAYWRIGHT_VERSION\}-noble/);
  assert.match(dockerfile, /CMD \["node", "dist\/worker\.js"\]/);
});

test('container-pool seed defines the browser-jobs warm pool + base image', async () => {
  const seed = await readRepoFile('remote/databases/pg/seeds/container-pool-app-config.sql');

  // baseImages entry so the pool's image build/sync path knows the worker image.
  assert.match(seed, /"runtime": "browser-jobs"/);
  assert.match(seed, /remote\/deployments\/browser-job-runner-rs\/worker\/Dockerfile/);
  assert.match(seed, /"buildContext": "remote\/deployments\/browser-job-runner-rs\/worker"/);

  // The pool itself: HTTP serve worker, /run + /healthz, the dispatch subject,
  // one job per container, and a 9-minute request timeout.
  assert.match(seed, /"slug": "browser-jobs"/);
  assert.match(seed, /"image": "docker\.io\/library\/dd-browser-job-worker:dev"/);
  assert.match(seed, /"requestPath": "\/run"/);
  assert.match(seed, /"healthPath": "\/healthz"/);
  assert.match(seed, /"natsSubject": "dd\.remote\.container_pool\.browser-jobs\.requests"/);
  assert.match(seed, /"maxConcurrencyPerContainer": 1/);
  assert.match(seed, /"requestTimeoutMs": 540000/);
});

test('browser-job-runner deploys as a privileged host-network orchestrator through Argo and the gateway', async () => {
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

  // Port + NATS + pool wiring + namespace + worker image + the 9-minute ceiling.
  assert.match(deployment, /containerPort:\s*8106/);
  assert.match(deployment, /name:\s*PORT[\s\S]*value:\s*'8106'/);
  assert.match(deployment, /name:\s*NATS_URL[\s\S]*nats:\/\/dd-nats\.messaging\.svc\.cluster\.local:4222/);
  assert.match(deployment, /name:\s*BROWSER_JOB_POOL_ENABLED[\s\S]*value:\s*'true'/);
  assert.match(deployment, /name:\s*BROWSER_JOB_POOL_SUBJECT[\s\S]*dd\.remote\.container_pool\.browser-jobs\.requests/);
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

  // The Service exposes only the orchestrator API; no browser/grid port is published.
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
  assert.match(runtimeReadme, /dd-container-pool/);
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
