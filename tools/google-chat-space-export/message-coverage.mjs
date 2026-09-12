import { createHash, createHmac } from 'node:crypto';

// Internal audit evidence, not a product wire authority or a network client.
// Inputs come from the trusted export/sanitizer; raw text never enters this API.
const HEX = /^[0-9a-f]{64}$/;
const SHA = /^[0-9a-f]{40}$/;
const SPACE = 'spaces/AAQAoHKdzvI';
const NAME = /^spaces\/AAQAoHKdzvI\/messages\/[A-Za-z0-9_.-]{1,200}$/;
const THREAD = /^spaces\/AAQAoHKdzvI\/threads\/[A-Za-z0-9_-]{1,200}$/;
const MAX_RECORDS = 100000;
const DISPOSITIONS = new Set(['actionable', 'blocked', 'superseded', 'context',
  'credential_only', 'private', 'reference', 'acknowledgement', 'deleted']);
const cmp = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const fail = (code) => { throw new Error(code); };
const digest = (value) => createHash('sha256').update(JSON.stringify(value)).digest('hex');

function shape(value, keys, required = keys) {
  if (!value || Object.getPrototypeOf(value) !== Object.prototype) fail('invalid_object');
  if (Object.keys(value).some((key) => !keys.includes(key))) fail('unknown_field');
  if (required.some((key) => !Object.hasOwn(value, key))) fail('missing_field');
}
function boundedArray(value, maximum = MAX_RECORDS) {
  if (!Array.isArray(value) || value.length > maximum) fail('invalid_array');
  return value;
}
function integer(value, maximum = MAX_RECORDS) {
  if (!Number.isSafeInteger(value) || value < 0 || value > maximum) fail('invalid_count');
  return value;
}
function hex(value) {
  if (typeof value !== 'string' || !HEX.test(value)) fail('invalid_digest');
  return value;
}
function bool(value) {
  if (typeof value !== 'boolean') fail('invalid_boolean');
  return value;
}

// Date.parse truncates fractional milliseconds. Preserve nanoseconds when deciding
// exact inclusive/exclusive boundaries and reject normalized impossible dates.
export function canonicalChatInstant(value) {
  if (typeof value !== 'string') fail('invalid_instant');
  const match = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?Z$/.exec(value);
  if (!match) fail('invalid_instant');
  const milliseconds = Date.parse(`${match[1]}Z`);
  if (!Number.isFinite(milliseconds) || new Date(milliseconds).toISOString() !== `${match[1]}.000Z`) fail('invalid_instant');
  return `${match[1]}.${(match[2] || '').padEnd(9, '0')}Z`;
}

function sourceView(source) {
  shape(source, ['kind', 'artifactSha256', 'paginationComplete', 'snapshotTime']);
  if (!['bridge-export', 'retained-ledger'].includes(source.kind)) fail('invalid_source_kind');
  return { kind: source.kind, artifactSha256: hex(source.artifactSha256),
    paginationComplete: bool(source.paginationComplete), snapshotTime: canonicalChatInstant(source.snapshotTime) };
}
function descriptor(raw) {
  shape(raw, ['messageName', 'threadName', 'createTime', 'lastUpdateTime', 'bodySha256', 'attachmentCount', 'deleted']);
  if (typeof raw.messageName !== 'string' || !NAME.test(raw.messageName)) fail('invalid_message_name');
  if (typeof raw.threadName !== 'string' || !THREAD.test(raw.threadName)) fail('invalid_thread_name');
  const createTime = canonicalChatInstant(raw.createTime);
  const lastUpdateTime = raw.lastUpdateTime === null ? null : canonicalChatInstant(raw.lastUpdateTime);
  if (lastUpdateTime !== null && lastUpdateTime < createTime) fail('invalid_update_order');
  return { messageName: raw.messageName, threadName: raw.threadName, createTime, lastUpdateTime,
    bodySha256: hex(raw.bodySha256), attachmentCount: integer(raw.attachmentCount, 100), deleted: bool(raw.deleted) };
}

/** Build deterministic keyed provenance from full-body hashes supplied by a trusted
 * extractor. The key is a separate audit key, never a bridge token. Store it only
 * in an approved private runtime channel if repeatable future joins are needed. */
