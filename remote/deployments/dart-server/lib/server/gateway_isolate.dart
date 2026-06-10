/// Self-contained WS gateway shard.
///
/// Topology
/// --------
/// The dd-dart-server process runs N gateway shards behind one TCP
/// listening port (8089). Each shard is a separate Dart isolate that
/// owns its own:
///
///   * HttpServer (bound `shared: true` so the kernel SO_REUSEPORT
///     hash distributes incoming connections across the shard pool),
///   * EventBus / Presence / ConversationRegistry,
///   * SessionSupervisor + the host-isolate pool it manages,
///   * per-shard `Metrics` (counters tracked locally and forwarded to
///     the coordinator as [MetricEvent]s; gauges snapshotted in
///     periodic [GaugeReport]s).
///
/// What this shard does NOT own:
///
///   * the dedicated HTTP isolate (pages / app / mobile / assets /
///     /metrics / /healthz / /readyz / /dart/admin/*) — that runs on
///     port 8090, owned by the main coordinator,
///   * any cross-shard state — sessions on shard A do not see bus
///     publishes from shard B. For the WS load-test workload that's
///     fine; for real product workloads use Postgres or a sidecar
///     registry isolate.
///
/// Shard lifecycle
/// ---------------
///   spawn → handshake (sends control SendPort to coordinator) →
///   server.bind → accept loop → ShardShutdown → drain → exit.
///
/// Errors raised inside one session never escape its host isolate; a
/// host crash is observed by the supervisor and tears down only that
/// host's attached sessions, leaving sibling shards completely
/// unaffected.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import '../shared/wire_messages.dart';
import 'conversation_registry.dart';
import 'event_bus.dart';
import 'metrics.dart';
import 'presence.dart';
import 'session_supervisor.dart';

const String _wssPath = '/dart/wss';

/// Entry point handed to `Isolate.spawn(gatewayShardEntry, boot)`. Runs
/// for the lifetime of the shard.
///
/// The body runs inside a [runZonedGuarded] so an uncaught asynchronous
/// error in the accept loop, a control-message handler, or a forwarded
/// callback cannot escape to the VM. The shard is already spawned
/// `errorsAreFatal: false`, but the zone gives us a clean, rate-limited
/// log plus a forwarded `dart_gateway_shard_uncaught_errors_total` counter
/// AND keeps the shard's HttpServer accept loop alive — losing a shard
/// would strand every WebSocket the kernel routed to it (~connections ÷
/// shards), so we work hard to keep it running.
Future<void> gatewayShardEntry(GatewayShardBoot boot) async {
  await runZonedGuarded(() async {
    try {
      await _runShard(boot);
    } catch (e, st) {
      _logShardUncaught(boot, e, st);
    }
  }, (error, stack) => _logShardUncaught(boot, error, stack));
}

int _shardUncaughtErrors = 0;
int _shardUncaughtLoggedAtMs = 0;

/// Zone handler for a gateway shard. Forwards a counter to the coordinator
/// (so /metrics surfaces shard error volume) and logs a rate-limited line.
void _logShardUncaught(GatewayShardBoot boot, Object error, StackTrace st) {
  _shardUncaughtErrors++;
  try {
    boot.metricsBus.send(const MetricEvent('dart_gateway_shard_uncaught_errors_total'));
  } catch (_) {/* coordinator gone or in shutdown */}
  final now = DateTime.now().millisecondsSinceEpoch;
  if (now - _shardUncaughtLoggedAtMs < 1000) return;
  _shardUncaughtLoggedAtMs = now;
  // ignore: avoid_print
  print(jsonEncode({
    'event': 'gateway_shard_uncaught_error',
    'shard': boot.shardId,
    'total': _shardUncaughtErrors,
    'error': '$error',
    'stack': st.toString().split('\n').take(3).join(' | '),
  }));
}

