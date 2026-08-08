import 'dart:async';

import 'api.dart';
import 'models.dart';

typedef Handler = Future<JsonObject> Function(TaskContext context);

class WorkerFailure implements Exception {
  const WorkerFailure({
    required this.code,
    required this.message,
    this.retryable = false,
  });

  final String code;
  final String message;
  final bool retryable;

  @override
  String toString() => 'worker failure $code: $message';
}

class WorkerCancelledException implements Exception {
  const WorkerCancelledException([this.message = 'worker operation cancelled']);

  final String message;

  @override
  String toString() => message;
}

class WorkerTimeoutException implements Exception {
  const WorkerTimeoutException();

  @override
  String toString() => 'handler exceeded assignment timeout';
}

class CancellationToken {
  final Completer<Object> _cancelled = Completer<Object>();
  Object? _reason;

  bool get isCancelled => _cancelled.isCompleted;

  Object? get reason => _reason;

  Future<Object> get whenCancelled => _cancelled.future;

  void cancel([Object? reason]) {
    if (_cancelled.isCompleted) {
      return;
    }
    final Object resolved = reason ?? const WorkerCancelledException();
    _reason = resolved;
    _cancelled.complete(resolved);
  }

  void throwIfCancelled() {
    final Object? current = _reason;
    if (current is Error) {
      throw current;
    }
    if (current is Exception) {
      throw current;
    }
    if (current != null) {
      throw WorkerCancelledException(current.toString());
    }
  }
}

class WorkerConfig {
  const WorkerConfig({
    required this.workerId,
    required this.queues,
    this.capabilities = const <String>[],
    this.labels = const <String, Object?>{},
    this.slots = 1,
    this.ttl = const Duration(seconds: 45),
    this.pollWait = const Duration(seconds: 30),
    this.workerHeartbeat = const Duration(seconds: 15),
    this.stepHeartbeat = const Duration(seconds: 15),
    this.idleSleep = const Duration(milliseconds: 100),
    this.maxAssignments,
  });

  final String workerId;
  final List<String> queues;
  final List<String> capabilities;
  final JsonObject labels;
  final int slots;
  final Duration ttl;
  final Duration pollWait;
  final Duration workerHeartbeat;
  final Duration stepHeartbeat;
  final Duration idleSleep;
  final int? maxAssignments;

  void validate() {
    if (workerId.trim().isEmpty) {
      throw ArgumentError.value(workerId, 'workerId', 'must be non-empty');
    }
    if (queues.isEmpty || queues.any((String queue) => queue.trim().isEmpty)) {
      throw ArgumentError.value(queues, 'queues', 'must contain a queue');
    }
    if (slots < 1) {
      throw ArgumentError.value(slots, 'slots', 'must be positive');
    }
    if (ttl <= Duration.zero ||
        workerHeartbeat <= Duration.zero ||
        stepHeartbeat <= Duration.zero) {
      throw ArgumentError(
        'ttl, workerHeartbeat, and stepHeartbeat must be positive',
      );
    }
    if (pollWait.isNegative || idleSleep.isNegative) {
      throw ArgumentError('pollWait and idleSleep must be non-negative');
    }
    if (maxAssignments != null && maxAssignments! < 1) {
      throw ArgumentError.value(
        maxAssignments,
        'maxAssignments',
        'must be positive when present',
      );
    }
  }
}

class WorkerSummary {
  const WorkerSummary({
    required this.accepted,
    required this.completed,
    required this.failed,
    required this.leaseLost,
    required this.protocolErrors,
  });

  final int accepted;
  final int completed;
  final int failed;
  final int leaseLost;
  final int protocolErrors;
}

class _MutableSummary {
  int accepted = 0;
  int completed = 0;
  int failed = 0;
  int leaseLost = 0;
  int protocolErrors = 0;

  WorkerSummary freeze() => WorkerSummary(
        accepted: accepted,
        completed: completed,
        failed: failed,
        leaseLost: leaseLost,
        protocolErrors: protocolErrors,
      );
}

class TaskContext {
  TaskContext({
    required WorkerApi api,
    required this.assignment,
    required Lease lease,
    required CancellationToken cancellation,
  })  : _api = api,
        _lease = lease,
        _cancellation = cancellation;

