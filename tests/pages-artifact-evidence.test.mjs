import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import {
  mkdir,
  mkdtemp,
  readFile,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { promisify } from 'node:util';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  inventoryPagesTree,
  writePagesEvidence,
} from '../scripts/pages-artifact-evidence.mjs';

const execFileAsync = promisify(execFile);

async function fixtureRoot(t) {
  const temporary = await mkdtemp(path.join(os.tmpdir(), 'sonus-pages-'));
  t.after(async () => {
    const { rm } = await import('node:fs/promises');
    await rm(temporary, { recursive: true, force: true });
  });
  const root = path.join(temporary, 'dist');
  await mkdir(path.join(root, '.well-known'), { recursive: true });
  await writeFile(path.join(root, 'index.html'), '<h1>Sonus Auris</h1>\n');
  await writeFile(
    path.join(root, 'deployment.json'),
    '{"schemaVersion":1,"repository":"sonus-auris/sonus-auris-site.web"}\n',
  );
  await writeFile(
    path.join(root, '.well-known', 'security.txt'),
    'Contact: mailto:security@example.test\n',
  );
  return { root, temporary };
}

test('inventory is deterministic and includes dot-directories', async (t) => {
  const { root } = await fixtureRoot(t);
  const first = await inventoryPagesTree(root);
  const second = await inventoryPagesTree(root);

  assert.deepEqual(first, second);
  assert.deepEqual(
    first.entries.map((entry) => entry.path),
    ['.well-known/security.txt', 'deployment.json', 'index.html'],
  );
  assert.match(first.treeSha256, /^[0-9a-f]{64}$/);
  assert.equal(
    createHash('sha256').update(first.sha256Manifest).digest('hex'),
    first.treeSha256,
  );
});

test('symlinks fail closed before a Pages archive can dereference them', async (t) => {
  const { root, temporary } = await fixtureRoot(t);
  const outside = path.join(temporary, 'outside-secret.txt');
  await writeFile(outside, 'must not enter the deployment');
  await symlink(outside, path.join(root, 'leak.txt'));

  await assert.rejects(
    inventoryPagesTree(root),
    /may not contain symlinks: leak\.txt/,
  );
});

test('evidence is written outside the deployed tree with no secret-shaped fields', async (t) => {
  const { root, temporary } = await fixtureRoot(t);
  const output = path.join(temporary, 'evidence');
  const priorRepository = process.env.GITHUB_REPOSITORY;
  const priorSha = process.env.GITHUB_SHA;
  const priorRef = process.env.GITHUB_REF_NAME;
  process.env.GITHUB_REPOSITORY = 'sonus-auris/sonus-auris-site.web';
  process.env.GITHUB_SHA = 'a'.repeat(40);
  process.env.GITHUB_REF_NAME = 'main';
  t.after(() => {
    if (priorRepository === undefined) delete process.env.GITHUB_REPOSITORY;
    else process.env.GITHUB_REPOSITORY = priorRepository;
    if (priorSha === undefined) delete process.env.GITHUB_SHA;
    else process.env.GITHUB_SHA = priorSha;
    if (priorRef === undefined) delete process.env.GITHUB_REF_NAME;
    else process.env.GITHUB_REF_NAME = priorRef;
  });

  const { evidencePath, manifestPath } = await writePagesEvidence({
    rootDirectory: root,
    outputDirectory: output,
  });
  const evidenceText = await readFile(evidencePath, 'utf8');
  const evidence = JSON.parse(evidenceText);
  const manifest = await readFile(manifestPath, 'utf8');

  assert.equal(evidence.commitSha, 'a'.repeat(40));
  assert.equal(evidence.refName, 'main');
  assert.equal(evidence.fileCount, 3);
  assert.ok(manifest.includes('  .well-known/security.txt\n'));
  assert.doesNotMatch(
    evidenceText.toLowerCase(),
    /authorization|cookie|password|private_key|secret|token/,
  );

  await assert.rejects(
    writePagesEvidence({
      rootDirectory: root,
      outputDirectory: path.join(root, 'evidence'),
    }),
    /outside the deployed tree/,
  );
});

test('packaging is deterministic and preserves .well-known/security.txt', async (t) => {
  const { root, temporary } = await fixtureRoot(t);
  const outputOne = path.join(temporary, 'one.tar');
  const outputTwo = path.join(temporary, 'two.tar');
  const evidenceOne = path.join(temporary, 'evidence-one');
  const evidenceTwo = path.join(temporary, 'evidence-two');

  for (const [output, evidence] of [
    [outputOne, evidenceOne],
    [outputTwo, evidenceTwo],
  ]) {
    await execFileAsync(
      'bash',
      ['scripts/package-pages-artifact.sh', root, output, evidence],
      { cwd: path.resolve('.') },
    );
  }

  const [one, two] = await Promise.all([
    readFile(outputOne),
    readFile(outputTwo),
  ]);
  assert.equal(
    createHash('sha256').update(one).digest('hex'),
    createHash('sha256').update(two).digest('hex'),
  );

  const { stdout } = await execFileAsync('tar', ['-tf', outputOne]);
  assert.match(stdout, /^\.\/\.well-known\/security\.txt$/m);
  assert.match(stdout, /^\.\/deployment\.json$/m);
  assert.match(stdout, /^\.\/index\.html$/m);
});
