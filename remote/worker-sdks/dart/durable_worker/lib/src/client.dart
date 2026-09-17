import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'api.dart';
import 'models.dart';

const Set<int> _transientStatuses = <int>{
  HttpStatus.requestTimeout,
  425,
  HttpStatus.tooManyRequests,
  HttpStatus.internalServerError,
  HttpStatus.badGateway,
  HttpStatus.serviceUnavailable,
  HttpStatus.gatewayTimeout,
};

class DurableWorkerClient implements WorkerApi {
  DurableWorkerClient(
    String baseUrl,
    String authSecret, {
    String authHeader = 'X-Worker-Auth',
    Duration timeout = const Duration(seconds: 30),
    int maxRetries = 3,
    Duration initialBackoff = const Duration(milliseconds: 200),
    Duration maxBackoff = const Duration(seconds: 5),
    int maxResponseBytes = 2 * 1024 * 1024,
    HttpClient? httpClient,
    Future<void> Function(Duration duration)? sleep,
    Random? random,
  })  : _baseUrl = _parseBaseUrl(baseUrl),
        _authSecret = _validateAuthSecret(authSecret),
        _authHeader = _validateAuthHeader(authHeader),
        _timeout = _positiveDuration(timeout, 'timeout'),
        _maxRetries = _nonNegativeInt(maxRetries, 'maxRetries'),
        _initialBackoff =
            _nonNegativeDuration(initialBackoff, 'initialBackoff'),
        _maxBackoff = _nonNegativeDuration(maxBackoff, 'maxBackoff'),
        _maxResponseBytes = _positiveInt(maxResponseBytes, 'maxResponseBytes'),
        _httpClient = httpClient ?? HttpClient(),
        _ownsHttpClient = httpClient == null,
        _sleep = sleep ?? Future<void>.delayed,
        _random = random ?? Random.secure() {
    if (_maxBackoff < _initialBackoff) {
      throw ArgumentError.value(
        maxBackoff,
        'maxBackoff',
        'must be greater than or equal to initialBackoff',
      );
    }
    _httpClient.connectionTimeout = _timeout;
  }

  static const String _userAgent = 'oresoftware-durable-worker-dart/0.1.0';

  final Uri _baseUrl;
  final String _authSecret;
  final String _authHeader;
  final Duration _timeout;
  final int _maxRetries;
  final Duration _initialBackoff;
  final Duration _maxBackoff;
  final int _maxResponseBytes;
  final HttpClient _httpClient;
  final bool _ownsHttpClient;
  final Future<void> Function(Duration duration) _sleep;
  final Random _random;

  static Uri _parseBaseUrl(String value) {
    final Uri uri = Uri.parse(value);
    if (!uri.hasScheme ||
        (uri.scheme != 'http' && uri.scheme != 'https') ||
        uri.host.isEmpty) {
      throw ArgumentError.value(
          value, 'baseUrl', 'must be an absolute HTTP URL');
    }
    if (uri.userInfo.isNotEmpty || uri.hasQuery || uri.hasFragment) {
      throw ArgumentError.value(
        value,
        'baseUrl',
        'must not contain credentials, a query, or a fragment',
      );
    }
    final String normalized = uri.toString().replaceFirst(RegExp(r'/+$'), '');
    return Uri.parse(normalized);
  }

  static String _validateAuthSecret(String value) {
    if (value.trim().isEmpty || value.contains('\r') || value.contains('\n')) {
      throw ArgumentError.value(
        '<redacted>',
        'authSecret',
        'must be a non-empty single-line value',
      );
    }
    return value;
  }

  static String _validateAuthHeader(String value) {
    final RegExp token = RegExp(r"^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$");
    if (!token.hasMatch(value)) {
      throw ArgumentError.value(value, 'authHeader', 'must be an HTTP token');
    }
    return value;
  }

  static int _nonNegativeInt(int value, String name) {
    if (value < 0) {
      throw ArgumentError.value(value, name, 'must be non-negative');
    }
    return value;
  }

  static int _positiveInt(int value, String name) {
    if (value < 1) {
      throw ArgumentError.value(value, name, 'must be positive');
    }
    return value;
  }

  static Duration _positiveDuration(Duration value, String name) {
    if (value <= Duration.zero) {
      throw ArgumentError.value(value, name, 'must be positive');
    }
    return value;
  }

  static Duration _nonNegativeDuration(Duration value, String name) {
    if (value.isNegative) {
      throw ArgumentError.value(value, name, 'must be non-negative');
    }
    return value;
  }

  void close({bool force = true}) {
    if (_ownsHttpClient) {
      _httpClient.close(force: force);
    }
  }

  Future<JsonObject> submitTask(JsonObject task) {
    final String? identity = _nonEmptyString(task['idempotencyKey']);
    return _request(
      'POST',
      '/api/v1/tasks',
      task,
      idempotent: identity != null,
    );
  }

  Future<JsonObject> submitRun(JsonObject run) {
    final String? identity = _nonEmptyString(run['idempotencyKey']);
    return _request(
      'POST',
      '/api/v1/runs',
      run,
      idempotent: identity != null,
    );
  }

