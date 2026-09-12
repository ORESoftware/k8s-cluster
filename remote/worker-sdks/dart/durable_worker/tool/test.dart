import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:oresoftware_durable_worker/durable_worker.dart';

typedef AsyncTest = Future<void> Function();

Future<void> main(List<String> arguments) async {
  final bool fencingOnly = arguments.contains('--fencing-only');
  final List<(String, AsyncTest)> tests = fencingOnly
      ? <(String, AsyncTest)>[
          (
            'heartbeat fencing cancels handlers and suppresses terminal writes',
            _heartbeatFencing,
          ),
          (
            'output fencing cancels handlers and suppresses terminal writes',
            _outputFencing,
          ),
        ]
      : <(String, AsyncTest)>[
          ('shared protocol fixture remains compatible', _protocolFixture),
          (
            'client retry, redirect, body, and lease boundaries',
            _clientContract
          ),
          (
            'worker streams deterministic progress and completes',
            _workerCompletion,
          ),
          (
            'heartbeat fencing cancels handlers and suppresses terminal writes',
            _heartbeatFencing,
          ),
          (
            'output fencing cancels handlers and suppresses terminal writes',
            _outputFencing,
          ),
          ('handler retryability is preserved', _handlerFailure),
        ];

  int passed = 0;
  for (final (String name, AsyncTest body) in tests) {
    try {
      await body();
      stdout.writeln('ok - $name');
      passed += 1;
    } on Object catch (error, stackTrace) {
      stderr.writeln('not ok - $name');
      stderr.writeln(error);
      stderr.writeln(stackTrace);
      exitCode = 1;
      return;
    }
  }
  stdout.writeln('$passed tests passed');
}

void _expect(bool condition, String message) {
  if (!condition) {
    throw StateError(message);
  }
}

Future<T> _expectThrows<T extends Object>(
  FutureOr<void> Function() operation, {
  bool Function(T error)? where,
}) async {
  try {
    await operation();
  } on Object catch (error) {
    if (error is! T) {
      throw StateError('expected $T but caught ${error.runtimeType}: $error');
    }
    if (where != null && !where(error)) {
      throw StateError('caught $T but predicate rejected $error');
    }
    return error;
  }
  throw StateError('expected $T');
}

Future<void> _protocolFixture() async {
  final File fixture = File('../../fixtures/durable-worker-protocol-v1.json');
  final JsonObject payload =
      objectValue(jsonDecode(await fixture.readAsString()));
  _expect(payload['version'] == 1, 'fixture version drifted');
  _expect(payload['delivery'] == 'at-least-once', 'delivery contract drifted');
  _expect(
    payload['progressChunkId'] == '{stepId}:{leaseGeneration}:{sequence}',
    'progress identity drifted',
  );
  final List<Object?> fragments =
      List<Object?>.from(payload['endpointFragments']! as List<Object?>);
  for (final String fragment in <String>[
    '/api/v1/tasks',
    '/api/v1/runs',
    '/api/v1/workers/register',
    '/poll?waitMs=',
    '/output',
    '/complete',
    '/fail',
  ]) {
    _expect(fragments.contains(fragment), 'fixture missing $fragment');
  }
  final Assignment assignment =
      Assignment.fromJson(objectValue(payload['assignment']));
  _expect(assignment.leaseGeneration == 3, 'fixture lease generation drifted');
  _expect(assignment.fencingToken == 9, 'fixture fencing token drifted');
}

