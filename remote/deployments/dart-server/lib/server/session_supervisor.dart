/// Host-pool session supervisor.
///
/// Lives on the main isolate. Maintains a small pool of "session-host"
/// isolates spawned lazily as load arrives. Each host runs up to
/// [SessionSupervisor.sessionsPerHost] sessions side-by-side as plain
/// Dart objects in one event loop (see `Session` in `isolate_session.dart`).
///
/// For each accepted WebSocket the supervisor:
///
///   1. Picks the least-loaded host that still has free capacity, or
///      spawns a fresh host if none exist / all are full.
///   2. Creates a per-session outbound `ReceivePort`, sends an
///      `AttachSession(boot)` to the host where `boot.outbound` points
///      at that ReceivePort's SendPort.
///   3. Pumps WS inbound frames into the host as `RouteToSession(...)`.
///   4. Pumps `OutboundFrame`s coming back on the per-session
///      ReceivePort into the WS / metrics aggregator / EventBus /
///      Presence / ConversationRegistry, exactly as before.
///   5. Cleans up on disconnect / host-error / host-exit.
///
/// The `adopt(...)` API and the OutboundFrame protocol are unchanged
/// from the original 1-isolate-per-session implementation. Only the
/// "where the session runtime actually executes" knob has changed.
///
/// In addition to per-session plumbing, the supervisor still owns the
/// integration glue that keeps the four main-isolate stores consistent:
///
///   * [EventBus]              — pubsub / fanout topology
///   * [Presence]              — userId ↔ sessionId index
///   * [ConversationRegistry]  — conversations / members / recent-msgs cache
library;

import 'dart:async';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:rxdart/rxdart.dart';

import '../shared/wire_messages.dart';
import 'conversation_registry.dart';
import 'event_bus.dart';
import 'isolate_session.dart';
import 'metrics.dart';
import 'presence.dart';

/// Topic every session auto-joins so it sees identity churn for any user.
const String presenceTopic = 'presence';

/// Topic every session auto-joins so it sees the global "conversation
/// directory" mutate (created/deleted, message counts).
const String conversationListTopic = 'conv-list';

/// Default capacity per host isolate. Override with `SESSIONS_PER_HOST`.
const int kDefaultSessionsPerHost = 100;

/// Hard floor / ceiling for the per-host capacity. Tuning under 10 makes
/// the host pool degenerate into "almost one isolate per session"; over
/// 2000 the per-host event loop saturation we just escaped from comes
/// back, since 2K RxDart graphs on one isolate do real work per tick.
const int kMinSessionsPerHost = 1;
const int kMaxSessionsPerHost = 2000;

/// Default idle timeout for WebSocket sessions (20 minutes). Override
/// with `WS_IDLE_TIMEOUT_SECONDS`. Set to 0 to disable.
const int kDefaultIdleTimeoutSeconds = 1200;

/// Default hard upper bound on session age (3 hours). Sessions older
/// than this get evicted as soon as they've been idle for at least
/// [kDefaultAgeBasedIdleSeconds]. Override with `WS_MAX_AGE_SECONDS`.
/// Set to 0 to disable age-based eviction.
const int kDefaultMaxAgeSeconds = 10800;

/// Idle threshold paired with [kDefaultMaxAgeSeconds]. Override with
/// `WS_AGE_BASED_IDLE_SECONDS`.
const int kDefaultAgeBasedIdleSeconds = 30;

/// Default interval between server-driven Clock fragments (1 Hz).
/// Override with `WS_CLOCK_INTERVAL_SECONDS`. At 20K connections a
/// 1 Hz jaspr render rate is ~20 cores; bump this to 5–15 s when
/// running the connection-count benchmark and let the loader's
/// `RECEIVE_TIMEOUT_SECONDS` rise above this value. Setting to 0
/// disables the clock entirely (idle/age timers still fire).
const int kDefaultClockIntervalSeconds = 1;

/// When `true`, supervisor.adopt skips the auto-bus-register +
/// presence.bind path AND suppresses the `presence.session_joined` /
/// `presence.session_left` broadcasts on connect/disconnect. Normal
/// app traffic stays functional (sessions can still publish bus
/// frames and identify) but connect/disconnect churn no longer scales
/// O(N) per event. Override with `WS_BENCHMARK_MODE=true` while
/// running pure-connection benchmarks to remove the O(N²) fanout
/// death spiral that bites every event-bus design under high churn.
const bool kDefaultBenchmarkMode = false;

/// Maximum size, in bytes, of an inbound WS text/binary frame the
/// supervisor will accept from a peer. Anything larger triggers a
/// 1009 (`message too big`) close. Override with `WS_MAX_INBOUND_BYTES`.
/// 64 KiB is plenty for our HTMX form payloads (which are tiny JSON
/// objects); raising it is cheap memory-wise but expands the DoS
/// surface for slow-byte / huge-frame attackers.
const int kDefaultMaxInboundBytes = 64 * 1024;

/// Outbound frames per second a session is allowed to emit before the
/// supervisor treats it as a slow / runaway client and force-closes.
/// The 1 Hz clock fragment is the only steady-state emitter, so even
/// a chatty bus subscriber typically stays under 50 fps. Override with
/// `WS_MAX_OUTBOUND_RATE_PER_SECOND`.
const int kDefaultMaxOutboundRatePerSecond = 200;

