import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workflow = await readFile(
  new URL("../../../.github/workflows/pg-defs-check.yml", import.meta.url),
  "utf8",
);
const action = await readFile(
  new URL("../../../.github/actions/checkout-remote-libs/action.yml", import.meta.url),
  "utf8",
);
const repoChecks = await readFile(
  new URL("../../../.github/workflows/repo-checks.yml", import.meta.url),
  "utf8",
);
const seaOrm = await readFile(
  new URL("../../../.github/workflows/contract-service-seaorm-branch.yml", import.meta.url),
  "utf8",
);

test("all pg-defs jobs use the exact-gitlink private library action", () => {
  const uses = workflow.match(/uses: \.\/\.github\/actions\/checkout-remote-libs/g) ?? [];
  assert.equal(uses.length, 3);
  assert.doesNotMatch(workflow, /git submodule update[^\n]*remote\/libs/);
  assert.match(workflow, /'\.github\/actions\/checkout-remote-libs\/action\.yml'/);
  assert.match(workflow, /'remote\/tests\/general\/pg-defs-private-libs-checkout\.test\.mjs'/);
});

test("the reusable action resolves and verifies the superproject gitlink", () => {
  assert.match(action, /git ls-files --stage -- remote\/libs/);
  assert.match(action, /\$1 == 160000/);
  assert.match(action, /repository: ORESoftware\/k8s-libs-and-shared-defs/);
  assert.match(action, /ref: \$\{\{ steps\.pin\.outputs\.sha \}\}/);
  assert.match(action, /path: remote\/libs/);
  assert.match(action, /ssh-key: \$\{\{ inputs\.ssh-key \}\}/);
  assert.match(action, /persist-credentials: false/);
  assert.match(action, /git -C remote\/libs rev-parse HEAD/);
  assert.doesNotMatch(action, /submodules:\s*(true|recursive)/);
  assert.doesNotMatch(action, /https:\/\/[^\s]+@github\.com/);
});

test("other shared-definition consumers use the same immutable checkout boundary", () => {
  const repoCheckUses =
    repoChecks.match(/uses: \.\/\.github\/actions\/checkout-remote-libs/g) ?? [];
  const seaOrmUses =
    seaOrm.match(/uses: \.\/\.github\/actions\/checkout-remote-libs/g) ?? [];
  assert.equal(repoCheckUses.length, 2);
  assert.equal(seaOrmUses.length, 1);
  assert.doesNotMatch(repoChecks, /init-submodules-with-report\.sh remote\/libs/);
  assert.doesNotMatch(seaOrm, /git -C remote\/libs (?:fetch|checkout)/);
  assert.match(
    seaOrm,
    /EXPECTED_SHARED_COMMIT: \$\{\{ steps\.shared\.outputs\.sha \}\}/,
  );
  assert.doesNotMatch(seaOrm, /cargo fmt --all/);
  assert.match(seaOrm, /cargo fmt -- --check/);
  assert.match(
    seaOrm,
    /--manifest-path \.\.\/\.\.\/libs\/pg-defs\/generated\/rust\/sea-orm\/Cargo\.toml/,
  );
});