export function buildMessageInventory({ records, source, windowStartInclusive, windowEndExclusive, provenanceKey }) {
  boundedArray(records);
  if (!(provenanceKey instanceof Uint8Array) || provenanceKey.byteLength < 32 || provenanceKey.byteLength > 64) fail('invalid_provenance_key');
  const keyed = (domain, value) => createHmac('sha256', provenanceKey).update(`${domain}\0${JSON.stringify(value)}`).digest('hex');
  const start = canonicalChatInstant(windowStartInclusive);
  const end = canonicalChatInstant(windowEndExclusive);
  if (start < '2026-05-10T04:00:00.000000000Z' || start >= end) fail('invalid_window');
  const normalizedSource = sourceView(source);
  const seen = new Map();
  let duplicates = 0;
  for (const raw of records) {
    const row = descriptor(raw);
    if (row.createTime > normalizedSource.snapshotTime || (row.lastUpdateTime !== null && row.lastUpdateTime > normalizedSource.snapshotTime)) fail('record_after_snapshot');
    const messageKey = keyed('chat-message-v1', row.messageName);
    const versionDigest = keyed('chat-version-v1', row);
    const previous = seen.get(messageKey);
    if (previous) {
      if (previous.versionDigest !== versionDigest) fail('conflicting_message_version');
      duplicates += 1;
      continue;
    }
    seen.set(messageKey, { messageKey, versionDigest, threadKey: keyed('chat-thread-v1', row.threadName),
      createTime: row.createTime, lastUpdateTime: row.lastUpdateTime,
      attachmentCount: row.attachmentCount, deleted: row.deleted });
  }
  const selected = [...seen.values()].filter((row) => row.createTime >= start && row.createTime < end)
    .sort((a, b) => cmp(a.createTime, b.createTime) || cmp(a.messageKey, b.messageKey));
  const inventory = { schemaVersion: 1, space: SPACE, source: normalizedSource,
    windowStartInclusive: start, windowEndExclusive: end,
    counts: { input: records.length, unique: seen.size, duplicates, inWindow: selected.length, outsideWindow: seen.size - selected.length },
    records: selected };
  return { ...inventory, inventoryDigest: digest(inventory) };
}

function validateInventory(raw) {
  shape(raw, ['schemaVersion', 'space', 'source', 'windowStartInclusive', 'windowEndExclusive', 'counts', 'records', 'inventoryDigest']);
  if (raw.schemaVersion !== 1 || raw.space !== SPACE) fail('invalid_inventory_version');
  const source = sourceView(raw.source);
  const start = canonicalChatInstant(raw.windowStartInclusive);
  const end = canonicalChatInstant(raw.windowEndExclusive);
  if (start < '2026-05-10T04:00:00.000000000Z' || start >= end) fail('invalid_window');
  shape(raw.counts, ['input', 'unique', 'duplicates', 'inWindow', 'outsideWindow']);
  for (const value of Object.values(raw.counts)) integer(value);
  if (raw.counts.input !== raw.counts.unique + raw.counts.duplicates || raw.counts.unique !== raw.counts.inWindow + raw.counts.outsideWindow) fail('invalid_inventory_counts');
  const seen = new Set();
  const records = boundedArray(raw.records).map((row) => {
    shape(row, ['messageKey', 'versionDigest', 'threadKey', 'createTime', 'lastUpdateTime', 'attachmentCount', 'deleted']);
    hex(row.messageKey); hex(row.versionDigest); hex(row.threadKey);
    if (seen.has(row.messageKey)) fail('duplicate_inventory_message');
    seen.add(row.messageKey);
    const created = canonicalChatInstant(row.createTime);
    const updated = row.lastUpdateTime === null ? null : canonicalChatInstant(row.lastUpdateTime);
    if (created < start || created >= end || created > source.snapshotTime || (updated !== null && (updated < created || updated > source.snapshotTime))) fail('invalid_record_time');
    integer(row.attachmentCount, 100); bool(row.deleted);
    return { messageKey: row.messageKey, versionDigest: row.versionDigest, threadKey: row.threadKey,
      createTime: created, lastUpdateTime: updated, attachmentCount: row.attachmentCount, deleted: row.deleted };
  }).sort((a, b) => cmp(a.createTime, b.createTime) || cmp(a.messageKey, b.messageKey));
  if (records.length !== raw.counts.inWindow) fail('invalid_inventory_counts');
  const inventory = { schemaVersion: 1, space: SPACE, source, windowStartInclusive: start, windowEndExclusive: end,
    counts: { input: raw.counts.input, unique: raw.counts.unique, duplicates: raw.counts.duplicates,
      inWindow: raw.counts.inWindow, outsideWindow: raw.counts.outsideWindow }, records };
  if (digest(inventory) !== hex(raw.inventoryDigest)) fail('inventory_digest_mismatch');
  return inventory;
}

