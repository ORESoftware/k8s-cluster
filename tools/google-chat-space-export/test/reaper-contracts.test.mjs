import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import {
  CANDIDATE_ACTIONS,
  EVIDENCE_DISPOSITIONS,
  MATERIALIZATION_OPERATIONS,
  REAPER_REASON_CODES,
  RECEIPT_DISPOSITIONS,
  assertCoverageEvidenceContract,
  assertReaperSummaryContract,
  assertReconciliationReceiptContract,
} from '../reaper-contracts.mjs';

const CANDIDATE_A = 'google-chat:AAQAoHKdzvI:aaaaaaaaaaaaaaaaaaaaaaaa';
const CANDIDATE_B = 'google-chat:AAQAoHKdzvI:bbbbbbbbbbbbbbbbbbbbbbbb';
const PLAN_ID = 'google-chat-import-plan:cccccccccccccccccccccccc';

async function authoredDefinitions() {
  const pathname = new URL('../contracts/authored.schema.json', import.meta.url);
  const document = JSON.parse(await readFile(pathname, 'utf8'));
  return document.$defs;
}

function validEvidence() {
  return {
    schemaVersion: 1,
    planId: PLAN_ID,
    entries: [
      {
        candidateKey: CANDIDATE_A,
        disposition: 'covered',
        linearIssues: ['DEN-3473'],
        pullRequests: ['ORESoftware/k8s-cluster#1539'],
        defaultBranchCommits: [],
      },
      {
        candidateKey: CANDIDATE_B,
        disposition: 'excluded',
        reasonCode: 'privacy_sensitive',
      },
    ],
  };
}

function validSummary() {
  return {
    schemaVersion: 1,
    planId: PLAN_ID,
    counts: {
      candidates: 2,
      linearCreated: 1,
      linearUpdated: 0,
      linearReused: 0,
      coveredWithImplementation: 0,
      awaitingImplementation: 0,
      quarantined: 1,
      excluded: 1,
    },
    operations: [
      {
        candidateKey: CANDIDATE_A,
        operation: 'created',
        linearIssue: 'DEN-3473',
      },
      {
        candidateKey: CANDIDATE_B,
        operation: 'excluded',
        reasonCode: 'context_only',
      },
    ],
    summaryId: 'google-chat-reaper-summary:dddddddddddddddddddddddd',
  };
}

function validReceipt() {
  return {
    schemaVersion: 1,
    planId: PLAN_ID,
    source: {
      spaceName: 'spaces/AAQAoHKdzvI',
      windowStartInclusive: '2026-08-24T00:00:00.000Z',
      windowEndExclusive: '2026-09-08T00:00:00.000Z',
    },
    counts: {
      scanned: 2,
      actionable: 2,
      covered: 0,
      excluded: 1,
      quarantined: 1,
      gaps: 0,
      candidates: {
        total: 2,
        actionable: 2,
        covered: 0,
        excluded: 1,
        quarantined: 1,
        gaps: 0,
      },
      complete: true,
    },
    dispositions: [
      {
        candidateKey: CANDIDATE_A,
        messageCount: 1,
        disposition: 'quarantined',
        linearIssues: [],
        pullRequests: [],
        defaultBranchCommits: [],
        reasonCode: 'requires_human_review',
      },
      {
        candidateKey: CANDIDATE_B,
        messageCount: 1,
        disposition: 'excluded',
        linearIssues: [],
        pullRequests: [],
        defaultBranchCommits: [],
        reasonCode: 'unsafe_or_deceptive',
      },
    ],
    receiptId: 'google-chat-reconciliation-receipt:eeeeeeeeeeeeeeeeeeeeeeee',
  };
}

test('authored JSON Schema vocabularies equal the runtime contract vocabularies', async () => {
  const defs = await authoredDefinitions();
  const cases = [
    ['CandidateAction', CANDIDATE_ACTIONS],
    ['EvidenceDisposition', EVIDENCE_DISPOSITIONS],
    ['ReceiptDispositionKind', RECEIPT_DISPOSITIONS],
    ['MaterializationOperation', MATERIALIZATION_OPERATIONS],
    ['ReaperReasonCode', REAPER_REASON_CODES],
  ];
  for (const [name, runtimeValues] of cases) {
    assert.deepEqual(
      [...defs[name].enum].sort(),
      [...runtimeValues].sort(),
      `${name} drifted between authored JSON Schema and runtime code`,
    );
  }
});

test('runtime validators accept the canonical evidence, summary, and receipt shapes', () => {
  assert.equal(assertCoverageEvidenceContract(validEvidence()).schemaVersion, 1);
  assert.equal(assertReaperSummaryContract(validSummary()).counts.candidates, 2);
  assert.equal(assertReconciliationReceiptContract(validReceipt()).counts.complete, true);
});

test('new pre-mutation exclusion reasons cross the receipt boundary', () => {
  for (const reasonCode of ['privacy_sensitive', 'unsafe_or_deceptive', 'context_only']) {
    const evidence = validEvidence();
    evidence.entries[1].reasonCode = reasonCode;
    assert.doesNotThrow(() => assertCoverageEvidenceContract(evidence));

    const receipt = validReceipt();
    receipt.dispositions[1].reasonCode = reasonCode;
    assert.doesNotThrow(() => assertReconciliationReceiptContract(receipt));
  }
});

test('runtime validators reject freeform fields and conditional evidence leakage', () => {
  const freeform = validEvidence();
  freeform.entries[0].promptText = 'must never cross the content-free boundary';
  assert.throws(
    () => assertCoverageEvidenceContract(freeform),
    /forbidden field promptText/,
  );

  const leaked = validEvidence();
  leaked.entries[1].linearIssues = [];
  assert.throws(
    () => assertCoverageEvidenceContract(leaked),
    /linearIssues is forbidden for excluded evidence/,
  );
});

test('runtime validators reject inconsistent counters and operation evidence', () => {
  const summary = validSummary();
  summary.counts.linearCreated = 0;
  assert.throws(
    () => assertReaperSummaryContract(summary),
    /created count does not match operations/,
  );

  const receipt = validReceipt();
  receipt.counts.excluded = 0;
  assert.throws(
    () => assertReconciliationReceiptContract(receipt),
    /excluded message count is inconsistent/,
  );
});

test('runtime validators reject unknown reason codes and false completion claims', () => {
  const evidence = validEvidence();
  evidence.entries[1].reasonCode = 'invented_reason';
  assert.throws(
    () => assertCoverageEvidenceContract(evidence),
    /reasonCode is not allowed/,
  );

  const receipt = validReceipt();
  receipt.counts.complete = false;
  assert.throws(
    () => assertReconciliationReceiptContract(receipt),
    /complete flag is inconsistent/,
  );
});
