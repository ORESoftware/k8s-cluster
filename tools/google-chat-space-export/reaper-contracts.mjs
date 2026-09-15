import {
  CANDIDATE_KEY_PATTERN,
  COMMIT_REFERENCE_PATTERN,
  LINEAR_IDENTIFIER_PATTERN,
  PR_REFERENCE_PATTERN,
  assertPlainObject,
} from './reaper-utils.mjs';

export const REAPER_SCHEMA_VERSION = 1;
export const MAX_LINEAR_ISSUES = 2;
export const MAX_IMPLEMENTATION_REFERENCES = 32;

export const CANDIDATE_ACTIONS = Object.freeze([
  'create',
  'comment-existing',
  'manual-review',
  'skip-non-actionable',
]);

export const EVIDENCE_DISPOSITIONS = Object.freeze([
  'covered',
  'excluded',
  'quarantined',
]);

export const RECEIPT_DISPOSITIONS = Object.freeze([
  ...EVIDENCE_DISPOSITIONS,
  'gap',
]);

export const MATERIALIZATION_OPERATIONS = Object.freeze([
  'created',
  'updated',
  'reused',
  'excluded',
]);

export const EXCLUSION_REASON_CODES = Object.freeze([
  'non_actionable',
  'private_or_personal',
  'credential_only',
  'duplicate_refinement',
  'invalid_prompt',
  'out_of_scope',
  'privacy_sensitive',
  'unsafe_or_deceptive',
  'context_only',
]);

export const QUARANTINE_REASON_CODES = Object.freeze([
  'sensitive_content',
  'ambiguous_scope',
  'unsafe_automation',
  'requires_human_review',
]);

export const GAP_REASON_CODES = Object.freeze([
  'missing_evidence',
  'missing_implementation_evidence',
]);

export const REAPER_REASON_CODES = Object.freeze([
  ...EXCLUSION_REASON_CODES,
  ...QUARANTINE_REASON_CODES,
  ...GAP_REASON_CODES,
]);

export const CANDIDATE_ACTION_SET = new Set(CANDIDATE_ACTIONS);
export const EVIDENCE_DISPOSITION_SET = new Set(EVIDENCE_DISPOSITIONS);
export const RECEIPT_DISPOSITION_SET = new Set(RECEIPT_DISPOSITIONS);
export const MATERIALIZATION_OPERATION_SET = new Set(MATERIALIZATION_OPERATIONS);
export const EXCLUSION_REASON_SET = new Set(EXCLUSION_REASON_CODES);
export const QUARANTINE_REASON_SET = new Set(QUARANTINE_REASON_CODES);
export const GAP_REASON_SET = new Set(GAP_REASON_CODES);
export const REAPER_REASON_SET = new Set(REAPER_REASON_CODES);

const PLAN_ID_PATTERN = /^google-chat-import-plan:[0-9a-f]{24}$/;
const SUMMARY_ID_PATTERN = /^google-chat-reaper-summary:[0-9a-f]{24}$/;
const RECEIPT_ID_PATTERN = /^google-chat-reconciliation-receipt:[0-9a-f]{24}$/;
const REASON_CODE_PATTERN = /^[a-z][a-z0-9_]{1,63}$/;

const EVIDENCE_KEYS = new Set(['schemaVersion', 'planId', 'entries']);
const EVIDENCE_ENTRY_KEYS = new Set([
  'candidateKey',
  'disposition',
  'linearIssues',
  'pullRequests',
  'defaultBranchCommits',
  'reasonCode',
]);
const SUMMARY_KEYS = new Set([
  'schemaVersion',
  'planId',
  'counts',
  'operations',
  'summaryId',
]);
const SUMMARY_COUNT_KEYS = new Set([
  'candidates',
  'linearCreated',
  'linearUpdated',
  'linearReused',
  'coveredWithImplementation',
  'awaitingImplementation',
  'quarantined',
  'excluded',
]);
const OPERATION_KEYS = new Set([
  'candidateKey',
  'operation',
  'linearIssue',
  'reasonCode',
]);
const RECEIPT_KEYS = new Set([
  'schemaVersion',
  'planId',
  'source',
  'counts',
  'dispositions',
  'receiptId',
]);
const RECEIPT_SOURCE_KEYS = new Set([
  'spaceName',
  'windowStartInclusive',
  'windowEndExclusive',
]);
const RECEIPT_COUNT_KEYS = new Set([
  'scanned',
  'actionable',
  'covered',
  'excluded',
  'quarantined',
  'gaps',
  'candidates',
  'complete',
]);
const RECEIPT_CANDIDATE_COUNT_KEYS = new Set([
  'total',
  'actionable',
  'covered',
  'excluded',
  'quarantined',
  'gaps',
]);
const RECEIPT_DISPOSITION_KEYS = new Set([
  'candidateKey',
  'messageCount',
  'disposition',
  'linearIssues',
  'pullRequests',
  'defaultBranchCommits',
  'reasonCode',
]);