// Anchors, query strings, credentials, percent encodings and URL normalization are
// deliberately not accepted in audit authority references.
function githubUrl(value, kind) {
  if (typeof value !== 'string' || value.length > 300) fail('invalid_reference');
  const match = /^https:\/\/github\.com\/([A-Za-z0-9_-][A-Za-z0-9_-]{0,99})\/([A-Za-z0-9_.-]{1,100})\/(issues|pull)\/([1-9][0-9]*)$/.exec(value);
  if (!match || match[3] !== kind || ['.', '..'].includes(match[2]) || !Number.isSafeInteger(Number(match[4]))) fail('invalid_reference');
  return value;
}
function requirementsView(raw) {
  const seen = new Set();
  return boundedArray(raw, 64).map((requirement) => {
    shape(requirement, ['key', 'issues', 'pullRequests']);
    if (typeof requirement.key !== 'string' || !/^[A-Z][A-Z0-9_-]{0,63}$/.test(requirement.key)) fail('invalid_requirement_key');
    if (seen.has(requirement.key)) fail('duplicate_requirement');
    seen.add(requirement.key);
    const issues = boundedArray(requirement.issues, 32).map((url) => githubUrl(url, 'issues')).sort(cmp);
    if (new Set(issues).size !== issues.length) fail('duplicate_reference');
    const prs = new Set();
    const pullRequests = boundedArray(requirement.pullRequests, 32).map((pr) => {
      shape(pr, ['url', 'headSha', 'kind']);
      githubUrl(pr.url, 'pull');
      if (typeof pr.headSha !== 'string' || !SHA.test(pr.headSha)) fail('invalid_head');
      if (!['implementation', 'audit'].includes(pr.kind)) fail('invalid_pr_kind');
      if (prs.has(pr.url)) fail('duplicate_reference');
      prs.add(pr.url);
      return { url: pr.url, headSha: pr.headSha, kind: pr.kind };
    }).sort((a, b) => cmp(a.url, b.url));
    return { key: requirement.key, issues, pullRequests };
  }).sort((a, b) => cmp(a.key, b.key));
}

/** Validate review annotations against one exact inventory. Link presence and a
 * reviewer assertion are never promoted to a live issue, tested PR or feature
 * completion claim. That requires independent repository/evidence review. */
