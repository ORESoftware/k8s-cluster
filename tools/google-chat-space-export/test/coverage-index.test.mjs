import assert from 'node:assert/strict';
import test from 'node:test';
import { collectCoverageIndex, githubReference, GitHubReader, legacyOrdinals, parseLegacyTables, referencesIn, safeTitle } from '../coverage-index.mjs';

const secret = 'synthetic-test-value';
const response = (value, status = 200) => new Response(JSON.stringify(value), { status });

for (const value of ['https://evil.invalid/a/b/issues/1', 'http://github.com/a/b/issues/1', 'https://user:secret@github.com/a/b/issues/1', 'https://github.com/a/b/issues/1?token=x', 'https://github.com/a/b/issues/0', 'https://github.com/a/b/issues/99999999999999999999', 'https://github.com/a/b/issues/1/elsewhere', null]) {
  test(`rejects noncanonical reference ${String(value)}`, () => assert.equal(githubReference(value), null));
}
test('normalizes a valid GitHub PR anchor without retaining it', () => assert.deepEqual(githubReference('https://github.com/owner/repo/pull/2#discussion'), { repository: 'owner/repo', kind: 'pull', number: 2, url: 'https://github.com/owner/repo/pull/2' }));
test('deduplicates body references and never retains bodies', () => assert.equal(referencesIn('https://github.com/a/b/issues/1 https://github.com/a/b/issues/1').length, 1));
test('expands, sorts and deduplicates ordinal ranges', () => assert.deepEqual(legacyOrdinals('5, 2-4, 3'), [2, 3, 4, 5]));
for (const value of ['0', '3-1', '1-10000', 'text', '-2', '9999999999999999999999']) test(`rejects invalid ordinals ${value}`, () => assert.throws(() => legacyOrdinals(value)));
test('parses filed and unfiled legacy rows without creating issues', () => {
  const parsed = parseLegacyTables([{ id: 1, body: '| `FIRST` | a requirement | `owner/repo` | [issue](https://github.com/owner/repo/issues/1) | 1, 2 |\n| `SECOND` | another | `owner/repo` | 3-4 |' }]);
  assert.equal(parsed.errors.length, 0);
  assert.equal(parsed.requirements.length, 2);
  assert.equal(parsed.requirements[0].issueReferences.length, 1);
  assert.equal(parsed.requirements[1].issueReferences.length, 0);
});
test('does not silently discard malformed legacy requirement rows', () => assert.equal(parseLegacyTables([{ body: '| `BAD-ROW` | text | `unknown` | 1 |' }]).errors.length, 1));
test('reports contradictory legacy ownership', () => assert.equal(parseLegacyTables([{ body: '| `SAME` | text | `a/b` | 1 |\n| `SAME` | text | `c/d` | 2 |' }]).errors.length, 1));
test('redacts token-shaped and personal data from titles', () => {
  // Generate an unmistakably synthetic value; keep the real secret scan enabled.
  const syntheticToken = `ghp_${'x'.repeat(36)}`;
  assert.match(syntheticToken, /^ghp_[a-z]{36}$/);
  const title = safeTitle(`test@example.invalid ${syntheticToken} +1 (555) 555-0101`);
  assert.equal(title, '[redacted] [redacted] [redacted]');
  assert.ok(!title.includes('@'));
  assert.ok(!title.includes('ghp_'));
  assert.ok(!title.includes('555'));
});
test('uses GET, rejects redirects, and sends authentication only as a header', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: async (url, options) => {
    assert.ok(!url.includes(secret));
    assert.equal(options.method, 'GET');
    assert.equal(options.redirect, 'error');
    assert.equal(options.headers.Authorization, `Bearer ${secret}`);
    assert.equal(options.body, undefined);
    return response([]);
  } });
  await client.pages('/repos/a/b/issues');
});
test('rejects arbitrary hosts before making a request', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: () => { throw new Error('must not call'); } });
  await assert.rejects(client.json('//evil.invalid/path'), /invalid_api_path/);
});
test('paginates until the terminating partial page', async () => {
  let call = 0;
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response(++call === 1 ? Array(100).fill(1) : [2]) });
  assert.equal((await client.pages('/repos/a/b/issues')).length, 101);
  assert.equal(call, 2);
});
test('an exact multiple requires the final empty page', async () => {
  let call = 0;
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response(++call === 1 ? Array(100).fill(1) : []) });
  assert.equal((await client.pages('/repos/a/b/issues')).length, 100);
  assert.equal(call, 2);
});
test('fails closed at a pagination budget', async () => {
  const client = new GitHubReader({ token: secret, maxPages: 1, fetchImpl: async () => response(Array(100).fill(1)) });
  await assert.rejects(client.pages('/repos/a/b/issues'), /pagination_budget_exhausted/);
});
test('fails closed for incomplete GitHub search results', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response({ items: [], incomplete_results: true }) });
  await assert.rejects(client.pages('/search/issues?q=x', 'items'), /github_incomplete_search/);
});
test('fails closed for GitHub search result caps', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response({ items: [], total_count: 1001 }) });
  await assert.rejects(client.pages('/search/issues?q=x', 'items'), /github_search_limit_exceeded/);
});
test('does not treat a 404 as an empty repository', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response({}, 404) });
  await assert.rejects(client.pages('/repos/a/b/issues'), /github_http_404/);
});
test('bounds response bytes', async () => {
  const client = new GitHubReader({ token: secret, responseBytes: 2, fetchImpl: async () => response([1, 2]) });
  await assert.rejects(client.json('/repos/a/b/issues'), /github_response_too_large/);
});
test('bounds total requests', async () => {
  const client = new GitHubReader({ token: secret, maxRequests: 1, fetchImpl: async () => response([]) });
  await client.json('/repos/a/b/issues');
  await assert.rejects(client.json('/repos/a/b/issues'), /request_budget_exhausted/);
});
test('retains access errors and never claims semantic completion', async () => {
  const reader = { requests: 2, pages: async () => { throw new Error('github_http_403'); } };
  const result = await collectCoverageIndex({ reader, now: () => '2026-09-08T00:00:00.000Z' });
  assert.equal(result.readComplete, false);
  assert.equal(result.semanticCoverageVerified, false);
  assert.equal(result.errors.length, 2);
  assert.equal(result.errors[0].code, 'github_http_403');
});
test('an empty inventory is not a semantic coverage claim', async () => {
  const result = await collectCoverageIndex({ reader: { requests: 2, pages: async () => [] } });
  assert.equal(result.semanticCoverageVerified, false);
  assert.equal(result.issues.length, 0);
});

test('rejects dot-segment API traversal before requesting', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: () => { throw new Error('must not call'); } });
  await assert.rejects(client.json('/repos/../user'), /invalid_api_path/);
  await assert.rejects(client.json('/repos/%2e%2e/user'), /invalid_api_path/);
});
test('does not silently accept a short search page below total_count', async () => {
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response({ items: [], total_count: 1 }) });
  await assert.rejects(client.pages('/search/issues?q=x', 'items'), /github_search_count_mismatch/);
});
test('records non-snapshot search changes as an error', async () => {
  let call = 0;
  const client = new GitHubReader({ token: secret, fetchImpl: async () => response(++call === 1 ? { items: Array(100).fill(1), total_count: 101 } : { items: [2, 3], total_count: 102 }) });
  await assert.rejects(client.pages('/search/issues?q=x', 'items'), /github_search_changed_during_pagination/);
});
