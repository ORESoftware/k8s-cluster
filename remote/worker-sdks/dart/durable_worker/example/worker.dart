import 'dart:io';

import 'package:oresoftware_durable_worker/durable_worker.dart';

Future<void> main() async {
  final String secret =
      Platform.environment['DURABLE_WORKER_AUTH_SECRET'] ?? '';
  if (secret.isEmpty) {
    stderr.writeln('DURABLE_WORKER_AUTH_SECRET is required');
    exitCode = 2;
    return;
  }

  final DurableWorkerClient client = DurableWorkerClient(
    Platform.environment['DURABLE_WORKER_URL'] ?? 'http://127.0.0.1:8152',
    secret,
  );
  final Worker worker = Worker(
    api: client,
    config: const WorkerConfig(
      workerId: 'dart-example-worker',
      queues: <String>['default'],
      capabilities: <String>['example:double'],
      slots: 2,
    ),
    handlers: <String, Handler>{
      'example:double': (TaskContext context) async {
        final Object? value = context.input['value'];
        if (value is! num) {
          throw const WorkerFailure(
            code: 'invalid_input',
            message: 'value must be numeric',
          );
        }
        await context.emit('doubling');
        context.checkCancelled();
        return <String, Object?>{'value': value * 2};
      },
    },
  );

  try {
    final WorkerSummary summary = await worker.run();
    stdout.writeln(
      'accepted=${summary.accepted} completed=${summary.completed} '
      'failed=${summary.failed} leaseLost=${summary.leaseLost}',
    );
  } finally {
    client.close();
  }
}