function rejectUnknownKeys(value, allowed, label) {
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) throw new Error(`${label} contains forbidden field ${key}`);
  }
}

function assertSchemaVersion(value, label) {
  if (value !== REAPER_SCHEMA_VERSION) {
    throw new Error(`${label} must be ${REAPER_SCHEMA_VERSION}`);
  }
}

function assertPlanId(value, label) {
  if (typeof value !== 'string' || !PLAN_ID_PATTERN.test(value)) {
    throw new Error(`${label} is invalid`);
  }
}

function assertNonNegativeInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`${label} must be a non-negative safe integer`);
  }
}

function assertPositiveInteger(value, label) {
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new Error(`${label} must be a positive safe integer`);
  }
}

function assertReasonCode(value, allowed, label) {
  if (typeof value !== 'string' || !REASON_CODE_PATTERN.test(value) || !allowed.has(value)) {
    throw new Error(`${label} is not allowed`);
  }
}

function assertUniqueStringArray(value, label, pattern, maximum, { required = false } = {}) {
  if (value === undefined) {
    if (required) throw new Error(`${label} is required`);
    return [];
  }
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  if (value.length > maximum) throw new Error(`${label} may contain at most ${maximum} values`);
  const values = value.map((item, index) => {
    if (typeof item !== 'string' || !pattern.test(item)) {
      throw new Error(`${label}[${index}] is invalid`);
    }
    return item;
  });
  if (new Set(values).size !== values.length) throw new Error(`${label} contains duplicate values`);
  return values;
}

function assertCandidateKey(value, label) {
  if (typeof value !== 'string' || !CANDIDATE_KEY_PATTERN.test(value)) {
    throw new Error(`${label} is invalid`);
  }
}

function assertEvidenceEntry(entry, label) {
  assertPlainObject(entry, label);
  rejectUnknownKeys(entry, EVIDENCE_ENTRY_KEYS, label);
  assertCandidateKey(entry.candidateKey, `${label}.candidateKey`);
  if (!EVIDENCE_DISPOSITION_SET.has(entry.disposition)) {
    throw new Error(`${label}.disposition is invalid`);
  }

  if (entry.disposition === 'covered') {
    const linearIssues = assertUniqueStringArray(
      entry.linearIssues,
      `${label}.linearIssues`,
      LINEAR_IDENTIFIER_PATTERN,
      MAX_LINEAR_ISSUES,
      { required: true },
    );
    if (linearIssues.length < 1) throw new Error(`${label}.linearIssues must not be empty`);
    assertUniqueStringArray(
      entry.pullRequests,
      `${label}.pullRequests`,
      PR_REFERENCE_PATTERN,
      MAX_IMPLEMENTATION_REFERENCES,
      { required: true },
    );
    assertUniqueStringArray(
      entry.defaultBranchCommits,
      `${label}.defaultBranchCommits`,
      COMMIT_REFERENCE_PATTERN,
      MAX_IMPLEMENTATION_REFERENCES,
      { required: true },
    );
    if (entry.reasonCode !== undefined) {
      throw new Error(`${label}.reasonCode is forbidden for covered evidence`);
    }
    return;
  }

  for (const field of ['linearIssues', 'pullRequests', 'defaultBranchCommits']) {
    if (entry[field] !== undefined) {
      throw new Error(`${label}.${field} is forbidden for ${entry.disposition} evidence`);
    }
  }
  assertReasonCode(
    entry.reasonCode,
    entry.disposition === 'excluded' ? EXCLUSION_REASON_SET : QUARANTINE_REASON_SET,
    `${label}.reasonCode`,
  );
}

export function assertCoverageEvidenceContract(value) {
  assertPlainObject(value, 'coverage evidence');
  rejectUnknownKeys(value, EVIDENCE_KEYS, 'coverage evidence');
  assertSchemaVersion(value.schemaVersion, 'coverage evidence.schemaVersion');
  assertPlanId(value.planId, 'coverage evidence.planId');
  if (!Array.isArray(value.entries)) throw new Error('coverage evidence.entries must be an array');
  const seen = new Set();
  value.entries.forEach((entry, index) => {
    assertEvidenceEntry(entry, `coverage evidence.entries[${index}]`);
    if (seen.has(entry.candidateKey)) {
      throw new Error(`coverage evidence contains duplicate candidate ${entry.candidateKey}`);
    }
    seen.add(entry.candidateKey);
  });
  return value;
}