Future<void> _clientContract() async {
  int idempotentRequests = 0;
  int ambiguousRequests = 0;
  int redirectedRequests = 0;
  int bodyTimeoutRequests = 0;

  final HttpServer redirectTarget =
      await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  Future<void> handleRedirect(HttpRequest request) async {
    redirectedRequests += 1;
    request.response
      ..statusCode = HttpStatus.ok
      ..headers.contentType = ContentType.json
      ..write('{}');
    await request.response.close();
  }

  final StreamSubscription<HttpRequest> redirectSubscription =
      redirectTarget.listen((HttpRequest request) {
    unawaited(handleRedirect(request));
  });

  final HttpServer server =
      await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  Future<void> handleRequest(HttpRequest request) async {
    final String body = await utf8.decoder.bind(request).join();
    final JsonObject payload =
        body.isEmpty ? <String, Object?>{} : objectValue(jsonDecode(body));

    if (request.headers.value('X-Worker-Auth') != 'test-secret') {
      request.response
        ..statusCode = HttpStatus.unauthorized
        ..headers.contentType = ContentType.json
        ..write('{"code":"unauthorized","message":"missing auth"}');
      await request.response.close();
      return;
    }

    if (request.uri.path == '/api/v1/tasks') {
      if (payload['idempotencyKey'] == 'task:stable') {
        idempotentRequests += 1;
        if (idempotentRequests == 1) {
          request.response
            ..statusCode = HttpStatus.serviceUnavailable
            ..headers.contentType = ContentType.json
            ..write(
              '{"code":"temporarily_unavailable",'
              '"message":"retry","retryable":true}',
            );
        } else {
          request.response
            ..statusCode = HttpStatus.ok
            ..headers.contentType = ContentType.json
            ..write('{"taskId":"task-1"}');
        }
      } else {
        ambiguousRequests += 1;
        request.response
          ..statusCode = HttpStatus.serviceUnavailable
          ..headers.contentType = ContentType.json
          ..write(
            '{"code":"temporarily_unavailable",'
            '"message":"do not retry","retryable":true}',
          );
      }
      await request.response.close();
      return;
    }

    if (request.uri.path == '/api/v1/runs/redirect') {
      request.response
        ..statusCode = HttpStatus.temporaryRedirect
        ..headers.set(
          HttpHeaders.locationHeader,
          'http://${redirectTarget.address.host}:${redirectTarget.port}/sink',
        );
      await request.response.close();
      return;
    }

    if (request.uri.path == '/api/v1/runs/large') {
      request.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json
        ..write('{"payload":"${List<String>.filled(256, "x").join()}"}');
      await request.response.close();
      return;
    }

    if (request.uri.path == '/api/v1/runs/non-object') {
      request.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json
        ..write('[]');
      await request.response.close();
      return;
    }

    if (request.uri.path == '/api/v1/runs/slow-body') {
      bodyTimeoutRequests += 1;
      request.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.json;
      if (bodyTimeoutRequests == 1) {
        request.response.write('{"partial":');
        await request.response.flush();
        await Future<void>.delayed(const Duration(milliseconds: 100));
        try {
          request.response.write('true}');
          await request.response.close();
        } on Object {
          // The first client intentionally abandons the timed-out body.
        }
      } else {
        request.response.write('{"ok":true}');
        await request.response.close();
      }
      return;
    }

    if (request.uri.path == '/api/v1/steps/fenced/heartbeat') {
      request.response
        ..statusCode = HttpStatus.conflict
        ..headers.contentType = ContentType.json
        ..write('{"code":"lease_lost","message":"fenced"}');
      await request.response.close();
      return;
    }

    request.response
      ..statusCode = HttpStatus.ok
      ..headers.contentType = ContentType.json
      ..write('{}');
    await request.response.close();
  }

  final StreamSubscription<HttpRequest> subscription =
      server.listen((HttpRequest request) {
    unawaited(handleRequest(request));
  });

  final DurableWorkerClient client = DurableWorkerClient(
    'http://${server.address.host}:${server.port}',
    'test-secret',
    initialBackoff: Duration.zero,
    maxBackoff: Duration.zero,
    maxResponseBytes: 128,
    random: _ZeroRandom(),
  );

  final DurableWorkerClient bodyTimeoutClient = DurableWorkerClient(
    'http://${server.address.host}:${server.port}',
    'test-secret',
    timeout: const Duration(milliseconds: 20),
    maxRetries: 1,
    initialBackoff: Duration.zero,
    maxBackoff: Duration.zero,
    maxResponseBytes: 128,
    random: _ZeroRandom(),
  );

  try {
    final JsonObject result = await client.submitTask(
      <String, Object?>{'idempotencyKey': 'task:stable'},
    );
    _expect(result['taskId'] == 'task-1', 'idempotent retry did not succeed');
    _expect(idempotentRequests == 2, 'idempotent call was not retried once');

    await _expectThrows<DurableWorkerException>(
      () => client.submitTask(<String, Object?>{'kind': 'ambiguous'}),
      where: (DurableWorkerException error) =>
          error.status == HttpStatus.serviceUnavailable,
    );
    _expect(ambiguousRequests == 1, 'unbound submission was retried');

    await _expectThrows<DurableWorkerException>(
      () => client.getRun('redirect'),
      where: (DurableWorkerException error) =>
          error.status == HttpStatus.temporaryRedirect,
    );
    _expect(redirectedRequests == 0, 'client followed a credentialed redirect');

    await _expectThrows<DurableWorkerException>(
      () => client.getRun('large'),
      where: (DurableWorkerException error) =>
          error.code == 'response_too_large',
    );

    await _expectThrows<DurableWorkerException>(
      () => client.getRun('non-object'),
      where: (DurableWorkerException error) => error.code == 'invalid_response',
    );

    final JsonObject recoveredBody =
        await bodyTimeoutClient.getRun('slow-body');
    _expect(
        recoveredBody['ok'] == true, 'idempotent body timeout did not recover');
    _expect(
        bodyTimeoutRequests == 2, 'body timeout was not retried exactly once');

    await _expectThrows<LeaseLostException>(
      () => client.heartbeatStep(
        'fenced',
        const Lease(
          workerId: 'worker-1',
          leaseToken: 'lease',
          leaseGeneration: 1,
        ),
      ),
    );
  } finally {
    bodyTimeoutClient.close();
    client.close();
    await subscription.cancel();
    await server.close(force: true);
    await redirectSubscription.cancel();
    await redirectTarget.close(force: true);
  }
}