/// Number of consecutive 1-second windows the per-session outbound
/// counter must exceed [kDefaultMaxOutboundRatePerSecond] before the
/// session is killed. Lets a transient burst (e.g. a join flood)
/// settle before we punish it. Override with
/// `WS_SLOW_CLIENT_WINDOWS`.
const int kDefaultSlowClientWindows = 5;

/// Hard ceiling on distinct conversations one shard's [ConversationRegistry]
/// will create. The conversation metadata / member / reverse-index maps are
/// otherwise unbounded: a single unauthenticated connection can drive
/// `open-conv` / `join-conv` / `say-conv` with an endless stream of fresh
/// ids and grow them without limit (the recent-message *cache* is LRU-capped
/// at 1024, but the metadata maps are not). At the ceiling the supervisor
/// refuses to create *new* conversations (existing ones still work) and
/// counts `dart_conv_create_refused_total`. 100K rows is generous for the
/// demo workload while keeping the worst-case heap bounded.
const int kMaxConversationsPerShard = 100000;

class SessionSupervisor {
  SessionSupervisor({
    required this.metrics,
    required this.bus,
    required this.presence,
    required this.conversations,
    int sessionsPerHost = kDefaultSessionsPerHost,
    this.idleTimeoutSeconds = kDefaultIdleTimeoutSeconds,
    this.maxAgeSeconds = kDefaultMaxAgeSeconds,
    this.ageBasedIdleSeconds = kDefaultAgeBasedIdleSeconds,
    this.maxInboundBytes = kDefaultMaxInboundBytes,
    this.maxOutboundRatePerSecond = kDefaultMaxOutboundRatePerSecond,
    this.slowClientWindows = kDefaultSlowClientWindows,
    this.clockIntervalSeconds = kDefaultClockIntervalSeconds,
    this.benchmarkMode = kDefaultBenchmarkMode,
    this.poolControllerEnabled = false,
    this.poolMinWarmHosts = 0,
    this.poolMaxHosts = 0,
    this.poolReconcileMaxSpawnPerTick = 8,
    this.poolRetireCooldownMs = 15000,
  }) : _sessionsPerHost = sessionsPerHost
            .clamp(kMinSessionsPerHost, kMaxSessionsPerHost);

  final Metrics metrics;
  final EventBus bus;
  final Presence presence;
  final ConversationRegistry conversations;

  /// Maximum sessions one session-host isolate is allowed to own. The
  /// supervisor lazily spawns a new host when all existing hosts are at
  /// this cap. Mutable: the MDP autotuner's *density* action retunes it at
  /// runtime via [applyTargetHosts] (controller mode only). Always clamped
  /// to `[kMinSessionsPerHost, kMaxSessionsPerHost]`.
  int _sessionsPerHost;
  int get sessionsPerHost => _sessionsPerHost;

  /// Per-session idle timeout, in seconds, propagated into the
  /// `SessionBootMessage` so each session enforces it independently.
  /// Sessions silent for this long emit a 4001 `idle_timeout` close.
  /// 0 disables the check.
  final int idleTimeoutSeconds;

  /// Hard age limit on a session (seconds). Sessions older than this
  /// AND idle for [ageBasedIdleSeconds] get evicted with a 4003
  /// `session_aged` close. Lets old session-host isolates retire as
  /// their slot occupants naturally lapse, capping per-host RAM growth
  /// and giving the host pool a steady churn rate. 0 disables.
  final int maxAgeSeconds;

  /// Idle threshold that pairs with [maxAgeSeconds].
  final int ageBasedIdleSeconds;

  /// Inbound text/binary frames larger than this trigger a 1009
  /// `message too big` close. Set to 0 to disable.
  final int maxInboundBytes;

  /// Outbound rate (frames/second) above which a session is treated as
  /// a slow/runaway client. Set to 0 to disable.
  final int maxOutboundRatePerSecond;

  /// Consecutive over-limit windows before we actually kill the slow
  /// session. Smooths over transient bursts.
  final int slowClientWindows;

  /// Per-session interval between Clock OOB swap fragments. Threaded
  /// into each [SessionBootMessage] so individual sessions enforce it.
  final int clockIntervalSeconds;

  /// When true, the supervisor skips bus.register / presence.bind on
  /// adopt and suppresses presence.session_joined / session_left
  /// broadcasts. Removes the O(N²) fanout that otherwise dominates
  /// CPU in pure-connection benchmarks at >10K live sessions.
  final bool benchmarkMode;

  /// When true, the host pool is driven by [applyTargetHosts] (the
  /// coordinator's MDP autotuner): hosts are pre-spawned up to the target
  /// off the adopt hot path and idle hosts are gracefully retired back
  /// down. When false, the legacy lazy-spawn-only behaviour is preserved
  /// exactly (no pre-spawn, no retire, no cap, no refusals).
  final bool poolControllerEnabled;

  /// Floor on warm host isolates (only meaningful when
  /// [poolControllerEnabled]).
  final int poolMinWarmHosts;

  /// Hard ceiling on host isolates. 0 = unbounded. At the ceiling a full
  /// pool refuses new sessions with a 1013 close (load shedding).
  final int poolMaxHosts;

  /// Max hosts pre-spawned per reconcile pass.
  final int poolReconcileMaxSpawnPerTick;

  /// Idle dwell time before an empty host may be retired.
  final int poolRetireCooldownMs;

