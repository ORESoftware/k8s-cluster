# ORESoftware durable-worker Dart SDK

A dependency-free Dart 3 client and lifecycle-aware worker loop for the
independent `dd-durable-worker-server` in `ORESoftware/k8s-cluster`.

This package is deliberately separate from generated OpenAPI clients. It owns
worker lifecycle semantics that generated endpoint bindings cannot safely infer:

- retries only when the call carries a stable idempotency identity;
- worker polling, signals, and unbound task/run submissions are sent once;
- HTTP redirects are refused so worker credentials stay on the configured
  origin;
- response bodies are bounded before JSON decoding;
- worker and step heartbeats are independent;
- progress chunk IDs are scoped to `{stepId}:{leaseGeneration}:{sequence}`;
- lease loss cancels the handler and suppresses stale completion/failure writes;
- delivery is **at least once**, so downstream effects must use idempotency keys
  or the assignment `fencingToken`.

## Client

```dart
import 'package:oresoftware_durable_worker/durable_worker.dart';

final client = DurableWorkerClient(
  'https://worker.example.internal',
  Platform.environment['DURABLE_WORKER_AUTH_SECRET']!,
);

final run = await client.submitRun(<String, Object?>{
  'idempotencyKey': 'invoice:2026-08-08',
  'steps': <Object?>[],
});
```

## Worker

```dart
final worker = Worker(
  api: client,
  config: const WorkerConfig(
    workerId: 'dart-worker-1',
    queues: <String>['default'],
    capabilities: <String>['example:task'],
    slots: 4,
  ),
  handlers: <String, Handler>{
    'example:task': (TaskContext context) async {
      await context.emit('started');
      context.checkCancelled();

      // Fence external writes with context.fencingToken.
      return <String, Object?>{'ok': true};
    },
  },
);

final summary = await worker.run();
```

`TaskContext.cancellation` must be observed during long-running work. A worker
whose step heartbeat is rejected or becomes uncertain cancels the context and
will not report completion or failure under the stale lease generation.
