import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';

const config = JSON.parse(readFileSync('monorepo.config.json', 'utf8'));
const gitmodules = readFileSync('.gitmodules', 'utf8');

const entries = [...gitmodules.matchAll(
  /\[submodule "([^"]+)"\]\s*\n\s*path = (\S+)\s*\n\s*url = (\S+)\s*\n\s*branch = (\S+)/g,
)].map(([, name, path, url, branch]) => ({ name, path, url, branch }));
const appEntries = entries.filter((entry) => entry.path.startsWith('apps/'));

test('every configured app is a submodule under apps/', () => {
  const paths = appEntries.map((e) => e.path).sort();
  assert.deepEqual(paths, config.apps.map((a) => `apps/${a}`).sort());
});

test('application submodules track branch main over SSH inside the org', () => {
  for (const e of appEntries) {
    assert.equal(e.branch, 'main', `${e.path} branch`);
    assert.match(e.url, new RegExp(`^git@github\\.com:${config.org}/`), `${e.path} url`);
    assert.ok(e.path.startsWith('apps/'), `${e.path} location`);
  }
});

test('flags-2-env is pinned from its canonical public repository', () => {
  const tool = entries.find((entry) => entry.path === 'tools/flags-2-env');
  assert.ok(tool, 'tools/flags-2-env submodule is missing');
  assert.equal(tool.branch, 'main');
  assert.equal(tool.url, 'https://github.com/ORESoftware/flags-2-env.git');
});

test('index gitlinks match .gitmodules exactly', () => {
  const out = execFileSync('git', ['ls-files', '-s'], { encoding: 'utf8' });
  const gitlinks = out.split('\n').filter((l) => l.startsWith('160000')).map((l) => l.split('\t')[1]).sort();
  assert.deepEqual(gitlinks, entries.map((e) => e.path).sort());
  for (const l of out.split('\n')) {
    if (!l) continue;
    const [meta, path] = l.split('\t');
    if (path.startsWith('apps/') && !meta.startsWith('160000')) {
      assert.fail(`stray tracked file under apps/: ${path}`);
    }
  }
});

test('pinned gitlink SHAs are well-formed', () => {
  const out = execFileSync('git', ['ls-files', '-s'], { encoding: 'utf8' });
  for (const l of out.split('\n').filter((x) => x.startsWith('160000'))) {
    assert.match(l.split(/\s+/)[1], /^[0-9a-f]{40}$/);
  }
});
