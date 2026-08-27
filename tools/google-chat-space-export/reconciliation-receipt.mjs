#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { pathToFileURL } from 'node:url';

import {
  EXPECTED_SPACE_ID,
  EXPECTED_SPACE_NAME,
  PLAN_SCHEMA_VERSION,
  computePlanId,
} from './import-plan.mjs';

export const RECEIPT_SCHEMA_VERSION = 1;

const ROLLING_WINDOW_MILLISECONDS = 15 * 24 * 60 * 60 * 1000;
const PLAN_ID_PATTERN = /^google-chat-import-plan:[0-9a-f]{24}$/;
const CANDIDATE_KEY_PATTERN = new RegExp(
  `^google-chat:${EXPECTED_SPACE_ID}:[0-9a-f]{24}$`,
);
const MAX_IMPLEMENTATION_REFERENCES = 32;

const TOP_LEVEL_KEYS = new Set(['schemaVersion', 'planId', 'entries']);
const ENTRY_KEYS = new Set([
  'candidateKey',
  'disposition',
  'linearIssues',
  'pullRequests',
  'defaultBranchCommits',
  'reasonCode',
]);
const EXCLUSION_REASONS = new Set([
  'non_actionable',
  'private_or_personal',
  'credential_only',
  'duplicate_refinement',
  'invalid_prompt',
  'out_of_scope',
]);
const QUARANTINE_REASONS = new Set([
  'sensitive_content',
  'ambiguous_scope',
  'unsafe_automation',
  'requires_human_review',
]);

function usage() {
  return `Usage:
  node tools/google-chat-space-export/reconciliation-receipt.mjs \\
    --plan <import-plan.json> --evidence <coverage-evidence.json> \\
    [--json <receipt.json>] [--require-complete]

The command validates content-free Linear/GitHub coverage evidence and emits a
machine-readable receipt. It never calls Linear or GitHub and rejects unknown
fields so prompt text and credentials cannot be copied into the receipt.
`;
}

function parseArgs(argv) {
  const options = {
    plan: null,
    evidence: null,
    jsonOutput: null,
    requireComplete: false,
    help: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith('--')) {
        throw new Error(`Missing value for ${arg}`);
      }
      return argv[index];
    };
    switch (arg) {
      case '--plan':
        options.plan = next();
        break;
      case '--evidence':
        options.evidence = next();
        break;
      case '--json':
        options.jsonOutput = next();
        break;
      case '--require-complete':
        options.requireComplete = true;
        break;
      case '--help':
      case '-h':
        options.help = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

function sha256(value) {
  return createHash('sha256').update(String(value)).digest('hex');
}

function stableValue(value) {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, stableValue(value[key])]),
    );
  }
  return value;
}

function stableStringify(value) {
  return JSON.stringify(stableValue(value));
}

function assertPlainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
}

