export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
export interface JsonObject {
  [key: string]: JsonValue;
}

export interface RetryOptions {
  maxRetries?: number;
  initialDelayMs?: number;
  maxDelayMs?: number;
  multiplier?: number;
}

export interface DurableWorkerClientOptions {
  baseUrl: string;
  authSecret: string;
  authHeader?: 'x-worker-auth' | 'x-server-auth' | string;
  fetch?: typeof globalThis.fetch;
  requestTimeoutMs?: number;
  retry?: RetryOptions;
  randomUUID?: () => string;
}

export interface RequestOptions {
  method?: string;
  body?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
  timeoutMs?: number;
  idempotent?: boolean;
}

export interface RetryPolicy {
  maxAttempts?: number;
  initialBackoffMs?: number;
  maxBackoffMs?: number;
  multiplier?: number;
}

export interface ConcurrencyPolicy {
  key: string;
  limit: number;
}

export interface StepDefinition<TInput extends JsonObject = JsonObject> {
  key: string;
  taskType: string;
  queue?: string;
  input?: TInput;
  dependsOn?: string[];
  priority?: number;
  requiredCapabilities?: string[];
  retry?: RetryPolicy;
  timeoutMs?: number;
  leaseMs?: number;
  notBeforeMs?: number;
  waitForSignal?: string;
  concurrency?: ConcurrencyPolicy;
  affinityKey?: string;
}

export interface SubmitRunRequest {
  idempotencyKey?: string;
  name?: string;
  metadata?: JsonObject;
  deadlineMs?: number;
  steps: StepDefinition[];
}

export interface SubmitTaskRequest<TInput extends JsonObject = JsonObject> {
  idempotencyKey?: string;
  name?: string;
  metadata?: JsonObject;
  deadlineMs?: number;
  taskType: string;
  queue?: string;
  input?: TInput;
  priority?: number;
  requiredCapabilities?: string[];
  retry?: RetryPolicy;
  timeoutMs?: number;
  leaseMs?: number;
  notBeforeMs?: number;
  waitForSignal?: string;
  concurrency?: ConcurrencyPolicy;
  affinityKey?: string;
}

export interface SubmitRunResponse {
  runId: string;
  status: string;
  idempotentReplay: boolean;
}

export interface MutationResponse {
  ok: boolean;
  runId?: string | null;
  stepId?: string | null;
  status?: string | null;
}

export interface SignalResponse {
  runId: string;
  signalName: string;
  releasedSteps: number;
}

export interface WorkerRegistration {
  workerId: string;
  queues?: string[];
  capabilities?: string[];
  labels?: JsonObject;
  slots?: number;
  ttlMs?: number;
  drain?: boolean;
}

export interface WorkerRecord {
  workerId: string;
  queues: string[];
  capabilities: string[];
  labels: JsonObject;
  slots: number;
  ttlMs: number;
  status: string;
  registeredAtMs: number;
  lastHeartbeatMs: number;
}

export interface StepAssignment<TInput extends JsonObject = JsonObject> {
  runId: string;
  stepId: string;
  stepKey: string;
  taskType: string;
  queue: string;
  input: TInput;
  attempt: number;
  leaseToken: string;
  leaseGeneration: number;
  fencingToken: number;
  leaseExpiresAtMs: number;
  timeoutMs: number;
  affinityKey?: string | null;
}

export interface PollResponse<TInput extends JsonObject = JsonObject> {
  assignment?: StepAssignment<TInput> | null;
  retryAfterMs: number;
}

export interface LeaseCommand {
  workerId: string;
  leaseToken: string;
  leaseGeneration: number;
}

export interface StepOutput {
  chunkId?: string;
  stream?: string;
  chunk: string;
  finalChunk?: boolean;
}

export interface StepFailure {
  code: string;
  message: string;
  retryable?: boolean;
}

export interface OutputReceipt {
  chunkId: string;
  payloadHash: string;
  sequence: number;
  eventId: string;
  occurredAtMs: number;
  stream: string;
  finalChunk: boolean;
}

export interface LeaseRecord {
  token: string;
  generation: number;
  workerId: string;
  acquiredAtMs: number;
  expiresAtMs: number;
  fencingToken: number;
}

export interface StepRecord {
  id: string;
  runId: string;
  key: string;
  taskType: string;
  queue: string;
  input: JsonObject;
  dependsOn: string[];
  priority: number;
  requiredCapabilities: string[];
  retry: Required<RetryPolicy>;
  timeoutMs: number;
  leaseMs: number;
  notBeforeMs?: number | null;
  waitForSignal?: string | null;
  concurrency?: ConcurrencyPolicy | null;
  affinityKey?: string | null;
  status: string;
  attempt: number;
  leaseGeneration: number;
  lease?: LeaseRecord | null;
  lastLease?: LeaseRecord | null;
  result: JsonObject;
  failure?: StepFailure | null;
  outputSequence: number;
  lastOutput?: OutputReceipt | null;
  createdAtMs: number;
  updatedAtMs: number;
  startedAtMs?: number | null;
  completedAtMs?: number | null;
}