Future<void> _runShard(GatewayShardBoot boot) async {
  // Per-shard `Metrics` subclass. Counters increment locally AND get
  // forwarded as [MetricEvent]s to the coordinator so the canonical
  // /metrics on port 8088 stays globally accurate. Gauges only live
  // here; we periodically push their snapshots to the coordinator
  // via [GaugeReport].
  final metrics = _ForwardingMetrics(boot.metricsBus);

  final bus = EventBus(metrics: metrics);
  final presence = Presence();
  final conversations = ConversationRegistry();
  final supervisor = SessionSupervisor(
    metrics: metrics,
    bus: bus,
    presence: presence,
    conversations: conversations,
    sessionsPerHost: boot.sessionsPerHost,
    idleTimeoutSeconds: boot.idleTimeoutSeconds,
    maxAgeSeconds: boot.maxAgeSeconds,
    ageBasedIdleSeconds: boot.ageBasedIdleSeconds,
    maxInboundBytes: boot.maxInboundBytes,
    maxOutboundRatePerSecond: boot.maxOutboundRatePerSecond,
    slowClientWindows: boot.slowClientWindows,
    clockIntervalSeconds: boot.clockIntervalSeconds,
    benchmarkMode: boot.benchmarkMode,
    poolControllerEnabled: boot.poolControllerEnabled,
    poolMinWarmHosts: boot.poolMinWarmHosts,
    poolMaxHosts: boot.poolMaxHosts,
    poolReconcileMaxSpawnPerTick: boot.poolReconcileMaxSpawnPerTick,
    poolRetireCooldownMs: boot.poolRetireCooldownMs,
  );

  // Control port: the coordinator sends [ShardShutdown] when the pod
  // gets SIGTERM. We acknowledge by closing the listener and asking
  // the supervisor to drain.
  final control = ReceivePort('dd-dart-shard-${boot.shardId}-control');
  boot.handshake.send(control.sendPort);

  // Pre-register gauge readers so they're consistent with what the
  // coordinator sees at /metrics. We don't render them locally (no
  // /metrics endpoint on this isolate), but it keeps the API uniform.
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
    ..registerGauge('dart_pool_idle_hosts', () => supervisor.idleHostCount)
    ..registerGauge('dart_pool_free_slots', () => supervisor.freeSlots)
    ..registerGauge('dart_pool_target_hosts', () => supervisor.targetHosts)
    ..registerGauge('dart_ws_idle_timeout_seconds',
        () => supervisor.idleTimeoutSeconds)
    ..registerGauge('dart_ws_max_age_seconds',
        () => supervisor.maxAgeSeconds)
    ..registerGauge('dart_ws_age_based_idle_seconds',
        () => supervisor.ageBasedIdleSeconds)
    ..registerGauge('dart_ws_clock_interval_seconds',
        () => supervisor.clockIntervalSeconds)
    ..registerGauge('dart_eventbus_topics', () => bus.topicCount)
    ..registerGauge('dart_eventbus_sessions', () => bus.sessionCount)
    ..registerGauge('dart_eventbus_total_joins', () => bus.totalJoinCount)
    ..registerGauge('dart_presence_users', () => presence.userCount)
    ..registerGauge('dart_presence_sessions', () => presence.sessionCount)
    ..registerGauge('dart_conversations',
        () => conversations.conversationCount)
    ..registerGauge('dart_conversation_memberships',
        () => conversations.totalMemberships);

  // Periodic gauge snapshot to the coordinator. The coordinator keeps
  // a per-shard map keyed by [boot.shardId] and renders summed values
  // in its own /metrics.
  final reportTimer = Timer.periodic(
      Duration(milliseconds: boot.gaugeReportIntervalMs), (_) {
    try {
      final values = <String, num>{
        'dart_sessions_live': supervisor.liveCount,
        'dart_sessions_spawned': supervisor.spawnedTotal,
        'dart_session_hosts_live': supervisor.hostCount,
        'dart_session_hosts_spawned': supervisor.hostsSpawnedTotal,
        'dart_session_hosts_terminated': supervisor.hostsTerminatedTotal,
        'dart_sessions_per_host_cap': supervisor.sessionsPerHost,
        'dart_pool_idle_hosts': supervisor.idleHostCount,
        'dart_pool_free_slots': supervisor.freeSlots,
        'dart_pool_target_hosts': supervisor.targetHosts,
        'dart_ws_idle_timeout_seconds': supervisor.idleTimeoutSeconds,
        'dart_ws_max_age_seconds': supervisor.maxAgeSeconds,
        'dart_ws_age_based_idle_seconds': supervisor.ageBasedIdleSeconds,
        'dart_ws_clock_interval_seconds': supervisor.clockIntervalSeconds,
        'dart_eventbus_topics': bus.topicCount,
        'dart_eventbus_sessions': bus.sessionCount,
        'dart_eventbus_total_joins': bus.totalJoinCount,
        'dart_presence_users': presence.userCount,
        'dart_presence_sessions': presence.sessionCount,
        'dart_conversations': conversations.conversationCount,
        'dart_conversation_memberships': conversations.totalMemberships,
      };
      boot.metricsBus.send(GaugeReport(shardId: boot.shardId, values: values));
    } catch (e, st) {
      _logShardUncaught(boot, e, st);
    }
  });

  final HttpServer server;
  try {
    server = await HttpServer.bind(boot.host, boot.port, shared: true);
  } catch (e, st) {
    // ignore: avoid_print
    print(jsonEncode({
      'event': 'gateway_shard_bind_failed',
      'shard': boot.shardId,
      'host': boot.host,
      'port': boot.port,
      'error': '$e',
      'stack': '$st',
    }));
    rethrow;
  }
  server.autoCompress = true;
  // ignore: avoid_print
  print(jsonEncode({
    'event': 'gateway_shard_listening',
    'shard': boot.shardId,
    'host': boot.host,
    'port': server.port,
    'sessions_per_host': boot.sessionsPerHost,
    'idle_timeout_seconds': boot.idleTimeoutSeconds,
    'max_age_seconds': boot.maxAgeSeconds,
    'age_based_idle_seconds': boot.ageBasedIdleSeconds,
    'benchmark_mode': boot.benchmarkMode,
  }));

  // Listen for ShardShutdown from the coordinator. Idempotent.
  var shuttingDown = false;
  Future<void> drain() async {
    if (shuttingDown) return;
    shuttingDown = true;
    // ignore: avoid_print
    print(jsonEncode({
      'event': 'gateway_shard_drain_begin',
      'shard': boot.shardId,
      'live_sessions': supervisor.liveCount,
      'live_hosts': supervisor.hostCount,
    }));
    try {
      await server.close(force: false);
    } catch (_) {/* swallow */}
    supervisor.requestDrain();
  }

  control.listen((msg) {
    try {
      if (msg is ShardShutdown) {
        unawaited(drain());
      } else if (msg is ShardPoolDirective) {
        // MDP autotuner setpoint from the coordinator: reconcile the warm
        // pool toward the per-shard host-isolate target and adopt the
        // chosen per-host density (`dart_sessions_per_host_cap` tracks it).
        supervisor.applyTargetHosts(
          msg.targetHosts,
          sessionsPerHost: msg.sessionsPerHost,
        );
      } else if (msg is ShardDebugCrashHost) {
        // Chaos hook (coordinator only sends this with WS_DEBUG_CRASH set):
        // simulate a host crash to exercise the supervisor teardown path.
        final lost = supervisor.debugKillOneHost();
        // ignore: avoid_print
        print(jsonEncode({
          'event': 'gateway_shard_debug_crash_host',
          'shard': boot.shardId,
          'sessions_on_killed_host': lost,
        }));
      }
    } catch (e, st) {
      // A malformed/edge-case directive must not take down the shard's
      // control plane (which would also stop draining on SIGTERM).
      _logShardUncaught(boot, e, st);
    }
  });

  // Main accept loop. Each accepted request is dispatched to a
  // microtask so the loop itself never blocks on a slow upgrade.
  await for (final req in server) {
    metrics.inc('dart_http_requests_total');
    unawaited(_route(
      req,
      shardId: boot.shardId,
      metrics: metrics,
      supervisor: supervisor,
      allowedOrigins: boot.allowedOrigins,
    ));
  }

  reportTimer.cancel();
  control.close();
  await supervisor.close();
  await conversations.close();
  await presence.close();
  await bus.close();
  await metrics.close();
}

