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
///   GET  /docs/api, /api/docs, /api/docs.json — generated API docs
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
import 'package:dd_dart_server/server/optimizer_client.dart';
import 'package:dd_dart_server/server/pool_autotuner.dart';
import 'package:dd_dart_server/server/pool_shield.dart';
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

/// Default MDP autotuner control-loop cadence. Each tick the coordinator
/// reads aggregated telemetry, picks a pod-wide host-isolate target, and
/// broadcasts a per-shard [ShardPoolDirective]. Override with
/// `WS_MDP_CONTROL_INTERVAL_MS`.
const int _kDefaultMdpControlIntervalMs = 5000;

/// Default discrete host-isolate ladder the autotuner chooses between.
/// Override with `WS_POOL_SIZE_LEVELS` (comma-separated).
const List<int> _kDefaultPoolSizeLevels = <int>[20, 30, 40, 50];

/// Default discrete per-host density ladder (`sessionsPerHost` caps) the
/// autotuner jointly tunes alongside pool size. Override with
/// `WS_HOST_DENSITY_LEVELS` (comma-separated). A single value collapses the
/// density dimension back to the pool-size-only experiment.
const List<int> _kDefaultHostDensityLevels = <int>[100, 250, 500, 1000];

/// Default pod-wide ceiling on session-host isolates, summed across shards.
/// The per-shard cap derives from this (`ceil(pod / shards)`). Sized so the
/// storm-dampening shield ([shieldPoolDirective]) can hold ~50K WS at a
/// *mid* density level (≈500/host ⇒ ~115 hosts) with cold-start headroom,
/// while still bounding host-isolate base-heap RAM well under the 6 GB pod
/// target — the load test peaked at ~2.3 Gi with ~58 hosts, so ~192 idle-cap
/// isolates leaves comfortable margin. It is the shield's memory governor:
/// the density feasibility floor rises as this falls. Override with
/// `WS_POOL_MAX_HOSTS_PER_SHARD` (per-shard, not pod-wide).
const int _kDefaultMaxHostIsolatesPerPod = 192;

/// Parse the `WS_MDP_MODE` env var. `off` (default) keeps the legacy
/// lazy-spawn supervisor; `local` runs the in-process Q-learner; `remote`
/// delegates to the `dd-mdp-optimizer` service (falling back to a held
/// setpoint when it is unreachable).
enum MdpMode { off, local, remote }

MdpMode _parseMdpMode(String? raw) {
  switch ((raw ?? '').toLowerCase().trim()) {
    case 'local':
      return MdpMode.local;
    case 'remote':
      return MdpMode.remote;
    case 'on':
    case 'true':
      return MdpMode.local;
    default:
      return MdpMode.off;
  }
}

/// Parse a comma-separated, positive, ascending integer ladder (e.g.
/// `WS_POOL_SIZE_LEVELS` / `WS_HOST_DENSITY_LEVELS`). Falls back to
/// [fallback] when unset/empty/all-invalid.
List<int> _parseIntLevels(String? raw, List<int> fallback) {
  if (raw == null || raw.trim().isEmpty) return fallback;
  final parsed = raw
      .split(',')
      .map((s) => int.tryParse(s.trim()))
      .whereType<int>()
      .where((n) => n > 0)
      .toSet()
      .toList()
    ..sort();
  return parsed.isEmpty ? fallback : parsed;
}

double _envDouble(String name, double fallback) =>
    double.tryParse(Platform.environment[name] ?? '') ?? fallback;