export interface RunRecord {
  id: string;
  name?: string | null;
  status: string;
  metadata: JsonObject;
  stepIds: Record<string, string>;
  counts: Record<string, number>;
  deadlineMs?: number | null;
  createdAtMs: number;
  updatedAtMs: number;
  completedAtMs?: number | null;
}

export interface RunSnapshot {
  run: RunRecord;
  steps: StepRecord[];
}

export interface AssignmentContext<TInput extends JsonObject = JsonObject> {
  readonly assignment: StepAssignment<TInput>;
  readonly client: DurableWorkerClient;
  readonly fencingToken: number;
  readonly runId: string;
  readonly stepId: string;
  readonly signal: AbortSignal;
  heartbeat(): Promise<MutationResponse>;
  progress(
    chunk: string,
    options?: Omit<StepOutput, 'chunk'>,
  ): Promise<MutationResponse>;
}

export type TaskHandler<
  TInput extends JsonObject = JsonObject,
  TResult = JsonObject,
> = (
  input: TInput,
  context: AssignmentContext<TInput>,
) => TResult | Promise<TResult>;

export interface WorkerErrorContext {
  phase: string;
  workerId: string;
  assignment?: StepAssignment;
}

export interface RunWorkerOptions {
  workerId: string;
  queues?: Iterable<string>;
  capabilities?: Iterable<string>;
  labels?: JsonObject;
  slots?: number;
  ttlMs?: number;
  pollWaitMs?: number;
  workerHeartbeatMs?: number;
  leaseHeartbeatFraction?: number;
  maxAssignments?: number;
  drainOnStop?: boolean;
  signal?: AbortSignal;
  handlers: Record<string, TaskHandler> | Map<string, TaskHandler>;
  onError?: (error: unknown, context: WorkerErrorContext) => void;
}

export interface WorkerRunSummary {
  accepted: number;
  succeeded: number;
  failed: number;
  leaseLost: number;
}

export class DurableWorkerError extends Error {
  readonly status: number | null;
  readonly code: string;
  readonly retryable: boolean;
  readonly details: unknown;
  constructor(
    message: string,
    options?: {
      status?: number | null;
      code?: string;
      retryable?: boolean;
      details?: unknown;
      cause?: unknown;
    },
  );
}

export class LeaseLostError extends DurableWorkerError {}

export function sleep(ms: number, signal?: AbortSignal): Promise<void>;

export class DurableWorkerClient {
  readonly baseUrl: string;
  readonly authSecret: string;
  readonly authHeader: string;
  readonly requestTimeoutMs: number;

  constructor(options: DurableWorkerClientOptions);

  request<T = unknown>(path: string, options?: RequestOptions): Promise<T>;
  submitTask<TInput extends JsonObject = JsonObject>(
    task: SubmitTaskRequest<TInput>,
    options?: RequestOptions,
  ): Promise<SubmitRunResponse>;
  submitRun(run: SubmitRunRequest, options?: RequestOptions): Promise<SubmitRunResponse>;
  getRun(runId: string, options?: RequestOptions): Promise<RunSnapshot>;
  signalRun(
    runId: string,
    signalName: string,
    payload?: JsonObject,
    options?: RequestOptions,
  ): Promise<SignalResponse>;
  pauseRun(runId: string, options?: RequestOptions): Promise<MutationResponse>;
  resumeRun(runId: string, options?: RequestOptions): Promise<MutationResponse>;
  cancelRun(runId: string, options?: RequestOptions): Promise<MutationResponse>;
  registerWorker(
    registration: WorkerRegistration,
    options?: RequestOptions,
  ): Promise<WorkerRecord>;
  heartbeatWorker(
    workerId: string,
    heartbeat?: { drain?: boolean },
    options?: RequestOptions,
  ): Promise<WorkerRecord>;
  pollWorker<TInput extends JsonObject = JsonObject>(
    workerId: string,
    poll?: { waitMs?: number },
    options?: RequestOptions,
  ): Promise<PollResponse<TInput>>;
  startStep(
    stepId: string,
    command: LeaseCommand,
    options?: RequestOptions,
  ): Promise<MutationResponse>;
  heartbeatStep(
    stepId: string,
    command: LeaseCommand,
    options?: RequestOptions,
  ): Promise<MutationResponse>;
  completeStep(
    stepId: string,
    command: LeaseCommand,
    result?: JsonObject,
    options?: RequestOptions,
  ): Promise<MutationResponse>;
  failStep(
    stepId: string,
    command: LeaseCommand,
    failure: StepFailure,
    options?: RequestOptions,
  ): Promise<MutationResponse>;
  appendOutput(
    stepId: string,
    command: LeaseCommand,
    output: StepOutput,
    options?: RequestOptions,
  ): Promise<MutationResponse>;
  runWorker(options: RunWorkerOptions): Promise<WorkerRunSummary>;
}