export function assertReaperSummaryContract(value) {
  assertPlainObject(value, 'reaper summary');
  rejectUnknownKeys(value, SUMMARY_KEYS, 'reaper summary');
  assertSchemaVersion(value.schemaVersion, 'reaper summary.schemaVersion');
  assertPlanId(value.planId, 'reaper summary.planId');
  if (typeof value.summaryId !== 'string' || !SUMMARY_ID_PATTERN.test(value.summaryId)) {
    throw new Error('reaper summary.summaryId is invalid');
  }

  assertPlainObject(value.counts, 'reaper summary.counts');
  rejectUnknownKeys(value.counts, SUMMARY_COUNT_KEYS, 'reaper summary.counts');
  for (const key of SUMMARY_COUNT_KEYS) {
    assertNonNegativeInteger(value.counts[key], `reaper summary.counts.${key}`);
  }
  if (
    value.counts.coveredWithImplementation +
      value.counts.awaitingImplementation +
      value.counts.quarantined +
      value.counts.excluded !==
    value.counts.candidates
  ) {
    throw new Error('reaper summary disposition counts do not equal candidates');
  }

  if (!Array.isArray(value.operations)) throw new Error('reaper summary.operations must be an array');
  if (value.operations.length !== value.counts.candidates) {
    throw new Error('reaper summary.operations length does not equal candidates');
  }
  const seen = new Set();
  const operationCounts = new Map(MATERIALIZATION_OPERATIONS.map((operation) => [operation, 0]));
  value.operations.forEach((operation, index) => {
    const label = `reaper summary.operations[${index}]`;
    assertPlainObject(operation, label);
    rejectUnknownKeys(operation, OPERATION_KEYS, label);
    assertCandidateKey(operation.candidateKey, `${label}.candidateKey`);
    if (seen.has(operation.candidateKey)) {
      throw new Error(`reaper summary contains duplicate candidate ${operation.candidateKey}`);
    }
    seen.add(operation.candidateKey);
    if (!MATERIALIZATION_OPERATION_SET.has(operation.operation)) {
      throw new Error(`${label}.operation is invalid`);
    }
    operationCounts.set(operation.operation, operationCounts.get(operation.operation) + 1);

    if (operation.operation === 'excluded') {
      if (operation.linearIssue !== undefined) {
        throw new Error(`${label}.linearIssue is forbidden for excluded operations`);
      }
      if (operation.reasonCode !== undefined) {
        assertReasonCode(operation.reasonCode, EXCLUSION_REASON_SET, `${label}.reasonCode`);
      }
      return;
    }
    if (
      typeof operation.linearIssue !== 'string' ||
      !LINEAR_IDENTIFIER_PATTERN.test(operation.linearIssue)
    ) {
      throw new Error(`${label}.linearIssue is invalid`);
    }
    if (operation.reasonCode !== undefined) {
      throw new Error(`${label}.reasonCode is forbidden for ${operation.operation} operations`);
    }
  });

  const expectedOperationCounts = {
    created: value.counts.linearCreated,
    updated: value.counts.linearUpdated,
    reused: value.counts.linearReused,
    excluded: value.counts.excluded,
  };
  for (const [operation, expected] of Object.entries(expectedOperationCounts)) {
    if (operationCounts.get(operation) !== expected) {
      throw new Error(`reaper summary ${operation} count does not match operations`);
    }
  }
  return value;
}

function assertReceiptCounts(counts) {
  assertPlainObject(counts, 'reconciliation receipt.counts');
  rejectUnknownKeys(counts, RECEIPT_COUNT_KEYS, 'reconciliation receipt.counts');
  for (const key of ['scanned', 'actionable', 'covered', 'excluded', 'quarantined', 'gaps']) {
    assertNonNegativeInteger(counts[key], `reconciliation receipt.counts.${key}`);
  }
  if (typeof counts.complete !== 'boolean') {
    throw new Error('reconciliation receipt.counts.complete must be a boolean');
  }
  assertPlainObject(counts.candidates, 'reconciliation receipt.counts.candidates');
  rejectUnknownKeys(
    counts.candidates,
    RECEIPT_CANDIDATE_COUNT_KEYS,
    'reconciliation receipt.counts.candidates',
  );
  for (const key of RECEIPT_CANDIDATE_COUNT_KEYS) {
    assertNonNegativeInteger(
      counts.candidates[key],
      `reconciliation receipt.counts.candidates.${key}`,
    );
  }
}

