/// Phoenix-style per-connection supervisor.
///
/// Lives on the main isolate. For each accepted WebSocket it:
///
///   1. Spawns a fresh session isolate (`isolateSessionEntry`).
///   2. Performs the SendPort handshake.
///   3. Sends a [SessionBootMessage] to the isolate.
///   4. Pumps WS inbound frames into the isolate as [InboundEvent]s.
///   5. Pumps [OutboundFrame]s coming out of the isolate back into the WS
///      (or into the metrics aggregator / pg-style EventBus / Presence /
///      ConversationRegistry, depending on type).
///   6. Cleans up on disconnect / error / `errorsAreFatal` exit.
///
/// The supervisor never blocks one session on another: each connection owns
/// its own isolate and its own RxDart graph.
///
/// In addition to per-session plumbing, the supervisor owns the
/// integration glue that keeps the four main-isolate stores consistent:
///
///   * [EventBus]              — pubsub / fanout topology
///   * [Presence]              — userId ↔ sessionId index
///   * [ConversationRegistry]  — conversations / members / recent-msgs cache
///
/// Sessions only ever talk to the supervisor (via OutboundFrame) and to
/// each other through the bus.
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

class SessionSupervisor {
  SessionSupervisor({
    required this.metrics,
    required this.bus,
    required this.presence,
    required this.conversations,
  });

  final Metrics metrics;
  final EventBus bus;
  final Presence presence;
  final ConversationRegistry conversations;

  final _liveCount = BehaviorSubject<int>.seeded(0);
  int _spawnedTotal = 0;

  Stream<int> get liveCountStream => _liveCount.stream;
  int get liveCount => _liveCount.value;
  int get spawnedTotal => _spawnedTotal;

  Future<void> adopt(
    WebSocket socket, {
    required String sessionId,
    required String remoteAddr,
    required String requestPath,
    required Map<String, String> headers,
  }) async {
    final handshake = ReceivePort('dd-dart-handshake-$sessionId');
    final outbound = ReceivePort('dd-dart-outbound-$sessionId');
    final exit = ReceivePort('dd-dart-exit-$sessionId');
    final error = ReceivePort('dd-dart-error-$sessionId');

    Isolate? isolate;
    SendPort? sessionMailbox;
    StreamSubscription<dynamic>? inboundSub;
    StreamSubscription<dynamic>? outboundSub;
    StreamSubscription<dynamic>? exitSub;
    StreamSubscription<dynamic>? errorSub;
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

      try {
        await inboundSub?.cancel();
      } catch (_) {/* swallow */}
      try {
        await outboundSub?.cancel();
      } catch (_) {/* swallow */}
      try {
        await exitSub?.cancel();
      } catch (_) {/* swallow */}
      try {
        await errorSub?.cancel();
      } catch (_) {/* swallow */}
      handshake.close();
      outbound.close();
      exit.close();
      error.close();
      try {
        isolate?.kill(priority: Isolate.beforeNextEvent);
      } catch (_) {/* swallow */}
      try {
        if (socket.readyState != WebSocket.closed) {
          await socket.close(1000, why);
        }
      } catch (_) {/* swallow */}
      if (!done.isCompleted) done.complete();
    }

    try {
      isolate = await Isolate.spawn<SendPort>(
        isolateSessionEntry,
        handshake.sendPort,
        debugName: 'dd-dart-session-$sessionId',
        errorsAreFatal: true,
        onExit: exit.sendPort,
        onError: error.sendPort,
      );
    } catch (e) {
      metrics.inc('dart_sessions_spawn_failed_total');
      await teardown('spawn_failed:$e');
      rethrow;
    }

    _spawnedTotal++;
    _liveCount.add(_liveCount.value + 1);
    metrics.inc('dart_sessions_spawned_total');

    sessionMailbox = (await handshake.first) as SendPort;

    // Pre-register with the bus + presence index BEFORE handing the boot
    // message to the isolate, so the session can issue BusJoin /
    // ConversationJoin synchronously during its bootstrap.
    bus.register(sessionId, sessionMailbox);
    presence.bind(
      sessionId,
      _anonymousUserIdFor(sessionId),
      displayName: _anonymousDisplayNameFor(sessionId),
    );

    sessionMailbox.send(SessionBootMessage(
      sessionId: sessionId,
      remoteAddr: remoteAddr,
      requestPath: requestPath,
      headers: headers,
      outbound: outbound.sendPort,
      spawnedAtUs: DateTime.now().microsecondsSinceEpoch,
    ));

    inboundSub = socket.listen(
      (data) {
        if (data is String) {
          sessionMailbox?.send(InboundText(data));
        } else if (data is List<int>) {
          sessionMailbox?.send(InboundBinary(_asUint8List(data)));
        }
      },
      onError: (Object err, StackTrace st) {
        metrics.inc('dart_sessions_ws_error_total');
        unawaited(teardown('ws_error:$err'));
      },
      onDone: () {
        sessionMailbox?.send(InboundClosed(socket.closeCode, socket.closeReason));
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

    exitSub = exit.listen((_) {
      unawaited(teardown('isolate_exit'));
    });
    errorSub = error.listen((err) {
      metrics.inc('dart_sessions_isolate_error_total');
      unawaited(teardown('isolate_error:$err'));
    });

    socket.done.whenComplete(() {
      if (!done.isCompleted) {
        unawaited(teardown('socket_done'));
      }
    });
    return done.future;
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
    await _liveCount.close();
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
