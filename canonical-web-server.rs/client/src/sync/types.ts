export const PROTOCOL_VERSION = 1 as const;
export const DRAFT_NOTE_SCHEMA_VERSION = 1 as const;
export const MAX_DRAFT_NOTE_TITLE_LENGTH = 200;
export const MAX_DRAFT_NOTE_BODY_LENGTH = 100_000;

export type DecimalVersion = string;
export type DraftNoteKind = "draft_note";
export type MutationAction = "put" | "delete";
export type OutboxStatus = "queued" | "inflight" | "blocked" | "failed";
export type RecordSyncState = "synced" | "pending" | "conflict" | "failed";

export interface DraftNoteValue {
  title: string;
  body: string;
}

export interface DraftNoteKey {
  kind: DraftNoteKind;
  id: string;
}

export interface WireRecord {
  key: DraftNoteKey;
  version: DecimalVersion;
  schemaVersion: typeof DRAFT_NOTE_SCHEMA_VERSION;
  deleted: boolean;
  value?: DraftNoteValue;
}

export interface ConfirmedShadow {
  version: DecimalVersion;
  schemaVersion: typeof DRAFT_NOTE_SCHEMA_VERSION;
  deleted: boolean;
  value?: DraftNoteValue;
}

export interface OptimisticShadow {
  localSeq: number;
  action: MutationAction;
  value?: DraftNoteValue;
}

export interface LocalDraftNoteRecord {
  accountKey: string;
  kind: DraftNoteKind;
  id: string;
  confirmed: ConfirmedShadow | null;
  optimistic: OptimisticShadow | null;
  state: RecordSyncState;
  updatedAt: number;
}

export interface OutboxMutation {
  accountKey: string;
  mutationId: string;
  key: DraftNoteKey;
  action: MutationAction;
  baseVersion: DecimalVersion | null;
  schemaVersion: typeof DRAFT_NOTE_SCHEMA_VERSION;
  value?: DraftNoteValue;
  localSeq: number;
  status: OutboxStatus;
  dependsOnMutationId: string | null;
  attempts: number;
  nextAttemptAt: number;
  leaseOwner: string | null;
  leaseExpiresAt: number | null;
  createdAt: number;
  lastError: string | null;
}

export interface SyncConflict {
  accountKey: string;
  kind: DraftNoteKind;
  id: string;
  mutationId: string;
  reason: "conflict" | "gone";
  baseVersion: DecimalVersion | null;
  local: OptimisticShadow;
  server: ConfirmedShadow | null;
  detectedAt: number;
}

export interface MutationOperation {
  mutationId: string;
  key: DraftNoteKey;
  action: MutationAction;
  baseVersion: DecimalVersion | null;
  schemaVersion: typeof DRAFT_NOTE_SCHEMA_VERSION;
  value?: DraftNoteValue;
}

export interface MutationRequest {
  protocolVersion: typeof PROTOCOL_VERSION;
  clientId: string;
  operations: MutationOperation[];
}

export type MutationResultStatus =
  | "applied"
  | "conflict"
  | "gone"
  | "invalid"
  | "forbidden"
  | "idempotency_key_reused";

export interface MutationResult {
  mutationId: string;
  status: MutationResultStatus;
  record?: WireRecord;
  current?: WireRecord;
  error?: string;
  message?: string;
}

export interface MutationResponse {
  results: MutationResult[];
}

export interface ChangesResponse {
  changes: WireRecord[];
  nextCursor: string;
  caughtUp: boolean;
}

export interface EffectiveDraftNote {
  id: string;
  value: DraftNoteValue;
  version: DecimalVersion | null;
  pending: boolean;
  conflicted: boolean;
  failed: boolean;
}

export interface SyncInvalidationMessage {
  type: "sync.invalidated";
  latestHint?: string;
}

export type SyncSocketMessage =
  | SyncInvalidationMessage
  | { type: "hello"; protocolVersion: number; heartbeatSeconds?: number }
  | { type: "resync_required" }
  | { type: "access_revoked" };

export interface SyncChangeNotification {
  accountKey: string;
  source: "local" | "pull" | "push" | "conflict" | "logout";
}