  /// Coordinator-set warm-pool target (host isolates for this shard). 0
  /// until the first [ShardPoolDirective] arrives.
  int _targetHosts = 0;
  int get targetHosts => _targetHosts;

  // Reconciler / autotuner telemetry, surfaced as gauges + counters.
  int _coldStartSpawns = 0;
  int _refusals = 0;
  int _prewarmInFlight = 0;
  int _retiredHosts = 0;
  int _prewarmedHosts = 0;

  int get coldStartSpawnsTotal => _coldStartSpawns;
  int get refusalsTotal => _refusals;
  int get retiredHostsTotal => _retiredHosts;
  int get prewarmedHostsTotal => _prewarmedHosts;

  /// `true` once SIGTERM (or supervisor.requestDrain) has run. The
  /// supervisor refuses new attaches and forwards the drain sentinel
  /// to all hosts. Idempotent.
  bool _draining = false;
  bool get isDraining => _draining;

  final _hosts = <_HostState>[];

  final _liveCount = BehaviorSubject<int>.seeded(0);
  int _spawnedTotal = 0;
  int _hostsSpawnedTotal = 0;
  int _hostsTerminatedTotal = 0;

  Stream<int> get liveCountStream => _liveCount.stream;
  int get liveCount => _liveCount.value;
  int get spawnedTotal => _spawnedTotal;

  /// Number of session-host isolates currently alive (includes hosts that
  /// are draining toward retirement until their isolate actually exits).
  int get hostCount => _hosts.where((h) => !h.dead).length;

  /// Hosts available to accept new sessions (alive AND not retiring). Used
  /// by the reconciler's target math and the autotuner's utilisation
  /// signal so a draining host isn't counted as live capacity.
  int get liveHostCount => _hosts.where((h) => !h.dead && !h.retiring).length;

  /// Hosts that currently own zero sessions (and have none pending). These
  /// are the over-provisioning cost the autotuner trades against
  /// cold-start latency.
  int get idleHostCount => _hosts
      .where((h) =>
          !h.dead && !h.retiring && h.sessionCount == 0 && h.pendingAttaches == 0)
      .length;

  /// Total free session slots across non-retiring live hosts. Clamped
  /// per-host so a host that is over a freshly-lowered density cap (the
  /// autotuner can shrink `sessionsPerHost` mid-flight) contributes zero
  /// rather than a negative that would mask free slots on sibling hosts.
  int get freeSlots {
    var free = 0;
    for (final h in _hosts) {
      if (h.dead || h.retiring) continue;
      final slot = _sessionsPerHost - (h.sessionCount + h.pendingAttaches);
      if (slot > 0) free += slot;
    }
    return free;
  }

  /// Total session-host isolates ever spawned in this process.
  int get hostsSpawnedTotal => _hostsSpawnedTotal;

  /// Total session-host isolates that have exited (clean or otherwise).
  int get hostsTerminatedTotal => _hostsTerminatedTotal;

