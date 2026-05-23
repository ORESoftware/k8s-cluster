/// dd-dart-server process entry point.
///
/// Isolate topology
/// ----------------
///   * **Gateway (main isolate, 1)** — port 8089. Owns the WS-side
///     `HttpServer.bind`, the WSS upgrade path, per-WS socket
///     reader/writer pumps, and the supervisor state (`EventBus`,
///     `Presence`, `ConversationRegistry`). Locally serves `/healthz`,
///     `/readyz`, `/metrics`, `/dart/wss`, `/dart/admin/*`. Other paths
///     return 404 — the ingress is expected to route them to the HTTP
///     isolate's port.
///   * **HTTP isolate (1)** — port 8090 (configurable via
///     `HTTP_INTERNAL_PORT`). Dedicated event loop for every non-WS,
///     non-admin route: `/dart/pages/*` (Jaspr SSR), `/dart/app/*` and
///     `/dart/mobile/*` (Flutter web bundles), `/dart/assets/*`, and
///     the `/` root redirect. Folds its per-route counters into the
///     gateway's `Metrics` via a `MetricEvent` SendPort. Separating
///     HTTP from the WS gateway means a fully-saturated WS pump cannot
///     stall HTML rendering or static file IO. **All HTTP requests
///     share this single isolate.**
///   * **Session-host pool (M isolates)** — each owns up to
///     `SESSIONS_PER_HOST` (default 100, range 1..2000) WebSocket
///     session runtimes as plain Dart objects sharing one event loop.
///     Hosts are spawned lazily by `SessionSupervisor` as load arrives;
///     `M = ceil(connections / SESSIONS_PER_HOST)`.
///
/// Setting `SESSIONS_PER_HOST=1` reproduces the legacy
/// one-isolate-per-WebSocket model for direct A/B comparison.
///
/// Routes (gateway, port 8089):
///   GET  /healthz            — liveness probe
///   GET  /readyz             — readiness probe
///   GET  /metrics            — Prometheus exposition (folds HTTP-isolate counters)
///   GET  /dart/wss           — WebSocket upgrade → routed onto a session-host isolate
///   GET  /dart/admin/*       — hot-reload + pg-defs admin (gateway-local state)
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
import 'package:dd_dart_server/server/conversation_registry.dart';
import 'package:dd_dart_server/server/event_bus.dart';
import 'package:dd_dart_server/server/hot_reloader.dart';
import 'package:dd_dart_server/server/http_isolate.dart';
import 'package:dd_dart_server/server/metrics.dart';
import 'package:dd_dart_server/server/postgres.dart';
import 'package:dd_dart_server/server/presence.dart';
import 'package:dd_dart_server/server/session_supervisor.dart';
import 'package:dd_dart_server/shared/wire_messages.dart';

const _wssPath = '/dart/wss';

