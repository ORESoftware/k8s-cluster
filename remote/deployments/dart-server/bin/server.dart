/// dd-dart-server process entry point.
///
/// Isolate topology
/// ----------------
///   * **Coordinator (main isolate, 1)** — port `ADMIN_PORT` (8088).
///     Owns pgPool, hotReloader, the canonical `Metrics` aggregator,
///     and the inter-isolate metrics inbox. Serves probes + Prometheus
///     exposition + `/dart/admin/*`. Does NOT bind the WS port and
///     does NOT do any WS frame I/O.
///   * **HTTP isolate (1)** — port `HTTP_INTERNAL_PORT` (8090). Single
///     dedicated event loop for every non-WS HTTP route: Jaspr SSR
///     (`/dart/pages/*`), Flutter SPAs (`/dart/app/*`,
///     `/dart/mobile/*`), assets, root redirect. Forwards per-route
///     counters to the coordinator via `MetricEvent` SendPort.
///   * **Gateway shards (N isolates, default `WS_GATEWAY_SHARDS=8`)**
///     — port `HTTP_PORT` (8089), bound `shared: true` on every shard.
///     The kernel SO_REUSEPORT hash distributes incoming connections
///     across shards. Each shard owns its sockets through the full WS
///     lifecycle (accept → upgrade → frame I/O → close) and runs its
///     own `EventBus` / `Presence` / `ConversationRegistry` /
///     `SessionSupervisor` + child host pool. Shards forward counter
///     increments and periodic gauge snapshots to the coordinator;
///     gauges are summed across shards in the coordinator's
///     `/metrics`.
///   * **Session-host pool (M isolates, owned per-shard)** — each
///     hosts up to `SESSIONS_PER_HOST` (default 100) WebSocket session
///     runtimes as plain Dart objects sharing one event loop. Spawned
///     lazily by each shard's `SessionSupervisor`.
///
/// Single-pod headroom math (8 cores, 4 Gi RAM):
///   * 8 shards × 1000 sockets/shard = 8 000 active WS conns
///     comfortably; 20K is feasible with `WS_GATEWAY_SHARDS=16+` and a
///     non-default `WS_CLOCK_INTERVAL_SECONDS` (5–15 s) so the per-tick
///     jaspr fan-out cost stays under the pod CPU limit.
///
/// Routes (coordinator, port 8088):
///   GET  /healthz            — liveness probe
///   GET  /readyz             — readiness probe
///   GET  /metrics            — Prometheus exposition (counters from
///                              every isolate + summed gauges)
///   GET  /dart/admin/*       — hot-reload + pg-defs admin
///
/// Routes (gateway shards, port 8089):
///   GET  /healthz            — cheap mirror so probes can hit :8089
///   GET  /dart/wss           — WebSocket upgrade → adopted onto the
///                              shard's session-host pool
///
/// Routes (HTTP isolate, port 8090):
///   GET  /                   — 302 → /dart/pages
///   GET  /dart, /dart/       — 302 → /dart/pages
///   GET  /dart/pages, /dart/pages/*  — Jaspr SSR
///   GET  /dart/app, /dart/app/*      — Flutter SPA static
///   GET  /dart/mobile, /dart/mobile/* — Flutter mobile static
///   GET  /dart/assets/*      — Flutter asset files
///   GET  /healthz            — local mirror (probes on :8090 work too)
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:dd_dart_server/db/pg_contract.dart' as pg_contract;
import 'package:dd_dart_server/db/presence_convs_repo.dart';
import 'package:dd_dart_server/server/gateway_isolate.dart';
import 'package:dd_dart_server/server/hot_reloader.dart';
import 'package:dd_dart_server/server/http_isolate.dart';
import 'package:dd_dart_server/server/metrics.dart';
import 'package:dd_dart_server/server/postgres.dart';
import 'package:dd_dart_server/server/session_supervisor.dart';
import 'package:dd_dart_server/shared/wire_messages.dart';

/// How frequently each gateway shard pushes a [GaugeReport] to the
/// coordinator. Short enough that `/metrics` is fresh; long enough that
/// the inter-isolate traffic stays trivial. Override with
/// `WS_GAUGE_REPORT_INTERVAL_MS`.
const int _kDefaultGaugeReportIntervalMs = 1000;