  Future<void> adopt(
    WebSocket socket, {
    required String sessionId,
    required String remoteAddr,
    required String requestPath,
    required Map<String, String> headers,
  }) async {
    // Refuse new sessions during drain. Caller is expected to have
    // already accepted the upgrade; close cleanly instead of attaching.
    if (_draining) {
      metrics.inc('dart_sessions_refused_draining_total');
      try {
        await socket.close(1012, 'server_draining');
      } catch (_) {/* swallow */}
      return;
    }

    // Pick or spawn a host BEFORE we start mutating session-scoped state
    // — if Isolate.spawn fails we want a clean error. `_acquireHost`
    // returns null only when the pool is capped and full (controller
    // mode): shed the connection instead of forking past the ceiling.
    final adoptStartUs = DateTime.now().microsecondsSinceEpoch;
    final host = await _acquireHost();
    if (host == null) {
      _refusals++;
      metrics.inc('dart_sessions_refused_capacity_total');
      try {
        await socket.close(1013, 'try_again_later');
      } catch (_) {/* swallow */}
      return;
    }

    final outbound = ReceivePort('dd-dart-outbound-$sessionId');
    // First-frame latency: measured from host-attach to the first outbound
    // text/binary frame actually written to this socket.
    var firstFrameSent = false;

    StreamSubscription<dynamic>? inboundSub;
    StreamSubscription<dynamic>? outboundSub;
    var teardownStarted = false;
    final done = Completer<void>();

    Future<void> teardown(String why) async {
      if (teardownStarted) return;
      teardownStarted = true;
      metrics.inc('dart_sessions_teardown_total');

      if (!benchmarkMode) {
        // Order matters: unbind presence + announce departure BEFORE we
        // unregister from the bus, so the announcement actually fans out.
        final userId = presence.userIdFor(sessionId);
        if (userId != null) {
          // Did this disconnect take the user offline (no remaining
          // sessions)? Capture before we unbind.
          final wasLastSession = presence.sessionsFor(userId).length <= 1;
          presence.unbind(sessionId);
          bus.publish(
            topic: presenceTopic,
            kind: 'presence.session_left',
            data: <String, Object?>{
              'sessionId': sessionId,
              'userId': userId,
              'displayName': presence.displayNameFor(userId),
              'userOffline': wasLastSession,
            },
            fromSessionId: _systemSessionId,
          );
        }
        bus.unregister(sessionId);
      }
      _liveCount.add((_liveCount.value - 1).clamp(0, 1 << 30));

      // Detach from host BEFORE we close ports so any in-flight bus
      // delivery has somewhere to land. The host is allowed to silently
      // drop after the session is removed from its routing table.
      host.detach(sessionId);

      try {
        await inboundSub?.cancel();
      } catch (_) {/* swallow */}
      try {
        await outboundSub?.cancel();
      } catch (_) {/* swallow */}
      outbound.close();
      try {
        if (socket.readyState != WebSocket.closed) {
          await socket.close(1000, why);
        }
      } catch (_) {/* swallow */}
      if (!done.isCompleted) done.complete();
    }

    final boot = SessionBootMessage(
      sessionId: sessionId,
      remoteAddr: remoteAddr,
      requestPath: requestPath,
      headers: headers,
      outbound: outbound.sendPort,
      spawnedAtUs: DateTime.now().microsecondsSinceEpoch,
      idleTimeoutSeconds: idleTimeoutSeconds,
      maxAgeSeconds: maxAgeSeconds,
      ageBasedIdleSeconds: ageBasedIdleSeconds,
      clockIntervalSeconds: clockIntervalSeconds,
    );

    if (!benchmarkMode) {
      // Pre-register with the bus + presence index BEFORE handing the
      // attach message to the host, so the session can issue BusJoin
      // / ConversationJoin synchronously during its bootstrap and have
      // those land on the supervisor in order.
      bus.register(sessionId, host.mailbox);
      presence.bind(
        sessionId,
        _anonymousUserIdFor(sessionId),
        displayName: _anonymousDisplayNameFor(sessionId),
      );
    }

    // Route this session's lifetime to the host so the supervisor can
    // tear down the WS if the host isolate ever dies.
    host.attach(sessionId, () => unawaited(teardown('host_failed')));
    _spawnedTotal++;
    _liveCount.add(_liveCount.value + 1);
    metrics.inc('dart_sessions_spawned_total');

    host.mailbox.send(AttachSession(boot));

    // Adopt latency: acquire-or-spawn-a-host + attach, the part of the
    // connection setup the isolate-pool sizing actually controls. Forwarded
    // to the coordinator's canonical histogram via `ObserveEvent`.
    final attachedAtUs = DateTime.now().microsecondsSinceEpoch;
    metrics.observe(
      'dart_ws_adopt_latency_seconds',
      (attachedAtUs - adoptStartUs) / 1000000.0,
    );

    // Slow-client tracker. We bucket outbound frames into 1-second
    // windows and compare against [maxOutboundRatePerSecond]. The
    // supervisor force-closes a session that exceeds the limit for
    // [slowClientWindows] consecutive seconds.
    var outboundWindowStartMs =
        DateTime.now().millisecondsSinceEpoch ~/ 1000;
    var outboundFramesThisWindow = 0;
    var consecutiveOverLimit = 0;

    inboundSub = socket.listen(
      (data) {
        if (data is String) {
          if (maxInboundBytes > 0 && data.length > maxInboundBytes) {
            metrics.inc('dart_sessions_oversized_inbound_total');
            unawaited(socket.close(1009, 'frame_too_large'));
            return;
          }
          host.mailbox.send(RouteToSession(
            sessionId: sessionId,
            event: InboundText(data),
          ));
        } else if (data is List<int>) {
          if (maxInboundBytes > 0 && data.length > maxInboundBytes) {
            metrics.inc('dart_sessions_oversized_inbound_total');
            unawaited(socket.close(1009, 'frame_too_large'));
            return;
          }
          host.mailbox.send(RouteToSession(
            sessionId: sessionId,
            event: InboundBinary(_asUint8List(data)),
          ));
        }
      },
      onError: (Object err, StackTrace st) {
        metrics.inc('dart_sessions_ws_error_total');
        unawaited(teardown('ws_error:$err'));
      },
      onDone: () {
        host.mailbox.send(RouteToSession(
          sessionId: sessionId,
          event: InboundClosed(socket.closeCode, socket.closeReason),
        ));
        unawaited(teardown('ws_done'));
      },
      cancelOnError: true,
    );

    /// Returns true if the current frame should be dropped because the
    /// peer is too slow to drain. Updates the per-second sliding window
    /// each call and force-closes the WS once the over-limit counter
    /// breaches [slowClientWindows].
    bool tickOutbound() {
      if (maxOutboundRatePerSecond <= 0) return false;
      final nowSec = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      if (nowSec != outboundWindowStartMs) {
        if (outboundFramesThisWindow > maxOutboundRatePerSecond) {
          consecutiveOverLimit++;
          if (consecutiveOverLimit >= slowClientWindows) {
            metrics.inc('dart_sessions_slow_client_killed_total');
            unawaited(socket.close(1011, 'slow_client'));
            return true;
          }
        } else {
          consecutiveOverLimit = 0;
        }
        outboundWindowStartMs = nowSec;
        outboundFramesThisWindow = 0;
      }
      outboundFramesThisWindow++;
      return false;
    }

    void recordFirstFrame() {
      if (firstFrameSent) return;
      firstFrameSent = true;
      metrics.observe(
        'dart_ws_first_frame_latency_seconds',
        (DateTime.now().microsecondsSinceEpoch - attachedAtUs) / 1000000.0,
      );
    }

    outboundSub = outbound.listen((msg) {
      switch (msg) {
        case OutboundText(:final text):
          if (tickOutbound()) return;
          if (socket.readyState == WebSocket.open) {
            recordFirstFrame();
            socket.add(text);
          }
        case OutboundBinary(:final bytes):
          if (tickOutbound()) return;
          if (socket.readyState == WebSocket.open) {
            recordFirstFrame();
            socket.add(bytes);
          }
        case OutboundClose(:final code, :final reason):
          unawaited(socket.close(code, reason));
        case MetricEvent(:final name, :final delta):
          metrics.inc(name, delta);
        // The bus / presence / conversation handlers below are
        // skipped wholesale in benchmark mode so a Session that
        // optimistically emits BusJoin during bootstrap doesn't end
        // up registered against a topic it can't be delivered to.
        case BusJoin(:final topic):
          if (benchmarkMode) break;
          bus.join(sessionId, topic);
        case BusLeave(:final topic):
          if (benchmarkMode) break;
          bus.leave(sessionId, topic);
        case BusPublish(:final topic, :final kind, :final data, :final includeSelf):
          if (benchmarkMode) break;
          bus.publish(
            topic: topic,
            kind: kind,
            data: data,
            fromSessionId: sessionId,
            includeSelf: includeSelf,
          );
        case Identify(:final userId, :final displayName):
          if (benchmarkMode) break;
          _handleIdentify(sessionId, userId, displayName);
        case ConversationOpen(
              :final conversationId,
              :final title,
              :final kind,
            ):
          if (benchmarkMode) break;
          _handleConversationOpen(sessionId, conversationId, title, kind);
        case ConversationJoin(:final conversationId):
          if (benchmarkMode) break;
          _handleConversationJoin(sessionId, conversationId);
        case ConversationLeave(:final conversationId, :final dropMembership):
          if (benchmarkMode) break;
          _handleConversationLeave(sessionId, conversationId, dropMembership);
        case ConversationSay(:final conversationId, :final text):
          if (benchmarkMode) break;
          _handleConversationSay(sessionId, conversationId, text);
        case ConversationDelete(:final conversationId):
          if (benchmarkMode) break;
          _handleConversationDelete(sessionId, conversationId);
        case _:
          // Forward-compat: ignore unrecognised frames so a newer worker
          // doesn't kill an older bridge.
          break;
      }
    });

    socket.done.whenComplete(() {
      if (!done.isCompleted) {
        unawaited(teardown('socket_done'));
      }
    });
    return done.future;
  }

