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

class SessionSupervisor {
  SessionSupervisor({
    required this.metrics,
    required this.bus,
    required this.presence,
    required this.conversations,
    int sessionsPerHost = kDefaultSessionsPerHost,
  }) : sessionsPerHost = sessionsPerHost
            .clamp(kMinSessionsPerHost, kMaxSessionsPerHost);

  final Metrics metrics;
  final EventBus bus;
  final Presence presence;
  final ConversationRegistry conversations;

  /// Maximum sessions one session-host isolate is allowed to own. The
  /// supervisor lazily spawns a new host when all existing hosts are
  /// at this cap.
  final int sessionsPerHost;

  final _hosts = <_HostState>[];

  final _liveCount = BehaviorSubject<int>.seeded(0);
  int _spawnedTotal = 0;
  int _hostsSpawnedTotal = 0;
  int _hostsTerminatedTotal = 0;

  Stream<int> get liveCountStream => _liveCount.stream;
  int get liveCount => _liveCount.value;
  int get spawnedTotal => _spawnedTotal;

  /// Number of session-host isolates currently alive.
  int get hostCount => _hosts.where((h) => !h.dead).length;

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
    // Pick or spawn a host BEFORE we start mutating session-scoped state
    // — if Isolate.spawn fails we want a clean error.
    final host = await _acquireHost();

    final outbound = ReceivePort('dd-dart-outbound-$sessionId');

    StreamSubscription<dynamic>? inboundSub;
    StreamSubscription<dynamic>? outboundSub;
    var teardownStarted = false;
    final done = Completer<void>();

    Future<void> teardown(String why) async {
      if (teardownStarted) return;
      teardownStarted = true;
      metrics.inc('dart_sessions_teardown_total');

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
    );

    // Pre-register with the bus + presence index BEFORE handing the
    // attach message to the host, so the session can issue BusJoin /
    // ConversationJoin synchronously during its bootstrap and have
    // those land on the supervisor in order.
    bus.register(sessionId, host.mailbox);
    presence.bind(
      sessionId,
      _anonymousUserIdFor(sessionId),
      displayName: _anonymousDisplayNameFor(sessionId),
    );

    // Route this session's lifetime to the host so the supervisor can
    // tear down the WS if the host isolate ever dies.
    host.attach(sessionId, () => unawaited(teardown('host_failed')));
    _spawnedTotal++;
    _liveCount.add(_liveCount.value + 1);
    metrics.inc('dart_sessions_spawned_total');

    host.mailbox.send(AttachSession(boot));

    inboundSub = socket.listen(
      (data) {
        if (data is String) {
          host.mailbox.send(RouteToSession(
            sessionId: sessionId,
            event: InboundText(data),
          ));
        } else if (data is List<int>) {
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

    outboundSub = outbound.listen((msg) {
      switch (msg) {
        case OutboundText(:final text):
          if (socket.readyState == WebSocket.open) socket.add(text);
        case OutboundBinary(:final bytes):
          if (socket.readyState == WebSocket.open) socket.add(bytes);
        case OutboundClose(:final code, :final reason):
          unawaited(socket.close(code, reason));
        case MetricEvent(:final name, :final delta):
          metrics.inc(name, delta);
        case BusJoin(:final topic):
          bus.join(sessionId, topic);
        case BusLeave(:final topic):
          bus.leave(sessionId, topic);
        case BusPublish(:final topic, :final kind, :final data, :final includeSelf):
          bus.publish(
            topic: topic,
            kind: kind,
            data: data,
            fromSessionId: sessionId,
            includeSelf: includeSelf,
          );
        case Identify(:final userId, :final displayName):
          _handleIdentify(sessionId, userId, displayName);
        case ConversationOpen(
              :final conversationId,
              :final title,
              :final kind,
            ):
          _handleConversationOpen(sessionId, conversationId, title, kind);
        case ConversationJoin(:final conversationId):
          _handleConversationJoin(sessionId, conversationId);
        case ConversationLeave(:final conversationId, :final dropMembership):
          _handleConversationLeave(sessionId, conversationId, dropMembership);
        case ConversationSay(:final conversationId, :final text):
          _handleConversationSay(sessionId, conversationId, text);
        case ConversationDelete(:final conversationId):
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
  /// every existing host is dead or full. Increments the host's reserved
  /// count synchronously so concurrent adopt() calls don't oversubscribe.
  Future<_HostState> _acquireHost() async {
    _HostState? best;
    for (final h in _hosts) {
      if (h.dead) continue;
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
    final fresh = await _spawnHost();
    fresh.pendingAttaches++;
    return fresh;
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
        // Sessions inside a host swallow their own errors. We only kill
        // the host on a truly hard failure, in which case the supervisor
        // observes via `error` / `exit` and tears down all attached
        // sessions.
        errorsAreFatal: true,
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