  final WorkerApi _api;
  final Lease _lease;
  final CancellationToken _cancellation;
  final Assignment assignment;
  int _sequence = 0;

  JsonObject get input => cloneJson(assignment.input);

  String get runId => assignment.runId;

  String get stepId => assignment.stepId;

  int get fencingToken => assignment.fencingToken;

  CancellationToken get cancellation => _cancellation;

  void checkCancelled() => _cancellation.throwIfCancelled();

  Future<void> emit(
    String chunk, {
    String stream = 'progress',
    bool finalChunk = false,
    String? chunkId,
  }) async {
    checkCancelled();
    _sequence += 1;
    final String identity =
        chunkId ?? '${assignment.stepId}:${_lease.leaseGeneration}:$_sequence';
    try {
      await _api.appendStepOutput(
        assignment.stepId,
        StepOutput(
          lease: _lease,
          chunkId: identity,
          chunk: chunk,
          stream: stream,
          finalChunk: finalChunk,
        ),
      );
    } on LeaseLostException catch (error) {
      _cancellation.cancel(error);
      rethrow;
    }
  }
}

class Worker {
  Worker({
    required this.api,
    required Map<String, Handler> handlers,
    required this.config,
  }) : handlers = Map<String, Handler>.unmodifiable(handlers) {
    config.validate();
  }

  final WorkerApi api;
  final Map<String, Handler> handlers;
  final WorkerConfig config;

  Future<WorkerSummary> run({CancellationToken? cancellation}) async {
    final CancellationToken stop = cancellation ?? CancellationToken();
    await api.registerWorker(
      WorkerRegistration(
        workerId: config.workerId,
        queues: config.queues,
        capabilities: config.capabilities,
        labels: config.labels,
        slots: config.slots,
        ttlMs: config.ttl.inMilliseconds,
        drain: false,
      ),
    );

    final CancellationToken heartbeatStop = CancellationToken();
    final Completer<Object> fatalHeartbeat = Completer<Object>();
    final Future<void> workerHeartbeat = _workerHeartbeatLoop(
      heartbeatStop,
      fatalHeartbeat,
    );

    final _MutableSummary summary = _MutableSummary();
    final Set<Future<void>> active = <Future<void>>{};
    Object? loopError;

    while (!stop.isCancelled &&
        !fatalHeartbeat.isCompleted &&
        (config.maxAssignments == null ||
            summary.accepted < config.maxAssignments!)) {
      if (active.length >= config.slots) {
        await Future.any<void>(active);
        continue;
      }

      final Future<_PollOutcome> pollFuture = api
          .pollWorker(config.workerId, wait: config.pollWait)
          .then<_PollOutcome>(
            _PollOutcome.success,
            onError: _PollOutcome.error,
          );
      final List<Future<_PollOutcome>> candidates = <Future<_PollOutcome>>[
        pollFuture,
        stop.whenCancelled.then<_PollOutcome>(_PollOutcome.cancelled),
        fatalHeartbeat.future.then<_PollOutcome>(_PollOutcome.fatal),
      ];
      final _PollOutcome outcome = await Future.any<_PollOutcome>(candidates);
      if (outcome.kind == _PollOutcomeKind.cancelled) {
        break;
      }
      if (outcome.kind == _PollOutcomeKind.fatal ||
          outcome.kind == _PollOutcomeKind.error) {
        loopError = outcome.error;
        break;
      }

      final WorkerPoll poll = outcome.poll!;
      final Assignment? assignment = poll.assignment;
      if (assignment == null) {
        final Duration wait = poll.retryAfterMs > 0
            ? Duration(milliseconds: poll.retryAfterMs)
            : config.idleSleep;
        if (wait > Duration.zero) {
          await Future.any<Object>(<Future<Object>>[
            Future<void>.delayed(wait).then<Object>((_) => const Object()),
            stop.whenCancelled,
            fatalHeartbeat.future,
          ]);
          if (stop.isCancelled || fatalHeartbeat.isCompleted) {
            break;
          }
        }
        continue;
      }

      summary.accepted += 1;
      late final Future<void> task;
      task = _executeAssignment(assignment, summary).whenComplete(() {
        active.remove(task);
      });
      active.add(task);
    }

    if (active.isNotEmpty) {
      await Future.wait<void>(active.toList(), eagerError: false);
    }
    heartbeatStop.cancel();
    await workerHeartbeat;
    try {
      await api.heartbeatWorker(config.workerId, drain: true);
    } on Object {
      // Final drain is advisory after accepted tasks have completed.
    }
    if (loopError != null) {
      summary.protocolErrors += 1;
    }
    return summary.freeze();
  }

