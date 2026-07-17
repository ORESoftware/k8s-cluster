/// Dedicated HTTP-handler isolate.
///
/// Lives on its own internal port (default 8090, configurable via
/// `HTTP_INTERNAL_PORT`) so non-WS HTTP traffic (Jaspr SSR, the Flutter
/// SPAs, asset files, the root redirect) never competes with the
/// gateway isolate's per-WebSocket socket pumps for event-loop time.
///
/// Topology:
///
///   Gateway isolate (main, port 8089)
///   ├── /healthz, /readyz, /metrics    — local
///   ├── /dart/wss                       — WS upgrade + per-WS pumps
///   ├── /dart/admin/*                   — main-isolate-only state
///   └── everything else                 — 404 (ingress should not route
///                                         non-WS, non-admin paths here)
///
///   HTTP isolate (port 8090)
///   ├── /                   — 302 → /dart/pages
///   ├── /dart, /dart/       — 302 → /dart/pages
///   ├── /dart/pages/*       — Jaspr SSR
///   ├── /dart/app/*         — Flutter SPA static
///   ├── /dart/mobile/*      — Flutter mobile static
///   ├── /dart/assets/*      — Flutter asset files
///   ├── /docs/api, /api/docs — generated API docs
///   ├── /api/docs.json      — generated API docs metadata
///   ├── /healthz            — local liveness mirror (so probes can hit
///                             either port without depending on the
///                             gateway being responsive under WS load)
///   └── everything else     — 404
///
/// The HTTP isolate has no awareness of the EventBus / SessionSupervisor
/// / Presence / ConversationRegistry. It increments per-route counters
/// by sending `MetricEvent`s to the gateway via [HttpIsolateBoot.metricsBus],
/// where they fold into the global Prometheus exposition exactly the
/// same way per-session metrics do.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import '../jaspr/render.dart';
import '../shared/wire_messages.dart';
import 'static_files.dart';
import 'wss_components.dart';

/// Boot payload handed to the HTTP isolate. Plain values + SendPorts
/// only, so it crosses the isolate boundary cleanly.
final class HttpIsolateBoot {
  const HttpIsolateBoot({
    required this.handshake,
    required this.host,
    required this.port,
    required this.staticDirPath,
    required this.mobileStaticDirPath,
    required this.apiDocsDirPath,
    required this.metricsBus,
  });

  /// One-shot SendPort the gateway listens on for the "I'm bound and
  /// ready" handshake. Receives the actual port number HttpServer ended
  /// up bound to (useful when the supervisor passes 0 for ephemeral).
  final SendPort handshake;

  final String host;
  final int port;
  final String staticDirPath;
  final String mobileStaticDirPath;
  final String apiDocsDirPath;

  /// SendPort to which the HTTP isolate posts [MetricEvent]s. The
  /// gateway folds them into the global Metrics object so the unified
  /// `/metrics` endpoint reports HTTP route counters even though the
  /// route handlers run on a different isolate.
  final SendPort metricsBus;
}

const String _wssPath = '/dart/wss';
const String _pagesPrefix = '/dart/pages';
const String _appPrefix = '/dart/app';
const String _mobilePrefix = '/dart/mobile';
const String _assetsPrefix = '/dart/assets';

/// Entry point for `Isolate.spawn(httpIsolateEntry, boot)`.
Future<void> httpIsolateEntry(HttpIsolateBoot boot) async {
  ensureJasprInit();

  final staticFiles = StaticFileServer(Directory(boot.staticDirPath));
  final mobileStaticFiles = StaticFileServer(
    Directory(boot.mobileStaticDirPath),
    serviceWorkerAllowedScope: '$_mobilePrefix/',
  );

  void inc(String name, [int delta = 1]) {
    boot.metricsBus.send(MetricEvent(name, delta));
  }

  final server = await HttpServer.bind(boot.host, boot.port, shared: false);
  server.autoCompress = true;
  // ignore: avoid_print
  print(jsonEncode({
    'event': 'dart_http_isolate_listening',
    'host': boot.host,
    'port': server.port,
    'static_dir': boot.staticDirPath,
    'mobile_static_dir': boot.mobileStaticDirPath,
    'api_docs_dir': boot.apiDocsDirPath,
  }));
  boot.handshake.send(server.port);

  // SIGTERM handling lives on the gateway; the gateway calls
  // `server.close()` on the http port via a separate control SendPort
  // when it needs to drain. For now the gateway sends a kill on the
  // isolate via Isolate.kill, which is sufficient since http-isolate
  // state is transient.
  await for (final req in server) {
    inc('dart_http_requests_total');
    unawaited(_route(
      req,
      staticFiles: staticFiles,
      mobileStaticFiles: mobileStaticFiles,
      apiDocsDirPath: boot.apiDocsDirPath,
      inc: inc,
    ));
  }
}

