// Single source of truth for the rust-network-mutex-rs wire format from the
// TypeScript side. The shape mirrors `src/protocol.rs` (see ../../PROTOCOL.md
// in this deployment) and is camelCase on the wire. We use a discriminated
// union on `type` so any new variant added to the Rust enum produces a
// compile error here until handled — matching the semantic of the Rust enum.

// ---------- Request types ------------------------------------------------

export type RequestType =
  | "version"
  | "auth"
  | "lock"
  | "unlock"
  | "registerRead"
  | "registerWrite"
  | "endRead"
  | "endWrite"
  | "lockInfo"
  | "ls"
  | "heartbeat";

export interface VersionRequest {
  type: "version";
  uuid: string;
  value: string;
}

export interface AuthRequest {
  type: "auth";
  uuid: string;
  token: string;
}

export interface LockRequest {
  type: "lock";
  uuid: string;
  key?: string;
  keys?: string[];
  pid?: number;
  ttl?: number;
  max?: number | null;
  force?: boolean;
  retryCount?: number;
  keepLocksAfterDeath?: boolean;
  /** When false, the broker fails fast (acquired:false) instead of queuing.
   * Absent/true = block until grant. */
  wait?: boolean;
}

export interface UnlockRequest {
  type: "unlock";
  uuid: string;
  key?: string;
  keys?: string[];
  lockUuid?: string;
  force?: boolean;
}

export interface RegisterReadRequest {
  type: "registerRead";
  uuid: string;
  key: string;
}

export interface RegisterWriteRequest {
  type: "registerWrite";
  uuid: string;
  key: string;
}

export interface EndReadRequest {
  type: "endRead";
  uuid: string;
  key: string;
}

export interface EndWriteRequest {
  type: "endWrite";
  uuid: string;
  key: string;
}

export interface LockInfoRequest {
  type: "lockInfo";
  uuid: string;
  key: string;
}

export interface LsRequest {
  type: "ls";
  uuid: string;
}

export interface HeartbeatRequest {
  type: "heartbeat";
  uuid: string;
}

export type Request =
  | VersionRequest
  | AuthRequest
  | LockRequest
  | UnlockRequest
  | RegisterReadRequest
  | RegisterWriteRequest
  | EndReadRequest
  | EndWriteRequest
  | LockInfoRequest
  | LsRequest
  | HeartbeatRequest;

// ---------- Response types -----------------------------------------------

export type ResponseType =
  | "version"
  | "auth"
  | "lock"
  | "compositeLock"
  | "unlock"
  | "registerReadResult"
  | "registerWriteResult"
  | "endReadResult"
  | "endWriteResult"
  | "lockInfo"
  | "lsResult"
  | "reelection"
  | "error"
  | "ok";

export interface VersionResponse {
  type: "version";
  uuid: string;
  brokerVersion: string;
  ok: boolean;
  error?: string | null;
}

export interface AuthResponse {
  type: "auth";
  uuid: string;
  ok: boolean;
  error?: string | null;
}

export interface LockResponse {
  type: "lock";
  uuid: string;
  key: string;
  acquired: boolean;
  lockRequestCount: number;
  lockUuid?: string | null;
  fencingToken?: number | null;
  readersCount?: number | null;
  error?: string | null;
}

export interface CompositeLockResponse {
  type: "compositeLock";
  uuid: string;
  keys: string[];
  acquired: boolean;
  lockUuid?: string | null;
  fencingTokens?: Record<string, number> | null;
  error?: string | null;
}

export interface UnlockResponse {
  type: "unlock";
  uuid: string;
  keys: string[];
  unlocked: boolean;
  lockRequestCount: number;
  error?: string | null;
}

export interface RegisterReadResultResponse {
  type: "registerReadResult";
  uuid: string;
  key: string;
  readersCount: number;
  writerFlag: boolean;
  granted: boolean;
  lockUuid?: string | null;
  fencingToken?: number | null;
}

export interface RegisterWriteResultResponse {
  type: "registerWriteResult";
  uuid: string;
  key: string;
  readersCount: number;
  writerFlag: boolean;
  granted: boolean;
  lockUuid?: string | null;
  fencingToken?: number | null;
}

export interface EndReadResultResponse {
  type: "endReadResult";
  uuid: string;
  key: string;
  readersCount: number;
}

export interface EndWriteResultResponse {
  type: "endWriteResult";
  uuid: string;
  key: string;
  readersCount: number;
  writerFlag: boolean;
}

export interface LockInfoResponse {
  type: "lockInfo";
  uuid: string;
  key: string;
  isLocked: boolean;
  lockholderUuids: string[];
  lockRequestCount: number;
  readersCount: number;
  writerFlag: boolean;
}

export interface LsResultResponse {
  type: "lsResult";
  uuid: string;
  keys: string[];
}

export interface ReelectionResponse {
  type: "reelection";
  uuid: string;
  key: string;
}

export interface ErrorResponse {
  type: "error";
  uuid: string;
  error: string;
}

export interface OkResponse {
  type: "ok";
  uuid: string;
}

export type Response =
  | VersionResponse
  | AuthResponse
  | LockResponse
  | CompositeLockResponse
  | UnlockResponse
  | RegisterReadResultResponse
  | RegisterWriteResultResponse
  | EndReadResultResponse
  | EndWriteResultResponse
  | LockInfoResponse
  | LsResultResponse
  | ReelectionResponse
  | ErrorResponse
  | OkResponse;

// Compile-time exhaustiveness helper. Use inside `switch (resp.type) { … }`
// in the `default` branch to force every variant to be handled.
export function assertNever(value: never): never {
  throw new Error(`unexpected variant: ${JSON.stringify(value)}`);
}
