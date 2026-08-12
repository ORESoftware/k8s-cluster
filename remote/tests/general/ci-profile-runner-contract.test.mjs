import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import test from 'node:test';

const source = fs.readFileSync('remote/deployments/ci-profile-runner-rs/src/main.rs', 'utf8');
const cargo = fs.readFileSync('remote/deployments/ci-profile-runner-rs/Cargo.toml', 'utf8');
const dockerfile = fs.readFileSync(
  'remote/deployments/ci-profile-runner-rs/Dockerfile',
  'utf8',
);
const buildProfiles = fs.readFileSync(
  'remote/deployments/build-server-rs/src/profiles.rs',
  'utf8',
);
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

test('ci profile runner accepts only exact reviewed repository identities', () => {
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
  assert.match(deployment, /ORESoftware\/k8s-cluster\":\"rust-verify/);
  assert.doesNotMatch(deployment, /discrete-event-systems-test\/\*/);
  assert.doesNotMatch(deployment, /ORESoftware\/\*/);
});

test('runner images and commands are compiled fixed profiles, not request fields', () => {
  const requestStruct = source.match(/struct RunRequest \{([^}]*)\}/s)?.[1] ?? '';
  assert.notEqual(requestStruct, '');
  assert.match(source, /mcr\.microsoft\.com\/playwright:v1\.60\.0-noble/);
  assert.match(source, /docker\.io\/library\/rust:1\.90-bookworm/);
  assert.match(source, /npm ci && npx playwright test/);
  assert.match(source, /npm ci && npm run test:puppeteer/);
  assert.match(source, /cargo clippy --locked --all-targets --all-features -- -D warnings/);
  assert.match(source, /cargo test --locked --all-targets --all-features/);
  assert.doesNotMatch(requestStruct, /\bimage:/);
  assert.doesNotMatch(requestStruct, /\bcommand:/);
  assert.doesNotMatch(requestStruct, /\bshell:/);
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
  assert.doesNotMatch(deployment, /mountPath: \/opt\/dd-next-1/);
  assert.doesNotMatch(deployment, /hostNetwork: true/);

  assert.match(buildDeployment, /allowPrivilegeEscalation: false/);
  assert.match(buildDeployment, /capabilities:\n\s+drop:\n\s+- ALL/);
  assert.doesNotMatch(buildDeployment, /privileged: true/);
  assert.doesNotMatch(buildRbac, /resources:\s*\[[^\]]*pods/i);
  assert.doesNotMatch(buildRbac, /resources:\s*\[[^\]]*jobs/i);
});

test('host containerd runtime state is shared with the shim, not socket-only', () => {
  assert.match(
    deployment,
    /- name: containerd-run\n\s+mountPath: \/run\/containerd/,
  );
  assert.match(
    deployment,
    /- name: containerd-run\n\s+hostPath:\n\s+path: \/run\/containerd\n\s+type: Directory/,
  );
  assert.doesNotMatch(
    deployment,
    /mountPath: \/run\/containerd\/containerd\.sock/,
  );
  assert.doesNotMatch(
    deployment,
    /path: \/run\/containerd\/containerd\.sock\n\s+type: Socket/,
  );
});

