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

test('derives registry cardinality from the supplied rows by default', () => {
  assert.equal(validateRegistry([validRows[0]]).length, 1);
  assert.equal(validateRegistry(validRows).length, 2);
  expectFailure(() => validateRegistry(validRows, { expectedCount: 3 }), 'expected 3 organizations');
  expectFailure(() => validateRegistry(validRows, { expectedCount: 0 }), 'positive integer');
});

test('accepts sorted unique rows and renders all canonical links', () => {
  const records = validateRegistry(validRows);
  const markdown = renderMarkdown(records);
  assert.match(markdown, /Validated organizations: \*\*2\*\*/);
  assert.match(markdown, /https:\/\/github\.com\/orgs\/alpha-org\/projects\/1/);
  assert.match(markdown, /https:\/\/github\.com\/orgs\/dancing-dragons\/projects\/4/);
  assert.match(markdown, /https:\/\/github\.com\/alpha-org\/\.github/);
});

test('rejects duplicate organization and Linear ownership', () => {
  expectFailure(
    () => validateRegistry([validRows[0], { ...validRows[0], organization: 'ALPHA-ORG', sourceLine: 3 }]),
    'duplicate organization',
  );

  expectFailure(
    () => validateRegistry([validRows[0], { ...validRows[1], linearUrl: validRows[0].linearUrl }]),
    'duplicate Linear project URL',
  );
});

test('rejects unsorted, malformed, credentialed, and ambiguous URLs', () => {
  expectFailure(() => validateRegistry([...validRows].reverse()), 'must be sorted');
  expectFailure(
    () => validateRegistry([{ ...validRows[0], organization: 'bad--org' }, validRows[1]]),
    'consecutive hyphens',
  );
  expectFailure(
    () =>
      validateRegistry([
        {
          ...validRows[0],
          linearUrl: 'https://linear.app/denman/project/example?token=secret',
        },
        validRows[1],
      ]),
    'invalid Linear project URL',
  );

  const credentialedUrl = ['https://', 'user', ':', 'password', '@linear.app/denman/project/example'].join('');
  expectFailure(
    () => parseRegistry(`organization\tlinear_url\nalpha-org\t${credentialedUrl}\n`),
    'credential-shaped',
  );
});

test('requires the semantic conflict-resolution and dynamic-cardinality contract in docs', () => {
  const complete = [
    'ops/portfolio/github-linear-project-registry.tsv',
    'cardinality is derived from the checked-in TSV',
    '<canonical-org-login>-project',
    'dancing-dragons',
    'project `4`',
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
    () => validateDocumentation(complete.replace('cardinality is derived from the checked-in TSV', 'cardinality is fixed')),
    'cardinality is derived',
  );
});
