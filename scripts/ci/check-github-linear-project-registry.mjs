#!/usr/bin/env node

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const EXPECTED_ORGANIZATION_COUNT = 64;
export const PROJECT_NUMBER_EXCEPTIONS = new Map([['dancing-dragons', 4]]);

const ORG_PATTERN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
const LINEAR_PROJECT_PATTERN = /^https:\/\/linear\.app\/denman\/project\/[a-z0-9][a-z0-9-]*$/;
const CREDENTIAL_PATTERN = /(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|https?:\/\/[^/\s:@]+:[^/\s@]+@)/;

function asciiCompare(left, right) {
  return left.toLowerCase() < right.toLowerCase()
    ? -1
    : left.toLowerCase() > right.toLowerCase()
      ? 1
      : left < right
        ? -1
        : left > right
          ? 1
          : 0;
}

function assertSafeAbsoluteUrl(value, expectedOrigin, label) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${label} is not a valid absolute URL: ${value}`);
  }
  assert.equal(url.origin, expectedOrigin, `${label} must use ${expectedOrigin}`);
  assert.equal(url.username, '', `${label} must not contain a username`);
  assert.equal(url.password, '', `${label} must not contain a password`);
  assert.equal(url.search, '', `${label} must not contain a query string`);
  assert.equal(url.hash, '', `${label} must not contain a fragment`);
  assert.equal(url.protocol, 'https:', `${label} must use HTTPS`);
}

export function parseRegistry(text) {
  assert.equal(typeof text, 'string', 'registry must be text');
  assert(!text.includes('\r'), 'registry must use LF line endings');
  assert(!CREDENTIAL_PATTERN.test(text), 'registry contains a credential-shaped value');

  const lines = text.trimEnd().split('\n');
  assert.equal(lines[0], 'organization\tlinear_url', 'registry header changed');
  assert(lines.length > 1, 'registry must contain organization rows');

  return lines.slice(1).map((line, index) => {
    assert(line.length > 0, `row ${index + 2} is empty`);
    const columns = line.split('\t');
    assert.equal(columns.length, 2, `row ${index + 2} must contain exactly two TSV columns`);
    return {
      organization: columns[0],
      linearUrl: columns[1],
      sourceLine: index + 2,
    };
  });
}

export function deriveRecord(row) {
  const key = row.organization.toLowerCase();
  const projectNumber = PROJECT_NUMBER_EXCEPTIONS.get(key) ?? 1;
  return {
    organization: row.organization,
    organizationUrl: `https://github.com/${row.organization}`,
    governanceRepository: `${row.organization}/.github`,
    governanceRepositoryUrl: `https://github.com/${row.organization}/.github`,
    projectTitle: `${row.organization}-project`,
    projectNumber,
    projectUrl: `https://github.com/orgs/${row.organization}/projects/${projectNumber}`,
    linearUrl: row.linearUrl,
  };
}

export function validateRegistry(rows, { expectedCount = EXPECTED_ORGANIZATION_COUNT } = {}) {
  assert.equal(rows.length, expectedCount, `expected ${expectedCount} organizations, found ${rows.length}`);

  const seenOrganizations = new Map();
  const seenLinearUrls = new Map();
  let previous = null;

  for (const row of rows) {
    const { organization, linearUrl, sourceLine } = row;
    assert(ORG_PATTERN.test(organization), `invalid GitHub organization login on line ${sourceLine}: ${organization}`);
    assert(!organization.includes('--'), `organization login contains consecutive hyphens on line ${sourceLine}`);

    const key = organization.toLowerCase();
    assert(!seenOrganizations.has(key), `duplicate organization (case-insensitive): ${organization}`);
    seenOrganizations.set(key, sourceLine);

    assert(LINEAR_PROJECT_PATTERN.test(linearUrl), `invalid Linear project URL on line ${sourceLine}: ${linearUrl}`);
    assertSafeAbsoluteUrl(linearUrl, 'https://linear.app', `Linear URL on line ${sourceLine}`);
    assert(!seenLinearUrls.has(linearUrl), `duplicate Linear project URL on line ${sourceLine}: ${linearUrl}`);
    seenLinearUrls.set(linearUrl, sourceLine);

    if (previous) {
      assert(
        asciiCompare(previous.organization, organization) < 0,
        `registry must be sorted by organization: ${previous.organization} appears before ${organization}`,
      );
    }
    previous = row;

    const derived = deriveRecord(row);
    assertSafeAbsoluteUrl(derived.organizationUrl, 'https://github.com', `organization URL for ${organization}`);
    assertSafeAbsoluteUrl(derived.governanceRepositoryUrl, 'https://github.com', `governance repository URL for ${organization}`);
    assertSafeAbsoluteUrl(derived.projectUrl, 'https://github.com', `GitHub Project URL for ${organization}`);
    assert.equal(
      derived.projectNumber,
      key === 'dancing-dragons' ? 4 : 1,
      `unexpected project-number rule for ${organization}`,
    );
  }

  return rows.map(deriveRecord);
}

export function validateDocumentation(markdown) {
  assert.equal(typeof markdown, 'string', 'documentation must be text');
  assert(!CREDENTIAL_PATTERN.test(markdown), 'documentation contains a credential-shaped value');

  const requiredFragments = [
    'ops/portfolio/github-linear-project-registry.tsv',
    '<canonical-org-login>-project',
    'dancing-dragons',
    'project `4`',
    'public `<org>/.github` repository',
    'resolved semantically',
    'never resolve conflicts by blindly choosing one side',
  ];
  for (const fragment of requiredFragments) {
    assert(markdown.includes(fragment), `documentation is missing required contract text: ${fragment}`);
  }
}

export function renderMarkdown(records) {
  const lines = [
    '# GitHub organization ↔ GitHub Project ↔ Linear registry',
    '',
    `Validated organizations: **${records.length}**`,
    '',
    '| GitHub organization | Canonical GitHub Project | Linear project | Governance repository |',
    '| --- | --- | --- | --- |',
  ];
  for (const record of records) {
    lines.push(
      `| [${record.organization}](${record.organizationUrl}) | [${record.projectTitle}](${record.projectUrl}) | [Linear](${record.linearUrl}) | [${record.governanceRepository}](${record.governanceRepositoryUrl}) |`,
    );
  }
  lines.push('');
  return lines.join('\n');
}

export async function validateRepository(root = process.cwd()) {
  const registryPath = path.join(root, 'ops/portfolio/github-linear-project-registry.tsv');
  const docsPath = path.join(root, 'docs/portfolio/github-linear-project-registry.md');
  const [registryText, docsText] = await Promise.all([
    readFile(registryPath, 'utf8'),
    readFile(docsPath, 'utf8'),
  ]);
  const records = validateRegistry(parseRegistry(registryText));
  validateDocumentation(docsText);
  return records;
}

async function main() {
  const records = await validateRepository();
  if (process.argv.includes('--markdown')) {
    process.stdout.write(renderMarkdown(records));
    return;
  }
  process.stdout.write(
    `${JSON.stringify({ schemaVersion: 1, organizationCount: records.length, records }, null, 2)}\n`,
  );
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.stack : String(error));
    process.exitCode = 1;
  });
}