  // ---- Host-pool management ---------------------------------------------

  /// Returns a host with at least one free slot, spawning a new one if
  /// every existing host is dead/retiring/full. Increments the host's
  /// reserved count synchronously so concurrent adopt() calls don't
  /// oversubscribe.
  ///
  /// Returns null only in controller mode when the pool is at its hard
  /// [poolMaxHosts] ceiling and full — the caller sheds the connection.
  /// In legacy mode (controller disabled, `poolMaxHosts == 0`) it never
  /// returns null: behaviour is identical to the original lazy spawn.
  Future<_HostState?> _acquireHost() async {
    _HostState? best;
    for (final h in _hosts) {
      if (h.dead || h.retiring) continue;
      if (h.sessionCount + h.pendingAttaches >= sessionsPerHost) continue;
      if (best == null ||
          h.sessionCount + h.pendingAttaches <
              best.sessionCount + best.pendingAttaches) {
        best = h;
      }
    }
    if (best != null) {
      best.pendingAttaches++;
      return best;
    }
    // No warm host with a free slot. Either spawn one on this connection's
    // hot path (a "cold start" — the latency the autotuner learns to hide
    // by pre-warming) or, if capped and full, shed the connection.
    if (poolMaxHosts > 0 && liveHostCount >= poolMaxHosts) {
      return null;
    }
    _coldStartSpawns++;
    metrics.inc('dart_session_cold_start_spawns_total');
    final fresh = await _spawnHost();
    fresh.pendingAttaches++;
    return fresh;
  }

  // ---- Warm-pool reconciler (controller mode) ---------------------------

  /// Apply a coordinator MDP-autotuner directive: set this shard's warm
  /// host-isolate target (and, when [sessionsPerHost] > 0, its per-host
  /// density) then reconcile the pool toward the target. No-op when the
  /// controller is disabled.
  ///
  /// A density change only affects *future* placements: existing hosts keep
  /// the sessions they already own. Lowering the cap below a host's current
  /// occupancy simply stops that host accepting more until it drains;
  /// raising it lets warm hosts absorb additional sessions before the pool
  /// needs to grow. The new cap is clamped to
  /// `[kMinSessionsPerHost, kMaxSessionsPerHost]`.
  void applyTargetHosts(int targetHosts, {int sessionsPerHost = 0}) {
    if (!poolControllerEnabled) return;
    if (sessionsPerHost > 0) {
      _sessionsPerHost =
          sessionsPerHost.clamp(kMinSessionsPerHost, kMaxSessionsPerHost);
    }
    _targetHosts = targetHosts < 0 ? 0 : targetHosts;
    _reconcile();
  }

  /// Clamp the raw target into `[minWarm, maxHosts]`.
  int _desiredHosts() {
    var d = _targetHosts;
    if (poolMinWarmHosts > 0 && d < poolMinWarmHosts) d = poolMinWarmHosts;
    if (poolMaxHosts > 0 && d > poolMaxHosts) d = poolMaxHosts;
    return d;
  }