  Future<JsonObject> getRun(String runId) {
    return _request(
      'GET',
      '/api/v1/runs/${_segment(runId)}',
      null,
      idempotent: true,
    );
  }

  Future<JsonObject> signalRun(
    String runId,
    String signalName, [
    JsonObject payload = const <String, Object?>{},
  ]) {
    return _request(
      'POST',
      '/api/v1/runs/${_segment(runId)}/signals/${_segment(signalName)}',
      <String, Object?>{'payload': cloneJson(payload)},
      idempotent: false,
    );
  }

  Future<JsonObject> pauseRun(String runId) => _runMutation(runId, 'pause');

  Future<JsonObject> resumeRun(String runId) => _runMutation(runId, 'resume');

  Future<JsonObject> cancelRun(String runId) => _runMutation(runId, 'cancel');

  Future<JsonObject> _runMutation(String runId, String operation) {
    return _request(
      'POST',
      '/api/v1/runs/${_segment(runId)}/$operation',
      const <String, Object?>{},
      idempotent: true,
    );
  }

  @override
  Future<void> registerWorker(WorkerRegistration registration) async {
    await _request(
      'POST',
      '/api/v1/workers/register',
      registration.toJson(),
      idempotent: true,
    );
  }

  @override
  Future<void> heartbeatWorker(String workerId, {bool? drain}) async {
    await _request(
      'POST',
      '/api/v1/workers/${_segment(workerId)}/heartbeat',
      <String, Object?>{
        if (drain != null) 'drain': drain,
      },
      idempotent: true,
    );
  }

  @override
  Future<WorkerPoll> pollWorker(
    String workerId, {
    required Duration wait,
  }) async {
    if (wait.isNegative) {
      throw ArgumentError.value(wait, 'wait', 'must be non-negative');
    }
    final JsonObject response = await _request(
      'POST',
      '/api/v1/workers/${_segment(workerId)}/poll'
          '?waitMs=${wait.inMilliseconds}',
      const <String, Object?>{},
      idempotent: false,
    );
    return WorkerPoll.fromJson(response);
  }

  @override
  Future<void> startStep(String stepId, Lease lease) async {
    await _leaseMutation(stepId, 'start', lease);
  }

  @override
  Future<void> heartbeatStep(String stepId, Lease lease) async {
    await _leaseMutation(stepId, 'heartbeat', lease);
  }

  Future<void> _leaseMutation(
    String stepId,
    String operation,
    Lease lease,
  ) async {
    await _request(
      'POST',
      '/api/v1/steps/${_segment(stepId)}/$operation',
      lease.toJson(),
      idempotent: true,
      leaseSensitive: true,
    );
  }

  @override
  Future<void> appendStepOutput(String stepId, StepOutput output) async {
    await _request(
      'POST',
      '/api/v1/steps/${_segment(stepId)}/output',
      output.toJson(),
      idempotent: output.chunkId.trim().isNotEmpty,
      leaseSensitive: true,
    );
  }

  @override
  Future<void> completeStep(
    String stepId,
    StepCompletion completion,
  ) async {
    await _request(
      'POST',
      '/api/v1/steps/${_segment(stepId)}/complete',
      completion.toJson(),
      idempotent: true,
      leaseSensitive: true,
    );
  }

  @override
  Future<void> failStep(String stepId, StepFailure failure) async {
    await _request(
      'POST',
      '/api/v1/steps/${_segment(stepId)}/fail',
      failure.toJson(),
      idempotent: true,
      leaseSensitive: true,
    );
  }