export function reconcileMessageCoverage({ inventory: rawInventory, review }) {
  const inventory = validateInventory(rawInventory);
  shape(review, ['schemaVersion', 'inventoryDigest', 'entries']);
  if (review.schemaVersion !== 1 || review.inventoryDigest !== rawInventory.inventoryDigest) fail('review_inventory_mismatch');
  const records = new Map(inventory.records.map((row) => [row.messageKey, row]));
  const reviewed = new Map();
  for (const raw of boundedArray(review.entries)) {
    shape(raw, ['messageKey', 'versionDigest', 'disposition', 'requirements', 'attachmentReview', 'supersededBy'],
      ['messageKey', 'versionDigest', 'disposition', 'requirements', 'attachmentReview']);
    const record = records.get(raw.messageKey);
    if (!record) fail('unknown_review_message');
    if (reviewed.has(raw.messageKey)) fail('duplicate_review_message');
    if (raw.versionDigest !== record.versionDigest) fail('stale_message_review');
    if (!DISPOSITIONS.has(raw.disposition)) fail('invalid_disposition');
    if ((raw.disposition === 'deleted') !== record.deleted) fail('deleted_disposition_mismatch');
    const requirements = requirementsView(raw.requirements);
    const actionable = ['actionable', 'blocked'].includes(raw.disposition);
    if (actionable !== (requirements.length > 0)) fail('invalid_requirement_disposition');
    const supersededBy = raw.supersededBy ?? null;
    if (raw.disposition === 'superseded') {
      if (!records.has(supersededBy) || supersededBy === raw.messageKey) fail('invalid_supersession');
    } else if (supersededBy !== null) fail('invalid_supersession');
    shape(raw.attachmentReview, ['reviewedCount', 'evidenceDigest']);
    const reviewedCount = integer(raw.attachmentReview.reviewedCount, 100);
    if (reviewedCount > record.attachmentCount) fail('invalid_attachment_count');
    if (reviewedCount > 0) hex(raw.attachmentReview.evidenceDigest);
    else if (raw.attachmentReview.evidenceDigest !== null) fail('unexpected_attachment_evidence');
    reviewed.set(raw.messageKey, { messageKey: raw.messageKey, versionDigest: record.versionDigest,
      disposition: raw.disposition, requirements, attachmentReview: { reviewedCount, evidenceDigest: raw.attachmentReview.evidenceDigest }, supersededBy });
  }
  // Resolve iteratively, bounded by the finite inventory; a chain must finish at
  // a reviewed actionable message. A context/acknowledgement cannot erase work.
  const roots = new Map();
  for (const entry of reviewed.values()) {
    const path = new Set();
    let current = entry;
    while (current.disposition === 'superseded') {
      if (path.has(current.messageKey)) fail('supersession_cycle');
      path.add(current.messageKey);
      current = reviewed.get(current.supersededBy);
      if (!current) fail('unreviewed_supersession_target');
      if (roots.has(current.messageKey)) { current = reviewed.get(roots.get(current.messageKey)); break; }
    }
    if (entry.disposition === 'superseded' && !['actionable', 'blocked'].includes(current.disposition)) fail('nonactionable_supersession_target');
    roots.set(entry.messageKey, current.messageKey);
    for (const key of path) roots.set(key, current.messageKey);
  }
  const requirements = new Map();
  for (const entry of reviewed.values()) for (const requirement of entry.requirements) {
    const previous = requirements.get(requirement.key);
    if (previous && JSON.stringify(previous) !== JSON.stringify(requirement)) fail('conflicting_requirement_links');
    requirements.set(requirement.key, requirement);
  }
  const rows = inventory.records.map((record) => {
    const entry = reviewed.get(record.messageKey);
    if (!entry) return { messageKey: record.messageKey, versionDigest: record.versionDigest,
      disposition: 'unreviewed', requirementKeys: [], canonicalMessageKey: null, unreviewedAttachments: record.attachmentCount };
    const root = reviewed.get(roots.get(entry.messageKey));
    return { messageKey: entry.messageKey, versionDigest: entry.versionDigest, disposition: entry.disposition,
      requirementKeys: root.requirements.map((r) => r.key), canonicalMessageKey: root.messageKey,
      unreviewedAttachments: record.attachmentCount - entry.attachmentReview.reviewedCount };
  });
  const allRequirements = [...requirements.values()].sort((a, b) => cmp(a.key, b.key));
  const counts = { messages: rows.length, reviewed: reviewed.size, unreviewed: rows.length - reviewed.size,
    blocked: rows.filter((row) => row.disposition === 'blocked').length,
    deletedContentUnavailable: rows.filter((row) => row.disposition === 'deleted').length,
    superseded: rows.filter((row) => row.disposition === 'superseded').length,
    unreviewedAttachments: rows.reduce((sum, row) => sum + row.unreviewedAttachments, 0),
    requirements: allRequirements.length,
    requirementsWithIssueLinks: allRequirements.filter((r) => r.issues.length > 0).length,
    requirementsWithImplementationPrLinks: allRequirements.filter((r) => r.pullRequests.some((pr) => pr.kind === 'implementation')).length };
  const receipt = { schemaVersion: 1, inventoryDigest: rawInventory.inventoryDigest,
    reviewDigest: digest([...reviewed.values()].sort((a, b) => cmp(a.messageKey, b.messageKey))),
    sourceKind: inventory.source.kind, windowStartInclusive: inventory.windowStartInclusive, windowEndExclusive: inventory.windowEndExclusive,
    sourceCompletenessReported: inventory.source.kind === 'bridge-export' && inventory.source.paginationComplete && inventory.source.snapshotTime >= inventory.windowEndExclusive,
    accountingComplete: rows.length > 0 && counts.unreviewed === 0,
    attachmentReviewRecorded: counts.unreviewedAttachments === 0,
    // These assertions deliberately cannot be authorized by a JSON annotation.
    sourceCompletenessIndependentlyVerified: false, issueCoverageVerified: false,
    implementationCoverageVerified: false, completionClaimAllowed: false,
    counts, records: rows, requirements: allRequirements };
  return { ...receipt, receiptDigest: digest(receipt) };
}
