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

test('browser e2e uses the narrow exact-gitlink remote/libs checkout', () => {
  assert.match(workflow, /uses: \.\/\.github\/actions\/checkout-remote-libs/);
  assert.match(
    workflow,
    /ssh-key:\s*\$\{\{\s*secrets\.K8S_LIBS_DEPLOY_KEY\s*\}\}/,
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
