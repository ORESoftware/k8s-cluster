/// Minimal, self-contained OpenTelemetry tracing for dd-dart-server.
///
/// There is no mature Dart OpenTelemetry SDK, so this is a hand-rolled OTLP/HTTP
/// exporter: it builds spans in memory and POSTs them as OTLP-JSON to the
/// collector's `/v1/traces` endpoint. The JSON envelope (resourceSpans →
/// scopeSpans → spans, with the `{intValue|doubleValue|boolValue|stringValue}`
/// attribute shape) matches the OTLP/HTTP-JSON protocol and mirrors the working
/// precedent in `deployments/dev-server/src/telemetry.ts`.
///
/// Nothing here patches the runtime: [traced] wraps a `dart:io` request handler
/// explicitly. It is wired into the coordinator's admin request loop in
/// `bin/server.dart`.
///
/// Configuration (env, all optional):
///   * `OTEL_EXPORTER_OTLP_ENDPOINT` — OTLP/HTTP base URL; defaults to the
///     in-cluster `dd-otel-collector`. `/v1/traces` is appended.
///   * `OTEL_SERVICE_NAME` — resource `service.name`; defaults to
///     `dd-dart-server`.
///   * `POD_NAME` / `POD_NAMESPACE` — stamped as `k8s.pod.name` /
///     `k8s.namespace.name` resource attributes (Kubernetes downward API).
///
/// Tracing is enabled only when an OTLP endpoint is resolvable (it always is by
/// default). Export is best-effort and fire-and-forget: a span builder failure
/// or an unreachable collector never disrupts request handling.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

/// OTLP span kind enum values (per the OTLP proto). We emit SERVER spans for
/// inbound HTTP requests.
const int _spanKindServer = 2;

/// OTLP status codes: 0 unset, 1 ok, 2 error.
const int _statusUnset = 0;
const int _statusError = 2;

final Random _idRng = Random.secure();

const String _defaultEndpoint =
    'http://dd-otel-collector.observability.svc.cluster.local:4318';

/// A single in-flight span. Created via [Telemetry.startSpan]; finished via
/// [end]. Attributes accumulate until the span ends, at which point the owning
/// [Telemetry] serialises and exports it.
class OtelSpan {
  OtelSpan._(
    this._owner, {
    required this.name,
    required this.traceId,
    required this.spanId,
    required this.parentSpanId,
  }) : startTimeUnixNano = _nowUnixNano();

  final Telemetry _owner;
  final String name;

  /// 32 lowercase hex chars (16 bytes).
  final String traceId;

  /// 16 lowercase hex chars (8 bytes).
  final String spanId;

  /// 16 lowercase hex chars, or empty when this is a root span.
  final String parentSpanId;

  final int startTimeUnixNano;
  int _endTimeUnixNano = 0;
  int _statusCode = _statusUnset;
  String _statusMessage = '';
  bool _ended = false;

  final Map<String, Object> _attributes = <String, Object>{};

  /// Sets a span attribute. Only String / int / double / bool are kept; other
  /// types are ignored so a bad value can never break serialisation.
  void setAttribute(String key, Object? value) {
    if (value is String) {
      _attributes[key] = value;
    } else if (value is bool) {
      _attributes[key] = value;
    } else if (value is int) {
      _attributes[key] = value;
    } else if (value is double) {
      _attributes[key] = value;
    }
  }

  /// Marks the span as errored and records a short message.
  void setError(String message) {
    _statusCode = _statusError;
    _statusMessage = message;
  }

  /// Ends the span and hands it to the exporter (fire-and-forget). Idempotent.
  void end() {
    if (_ended) return;
    _ended = true;
    _endTimeUnixNano = _nowUnixNano();
    _owner._export(this);
  }

  /// The W3C `traceparent` header value for this span, for outbound propagation
  /// (`00-<trace-id>-<span-id>-01`).
  String toTraceparent() => '00-$traceId-$spanId-01';