  /// Pre-spawn hosts toward the desired count (bounded per pass) and retire
  /// hosts that have been idle past the cooldown. Driven by the periodic
  /// [ShardPoolDirective] cadence; the hot path never blocks on this.
  void _reconcile() {
    if (!poolControllerEnabled || _draining) return;
    _hosts.removeWhere((h) => h.dead);
    final desired = _desiredHosts();

    final deficit = desired - liveHostCount - _prewarmInFlight;
    if (deficit > 0) {
      final toSpawn = deficit.clamp(0, poolReconcileMaxSpawnPerTick);
      for (var i = 0; i < toSpawn; i++) {
        _prewarmInFlight++;
        unawaited(_spawnHostPrewarm());
      }
    }

    _retireIdleHosts(desired);
  }

  Future<void> _spawnHostPrewarm() async {
    try {
      await _spawnHost();
      _prewarmedHosts++;
      metrics.inc('dart_session_hosts_prewarmed_total');
    } catch (_) {
      // _spawnHost already emitted dart_session_hosts_spawn_failed_total.
    } finally {
      if (_prewarmInFlight > 0) _prewarmInFlight--;
    }
  }

  void _retireIdleHosts(int desired) {
    final now = DateTime.now().millisecondsSinceEpoch;
    for (final h in _hosts) {
      if (h.dead || h.retiring) continue;
      if (h.sessionCount != 0 || h.pendingAttaches != 0) {
        h.emptySinceMs = 0;
        continue;
      }
      // Empty host: start (or honour) its cooldown before retiring.
      if (h.emptySinceMs == 0) {
        h.emptySinceMs = now;
        continue;
      }
      if (now - h.emptySinceMs < poolRetireCooldownMs) continue;
      // Stop once we're at/under target or the warm floor. liveHostCount
      // excludes hosts already marked retiring this pass, so the counts
      // stay correct as we retire several in one sweep.
      if (liveHostCount <= desired) break;
      if (poolMinWarmHosts > 0 && liveHostCount <= poolMinWarmHosts) break;
      h.retiring = true;
      _retiredHosts++;
      metrics.inc('dart_session_hosts_retired_total');
      try {
        // Empty host drains instantly; its exit port fires _markHostDead.
        requestHostShutdown(h.mailbox);
      } catch (_) {/* swallow */}
    }
  }

  Future<_HostState> _spawnHost() async {
    final hostId = _hostsSpawnedTotal;
    final handshake = ReceivePort('dd-dart-host-handshake-$hostId');
    final exit = ReceivePort('dd-dart-host-exit-$hostId');
    final error = ReceivePort('dd-dart-host-error-$hostId');

    Isolate isolate;
    try {
      isolate = await Isolate.spawn<SendPort>(
        hostIsolateEntry,
        handshake.sendPort,
        debugName: 'dd-dart-session-host-$hostId',
        // Non-fatal: the host wraps its event loop in `runZonedGuarded`
        // and every session guards its own pipelines, so an app-level
        // error is caught and logged rather than killing the isolate and
        // dropping all ~sessionsPerHost sessions on it. We still watch the
        // `error` / `exit` ports: a genuine isolate termination (OOM,
        // explicit kill, VM-fatal) fires `exit`, and the supervisor then
        // tears the attached sessions down cleanly (each WS closes 1000).
        errorsAreFatal: false,
        onExit: exit.sendPort,
        onError: error.sendPort,
      );
    } catch (e) {
      handshake.close();
      exit.close();
      error.close();
      metrics.inc('dart_session_hosts_spawn_failed_total');
      rethrow;
    }

    final mailbox = (await handshake.first) as SendPort;
    handshake.close();

    final state = _HostState(
      hostId: hostId,
      isolate: isolate,
      mailbox: mailbox,
    );
    _hosts.add(state);
    _hostsSpawnedTotal++;
    metrics.inc('dart_session_hosts_spawned_total');

    state.exitSub = exit.listen((_) {
      _markHostDead(state, 'exit');
      exit.close();
    });
    state.errorSub = error.listen((err) {
      metrics.inc('dart_session_hosts_error_total');
      _markHostDead(state, 'error:$err');
      error.close();
    });

    return state;
  }

  void _markHostDead(_HostState host, String reason) {
    if (host.dead) return;
    host.dead = true;
    _hostsTerminatedTotal++;
    metrics.inc('dart_session_hosts_terminated_total');
    // Snapshot then iterate; teardown callbacks will mutate the map.
    final attached = host.attachments.entries.toList(growable: false);
    host.attachments.clear();
    for (final entry in attached) {
      try {
        entry.value();
      } catch (_) {/* swallow */}
    }
    try {
      host.exitSub?.cancel();
    } catch (_) {/* swallow */}
    try {
      host.errorSub?.cancel();
    } catch (_) {/* swallow */}
  }

  /// Chaos hook (only reachable when the coordinator runs with
  /// `WS_DEBUG_CRASH=1`): hard-kill the most-loaded live host isolate to
  /// simulate a host crash. The kill fires the host's exit port, which
  /// drives [_markHostDead] → every attached session's WebSocket is closed
  /// cleanly (1000) while this shard and its sibling hosts keep serving.
  /// Returns the number of sessions that were on the killed host (0 if no
  /// killable host exists). Never invoked in normal operation.
  int debugKillOneHost() {
    _HostState? victim;
    for (final h in _hosts) {
      if (h.dead || h.retiring) continue;
      if (victim == null || h.sessionCount > victim.sessionCount) {
        victim = h;
      }
    }
    if (victim == null) return 0;
    final lost = victim.sessionCount;
    metrics.inc('dart_session_hosts_debug_crashed_total');
    try {
      victim.isolate.kill(priority: Isolate.immediate);
    } catch (_) {/* swallow */}
    return lost;
  }

