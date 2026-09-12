import { createHash } from 'node:crypto';

// Read-only discovery, not semantic implementation verification. No message body,
// issue body, comment body, credential, or file patch is retained in the result.
const API_ORIGIN = 'https://api.github.com';
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const LABEL = 'chat-audit-2026-08-28';
const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

export function githubReference(value) {
  if (typeof value !== 'string') return null;
  let url;
  try { url = new URL(value); } catch { return null; }
  if (url.origin !== 'https://github.com' || url.username || url.password || url.search) return null;
  const match = /^\/([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)\/(issues|pull)\/([1-9][0-9]*)\/?$/.exec(url.pathname);
  if (!match || !Number.isSafeInteger(Number(match[3]))) return null;
  return { repository: match[1], kind: match[2] === 'pull' ? 'pull' : 'issue', number: Number(match[3]), url: `${url.origin}/${match[1]}/${match[2]}/${match[3]}` };
}

export function referencesIn(value) {
  const found = new Map();
  for (const match of String(value || '').matchAll(/https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/(?:issues|pull)\/[1-9][0-9]*/g)) {
    const ref = githubReference(match[0]);
    if (ref) found.set(ref.url, ref);
  }
  return [...found.values()].sort((a, b) => a.url.localeCompare(b.url));
}

export function legacyOrdinals(value) {
  const result = new Set();
  for (const token of String(value).replace(/`/g, '').split(',')) {
    const match = /^\s*([1-9][0-9]*)(?:\s*[-–]\s*([1-9][0-9]*))?\s*$/.exec(token);
    if (!match) throw new Error('invalid_legacy_ordinal');
    const first = Number(match[1]);
    const last = Number(match[2] || first);
    if (!Number.isSafeInteger(last) || first > last || last > 10000 || last - first > 1000) throw new Error('invalid_legacy_range');
    for (let n = first; n <= last; n += 1) result.add(n);
  }
  return [...result].sort((a, b) => a - b);
}

export function safeTitle(value) {
  return String(value || '')
    .replace(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi, '[redacted]')
    .replace(/(?:github_pat_|gh[pousr]_|lin_api_|sk-|SG\.)[A-Za-z0-9_.-]+/g, '[redacted]')
    .replace(/[A-Za-z0-9_+=\/-]{40,}/g, '[redacted]')
    .replace(/\+\d[\d ()-]{7,}\d/g, '[redacted]')
    .replace(/[\u0000-\u001f\u007f]/g, ' ')
    .slice(0, 300);
}

export function parseLegacyTables(comments) {
  if (!Array.isArray(comments)) throw new Error('comments_must_be_array');
  const requirements = new Map();
  const errors = [];
  for (const comment of comments) {
    for (const line of String(comment.body || '').split('\n')) {
      if (!/^\|\s*`[A-Z][A-Z0-9-]+`\s*\|/.test(line)) continue;
      const cells = line.split('|').slice(1, -1).map((cell) => cell.trim());
      const id = cells[0].replace(/`/g, '');
      const repository = (cells[2] || '').replace(/`/g, '');
      if (![4, 5].includes(cells.length) || !REPOSITORY.test(repository)) {
        errors.push({ code: 'invalid_legacy_row', requirement: id });
        continue;
      }
      let ordinals;
      try { ordinals = legacyOrdinals(cells.at(-1)); } catch {
        errors.push({ code: 'invalid_legacy_ordinals', requirement: id });
        continue;
      }
      const row = {
        id,
        description: safeTitle(cells[1]),
        repository,
        legacySnapshot: 'ORESoftware/my-ai#22:2026-08-02--2026-08-28',
        legacyOrdinals: ordinals,
        issueReferences: cells.length === 5 ? referencesIn(cells[3]).filter((ref) => ref.kind === 'issue') : [],
        sourceCommentId: Number(comment.id),
      };
      if (requirements.has(id)) {
        const previous = requirements.get(id);
        if (previous.repository !== row.repository) {
          errors.push({ code: 'conflicting_legacy_repository', requirement: id });
          continue;
        }
        row.legacyOrdinals = [...new Set([...previous.legacyOrdinals, ...row.legacyOrdinals])].sort((a, b) => a - b);
        row.issueReferences = [...new Map([...previous.issueReferences, ...row.issueReferences].map((ref) => [ref.url, ref])).values()];
      }
      requirements.set(id, row);
    }
  }
  return { requirements: [...requirements.values()].sort((a, b) => a.id.localeCompare(b.id)), errors };
}

export class GitHubReader {
  constructor({ token, fetchImpl = globalThis.fetch, maxRequests = 1000, maxPages = 20, responseBytes = MAX_RESPONSE_BYTES }) {
    if (typeof token !== 'string' || !token) throw new Error('github_token_required');
    if (!Number.isInteger(maxRequests) || maxRequests < 1 || maxRequests > 5000) throw new Error('invalid_request_budget');
    if (!Number.isInteger(maxPages) || maxPages < 1 || maxPages > 100) throw new Error('invalid_page_budget');
    this.token = token;
    this.fetchImpl = fetchImpl;
    this.maxRequests = maxRequests;
    this.maxPages = maxPages;
    this.responseBytes = responseBytes;
    this.requests = 0;
  }

  async json(pathname) {
    if (typeof pathname !== 'string' || !/^\/(?:repos\/|search\/issues\?)/.test(pathname) || pathname.startsWith('//') || pathname.includes('#') || pathname.includes('\\')) throw new Error('invalid_api_path');
    const url = new URL(pathname, API_ORIGIN);
    if (url.origin !== API_ORIGIN) throw new Error('invalid_api_origin');
    if (url.pathname !== pathname.split('?')[0] || !/^\/(?:repos\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\/|$)|search\/issues$)/.test(url.pathname)) throw new Error('invalid_api_path');
    if (++this.requests > this.maxRequests) throw new Error('request_budget_exhausted');
    let response;
    try {
      response = await this.fetchImpl(url.href, {
        method: 'GET', redirect: 'error', signal: AbortSignal.timeout(30000),
        headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${this.token}`, 'X-GitHub-Api-Version': '2022-11-28' },
      });
    } catch { throw new Error('github_transport_failed'); }
    if (!response.ok) throw new Error(`github_http_${response.status}`);
    if (!response.body) throw new Error('github_empty_body');
    const reader = response.body.getReader();
    let length = 0;
    const chunks = [];
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        length += value.byteLength;
        if (length > this.responseBytes) { await reader.cancel(); throw new Error('github_response_too_large'); }
        chunks.push(value);
      }
    } finally { reader.releaseLock(); }
    try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { throw new Error('github_invalid_json'); }
  }

  async pages(pathname, field = null) {
    const result = [];
    let expectedTotal = null;
    for (let page = 1; page <= this.maxPages; page += 1) {
      const separator = pathname.includes('?') ? '&' : '?';
      const payload = await this.json(`${pathname}${separator}per_page=100&page=${page}`);
      if (payload?.incomplete_results === true) throw new Error('github_incomplete_search');
      if (pathname.startsWith('/search/')) {
        if (!Number.isSafeInteger(payload.total_count) || payload.total_count < 0) throw new Error('github_invalid_search_count');
        if (payload.total_count > 1000) throw new Error('github_search_limit_exceeded');
        if (expectedTotal !== null && expectedTotal !== payload.total_count) throw new Error('github_search_changed_during_pagination');
        expectedTotal = payload.total_count;
      }
      const items = field ? payload?.[field] : payload;
      if (!Array.isArray(items)) throw new Error('github_invalid_page');
      result.push(...items);
      if (items.length < 100) {
        if (expectedTotal !== null && result.length !== expectedTotal) throw new Error('github_search_count_mismatch');
        return result;
      }
    }
    throw new Error('pagination_budget_exhausted');
  }
}

