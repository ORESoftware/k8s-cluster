/// Postgres connectivity for dd-dart-server.
///
/// Thin wrapper around [`package:postgres`'s `Pool`](https://pub.dev/packages/postgres)
/// that:
///
///  * is **opt-in** via `DATABASE_URL` (or `RDS_DATABASE_URL`) — when the
///    env var is unset the rest of the server runs in pure in-memory
///    mode. This keeps the WSS / SSR / hot-reload demos zero-dependency.
///  * speaks the canonical schema from `remote/libs/pg-defs` only —
///    callers are expected to consume `*SelectSql` constants and decode
///    via `*Row.fromJson` (after column-name normalisation).
///  * normalises Postgres `column_map` keys (snake_case + `_json`
///    suffix) into the camelCase keys the pg-defs Row factories expect.
///    See [normalisePgColumnMap].
///  * exposes a tiny [PgMetrics] surface so we can wire pool stats and
///    query-volume counters into `/metrics`.
///
/// The single import site for application code is `lib/db/pg_contract.dart`
/// — this module is the transport, that one is the contract.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:postgres/postgres.dart';
import 'package:rxdart/rxdart.dart';

import 'metrics.dart';

/// Per-pool counters wired into `/metrics` via [bindMetrics].
class PgMetrics {
  PgMetrics();

  int queries = 0;
  int queryErrors = 0;
  int rowsRead = 0;
  int connectionsOpened = 0;
  int connectionsClosed = 0;
  int notifyEventsReceived = 0;

  void bind(Metrics metrics) {
    metrics
      ..registerGauge('dart_pg_queries_total', () => queries)
      ..registerGauge('dart_pg_query_errors_total', () => queryErrors)
      ..registerGauge('dart_pg_rows_read_total', () => rowsRead)
      ..registerGauge('dart_pg_connections_opened_total', () => connectionsOpened)
      ..registerGauge('dart_pg_connections_closed_total', () => connectionsClosed)
      ..registerGauge('dart_pg_notify_events_total', () => notifyEventsReceived);
  }
}

/// Lightweight wrapper around `Pool` that adds metrics + helpers.
///
/// The pool is intentionally constructed lazily through [open] so we can
/// still boot the rest of the server even when Postgres is unreachable
/// (hot reload demo, local dev with no RDS, etc.).
class PgPool {
  PgPool._({
    required this.url,
    required this.pool,
    required this.metrics,
  });

  /// Original connection string (sanitised — the password is masked when
  /// surfaced via `serviceUri` getters).
  final String url;

  final Pool pool;
  final PgMetrics metrics;

  bool get isOpen => _disposed == false;
  bool _disposed = false;
  final _events = PublishSubject<PgEvent>();

  /// Stream of operational events (queries, errors, notify, lifecycle).
  Stream<PgEvent> get events => _events.stream;

  /// Open a pool from an env var URL. Returns null when the env var is
  /// not set OR when the URL fails to parse — the caller decides whether
  /// to fail the boot or run in degraded (DB-less) mode.
  static Future<PgPool?> open({
    required String? url,
    PgMetrics? metrics,
    int maxConnectionCount = 10,
    Duration maxConnectionAge = const Duration(hours: 1),
    Duration connectTimeout = const Duration(seconds: 5),
    Duration queryTimeout = const Duration(seconds: 30),
  }) async {
    if (url == null || url.trim().isEmpty) return null;
    final parsed = _ensurePoolDefaults(
      url.trim(),
      maxConnectionCount: maxConnectionCount,
      maxConnectionAge: maxConnectionAge,
      connectTimeout: connectTimeout,
      queryTimeout: queryTimeout,
    );
    final m = metrics ?? PgMetrics();
    try {
      final pool = Pool.withUrl(parsed);
      // Ping once so we fail fast at boot when the DB is unreachable.
      await pool.execute('select 1');
      m.connectionsOpened++;
      stderr.writeln(
        '[postgres] connected pool to ${_redact(parsed)} '
        '(max_connection_count=$maxConnectionCount)',
      );
      return PgPool._(url: parsed, pool: pool, metrics: m);
    } catch (e) {
      m.queryErrors++;
      stderr.writeln(
        '[postgres] pool open failed for ${_redact(parsed)}: $e',
      );
      return null;
    }
  }

  /// Run a SELECT and decode each row through [decode].
  ///
  /// `parameters` is forwarded to `package:postgres` substitution
  /// (use `Sql.named(...)` semantics — `@name` placeholders).
  Future<List<T>> selectRows<T>(
    String sql, {
    required T Function(Map<String, Object?> row) decode,
    Map<String, Object?>? parameters,
  }) async {
    if (_disposed) {
      throw StateError('PgPool has been closed');
    }
    final t0 = DateTime.now();
    try {
      final result = await pool.execute(
        Sql.named(sql),
        parameters: parameters,
      );
      metrics.queries++;
      metrics.rowsRead += result.length;
      _events.add(PgQueryEvent(
        sqlPreview: _preview(sql),
        durationUs: DateTime.now().difference(t0).inMicroseconds,
        rows: result.length,
      ));
      return result.map((row) {
        final raw = row.toColumnMap();
        return decode(normalisePgColumnMap(raw));
      }).toList(growable: false);
    } catch (e, st) {
      metrics.queries++;
      metrics.queryErrors++;
      _events.add(PgErrorEvent(
        sqlPreview: _preview(sql),
        error: '$e',
        stack: '$st',
      ));
      rethrow;
    }
  }

