import assert from 'node:assert/strict';
import test from 'node:test';

import { scanWorkflowText } from './check-pr-workflow-safety.mjs';

const CHECKOUT_SHA = '11bd71901bbe5b1630ceea73d27597364c9af683';

function codes(source) {
  return scanWorkflowText(source, '.github/workflows/fixture.yml').map((issue) => issue.code);
}

function safePullRequestWorkflow(extra = '') {
  return `
name: Safe pull request workflow
on:
  pull_request:
permissions:
  contents: read
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@${CHECKOUT_SHA}
        with:
          persist-credentials: false
      - run: node --test
${extra}`;
}

test('accepts a read-only pull_request workflow with immutable checkout', () => {
  assert.deepEqual(codes(safePullRequestWorkflow()), []);
});

test('recognizes quoted and inline pull request events', () => {
  const source = `
name: Inline
"on": [push, pull_request]
permissions:
  contents: read
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@${CHECKOUT_SHA}
        with:
          persist-credentials: false
      - run: true
`;
  assert.deepEqual(codes(source), []);
});

test('ignores push-only workflows because this guard is scoped to PR trust boundaries', () => {
  const source = `
name: Release
on:
  push:
    tags: ['v*']
permissions:
  contents: write
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: git push --tags
`;
  assert.deepEqual(codes(source), []);
});

test('rejects write-all and write-capable permissions', () => {
  const source = `
name: Writable
on: pull_request
permissions: write-all
jobs:
  publish:
    permissions:
      contents: write
    runs-on: ubuntu-latest
    steps:
      - run: true
`;
  assert.deepEqual(new Set(codes(source)), new Set(['write-all-permissions', 'write-permission']));
});

test('rejects mutable action references', () => {
  const source = safePullRequestWorkflow().replace(`actions/checkout@${CHECKOUT_SHA}`, 'actions/checkout@v4');
  assert.ok(codes(source).includes('mutable-action-ref'));
});

test('rejects a checkout that retains repository credentials', () => {
  const source = safePullRequestWorkflow().replace('persist-credentials: false', 'persist-credentials: true');
  assert.ok(codes(source).includes('checkout-persists-credentials'));
});

test('rejects pull_request_target checkout of attacker-controlled head content', () => {
  const source = `
name: Unsafe target checkout
on:
  pull_request_target:
permissions:
  contents: read
  pull-requests: read
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@${CHECKOUT_SHA}
        with:
          ref: \${{ github.event.pull_request.head.sha }}
          repository: \${{ github.event.pull_request.head.repo.full_name }}
          persist-credentials: false
      - run: node scripts/inspect.mjs
`;
  assert.ok(codes(source).includes('target-checkout-head'));
});

test('accepts pull_request_target when checkout is explicitly pinned to the trusted base', () => {
  const source = `
name: Trusted target inspection
on:
  pull_request_target:
permissions:
  contents: read
  pull-requests: read
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@${CHECKOUT_SHA}
        with:
          ref: \${{ github.event.repository.default_branch }}
          persist-credentials: false
      - run: node scripts/inspect-inert-data.mjs
`;
  assert.deepEqual(codes(source), []);
});

test('rejects branch publication and local commit commands', () => {
  const source = safePullRequestWorkflow(`
      - run: |
          git commit -am generated
          git push --force origin HEAD
`);
  const found = codes(source);
  assert.ok(found.includes('git-commit'));
  assert.ok(found.includes('git-push'));
});

test('rejects pull request mutation commands', () => {
  const source = safePullRequestWorkflow(`
      - run: gh pr merge --squash 123
`);
  assert.ok(codes(source).includes('gh-pr-mutation'));
});

test('rejects workflow self-deletion', () => {
  const source = safePullRequestWorkflow(`
      - run: git rm .github/workflows/generated.yml
`);
  assert.ok(codes(source).includes('workflow-self-delete'));
});

test('rejects encoded data piped into a shell', () => {
  const source = safePullRequestWorkflow(`
      - run: printf '%s' "$PAYLOAD" | base64 --decode | bash
`);
  assert.ok(codes(source).includes('decoded-shell'));
});

test('rejects direct curl mutation of GitHub refs and workflows', () => {
  const source = safePullRequestWorkflow(`
      - run: curl -X POST https://api.github.com/repos/o/r/git/refs
`);
  assert.ok(codes(source).includes('github-ref-api-mutation'));
});

test('rejects untrusted pull request text interpolated into a target shell command', () => {
  const source = `
name: Unsafe interpolation
on: pull_request_target
permissions:
  contents: read
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - run: echo "\${{ github.event.pull_request.title }}"
`;
  assert.ok(codes(source).includes('target-untrusted-shell-input'));
});

test('rejects mutable Docker actions', () => {
  const source = `
name: Mutable container action
on: pull_request
permissions:
  contents: read
jobs:
  inspect:
    runs-on: ubuntu-latest
    steps:
      - uses: docker://alpine:3.20
`;
  assert.ok(codes(source).includes('mutable-docker-action'));
});

test('rejects oversized and NUL-containing workflow data before scanning', () => {
  assert.deepEqual(codes(`on: pull_request\n# ${'x'.repeat(512 * 1024)}`), ['workflow-too-large']);
  assert.deepEqual(codes('on: pull_request\n\0'), ['nul-byte']);
});