class _ZeroRandom implements Random {
  @override
  bool nextBool() => false;

  @override
  double nextDouble() => 0;

  @override
  int nextInt(int max) => 0;
}

class _FakeApi implements WorkerApi {
  _FakeApi({Assignment? assignment}) : _assignment = assignment;

  Assignment? _assignment;
  final List<String> operations = <String>[];
  final List<String> outputChunkIds = <String>[];
  final Completer<void> heartbeatObserved = Completer<void>();
  bool fenceHeartbeat = false;
  bool fenceOutput = false;
  int heartbeatCount = 0;
  StepCompletion? completion;
  StepFailure? failure;

  @override
  Future<void> registerWorker(WorkerRegistration registration) async {
    operations.add('register');
  }

  @override
  Future<void> heartbeatWorker(String workerId, {bool? drain}) async {
    operations.add(drain == true ? 'worker-drain' : 'worker-heartbeat');
  }

  @override
  Future<WorkerPoll> pollWorker(
    String workerId, {
    required Duration wait,
  }) async {
    operations.add('poll');
    final Assignment? assignment = _assignment;
    _assignment = null;
    return WorkerPoll(assignment: assignment, retryAfterMs: 1);
  }

  @override
  Future<void> startStep(String stepId, Lease lease) async {
    operations.add('start');
  }

  @override
  Future<void> heartbeatStep(String stepId, Lease lease) async {
    operations.add('step-heartbeat');
    heartbeatCount += 1;
    if (!heartbeatObserved.isCompleted) {
      heartbeatObserved.complete();
    }
    if (fenceHeartbeat) {
      throw LeaseLostException(
        code: 'lease_lost',
        message: 'heartbeat fenced',
        status: HttpStatus.conflict,
      );
    }
  }

  @override
  Future<void> appendStepOutput(String stepId, StepOutput output) async {
    operations.add('output');
    outputChunkIds.add(output.chunkId);
    if (fenceOutput) {
      throw LeaseLostException(
        code: 'lease_lost',
        message: 'output fenced',
        status: HttpStatus.conflict,
      );
    }
  }

  @override
  Future<void> completeStep(
    String stepId,
    StepCompletion completion,
  ) async {
    operations.add('complete');
    this.completion = completion;
  }

  @override
  Future<void> failStep(String stepId, StepFailure failure) async {
    operations.add('fail');
    this.failure = failure;
  }
}

Assignment _assignment() => Assignment.fromJson(<String, Object?>{
      'runId': 'run-1',
      'stepId': 'step-1',
      'stepKey': 'task',
      'taskType': 'demo',
      'queue': 'default',
      'input': <String, Object?>{'value': 7},
      'attempt': 1,
      'leaseToken': 'lease-token',
      'leaseGeneration': 3,
      'fencingToken': 9,
      'leaseExpiresAtMs': 4102444800000,
      'timeoutMs': 60000,
      'affinityKey': null,
    });