  Map<String, Object?> _toJson() {
    final attrs = <Map<String, Object?>>[
      for (final entry in _attributes.entries)
        {'key': entry.key, 'value': _otlpAnyValue(entry.value)},
    ];
    final json = <String, Object?>{
      'traceId': traceId,
      'spanId': spanId,
      'name': name,
      'kind': _spanKindServer,
      'startTimeUnixNano': startTimeUnixNano.toString(),
      'endTimeUnixNano': _endTimeUnixNano.toString(),
      'attributes': attrs,
      'status': {
        'code': _statusCode,
        if (_statusMessage.isNotEmpty) 'message': _statusMessage,
      },
    };
    if (parentSpanId.isNotEmpty) {
      json['parentSpanId'] = parentSpanId;
    }
    return json;
  }
}

/// Owns the OTLP exporter config + the resource attributes, and mints spans.
/// One instance lives on the coordinator isolate.
class Telemetry {
  Telemetry._({
    required this.enabled,
    required Uri? tracesUri,
    required List<Map<String, Object?>> resourceAttributes,
    required this.serviceName,
    Duration timeout = const Duration(milliseconds: 1500),
  })  : _tracesUri = tracesUri,
        _resourceAttributes = resourceAttributes,
        _http = (HttpClient()..connectionTimeout = timeout),
        _timeout = timeout;

  /// Builds a [Telemetry] from the process environment. Reads
  /// `OTEL_EXPORTER_OTLP_ENDPOINT` (default in-cluster collector),
  /// `OTEL_SERVICE_NAME`, and the `POD_NAME` / `POD_NAMESPACE` downward-API
  /// vars. Tracing is enabled whenever the endpoint parses to a valid URL.
  factory Telemetry.fromEnv() {
    final rawEndpoint =
        (Platform.environment['OTEL_EXPORTER_OTLP_ENDPOINT'] ?? _defaultEndpoint)
            .trim();
    final base = rawEndpoint.endsWith('/')
        ? rawEndpoint.substring(0, rawEndpoint.length - 1)
        : rawEndpoint;
    final serviceName =
        (Platform.environment['OTEL_SERVICE_NAME'] ?? 'dd-dart-server').trim();

    Uri? tracesUri;
    try {
      final parsed = Uri.parse('$base/v1/traces');
      if (parsed.hasScheme && parsed.host.isNotEmpty) {
        tracesUri = parsed;
      }
    } on FormatException {
      tracesUri = null;
    }

    final resourceAttributes = <Map<String, Object?>>[
      {'key': 'service.name', 'value': _otlpAnyValue(serviceName)},
    ];
    final podName = Platform.environment['POD_NAME']?.trim();
    if (podName != null && podName.isNotEmpty) {
      resourceAttributes
          .add({'key': 'k8s.pod.name', 'value': _otlpAnyValue(podName)});
    }
    final podNamespace = Platform.environment['POD_NAMESPACE']?.trim();
    if (podNamespace != null && podNamespace.isNotEmpty) {
      resourceAttributes.add(
          {'key': 'k8s.namespace.name', 'value': _otlpAnyValue(podNamespace)});
    }

    return Telemetry._(
      enabled: tracesUri != null,
      tracesUri: tracesUri,
      resourceAttributes: resourceAttributes,
      serviceName: serviceName,
    );
  }

  /// True when an OTLP endpoint resolved and spans will be exported.
  final bool enabled;
  final String serviceName;

  final Uri? _tracesUri;
  final List<Map<String, Object?>> _resourceAttributes;
  final HttpClient _http;
  final Duration _timeout;
  bool _closed = false;

  /// Starts a SERVER span named [name]. If [traceparent] is a valid W3C header
  /// the new span continues that trace (same trace id, header span as parent);
  /// otherwise a fresh root trace is started.
  OtelSpan startSpan(String name, {String? traceparent}) {
    final parent = _parseTraceparent(traceparent);
    final traceId = parent?.traceId ?? _randomHex(16);
    final parentSpanId = parent?.spanId ?? '';
    return OtelSpan._(
      this,
      name: name,
      traceId: traceId,
      spanId: _randomHex(8),
      parentSpanId: parentSpanId,
    );
  }

  void _export(OtelSpan span) {
    if (!enabled || _closed) return;
    final uri = _tracesUri;
    if (uri == null) return;
    // Fire-and-forget; failures are swallowed so tracing never affects the
    // request path. Metrics + logs still cover the runtime if traces drop.
    unawaited(_post(uri, span).catchError((Object _) {}));
  }