  Future<JsonObject> _request(
    String method,
    String path,
    JsonObject? payload, {
    required bool idempotent,
    bool leaseSensitive = false,
  }) async {
    if (!path.startsWith('/')) {
      throw ArgumentError.value(path, 'path', 'must start with /');
    }
    final List<int>? encodedPayload =
        payload == null ? null : utf8.encode(jsonEncode(payload));
    final int attempts = idempotent ? _maxRetries + 1 : 1;

    for (int attempt = 0; attempt < attempts; attempt += 1) {
      HttpClientResponse response;
      try {
        response = await _send(method, path, encodedPayload);
      } on TimeoutException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_timeout',
          message: 'durable-worker request timed out',
          retryable: true,
          cause: error,
        );
      } on SocketException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_error',
          message: 'durable-worker request failed: $error',
          retryable: true,
          cause: error,
        );
      } on HttpException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_error',
          message: 'durable-worker request failed: $error',
          retryable: true,
          cause: error,
        );
      } on IOException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_error',
          message: 'durable-worker request failed: $error',
          retryable: true,
          cause: error,
        );
      }

      late final JsonObject body;
      try {
        body = await _decodeResponse(
          response,
          strictJson: response.statusCode >= 200 && response.statusCode < 300,
        );
      } on TimeoutException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_timeout',
          message: 'durable-worker response body timed out',
          retryable: true,
          cause: error,
        );
      } on SocketException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_error',
          message: 'durable-worker response body failed: $error',
          retryable: true,
          cause: error,
        );
      } on HttpException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_error',
          message: 'durable-worker response body failed: $error',
          retryable: true,
          cause: error,
        );
      } on IOException catch (error) {
        if (idempotent && attempt + 1 < attempts) {
          await _sleep(_backoff(attempt, null));
          continue;
        }
        throw DurableWorkerException(
          code: 'transport_error',
          message: 'durable-worker response body failed: $error',
          retryable: true,
          cause: error,
        );
      }
      if (response.statusCode >= 200 && response.statusCode < 300) {
        return body;
      }

      final DurableWorkerException protocolError = _httpError(
        response.statusCode,
        body,
        leaseSensitive: leaseSensitive,
      );
      if (idempotent &&
          attempt + 1 < attempts &&
          _transientStatuses.contains(response.statusCode) &&
          protocolError.retryable) {
        await _sleep(
          _backoff(attempt, response.headers.value('retry-after')),
        );
        continue;
      }
      throw protocolError;
    }
    throw StateError('durable-worker retry loop exhausted unexpectedly');
  }

  Future<HttpClientResponse> _send(
    String method,
    String path,
    List<int>? payload,
  ) async {
    final Uri uri = Uri.parse('${_baseUrl.toString()}$path');
    final HttpClientRequest request =
        await _httpClient.openUrl(method, uri).timeout(_timeout);
    request.followRedirects = false;
    request.headers.set(_authHeader, _authSecret);
    request.headers.set(HttpHeaders.acceptHeader, ContentType.json.mimeType);
    request.headers.set(HttpHeaders.userAgentHeader, _userAgent);
    if (payload != null) {
      request.headers.contentType = ContentType.json;
      request.contentLength = payload.length;
      request.add(payload);
    }
    return request.close().timeout(_timeout);
  }

  Future<JsonObject> _decodeResponse(
    HttpClientResponse response, {
    required bool strictJson,
  }) async {
    if (response.contentLength > _maxResponseBytes) {
      throw DurableWorkerException(
        code: 'response_too_large',
        message: 'durable-worker response exceeds configured limit',
      );
    }

    final BytesBuilder bytes = BytesBuilder(copy: false);
    int count = 0;
    await for (final List<int> chunk in response.timeout(_timeout)) {
      count += chunk.length;
      if (count > _maxResponseBytes) {
        throw DurableWorkerException(
          code: 'response_too_large',
          message: 'durable-worker response exceeds configured limit',
        );
      }
      bytes.add(chunk);
    }

    final Uint8List body = bytes.takeBytes();
    if (body.isEmpty) {
      return <String, Object?>{};
    }
    try {
      final Object? decoded = jsonDecode(utf8.decode(body));
      if (decoded is! Map<Object?, Object?>) {
        throw const FormatException('response JSON must be an object');
      }
      return objectValue(decoded);
    } on FormatException catch (error) {
      if (!strictJson) {
        return <String, Object?>{};
      }
      throw DurableWorkerException(
        code: 'invalid_response',
        message: 'durable-worker returned invalid object JSON',
        cause: error,
      );
    }
  }

  DurableWorkerException _httpError(
    int status,
    JsonObject body, {
    required bool leaseSensitive,
  }) {
    final String code = _nonEmptyString(body['code']) ?? 'http_error';
    final String message = _nonEmptyString(body['message']) ??
        'durable-worker returned HTTP $status';
    final bool retryable = body['retryable'] is bool
        ? body['retryable']! as bool
        : _transientStatuses.contains(status);
    if (leaseSensitive &&
        (status == HttpStatus.notFound || status == HttpStatus.conflict)) {
      return LeaseLostException(
        code: code,
        message: message,
        status: status,
        retryable: retryable,
      );
    }
    return DurableWorkerException(
      code: code,
      message: message,
      status: status,
      retryable: retryable,
    );
  }

  Duration _backoff(int attempt, String? retryAfter) {
    if (retryAfter != null) {
      final double? seconds = double.tryParse(retryAfter.trim());
      if (seconds != null && seconds >= 0) {
        final int milliseconds = (seconds * 1000).round();
        final Duration requested = Duration(milliseconds: milliseconds);
        return requested > _maxBackoff ? _maxBackoff : requested;
      }
    }

    int ceiling = _initialBackoff.inMilliseconds;
    for (int index = 0; index < attempt; index += 1) {
      ceiling = min(ceiling * 2, _maxBackoff.inMilliseconds);
    }
    ceiling = min(ceiling, _maxBackoff.inMilliseconds);
    if (ceiling <= 0) {
      return Duration.zero;
    }
    final int floor = ceiling ~/ 2;
    final int jitter =
        floor == ceiling ? 0 : _random.nextInt((ceiling - floor) + 1);
    return Duration(milliseconds: floor + jitter);
  }

  static String _segment(String value) {
    if (value.trim().isEmpty) {
      throw ArgumentError.value(value, 'identifier', 'must be non-empty');
    }
    return Uri.encodeComponent(value);
  }

  static String? _nonEmptyString(Object? value) {
    if (value is String && value.trim().isNotEmpty) {
      return value;
    }
    return null;
  }
}
