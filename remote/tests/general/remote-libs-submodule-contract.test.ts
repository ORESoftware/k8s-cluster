import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';

const LIBS_PATH = 'remote/libs';
const LIBS_URL = 'git@github.com:ORESoftware/k8s-libs-and-shared-defs.git';

function findRepoRoot(): string {
  for (const candidate of [
    process.cwd(),
    resolve(process.cwd(), '..'),
    resolve(process.cwd(), '..', '..'),
  ]) {
    if (existsSync(resolve(candidate, '.gitmodules'))
      && existsSync(resolve(candidate, LIBS_PATH, 'nats/subject-defs/schema/index.json'))) {
      return candidate;
    }
  }

  throw new Error(`Unable to locate an initialized k8s-cluster checkout from ${process.cwd()}`);
}

function runGit(repoRoot: string, args: string[]): string {
  return execFileSync('git', args, { cwd: repoRoot, encoding: 'utf8' }).trim();
}

function gitmoduleValue(repoRoot: string, key: string): string {
  return runGit(repoRoot, ['config', '-f', '.gitmodules', '--get', `submodule.remote/libs.${key}`]);
}

function trackedFiles(repoRoot: string, pathspec: string): string[] {
  return runGit(repoRoot, ['ls-files', '--', pathspec]).split('\n').filter(Boolean);
}

function dependencyPath(contents: string, dependency: string): string | undefined {
  const escaped = dependency.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return contents.match(new RegExp(`^\\s*${escaped}\\s*=\\s*\\{[^}]*\\bpath\\s*=\\s*"([^"]+)"`, 'm'))?.[1];
}

test('remote/libs is the canonical main-branch git submodule', () => {
  const repoRoot = findRepoRoot();
  const gitmodules = readFileSync(resolve(repoRoot, '.gitmodules'), 'utf8');

  assert.equal(gitmoduleValue(repoRoot, 'path'), LIBS_PATH);
  assert.equal(gitmoduleValue(repoRoot, 'url'), LIBS_URL);
  assert.equal(gitmoduleValue(repoRoot, 'branch'), 'main');
  assert.doesNotMatch(
    gitmodules,
    /^\[submodule "remote\/libs\/async-java"\]$/m,
    'async-java belongs to the shared-definitions repository, not the cluster superproject.',
  );

  const gitlink = runGit(repoRoot, ['ls-files', '--stage', '--', LIBS_PATH]);
  const match = gitlink.match(/^160000 ([0-9a-f]{40}) 0\tremote\/libs$/);
  assert.ok(match, 'remote/libs must be stored as one gitlink, not copied files.');
  assert.equal(runGit(repoRoot, ['-C', LIBS_PATH, 'rev-parse', 'HEAD']), match[1]);
});

test('remote/libs and its nested dependency are initialized at their pinned commits', () => {
  const repoRoot = findRepoRoot();
  const topLevelGitlink = runGit(repoRoot, ['ls-files', '--stage', '--', LIBS_PATH]);
  const topLevelMatch = topLevelGitlink.match(/^160000 ([0-9a-f]{40}) 0\tremote\/libs$/);
  assert.ok(topLevelMatch, 'remote/libs must resolve to one mode-160000 gitlink.');
  assert.equal(
    runGit(repoRoot, ['-C', LIBS_PATH, 'rev-parse', 'HEAD']),
    topLevelMatch[1],
    'remote/libs checkout must match the superproject gitlink.',
  );

  const nestedGitlinks = runGit(repoRoot, ['-C', LIBS_PATH, 'ls-files', '--stage'])
    .split('\n')
    .flatMap((line) => {
      const match = line.match(/^160000 ([0-9a-f]{40}) 0\t(.+)$/);
      return match ? [{ sha: match[1], path: match[2] }] : [];
    });
  assert.deepEqual(
    nestedGitlinks.map(({ path }) => path),
    ['async-java'],
    'remote/libs should contain exactly its pinned async-java submodule.',
  );
  assert.equal(
    runGit(repoRoot, ['-C', `${LIBS_PATH}/async-java`, 'rev-parse', 'HEAD']),
    nestedGitlinks[0].sha,
    'remote/libs/async-java checkout must match the nested gitlink.',
  );
});

test('the pinned repository exposes the shared contract surface used by the cluster', () => {
  const repoRoot = findRepoRoot();
  const required = [
    'nats/subject-defs/schema/index.json',
    'nats/subject-defs/src/generate.mjs',
    'nats/subject-defs/generated/rust/Cargo.toml',
    'nats/subject-defs/generated/gleam/gleam.toml',
    'nats/subject-defs/generated/javascript/index.mjs',
    'nats/subject-defs/generated/python/dd_nats_subject_defs.py',
    'pg-defs/schema/schema.sql',
    'pg-defs/generated/rust/Cargo.toml',
    'interfaces/redis/schema/index.json',
    'interfaces/shared/schema/index.json',
    'runtime-config-client-gleam/gleam.toml',
    'runtime-config-client-rs/Cargo.toml',
  ];

  const missing = required.filter((path) => !existsSync(resolve(repoRoot, LIBS_PATH, path)));
  assert.deepEqual(missing, [], `Pinned remote/libs is missing shared contract files: ${missing.join(', ')}`);
});

