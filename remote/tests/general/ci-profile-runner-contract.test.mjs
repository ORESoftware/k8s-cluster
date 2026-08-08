import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import test from 'node:test';

const source = fs.readFileSync('remote/deployments/ci-profile-runner-rs/src/main.rs', 'utf8');
const cargo = fs.readFileSync('remote/deployments/ci-profile-runner-rs/Cargo.toml', 'utf8');
const deployment = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-ci-profile-runner.deployment.yaml',
  'utf8',
);
const service = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-ci-profile-runner.service.yaml',
  'utf8',
);
const runnerPolicy = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-ci-profile-runner.networkpolicy.yaml',
  'utf8',
);
const buildDeployment = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml',
  'utf8',
);
const buildPolicy = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-build-server.networkpolicy.yaml',
  'utf8',
);
const buildPatch = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
  'utf8',
);
const buildConfig = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-build-server.configmap.yaml',
  'utf8',
);
const buildRbac = fs.readFileSync(
  'remote/argocd/dd-next-runtime/dd-build-server-rbac.yaml',
  'utf8',
);
const kustomization = fs.readFileSync(
  'remote/argocd/dd-next-runtime/kustomization.yaml',
  'utf8',
);

function extractAdapter() {
  const marker = '  nerdctl-profile-adapter.sh: |\n';
  const start = buildConfig.indexOf(marker);
  assert.notEqual(start, -1, 'adapter must be present in build-server rules ConfigMap');
  const lines = buildConfig.slice(start + marker.length).split('\n');
  const body = [];
  for (const line of lines) {
    if (line.length > 0 && !line.startsWith('    ')) break;
    body.push(line.startsWith('    ') ? line.slice(4) : line);
  }
  return `${body.join('\n')}\n`;
}

test('ci profile runner accepts only exact DES browser identities', () => {
  assert.match(source, /const SCHEMA: &str = "ci-profile-runner\.v1"/);
  assert.match(source, /value\.len\(\) == 40/);
  assert.match(source, /--no-recurse-submodules/);
  assert.match(source, /protocol\.ext\.allow=never/);
  assert.match(source, /protocol\.file\.allow=never/);
  assert.match(source, /protocol\.local\.allow=never/);
  assert.match(source, /checked out revision does not match requested immutable commit/);
  assert.match(
    deployment,
    /discrete-event-systems-test\/des-web-playwright-e2e\":\"playwright/,
  );
  assert.match(
    deployment,
    /discrete-event-systems-test\/des-web-puppeteer-e2e\":\"puppeteer/,
  );
  assert.doesNotMatch(deployment, /discrete-event-systems-test\/\*|discrete-event-systems-test\/\"/);
});

test('runner images and commands are compiled fixed profiles, not request fields', () => {
  assert.match(source, /mcr\.microsoft\.com\/playwright:v1\.60\.0-noble/);
  assert.match(source, /npm ci && npx playwright test/);
  assert.match(source, /npm ci && npm run test:puppeteer/);
  assert.doesNotMatch(source, /struct RunRequest[\s\S]*\bimage:/);
  assert.doesNotMatch(source, /struct RunRequest[\s\S]*\bcommand:/);
  assert.doesNotMatch(source, /struct RunRequest[\s\S]*\bshell:/);
  assert.match(source, /--security-opt=no-new-privileges/);
  assert.match(source, /--cap-drop=ALL/);
  assert.match(source, /--cpus=/);
  assert.match(source, /--memory=/);
  assert.match(source, /--pids-limit=/);
});

test('privilege is isolated to the dedicated host-containerd runner', () => {
  assert.match(deployment, /name: dd-ci-profile-runner/);
  assert.match(deployment, /privileged: true/);
  assert.match(deployment, /automountServiceAccountToken: false/);
  assert.match(deployment, /mountPath: \/var\/lib\/containerd/);
  assert.match(deployment, /mountPropagation: Bidirectional/);
  assert.match(deployment, /mountPath: \/opt\/dd-next-1\n\s+readOnly: true/);
  assert.doesNotMatch(deployment, /hostNetwork: true/);

  assert.match(buildDeployment, /allowPrivilegeEscalation: false/);
  assert.match(buildDeployment, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.doesNotMatch(buildDeployment, /privileged: true/);
  assert.doesNotMatch(buildRbac, /resources:\s*\[[^\]]*pods/i);
  assert.doesNotMatch(buildRbac, /resources:\s*\[[^\]]*jobs/i);
});

test('build-server delegates only the exact fixed DES nerdctl shape', () => {
  const adapter = extractAdapter();
  const syntax = spawnSync('bash', ['-n'], { input: adapter, encoding: 'utf8' });
  assert.equal(syntax.status, 0, syntax.stderr);
  assert.match(adapter, /PLAYWRIGHT_URL=https:\/\/github\.com\/discrete-event-systems-test\/des-web-playwright-e2e\.git/);
  assert.match(adapter, /PUPPETEER_URL=https:\/\/github\.com\/discrete-event-systems-test\/des-web-puppeteer-e2e\.git/);
  assert.match(adapter, /\^\[0-9a-f\]\{40\}\$/);
  assert.match(adapter, /\/var\/lib\/dd-build-server\/jobs\/\*\/repo/);
  assert.match(adapter, /--security-opt=no-new-privileges/);
  assert.match(adapter, /--cap-drop=ALL/);
  assert.match(adapter, /fallback \"\$@\"/);
  assert.match(adapter, /\/dev\/tcp\/\$\{RUNNER_HOST\}\/\$\{RUNNER_PORT\}/);
  assert.doesNotMatch(adapter, /curl .*SERVER_AUTH_SECRET|wget .*SERVER_AUTH_SECRET/);

  assert.match(buildPatch, /BUILD_SERVER_NERDCTL_BIN/);
  assert.match(buildPatch, /\/etc\/dd-build-server\/nerdctl-profile-adapter\.sh/);
  assert.match(buildPatch, /defaultMode: 0555/);
});

test('network and kustomize wiring is narrow and complete', () => {
  assert.match(service, /name: dd-ci-profile-runner/);
  assert.match(service, /port: 8147/);
  assert.match(runnerPolicy, /app: dd-build-server/);
  assert.match(runnerPolicy, /port: 8147/);
  assert.match(buildPolicy, /app: dd-ci-profile-runner/);
  assert.match(buildPolicy, /port: 8147/);
  assert.match(kustomization, /dd-ci-profile-runner\.deployment\.yaml/);
  assert.match(kustomization, /dd-ci-profile-runner\.service\.yaml/);
  assert.match(kustomization, /dd-ci-profile-runner\.networkpolicy\.yaml/);
});

test('runner builds from pod-local source scratch rather than mutating host checkout', () => {
  assert.match(deployment, /source_root=\/tmp\/dd-ci-profile-runner-source/);
  assert.match(deployment, /cp -a \/opt\/dd-next-1\/remote\/deployments\/ci-profile-runner-rs/);
  assert.match(deployment, /cp -a \/opt\/dd-next-1\/remote\/libs/);
  assert.match(deployment, /CARGO_TARGET_DIR=\/tmp\/dd-ci-profile-runner-target/);
  assert.match(cargo, /name = "dd-ci-profile-runner"/);
});
