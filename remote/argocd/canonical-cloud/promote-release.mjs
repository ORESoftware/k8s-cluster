#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const releaseShaPattern = /^[0-9a-f]{40}$/;
const digestPattern = /^sha256:[0-9a-f]{64}$/;

function optionValue(arguments_, name) {
  const index = arguments_.indexOf(name);
  if (index === -1 || index + 1 >= arguments_.length) {
    throw new Error(`missing required option ${name}`);
  }
  return arguments_[index + 1];
}

function replaceExactly(source, pattern, replacement, expectedCount, label) {
  const matches = source.match(pattern) ?? [];
  if (matches.length !== expectedCount) {
    throw new Error(`${label}: expected ${expectedCount} matches, found ${matches.length}`);
  }
  return source.replace(pattern, replacement);
}

export function renderPromotion(source, { repository, digest, releaseSha, label }) {
  if (!releaseShaPattern.test(releaseSha)) {
    throw new Error(`${label}: release SHA must be exactly 40 lowercase hexadecimal characters`);
  }
  if (!digestPattern.test(digest)) {
    throw new Error(`${label}: digest must match sha256:<64 lowercase hexadecimal characters>`);
  }

  const escapedRepository = repository.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  let rendered = replaceExactly(
    source,
    new RegExp(`image: ${escapedRepository}(?:[^\\s]+)`, 'g'),
    `image: ${repository}@${digest}`,
    1,
    `${label} image`,
  );
  rendered = replaceExactly(
    rendered,
    /canonical\.cloud\/release-sha: "[0-9a-f]{40}"/g,
    `canonical.cloud/release-sha: "${releaseSha}"`,
    2,
    `${label} release annotations`,
  );
  return rendered;
}

async function promoteFile(path, promotion, checkOnly) {
  const source = await readFile(path, 'utf8');
  const rendered = renderPromotion(source, promotion);
  if (checkOnly) {
    if (rendered !== source) {
      throw new Error(`${promotion.label}: manifest does not match the requested release`);
    }
    return;
  }
  if (rendered !== source) {
    await writeFile(path, rendered, 'utf8');
  }
}

async function main() {
  const arguments_ = process.argv.slice(2);
  const checkOnly = arguments_.includes('--check');
  const releaseSha = optionValue(arguments_, '--release-sha');
  const webDigest = optionValue(arguments_, '--web-digest');
  const revokerDigest = optionValue(arguments_, '--revoker-digest');
  const base = dirname(fileURLToPath(import.meta.url));

  await promoteFile(
    resolve(base, 'web.deployment.yaml'),
    {
      repository: 'ghcr.io/canonical-cloud/canonical-web-server',
      digest: webDigest,
      releaseSha,
      label: 'web',
    },
    checkOnly,
  );
  await promoteFile(
    resolve(base, 'revoker.deployment.yaml'),
    {
      repository: 'ghcr.io/canonical-cloud/canonical-session-revoker',
      digest: revokerDigest,
      releaseSha,
      label: 'revoker',
    },
    checkOnly,
  );

  process.stdout.write(
    checkOnly
      ? `canonical-cloud manifests already match ${releaseSha}\n`
      : `promoted canonical-cloud manifests to ${releaseSha}; review and commit the digest change\n`,
  );
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : '';
if (invokedPath === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