/// `Metrics` subclass that increments locally AND forwards every
/// counter mutation to the coordinator as a [MetricEvent]. The
/// coordinator never asks the shard for rendered exposition — its
/// /metrics is rendered from the canonical (main-isolate) `Metrics`
/// instance — but the local copy keeps gauges callable inside the
/// shard for the periodic [GaugeReport] path.
class _ForwardingMetrics extends Metrics {
  _ForwardingMetrics(this._bus);
  final SendPort _bus;

  @override
  void inc(String name, [int delta = 1]) {
    super.inc(name, delta);
    try {
      _bus.send(MetricEvent(name, delta));
    } catch (_) {/* coordinator gone or in shutdown */}
  }

  @override
  void observe(String name, double value, {List<double>? bounds}) {
    // Histograms are rendered only on the coordinator; forward the sample
    // (as integer microseconds) instead of keeping a per-shard copy that
    // nothing renders. The coordinator folds every shard's samples into
    // one canonical histogram.
    try {
      _bus.send(ObserveEvent(name, (value * 1000000.0).round()));
    } catch (_) {/* coordinator gone or in shutdown */}
  }
}

Future<void> _route(
  HttpRequest req, {
  required int shardId,
  required Metrics metrics,
  required SessionSupervisor supervisor,
  List<String> allowedOrigins = const <String>[],
}) async {
  final path = req.uri.path;
  final method = req.method.toUpperCase();

  // Cheap healthz mirror so probes can hit the WS port too.
  if (method == 'GET' && path == '/healthz') {
    await _plain(req, 'ok\n');
    return;
  }

  if (method == 'GET' && path == _wssPath) {
    if (!WebSocketTransformer.isUpgradeRequest(req)) {
      await _plain(
        req,
        'expected websocket upgrade\n',
        status: HttpStatus.upgradeRequired,
      );
      return;
    }
    // Cross-site WebSocket hijacking (CSWSH) defence. The browser sets
    // `Origin` on a WS handshake but — unlike fetch/XHR — the same-origin
    // policy does NOT block cross-origin WebSocket connections, so without
    // this check any page in a victim's browser could open `/dart/wss` and
    // drive the full protocol as that browser. We only enforce when an
    // allowlist is configured AND the request actually carries an Origin
    // (non-browser clients omit it), so load testers and server-to-server
    // callers are unaffected.
    if (allowedOrigins.isNotEmpty) {
      final origin = req.headers.value('origin');
      if (origin != null && !allowedOrigins.contains(origin)) {
        metrics.inc('dart_wss_upgrade_rejected_origin_total');
        await _plain(
          req,
          'forbidden_origin\n',
          status: HttpStatus.forbidden,
        );
        return;
      }
    }
    if (supervisor.isDraining) {
      metrics.inc('dart_wss_upgrade_refused_draining_total');
      try {
        final s = await WebSocketTransformer.upgrade(req);
        await s.close(1012, 'server_draining');
      } catch (_) {/* swallow */}
      return;
    }
    metrics.inc('dart_wss_upgrade_total');
    final WebSocket socket;
    try {
      socket = await WebSocketTransformer.upgrade(req);
    } catch (e) {
      metrics.inc('dart_wss_upgrade_failed_total');
      try {
        await _plain(req, 'upgrade_failed\n',
            status: HttpStatus.internalServerError);
      } catch (_) {/* swallow */}
      return;
    }
    final sessionId = _newSessionId(shardId);
    final remote = req.connectionInfo?.remoteAddress.address ?? 'unknown';
    final headers = <String, String>{};
    req.headers.forEach((k, v) => headers[k] = v.join(','));
    try {
      await supervisor.adopt(
        socket,
        sessionId: sessionId,
        remoteAddr: remote,
        requestPath: path,
        headers: headers,
      );
    } catch (e, st) {
      metrics.inc('dart_sessions_adopt_failed_total');
      // ignore: avoid_print
      print(jsonEncode({
        'event': 'wss_session_error',
        'shard': shardId,
        'session_id': sessionId,
        'error': '$e',
        'stack': '$st',
      }));
      try {
        await socket.close(1011, 'server_overloaded');
      } catch (_) {/* swallow */}
    }
    return;
  }

  // Anything else is a misrouted ingress (operator should hit the
  // HTTP isolate's port for /dart/pages, /admin, etc.).
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

/// Short, URL-safe per-shard session id. Encodes the shard id so the
/// coordinator's `/metrics` and any future cross-shard routing can
/// tell which shard a session lives on.
String _newSessionId(int shardId) {
  final us = DateTime.now().microsecondsSinceEpoch;
  final rnd = (us * 2654435761) & 0xffffffff;
  final shardHex = shardId.toRadixString(16).padLeft(2, '0');
  return '$shardHex-${rnd.toRadixString(36).padLeft(7, '0')}';
}
