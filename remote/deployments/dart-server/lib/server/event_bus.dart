/// `pg`-style cross-isolate event bus.
///
/// Conceptually mirrors Erlang's [`:pg`](https://www.erlang.org/doc/man/pg)
/// process-group registry: any session can `join(topic)` and any session
/// can `publish(topic, ...)` to fan a payload out to every joiner.
///
/// In the host-pool architecture each session lives inside one of M
/// session-host isolates; the bus does not address sessions directly but
/// addresses `(hostMailbox, sessionId)` pairs. Each registered "mailbox"
/// is the SendPort of the session-host isolate that owns the session,
/// and every delivery is wrapped in [RouteToSession] so the host can
/// demultiplex. This lets sessions sharing a host receive bus events
/// independently while keeping the SendPort topology star-shaped from
/// the bus's perspective.
///
/// Threading / concurrency model
/// -----------------------------
/// All bus operations run on the main isolate's event loop, so the
/// internal maps don't need locks. Callers must not call into the bus
/// from background isolates directly — they go through their outbound
/// SendPort, which is processed by the supervisor on the main isolate.
///
/// Backpressure
/// ------------
/// SendPort.send is fire-and-forget and lock-free; it cannot fail or
/// block. For now we emit a metric tick per delivery and rely on the
/// downstream WebSocket's own buffer for backpressure. If a single noisy
/// topic ever swamps a slow client we'll add a bounded per-session queue
/// in front of the outbound port; until then this stays simple.
library;

import 'dart:isolate';

import 'package:rxdart/rxdart.dart';

import '../shared/wire_messages.dart';
import 'metrics.dart';

class _Subscriber {
  _Subscriber(this.sessionId, this.hostMailbox);
  final String sessionId;

  /// SendPort of the session-host isolate that owns [sessionId]. The
  /// bus pushes `RouteToSession(sessionId, BusDelivery(...))` onto this
  /// port; the host's mailbox loop demultiplexes per-session.
  final SendPort hostMailbox;
}

class EventBus {
  EventBus({required this.metrics});

  final Metrics metrics;

  /// topic → ordered list of subscribers (insertion order, not session id).
  final _topics = <String, List<_Subscriber>>{};

  /// sessionId → set of topics the session is currently joined to. Lets us
  /// tear a session down in O(degree) instead of scanning every topic on
  /// disconnect.
  final _bySession = <String, Set<String>>{};

  /// sessionId → host-mailbox SendPort (the session-host isolate that
  /// currently owns the session). May get rewritten if the supervisor
  /// ever migrates a session between hosts; never null while registered.
  final _mailboxes = <String, SendPort>{};

  /// Live event log for observability. The metrics endpoint and tests can
  /// observe membership churn without touching internals.
  final _events = PublishSubject<EventBusChange>();
  Stream<EventBusChange> get events => _events.stream;

  /// Number of distinct topics currently with at least one joiner.
  int get topicCount => _topics.length;

  /// Number of sessions registered with the bus (regardless of joins).
  int get sessionCount => _mailboxes.length;

  /// Sum of joiners across all topics. Equivalent to
  /// `Σ |members(topic)|`.
  int get totalJoinCount {
    var n = 0;
    for (final list in _topics.values) {
      n += list.length;
    }
    return n;
  }

  /// Sessions joined to [topic], preserving join order. Returns an empty
  /// list (not null) for unknown topics.
  List<String> members(String topic) =>
      _topics[topic]?.map((s) => s.sessionId).toList(growable: false) ??
      const <String>[];

  /// Topics [sessionId] is currently joined to.
  Set<String> joinedTopics(String sessionId) =>
      _bySession[sessionId] ?? const <String>{};

  /// Register a session with the bus. Must be called once per session,
  /// before any join/publish from that session is processed. [hostMailbox]
  /// is the SendPort of the session-host isolate that currently owns the
  /// session; the bus only stores the reference and lets the supervisor
  /// drive teardown.
  void register(String sessionId, SendPort hostMailbox) {
    _mailboxes[sessionId] = hostMailbox;
    _bySession[sessionId] ??= <String>{};
    metrics.inc('dart_eventbus_register_total');
    _events.add(EventBusChange.register(sessionId));
  }