  Future<void> _post(Uri uri, OtelSpan span) async {
    final payload = <String, Object?>{
      'resourceSpans': [
        {
          'resource': {'attributes': _resourceAttributes},
          'scopeSpans': [
            {
              'scope': {'name': serviceName},
              'spans': [span._toJson()],
            },
          ],
        },
      ],
    };
    final req = await _http.postUrl(uri).timeout(_timeout);
    req.headers.contentType = ContentType.json;
    req.add(utf8.encode(jsonEncode(payload)));
    final resp = await req.close().timeout(_timeout);
    // Drain so the connection can be reused/released.
    await resp.drain<void>();
  }

  /// Closes the underlying HTTP client. Call on graceful shutdown.
  void close() {
    if (_closed) return;
    _closed = true;
    _http.close(force: true);
  }
}

/// Wraps a `dart:io` request handler in a SERVER span. The span is named
/// `"HTTP <METHOD> <path>"`, continues any inbound W3C `traceparent`, records
/// the response status code, and ends in a `finally` so it closes on every
/// path (success, thrown error, early return).
///
/// [statusOf] reads the status code back off the request after [handle] runs
/// (the coordinator/HTTP isolates write `req.response.statusCode` directly), so
/// the span captures the real outcome without changing handler control flow.
Future<void> traced(
  Telemetry telemetry,
  HttpRequest req,
  Future<void> Function() handle,
) async {
  if (!telemetry.enabled) {
    await handle();
    return;
  }
  final method = req.method.toUpperCase();
  final path = req.uri.path;
  final span = telemetry.startSpan(
    'HTTP $method $path',
    traceparent: req.headers.value('traceparent'),
  );
  span
    ..setAttribute('http.request.method', method)
    ..setAttribute('url.path', path)
    ..setAttribute('server.address', req.requestedUri.host);
  try {
    await handle();
  } catch (e) {
    span.setError(e.toString());
    rethrow;
  } finally {
    // statusCode is set by the handler before req.response.close(); reading it
    // here reflects the response actually produced.
    final status = req.response.statusCode;
    span.setAttribute('http.response.status_code', status);
    if (status >= 500) {
      span.setError('HTTP $status');
    }
    span.end();
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

class _ParentContext {
  const _ParentContext(this.traceId, this.spanId);
  final String traceId;
  final String spanId;
}

/// Parses a W3C `traceparent` (`00-<32hex>-<16hex>-<2hex>`). Returns null for
/// any malformed value, an all-zero trace/span id, or an unsupported version.
_ParentContext? _parseTraceparent(String? value) {
  if (value == null) return null;
  final parts = value.trim().split('-');
  if (parts.length < 4) return null;
  final version = parts[0];
  final traceId = parts[1].toLowerCase();
  final spanId = parts[2].toLowerCase();
  if (version.length != 2 || version == 'ff') return null;
  if (traceId.length != 32 || !_isHex(traceId) || _isAllZero(traceId)) {
    return null;
  }
  if (spanId.length != 16 || !_isHex(spanId) || _isAllZero(spanId)) {
    return null;
  }
  return _ParentContext(traceId, spanId);
}

bool _isHex(String s) {
  for (final c in s.codeUnits) {
    final isDigit = c >= 0x30 && c <= 0x39;
    final isLowerAf = c >= 0x61 && c <= 0x66;
    if (!isDigit && !isLowerAf) return false;
  }
  return true;
}

bool _isAllZero(String s) {
  for (final c in s.codeUnits) {
    if (c != 0x30) return false;
  }
  return true;
}

/// Random lowercase hex string of [bytes] bytes (2 hex chars per byte).
String _randomHex(int bytes) {
  const hex = '0123456789abcdef';
  final sb = StringBuffer();
  for (var i = 0; i < bytes; i++) {
    final b = _idRng.nextInt(256);
    sb
      ..write(hex[(b >> 4) & 0xf])
      ..write(hex[b & 0xf]);
  }
  return sb.toString();
}

int _nowUnixNano() => DateTime.now().microsecondsSinceEpoch * 1000;

/// Wraps a scalar into the OTLP AnyValue JSON shape. Integers serialise as
/// `intValue` (a string per the proto JSON mapping); doubles as `doubleValue`.
Map<String, Object?> _otlpAnyValue(Object value) {
  if (value is bool) return {'boolValue': value};
  if (value is int) return {'intValue': value.toString()};
  if (value is double) return {'doubleValue': value};
  return {'stringValue': value.toString()};
}