WorkerConfig _config() => const WorkerConfig(
      workerId: 'dart-worker-1',
      queues: <String>['default'],
      capabilities: <String>['demo'],
      slots: 1,
      maxAssignments: 1,
      pollWait: Duration(milliseconds: 1),
      workerHeartbeat: Duration(milliseconds: 5),
      stepHeartbeat: Duration(milliseconds: 5),
      idleSleep: Duration(milliseconds: 1),
    );

Future<void> _workerCompletion() async {
  final _FakeApi api = _FakeApi(assignment: _assignment());
  final Worker worker = Worker(
    api: api,
    config: _config(),
    handlers: <String, Handler>{
      'demo': (TaskContext context) async {
        await context.emit('working');
        await Future<void>.delayed(const Duration(milliseconds: 15));
        await context.emit('done', finalChunk: true);
        return <String, Object?>{'answer': 14};
      },
    },
  );
  final WorkerSummary summary = await worker.run();
  _expect(summary.accepted == 1, 'assignment not accepted');
  _expect(summary.completed == 1, 'assignment not completed');
  _expect(summary.failed == 0, 'unexpected failure');
  _expect(api.heartbeatCount > 0, 'step heartbeat never ran');
  _expect(
    _listEquals(
      api.outputChunkIds,
      <String>['step-1:3:1', 'step-1:3:2'],
    ),
    'progress identities are not deterministic',
  );
  _expect(api.completion?.result['answer'] == 14, 'completion result missing');
  _expect(
    api.operations.last == 'worker-drain',
    'worker did not send a final drain heartbeat',
  );
}

Future<void> _heartbeatFencing() async {
  final _FakeApi api = _FakeApi(assignment: _assignment())
    ..fenceHeartbeat = true;
  bool cancellationObserved = false;
  final Worker worker = Worker(
    api: api,
    config: _config(),
    handlers: <String, Handler>{
      'demo': (TaskContext context) async {
        await api.heartbeatObserved.future;
        await context.cancellation.whenCancelled
            .timeout(const Duration(seconds: 1));
        cancellationObserved = true;
        context.checkCancelled();
        return <String, Object?>{};
      },
    },
  );
  final WorkerSummary summary = await worker.run();
  await Future<void>.delayed(Duration.zero);
  _expect(cancellationObserved, 'handler did not observe fencing cancellation');
  _expect(summary.leaseLost == 1, 'lease loss was not counted');
  _expect(!api.operations.contains('complete'), 'stale completion was emitted');
  _expect(!api.operations.contains('fail'), 'stale failure was emitted');
}

Future<void> _outputFencing() async {
  final _FakeApi api = _FakeApi(assignment: _assignment())..fenceOutput = true;
  final Worker worker = Worker(
    api: api,
    config: _config(),
    handlers: <String, Handler>{
      'demo': (TaskContext context) async {
        await context.emit('stale');
        return <String, Object?>{};
      },
    },
  );
  final WorkerSummary summary = await worker.run();
  _expect(summary.leaseLost == 1, 'output fencing was not counted');
  _expect(
    _listEquals(api.outputChunkIds, <String>['step-1:3:1']),
    'fenced output identity drifted',
  );
  _expect(!api.operations.contains('complete'), 'stale completion was emitted');
  _expect(!api.operations.contains('fail'), 'stale failure was emitted');
}

Future<void> _handlerFailure() async {
  final _FakeApi api = _FakeApi(assignment: _assignment());
  final Worker worker = Worker(
    api: api,
    config: _config(),
    handlers: <String, Handler>{
      'demo': (TaskContext context) async {
        throw const WorkerFailure(
          code: 'upstream_busy',
          message: 'try later',
          retryable: true,
        );
      },
    },
  );
  final WorkerSummary summary = await worker.run();
  _expect(summary.failed == 1, 'handler failure was not reported');
  _expect(api.failure?.code == 'upstream_busy', 'failure code drifted');
  _expect(api.failure?.retryable == true, 'retryability was not preserved');
}

bool _listEquals(List<String> left, List<String> right) {
  if (left.length != right.length) {
    return false;
  }
  for (int index = 0; index < left.length; index += 1) {
    if (left[index] != right[index]) {
      return false;
    }
  }
  return true;
}