  /// Unregister a session. Removes it from every topic in O(degree).
  void unregister(String sessionId) {
    final topics = _bySession.remove(sessionId);
    _mailboxes.remove(sessionId);
    if (topics != null) {
      for (final topic in topics) {
        final list = _topics[topic];
        if (list == null) continue;
        list.removeWhere((s) => s.sessionId == sessionId);
        if (list.isEmpty) _topics.remove(topic);
      }
    }
    metrics.inc('dart_eventbus_unregister_total');
    _events.add(EventBusChange.unregister(sessionId));
  }

  /// Add [sessionId] to [topic]'s member list. Idempotent.
  void join(String sessionId, String topic) {
    final hostMailbox = _mailboxes[sessionId];
    if (hostMailbox == null) return;
    final joined = _bySession[sessionId] ??= <String>{};
    if (!joined.add(topic)) return; // already joined
    final list = _topics.putIfAbsent(topic, () => <_Subscriber>[]);
    list.add(_Subscriber(sessionId, hostMailbox));
    metrics.inc('dart_eventbus_join_total');
    _events.add(EventBusChange.join(sessionId, topic));
  }

  /// Remove [sessionId] from [topic]. Idempotent.
  void leave(String sessionId, String topic) {
    final joined = _bySession[sessionId];
    if (joined == null || !joined.remove(topic)) return;
    final list = _topics[topic];
    if (list != null) {
      list.removeWhere((s) => s.sessionId == sessionId);
      if (list.isEmpty) _topics.remove(topic);
    }
    metrics.inc('dart_eventbus_leave_total');
    _events.add(EventBusChange.leave(sessionId, topic));
  }

  /// Fan a [BusDelivery] out to every member of [topic]. The publisher
  /// itself is included only when [includeSelf] is true (matches Phoenix's
  /// `broadcast_from`/`broadcast` distinction).
  ///
  /// Returns the number of sessions the message was actually delivered to.
  int publish({
    required String topic,
    required String kind,
    required Map<String, Object?> data,
    required String fromSessionId,
    bool includeSelf = true,
  }) {
    final list = _topics[topic];
    if (list == null || list.isEmpty) {
      metrics.inc('dart_eventbus_publish_empty_total');
      return 0;
    }
    final delivery = BusDelivery(
      topic: topic,
      kind: kind,
      data: data,
      fromSessionId: fromSessionId,
      atUs: DateTime.now().microsecondsSinceEpoch,
    );
    var delivered = 0;
    for (final sub in list) {
      if (!includeSelf && sub.sessionId == fromSessionId) continue;
      try {
        sub.hostMailbox.send(RouteToSession(
          sessionId: sub.sessionId,
          event: delivery,
        ));
        delivered++;
      } catch (_) {
        // SendPort lifetime is owned by the supervisor. If the receiver
        // host isolate has already exited the send is a no-op; we ignore.
      }
    }
    metrics.inc('dart_eventbus_publish_total');
    metrics.inc('dart_eventbus_delivered_total', delivered);
    return delivered;
  }

  Future<void> close() async {
    await _events.close();
    _topics.clear();
    _bySession.clear();
    _mailboxes.clear();
  }
}

/// Single membership-churn event emitted by [EventBus.events].
final class EventBusChange {
  const EventBusChange._(this.kind, this.sessionId, this.topic);

  final String kind;
  final String sessionId;
  final String? topic;

  factory EventBusChange.register(String sessionId) =>
      EventBusChange._('register', sessionId, null);
  factory EventBusChange.unregister(String sessionId) =>
      EventBusChange._('unregister', sessionId, null);
  factory EventBusChange.join(String sessionId, String topic) =>
      EventBusChange._('join', sessionId, topic);
  factory EventBusChange.leave(String sessionId, String topic) =>
      EventBusChange._('leave', sessionId, topic);

  @override
  String toString() => 'EventBusChange($kind, $sessionId, $topic)';
}