test('build-server delegates only exact fixed reviewed nerdctl shapes', () => {
  const adapter = extractAdapter();
  const syntax = spawnSync('bash', ['-n'], { input: adapter, encoding: 'utf8' });
  assert.equal(syntax.status, 0, syntax.stderr);
  assert.match(adapter, /PLAYWRIGHT_URL=https:\/\/github\.com\/discrete-event-systems-test\/des-web-playwright-e2e\.git/);
  assert.match(adapter, /PUPPETEER_URL=https:\/\/github\.com\/discrete-event-systems-test\/des-web-puppeteer-e2e\.git/);
  assert.match(adapter, /RUST_URL=https:\/\/github\.com\/ORESoftware\/k8s-cluster\.git/);
  assert.match(adapter, /RUST_IMAGE=docker\.io\/library\/rust:1\.90-bookworm/);
  assert.match(adapter, /cargo clippy --locked --all-targets --all-features -- -D warnings/);
  assert.match(adapter, /cargo test --locked --all-targets --all-features/);
  assert.match(adapter, /\^\[0-9a-f\]\{40\}\$/);
  assert.match(adapter, /\/var\/lib\/dd-build-server\/jobs\/\*\/repo/);
  assert.match(adapter, /--security-opt=no-new-privileges/);
  assert.match(adapter, /--cap-drop=ALL/);
  assert.match(adapter, /fallback "\$@"/);
  assert.match(adapter, /\/dev\/tcp\/\$\{RUNNER_HOST\}\/\$\{RUNNER_PORT\}/);
  assert.doesNotMatch(adapter, /curl .*SERVER_AUTH_SECRET|wget .*SERVER_AUTH_SECRET/);

  assert.match(buildPatch, /BUILD_SERVER_NERDCTL_BIN/);
  assert.match(buildPatch, /\/etc\/dd-build-server\/nerdctl-profile-adapter\.sh/);
  assert.match(buildPatch, /defaultMode: 0555/);
});

test('profile adapter auth is a bounded read-only projected secret', () => {
  const adapter = extractAdapter();
  assert.match(
    adapter,
    /AUTH_FILE=\/var\/run\/secrets\/dd-ci-profile-runner-auth\/SERVER_AUTH_SECRET/,
  );
  assert.match(adapter, /\[\[ -r "\$AUTH_FILE" \]\]/);
  assert.ok(adapter.includes('auth="$(<"$AUTH_FILE")"'));
  assert.ok(
    adapter.includes(
      "if [[ -z \"$auth\" || ${#auth} -gt 4096 || \"$auth\" == *$'\\n'* || \"$auth\" == *$'\\r'* ]]; then",
    ),
  );
  assert.ok(!adapter.includes('${#auth} -ge 20'));
  assert.doesNotMatch(adapter, /\$\{SERVER_AUTH_SECRET/);
  assert.match(source, /env_string\("SERVER_AUTH_SECRET"\)/);

  assert.match(
    buildPatch,
    /volumeMounts:\n\s+- name: profile-runner-auth\n\s+mountPath: \/var\/run\/secrets\/dd-ci-profile-runner-auth\n\s+readOnly: true/,
  );
  assert.match(
    buildPatch,
    /- name: profile-runner-auth\n\s+secret:\n\s+secretName: dd-agent-secrets\n\s+defaultMode: 0400\n\s+items:\n\s+- key: SERVER_AUTH_SECRET\n\s+path: SERVER_AUTH_SECRET/,
  );
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

test('runner service uses a digest-pinned minimal image without runtime compilation', () => {
  assert.match(
    deployment,
    /image: ghcr\.io\/oresoftware\/ci-profile-runner@sha256:[0-9a-f]{64}/,
  );
  assert.doesNotMatch(deployment, /cargo run|source_root=|\/opt\/dd-next-1/);
  assert.match(dockerfile, /FROM docker\.io\/library\/rust:1\.90-alpine AS builder/);
  assert.match(dockerfile, /FROM docker\.io\/library\/alpine:3\.22/);
  assert.match(dockerfile, /cargo build --locked --release/);
  assert.match(dockerfile, /ENTRYPOINT \[\"\/usr\/local\/bin\/dd-ci-profile-runner\"\]/);
  assert.match(cargo, /name = "dd-ci-profile-runner"/);
});

test('rust-verify script is identical at all three fixed-profile boundaries', () => {
  const profileScript = buildProfiles.match(
    /const RUST_VERIFY_STEPS:.*?script: r#"(.*?)"#,/s,
  )?.[1];
  const runnerScript = source.match(
    /const RUST_VERIFY_SCRIPT: &str = r#"(.*?)"#;/s,
  )?.[1];
  const adapterScript = extractAdapter().match(
    /readonly RUST_SCRIPT='(.*?)'\nreadonly AUTH_FILE/s,
  )?.[1];
  assert.ok(profileScript);
  assert.equal(runnerScript, profileScript);
  assert.equal(adapterScript, profileScript);
});