  Future<void> _workerHeartbeatLoop(
    CancellationToken stop,
    Completer<Object> fatal,
  ) async {
    while (!stop.isCancelled) {
      final Object outcome = await Future.any<Object>(<Future<Object>>[
        Future<void>.delayed(config.workerHeartbeat)
            .then<Object>((_) => const Object()),
        stop.whenCancelled,
      ]);
      if (stop.isCancelled || outcome is WorkerCancelledException) {
        return;
      }
      try {
        await api.heartbeatWorker(config.workerId, drain: false);
      } on DurableWorkerException catch (error) {
        if (error.retryable) {
          continue;
        }
        if (!fatal.isCompleted) {
          fatal.complete(error);
        }
        return;
      } on Object catch (error) {
        if (!fatal.isCompleted) {
          fatal.complete(error);
        }
        return;
      }
    }
  }

  Future<void> _executeAssignment(
    Assignment assignment,
    _MutableSummary summary,
  ) async {
    final Lease lease = Lease(
      workerId: config.workerId,
      leaseToken: assignment.leaseToken,
      leaseGeneration: assignment.leaseGeneration,
    );
    try {
      await api.startStep(assignment.stepId, lease);
    } on LeaseLostException {
      summary.leaseLost += 1;
      return;
    } on Object {
      summary.protocolErrors += 1;
      return;
    }

    final CancellationToken taskCancellation = CancellationToken();
    final CancellationToken heartbeatStop = CancellationToken();
    final Future<void> heartbeat = _stepHeartbeatLoop(
      assignment.stepId,
      lease,
      taskCancellation,
      heartbeatStop,
    );
    final TaskContext context = TaskContext(
      api: api,
      assignment: assignment,
      lease: lease,
      cancellation: taskCancellation,
    );

    final Handler? handler = handlers[assignment.taskType];
    final Future<_HandlerOutcome> handlerFuture = Future<JsonObject>.sync(() {
      if (handler == null) {
        throw WorkerFailure(
          code: 'handler_not_found',
          message: 'no handler registered for task type ${assignment.taskType}',
        );
      }
      return handler(context);
    }).then<_HandlerOutcome>(
      _HandlerOutcome.success,
      onError: _HandlerOutcome.error,
    );

    final List<Future<_HandlerOutcome>> candidates = <Future<_HandlerOutcome>>[
      handlerFuture,
      taskCancellation.whenCancelled
          .then<_HandlerOutcome>(_HandlerOutcome.cancelled),
      if (assignment.timeoutMs > 0)
        Future<void>.delayed(Duration(milliseconds: assignment.timeoutMs))
            .then<_HandlerOutcome>((_) => _HandlerOutcome.timeout()),
    ];
    final _HandlerOutcome outcome =
        await Future.any<_HandlerOutcome>(candidates);
    if (outcome.kind == _HandlerOutcomeKind.timeout) {
      taskCancellation.cancel(const WorkerTimeoutException());
    }

    heartbeatStop.cancel();
    await heartbeat;

    final Object? cancellation = taskCancellation.reason;
    if (cancellation != null) {
      if (isLeaseLost(cancellation)) {
        summary.leaseLost += 1;
        return;
      }
      if (cancellation is WorkerTimeoutException) {
        await _reportFailure(
          assignment,
          lease,
          summary,
          const WorkerFailure(
            code: 'handler_timeout',
            message: 'handler exceeded assignment timeout',
            retryable: true,
          ),
        );
        return;
      }
      summary.protocolErrors += 1;
      return;
    }

    if (outcome.kind == _HandlerOutcomeKind.error) {
      final Object error = outcome.error!;
      final WorkerFailure failure = error is WorkerFailure
          ? error
          : WorkerFailure(
              code: 'handler_error',
              message: error.toString(),
            );
      await _reportFailure(assignment, lease, summary, failure);
      return;
    }

    final JsonObject result = outcome.result ?? <String, Object?>{};
    try {
      await api.completeStep(
        assignment.stepId,
        StepCompletion(lease: lease, result: result),
      );
      summary.completed += 1;
    } on LeaseLostException {
      summary.leaseLost += 1;
    } on Object {
      summary.protocolErrors += 1;
    }
  }