  /// Run a one-shot statement (INSERT / UPDATE / DELETE / DDL).
  ///
  /// Returns the row count (or 0 if the statement doesn't report one).
  Future<int> execute(
    String sql, {
    Map<String, Object?>? parameters,
  }) async {
    if (_disposed) {
      throw StateError('PgPool has been closed');
    }
    final t0 = DateTime.now();
    try {
      final result = await pool.execute(
        Sql.named(sql),
        parameters: parameters,
      );
      metrics.queries++;
      _events.add(PgQueryEvent(
        sqlPreview: _preview(sql),
        durationUs: DateTime.now().difference(t0).inMicroseconds,
        rows: result.affectedRows,
      ));
      return result.affectedRows;
    } catch (e, st) {
      metrics.queries++;
      metrics.queryErrors++;
      _events.add(PgErrorEvent(
        sqlPreview: _preview(sql),
        error: '$e',
        stack: '$st',
      ));
      rethrow;
    }
  }

  /// Run [body] inside a single pooled transaction.
  Future<R> withTransaction<R>(Future<R> Function(PgTransaction tx) body) async {
    if (_disposed) {
      throw StateError('PgPool has been closed');
    }
    return pool.runTx<R>((session) async {
      return body(PgTransaction._(session, metrics, _events));
    });
  }

  /// Quick smoke test for `/dart/admin/db` and the readyz path.
  Future<Map<String, Object?>> ping() async {
    final t0 = DateTime.now();
    try {
      final result = await pool.execute('select now() at time zone \'utc\' as now_utc');
      metrics.queries++;
      final row = result.first.toColumnMap();
      return {
        'ok': true,
        'duration_ms': DateTime.now().difference(t0).inMilliseconds,
        'now_utc': row['now_utc']?.toString(),
      };
    } catch (e) {
      metrics.queries++;
      metrics.queryErrors++;
      return {
        'ok': false,
        'duration_ms': DateTime.now().difference(t0).inMilliseconds,
        'error': '$e',
      };
    }
  }

  Future<void> close() async {
    if (_disposed) return;
    _disposed = true;
    metrics.connectionsClosed++;
    try {
      await pool.close();
    } catch (_) {/* swallow */}
    if (!_events.isClosed) await _events.close();
  }
}

/// Restricted handle handed to [PgPool.withTransaction] callbacks.
///
/// Mirrors the surface of [PgPool] for the in-tx case so callers can use
/// the same `selectRows` / `execute` shape. Named [PgTransaction] (not
/// `TxSession`) on purpose: `package:postgres` already exports a class
/// called `TxSession`, and shadowing it here would surprise readers.
class PgTransaction {
  PgTransaction._(this._session, this._metrics, this._events);

  final Session _session;
  final PgMetrics _metrics;
  final PublishSubject<PgEvent> _events;

  Future<List<T>> selectRows<T>(
    String sql, {
    required T Function(Map<String, Object?> row) decode,
    Map<String, Object?>? parameters,
  }) async {
    final t0 = DateTime.now();
    try {
      final result = await _session.execute(
        Sql.named(sql),
        parameters: parameters,
      );
      _metrics.queries++;
      _metrics.rowsRead += result.length;
      _events.add(PgQueryEvent(
        sqlPreview: _preview(sql),
        durationUs: DateTime.now().difference(t0).inMicroseconds,
        rows: result.length,
      ));
      return result.map((row) {
        final raw = row.toColumnMap();
        return decode(normalisePgColumnMap(raw));
      }).toList(growable: false);
    } catch (e, st) {
      _metrics.queries++;
      _metrics.queryErrors++;
      _events.add(PgErrorEvent(
        sqlPreview: _preview(sql),
        error: '$e',
        stack: '$st',
      ));
      rethrow;
    }
  }

  Future<int> execute(
    String sql, {
    Map<String, Object?>? parameters,
  }) async {
    final t0 = DateTime.now();
    try {
      final result = await _session.execute(
        Sql.named(sql),
        parameters: parameters,
      );
      _metrics.queries++;
      _events.add(PgQueryEvent(
        sqlPreview: _preview(sql),
        durationUs: DateTime.now().difference(t0).inMicroseconds,
        rows: result.affectedRows,
      ));
      return result.affectedRows;
    } catch (e, st) {
      _metrics.queries++;
      _metrics.queryErrors++;
      _events.add(PgErrorEvent(
        sqlPreview: _preview(sql),
        error: '$e',
        stack: '$st',
      ));
      rethrow;
    }
  }
}

