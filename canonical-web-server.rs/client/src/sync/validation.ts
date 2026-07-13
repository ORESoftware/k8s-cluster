import {
  DRAFT_NOTE_SCHEMA_VERSION,
  MAX_DRAFT_NOTE_BODY_LENGTH,
  MAX_DRAFT_NOTE_TITLE_LENGTH,
  type DecimalVersion,
  type DraftNoteValue,
  type WireRecord,
} from "./types";

const DECIMAL_VERSION = /^(0|[1-9][0-9]*)$/;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function assertAccountKey(accountKey: string): void {
  if (accountKey.trim().length === 0 || accountKey.length > 512) {
    throw new TypeError("accountKey must be a non-empty string of at most 512 characters");
  }
}

export function assertRecordId(id: string): void {
  if (!UUID.test(id)) {
    throw new TypeError("record id must be a UUID");
  }
}

function hasAtMostCodePoints(value: string, maximum: number): boolean {
  let count = 0;
  for (const _character of value) {
    count += 1;
    if (count > maximum) {
      return false;
    }
  }
  return true;
}

export function assertDraftNoteValue(value: DraftNoteValue): void {
  if (
    typeof value.title !== "string" ||
    !hasAtMostCodePoints(value.title, MAX_DRAFT_NOTE_TITLE_LENGTH)
  ) {
    throw new TypeError(`draft note title must be at most ${MAX_DRAFT_NOTE_TITLE_LENGTH} characters`);
  }
  if (
    typeof value.body !== "string" ||
    !hasAtMostCodePoints(value.body, MAX_DRAFT_NOTE_BODY_LENGTH)
  ) {
    throw new TypeError(`draft note body must be at most ${MAX_DRAFT_NOTE_BODY_LENGTH} characters`);
  }
}

export function assertDecimalVersion(version: unknown): asserts version is DecimalVersion {
  if (typeof version !== "string" || !DECIMAL_VERSION.test(version)) {
    throw new TypeError("record version must be an unsigned decimal string");
  }
}

export function compareDecimalVersions(left: DecimalVersion, right: DecimalVersion): number {
  assertDecimalVersion(left);
  assertDecimalVersion(right);
  if (left.length !== right.length) {
    return left.length < right.length ? -1 : 1;
  }
  return left === right ? 0 : left < right ? -1 : 1;
}

export function assertWireRecord(record: WireRecord): void {
  if (record.key.kind !== "draft_note") {
    throw new TypeError("unsupported record kind");
  }
  assertRecordId(record.key.id);
  assertDecimalVersion(record.version);
  if (record.schemaVersion !== DRAFT_NOTE_SCHEMA_VERSION) {
    throw new TypeError("unsupported draft_note schema version");
  }
  if (typeof record.deleted !== "boolean") {
    throw new TypeError("record deleted flag must be boolean");
  }
  if (record.deleted) {
    if (record.value !== undefined) {
      throw new TypeError("deleted records must not include a value");
    }
    return;
  }
  if (record.value === undefined) {
    throw new TypeError("live records must include a value");
  }
  assertDraftNoteValue(record.value);
}