Future<void> _route(
  HttpRequest req, {
  required StaticFileServer staticFiles,
  required StaticFileServer mobileStaticFiles,
  required String apiDocsDirPath,
  required void Function(String name, [int delta]) inc,
}) async {
  final path = req.uri.path;
  final method = req.method.toUpperCase();

  // Mirror /healthz on the http port so the kubelet (or anything else
  // that probes :8090) can verify reachability without going through
  // the gateway isolate.
  if (method == 'GET' && path == '/healthz') {
    await _plain(req, 'ok\n');
    return;
  }

  // ---- Generated API docs --------------------------------------------------
  if (method == 'GET' && (path == '/docs/api' || path == '/api/docs')) {
    await _generatedFile(
      req,
      apiDocsDirPath,
      'api-docs.html',
      contentType: 'text/html; charset=utf-8',
    );
    return;
  }

  if (method == 'GET' && path == '/api/docs.json') {
    await _generatedFile(
      req,
      apiDocsDirPath,
      'api-docs.json',
      contentType: 'application/json; charset=utf-8',
    );
    return;
  }

  // The HTTP isolate explicitly does NOT serve /dart/wss. Anything that
  // arrives here is a misrouted ingress and should be told so.
  if (path == _wssPath) {
    await _plain(
      req,
      'wss not handled on http port; route /dart/wss to :8089\n',
      status: HttpStatus.misdirectedRequest,
    );
    return;
  }

  // ---- Jaspr SSR /dart/pages --------------------------------------------
  if (method == 'GET' &&
      (path == _pagesPrefix || path.startsWith('$_pagesPrefix/'))) {
    final route = path.substring(_pagesPrefix.length).isEmpty
        ? '/'
        : path.substring(_pagesPrefix.length);
    final query = req.uri.queryParameters;
    try {
      final html = await renderJasprPage(route, query: query);
      inc('dart_pages_rendered_total');
      req.response
        ..statusCode = HttpStatus.ok
        ..headers.contentType = ContentType.html
        ..headers.set('cache-control', 'no-cache')
        ..write(html);
      await req.response.close();
    } catch (e, st) {
      inc('dart_pages_render_error_total');
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
  if (method == 'GET' &&
      (path == _appPrefix || path.startsWith('$_appPrefix/'))) {
    final rel = path == _appPrefix ? '' : path.substring(_appPrefix.length + 1);
    inc('dart_app_requests_total');
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
  if (method == 'GET' &&
      (path == _mobilePrefix || path.startsWith('$_mobilePrefix/'))) {
    final rel = path == _mobilePrefix
        ? ''
        : path.substring(_mobilePrefix.length + 1);
    inc('dart_mobile_requests_total');
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

  // ---- Flutter asset files ----------------------------------------------
  if (method == 'GET' && path.startsWith('$_assetsPrefix/')) {
    final rel = path.substring(_assetsPrefix.length + 1);
    inc('dart_assets_requests_total');
    final served = await staticFiles.tryServe(req, requestPath: rel);
    if (!served) {
      await _plain(req, 'asset not found\n', status: HttpStatus.notFound);
    }
    return;
  }

  // ---- Root → SSR home --------------------------------------------------
  if (method == 'GET' && (path == '/' || path == '/dart' || path == '/dart/')) {
    req.response
      ..statusCode = HttpStatus.movedPermanently
      ..headers.set('location', '/dart/pages');
    await req.response.close();
    return;
  }

  // ---- Fallback ---------------------------------------------------------
  inc('dart_http_404_total');
  await _plain(req, 'not_found\n', status: HttpStatus.notFound);
}

String _joinPath(String dir, String fileName) {
  if (dir.endsWith(Platform.pathSeparator)) return '$dir$fileName';
  return '$dir${Platform.pathSeparator}$fileName';
}

Future<void> _generatedFile(
  HttpRequest req,
  String apiDocsDirPath,
  String fileName, {
  required String contentType,
}) async {
  final file = File(_joinPath(apiDocsDirPath, fileName));
  if (!await file.exists()) {
    await _plain(req, 'generated API docs not found\n', status: HttpStatus.notFound);
    return;
  }
  await _plain(req, await file.readAsString(), contentType: contentType);
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