function rejectUnknownKeys(value, allowed, label) {
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${label} contains forbidden field ${key}`);
    }
  }
}

function uniqueStrings(value, label, pattern, maximum = Number.POSITIVE_INFINITY) {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  if (value.length > maximum) throw new Error(`${label} may contain at most ${maximum} values`);
  const normalized = value.map((item, index) => {
    if (typeof item !== 'string' || !pattern.test(item)) {
      throw new Error(`${label}[${index}] is invalid`);
    }
    return item;
  });
  if (new Set(normalized).size !== normalized.length) {
    throw new Error(`${label} contains duplicate values`);
  }
  return [...normalized].sort();
}

function canonicalInstant(value, label) {
  if (typeof value !== 'string') throw new Error(`${label} must be a canonical RFC-3339 instant`);
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp) || new Date(timestamp).toISOString() !== value) {
    throw new Error(`${label} must be a canonical RFC-3339 instant`);
  }
  return timestamp;
}

function validatePlan(plan) {
  assertPlainObject(plan, 'plan');
  if (plan.schemaVersion !== PLAN_SCHEMA_VERSION) {
    throw new Error(`plan.schemaVersion must be ${PLAN_SCHEMA_VERSION}`);
  }
  if (typeof plan.planId !== 'string' || !PLAN_ID_PATTERN.test(plan.planId)) {
    throw new Error('plan.planId is not a canonical content-free identifier');
  }
  assertPlainObject(plan.source, 'plan.source');
  if (
    plan.source.spaceName !== EXPECTED_SPACE_NAME ||
    plan.source.spaceId !== EXPECTED_SPACE_ID
  ) {
    throw new Error('plan.source does not identify the fixed Google Chat space');
  }
  const windowStart = canonicalInstant(
    plan.source.windowStartInclusive,
    'plan.source.windowStartInclusive',
  );
  const windowEnd = canonicalInstant(
    plan.source.windowEndExclusive,
    'plan.source.windowEndExclusive',
  );
  if (windowEnd - windowStart !== ROLLING_WINDOW_MILLISECONDS) {
    throw new Error('plan source window must be exactly 15 days');
  }
  assertPlainObject(plan.stats, 'plan.stats');
  if (!Number.isSafeInteger(plan.stats.plannedMessages) || plan.stats.plannedMessages < 0) {
    throw new Error('plan.stats.plannedMessages must be a non-negative safe integer');
  }
  if (!Array.isArray(plan.candidates)) throw new Error('plan.candidates must be an array');
  const seen = new Set();
  const messageCounts = new Map();
  let plannedMessages = 0;
  let actionableMessages = 0;
  for (const candidate of plan.candidates) {
    assertPlainObject(candidate, 'plan candidate');
    if (
      typeof candidate.candidateKey !== 'string' ||
      !CANDIDATE_KEY_PATTERN.test(candidate.candidateKey)
    ) {
      throw new Error('every plan candidate needs a canonical content-free candidateKey');
    }
    if (seen.has(candidate.candidateKey)) {
      throw new Error(`duplicate plan candidateKey ${candidate.candidateKey}`);
    }
    seen.add(candidate.candidateKey);
    if (
      !['create', 'comment-existing', 'manual-review', 'skip-non-actionable'].includes(
        candidate.action,
      )
    ) {
      throw new Error(`unsupported plan action for ${candidate.candidateKey}`);
    }
    if (!Number.isSafeInteger(candidate.messageCount) || candidate.messageCount < 1) {
      throw new Error(`plan candidate ${candidate.candidateKey} needs a positive messageCount`);
    }
    messageCounts.set(candidate.candidateKey, candidate.messageCount);
    plannedMessages += candidate.messageCount;
    if (candidate.action !== 'skip-non-actionable') actionableMessages += candidate.messageCount;
  }
  if (plannedMessages !== plan.stats.plannedMessages) {
    throw new Error('candidate message counts do not equal plan.stats.plannedMessages');
  }
  const expectedPlanId = computePlanId({
    spaceName: plan.source.spaceName,
    windowStartInclusive: plan.source.windowStartInclusive,
    windowEndExclusive: plan.source.windowEndExclusive,
    candidates: plan.candidates,
  });
  if (plan.planId !== expectedPlanId) {
    throw new Error('plan.planId does not match the exact window and candidates');
  }
  return { messageCounts, plannedMessages, actionableMessages };
}

function normalizeEvidence(evidence, planId, candidateKeys) {
  assertPlainObject(evidence, 'evidence');
  rejectUnknownKeys(evidence, TOP_LEVEL_KEYS, 'evidence');
  if (evidence.schemaVersion !== RECEIPT_SCHEMA_VERSION) {
    throw new Error(`evidence.schemaVersion must be ${RECEIPT_SCHEMA_VERSION}`);
  }
  if (evidence.planId !== planId) throw new Error('evidence.planId does not match the plan');
  if (!Array.isArray(evidence.entries)) throw new Error('evidence.entries must be an array');

  const entries = new Map();
  for (const [index, raw] of evidence.entries.entries()) {
    const label = `evidence.entries[${index}]`;
    assertPlainObject(raw, label);
    rejectUnknownKeys(raw, ENTRY_KEYS, label);
    if (typeof raw.candidateKey !== 'string' || !candidateKeys.has(raw.candidateKey)) {
      throw new Error(`${label}.candidateKey does not identify a plan candidate`);
    }
    if (entries.has(raw.candidateKey)) {
      throw new Error(`duplicate evidence entry for ${raw.candidateKey}`);
    }
    if (!['covered', 'excluded', 'quarantined'].includes(raw.disposition)) {
      throw new Error(`${label}.disposition is invalid`);
    }

    const linearIssues = uniqueStrings(
      raw.linearIssues,
      `${label}.linearIssues`,
      /^[A-Z][A-Z0-9]+-[1-9][0-9]*$/,
      2,
    );
    const pullRequests = uniqueStrings(
      raw.pullRequests,
      `${label}.pullRequests`,
      /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+#[1-9][0-9]*$/,
      MAX_IMPLEMENTATION_REFERENCES,
    );
    const defaultBranchCommits = uniqueStrings(
      raw.defaultBranchCommits,
      `${label}.defaultBranchCommits`,
      /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+@[0-9a-f]{40}$/,
      MAX_IMPLEMENTATION_REFERENCES,
    );
    const reasonCode = raw.reasonCode;
    if (reasonCode !== undefined && (typeof reasonCode !== 'string' || !/^[a-z][a-z0-9_]{1,63}$/.test(reasonCode))) {
      throw new Error(`${label}.reasonCode is invalid`);
    }

    if (raw.disposition === 'covered') {
      if (reasonCode !== undefined) throw new Error(`${label}.reasonCode is not allowed for covered evidence`);
      if (linearIssues.length === 0) throw new Error(`${label} needs one or two Linear issues`);
    } else {
      if (linearIssues.length || pullRequests.length || defaultBranchCommits.length) {
        throw new Error(`${label} cannot attach implementation evidence to ${raw.disposition} content`);
      }
      const allowedReasons = raw.disposition === 'excluded' ? EXCLUSION_REASONS : QUARANTINE_REASONS;
      if (!reasonCode || !allowedReasons.has(reasonCode)) {
        throw new Error(`${label}.reasonCode is not allowed for ${raw.disposition}`);
      }
    }

    entries.set(raw.candidateKey, {
      candidateKey: raw.candidateKey,
      disposition: raw.disposition,
      linearIssues,
      pullRequests,
      defaultBranchCommits,
      ...(reasonCode ? { reasonCode } : {}),
    });
  }
  return entries;
}

function gap(candidateKey, reasonCode, messageCount, partial = {}) {
  return {
    candidateKey,
    messageCount,
    disposition: 'gap',
    linearIssues: partial.linearIssues || [],
    pullRequests: partial.pullRequests || [],
    defaultBranchCommits: partial.defaultBranchCommits || [],
    reasonCode,
  };
}

export function buildReconciliationReceipt(plan, evidence) {
  const validatedPlan = validatePlan(plan);
  const candidateKeys = new Set(plan.candidates.map((candidate) => candidate.candidateKey));
  const entries = normalizeEvidence(evidence, plan.planId, candidateKeys);
  const dispositions = [];

  for (const candidate of plan.candidates) {
    const supplied = entries.get(candidate.candidateKey);
    const messageCount = validatedPlan.messageCounts.get(candidate.candidateKey);
    if (candidate.action === 'skip-non-actionable') {
      if (supplied && !(supplied.disposition === 'excluded' && supplied.reasonCode === 'non_actionable')) {
        throw new Error(`${candidate.candidateKey} is non-actionable and may only be excluded as non_actionable`);
      }
      dispositions.push(
        supplied
          ? { ...supplied, messageCount }
          : {
              candidateKey: candidate.candidateKey,
              messageCount,
              disposition: 'excluded',
              linearIssues: [],
              pullRequests: [],
              defaultBranchCommits: [],
              reasonCode: 'non_actionable',
            },
      );
      continue;
    }

    if (!supplied) {
      dispositions.push(gap(candidate.candidateKey, 'missing_evidence', messageCount));
      continue;
    }
    if (supplied.disposition !== 'covered') {
      dispositions.push({ ...supplied, messageCount });
      continue;
    }
    if (supplied.pullRequests.length === 0 && supplied.defaultBranchCommits.length === 0) {
      dispositions.push(
        gap(candidate.candidateKey, 'missing_implementation_evidence', messageCount, supplied),
      );
      continue;
    }
    dispositions.push({ ...supplied, messageCount });
  }

  dispositions.sort((left, right) => left.candidateKey.localeCompare(right.candidateKey));
  const messageTotal = (disposition) =>
    dispositions
      .filter((entry) => entry.disposition === disposition)
      .reduce((sum, entry) => sum + entry.messageCount, 0);
  const counts = {
    scanned: validatedPlan.plannedMessages,
    actionable: validatedPlan.actionableMessages,
    covered: messageTotal('covered'),
    excluded: messageTotal('excluded'),
    quarantined: messageTotal('quarantined'),
    gaps: messageTotal('gap'),
    candidates: {
      total: dispositions.length,
      actionable: plan.candidates.filter(
        (candidate) => candidate.action !== 'skip-non-actionable',
      ).length,
      covered: dispositions.filter((entry) => entry.disposition === 'covered').length,
      excluded: dispositions.filter((entry) => entry.disposition === 'excluded').length,
      quarantined: dispositions.filter((entry) => entry.disposition === 'quarantined').length,
      gaps: dispositions.filter((entry) => entry.disposition === 'gap').length,
    },
  };
  counts.complete =
    counts.gaps === 0 &&
    counts.covered + counts.excluded + counts.quarantined === counts.scanned;

  const receiptCore = {
    schemaVersion: RECEIPT_SCHEMA_VERSION,
    planId: plan.planId,
    source: {
      spaceName: plan.source?.spaceName || null,
      windowStartInclusive: plan.source?.windowStartInclusive || null,
      windowEndExclusive: plan.source?.windowEndExclusive || null,
    },
    counts,
    dispositions,
  };
  return {
    ...receiptCore,
    receiptId: `google-chat-reconciliation-receipt:${sha256(stableStringify(receiptCore)).slice(0, 24)}`,
  };
}

async function readJson(pathname) {
  try {
    return JSON.parse(await fs.readFile(pathname, 'utf8'));
  } catch (error) {
    throw new Error(`Could not read JSON ${pathname}: ${error.message}`);
  }
}

async function writeJson(destination, value) {
  await fs.mkdir(path.dirname(path.resolve(destination)), { recursive: true });
  await fs.writeFile(destination, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    process.stdout.write(usage());
    return;
  }
  if (!options.plan || !options.evidence) throw new Error(`--plan and --evidence are required.\n\n${usage()}`);

  const receipt = buildReconciliationReceipt(
    await readJson(options.plan),
    await readJson(options.evidence),
  );
  if (options.jsonOutput) await writeJson(options.jsonOutput, receipt);
  else process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);

  if (options.requireComplete && !receipt.counts.complete) {
    process.exitCode = 2;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  main().catch((error) => {
    console.error(`reconciliation receipt failed: ${error.message}`);
    process.exitCode = 1;
  });
}
