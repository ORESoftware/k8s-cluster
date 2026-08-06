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

export function renderPromotion(
  source,
  { repository, sourceRepositories = [repository], digest, releaseSha, label },
) {
  if (!releaseShaPattern.test(releaseSha)) {
    throw new Error(`${label}: release SHA must be exactly 40 lowercase hexadecimal characters`);
  }
  if (!digestPattern.test(digest)) {
    throw new Error(`${label}: digest must match sha256:<64 lowercase hexadecimal characters>`);
  }

  const repositories = [...new Set([repository, ...sourceRepositories])];
  if (
    repositories.some(
      (candidate) =>
        typeof candidate !== 'string' ||
        !/^ghcr\.io\/[a-z0-9][a-z0-9._/-]*$/.test(candidate),
    )
  ) {
    throw new Error(`${label}: repositories must be non-empty GHCR paths`);
  }
  const escapedRepositories = repositories
    .map((candidate) => candidate.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('|');
  let rendered = replaceExactly(
    source,
    new RegExp(
      `^([ \\t]*(?:-[ \\t]+)?image:[ \\t]*)(?:${escapedRepositories})(?=[:@])\\S+[ \\t]*$`,
      'gm',
    ),
    `$1${repository}@${digest}`,
    1,
    `${label} image`,
  );
  rendered = replaceExactly(
    rendered,
    /^([ \t]*canonical\.cloud\/release-sha:[ \t]*)"[0-9a-f]{40}"[ \t]*$/gm,
    `$1"${releaseSha}"`,
    2,
    `${label} release annotations`,
  );
  return rendered;
}

export function renderPromotionBatch(entries) {
  return entries.map(({ path, source, promotion }) => ({
    path,
    source,
    promotion,
    rendered: renderPromotion(source, promotion),
  }));
}

export async function promoteFiles(
  requests,
  {
    checkOnly = false,
    read = readFile,
    write = writeFile,
  } = {},
) {
  const loaded = await Promise.all(
    requests.map(async (request) => ({
      ...request,
      source: await read(request.path, 'utf8'),
    })),
  );
  const prepared = renderPromotionBatch(loaded);

  if (checkOnly) {
    for (const entry of prepared) {
      if (entry.rendered !== entry.source) {
        throw new Error(
          `${entry.promotion.label}: manifest does not match the requested release`,
        );
      }
    }
    return prepared;
  }

  await Promise.all(
    prepared
      .filter((entry) => entry.rendered !== entry.source)
      .map((entry) => write(entry.path, entry.rendered, 'utf8')),
  );
  return prepared;
}

async function main() {
  const arguments_ = process.argv.slice(2);
  const checkOnly = arguments_.includes('--check');
  const releaseSha = optionValue(arguments_, '--release-sha');
  const webDigest = optionValue(arguments_, '--web-digest');
  const apiDigest = optionValue(arguments_, '--api-digest');
  const revokerDigest = optionValue(arguments_, '--revoker-digest');
  const base = dirname(fileURLToPath(import.meta.url));

  const requests = [
    {
      path: resolve(base, 'web.deployment.yaml'),
      promotion: {
        repository: 'ghcr.io/canonical-cloud/canonical-web-server',
        sourceRepositories: [
          'ghcr.io/canonical-cloud/canonical-web-server-rs',
        ],
        digest: webDigest,
        releaseSha,
        label: 'web',
      },
    },
    {
      path: resolve(base, 'api.deployment.yaml'),
      promotion: {
        repository: 'ghcr.io/canonical-cloud/canonical-api-server',
        sourceRepositories: [
          'ghcr.io/canonical-cloud/canonical-api-server',
          'ghcr.io/canonical-cloud/canonical-web-server-rs',
        ],
        digest: apiDigest,
        releaseSha,
        label: 'api',
      },
    },
    {
      path: resolve(base, 'revoker.deployment.yaml'),
      promotion: {
        repository: 'ghcr.io/canonical-cloud/canonical-session-revoker',
        sourceRepositories: [
          'ghcr.io/canonical-cloud/canonical-web-server-rs',
        ],
        digest: revokerDigest,
        releaseSha,
        label: 'revoker',
      },
    },
  ];
  await promoteFiles(requests, { checkOnly });

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
