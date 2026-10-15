import type { APIRoute } from 'astro';

export const prerender = true;

const defaultRepository = 'sonus-auris/sonus-auris-site.web';
const repository = process.env.GITHUB_REPOSITORY?.trim() || defaultRepository;
const rawCommitSha = process.env.GITHUB_SHA?.trim() || '';
const commitSha = /^[0-9a-f]{40}$/i.test(rawCommitSha)
  ? rawCommitSha.toLowerCase()
  : null;
const rawRefName = process.env.GITHUB_REF_NAME?.trim() || '';
const refName = rawRefName || (commitSha ? 'unknown' : 'local');
const sourceUrl = commitSha
  ? `https://github.com/${repository}/commit/${commitSha}`
  : null;

const deployment = Object.freeze({
  schemaVersion: 1,
  repository,
  commitSha,
  refName,
  sourceUrl,
  buildEnvironment: commitSha ? 'github-actions' : 'local',
});

export const GET: APIRoute = () =>
  new Response(`${JSON.stringify(deployment, null, 2)}\n`, {
    headers: {
      'Content-Type': 'application/json; charset=utf-8',
      'Cache-Control': 'public, max-age=60, must-revalidate',
      'X-Content-Type-Options': 'nosniff',
    },
  });
