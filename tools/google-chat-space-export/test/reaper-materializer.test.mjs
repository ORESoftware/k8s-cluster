import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildContentFreeLinearSection,
  fetchJsonWithRetry,
  materializePlan,
  redactSensitiveText,
  safeIssueTitle,
} from '../reaper-materializer.mjs';
import { createGitHubClient } from '../reaper-github.mjs';

function candidate(overrides = {}) {
  return {
    candidateKey: 'google-chat:AAQAoHKdzvI:111111111111111111111111',
    action: 'create',
    title: 'Harden the queue worker',
    sourceKeys: ['google-chat:AAQAoHKdzvI:spaces/AAQAoHKdzvI/messages/abc.abc'],
    messageCount: 1,
    substantiveMessageCount: 1,
    firstCreateTime: '2026-08-19T00:00:00.000Z',
    lastCreateTime: '2026-08-19T00:00:00.000Z',
    githubReferences: {
      repositories: ['ORESoftware/k8s-cluster'],
      organizations: ['ORESoftware'],
    },
    exactExistingIssues: [],
    ...overrides,
  };
}

function plan(candidates) {
  return {
    planId: 'google-chat-import-plan:222222222222222222222222',
    candidates,
  };
}

function fakeLinearStore() {
  const issues = [];
  let creates = 0;
  let updates = 0;
  return {
    issues,
    counts: () => ({ creates, updates }),
    client: {
      async getIssue(identifier) {
        return issues.find((issue) => issue.identifier === identifier) || null;
      },
      async findByCandidate(candidateKey, title) {
        return issues.filter(
          (issue) => issue.description.includes(candidateKey) || issue.title.toLowerCase() === title.toLowerCase(),
        );
      },
      async createIssue({ title, description }) {
        creates += 1;
        const issue = {
          id: `issue-${creates}`,
          identifier: `DEN-${9000 + creates}`,
          title,
          description,
          url: null,
          state: { name: 'Backlog', type: 'backlog' },
          project: null,
        };
        issues.push(issue);
        return issue;
      },
      async appendSection(issue, section) {
        updates += 1;
        issue.description = `${issue.description}\n\n${section.body}`;
        return issue;
      },
    },
  };
}

test('redacts common credential, email, and phone shapes before Linear egress', () => {
  const fakeGitHubToken = ['ghp', '123456789012345678901234567890'].join('_');
  const input = `use ${fakeGitHubToken} and alex@example.com +1 (512) 555-1212`;
  const output = redactSensitiveText(input);
  assert.equal(output.includes(fakeGitHubToken), false);
  assert.equal(output.includes('alex@example.com'), false);
  assert.equal(output.includes('555-1212'), false);
  assert.match(output, /\[REDACTED_SECRET\]/);
  assert.match(output, /\[REDACTED_EMAIL\]/);
  assert.match(output, /\[REDACTED_PHONE\]/);
});

test('builds a content-free provenance section with digests, not raw source identifiers', () => {
  const item = candidate();
  const section = buildContentFreeLinearSection(item);
  assert.match(section, /google-chat-reaper:google-chat:AAQAoHKdzvI:111111111111111111111111/);
  assert.match(section, /sha256:[0-9a-f]{64}/);
  assert.equal(section.includes(item.sourceKeys[0]), false);
  assert.equal(section.includes('message bodies'), true);
});

test('normalizes valid Google Chat microsecond instants for content-free provenance', () => {
  const section = buildContentFreeLinearSection(
    candidate({
      firstCreateTime: '2026-08-19T00:00:00.123456Z',
      lastCreateTime: '2026-08-19T00:00:01.987654Z',
    }),
  );
  assert.match(section, /First message time: 2026-08-19T00:00:00\.123Z/);
  assert.match(section, /Last message time: 2026-08-19T00:00:01\.987Z/);
});

test('materialization is idempotent and reuses the same Linear issue on rerun', async () => {
  const store = fakeLinearStore();
  const github = {
    async findEvidence() {
      return { pullRequests: ['ORESoftware/k8s-cluster#1307'], defaultBranchCommits: [] };
    },
  };
  const first = await materializePlan(plan([candidate()]), { linear: store.client, github });
  const second = await materializePlan(plan([candidate()]), { linear: store.client, github });
  assert.deepEqual(store.counts(), { creates: 1, updates: 0 });
  assert.equal(first.evidence.entries[0].linearIssues[0], second.evidence.entries[0].linearIssues[0]);
  assert.deepEqual(first.evidence, second.evidence);
  assert.equal(first.summary.counts.coveredWithImplementation, 1);
});

test('manual-review candidates are durably owned but quarantined from automated completion', async () => {
  const store = fakeLinearStore();
  const github = {
    async findEvidence() {
      throw new Error('GitHub evidence lookup must not run for quarantined candidates');
    },
  };
  const result = await materializePlan(
    plan([candidate({ action: 'manual-review', title: 'Ambiguous multi-project request' })]),
    { linear: store.client, github },
  );
  assert.equal(store.counts().creates, 1);
  assert.equal(result.evidence.entries[0].disposition, 'quarantined');
  assert.equal(result.evidence.entries[0].reasonCode, 'requires_human_review');
  assert.deepEqual(result.evidence.entries[0].linearIssues, undefined);
  assert.match(store.issues[0].title, /^\[Google Chat review\]/);
});