/// Default gateway shard count. Each shard owns its own
/// `HttpServer.bind(..., shared: true)` listener; the kernel
/// SO_REUSEPORT hash distributes incoming connections across them.
/// 8 is comfortable on a 4-vCPU pod for ~10K concurrent WS peers; bump
/// to 16–32 for 20K+ if pod CPU allows. Override with
/// `WS_GATEWAY_SHARDS`.
const int _kDefaultGatewayShards = 8;

Future<void> main(List<String> args) async {
  final host = Platform.environment['HTTP_HOST'] ?? '0.0.0.0';
  final wsPort = int.tryParse(Platform.environment['HTTP_PORT'] ?? '') ?? 8089;
  final httpInternalPort = int.tryParse(
        Platform.environment['HTTP_INTERNAL_PORT'] ?? '',
      ) ??
      8090;
  final adminPort =
      int.tryParse(Platform.environment['ADMIN_PORT'] ?? '') ?? 8088;
  final staticDirPath = Platform.environment['STATIC_DIR'] ?? './public';
  final mobileStaticDirPath =
      Platform.environment['MOBILE_STATIC_DIR'] ?? './mobile-public';
  final ready = Platform.environment['READY_AT_BOOT'] != 'false';
  final hotReloadEnabled = Platform.environment['HOT_RELOAD'] == 'true';
  final gatewayShards = (int.tryParse(
            Platform.environment['WS_GATEWAY_SHARDS'] ?? '',
          ) ??
          _kDefaultGatewayShards)
      .clamp(1, 256);
  final sessionsPerHost = int.tryParse(
        Platform.environment['SESSIONS_PER_HOST'] ?? '',
      ) ??
      kDefaultSessionsPerHost;
  final wsIdleTimeoutSeconds = int.tryParse(
        Platform.environment['WS_IDLE_TIMEOUT_SECONDS'] ?? '',
      ) ??
      kDefaultIdleTimeoutSeconds;
  final wsMaxAgeSeconds = int.tryParse(
        Platform.environment['WS_MAX_AGE_SECONDS'] ?? '',
      ) ??
      kDefaultMaxAgeSeconds;
  final wsAgeBasedIdleSeconds = int.tryParse(
        Platform.environment['WS_AGE_BASED_IDLE_SECONDS'] ?? '',
      ) ??
      kDefaultAgeBasedIdleSeconds;
  final wsClockIntervalSeconds = int.tryParse(
        Platform.environment['WS_CLOCK_INTERVAL_SECONDS'] ?? '',
      ) ??
      kDefaultClockIntervalSeconds;
  final wsMaxInboundBytes = int.tryParse(
        Platform.environment['WS_MAX_INBOUND_BYTES'] ?? '',
      ) ??
      kDefaultMaxInboundBytes;
  final wsMaxOutboundRate = int.tryParse(
        Platform.environment['WS_MAX_OUTBOUND_RATE_PER_SECOND'] ?? '',
      ) ??
      kDefaultMaxOutboundRatePerSecond;
  final wsSlowClientWindows = int.tryParse(
        Platform.environment['WS_SLOW_CLIENT_WINDOWS'] ?? '',
      ) ??
      kDefaultSlowClientWindows;
  final gaugeReportIntervalMs = int.tryParse(
        Platform.environment['WS_GAUGE_REPORT_INTERVAL_MS'] ?? '',
      ) ??
      _kDefaultGaugeReportIntervalMs;
  final shutdownGraceSeconds = int.tryParse(
        Platform.environment['SHUTDOWN_GRACE_SECONDS'] ?? '',
      ) ??
      25;
  final wsBenchmarkMode =
      Platform.environment['WS_BENCHMARK_MODE']?.toLowerCase() == 'true';
  final watchPaths = (Platform.environment['HOT_RELOAD_PATHS'] ?? 'lib,bin')
      .split(',')
      .map((s) => s.trim())
      .where((s) => s.isNotEmpty)
      .toList();

  final metrics = Metrics();

  pg_contract.assertPgContract();

  final pgUrl = Platform.environment['DATABASE_URL'] ??
      Platform.environment['RDS_DATABASE_URL'] ??
      Platform.environment['AGENT_TASKS_RDS_DATABASE_URL'];
  final pgMetrics = PgMetrics()..bind(metrics);
  final pgPool = await PgPool.open(url: pgUrl, metrics: pgMetrics);
  final presenceConvsRepo = pgPool != null ? PresenceConvsRepo(pgPool) : null;
  print(jsonEncode({
    'event': 'pg_init',
    'enabled': pgPool != null,
    'configured': pgUrl != null && pgUrl.isNotEmpty,
    'contract': pg_contract.pgContractSnapshot(),
  }));

  // Shared metrics inbox: HTTP isolate AND every gateway shard send
  // [MetricEvent]s here, and shards additionally send periodic
  // [GaugeReport]s. The coordinator folds counters into [metrics] and
  // keeps a per-shard gauge map for summed exposition.
  final metricsInbox = ReceivePort('dd-dart-coordinator-metrics-inbox');
  final perShardGauges = <int, Map<String, num>>{};
  final lastGaugeReportAtMs = <int, int>{};
  metricsInbox.listen((msg) {
    if (msg is MetricEvent) {
      metrics.inc(msg.name, msg.delta);
    } else if (msg is GaugeReport) {
      perShardGauges[msg.shardId] = msg.values;
      lastGaugeReportAtMs[msg.shardId] = DateTime.now().millisecondsSinceEpoch;
    }
  });

  // ---- HTTP isolate ------------------------------------------------------
  final httpHandshake = ReceivePort('dd-dart-http-isolate-handshake');
  await Isolate.spawn<HttpIsolateBoot>(
    httpIsolateEntry,
    HttpIsolateBoot(
      handshake: httpHandshake.sendPort,
      host: host,
      port: httpInternalPort,
      staticDirPath: staticDirPath,
      mobileStaticDirPath: mobileStaticDirPath,
      metricsBus: metricsInbox.sendPort,
    ),
    debugName: 'dd-dart-http',
    errorsAreFatal: false,
  );
  final httpBoundPort = await httpHandshake.first as int;
  httpHandshake.close();
  print(jsonEncode({
    'event': 'dart_http_isolate_ready',
    'requested_port': httpInternalPort,
    'bound_port': httpBoundPort,
  }));

  // ---- Gateway shards ----------------------------------------------------
  // Each shard binds the SAME WS port with `shared: true`; SO_REUSEPORT
  // distributes incoming connections across the shard pool. We spawn
  // shards eagerly at boot so the kernel hash is stable from the first
  // accepted connection onward (lazy spawn would re-shuffle established
  // accepts as the listening-socket count grows).
  final shards = <_GatewayShardHandle>[];
  for (var shardId = 0; shardId < gatewayShards; shardId++) {
    final handshake = ReceivePort('dd-dart-shard-$shardId-handshake');
    final exit = ReceivePort('dd-dart-shard-$shardId-exit');
    final error = ReceivePort('dd-dart-shard-$shardId-error');
    Isolate? isolate;
    try {
      isolate = await Isolate.spawn<GatewayShardBoot>(
        gatewayShardEntry,
        GatewayShardBoot(
          shardId: shardId,
          handshake: handshake.sendPort,
          metricsBus: metricsInbox.sendPort,
          host: host,
          port: wsPort,
          sessionsPerHost: sessionsPerHost,
          idleTimeoutSeconds: wsIdleTimeoutSeconds,
          maxAgeSeconds: wsMaxAgeSeconds,
          ageBasedIdleSeconds: wsAgeBasedIdleSeconds,
          maxInboundBytes: wsMaxInboundBytes,
          maxOutboundRatePerSecond: wsMaxOutboundRate,
          slowClientWindows: wsSlowClientWindows,
          clockIntervalSeconds: wsClockIntervalSeconds,
          benchmarkMode: wsBenchmarkMode,
          gaugeReportIntervalMs: gaugeReportIntervalMs,
        ),
        debugName: 'dd-dart-gateway-shard-$shardId',
        errorsAreFatal: false,
        onExit: exit.sendPort,
        onError: error.sendPort,
      );
    } catch (e, st) {
      // ignore: avoid_print
      print(jsonEncode({
        'event': 'gateway_shard_spawn_failed',
        'shard': shardId,
        'error': '$e',
        'stack': '$st',
      }));
      handshake.close();
      exit.close();
      error.close();
      rethrow;
    }
    final controlPort = await handshake.first as SendPort;
    handshake.close();
    final handle = _GatewayShardHandle(
      shardId: shardId,
      isolate: isolate,
      control: controlPort,
      exit: exit,
      error: error,
    );
    shards.add(handle);
    metrics.inc('dart_gateway_shards_spawned_total');
    handle.exitSub = exit.listen((_) {
      handle.dead = true;
      metrics.inc('dart_gateway_shards_terminated_total');
      exit.close();
    });
    handle.errorSub = error.listen((err) {
      metrics.inc('dart_gateway_shards_error_total');
      // ignore: avoid_print
      print(jsonEncode({
        'event': 'gateway_shard_error',
        'shard': shardId,
        'error': '$err',
      }));
    });
  }
  print(jsonEncode({
    'event': 'gateway_shards_ready',
    'count': shards.length,
    'port': wsPort,
    'sessions_per_host': sessionsPerHost,
    'ws_idle_timeout_seconds': wsIdleTimeoutSeconds,
    'ws_max_age_seconds': wsMaxAgeSeconds,
    'ws_age_based_idle_seconds': wsAgeBasedIdleSeconds,
    'ws_clock_interval_seconds': wsClockIntervalSeconds,
    'ws_benchmark_mode': wsBenchmarkMode,
  }));

  // ---- Coordinator gauges ------------------------------------------------
  // Coordinator-local gauges first; per-shard summed gauges below.
  num _sumShardGauge(String name) {
    var total = 0.0;
    var hasInt = true;
    for (final m in perShardGauges.values) {
      final v = m[name];
      if (v == null) continue;
      total += v.toDouble();
      if (v is! int) hasInt = false;
    }
    return hasInt ? total.toInt() : total;
  }

  num _maxShardGauge(String name) {
    num? best;
    for (final m in perShardGauges.values) {
      final v = m[name];
      if (v == null) continue;
      if (best == null || v > best) best = v;
    }
    return best ?? 0;
  }

  metrics
    ..registerGauge('dart_gateway_shards_configured', () => gatewayShards)
    ..registerGauge('dart_gateway_shards_live',
        () => shards.where((s) => !s.dead).length)
    ..registerGauge('dart_sessions_live',
        () => _sumShardGauge('dart_sessions_live'))
    ..registerGauge('dart_sessions_spawned',
        () => _sumShardGauge('dart_sessions_spawned'))
    ..registerGauge('dart_session_hosts_live',
        () => _sumShardGauge('dart_session_hosts_live'))
    ..registerGauge('dart_session_hosts_spawned',
        () => _sumShardGauge('dart_session_hosts_spawned'))
    ..registerGauge('dart_session_hosts_terminated',
        () => _sumShardGauge('dart_session_hosts_terminated'))
    ..registerGauge('dart_sessions_per_host_cap',
        () => _maxShardGauge('dart_sessions_per_host_cap'))
    ..registerGauge('dart_ws_idle_timeout_seconds',
        () => _maxShardGauge('dart_ws_idle_timeout_seconds'))
    ..registerGauge('dart_ws_max_age_seconds',
        () => _maxShardGauge('dart_ws_max_age_seconds'))
    ..registerGauge('dart_ws_age_based_idle_seconds',
        () => _maxShardGauge('dart_ws_age_based_idle_seconds'))
    ..registerGauge('dart_ws_clock_interval_seconds',
        () => _maxShardGauge('dart_ws_clock_interval_seconds'))
    ..registerGauge('dart_eventbus_topics',
        () => _sumShardGauge('dart_eventbus_topics'))
    ..registerGauge('dart_eventbus_sessions',
        () => _sumShardGauge('dart_eventbus_sessions'))
    ..registerGauge('dart_eventbus_total_joins',
        () => _sumShardGauge('dart_eventbus_total_joins'))
    ..registerGauge('dart_presence_users',
        () => _sumShardGauge('dart_presence_users'))
    ..registerGauge('dart_presence_sessions',
        () => _sumShardGauge('dart_presence_sessions'))
    ..registerGauge('dart_conversations',
        () => _sumShardGauge('dart_conversations'))
    ..registerGauge('dart_conversation_memberships',
        () => _sumShardGauge('dart_conversation_memberships'));

  // ---- Hot reloader ------------------------------------------------------
  HotReloader? hotReloader;
  if (hotReloadEnabled) {
    hotReloader = HotReloader(metrics: metrics, watchPaths: watchPaths);
    final ok = await hotReloader.start();
    print(jsonEncode({
      'event': 'hot_reload_init',
      'ok': ok,
      'serviceUri': hotReloader.serviceUri,
      'watchPaths': watchPaths,
    }));
    hotReloader.results.listen((r) {
      print(jsonEncode({'event': 'hot_reload_done', ...r.toJson()}));
    });
  }

  // ---- Admin server ------------------------------------------------------
  final adminServer = await HttpServer.bind(host, adminPort, shared: false);
  adminServer.autoCompress = true;
  // ignore: avoid_print
  print(jsonEncode({
    'event': 'dart_admin_listening',
    'host': host,
    'admin_port': adminServer.port,
    'ws_port': wsPort,
    'http_port': httpInternalPort,
    'gateway_shards': gatewayShards,
    'static_dir': staticDirPath,
    'mobile_static_dir': mobileStaticDirPath,
    'ready': ready,
  }));

  // ---- SIGTERM handling --------------------------------------------------
  // Coordinator drain order:
  //   1. Stop accepting new admin requests.
  //   2. Send ShardShutdown to every gateway shard (each shard closes
  //      its listening socket + asks its supervisor to drain).
  //   3. Wait up to SHUTDOWN_GRACE_SECONDS for `dart_sessions_live`
  //      across all shards to hit zero.
  //   4. Hard-kill any laggard shards and exit.
  Completer<void>? shuttingDown;
  Future<void> beginShutdown(String signal) async {
    if (shuttingDown != null) return shuttingDown!.future;
    final c = Completer<void>();
    shuttingDown = c;
    final liveShards = shards.where((s) => !s.dead).length;
    final liveSessions = _sumShardGauge('dart_sessions_live').toInt();
    // ignore: avoid_print
    print(jsonEncode({
      'event': 'shutdown_begin',
      'signal': signal,
      'live_shards': liveShards,
      'live_sessions': liveSessions,
      'grace_seconds': shutdownGraceSeconds,
    }));
    try {
      await adminServer.close(force: false);
    } catch (_) {/* swallow */}
    for (final shard in shards) {
      if (shard.dead) continue;
      try {
        shard.control.send(const ShardShutdown());
      } catch (_) {/* swallow */}
    }
    final deadline = DateTime.now().add(Duration(seconds: shutdownGraceSeconds));
    while (DateTime.now().isBefore(deadline)) {
      final stillLive = _sumShardGauge('dart_sessions_live').toInt();
      final stillRunningShards = shards.where((s) => !s.dead).length;
      if (stillLive == 0 && stillRunningShards == 0) break;
      await Future<void>.delayed(const Duration(milliseconds: 200));
    }
    // Final reaping. Kills any shard isolates still alive past the
    // grace deadline so the pod exits promptly.
    for (final shard in shards) {
      if (shard.dead) continue;
      try {
        shard.isolate.kill(priority: Isolate.beforeNextEvent);
      } catch (_) {/* swallow */}
    }
    // ignore: avoid_print
    print(jsonEncode({
      'event': 'shutdown_drain_complete',
      'final_live_sessions': _sumShardGauge('dart_sessions_live').toInt(),
      'final_live_shards': shards.where((s) => !s.dead).length,
    }));
    c.complete();
  }

  ProcessSignal.sigterm.watch().listen((_) {
    unawaited(beginShutdown('SIGTERM'));
  });
  ProcessSignal.sigint.watch().listen((_) {
    unawaited(beginShutdown('SIGINT'));
  });

  // ---- Admin request loop ------------------------------------------------
  await for (final req in adminServer) {
    metrics.inc('dart_admin_requests_total');
    unawaited(_routeAdmin(
      req,
      metrics: metrics,
      ready: ready,
      hotReloader: hotReloader,
      pgPool: pgPool,
      presenceConvsRepo: presenceConvsRepo,
    ));
  }

  await hotReloader?.close();
  metricsInbox.close();
  await pgPool?.close();
  await metrics.close();
}

