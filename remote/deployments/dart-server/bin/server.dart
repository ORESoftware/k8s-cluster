/// dd-dart-server process entry point.
///
/// One process, one main isolate, N session isolates (one per WebSocket).
///
/// Routes:
///   GET  /healthz            — liveness probe
///   GET  /readyz             — readiness probe
///   GET  /metrics            — Prometheus exposition
///   GET  /                   — 302 → /dart/pages
///   GET  /dart/pages         — Jaspr SSR home
///   GET  /dart/pages/*       — Jaspr SSR routed pages
///   GET  /dart/wss           — WebSocket upgrade → per-connection isolate
///   GET  /dart/app           — Flutter web SPA index.html
///   GET  /dart/app/*         — Flutter web SPA (with index.html fallback)
///   GET  /dart/mobile        — Mobile-optimized Flutter web bundle index.html
///   GET  /dart/mobile/*      — Mobile-optimized Flutter web bundle (with index.html fallback)
///   GET  /dart/assets/*      — Flutter web build assets (JS bundle, SW, icons)
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dd_dart_server/db/pg_contract.dart' as pg_contract;
import 'package:dd_dart_server/db/presence_convs_repo.dart';
import 'package:dd_dart_server/server/conversation_registry.dart';
import 'package:dd_dart_server/server/event_bus.dart';
import 'package:dd_dart_server/server/hot_reloader.dart';
import 'package:dd_dart_server/server/metrics.dart';
import 'package:dd_dart_server/server/postgres.dart';
import 'package:dd_dart_server/server/presence.dart';
import 'package:dd_dart_server/server/session_supervisor.dart';
import 'package:dd_dart_server/server/static_files.dart';
import 'package:dd_dart_server/jaspr/render.dart';

const _wssPath = '/dart/wss';
const _pagesPrefix = '/dart/pages';
const _appPrefix = '/dart/app';
const _mobilePrefix = '/dart/mobile';
const _assetsPrefix = '/dart/assets';

Future<void> main(List<String> args) async {
  final host = Platform.environment['HTTP_HOST'] ?? '0.0.0.0';
  final port = int.tryParse(Platform.environment['HTTP_PORT'] ?? '') ?? 8089;
  final staticDirPath = Platform.environment['STATIC_DIR'] ?? './public';
  // Independent Flutter web bundle served at /dart/mobile/. Defaults
  // mirror STATIC_DIR's local-dev shape so a fresh checkout works even
  // before scripts/build-and-run.sh has populated either tree.
  final mobileStaticDirPath =
      Platform.environment['MOBILE_STATIC_DIR'] ?? './mobile-public';
  final ready = Platform.environment['READY_AT_BOOT'] != 'false';
  final hotReloadEnabled = Platform.environment['HOT_RELOAD'] == 'true';
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
  );
  final staticFiles = StaticFileServer(Directory(staticDirPath));
  final mobileStaticFiles = StaticFileServer(
    Directory(mobileStaticDirPath),
    serviceWorkerAllowedScope: '$_mobilePrefix/',
  );

  metrics
    ..registerGauge('dart_sessions_live', () => supervisor.liveCount)
    ..registerGauge('dart_sessions_spawned', () => supervisor.spawnedTotal)
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
      staticFiles: staticFiles,
      mobileStaticFiles: mobileStaticFiles,
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
  required StaticFileServer staticFiles,
  required StaticFileServer mobileStaticFiles,
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

  // ---- Jaspr SSR /dart/pages --------------------------------------------
  if (method == 'GET' && (path == _pagesPrefix || path.startsWith('$_pagesPrefix/'))) {
    final route = path.substring(_pagesPrefix.length).isEmpty
        ? '/'
        : path.substring(_pagesPrefix.length);
    final query = req.uri.queryParameters;
    try {
      final html = await renderJasprPage(route, query: query);
      metrics.inc('dart_pages_rendered_total');
      req.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.html
        ..headers.set('cache-control', 'no-cache')
        ..write(html);
      await req.response.close();
    } catch (e, st) {
      metrics.inc('dart_pages_render_error_total');
      // ignore: avoid_print
      print(jsonEncode({
        'event': 'jaspr_render_error',
        'route': route,
        'error': '$e',
        'stack': '$st',
      }));
      await _plain(req, 'render_error\n', status: HttpStatus.internalServerError);
    }
    return;
  }

  // ---- Flutter SPA static files -----------------------------------------
  if (method == 'GET' && (path == _appPrefix || path.startsWith('$_appPrefix/'))) {
    final rel = path == _appPrefix ? '' : path.substring(_appPrefix.length + 1);
    metrics.inc('dart_app_requests_total');
    final served = await staticFiles.tryServe(
      req,
      requestPath: rel,
      fallbackHtml: 'index.html',
    );
    if (!served) {
      await _plain(req, 'flutter app not built\n', status: HttpStatus.notFound);
    }
    return;
  }

  // ---- Flutter mobile bundle static files -------------------------------
  // Independent Flutter web bundle, base-href=/dart/mobile/, lives in its
  // own MOBILE_STATIC_DIR. Jaspr SSR at /dart/pages is unaffected; this
  // handler only owns /dart/mobile/* and never falls through to pickPage.
  if (method == 'GET' &&
      (path == _mobilePrefix || path.startsWith('$_mobilePrefix/'))) {
    final rel =
        path == _mobilePrefix ? '' : path.substring(_mobilePrefix.length + 1);
    metrics.inc('dart_mobile_requests_total');
    final served = await mobileStaticFiles.tryServe(
      req,
      requestPath: rel,
      fallbackHtml: 'index.html',
    );
    if (!served) {
      await _plain(
        req,
        'flutter mobile app not built\n',
        status: HttpStatus.notFound,
      );
    }
    return;
  }

  if (method == 'GET' && path.startsWith('$_assetsPrefix/')) {
    final rel = path.substring(_assetsPrefix.length + 1);
    metrics.inc('dart_assets_requests_total');
    final served = await staticFiles.tryServe(req, requestPath: rel);
    if (!served) {
      await _plain(req, 'asset not found\n', status: HttpStatus.notFound);
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

  // ---- Root → SSR home ---------------------------------------------------
  if (method == 'GET' && (path == '/' || path == '/dart' || path == '/dart/')) {
    req.response
      ..statusCode = HttpStatus.movedPermanently
      ..headers.set('location', '/dart/pages');
    await req.response.close();
    return;
  }

  // ---- Fallback ----------------------------------------------------------
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
