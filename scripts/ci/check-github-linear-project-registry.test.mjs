import assert from 'node:assert/strict';
import test from 'node:test';

import {
  deriveRecord,
  parseRegistry,
  renderMarkdown,
  validateDocumentation,
  validateRegistry,
} from './check-github-linear-project-registry.mjs';

const validRows = [
  {
    organization: 'alpha-org',
    linearUrl: 'https://linear.app/denman/project/githubcomalpha-org-a1b2c3',
    sourceLine: 2,
  },
  {
    organization: 'dancing-dragons',
    linearUrl: 'https://linear.app/denman/project/githubcomdancing-dragons-d4e5f6',
    sourceLine: 3,
  },
];

const sharedRows = [
  {
    organization: 'flags-2-env',
    linearUrl: 'https://linear.app/denman/project/githubcomflags-2-env-05db5133a267',
    sourceLine: 2,
  },
  {
    organization: 'flags-2-env-test',
    linearUrl: 'https://linear.app/denman/project/githubcomflags-2-env-05db5133a267',
    sourceLine: 3,
  },
];

function expectFailure(callback, fragment) {
  assert.throws(callback, (error) => {
    assert(error instanceof Error);
    assert.match(error.message, new RegExp(fragment));
    return true;
  });
}

test('parses a strict two-column LF-only registry', () => {
  const rows = parseRegistry(
    'organization\tlinear_url\nalpha-org\thttps://linear.app/denman/project/githubcomalpha-org-a1b2c3\n',
  );
  assert.deepEqual(rows, [validRows[0]]);

  expectFailure(
    () => parseRegistry('organization\tlinear_url\r\nalpha-org\thttps://linear.app/denman/project/a\r\n'),
    'LF line endings',
  );
  expectFailure(
    () => parseRegistry('organization\tlinear_url\nalpha-org\thttps://linear.app/denman/project/a\textra\n'),
    'exactly two TSV columns',
  );
});

test('derives canonical GitHub Project and governance links', () => {
  const ordinary = deriveRecord(validRows[0]);
  assert.equal(ordinary.projectTitle, 'alpha-org-project');
  assert.equal(ordinary.projectNumber, 1);
  assert.equal(ordinary.projectUrl, 'https://github.com/orgs/alpha-org/projects/1');
  assert.equal(ordinary.governanceRepository, 'alpha-org/.github');

  const exception = deriveRecord(validRows[1]);
  assert.equal(exception.projectNumber, 4);
  assert.equal(exception.projectUrl, 'https://github.com/orgs/dancing-dragons/projects/4');
});

test('accepts sorted unique rows and renders all canonical links', () => {
  const records = validateRegistry(validRows, { expectedCount: 2 });
  const markdown = renderMarkdown(records);
  assert.match(markdown, /Validated organizations: \*\*2\*\*/);
  assert.match(markdown, /https:\/\/github\.com\/orgs\/alpha-org\/projects\/1/);
  assert.match(markdown, /https:\/\/github\.com\/orgs\/dancing-dragons\/projects\/4/);
  assert.match(markdown, /https:\/\/github\.com\/alpha-org\/\.github/);
});

test('allows only the explicit production/test Linear ownership shares', () => {
  assert.equal(validateRegistry(sharedRows, { expectedCount: 2 }).length, 2);

  expectFailure(
    () =>
      validateRegistry(
        [validRows[0], { ...validRows[1], linearUrl: validRows[0].linearUrl }],
        { expectedCount: 2 },
      ),
    'shared without an explicit production/test exception',
  );

  expectFailure(
    () =>
      validateRegistry(
        [sharedRows[0], { ...sharedRows[1], organization: 'flags-2-env-extra' }],
        { expectedCount: 2 },
      ),
    'share exception mismatch',
  );
});

test('rejects duplicate organization identity', () => {
  expectFailure(
    () =>
      validateRegistry(
        [validRows[0], { ...validRows[0], organization: 'ALPHA-ORG', sourceLine: 3 }],
        { expectedCount: 2 },
      ),
    'duplicate organization',
  );
});

test('rejects unsorted, malformed, credentialed, and ambiguous URLs', () => {
  expectFailure(
    () => validateRegistry([...validRows].reverse(), { expectedCount: 2 }),
    'must be sorted',
  );
  expectFailure(
    () =>
      validateRegistry(
        [{ ...validRows[0], organization: 'bad--org' }, validRows[1]],
        { expectedCount: 2 },
      ),
    'consecutive hyphens',
  );
  expectFailure(
    () =>
      validateRegistry(
        [
          {
            ...validRows[0],
            linearUrl: 'https://linear.app/denman/project/example?token=secret',
          },
          validRows[1],
        ],
        { expectedCount: 2 },
      ),
    'invalid Linear project URL',
  );

  const credentialedUrl = ['https://', 'user', ':', 'password', '@linear.app/denman/project/example'].join('');
  expectFailure(
    () => parseRegistry(`organization\tlinear_url\nalpha-org\t${credentialedUrl}\n`),
    'credential-shaped',
  );
});

test('requires the 71-org scope, semantic conflict policy, and explicit share contract in docs', () => {
  const complete = [
    'ops/portfolio/github-linear-project-registry.tsv',
    'current 71-organization fleet',
    'active 41-portfolio subset',
    '<canonical-org-login>-project',
    'dancing-dragons',
    'project `4`',
    'Linear project sharing is allowed only for',
    '`flags-2-env` and `flags-2-env-test`',
    '`networking-components` and `networking-components-test`',
    '`ores-otel` and `ores-otel-test`',
    'public `<org>/.github` repository',
    'resolved semantically',
    'never resolve conflicts by blindly choosing one side',
  ].join('\n');
  validateDocumentation(complete);

  expectFailure(
    () => validateDocumentation(complete.replace('resolved semantically', 'resolved automatically')),
    'resolved semantically',
  );
  expectFailure(
    () => validateDocumentation(complete.replace('`ores-otel` and `ores-otel-test`', '')),
    'ores-otel',
  );
});
