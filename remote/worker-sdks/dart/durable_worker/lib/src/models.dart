typedef JsonObject = Map<String, Object?>;

JsonObject cloneJson(JsonObject value) => Map<String, Object?>.from(value);

String requiredString(JsonObject value, String key) {
  final Object? raw = value[key];
  if (raw is! String || raw.trim().isEmpty) {
    throw FormatException('$key must be a non-empty string');
  }
  return raw;
}

String? optionalString(JsonObject value, String key) {
  final Object? raw = value[key];
  if (raw == null) {
    return null;
  }
  if (raw is! String) {
    throw FormatException('$key must be a string');
  }
  return raw;
}

int intValue(Object? value, {int fallback = 0}) {
  if (value is int) {
    return value;
  }
  if (value is num) {
    return value.toInt();
  }
  if (value is String) {
    return int.tryParse(value) ?? fallback;
  }
  return fallback;
}

bool boolValue(Object? value, {bool fallback = false}) {
  return value is bool ? value : fallback;
}

JsonObject objectValue(Object? value, {JsonObject? fallback}) {
  if (value is Map<String, Object?>) {
    return cloneJson(value);
  }
  if (value is Map<Object?, Object?>) {
    final JsonObject result = <String, Object?>{};
    for (final MapEntry<Object?, Object?> entry in value.entries) {
      final Object? key = entry.key;
      if (key is! String) {
        throw const FormatException('JSON object keys must be strings');
      }
      result[key] = entry.value;
    }
    return result;
  }
  return cloneJson(fallback ?? <String, Object?>{});
}

class DurableWorkerException implements Exception {
  DurableWorkerException({
    required this.code,
    required this.message,
    this.status,
    this.retryable = false,
    this.cause,
  });

  final String code;
  final String message;
  final int? status;
  final bool retryable;
  final Object? cause;

  @override
  String toString() {
    final String statusText = status == null ? '' : ' (HTTP $status)';
    return 'durable-worker $code$statusText: $message';
  }
}

class LeaseLostException extends DurableWorkerException {
  LeaseLostException({
    required super.code,
    required super.message,
    super.status,
    super.retryable,
    super.cause,
  });
}

bool isLeaseLost(Object error) => error is LeaseLostException;

class Lease {
  const Lease({
    required this.workerId,
    required this.leaseToken,
    required this.leaseGeneration,
  });

  final String workerId;
  final String leaseToken;
  final int leaseGeneration;

  JsonObject toJson() => <String, Object?>{
        'workerId': workerId,
        'leaseToken': leaseToken,
        'leaseGeneration': leaseGeneration,
      };
}

class Assignment {
  const Assignment({
    required this.runId,
    required this.stepId,
    required this.stepKey,
    required this.taskType,
    required this.queue,
    required this.input,
    required this.attempt,
    required this.leaseToken,
    required this.leaseGeneration,
    required this.fencingToken,
    required this.leaseExpiresAtMs,
    required this.timeoutMs,
    required this.affinityKey,
    required this.raw,
  });

  factory Assignment.fromJson(JsonObject value) {
    final Assignment assignment = Assignment(
      runId: requiredString(value, 'runId'),
      stepId: requiredString(value, 'stepId'),
      stepKey: requiredString(value, 'stepKey'),
      taskType: requiredString(value, 'taskType'),
      queue: requiredString(value, 'queue'),
      input: objectValue(value['input']),
      attempt: intValue(value['attempt']),
      leaseToken: requiredString(value, 'leaseToken'),
      leaseGeneration: intValue(value['leaseGeneration']),
      fencingToken: intValue(value['fencingToken']),
      leaseExpiresAtMs: intValue(value['leaseExpiresAtMs']),
      timeoutMs: intValue(value['timeoutMs']),
      affinityKey: value['affinityKey'],
      raw: cloneJson(value),
    );
    if (assignment.leaseGeneration < 1 || assignment.fencingToken < 1) {
      throw const FormatException(
        'assignment leaseGeneration and fencingToken must be positive',
      );
    }
    if (assignment.timeoutMs < 0) {
      throw const FormatException('assignment timeoutMs must be non-negative');
    }
    return assignment;
  }

  final String runId;
  final String stepId;
  final String stepKey;
  final String taskType;
  final String queue;
  final JsonObject input;
  final int attempt;
  final String leaseToken;
  final int leaseGeneration;
  final int fencingToken;
  final int leaseExpiresAtMs;
  final int timeoutMs;
  final Object? affinityKey;
  final JsonObject raw;
}

class WorkerRegistration {
  const WorkerRegistration({
    required this.workerId,
    required this.queues,
    required this.capabilities,
    required this.labels,
    required this.slots,
    required this.ttlMs,
    required this.drain,
  });

  final String workerId;
  final List<String> queues;
  final List<String> capabilities;
  final JsonObject labels;
  final int slots;
  final int ttlMs;
  final bool drain;

  JsonObject toJson() => <String, Object?>{
        'workerId': workerId,
        'queues': List<String>.from(queues),
        'capabilities': List<String>.from(capabilities),
        'labels': cloneJson(labels),
        'slots': slots,
        'ttlMs': ttlMs,
        'drain': drain,
      };
}

class WorkerPoll {
  const WorkerPoll({required this.assignment, required this.retryAfterMs});

  factory WorkerPoll.fromJson(JsonObject value) {
    final Object? rawAssignment = value['assignment'];
    return WorkerPoll(
      assignment: rawAssignment == null
          ? null
          : Assignment.fromJson(objectValue(rawAssignment)),
      retryAfterMs: intValue(value['retryAfterMs'], fallback: 100),
    );
  }

  final Assignment? assignment;
  final int retryAfterMs;
}

class StepOutput {
  const StepOutput({
    required this.lease,
    required this.chunkId,
    required this.chunk,
    required this.stream,
    required this.finalChunk,
  });

  final Lease lease;
  final String chunkId;
  final String chunk;
  final String stream;
  final bool finalChunk;

  JsonObject toJson() => <String, Object?>{
        ...lease.toJson(),
        'chunkId': chunkId,
        'chunk': chunk,
        'stream': stream,
        'finalChunk': finalChunk,
      };
}

class StepCompletion {
  const StepCompletion({required this.lease, required this.result});

  final Lease lease;
  final JsonObject result;

  JsonObject toJson() => <String, Object?>{
        ...lease.toJson(),
        'result': cloneJson(result),
      };
}

class StepFailure {
  const StepFailure({
    required this.lease,
    required this.code,
    required this.message,
    required this.retryable,
  });

  final Lease lease;
  final String code;
  final String message;
  final bool retryable;

  JsonObject toJson() => <String, Object?>{
        ...lease.toJson(),
        'code': code,
        'message': message,
        'retryable': retryable,
      };
}