function issueView(raw) {
  const reference = githubReference(raw.html_url);
  if (!reference || raw.pull_request || reference.kind !== 'issue') throw new Error('invalid_issue_snapshot');
  return {
    ...reference, title: safeTitle(raw.title), state: raw.state,
    createdAt: raw.created_at, updatedAt: raw.updated_at, closedAt: raw.closed_at,
    labels: (raw.labels || []).map((label) => safeTitle(typeof label === 'string' ? label : label.name)),
    bodySha256: createHash('sha256').update(String(raw.body || '')).digest('hex'),
    references: referencesIn(raw.body),
  };
}

export async function collectCoverageIndex(options) {
  const reader = options.reader || new GitHubReader(options);
  const now = options.now || (() => new Date().toISOString());
  const result = { schemaVersion: 1, startedAt: now(), scope: 'existing-August-28-issue-inventory', semanticCoverageVerified: false, readComplete: false, legacyRequirements: [], issues: [], pullRequests: [], errors: [] };
  const capture = async (stage, reference, operation) => {
    try { return await operation(); } catch (error) {
      const code = /^[a-z][a-z0-9_]{1,79}$/.test(error.message) ? error.message : 'unclassified_read_failure';
      result.errors.push({ stage, reference, code });
      return null;
    }
  };
  const comments = await capture('legacy-comments', 'ORESoftware/my-ai#22', () => reader.pages('/repos/ORESoftware/my-ai/issues/22/comments'));
  if (comments) {
    const legacy = parseLegacyTables(comments);
    result.legacyRequirements = legacy.requirements;
    result.errors.push(...legacy.errors.map((error) => ({ stage: 'legacy-table', ...error })));
  }
  const found = await capture('label-index', LABEL, () => reader.pages(`/search/issues?q=${encodeURIComponent(`is:issue label:${LABEL}`)}`, 'items'));
  const issues = new Map();
  for (const raw of found || []) {
    const issue = await capture('issue-normalization', String(raw.number), async () => issueView(raw));
    if (issue) issues.set(issue.url, issue);
  }
  for (const row of result.legacyRequirements) {
    for (const ref of row.issueReferences) {
      if (issues.has(ref.url)) continue;
      const issue = await capture('issue-snapshot', ref.url, async () => issueView(await reader.json(`/repos/${ref.repository}/issues/${ref.number}`)));
      if (issue) issues.set(issue.url, issue);
    }
  }
  const pulls = new Map();
  for (const issue of issues.values()) {
    const timeline = await capture('issue-timeline', issue.url, () => reader.pages(`/repos/${issue.repository}/issues/${issue.number}/timeline`));
    const refs = new Map(issue.references.filter((ref) => ref.kind === 'pull').map((ref) => [ref.url, ref]));
    for (const item of timeline || []) {
      if (item.event !== 'cross-referenced' || !item.source?.issue?.pull_request) continue;
      const ref = githubReference(item.source.issue.html_url);
      if (ref?.kind === 'pull') refs.set(ref.url, ref);
    }
    issue.candidatePullRequests = [...refs.values()].sort((a, b) => a.url.localeCompare(b.url));
    for (const ref of refs.values()) pulls.set(ref.url, ref);
  }
  for (const ref of pulls.values()) {
    const raw = await capture('pull-request', ref.url, () => reader.json(`/repos/${ref.repository}/pulls/${ref.number}`));
    if (!raw) continue;
    const files = await capture('pull-request-files', ref.url, () => reader.pages(`/repos/${ref.repository}/pulls/${ref.number}/files`));
    const checks = /^[0-9a-f]{40}$/.test(raw.head?.sha || '')
      ? await capture('head-checks', ref.url, () => reader.pages(`/repos/${ref.repository}/commits/${raw.head.sha}/check-runs`, 'check_runs')) : null;
    result.pullRequests.push({
      ...ref, title: safeTitle(raw.title), state: raw.state, draft: raw.draft,
      merged: raw.merged, mergedAt: raw.merged_at, updatedAt: raw.updated_at,
      headSha: raw.head?.sha || null, baseSha: raw.base?.sha || null,
      mergeCommitSha: raw.merge_commit_sha || null,
      changedFileCount: raw.changed_files,
      filesComplete: files !== null && files.length === raw.changed_files,
      files: (files || []).map((file) => ({ path: file.filename, status: file.status, additions: file.additions, deletions: file.deletions })),
      checksComplete: checks !== null,
      checks: (checks || []).map((check) => ({ name: safeTitle(check.name), status: check.status, conclusion: check.conclusion, headSha: check.head_sha, id: check.id })),
      semanticReview: 'not-performed-by-collector',
    });
  }
  result.issues = [...issues.values()].sort((a, b) => a.url.localeCompare(b.url));
  result.pullRequests.sort((a, b) => a.url.localeCompare(b.url));
  result.finishedAt = now();
  result.requestCount = reader.requests;
  result.readComplete = result.errors.length === 0 && result.pullRequests.every((pr) => pr.filesComplete && pr.checksComplete);
  return result;
}