/// Per-shard handle the coordinator keeps so it can monitor and tear
/// down each shard isolate.
class _GatewayShardHandle {
  _GatewayShardHandle({
    required this.shardId,
    required this.isolate,
    required this.control,
    required this.exit,
    required this.error,
  });

  final int shardId;
  final Isolate isolate;
  final SendPort control;
  final ReceivePort exit;
  final ReceivePort error;

  bool dead = false;
  StreamSubscription<dynamic>? exitSub;
  StreamSubscription<dynamic>? errorSub;
}

Future<void> _routeAdmin(
  HttpRequest req, {
  required Metrics metrics,
  required bool ready,
  HotReloader? hotReloader,
  PgPool? pgPool,
  PresenceConvsRepo? presenceConvsRepo,
}) async {
  final path = req.uri.path;
  final method = req.method.toUpperCase();

  // ---- Probes / metrics --------------------------------------------------
  if (method == 'GET' && path == '/healthz') {
    await _plain(req, 'ok\n');
    return;
  }
  if (method == 'GET' && path == '/readyz') {
    if (!ready) {
      await _plain(req, 'not_ready\n', status: HttpStatus.serviceUnavailable);
    } else {
      await _plain(req, 'ready\n');
    }
    return;
  }
  if (method == 'GET' && path == '/metrics') {
    await _plain(req, metrics.render(), contentType: 'text/plain; version=0.0.4');
    return;
  }

  // ---- Hot reload admin --------------------------------------------------
  if (path == '/dart/admin/hot-reload-status' && method == 'GET') {
    if (hotReloader == null) {
      await _plain(req, jsonEncode({'enabled': false}),
          contentType: 'application/json');
      return;
    }
    final body = jsonEncode({
      'enabled': true,
      'running': hotReloader.isRunning,
      'serviceUri': hotReloader.serviceUri,
      'reloads': hotReloader.reloads,
      'reloadsFailed': hotReloader.reloadsFailed,
      'lastDurationMs': hotReloader.lastDurationMs,
      'last': hotReloader.lastResult?.toJson(),
    });
    await _plain(req, body, contentType: 'application/json');
    return;
  }

  if (path == '/dart/admin/reload' && (method == 'POST' || method == 'GET')) {
    if (hotReloader == null) {
      await _plain(
        req,
        jsonEncode({
          'success': false,
          'message':
              'hot reload not enabled — set HOT_RELOAD=true and run with --enable-vm-service (JIT mode)',
        }),
        status: HttpStatus.serviceUnavailable,
        contentType: 'application/json',
      );
      return;
    }
    final force = req.uri.queryParameters['force'] == '1' ||
        req.uri.queryParameters['force'] == 'true';
    final result = await hotReloader.reloadAll(
      force: force,
      reason: 'admin-route',
    );
    await _plain(
      req,
      jsonEncode(result.toJson()),
      status: result.success ? HttpStatus.ok : HttpStatus.internalServerError,
      contentType: 'application/json',
    );
    return;
  }

  // ---- Postgres / pg-defs admin -----------------------------------------
  if (path == '/dart/admin/db' && method == 'GET') {
    if (pgPool == null) {
      await _plain(
        req,
        jsonEncode({
          'enabled': false,
          'reason':
              'DATABASE_URL not set. Export `DATABASE_URL` (or `RDS_DATABASE_URL`) and restart to enable the pg-defs path.',
          'contract': pg_contract.pgContractSnapshot(),
        }),
        contentType: 'application/json',
      );
      return;
    }
    final ping = await pgPool.ping();
    final body = jsonEncode({
      'enabled': true,
      'ping': ping,
      'contract': pg_contract.pgContractSnapshot(),
      'metrics': {
        'queries': pgPool.metrics.queries,
        'queryErrors': pgPool.metrics.queryErrors,
        'rowsRead': pgPool.metrics.rowsRead,
      },
    });
    await _plain(
      req,
      body,
      status: ping['ok'] == true
          ? HttpStatus.ok
          : HttpStatus.internalServerError,
      contentType: 'application/json',
    );
    return;
  }

  if (path == '/dart/admin/db/conversations' && method == 'GET') {
    if (presenceConvsRepo == null) {
      await _plain(
        req,
        jsonEncode({'enabled': false}),
        status: HttpStatus.serviceUnavailable,
        contentType: 'application/json',
      );
      return;
    }
    final limit = int.tryParse(req.uri.queryParameters['limit'] ?? '') ?? 25;
    try {
      final rows = await presenceConvsRepo.listActive(
        limit: limit.clamp(1, 200),
      );
      await _plain(
        req,
        jsonEncode({
          'count': rows.length,
          'rows': rows.map((r) => r.toJson()).toList(),
        }),
        contentType: 'application/json',
      );
    } catch (e) {
      await _plain(
        req,
        jsonEncode({'error': '$e'}),
        status: HttpStatus.internalServerError,
        contentType: 'application/json',
      );
    }
    return;
  }

  metrics.inc('dart_http_404_total');
  await _plain(req, 'not_found\n', status: HttpStatus.notFound);
}

Future<void> _plain(
  HttpRequest req,
  String body, {
  int status = HttpStatus.ok,
  String contentType = 'text/plain; charset=utf-8',
}) async {
  req.response
    ..statusCode = status
    ..headers.set('content-type', contentType)
    ..write(body);
  await req.response.close();
}