/// Coerce a possibly-NaN/Infinity/negative metric reading into a finite,
/// non-negative double so it can't poison the MDP learner's reward math.
double _finiteNonNeg(double v) => (v.isFinite && v > 0) ? v : 0.0;

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
  final apiDocsDirPath = Platform.environment['API_DOCS_DIR'] ?? './generated';
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
  // Optional WebSocket Origin allowlist (CSWSH defence). Comma-separated
  // exact scheme+host[:port] values; empty/unset accepts any origin (current
  // behaviour, so load tests / same-origin demos are unaffected). A request
  // with no Origin header (non-browser clients) is always allowed; the check
  // only rejects a *present, non-matching* browser Origin.
  final wsAllowedOrigins = (Platform.environment['WS_ALLOWED_ORIGINS'] ?? '')
      .split(',')
      .map((s) => s.trim())
      .where((s) => s.isNotEmpty)
      .toList(growable: false);
  // Chaos / fault-injection switch. OFF by default. When enabled, the
  // admin port exposes `POST /dart/admin/debug/crash-host`, which forces a
  // shard to hard-kill one session-host isolate so the supervisor teardown
  // path can be exercised under load. Admin port is non-public (see
  // AGENTS.md access posture); never enable on an Internet-exposed path.
  final wsDebugCrash = const {'1', 'true'}.contains(
      Platform.environment['WS_DEBUG_CRASH']?.toLowerCase().trim());
  // Optional shared-secret gate on the mutating/sensitive admin surface
  // (`/dart/admin/*`, incl. the debug crash-host hook). Defense-in-depth on
  // top of the "admin port is not routed publicly" posture. Unset/empty
  // disables the check (current behaviour). Probes (`/healthz`, `/readyz`)
  // and `/metrics` are intentionally NOT gated so Prometheus keeps scraping
  // without credentials. Callers present the token as `Authorization: Bearer
  // <token>` or `X-Admin-Token: <token>`.
  final adminAuthToken = Platform.environment['ADMIN_AUTH_TOKEN']?.trim();

  // ---- MDP isolate-pool autotuner config --------------------------------
  final mdpMode = _parseMdpMode(Platform.environment['WS_MDP_MODE']);
  final mdpEnabled = mdpMode != MdpMode.off;
  final poolSizeLevels = _parseIntLevels(
    Platform.environment['WS_POOL_SIZE_LEVELS'],
    _kDefaultPoolSizeLevels,
  );
  final hostDensityLevels = _parseIntLevels(
    Platform.environment['WS_HOST_DENSITY_LEVELS'],
    _kDefaultHostDensityLevels,
  );
  final mdpControlIntervalMs = int.tryParse(
        Platform.environment['WS_MDP_CONTROL_INTERVAL_MS'] ?? '',
      ) ??
      _kDefaultMdpControlIntervalMs;
  final poolMinWarmHosts =
      int.tryParse(Platform.environment['WS_POOL_MIN_WARM_HOSTS'] ?? '') ?? 1;
  // Hard per-shard host ceiling. The default is the larger of two bounds:
  //   * the top size level's fair share across shards (+ cold-start headroom),
  //     so a single shard can't fork past its slice of the largest target; and
  //   * the pod-wide memory governor (`_kDefaultMaxHostIsolatesPerPod`) split
  //     across shards, which gives the storm-dampening shield enough room to
  //     hold the offered load at a *lower* density without slamming into the
  //     ceiling (the failure mode that produced refusal storms when the
  //     ceiling was derived from the size ladder alone, i.e. only feasible at
  //     the top density).
  final poolMaxHostsBySize = (poolSizeLevels.last / gatewayShards).ceil() + 2;
  final poolMaxHostsByMem =
      (_kDefaultMaxHostIsolatesPerPod / gatewayShards).ceil();
  final poolMaxHostsPerShard = int.tryParse(
        Platform.environment['WS_POOL_MAX_HOSTS_PER_SHARD'] ?? '',
      ) ??
      (poolMaxHostsBySize > poolMaxHostsByMem
          ? poolMaxHostsBySize
          : poolMaxHostsByMem);
  final poolRetireCooldownMs = int.tryParse(
        Platform.environment['WS_POOL_RETIRE_COOLDOWN_MS'] ?? '',
      ) ??
      15000;
  final mdpOptimizerUrl = Platform.environment['WS_MDP_OPTIMIZER_URL'] ??
      'http://dd-mdp-optimizer.default.svc.cluster.local:8096';

  // Storm-dampening shield: clamps the broadcast pool directive into the
  // feasible, memory-bounded region so an exploratory (size, density) pick
  // can't starve the live load and trigger a cold-start / refusal storm. On
  // by default; set `WS_MDP_SHIELD=false` to broadcast raw policy choices.
  final mdpShieldEnabled =
      (Platform.environment['WS_MDP_SHIELD']?.toLowerCase().trim() ?? 'true') !=
          'false';
  final mdpCapacityHeadroom = _envDouble('WS_MDP_CAPACITY_HEADROOM', 0.2);
  final mdpDensityMaxDrop = _envDouble('WS_MDP_DENSITY_MAX_DROP', 0.5);
  // Pod-wide host-isolate budget the shield enforces, *independent* of the
  // operator's hard refusal ceiling (`WS_POOL_MAX_HOSTS_PER_SHARD`). This is
  // what keeps the warm pool + density memory-safe even when the refusal
  // ceiling is left unbounded (`0`, the "never shed connections" posture):
  // the shield's density floor rises as this budget falls, packing the load
  // onto at most this many isolates. Override with `WS_MDP_MAX_HOST_BUDGET`.
  final mdpMaxHostBudget =
      int.tryParse(Platform.environment['WS_MDP_MAX_HOST_BUDGET'] ?? '') ??
          _kDefaultMaxHostIsolatesPerPod;

  final autotunerConfig = AutotunerConfig(
    sizeLevels: poolSizeLevels,
    densityLevels: hostDensityLevels,
    alpha: _envDouble('WS_MDP_ALPHA', 0.2),
    gamma: _envDouble('WS_MDP_GAMMA', 0.6),
    epsilonStart: _envDouble('WS_MDP_EPSILON', 0.30),
    epsilonMin: _envDouble('WS_MDP_EPSILON_MIN', 0.02),
    epsilonDecay: _envDouble('WS_MDP_EPSILON_DECAY', 0.995),
    targetLatencySeconds: _envDouble('WS_MDP_TARGET_LATENCY_SECONDS', 0.05),
    wLatency: _envDouble('WS_MDP_W_LATENCY', 1.0),
    wColdStart: _envDouble('WS_MDP_W_COLD_START', 0.05),
    wRefusal: _envDouble('WS_MDP_W_REFUSAL', 0.5),
    wIdle: _envDouble('WS_MDP_W_IDLE', 0.02),
    wSize: _envDouble('WS_MDP_W_SIZE', 0.2),
  );
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
    } else if (msg is ObserveEvent) {
      // Latency samples from every shard fold into one canonical
      // histogram so /metrics renders a pod-wide adopt / first-frame
      // latency distribution (and the autotuner reads its p99).
      metrics.observe(msg.name, msg.micros / 1000000.0);
    }
  });

  // ---- HTTP isolate ------------------------------------------------------
  final httpHandshake = ReceivePort('dd-dart-http-isolate-handshake');
  final httpIsolate = await Isolate.spawn<HttpIsolateBoot>(
    httpIsolateEntry,
    HttpIsolateBoot(
      handshake: httpHandshake.sendPort,
      host: host,
      port: httpInternalPort,
      staticDirPath: staticDirPath,
      mobileStaticDirPath: mobileStaticDirPath,
      apiDocsDirPath: apiDocsDirPath,
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
  // Flipped true at the start of `beginShutdown` so the exit handler can
  // tell an intentional drain-kill (don't respawn) apart from an
  // unexpected shard death (self-heal by respawning). Closed over by both
  // `spawnShard` (below) and `beginShutdown` (later in this scope).
  var shardsDraining = false;
  var shardRespawns = 0;
  // Cap respawns so a deterministically-crashing shard can't spin a hot
  // restart loop forever; past the cap we leave the pool degraded and let
  // k8s liveness recycle the pod.
  const maxShardRespawns = 100;

  Future<void> spawnShard(int shardId) async {
    final handshake = ReceivePort('dd-dart-shard-$shardId-handshake');
    final exit = ReceivePort('dd-dart-shard-$shardId-exit');
    final error = ReceivePort('dd-dart-shard-$shardId-error');
    final Isolate isolate;
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
          allowedOrigins: wsAllowedOrigins,
          poolControllerEnabled: mdpEnabled,
          poolMinWarmHosts: poolMinWarmHosts,
          poolMaxHosts: poolMaxHostsPerShard,
          poolRetireCooldownMs: poolRetireCooldownMs,
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
      // Self-heal. An UNEXPECTED shard exit (not part of pod drain)
      // strands every WS the kernel had routed to that listener, and
      // there is no other way to get the SO_REUSEPORT pool back to full
      // width. Respawn a replacement after a short backoff. Established
      // connections on sibling shards are unaffected; only NEW accepts
      // rebalance across the (briefly narrower, then restored) pool.
      if (shardsDraining || shardRespawns >= maxShardRespawns) return;
      shardRespawns++;
      metrics.inc('dart_gateway_shards_respawned_total');
      // ignore: avoid_print
      print(jsonEncode({
        'event': 'gateway_shard_respawn',
        'shard': shardId,
        'respawns': shardRespawns,
      }));
      Timer(const Duration(seconds: 1), () {
        if (shardsDraining) return;
        unawaited(spawnShard(shardId).catchError((Object e, StackTrace st) {
          // ignore: avoid_print
          print(jsonEncode({
            'event': 'gateway_shard_respawn_failed',
            'shard': shardId,
            'error': '$e',
          }));
        }));
      });
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

  for (var shardId = 0; shardId < gatewayShards; shardId++) {
    await spawnShard(shardId);
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
    'ws_allowed_origins':
        wsAllowedOrigins.isEmpty ? 'any' : wsAllowedOrigins,
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
        () => _sumShardGauge('dart_conversation_memberships'))
    ..registerGauge('dart_pool_idle_hosts',
        () => _sumShardGauge('dart_pool_idle_hosts'))
    ..registerGauge('dart_pool_free_slots',
        () => _sumShardGauge('dart_pool_free_slots'))
    ..registerGauge('dart_pool_target_hosts',
        () => _sumShardGauge('dart_pool_target_hosts'));

  // ---- MDP isolate-pool autotuner ---------------------------------------
  // Coordinator-side control loop. Each tick it reads aggregated telemetry
  // (live sessions/hosts, cold-start + refusal counter deltas, p99 adopt /
  // first-frame latency), asks the policy for a pod-wide host-isolate
  // target from the {20,30,40,50} ladder, divides it across the live
  // shards, and pushes a `ShardPoolDirective` to each. Behind WS_MDP_MODE;
  // `off` skips all of this and leaves shards in legacy lazy-spawn mode.
  OptimizerClient? optimizerClient;
  Timer? mdpTimer;
  var mdpLastTargetGlobal = poolSizeLevels[(poolSizeLevels.length / 2).floor()];
  // Last per-host density the autotuner chose (broadcast to every shard).
  // Seeded from the static SESSIONS_PER_HOST so the first directive is a
  // no-op until the learner moves it.
  var mdpLastDensity = hostDensityLevels[
      (hostDensityLevels.length / 2).floor()];
  var mdpPrevSessions = 0;
  var mdpPrevColdStarts = 0;
  var mdpPrevRefusals = 0;

  if (mdpEnabled) {
    // `final` locals so the timer / gauge closures promote them to
    // non-null cleanly. Exactly one is non-null per mode.
    final autotuner =
        mdpMode == MdpMode.local ? PoolAutotuner(autotunerConfig) : null;
    final optimizer = mdpMode == MdpMode.remote
        ? OptimizerClient(baseUrl: mdpOptimizerUrl)
        : null;
    optimizerClient = optimizer; // hand to outer scope for shutdown close()

    // Autotuner telemetry as coordinator-local gauges.
    metrics
      ..registerGauge('dart_pool_target_hosts_global', () => mdpLastTargetGlobal)
      ..registerGauge('dart_pool_target_density', () => mdpLastDensity)
      ..registerGauge('dart_pool_size_levels_count', () => poolSizeLevels.length)
      ..registerGauge(
          'dart_pool_density_levels_count', () => hostDensityLevels.length)
      ..registerGauge(
          'dart_pool_autotuner_mode',
          () => switch (mdpMode) {
                MdpMode.local => 1,
                MdpMode.remote => 2,
                MdpMode.off => 0,
              })
      ..registerGauge(
          'dart_pool_shield_enabled', () => mdpShieldEnabled ? 1 : 0)
      ..registerGauge('dart_pool_shield_max_hosts', () {
        final live = shards.where((s) => !s.dead).length;
        final supCeil = poolMaxHostsPerShard > 0 ? poolMaxHostsPerShard * live : 0;
        if (supCeil <= 0) return mdpMaxHostBudget;
        return supCeil < mdpMaxHostBudget ? supCeil : mdpMaxHostBudget;
      });
    if (autotuner != null) {
      metrics
        ..registerGauge('dart_pool_autotuner_epsilon', () => autotuner.epsilon)
        ..registerGauge(
            'dart_pool_autotuner_reward_ema', () => autotuner.rewardEma)
        ..registerGauge('dart_pool_autotuner_updates', () => autotuner.updates)
        ..registerGauge('dart_pool_autotuner_states_visited',
            () => autotuner.statesVisited);
    }

    Future<void> mdpTick() async {
      final liveShards = shards.where((s) => !s.dead).toList(growable: false);
      if (liveShards.isEmpty) return;

      final totalSessions =
          (metrics.gauge('dart_sessions_live') ?? 0).toInt();
      final liveHosts =
          (metrics.gauge('dart_session_hosts_live') ?? 0).toInt();
      final idleHosts = (metrics.gauge('dart_pool_idle_hosts') ?? 0).toInt();
      // Ground-truth per-host density currently applied across shards (the
      // max per-shard cap; all shards share the broadcast density). Used for
      // the utilisation/capacity signal so the observation reflects what the
      // pool is actually doing, not last tick's chosen-but-not-yet-applied
      // setpoint. Falls back to the static cap before the first GaugeReport.
      final appliedDensityRaw =
          (metrics.gauge('dart_sessions_per_host_cap') ?? 0).toInt();
      final appliedDensity =
          appliedDensityRaw > 0 ? appliedDensityRaw : sessionsPerHost;
      final coldStartsTotal =
          metrics.counter('dart_session_cold_start_spawns_total');
      final refusalsTotal =
          metrics.counter('dart_sessions_refused_capacity_total');
      final coldStarts =
          (coldStartsTotal - mdpPrevColdStarts).clamp(0, 1 << 30).toInt();
      final refusals =
          (refusalsTotal - mdpPrevRefusals).clamp(0, 1 << 30).toInt();
      // Sanitise the latency quantiles before they feed the policy. An empty
      // histogram (no samples yet, or right after a reset) can yield NaN /
      // Infinity from `histogramQuantile`; an un-guarded NaN would flow into
      // the Q-learner's reward and EMA and poison every subsequent decision
      // (NaN propagates and never recovers). Clamp to a finite, non-negative
      // second value.
      final p99Adopt = _finiteNonNeg(
          metrics.histogramQuantile('dart_ws_adopt_latency_seconds', 0.99));
      final p99FirstFrame = _finiteNonNeg(metrics.histogramQuantile(
        'dart_ws_first_frame_latency_seconds',
        0.99,
      ));

      var targetGlobal = mdpLastTargetGlobal;
      var targetDensity = mdpLastDensity;
      if (optimizer != null) {
        final capacity = liveHosts * appliedDensity;
        final util = capacity > 0 ? totalSessions / capacity : 0.0;
        final windowS = mdpControlIntervalMs / 1000.0;
        final remote = await optimizer.recommend(
          OptimizerSignals(
            utilization: util,
            coldStartRate: windowS > 0 ? coldStarts / windowS : 0.0,
            refusalRate: windowS > 0 ? refusals / windowS : 0.0,
            p99AdoptSeconds: p99Adopt,
            windowMs: mdpControlIntervalMs,
            sizeLevels: poolSizeLevels,
            densityLevels: hostDensityLevels,
          ),
        );
        // Hold each lever's previous setpoint when the optimizer is
        // unreachable or returns an unmappable action for that lever.
        targetGlobal = remote.targetHosts ?? mdpLastTargetGlobal;
        targetDensity = remote.sessionsPerHost ?? mdpLastDensity;
        metrics.inc((remote.targetHosts != null || remote.sessionsPerHost != null)
            ? 'dart_pool_optimizer_ok_total'
            : 'dart_pool_optimizer_miss_total');
      } else if (autotuner != null) {
        final decision = autotuner.decide(AutotunerObservation(
          totalSessions: totalSessions,
          liveHosts: liveHosts,
          sessionsPerHost: appliedDensity,
          sessionsDelta: totalSessions - mdpPrevSessions,
          coldStarts: coldStarts,
          refusals: refusals,
          idleHosts: idleHosts,
          p99AdoptSeconds: p99Adopt,
          p99FirstFrameSeconds: p99FirstFrame,
        ));
        targetGlobal = decision.targetHosts;
        targetDensity = decision.sessionsPerHost;
      }

      // ---- Storm-dampening shield -----------------------------------------
      // Clamp the policy's choice into the feasible, memory-bounded region
      // before it leaves the coordinator. The learner above still recorded
      // its own (size, density) pick; only the broadcast setpoint is
      // corrected, so exploration can never starve the live load into a
      // cold-start / refusal storm. Applies identically to local + remote.
      if (mdpShieldEnabled) {
        // Effective pod-wide host cap for the shield: the operator's hard
        // refusal ceiling when set, else (unbounded ceiling) the shield's own
        // memory budget — and never above that budget. Keeps the shield's
        // feasibility floor / ceiling active regardless of the refusal-ceiling
        // posture, so an unbounded ceiling can't disable the memory governor.
        final supervisorPodCeiling =
            poolMaxHostsPerShard > 0 ? poolMaxHostsPerShard * liveShards.length : 0;
        final shieldMaxHosts = supervisorPodCeiling > 0
            ? (supervisorPodCeiling < mdpMaxHostBudget
                ? supervisorPodCeiling
                : mdpMaxHostBudget)
            : mdpMaxHostBudget;
        final shielded = shieldPoolDirective(
          chosenTargetHosts: targetGlobal,
          chosenDensity: targetDensity,
          lastDensity: mdpLastDensity,
          liveSessions: totalSessions,
          maxTotalHosts: shieldMaxHosts,
          headroom: mdpCapacityHeadroom,
          densityMaxDrop: mdpDensityMaxDrop,
          minDensity: kMinSessionsPerHost,
          maxDensity: kMaxSessionsPerHost,
        );
        targetGlobal = shielded.targetHosts;
        targetDensity = shielded.sessionsPerHost;
        if (shielded.engaged) metrics.inc('dart_pool_shield_engaged_total');
      }

      mdpLastTargetGlobal = targetGlobal;
      mdpLastDensity = targetDensity;
      mdpPrevSessions = totalSessions;
      mdpPrevColdStarts = coldStartsTotal;
      mdpPrevRefusals = refusalsTotal;

      // Split the pod-wide host target across live shards (ceil so the sum
      // is at least the target). Density is a per-host property, so it goes
      // to every shard unchanged. Each shard reconciles toward its slice.
      final perShard = (targetGlobal / liveShards.length).ceil();
      for (final shard in liveShards) {
        try {
          shard.control.send(ShardPoolDirective(
            targetHosts: perShard,
            sessionsPerHost: targetDensity,
          ));
        } catch (_) {/* shard gone mid-tick */}
      }
      metrics.inc('dart_pool_autotuner_ticks_total');
    }

    mdpTimer = Timer.periodic(
      Duration(milliseconds: mdpControlIntervalMs),
      (_) => unawaited(mdpTick()),
    );
    print(jsonEncode({
      'event': 'mdp_autotuner_init',
      'mode': mdpMode.name,
      'size_levels': poolSizeLevels,
      'density_levels': hostDensityLevels,
      'joint_action_count': poolSizeLevels.length * hostDensityLevels.length,
      'control_interval_ms': mdpControlIntervalMs,
      'min_warm_hosts': poolMinWarmHosts,
      'max_hosts_per_shard': poolMaxHostsPerShard,
      'shield': mdpShieldEnabled,
      'shield_host_budget': mdpMaxHostBudget,
      'capacity_headroom': mdpCapacityHeadroom,
      'density_max_drop': mdpDensityMaxDrop,
      'optimizer_url': mdpMode == MdpMode.remote ? mdpOptimizerUrl : null,
    }));
  }

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
    'api_docs_dir': apiDocsDirPath,
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
    // Stop the self-heal respawner before we start killing shards, so the
    // exit-port handler treats these terminations as an intentional drain.
    shardsDraining = true;
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
    mdpTimer?.cancel();
    try {
      await adminServer.close(force: false);
    } catch (_) {/* swallow */}
    httpIsolate.kill(priority: Isolate.immediate);
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

  final sigtermSub = ProcessSignal.sigterm.watch().listen((_) {
    unawaited(beginShutdown('SIGTERM'));
  });
  final sigintSub = ProcessSignal.sigint.watch().listen((_) {
    unawaited(beginShutdown('SIGINT'));
  });

  // ---- Admin request loop ------------------------------------------------
  await for (final req in adminServer) {
    metrics.inc('dart_admin_requests_total');
    // Gate the admin surface behind the shared secret when configured. Only
    // `/dart/admin/*` is protected; probes + `/metrics` fall through so the
    // kubelet and Prometheus keep working uncredentialed.
    if (req.uri.path.startsWith('/dart/admin') &&
        !_adminAuthorized(req, adminAuthToken)) {
      metrics.inc('dart_admin_auth_rejected_total');
      await _plain(req, 'unauthorized\n',
          status: HttpStatus.unauthorized);
      continue;
    }
    // Chaos probe (only mounted when WS_DEBUG_CRASH is set): pick the live
    // shard carrying the most sessions and tell it to hard-kill one host
    // isolate, simulating a crash so we can measure the real blast radius.
    if (wsDebugCrash &&
        req.method.toUpperCase() == 'POST' &&
        req.uri.path == '/dart/admin/debug/crash-host') {
      _GatewayShardHandle? target;
      var bestLive = -1.0;
      for (final s in shards) {
        if (s.dead) continue;
        final live =
            (perShardGauges[s.shardId]?['dart_sessions_live'] ?? 0).toDouble();
        if (target == null || live > bestLive) {
          target = s;
          bestLive = live;
        }
      }
      if (target != null) {
        try {
          target.control.send(const ShardDebugCrashHost());
        } catch (_) {/* swallow */}
        metrics.inc('dart_debug_crash_host_requests_total');
      }
      await _plain(
        req,
        jsonEncode({
          'ok': target != null,
          'shard': target?.shardId,
          'shard_sessions_live': bestLive < 0 ? 0 : bestLive.toInt(),
        }),
        contentType: 'application/json',
      );
      continue;
    }
    unawaited(_routeAdmin(
      req,
      metrics: metrics,
      ready: ready,
      hotReloader: hotReloader,
      pgPool: pgPool,
      presenceConvsRepo: presenceConvsRepo,
    ));
  }

  final shutdown = shuttingDown;
  if (shutdown != null) {
    await shutdown.future;
  }
  for (final shard in shards) {
    await shard.exitSub?.cancel();
    await shard.errorSub?.cancel();
    shard.exit.close();
    shard.error.close();
  }
  await sigtermSub.cancel();
  await sigintSub.cancel();
  mdpTimer?.cancel();
  optimizerClient?.close();
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

/// True when [req] may touch the admin surface. Auth is disabled (always
/// true) when [token] is null/empty. Otherwise the request must present the
/// token as `Authorization: Bearer <token>` or `X-Admin-Token: <token>`.
/// Comparison is length-then-constant-time to avoid leaking the token via
/// response timing on the (already non-public) admin port.
bool _adminAuthorized(HttpRequest req, String? token) {
  if (token == null || token.isEmpty) return true;
  final bearer = req.headers.value('authorization');
  final presented = (bearer != null && bearer.startsWith('Bearer '))
      ? bearer.substring(7).trim()
      : req.headers.value('x-admin-token')?.trim();
  if (presented == null) return false;
  return _constantTimeEquals(presented, token);
}

/// Constant-time string compare (length-independent short-circuit only).
bool _constantTimeEquals(String a, String b) {
  if (a.length != b.length) return false;
  var diff = 0;
  for (var i = 0; i < a.length; i++) {
    diff |= a.codeUnitAt(i) ^ b.codeUnitAt(i);
  }
  return diff == 0;
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
