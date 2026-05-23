// Thin wrapper around the global fetch that inherits the per-request
// AsyncLocalStorage context (see ./request-context.ts):
//
//   * x-request-id is forwarded so rest-api / Vercel / OTLP joins the
//     same trace as the originating dev-server request.
//   * x-dd-thread-id and x-dd-task-id are forwarded when known so
//     downstream logs can pivot by thread/task without re-parsing bodies.
//   * Network errors are re-thrown with the request context attached via
//     annotateError(), so handlers / process.on('unhandledRejection')
//     can pin the failure to the request that issued it.
//
// Use this in place of bare fetch at every outbound call site. Outside
// a request (boot-time register, heartbeat setInterval, etc.) the
// wrapper is still safe to call — it just skips header injection when
// there's no active context.

import { annotateError, getRequestContext } from './request-context.js';

type FetchArgs = Parameters<typeof fetch>;
type FetchInput = FetchArgs[0];
type FetchInit = FetchArgs[1];
type HeadersArg = NonNullable<FetchInit>['headers'];

export async function contextFetch(
  input: FetchInput,
  init?: FetchInit,
): Promise<Response> {
  const ctx = getRequestContext();

  let headers: Headers;
  if (init?.headers) {
    headers = new Headers(init.headers as HeadersArg as ConstructorParameters<typeof Headers>[0]);
  } else if (typeof input === 'object' && input !== null && 'headers' in input) {
    // Covers the Request instance case without needing a `Request` global type.
    const reqHeaders = (input as { headers: ConstructorParameters<typeof Headers>[0] }).headers;
    headers = new Headers(reqHeaders);
  } else {
    headers = new Headers();
  }

  if (ctx) {
    if (!headers.has('x-request-id')) {
      headers.set('x-request-id', ctx.requestId);
    }
    if (ctx.threadId && !headers.has('x-dd-thread-id')) {
      headers.set('x-dd-thread-id', ctx.threadId);
    }
    if (ctx.taskId && !headers.has('x-dd-task-id')) {
      headers.set('x-dd-task-id', ctx.taskId);
    }
  }

  const finalInit: FetchInit = { ...(init ?? {}), headers };

  const url =
    typeof input === 'string'
      ? input
      : input instanceof URL
        ? input.toString()
        : (input as { url?: string }).url ?? '<unknown>';

  const startedAt = Date.now();

  try {
    const res = await fetch(input, finalInit);
    if (!res.ok && ctx) {
      // Pin slow / failing upstream calls to the request that issued
      // them. Logs only — callers decide whether the non-2xx response
      // should throw. (Most call sites today swallow + log.)
      const elapsedMs = Date.now() - startedAt;
      process.stderr.write(
        `[context-fetch] ${res.status} ${url} requestId=${ctx.requestId} threadId=${ctx.threadId ?? '-'} taskId=${ctx.taskId ?? '-'} elapsedMs=${elapsedMs}\n`,
      );
    }
    return res;
  } catch (err) {
    throw annotateError(err);
  }
}
