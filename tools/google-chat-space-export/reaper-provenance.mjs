import {
  CANDIDATE_KEY_PATTERN,
  GITHUB_OWNER_PATTERN,
  GITHUB_REPOSITORY_PATTERN,
  assertPlainObject,
  candidateMarker,
  canonicalInstant,
  sourceKeyDigest,
  uniqueSorted,
} from './reaper-utils.mjs';

export function buildContentFreeLinearSection(candidate, { review = false } = {}) {
  assertPlainObject(candidate, 'candidate');
  if (!CANDIDATE_KEY_PATTERN.test(candidate.candidateKey || '')) {
    throw new Error('candidate.candidateKey is invalid');
  }
  const sourceDigests = uniqueSorted((candidate.sourceKeys || []).map(sourceKeyDigest)).slice(0, 64);
  const repositories = uniqueSorted(candidate.githubReferences?.repositories || [])
    .filter((item) => GITHUB_REPOSITORY_PATTERN.test(item))
    .slice(0, 32);
  const organizations = uniqueSorted(candidate.githubReferences?.organizations || [])
    .filter((item) => GITHUB_OWNER_PATTERN.test(item))
    .slice(0, 32);
  const firstCreateTime = candidate.firstCreateTime
    ? canonicalInstant(candidate.firstCreateTime, 'candidate.firstCreateTime')
    : null;
  const lastCreateTime = candidate.lastCreateTime
    ? canonicalInstant(candidate.lastCreateTime, 'candidate.lastCreateTime')
    : null;
  const lines = [
    candidateMarker(candidate.candidateKey),
    '',
    '## Google Chat reaper provenance',
    '',
    `- Candidate: \`${candidate.candidateKey}\``,
    `- Reaper disposition: \`${review ? 'requires-human-review' : 'implementation-required'}\``,
    `- Message count: ${Number(candidate.messageCount || 0)}`,
    `- Substantive message count: ${Number(candidate.substantiveMessageCount || 0)}`,
    `- First message time: ${firstCreateTime || 'unknown'}`,
    `- Last message time: ${lastCreateTime || 'unknown'}`,
    `- Source identifiers: ${sourceDigests.length} SHA-256 digests`,
    `- Referenced repositories: ${repositories.length ? repositories.map((item) => `\`${item}\``).join(', ') : 'none'}`,
    `- Referenced organizations: ${organizations.length ? organizations.map((item) => `\`${item}\``).join(', ') : 'none'}`,
    '',
    'The reaper intentionally omits message bodies, sender identities, credentials, contact values, and secret-bearing URLs. Completion requires independently verified GitHub implementation evidence; a planning ticket alone is not completion evidence.',
  ];
  if (sourceDigests.length) {
    lines.push('', '<details>', '<summary>Content-free source digests</summary>', '');
    for (const digest of sourceDigests) lines.push(`- \`${digest}\``);
    lines.push('', '</details>');
  }
  return lines.join('\n');
}

export function titleNeedsReview(title) {
  return /\[REDACTED_(?:SECRET|EMAIL|PHONE)\]/.test(title) || /(?:password|private key|api key|access token|credential)/i.test(title);
}

export function normalizeIssue(issue) {
  return {
    id: String(issue.id),
    identifier: String(issue.identifier),
    title: String(issue.title || ''),
    description: String(issue.description || ''),
    url: issue.url || null,
    state: issue.state ? { name: issue.state.name, type: issue.state.type } : null,
    project: issue.project ? { name: issue.project.name } : null,
    attachments: (issue.attachments?.nodes || issue.attachments || []).map((attachment) => ({
      title: String(attachment.title || ''),
      url: String(attachment.url || ''),
    })),
  };
}