test('actionable candidates without GitHub evidence remain explicit implementation gaps', async () => {
  const store = fakeLinearStore();
  const github = {
    async findEvidence() {
      return { pullRequests: [], defaultBranchCommits: [] };
    },
  };
  const result = await materializePlan(plan([candidate()]), { linear: store.client, github });
  assert.equal(result.evidence.entries[0].disposition, 'covered');
  assert.deepEqual(result.evidence.entries[0].pullRequests, []);
  assert.equal(result.summary.counts.awaitingImplementation, 1);
  assert.equal(result.summary.counts.coveredWithImplementation, 0);
});

test('non-actionable candidates never call Linear or GitHub', async () => {
  const fail = async () => { throw new Error('unexpected call'); };
  const result = await materializePlan(
    plan([candidate({ action: 'skip-non-actionable' })]),
    {
      linear: { getIssue: fail, findByCandidate: fail, createIssue: fail, appendSection: fail },
      github: { findEvidence: fail },
    },
  );
  assert.deepEqual(result.evidence.entries[0], {
    candidateKey: candidate().candidateKey,
    disposition: 'excluded',
    reasonCode: 'non_actionable',
  });
});

test('request helper retries 429 responses and respects a bounded attempt count', async () => {
  let calls = 0;
  const fetchImpl = async () => {
    calls += 1;
    if (calls === 1) {
      return new Response('rate limited', { status: 429, headers: { 'retry-after': '0' } });
    }
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  };
  const output = await fetchJsonWithRetry(
    'https://example.test/resource',
    {},
    { fetchImpl, attempts: 2, sleepImpl: async () => {}, timeoutMs: 1000 },
  );
  assert.deepEqual(output, { ok: true });
  assert.equal(calls, 2);
});

test('GitHub 422 search scopes remain explicit evidence gaps instead of aborting', async () => {
  const client = createGitHubClient({
    token: 'test-token',
    allowedOwners: ['ORESoftware'],
    fetchImpl: async () =>
      new Response(
        JSON.stringify({
          message: 'Validation Failed',
          errors: [{ resource: 'Search', field: 'q', code: 'invalid' }],
        }),
        { status: 422, headers: { 'content-type': 'application/json' } },
      ),
  });
  const evidence = await client.findEvidence({
    issueIdentifiers: ['DEN-4047'],
    candidate: {
      githubReferences: {
        repositories: ['ORESoftware/inaccessible-repository'],
        organizations: ['ORESoftware'],
      },
    },
  });
  assert.deepEqual(evidence, { pullRequests: [], defaultBranchCommits: [] });
});

test('GitHub 403 search limits remain explicit gaps and stop repeated searches', async () => {
  let calls = 0;
  const client = createGitHubClient({
    token: 'test-token',
    allowedOwners: ['ORESoftware'],
    fetchImpl: async () => {
      calls += 1;
      return new Response(JSON.stringify({ message: 'API rate limit exceeded' }), {
        status: 403,
        headers: { 'content-type': 'application/json' },
      });
    },
  });
  const request = {
    issueIdentifiers: ['DEN-4047'],
    candidate: {
      githubReferences: {
        repositories: ['ORESoftware/k8s-cluster'],
        organizations: ['ORESoftware'],
      },
    },
  };
  assert.deepEqual(
    await client.findEvidence(request),
    { pullRequests: [], defaultBranchCommits: [] },
  );
  assert.deepEqual(
    await client.findEvidence(request),
    { pullRequests: [], defaultBranchCommits: [] },
  );
  assert.equal(calls, 1);
});

test('title sanitizer bounds and redacts issue titles', () => {
  const title = safeIssueTitle(`Use lin_api_${'A'.repeat(80)} for ${'x'.repeat(200)}`);
  assert.equal(title.length <= 120, true);
  assert.equal(title.includes('lin_api_'), false);
});

test('create circuit breaker stops an unexpectedly large mutation batch', async () => {
  const store = fakeLinearStore();
  const github = { async findEvidence() { return { pullRequests: [], defaultBranchCommits: [] }; } };
  await assert.rejects(
    materializePlan(
      plan([
        candidate(),
        candidate({ candidateKey: 'google-chat:AAQAoHKdzvI:333333333333333333333333', title: 'Second item' }),
      ]),
      { linear: store.client, github },
      { maxCreates: 1 },
    ),
    /circuit breaker reached/,
  );
  assert.equal(store.counts().creates, 1);
});

test('duplicate candidate markers fail closed instead of selecting an arbitrary issue', async () => {
  const item = candidate();
  const marker = `<!-- google-chat-reaper:${item.candidateKey} -->`;
  const linear = {
    async getIssue() { return null; },
    async findByCandidate() {
      return [
        { id: '1', identifier: 'DEN-1', title: 'one', description: marker },
        { id: '2', identifier: 'DEN-2', title: 'two', description: marker },
      ];
    },
    async createIssue() { throw new Error('must not create'); },
    async appendSection() { throw new Error('must not update'); },
  };
  const github = { async findEvidence() { throw new Error('must not search'); } };
  await assert.rejects(
    materializePlan(plan([item]), { linear, github }),
    /Duplicate Linear issues contain candidate/,
  );
});
