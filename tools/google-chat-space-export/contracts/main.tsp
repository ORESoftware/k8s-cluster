import "@typespec/json-schema";
using TypeSpec.JsonSchema;

namespace Ores.GoogleChatReaper;

@id("CandidateAction")
enum CandidateAction {
  create: "create",
  commentExisting: "comment-existing",
  manualReview: "manual-review",
  skipNonActionable: "skip-non-actionable",
}

@id("EvidenceDisposition")
enum EvidenceDisposition {
  covered: "covered",
  excluded: "excluded",
  quarantined: "quarantined",
}

@id("ReceiptDispositionKind")
enum ReceiptDispositionKind {
  covered: "covered",
  excluded: "excluded",
  quarantined: "quarantined",
  gap: "gap",
}

@id("MaterializationOperation")
enum MaterializationOperation {
  created: "created",
  updated: "updated",
  reused: "reused",
  excluded: "excluded",
}

@id("ReaperReasonCode")
enum ReaperReasonCode {
  nonActionable: "non_actionable",
  privateOrPersonal: "private_or_personal",
  credentialOnly: "credential_only",
  duplicateRefinement: "duplicate_refinement",
  invalidPrompt: "invalid_prompt",
  outOfScope: "out_of_scope",
  privacySensitive: "privacy_sensitive",
  unsafeOrDeceptive: "unsafe_or_deceptive",
  contextOnly: "context_only",
  sensitiveContent: "sensitive_content",
  ambiguousScope: "ambiguous_scope",
  unsafeAutomation: "unsafe_automation",
  requiresHumanReview: "requires_human_review",
  missingEvidence: "missing_evidence",
  missingImplementationEvidence: "missing_implementation_evidence",
}

@id("CoverageEvidenceEntry")
model CoverageEvidenceEntry {
  candidateKey: string;
  disposition: EvidenceDisposition;
  linearIssues?: string[];
  pullRequests?: string[];
  defaultBranchCommits?: string[];
  reasonCode?: ReaperReasonCode;
}

@id("CoverageEvidence")
model CoverageEvidence {
  schemaVersion: 1;
  planId: string;
  entries: CoverageEvidenceEntry[];
}

@id("ReaperCounts")
model ReaperCounts {
  @minValue(0)
  candidates: int32;

  @minValue(0)
  linearCreated: int32;

  @minValue(0)
  linearUpdated: int32;

  @minValue(0)
  linearReused: int32;

  @minValue(0)
  coveredWithImplementation: int32;

  @minValue(0)
  awaitingImplementation: int32;

  @minValue(0)
  quarantined: int32;

  @minValue(0)
  excluded: int32;
}

@id("MaterializationRecord")
model MaterializationRecord {
  candidateKey: string;
  operation: MaterializationOperation;
  linearIssue?: string;
  reasonCode?: ReaperReasonCode;
}

@id("ReaperSummary")
model ReaperSummary {
  schemaVersion: 1;
  planId: string;
  counts: ReaperCounts;
  operations: MaterializationRecord[];
  summaryId: string;
}

@id("ReceiptCandidateCounts")
model ReceiptCandidateCounts {
  @minValue(0)
  total: int32;

  @minValue(0)
  actionable: int32;

  @minValue(0)
  covered: int32;

  @minValue(0)
  excluded: int32;

  @minValue(0)
  quarantined: int32;

  @minValue(0)
  gaps: int32;
}

@id("ReconciliationCounts")
model ReconciliationCounts {
  @minValue(0)
  scanned: int32;

  @minValue(0)
  actionable: int32;

  @minValue(0)
  covered: int32;

  @minValue(0)
  excluded: int32;

  @minValue(0)
  quarantined: int32;

  @minValue(0)
  gaps: int32;

  candidates: ReceiptCandidateCounts;
  complete: boolean;
}

@id("ReceiptSource")
model ReceiptSource {
  spaceName: string;
  windowStartInclusive: string;
  windowEndExclusive: string;
}

@id("ReconciliationDisposition")
model ReconciliationDisposition {
  candidateKey: string;

  @minValue(1)
  messageCount: int32;

  disposition: ReceiptDispositionKind;
  linearIssues: string[];
  pullRequests: string[];
  defaultBranchCommits: string[];
  reasonCode?: ReaperReasonCode;
}

@id("ReconciliationReceipt")
model ReconciliationReceipt {
  schemaVersion: 1;
  planId: string;
  source: ReceiptSource;
  counts: ReconciliationCounts;
  dispositions: ReconciliationDisposition[];
  receiptId: string;
}