  Future<void> _stepHeartbeatLoop(
    String stepId,
    Lease lease,
    CancellationToken taskCancellation,
    CancellationToken stop,
  ) async {
    while (!stop.isCancelled) {
      await Future.any<Object>(<Future<Object>>[
        Future<void>.delayed(config.stepHeartbeat)
            .then<Object>((_) => const Object()),
        stop.whenCancelled,
      ]);
      if (stop.isCancelled) {
        return;
      }
      try {
        await api.heartbeatStep(stepId, lease);
      } on LeaseLostException catch (error) {
        taskCancellation.cancel(error);
        return;
      } on Object catch (error) {
        taskCancellation.cancel(
          LeaseLostException(
            code: 'lease_heartbeat_uncertain',
            message: 'step heartbeat failed; lease authority is uncertain',
            retryable: true,
            cause: error,
          ),
        );
        return;
      }
    }
  }

  Future<void> _reportFailure(
    Assignment assignment,
    Lease lease,
    _MutableSummary summary,
    WorkerFailure failure,
  ) async {
    try {
      await api.failStep(
        assignment.stepId,
        StepFailure(
          lease: lease,
          code: failure.code,
          message: failure.message,
          retryable: failure.retryable,
        ),
      );
      summary.failed += 1;
    } on LeaseLostException {
      summary.leaseLost += 1;
    } on Object {
      summary.protocolErrors += 1;
    }
  }
}

enum _PollOutcomeKind { success, error, cancelled, fatal }

class _PollOutcome {
  const _PollOutcome._({
    required this.kind,
    this.poll,
    this.error,
    this.stackTrace,
  });

  factory _PollOutcome.success(WorkerPoll poll) =>
      _PollOutcome._(kind: _PollOutcomeKind.success, poll: poll);

  factory _PollOutcome.error(Object error, StackTrace stackTrace) =>
      _PollOutcome._(
        kind: _PollOutcomeKind.error,
        error: error,
        stackTrace: stackTrace,
      );

  factory _PollOutcome.cancelled(Object error) =>
      _PollOutcome._(kind: _PollOutcomeKind.cancelled, error: error);

  factory _PollOutcome.fatal(Object error) =>
      _PollOutcome._(kind: _PollOutcomeKind.fatal, error: error);

  final _PollOutcomeKind kind;
  final WorkerPoll? poll;
  final Object? error;
  final StackTrace? stackTrace;
}

enum _HandlerOutcomeKind { success, error, cancelled, timeout }

class _HandlerOutcome {
  const _HandlerOutcome._({
    required this.kind,
    this.result,
    this.error,
    this.stackTrace,
  });

  factory _HandlerOutcome.success(JsonObject result) =>
      _HandlerOutcome._(kind: _HandlerOutcomeKind.success, result: result);

  factory _HandlerOutcome.error(Object error, StackTrace stackTrace) =>
      _HandlerOutcome._(
        kind: _HandlerOutcomeKind.error,
        error: error,
        stackTrace: stackTrace,
      );

  factory _HandlerOutcome.cancelled(Object error) =>
      _HandlerOutcome._(kind: _HandlerOutcomeKind.cancelled, error: error);

  factory _HandlerOutcome.timeout() =>
      const _HandlerOutcome._(kind: _HandlerOutcomeKind.timeout);

  final _HandlerOutcomeKind kind;
  final JsonObject? result;
  final Object? error;
  final StackTrace? stackTrace;
}
