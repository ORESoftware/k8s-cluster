import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const workflow = await readFile(
  new URL('../../../.github/workflows/browser-e2e.yml', import.meta.url),
  'utf8',
);
const action = await readFile(
  new URL('../../../.github/actions/checkout-remote-libs/action.yml', import.meta.url),
  'utf8',
);

test('public browser E2E is credential-free and always runs the hermetic suite', () => {
  assert.match(workflow, /public-browser:[\s\S]*?browser\/harness-hardening\.test\.mjs browser\/func-approx-ui\.test\.mjs/);
  assert.match(workflow, /BROWSER_ARTIFACT_DIR:/);
  assert.match(workflow, /actions\/upload-artifact@[0-9a-f]{40}/);

  const publicJob = workflow.match(/  public-browser:[\s\S]*?\n  private-service-worker:/)?.[0] ?? '';
  assert.ok(publicJob, 'expected a public-browser job before the private job');
  assert.doesNotMatch(publicJob, /K8S_LIBS_DEPLOY_KEY|checkout-remote-libs/);
});

test('private service-worker E2E uses the narrow exact-gitlink checkout only when the key exists', () => {
  assert.match(workflow, /uses: \.\/\.github\/actions\/checkout-remote-libs/);
  assert.match(
    workflow,
    /if:\s*steps\.private_libs\.outputs\.available == 'true'[\s\S]*?ssh-key:\s*\$\{\{\s*secrets\.K8S_LIBS_DEPLOY_KEY\s*\}\}/,
  );
  assert.match(
    workflow,
    /K8S_LIBS_DEPLOY_KEY:\s*\$\{\{\s*secrets\.K8S_LIBS_DEPLOY_KEY\s*\}\}[\s\S]*?available=false/,
    'the workflow must explicitly record when private coverage cannot run',
  );
  assert.match(
    workflow,
    /uses:\s*actions\/checkout@[0-9a-f]{40}[\s\S]*?persist-credentials:\s*false/,
    'the primary checkout must not persist credentials before the private-library action',
  );
  assert.match(
    workflow,
    /remote\/tests\/general\/browser-e2e-private-libs-checkout\.test\.mjs/,
    'the workflow must execute its private-library policy guard before browser setup',
  );
  assert.match(
    workflow,
    /'\.github\/actions\/checkout-remote-libs\/action\.yml'/,
    'changes to the reusable checkout action must trigger browser e2e',
  );
  assert.doesNotMatch(workflow, /REMOTE_DEV_GH_PAT|x-access-token:/);
  assert.doesNotMatch(workflow, /git\s+submodule\s+update[^\n]*remote\/libs/);
  assert.doesNotMatch(workflow, /url\."https:\/\/github\.com\/"\.insteadOf/);
});

test('the reusable action resolves and verifies the immutable superproject gitlink', () => {
  assert.match(action, /git ls-files --stage -- remote\/libs/);
  assert.match(action, /\$1 == 160000/);
  assert.match(action, /repository: ORESoftware\/k8s-libs-and-shared-defs/);
  assert.match(action, /ref: \$\{\{ steps\.pin\.outputs\.sha \}\}/);
  assert.match(action, /path: remote\/libs/);
  assert.match(action, /ssh-key: \$\{\{ inputs\.ssh-key \}\}/);
  assert.match(action, /persist-credentials: false/);
  assert.match(action, /git -C remote\/libs rev-parse HEAD/);
  assert.doesNotMatch(action, /submodules:\s*(?:true|recursive)/);
  assert.doesNotMatch(action, /https:\/\/[^\s]+@github\.com/);
});