Future<void> main(List<String> args) async {
  final host = Platform.environment['HTTP_HOST'] ?? '0.0.0.0';
  final port = int.tryParse(Platform.environment['HTTP_PORT'] ?? '') ?? 8089;
  final httpInternalPort = int.tryParse(
        Platform.environment['HTTP_INTERNAL_PORT'] ?? '',
      ) ??
      8090;
  final staticDirPath = Platform.environment['STATIC_DIR'] ?? './public';
  // Independent Flutter web bundle served at /dart/mobile/. Defaults
  // mirror STATIC_DIR's local-dev shape so a fresh checkout works even
  // before scripts/build-and-run.sh has populated either tree.
  final mobileStaticDirPath =
      Platform.environment['MOBILE_STATIC_DIR'] ?? './mobile-public';
  final ready = Platform.environment['READY_AT_BOOT'] != 'false';
  final hotReloadEnabled = Platform.environment['HOT_RELOAD'] == 'true';
  final sessionsPerHost = int.tryParse(
        Platform.environment['SESSIONS_PER_HOST'] ?? '',
      ) ??
      kDefaultSessionsPerHost;
  final watchPaths = (Platform.environment['HOT_RELOAD_PATHS'] ?? 'lib,bin')
      .split(',')
      .map((s) => s.trim())
      .where((s) => s.isNotEmpty)
      .toList();

  final metrics = Metrics();

  // Validate the pg-defs contract before we touch the database. If
  // schema.sql was regenerated and a referenced table no longer exists,
  // we want a hard, descriptive boot failure here — not a SQL error in
  // the middle of a request.
  pg_contract.assertPgContract();

  // Postgres pool is opt-in. When DATABASE_URL is unset (the default in
  // local dev / hot-reload demo), the rest of the server still boots,
  // and `/dart/admin/db` reports `enabled: false` so operators can tell
  // at a glance.
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

  final bus = EventBus(metrics: metrics);
  final presence = Presence();
  final conversations = ConversationRegistry();
  final supervisor = SessionSupervisor(
    metrics: metrics,
    bus: bus,
    presence: presence,
    conversations: conversations,
    sessionsPerHost: sessionsPerHost,
  );

  // HTTP isolate folds its per-route counters into the gateway's
  // Metrics object via this inbox. Lives for the lifetime of the
  // process; the only sender is `httpIsolateEntry`.
  final httpMetricsInbox = ReceivePort('dd-dart-http-metrics-inbox');
  httpMetricsInbox.listen((msg) {
    if (msg is MetricEvent) {
      metrics.inc(msg.name, msg.delta);
    }
  });

  // Spawn the dedicated HTTP isolate before we start accepting WS
  // traffic, so probe paths on either port respond from the moment the
  // gateway is reachable.
  final httpHandshake = ReceivePort('dd-dart-http-isolate-handshake');
  await Isolate.spawn<HttpIsolateBoot>(
    httpIsolateEntry,
    HttpIsolateBoot(
      handshake: httpHandshake.sendPort,
      host: host,
      port: httpInternalPort,
      staticDirPath: staticDirPath,
      mobileStaticDirPath: mobileStaticDirPath,
      metricsBus: httpMetricsInbox.sendPort,
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

  metrics
    ..registerGauge('dart_sessions_live', () => supervisor.liveCount)
    ..registerGauge('dart_sessions_spawned', () => supervisor.spawnedTotal)
    ..registerGauge('dart_session_hosts_live', () => supervisor.hostCount)
    ..registerGauge(
        'dart_session_hosts_spawned', () => supervisor.hostsSpawnedTotal)
    ..registerGauge('dart_session_hosts_terminated',
        () => supervisor.hostsTerminatedTotal)
    ..registerGauge(
        'dart_sessions_per_host_cap', () => supervisor.sessionsPerHost)
    ..registerGauge('dart_eventbus_topics', () => bus.topicCount)
    ..registerGauge('dart_eventbus_sessions', () => bus.sessionCount)
    ..registerGauge('dart_eventbus_total_joins', () => bus.totalJoinCount)
    ..registerGauge('dart_presence_users', () => presence.userCount)
    ..registerGauge('dart_presence_sessions', () => presence.sessionCount)
    ..registerGauge('dart_conversations', () => conversations.conversationCount)
    ..registerGauge('dart_conversation_memberships', () => conversations.totalMemberships)
    ..registerGauge('dart_conversation_recent_cache_size',
        () => conversations.recentCache.size)
    ..registerGauge('dart_conversation_recent_cache_hits',
        () => conversations.recentCache.hits)
    ..registerGauge('dart_conversation_recent_cache_misses',
        () => conversations.recentCache.misses)
    ..registerGauge('dart_conversation_recent_cache_evicts',
        () => conversations.recentCache.evicts)
    ..registerGauge('dart_conversation_recent_cache_expires',
        () => conversations.recentCache.expires);

  // Optional: hot reload via the VM service. Only meaningful when the
  // process is running in JIT mode (`dart run --enable-vm-service`).
  // AOT binaries (`dart compile exe`) ship without a JIT, so the
  // `Service.getInfo()` URI is null and we just no-op.
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

  final server = await HttpServer.bind(host, port, shared: false);
  // Don't let the entire process get torn down by a TLS-aware load balancer
  // that probes us with malformed bytes.
  server.autoCompress = true;
  // ignore: avoid_print
  print(jsonEncode({
    'event': 'dart_server_listening',
    'host': host,
    'port': port,
    'static_dir': staticDirPath,
    'mobile_static_dir': mobileStaticDirPath,
    'ready': ready,
    'sessions_per_host_requested': sessionsPerHost,
    'sessions_per_host_effective': supervisor.sessionsPerHost,
  }));

  ProcessSignal.sigterm.watch().listen((_) async {
    // ignore: avoid_print
    print(jsonEncode({'event': 'sigterm_received'}));
    await server.close(force: false);
  });
  ProcessSignal.sigint.watch().listen((_) async {
    await server.close(force: false);
  });

  await for (final req in server) {
    metrics.inc('dart_http_requests_total');
    unawaited(_route(
      req,
      metrics: metrics,
      supervisor: supervisor,
      ready: ready,
      hotReloader: hotReloader,
      pgPool: pgPool,
      presenceConvsRepo: presenceConvsRepo,
    ));
  }

  await hotReloader?.close();
  await supervisor.close();
  await conversations.close();
  await presence.close();
  await bus.close();
  await pgPool?.close();
  await metrics.close();
}

Future<void> _route(
  HttpRequest req, {
  required Metrics metrics,
  required SessionSupervisor supervisor,
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

  // ---- WSS upgrade -------------------------------------------------------
  if (path == _wssPath) {
    if (!WebSocketTransformer.isUpgradeRequest(req)) {
      await _plain(
        req,
        'expected websocket upgrade\n',
        status: HttpStatus.upgradeRequired,
      );
      return;
    }
    metrics.inc('dart_wss_upgrade_total');
    final socket = await WebSocketTransformer.upgrade(req);
    final sessionId = _newSessionId();
    final remote = req.connectionInfo?.remoteAddress.address ?? 'unknown';
    final headers = <String, String>{};
    req.headers.forEach((k, v) => headers[k] = v.join(','));
    // ignore: avoid_print
    print(jsonEncode({
      'event': 'wss_session_open',
      'session_id': sessionId,
      'remote': remote,
    }));
    try {
      await supervisor.adopt(
        socket,
        sessionId: sessionId,
        remoteAddr: remote,
        requestPath: path,
        headers: headers,
      );
    } catch (e, st) {
      // ignore: avoid_print
      print(jsonEncode({
        'event': 'wss_session_error',
        'session_id': sessionId,
        'error': '$e',
        'stack': '$st',
      }));
    }
    return;
  }

  // ---- Hot reload admin --------------------------------------------------
  // Both routes are guarded on the reloader being initialised. They're
  // intentionally available without auth in this deployment because the
  // service is only reachable through the cluster's authenticated
  // gateway; production deployments should sit this behind dd-remote-auth.
  if (path == '/dart/admin/hot-reload-status' && method == 'GET') {
    if (hotReloader == null) {
      await _plain(req, jsonEncode({'enabled': false}), contentType: 'application/json');
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
  // `/dart/admin/db` is intentionally read-only and dependency-free of
  // the in-memory ConversationRegistry — it talks straight to RDS via
  // the pg-defs contract, so a divergence between the in-memory mirror
  // and the canonical schema is visible at a glance.
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

  // ---- Fallback ----------------------------------------------------------
  // The gateway intentionally does NOT serve /dart/pages, /dart/app,
  // /dart/mobile, /dart/assets, or the / root redirect — those live on
  // the dedicated HTTP isolate (port 8090). The ingress is expected to
  // route them there. If they reach us anyway, return a clear 404 so
  // misconfiguration is visible.
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

/// Short, URL-safe session id. We don't need cryptographic strength here —
/// the id is only used as a debug breadcrumb and as the EventBus key.
String _newSessionId() {
  final us = DateTime.now().microsecondsSinceEpoch;
  final rnd = (us * 2654435761) & 0xffffffff;
  return rnd.toRadixString(36).padLeft(7, '0');
}
