// Per-request context plumbed via node:async_hooks AsyncLocalStorage.
//
// Goals:
//   1. Every Fastify request runs inside a context store seeded with a
//      stable requestId, route, method, plus any identifiers the handler
//      discovers (taskId, threadId, userId, provider).
//   2. Outbound HTTPS calls (see ./wrapped-fetch.ts) read the store and
//      propagate x-request-id + x-dd-thread-id + x-dd-task-id headers, so
//      downstream services join the same trace without callers passing
//      the request object through five layers of code.
//   3. Runtime errors (Fastify handler throws, uncaughtException,
//      unhandledRejection) get tagged with the store contents via
//      annotateError(), pinning the crash to the request that caused it.

import { AsyncLocalStorage, AsyncResource } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';

export type RequestContext = {
  requestId: string;
  startedAt: number;
  method?: string;
  route?: string;
  path?: string;
  threadId?: string;
  taskId?: string;
  userId?: string;
  provider?: string;
  // Free-form bag for late-binding fields (e.g. branch, sessionId) that
  // handlers want to attach after their initial setContextField calls.
  extra: Record<string, string | number | boolean>;
};

const storage = new AsyncLocalStorage<RequestContext>();

export function runWithRequestContext<T>(
  seed: Partial<RequestContext>,
  fn: () => T,
): T {
  const ctx: RequestContext = {
    requestId: seed.requestId ?? randomUUID(),
    startedAt: seed.startedAt ?? Date.now(),
    method: seed.method,
    route: seed.route,
    path: seed.path,
    threadId: seed.threadId,
    taskId: seed.taskId,
    userId: seed.userId,
    provider: seed.provider,
    extra: { ...(seed.extra ?? {}) },
  };
  return storage.run(ctx, fn);
}

export function getRequestContext(): RequestContext | undefined {
  return storage.getStore();
}

export function setContextField<K extends keyof Omit<RequestContext, 'extra' | 'startedAt' | 'requestId'>>(
  key: K,
  value: RequestContext[K],
): void {
  const ctx = storage.getStore();
  if (!ctx) return;
  ctx[key] = value;
}

export function setContextExtra(
  key: string,
  value: string | number | boolean,
): void {
  const ctx = storage.getStore();
  if (!ctx) return;
  ctx.extra[key] = value;
}

export type SerializedRequestContext = Omit<RequestContext, 'extra'> & {
  extra: Record<string, string | number | boolean>;
};

export function snapshotRequestContext(
  ctx: RequestContext | undefined = storage.getStore(),
): SerializedRequestContext | null {
  if (!ctx) return null;
  return {
    requestId: ctx.requestId,
    startedAt: ctx.startedAt,
    method: ctx.method,
    route: ctx.route,
    path: ctx.path,
    threadId: ctx.threadId,
    taskId: ctx.taskId,
    userId: ctx.userId,
    provider: ctx.provider,
    extra: { ...ctx.extra },
  };
}

// Attach the current request context snapshot to an error as a
// non-enumerable `requestContext` property. Logger and crash handlers
// read it; JSON serialization of request bodies / responses is unaffected.
export function annotateError(err: unknown): Error {
  const e = err instanceof Error ? err : new Error(String(err));
  const snapshot = snapshotRequestContext();
  if (!snapshot) return e;
  if (Object.prototype.hasOwnProperty.call(e, 'requestContext')) {
    return e;
  }
  Object.defineProperty(e, 'requestContext', {
    value: snapshot,
    enumerable: false,
    configurable: true,
    writable: true,
  });
  return e;
}

export function readErrorRequestContext(err: unknown): SerializedRequestContext | null {
  if (!err || typeof err !== 'object') return null;
  const candidate = (err as { requestContext?: SerializedRequestContext }).requestContext;
  return candidate ?? null;
}

// Capture the active ALS context and return a NEW wrapped function
// that runs in that same context whenever it's invoked later. The
// returned function is a fresh closure — bindToCurrentContext never
// mutates `fn` or any object it was attached to (no monkeypatching).
//
// Native async/await and Promise chains already preserve the store,
// and EventEmitter listeners attached synchronously inside the run
// scope (the pattern every existing agent spawn uses) also keep it.
// Use this helper only for the lazy-attach case where the listener
// registration happens in a later tick / outside the run scope.
//
// Example:
//   runWithRequestContext(seed, async () => {
//     const child = spawn('claude', args);
//     // Same tick: native propagation handles this.
//     child.stdout.on('data', onData);
//     // Cross-tick: capture context now, attach later.
//     const onExitBound = bindToCurrentContext(onExit);
//     queueLater(() => child.on('exit', onExitBound));
//   });
// eslint-disable-next-line @typescript-eslint/no-explicit-any -- intentional generic for arbitrary callbacks
export function bindToCurrentContext<T extends (...args: any[]) => unknown>(fn: T): T {
  return AsyncResource.bind(fn) as unknown as T;
}
