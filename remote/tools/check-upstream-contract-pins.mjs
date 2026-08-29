#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDirectory, '..', '..');
const pinsDirectory = resolve(repoRoot, 'remote/api-contracts/upstream-pins');
const SHA_PATTERN = /^[0-9a-f]{40}$/;
const SERVICE_PATTERN = /^[a-z0-9][a-z0-9._-]*$/;

function git(args) {
  return execFileSync('git', args, {
    cwd: repoRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

function assertRepoPath(path, label) {
  assert.equal(typeof path, 'string', `${label} must be a string`);
  assert.ok(path.length > 0, `${label} must not be empty`);
  assert.ok(!path.includes('\0'), `${label} contains NUL`);
  assert.ok(!path.startsWith('/'), `${label} must be repository-relative`);
  assert.ok(
    path.split('/').every((segment) => segment && segment !== '.' && segment !== '..'),
    `${label} contains an unsafe path segment`,
  );
  const absolute = resolve(repoRoot, path);
  assert.ok(
    absolute === repoRoot || absolute.startsWith(`${repoRoot}${sep}`),
    `${label} escapes the repository`,
  );
  return absolute;
}

function stagedEntry(path) {
  const output = git(['ls-files', '--stage', '--', path]).trim();
  const lines = output.split('\n').filter(Boolean);
  assert.equal(lines.length, 1, `${path} must have exactly one index entry`);
  const match = lines[0].match(/^(\d{6}) ([0-9a-f]{40}) \d+\t(.+)$/);
  assert.ok(match, `unable to parse index entry for ${path}`);
  return { mode: match[1], sha: match[2], path: match[3] };
}

function submoduleUrl(gitmodules, path) {
  const escaped = path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const pattern = new RegExp(
    `\\[submodule "[^"]+"\\]\\s+path = ${escaped}\\s+url = ([^\\r\\n]+)`,
    'm',
  );
  return gitmodules.match(pattern)?.[1]?.trim() ?? null;
}

function githubRepositoryFromUrl(rawUrl) {
  const match = rawUrl?.match(/github\.com[:/]([^/]+\/[^/]+?)(?:\.git)?$/i);
  return match?.[1] ?? null;
}

async function main() {
  assert.ok(existsSync(pinsDirectory), 'remote/api-contracts/upstream-pins is missing');
  const files = (await readdir(pinsDirectory))
    .filter((name) => name.endsWith('.json'))
    .sort();
  assert.ok(files.length > 0, 'no upstream contract pin documents were found');

  const gitmodules = await readFile(resolve(repoRoot, '.gitmodules'), 'utf8');
  const config = JSON.parse(
    await readFile(resolve(repoRoot, 'remote/config/api-contracts.json'), 'utf8'),
  );
  const seenServices = new Set();
  const seenPaths = new Set();
  const report = [];

  for (const fileName of files) {
    const document = JSON.parse(await readFile(resolve(pinsDirectory, fileName), 'utf8'));
    assert.equal(document.schemaVersion, 1, `${fileName} schemaVersion drifted`);
    assert.match(document.service, SERVICE_PATTERN, `${fileName} has an invalid service`);
    assert.equal(fileName, `${document.service}.json`, `${fileName} must match its service key`);
    assert.ok(!seenServices.has(document.service), `duplicate service pin: ${document.service}`);
    seenServices.add(document.service);

    const deploymentPath = document.deploymentPath;
    assertRepoPath(deploymentPath, `${document.service}.deploymentPath`);
    assert.ok(
      deploymentPath.startsWith('remote/deployments/'),
      `${document.service} pin must target remote/deployments`,
    );
    assert.ok(!seenPaths.has(deploymentPath), `duplicate deployment pin: ${deploymentPath}`);
    seenPaths.add(deploymentPath);

    for (const field of ['acceptedCommit', 'rollbackCommit', 'sourceHeadCommit', 'contractGitBlobSha']) {
      assert.match(document[field], SHA_PATTERN, `${document.service}.${field} must be a Git SHA`);
    }
    assert.notEqual(
      document.acceptedCommit,
      document.rollbackCommit,
      `${document.service} accepted and rollback commits must differ`,
    );
    assert.equal(
      document.contractVerification,
      'upstream-ci-verified-parent-digest-pending',
      `${document.service} contract verification state drifted`,
    );

    const sourcePr = new URL(document.sourcePullRequest);
    assert.equal(sourcePr.protocol, 'https:', `${document.service} source PR must use HTTPS`);
    assert.equal(sourcePr.hostname, 'github.com', `${document.service} source PR must be on GitHub`);
    assert.match(sourcePr.pathname, /^\/[^/]+\/[^/]+\/pull\/\d+$/, `${document.service} source PR URL is invalid`);

    assertRepoPath(document.contractPath, `${document.service}.contractPath`);
    assert.ok(
      !document.contractPath.startsWith('remote/'),
      `${document.service}.contractPath must be relative to the source repository`,
    );

    const entry = stagedEntry(deploymentPath);
    assert.equal(entry.mode, '160000', `${deploymentPath} must remain a gitlink`);
    assert.equal(entry.path, deploymentPath, `${deploymentPath} index path drifted`);
    assert.equal(
      entry.sha,
      document.acceptedCommit,
      `${deploymentPath} is not pinned to the accepted source commit`,
    );

    const configuredRepository = config.submoduleSources?.[document.service];
    assert.equal(
      configuredRepository,
      document.repository,
      `${document.service} submoduleSources repository drifted`,
    );
    const rawUrl = submoduleUrl(gitmodules, deploymentPath);
    assert.ok(rawUrl, `${deploymentPath} is missing from .gitmodules`);
    assert.equal(
      githubRepositoryFromUrl(rawUrl),
      document.repository,
      `${deploymentPath} .gitmodules URL drifted`,
    );

    report.push({
      service: document.service,
      repository: document.repository,
      deploymentPath,
      acceptedCommit: document.acceptedCommit,
      rollbackCommit: document.rollbackCommit,
      sourcePullRequest: document.sourcePullRequest,
      contractPath: document.contractPath,
      contractGitBlobSha: document.contractGitBlobSha,
      contractVerification: document.contractVerification,
    });
  }

  process.stdout.write(`${JSON.stringify({ schemaVersion: 1, pins: report }, null, 2)}\n`);
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error.message ?? String(error)}\n`);
  process.exitCode = 1;
});