  // ---- Identity / conversation handlers ---------------------------------

  void _handleIdentify(String sessionId, String userId, String displayName) {
    final prevUser = presence.userIdFor(sessionId);
    final wentOffline = prevUser != null &&
        prevUser != userId &&
        presence.sessionsFor(prevUser).length <= 1;

    presence.bind(sessionId, userId, displayName: displayName);
    metrics.inc('dart_presence_identify_total');

    bus.publish(
      topic: presenceTopic,
      kind: 'presence.identified',
      data: <String, Object?>{
        'sessionId': sessionId,
        'userId': userId,
        'displayName': presence.displayNameFor(userId),
        'previousUserId': prevUser,
        'previousUserOffline': wentOffline,
      },
      fromSessionId: _systemSessionId,
    );
  }

  void _handleConversationOpen(
    String sessionId,
    String conversationId,
    String title,
    String kind,
  ) {
    final userId = presence.userIdFor(sessionId) ?? sessionId;
    final created = conversations.get(conversationId) == null;
    if (created &&
        conversations.conversationCount >= kMaxConversationsPerShard) {
      metrics.inc('dart_conv_create_refused_total');
      return;
    }
    final meta = conversations.upsert(
      conversationId: conversationId,
      title: title,
      kind: kind,
      createdByUserId: userId,
    );
    if (created) metrics.inc('dart_conv_created_total');

    bus.publish(
      topic: conversationListTopic,
      kind: created ? 'conv.created' : 'conv.updated',
      data: <String, Object?>{
        ...meta.toJson(),
        'memberCount': conversations.members(conversationId).length,
      },
      fromSessionId: _systemSessionId,
    );
  }

  void _handleConversationJoin(String sessionId, String conversationId) {
    final userId = presence.userIdFor(sessionId) ?? sessionId;
    // Auto-create on first join so the typical "join this room" UX
    // doesn't require a separate Open call.
    if (conversations.get(conversationId) == null) {
      if (conversations.conversationCount >= kMaxConversationsPerShard) {
        metrics.inc('dart_conv_create_refused_total');
        return;
      }
      conversations.upsert(
        conversationId: conversationId,
        title: conversationId,
        kind: 'chat',
        createdByUserId: userId,
      );
      metrics.inc('dart_conv_created_total');
    }
    final added = conversations.addMember(conversationId, userId);
    bus.join(sessionId, ConversationRegistry.topicFor(conversationId));
    metrics.inc('dart_conv_join_total');

    if (added) {
      bus.publish(
        topic: conversationListTopic,
        kind: 'conv.user_joined',
        data: <String, Object?>{
          'conversationId': conversationId,
          'userId': userId,
          'displayName': presence.displayNameFor(userId),
          'memberCount': conversations.members(conversationId).length,
        },
        fromSessionId: _systemSessionId,
      );
      bus.publish(
        topic: ConversationRegistry.topicFor(conversationId),
        kind: 'conv.user_joined',
        data: <String, Object?>{
          'conversationId': conversationId,
          'userId': userId,
          'displayName': presence.displayNameFor(userId),
        },
        fromSessionId: _systemSessionId,
      );
    }
  }

  void _handleConversationLeave(
    String sessionId,
    String conversationId,
    bool dropMembership,
  ) {
    final userId = presence.userIdFor(sessionId) ?? sessionId;
    bus.leave(sessionId, ConversationRegistry.topicFor(conversationId));
    metrics.inc('dart_conv_leave_total');

    if (dropMembership) {
      // Only fully drop user-level membership when ALL of this user's
      // sessions are no longer subscribed to the topic. Otherwise other
      // tabs/connections that didn't ask to leave keep the user a member.
      final stillJoined = presence
          .sessionsFor(userId)
          .where((sid) => bus
              .members(ConversationRegistry.topicFor(conversationId))
              .contains(sid))
          .isNotEmpty;
      if (!stillJoined) {
        if (conversations.removeMember(conversationId, userId)) {
          bus.publish(
            topic: conversationListTopic,
            kind: 'conv.user_left',
            data: <String, Object?>{
              'conversationId': conversationId,
              'userId': userId,
              'displayName': presence.displayNameFor(userId),
              'memberCount': conversations.members(conversationId).length,
            },
            fromSessionId: _systemSessionId,
          );
        }
      }
    }
  }

