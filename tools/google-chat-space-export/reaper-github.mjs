import { fetchJsonWithRetry } from './reaper-http.mjs';
import {
  COMMIT_REFERENCE_PATTERN,
  GITHUB_OWNER_PATTERN,
  GITHUB_REPOSITORY_PATTERN,
  PR_REFERENCE_PATTERN,
  uniqueSorted,
} from './reaper-utils.mjs';

const GITHUB_ENDPOINT = 'https://api.github.com';
const MAX_EVIDENCE_REFERENCES = 16;

function parseRepoFromApiUrl(repositoryUrl) {
  const match = String(repositoryUrl || '').match(/\/repos\/([^/]+)\/([^/]+)$/);
  return match ? `${match[1]}/${match[2]}` : null;
}

function ownersForCandidate(candidate, fallbackOwners) {
  const owners = new Set(fallbackOwners.filter((item) => GITHUB_OWNER_PATTERN.test(item)));
  for (const organization of candidate.githubReferences?.organizations || []) {
    if (GITHUB_OWNER_PATTERN.test(organization)) owners.add(organization);
  }
  for (const repository of candidate.githubReferences?.repositories || []) {
    if (!GITHUB_REPOSITORY_PATTERN.test(repository)) continue;
    const owner = String(repository).split('/')[0];
    if (owner) owners.add(owner);
  }
  return owners;
}

export function createGitHubClient({ token, allowedOwners, fetchImpl } = {}) {
  if (!token) {
    return {
      async findEvidence() { return { pullRequests: [], defaultBranchCommits: [] }; },
    };
  }
  const headers = {
    accept: 'application/vnd.github+json',
    authorization: `Bearer ${token}`,
    'x-github-api-version': '2022-11-28',
    'user-agent': 'google-chat-reaper/1',
  };
  async function github(pathname, extra = {}) {
    return fetchJsonWithRetry(`${GITHUB_ENDPOINT}${pathname}`, {
      ...extra,
      headers: { ...headers, ...(extra.headers || {}) },
    }, { fetchImpl });
  }

  async function searchPullRequests(issueIdentifiers, candidate) {
    const owners = ownersForCandidate(candidate, allowedOwners);
    const repositories = uniqueSorted(candidate.githubReferences?.repositories || [])
      .filter((item) => GITHUB_REPOSITORY_PATTERN.test(item));
    const scopes = repositories.length
      ? repositories.slice(0, 8).map((repository) => `repo:${repository}`)
      : [...owners].slice(0, 8).map((owner) => `org:${owner}`);
    const refs = new Set();
    for (const identifier of issueIdentifiers) {
      for (const scope of scopes.length ? scopes : ['']) {
        const query = [`"${identifier}"`, 'is:pr', 'in:title,body', scope].filter(Boolean).join(' ');
        const result = await github(`/search/issues?q=${encodeURIComponent(query)}&per_page=20&sort=updated&order=desc`);
        for (const item of result.items || []) {
          const repository = parseRepoFromApiUrl(item.repository_url);
          if (!repository || !owners.has(repository.split('/')[0])) continue;
          const pr = await github(new URL(item.pull_request.url).pathname);
          if (pr.state !== 'open' && !pr.merged_at) continue;
          const ref = `${repository}#${item.number}`;
          if (PR_REFERENCE_PATTERN.test(ref)) refs.add(ref);
          if (refs.size >= MAX_EVIDENCE_REFERENCES) return [...refs].sort();
        }
      }
    }
    return [...refs].sort();
  }

  async function commitIsOnDefaultBranch(repository, sha) {
    const repo = await github(`/repos/${repository}`);
    const comparison = await github(`/repos/${repository}/compare/${sha}...${encodeURIComponent(repo.default_branch)}`);
    return comparison.status === 'ahead' || comparison.status === 'identical';
  }

  async function searchCommits(issueIdentifiers, candidate) {
    const repositories = uniqueSorted(candidate.githubReferences?.repositories || [])
      .filter((item) => GITHUB_REPOSITORY_PATTERN.test(item))
      .slice(0, 8);
    if (!repositories.length) return [];
    const refs = new Set();
    for (const identifier of issueIdentifiers) {
      for (const repository of repositories) {
        const query = `"${identifier}" repo:${repository}`;
        const result = await github(
          `/search/commits?q=${encodeURIComponent(query)}&per_page=10&sort=committer-date&order=desc`,
          { headers: { accept: 'application/vnd.github+json' } },
        );
        for (const item of result.items || []) {
          if (!/^[0-9a-f]{40}$/.test(item.sha || '')) continue;
          if (!(await commitIsOnDefaultBranch(repository, item.sha))) continue;
          const ref = `${repository}@${item.sha}`;
          if (COMMIT_REFERENCE_PATTERN.test(ref)) refs.add(ref);
          if (refs.size >= MAX_EVIDENCE_REFERENCES) return [...refs].sort();
        }
      }
    }
    return [...refs].sort();
  }

  async function findEvidence({ issueIdentifiers, candidate }) {
    const pullRequests = await searchPullRequests(issueIdentifiers, candidate);
    const defaultBranchCommits = pullRequests.length
      ? []
      : await searchCommits(issueIdentifiers, candidate);
    return { pullRequests, defaultBranchCommits };
  }

  return { findEvidence };
}
