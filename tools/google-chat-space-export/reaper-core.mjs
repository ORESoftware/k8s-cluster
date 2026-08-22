import {
  buildContentFreeLinearSection,
  titleNeedsReview,
} from './reaper-provenance.mjs';
import {
  CANDIDATE_KEY_PATTERN,
  COMMIT_REFERENCE_PATTERN,
  LINEAR_IDENTIFIER_PATTERN,
  PR_REFERENCE_PATTERN,
  assertPlainObject,
  candidateMarker,
  safeIssueTitle,
  sha256,
  stableStringify,
  uniqueSorted,
} from './reaper-utils.mjs';

const DEFAULT_MAX_CREATES = 25;
const MAX_EVIDENCE_REFERENCES = 16;

function candidateExistingIdentifiers(candidate) {
  const identifiers = [];
  for (const item of candidate.exactExistingIssues || []) {
    if (LINEAR_IDENTIFIER_PATTERN.test(item.identifier || '')) identifiers.push(item.identifier);
  }
  return uniqueSorted(identifiers).slice(0, 2);
}

function reviewCandidate(candidate) {
  return candidate.action === 'manual-review' || titleNeedsReview(safeIssueTitle(candidate.title));
}

async function ensureLinearIssue(candidate, context, review) {
  const existingIdentifiers = candidateExistingIdentifiers(candidate);
  let issue = null;
  let reused = false;
  for (const identifier of existingIdentifiers) {
    issue = await context.linear.getIssue(identifier);
    if (issue) { reused = true; break; }
  }
  if (!issue) {
    const title = safeIssueTitle(candidate.title, review ? '[Google Chat review] ' : '[Google Chat] ');
    const matches = await context.linear.findByCandidate(candidate.candidateKey, title);
    const markerMatches = matches.filter((item) => item.description.includes(candidateMarker(candidate.candidateKey)));
    if (markerMatches.length > 1) {
      throw new Error(`Duplicate Linear issues contain candidate ${candidate.candidateKey}`);
    }
    if (markerMatches.length === 1) {
      issue = markerMatches[0];
      reused = true;
    } else {
      const exactTitleMatches = matches.filter((item) => item.title.toLocaleLowerCase('en-US') === title.toLocaleLowerCase('en-US'));
      if (exactTitleMatches.length > 1) {
        throw new Error(`Ambiguous exact-title Linear matches for ${candidate.candidateKey}`);
      }
      if (exactTitleMatches.length === 1) {
        issue = exactTitleMatches[0];
        reused = true;
      }
    }
  }
  const body = buildContentFreeLinearSection(candidate, { review });
  if (!issue) {
    if (context.created >= context.maxCreates) {
      throw new Error(`Linear create circuit breaker reached (${context.maxCreates})`);
    }
    issue = await context.linear.createIssue({
      title: safeIssueTitle(candidate.title, review ? '[Google Chat review] ' : '[Google Chat] '),
      description: body,
    });
    context.created += 1;
    return { issue, operation: 'created' };
  }
  if (!issue.description.includes(candidateMarker(candidate.candidateKey))) {
    issue = await context.linear.appendSection(issue, { candidateKey: candidate.candidateKey, body });
    context.updated += 1;
    return { issue, operation: 'updated' };
  }
  if (reused) context.reused += 1;
  return { issue, operation: 'reused' };
}

function validatePlan(plan) {
  assertPlainObject(plan, 'plan');
  if (typeof plan.planId !== 'string' || !/^google-chat-import-plan:[0-9a-f]{24}$/.test(plan.planId)) {
    throw new Error('plan.planId is invalid');
  }
  if (!Array.isArray(plan.candidates)) throw new Error('plan.candidates must be an array');
  const seen = new Set();
  for (const candidate of plan.candidates) {
    assertPlainObject(candidate, 'plan candidate');
    if (!CANDIDATE_KEY_PATTERN.test(candidate.candidateKey || '')) {
      throw new Error('plan candidate has an invalid candidateKey');
    }
    if (seen.has(candidate.candidateKey)) throw new Error(`duplicate candidate ${candidate.candidateKey}`);
    seen.add(candidate.candidateKey);
    if (!['create', 'comment-existing', 'manual-review', 'skip-non-actionable'].includes(candidate.action)) {
      throw new Error(`unsupported action ${candidate.action}`);
    }
  }
}

export async function materializePlan(plan, dependencies, options = {}) {
  validatePlan(plan);
  const context = {
    linear: dependencies.linear,
    github: dependencies.github,
    maxCreates: options.maxCreates ?? DEFAULT_MAX_CREATES,
    created: 0,
    updated: 0,
    reused: 0,
  };
  const entries = [];
  const operations = [];
  for (const candidate of [...plan.candidates].sort((a, b) => a.candidateKey.localeCompare(b.candidateKey))) {
    if (candidate.action === 'skip-non-actionable') {
      entries.push({
        candidateKey: candidate.candidateKey,
        disposition: 'excluded',
        reasonCode: 'non_actionable',
      });
      operations.push({ candidateKey: candidate.candidateKey, operation: 'excluded' });
      continue;
    }

    const review = reviewCandidate(candidate);
    const { issue, operation } = await ensureLinearIssue(candidate, context, review);
    operations.push({ candidateKey: candidate.candidateKey, operation, linearIssue: issue.identifier });
    if (review) {
      entries.push({
        candidateKey: candidate.candidateKey,
        disposition: 'quarantined',
        reasonCode: 'requires_human_review',
      });
      continue;
    }

    const issueIdentifiers = [issue.identifier];
    const evidence = await dependencies.github.findEvidence({ issueIdentifiers, candidate });
    entries.push({
      candidateKey: candidate.candidateKey,
      disposition: 'covered',
      linearIssues: issueIdentifiers,
      pullRequests: uniqueSorted(evidence.pullRequests || []).filter((item) => PR_REFERENCE_PATTERN.test(item)).slice(0, MAX_EVIDENCE_REFERENCES),
      defaultBranchCommits: uniqueSorted(evidence.defaultBranchCommits || []).filter((item) => COMMIT_REFERENCE_PATTERN.test(item)).slice(0, MAX_EVIDENCE_REFERENCES),
    });
  }

  const evidence = { schemaVersion: 1, planId: plan.planId, entries };
  const coverageCounts = {
    coveredWithImplementation: entries.filter((entry) => entry.disposition === 'covered' && (entry.pullRequests.length || entry.defaultBranchCommits.length)).length,
    awaitingImplementation: entries.filter((entry) => entry.disposition === 'covered' && !(entry.pullRequests.length || entry.defaultBranchCommits.length)).length,
    quarantined: entries.filter((entry) => entry.disposition === 'quarantined').length,
    excluded: entries.filter((entry) => entry.disposition === 'excluded').length,
  };
  const summaryCore = {
    schemaVersion: 1,
    planId: plan.planId,
    counts: {
      candidates: entries.length,
      linearCreated: context.created,
      linearUpdated: context.updated,
      linearReused: context.reused,
      ...coverageCounts,
    },
    operations,
  };
  const summary = {
    ...summaryCore,
    summaryId: `google-chat-reaper-summary:${sha256(stableStringify(summaryCore)).slice(0, 24)}`,
  };
  return { evidence, summary };
}