  void _handleConversationSay(
    String sessionId,
    String conversationId,
    String text,
  ) {
    final clean = text.trim();
    if (clean.isEmpty) return;
    final userId = presence.userIdFor(sessionId) ?? sessionId;
    final displayName = presence.displayNameFor(userId);

    // Auto-join + auto-create so a session can post even before joining.
    // Keeps the demo bulletproof; remove for stricter prod semantics.
    if (conversations.get(conversationId) == null) {
      _handleConversationOpen(sessionId, conversationId, conversationId, 'chat');
    }
    if (!conversations.members(conversationId).contains(userId)) {
      _handleConversationJoin(sessionId, conversationId);
    }

    final recent = conversations.appendMessage(
      conversationId: conversationId,
      userId: userId,
      text: clean,
    );
    metrics.inc('dart_conv_message_total');

    bus.publish(
      topic: ConversationRegistry.topicFor(conversationId),
      kind: 'conv.message',
      data: <String, Object?>{
        'conversationId': conversationId,
        'userId': userId,
        'displayName': displayName,
        'text': clean,
        'atUs': recent.last.atUs,
        'recentCount': recent.length,
      },
      fromSessionId: _systemSessionId,
    );
    // Also push a message-count update to the global directory so the
    // conversation list re-renders (last-activity reordering).
    final meta = conversations.get(conversationId);
    if (meta != null) {
      bus.publish(
        topic: conversationListTopic,
        kind: 'conv.bumped',
        data: <String, Object?>{
          ...meta.toJson(),
          'memberCount': conversations.members(conversationId).length,
        },
        fromSessionId: _systemSessionId,
      );
    }
  }

  void _handleConversationDelete(String sessionId, String conversationId) {
    final meta = conversations.get(conversationId);
    if (meta == null) return;
    conversations.delete(conversationId);
    metrics.inc('dart_conv_deleted_total');
    bus.publish(
      topic: conversationListTopic,
      kind: 'conv.deleted',
      data: <String, Object?>{'conversationId': conversationId},
      fromSessionId: _systemSessionId,
    );
  }

  // ---- Lifecycle --------------------------------------------------------

  /// Switch the supervisor into drain mode: refuse new attaches, ask
  /// every live host to detach all its sessions (which emits a clean
  /// close to each peer). Idempotent.
  ///
  /// The caller is expected to wait for `liveCount` to hit 0 (with a
  /// timeout) before calling [close].
  void requestDrain() {
    if (_draining) return;
    _draining = true;
    metrics.inc('dart_sessions_drain_requested_total');
    for (final host in _hosts) {
      if (host.dead) continue;
      // Tell the host to gracefully drain its sessions. The host's
      // mailbox loop will dispose each session, which emits the
      // OutboundClose → supervisor → socket.close path naturally.
      try {
        requestHostShutdown(host.mailbox);
      } catch (_) {/* swallow */}
    }
  }

  Future<void> close() async {
    for (final host in _hosts) {
      if (host.dead) continue;
      try {
        requestHostShutdown(host.mailbox);
      } catch (_) {/* swallow */}
      host.dead = true;
      try {
        host.exitSub?.cancel();
      } catch (_) {/* swallow */}
      try {
        host.errorSub?.cancel();
      } catch (_) {/* swallow */}
      try {
        host.isolate.kill(priority: Isolate.beforeNextEvent);
      } catch (_) {/* swallow */}
    }
    _hosts.clear();
    await _liveCount.close();
  }
}

class _HostState {
  _HostState({
    required this.hostId,
    required this.isolate,
    required this.mailbox,
  });

  final int hostId;
  final Isolate isolate;
  final SendPort mailbox;

  /// `sessionId → teardown callback`. The supervisor invokes the
  /// callback when the host dies so each session's WebSocket is closed
  /// cleanly even though the runtime that was driving it is gone.
  final attachments = <String, void Function()>{};

  /// Counts adopt() calls that reserved a slot but haven't yet fully
  /// attached. Prevents oversubscription when many adopts race in.
  int pendingAttaches = 0;

  bool dead = false;

  /// Set by the reconciler when this (empty) host has been asked to drain
  /// + exit. Retiring hosts are excluded from placement and from the live
  /// capacity counts, but stay in `_hosts` until their exit port fires.
  bool retiring = false;

  /// Wall-clock ms at which this host first became empty (0 = not empty).
  /// The reconciler retires it once it has been empty for at least
  /// `poolRetireCooldownMs`.
  int emptySinceMs = 0;

  StreamSubscription<dynamic>? exitSub;
  StreamSubscription<dynamic>? errorSub;

  int get sessionCount => attachments.length;

  void attach(String sessionId, void Function() onHostFailure) {
    attachments[sessionId] = onHostFailure;
    if (pendingAttaches > 0) pendingAttaches--;
  }

  void detach(String sessionId) {
    if (attachments.remove(sessionId) == null) return;
    if (dead) return;
    try {
      mailbox.send(DetachSession(sessionId));
    } catch (_) {/* swallow */}
  }
}

/// Sentinel session-id used as the publisher of system-emitted bus
/// events (presence churn, conversation churn). Sessions can filter
/// `delivery.fromSessionId == _systemSessionId` to distinguish "the
/// supervisor said so" from a peer broadcast.
const String _systemSessionId = '__system__';

/// Anonymous user-id assigned to a session before it calls Identify.
String _anonymousUserIdFor(String sessionId) => 'anon-$sessionId';

/// Friendly display name to use for the anonymous identity.
String _anonymousDisplayNameFor(String sessionId) =>
    'anon-${sessionId.substring(0, sessionId.length.clamp(0, 4))}';

/// Coerce a `List<int>` from a WebSocket frame into a `Uint8List` view
/// without copying when possible. Dart's `dart:io` already produces
/// `Uint8List`, but we accept the broader `List<int>` for safety.
Uint8List _asUint8List(List<int> data) =>
    data is Uint8List ? data : Uint8List.fromList(data);