/// Normalises a column map from `package:postgres` so it lines up with
/// the JSON keys pg-defs `*Row.fromJson` factories expect.
///
/// Rules (in order):
///   1. Strip the trailing `_json` suffix the pg-defs SELECT SQL uses for
///      `::text`-cast JSON columns (e.g. `value_json` → `value`,
///      `meta_data_json` → `meta_data`).
///   2. Convert remaining `snake_case` keys to `camelCase` so
///      `meta_data` → `metaData`, `is_soft_deleted` → `isSoftDeleted`.
///   3. JSON columns arrive as `String` (because pg-defs `::text`-casts
///      them in the SELECT) — keep them as String. The pg-defs row
///      factory will JSON-decode them via `_readRequiredObject`.
///
/// We don't recurse into nested maps (no use case yet) — JSON column
/// payloads stay opaque strings until the row factory parses them.
Map<String, Object?> normalisePgColumnMap(Map<String, dynamic> raw) {
  final out = <String, Object?>{};
  for (final entry in raw.entries) {
    final key = _stripJsonSuffix(entry.key);
    final camel = _snakeToCamel(key);
    out[camel] = _normaliseValue(entry.value);
  }
  return out;
}

String _stripJsonSuffix(String columnName) {
  if (columnName.endsWith('_json')) {
    return columnName.substring(0, columnName.length - '_json'.length);
  }
  return columnName;
}

String _snakeToCamel(String s) {
  if (!s.contains('_')) return s;
  final parts = s.split('_');
  final buf = StringBuffer(parts.first);
  for (var i = 1; i < parts.length; i++) {
    final p = parts[i];
    if (p.isEmpty) continue;
    buf.write(p[0].toUpperCase());
    if (p.length > 1) buf.write(p.substring(1));
  }
  return buf.toString();
}

/// Translate a few `package:postgres`-typed values to plain Dart so the
/// pg-defs row factories (which only know about Map / List / String /
/// int / bool / double) can decode them straight up.
Object? _normaliseValue(Object? value) {
  if (value == null) return null;
  if (value is String || value is int || value is bool || value is double) {
    return value;
  }
  if (value is num) return value;
  if (value is List<int>) return base64Encode(value);
  if (value is DateTime) {
    return value.toUtc().toIso8601String();
  }
  if (value is List) {
    return value.map(_normaliseValue).toList(growable: false);
  }
  if (value is Map) {
    return Map<String, Object?>.fromEntries(
      value.entries.map(
        (e) => MapEntry(e.key.toString(), _normaliseValue(e.value)),
      ),
    );
  }
  // Fallback — `Range`, `Interval`, custom codecs, etc. Stringify so the
  // pg-defs row class never crashes on a type it doesn't know about.
  return value.toString();
}

String _preview(String sql) {
  final flat = sql.replaceAll(RegExp(r'\s+'), ' ').trim();
  return flat.length > 96 ? '${flat.substring(0, 96)}…' : flat;
}

String _redact(String url) {
  // Strip password segment from the URL for safe logging.
  return url.replaceAllMapped(
    RegExp(r'(://[^:@/\s]+:)([^@/\s]+)(@)'),
    (m) => '${m.group(1)}***${m.group(3)}',
  );
}

String _ensurePoolDefaults(
  String url, {
  required int maxConnectionCount,
  required Duration maxConnectionAge,
  required Duration connectTimeout,
  required Duration queryTimeout,
}) {
  Uri parsed;
  try {
    parsed = Uri.parse(url);
  } catch (_) {
    return url;
  }
  final qp = Map<String, String>.of(parsed.queryParameters);
  qp.putIfAbsent(
      'max_connection_count', () => maxConnectionCount.toString());
  qp.putIfAbsent(
      'max_connection_age', () => maxConnectionAge.inSeconds.toString());
  qp.putIfAbsent('connect_timeout', () => connectTimeout.inSeconds.toString());
  qp.putIfAbsent('query_timeout', () => queryTimeout.inSeconds.toString());
  return parsed.replace(queryParameters: qp).toString();
}

// ---------------------------------------------------------------------------
// Event types — used for observability and future audit logging.
// ---------------------------------------------------------------------------

sealed class PgEvent {
  const PgEvent();
}

class PgQueryEvent extends PgEvent {
  const PgQueryEvent({
    required this.sqlPreview,
    required this.durationUs,
    required this.rows,
  });

  final String sqlPreview;
  final int durationUs;
  final int rows;
}

class PgErrorEvent extends PgEvent {
  const PgErrorEvent({
    required this.sqlPreview,
    required this.error,
    required this.stack,
  });

  final String sqlPreview;
  final String error;
  final String stack;
}
