# `@oresoftware/durable-worker-sdk`

Dependency-free TypeScript declarations and native ESM for the ORESoftware
durable worker runtime. It supports both long-lived worker processes and bounded
one-shot execution without embedding workflow code in the control plane.

## Design guarantees

- The client never logs or serializes the worker authentication secret.
- Submissions are retried automatically only when they carry an idempotency key.
- Signals and worker polls are sent once; an ambiguous poll stops admission and relies on lease expiry/redelivery.
- Redirects are handled manually so the worker credential is never forwarded to another origin.
- Lease-scoped operations carry the opaque lease token and monotonic generation.
- A fenced heartbeat aborts the handler and suppresses stale completion/failure.
- Progress chunks have explicit stable IDs so replay is idempotent.
- Worker and step heartbeats are independent.
- `maxAssignments` supports one-shot or serverless-style bounded workers.
- Shutdown drains the worker registration and waits for accepted assignments.

The runtime remains at-least-once. External side effects still need an
idempotency key or a downstream write guarded by `context.fencingToken`.

## Submit one task

```ts
import { DurableWorkerClient } from '@oresoftware/durable-worker-sdk';

const client = new DurableWorkerClient({
  baseUrl: process.env.DURABLE_WORKER_URL!,
  authSecret: process.env.DURABLE_WORKER_AUTH_SECRET!,
});

const submitted = await client.submitTask({
  idempotencyKey: `document:${documentId}:v3`,
  name: 'generate document',
  taskType: 'documents:generate',
  queue: 'documents',
  input: { documentId },
  priority: 50,
  requiredCapabilities: ['llm', 'pdf'],
  retry: {
    maxAttempts: 4,
    initialBackoffMs: 1_000,
    maxBackoffMs: 30_000,
    multiplier: 2,
  },
  timeoutMs: 20 * 60_000,
  leaseMs: 30_000,
  deadlineMs: Date.now() + 30 * 60_000,
  concurrency: { key: `customer:${customerId}`, limit: 2 },
});
```

A submission without `idempotencyKey` is sent only once. The SDK will not turn
an ambiguous network failure into a duplicate run.

## Long-lived worker

```ts
const summary = await client.runWorker({
  workerId: `documents-${process.pid}`,
  queues: ['documents'],
  capabilities: ['llm', 'pdf'],
  slots: 4,
  ttlMs: 60_000,
  handlers: {
    'documents:generate': async (input, context) => {
      const draft = await generateDraft(input.documentId, {
        signal: context.signal,
      });

      await context.progress(JSON.stringify({ stage: 'drafted' }), {
        chunkId: `draft:${input.documentId}`,
        stream: 'status',
      });

      await persistDocument({
        documentId: input.documentId,
        draft,
        fencingToken: context.fencingToken,
      });

      return { documentId: input.documentId, stored: true };
    },
  },
  onError(error, context) {
    logger.error({ error, phase: context.phase }, 'durable worker error');
  },
});
```

The SDK automatically:

1. registers the worker;
2. maintains the worker TTL heartbeat;
3. long-polls while local slots are available;
4. starts each assignment;
5. renews the step lease while the handler runs;
6. aborts the handler if a heartbeat is fenced;
7. completes or fails using the same lease generation; and
8. marks the worker draining during shutdown.

Handlers should pass `context.signal` to network and subprocess APIs. A fenced
worker must stop side effects promptly.

## Bounded worker

```ts
await client.runWorker({
  workerId: `serverless-${crypto.randomUUID()}`,
  queues: ['documents'],
  capabilities: ['llm'],
  maxAssignments: 1,
  handlers,
});
```

`maxAssignments` bounds admission, not completion. The call waits for every
accepted assignment before draining and returning its summary.

## Streaming output

```ts
await context.progress(token, {
  chunkId: `${context.stepId}:token:${tokenIndex}`,
  stream: 'tokens',
  finalChunk: tokenIndex === finalIndex,
});
```

Reusing the same `chunkId` with the same payload is safe. Reusing it with a
different payload is rejected by the server.

## Failure semantics

Throw an error with optional `code` and `retryable` properties:

```ts
throw Object.assign(new Error('source document is invalid'), {
  code: 'invalid_document',
  retryable: false,
});
```

The SDK emits one durable failure mutation. If the lease was already fenced, it
does not try to overwrite the newer owner.

## Authentication

The default header is `X-Worker-Auth`. Internal submitters may select
`authHeader: 'x-server-auth'` with a distinct server credential. Never share the
operator credential with untrusted worker code.