test('tracked Rust and Gleam consumers resolve to the canonical generated packages', () => {
  const repoRoot = findRepoRoot();
  const consumerGroups = [
    {
      dependency: 'dd-nats-subject-defs',
      files: trackedFiles(repoRoot, 'remote/deployments/**/Cargo.toml'),
      expected: resolve(repoRoot, LIBS_PATH, 'nats/subject-defs/generated/rust'),
      minimum: 30,
    },
    {
      dependency: 'dd_nats_subject_defs',
      files: trackedFiles(repoRoot, 'remote/deployments/**/gleam.toml'),
      expected: resolve(repoRoot, LIBS_PATH, 'nats/subject-defs/generated/gleam'),
      minimum: 5,
    },
  ];

  for (const group of consumerGroups) {
    const consumers = group.files.flatMap((file) => {
      const path = dependencyPath(readFileSync(resolve(repoRoot, file), 'utf8'), group.dependency);
      return path ? [{ file, path }] : [];
    });

    assert.ok(
      consumers.length >= group.minimum,
      `Expected at least ${group.minimum} tracked ${group.dependency} consumers, found ${consumers.length}.`,
    );
    for (const consumer of consumers) {
      assert.equal(
        resolve(dirname(resolve(repoRoot, consumer.file)), consumer.path),
        group.expected,
        `${consumer.file} must resolve ${group.dependency} from ${LIBS_PATH}.`,
      );
    }
  }
});

test('CI and repository documentation preserve recursive pinned checkout semantics', () => {
  const repoRoot = findRepoRoot();
  const repoChecks = readFileSync(resolve(repoRoot, '.github/workflows/repo-checks.yml'), 'utf8');
  const checkoutAction = readFileSync(
    resolve(repoRoot, '.github/actions/checkout-remote-libs/action.yml'),
    'utf8',
  );
  const helper = readFileSync(resolve(repoRoot, 'scripts/ci/init-submodules-with-report.sh'), 'utf8');
  const docs = readFileSync(resolve(repoRoot, 'docs/remote-libs-submodule.md'), 'utf8');
  const submodules = readFileSync(resolve(repoRoot, 'SUBMODULES.md'), 'utf8');

  const staticJob = repoChecks.slice(
    repoChecks.indexOf('  static-contracts:'),
    repoChecks.indexOf('  backend-contracts:'),
  );
  assert.match(staticJob, /K8S_LIBS_DEPLOY_KEY:\s*\$\{\{ secrets\.K8S_LIBS_DEPLOY_KEY \}\}/);
  assert.match(staticJob, /uses: \.\/\.github\/actions\/checkout-remote-libs/);
  assert.match(staticJob, /ssh-key:\s*\$\{\{ secrets\.K8S_LIBS_DEPLOY_KEY \}\}/);
  assert.doesNotMatch(staticJob, /SUBMODULE_AUTH_MODE/);
  assert.doesNotMatch(staticJob, /init-submodules-with-report\.sh remote\/libs/);
  assert.doesNotMatch(staticJob, /REMOTE_DEV_GH_PAT/);

  assert.match(checkoutAction, /git ls-files --stage -- remote\/libs/);
  assert.match(checkoutAction, /ref: \$\{\{ steps\.pin\.outputs\.sha \}\}/);
  assert.match(checkoutAction, /path: remote\/libs/);
  assert.match(checkoutAction, /ssh-key: \$\{\{ inputs\.ssh-key \}\}/);
  assert.match(checkoutAction, /persist-credentials: false/);
  assert.match(checkoutAction, /submodules: recursive/);
  assert.match(checkoutAction, /git -C remote\/libs rev-parse HEAD/);
  assert.match(checkoutAction, /git -C remote\/libs ls-files --stage -- async-java/);
  assert.match(checkoutAction, /git -C remote\/libs\/async-java rev-parse HEAD/);

  assert.match(
    helper,
    /git "\$\{path_git_config\[@\]\}" submodule update --init --recursive --depth 1 -- "\$path"/,
  );
  assert.match(helper, /git ls-files --stage -- "\$path"/);
  assert.match(helper, /git -C "\$path" rev-parse HEAD/);
  assert.match(helper, /pinned-commit-mismatch/);

  assert.match(docs, /K8S_LIBS_DEPLOY_KEY/);
  assert.match(docs, /init-submodules-with-report\.sh\s+remote\/libs/);
  assert.match(docs, /git submodule update --init --recursive remote\/libs/);
  assert.doesNotMatch(docs, /repo-checks\.yml` uses the[\s\S]{0,80}REMOTE_DEV_GH_PAT/);
  assert.match(
    submodules,
    /\| `remote\/libs` \| \[ORESoftware\/k8s-libs-and-shared-defs\]\(https:\/\/github\.com\/ORESoftware\/k8s-libs-and-shared-defs\) \| `main` \|/,
  );
});