function assertReceiptDisposition(entry, label) {
  assertPlainObject(entry, label);
  rejectUnknownKeys(entry, RECEIPT_DISPOSITION_KEYS, label);
  assertCandidateKey(entry.candidateKey, `${label}.candidateKey`);
  assertPositiveInteger(entry.messageCount, `${label}.messageCount`);
  if (!RECEIPT_DISPOSITION_SET.has(entry.disposition)) {
    throw new Error(`${label}.disposition is invalid`);
  }
  const linearIssues = assertUniqueStringArray(
    entry.linearIssues,
    `${label}.linearIssues`,
    LINEAR_IDENTIFIER_PATTERN,
    MAX_LINEAR_ISSUES,
    { required: true },
  );
  const pullRequests = assertUniqueStringArray(
    entry.pullRequests,
    `${label}.pullRequests`,
    PR_REFERENCE_PATTERN,
    MAX_IMPLEMENTATION_REFERENCES,
    { required: true },
  );
  const defaultBranchCommits = assertUniqueStringArray(
    entry.defaultBranchCommits,
    `${label}.defaultBranchCommits`,
    COMMIT_REFERENCE_PATTERN,
    MAX_IMPLEMENTATION_REFERENCES,
    { required: true },
  );

  if (entry.disposition === 'covered') {
    if (linearIssues.length < 1) throw new Error(`${label}.linearIssues must not be empty`);
    if (entry.reasonCode !== undefined) {
      throw new Error(`${label}.reasonCode is forbidden for covered dispositions`);
    }
    return;
  }
  if (entry.disposition === 'gap') {
    assertReasonCode(entry.reasonCode, GAP_REASON_SET, `${label}.reasonCode`);
    return;
  }
  if (linearIssues.length || pullRequests.length || defaultBranchCommits.length) {
    throw new Error(`${label} cannot attach implementation evidence to ${entry.disposition}`);
  }
  assertReasonCode(
    entry.reasonCode,
    entry.disposition === 'excluded' ? EXCLUSION_REASON_SET : QUARANTINE_REASON_SET,
    `${label}.reasonCode`,
  );
}

export function assertReconciliationReceiptContract(value) {
  assertPlainObject(value, 'reconciliation receipt');
  rejectUnknownKeys(value, RECEIPT_KEYS, 'reconciliation receipt');
  assertSchemaVersion(value.schemaVersion, 'reconciliation receipt.schemaVersion');
  assertPlanId(value.planId, 'reconciliation receipt.planId');
  if (typeof value.receiptId !== 'string' || !RECEIPT_ID_PATTERN.test(value.receiptId)) {
    throw new Error('reconciliation receipt.receiptId is invalid');
  }

  assertPlainObject(value.source, 'reconciliation receipt.source');
  rejectUnknownKeys(value.source, RECEIPT_SOURCE_KEYS, 'reconciliation receipt.source');
  for (const key of RECEIPT_SOURCE_KEYS) {
    if (typeof value.source[key] !== 'string' || value.source[key].length === 0) {
      throw new Error(`reconciliation receipt.source.${key} must be a non-empty string`);
    }
  }
  assertReceiptCounts(value.counts);
  if (!Array.isArray(value.dispositions)) {
    throw new Error('reconciliation receipt.dispositions must be an array');
  }
  if (value.dispositions.length !== value.counts.candidates.total) {
    throw new Error('reconciliation receipt disposition count does not match candidate total');
  }

  const seen = new Set();
  const messageCounts = new Map(RECEIPT_DISPOSITIONS.map((disposition) => [disposition, 0]));
  const candidateCounts = new Map(RECEIPT_DISPOSITIONS.map((disposition) => [disposition, 0]));
  value.dispositions.forEach((entry, index) => {
    assertReceiptDisposition(entry, `reconciliation receipt.dispositions[${index}]`);
    if (seen.has(entry.candidateKey)) {
      throw new Error(`reconciliation receipt contains duplicate candidate ${entry.candidateKey}`);
    }
    seen.add(entry.candidateKey);
    messageCounts.set(entry.disposition, messageCounts.get(entry.disposition) + entry.messageCount);
    candidateCounts.set(entry.disposition, candidateCounts.get(entry.disposition) + 1);
  });

  const messageFieldByDisposition = {
    covered: 'covered',
    excluded: 'excluded',
    quarantined: 'quarantined',
    gap: 'gaps',
  };
  for (const [disposition, field] of Object.entries(messageFieldByDisposition)) {
    if (value.counts[field] !== messageCounts.get(disposition)) {
      throw new Error(`reconciliation receipt ${field} message count is inconsistent`);
    }
    if (value.counts.candidates[field] !== candidateCounts.get(disposition)) {
      throw new Error(`reconciliation receipt candidate ${field} count is inconsistent`);
    }
  }
  if (
    value.counts.covered +
      value.counts.excluded +
      value.counts.quarantined +
      value.counts.gaps !==
    value.counts.scanned
  ) {
    throw new Error('reconciliation receipt disposition messages do not equal scanned messages');
  }
  const expectedComplete =
    value.counts.gaps === 0 &&
    value.counts.covered + value.counts.excluded + value.counts.quarantined ===
      value.counts.scanned;
  if (value.counts.complete !== expectedComplete) {
    throw new Error('reconciliation receipt complete flag is inconsistent');
  }
  return value;
}
