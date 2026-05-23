/// Wire-message types shared between the WSS bridge (main isolate) and each
/// per-connection session isolate.
///
/// All messages are plain Dart objects (no JSON involved on the SendPort).
/// JSON / HTML serialization happens at the WebSocket boundary; the isolate
/// boundary speaks Dart records & sealed classes directly.
///
/// The hierarchy is split in two halves:
///
///   * [InboundEvent]   — anything the session isolate's mailbox accepts.
///                        Includes raw WS frames (forwarded from the bridge)
///                        AND cross-isolate [BusDelivery] messages produced
///                        by the pg-style [EventBus] fanout.
///
///   * [OutboundFrame]  — anything a session isolate emits on its outbound
///                        SendPort. The bridge dispatches: WS-bound frames
///                        go to the socket; metrics frames go to the
///                        aggregator; bus frames go to the EventBus.
library;

import 'dart:typed_data';

/// Boot payload handed to a freshly-spawned session isolate. Contains the
/// information it needs to identify the connection plus the SendPort it
/// should use to push outbound frames back to the main isolate.
final class SessionBootMessage {
  SessionBootMessage({
    required this.sessionId,
    required this.remoteAddr,
    required this.requestPath,
    required this.headers,
    required this.outbound,
    required this.spawnedAtUs,
  });

  final String sessionId;
  final String remoteAddr;
  final String requestPath;
  final Map<String, String> headers;

  /// SendPort owned by the main isolate. Session writes [OutboundFrame]
  /// instances here and the bridge translates them appropriately.
  final dynamic outbound;

  final int spawnedAtUs;
}

// ---------------------------------------------------------------------------
// Inbound (bridge / bus → session isolate)
// ---------------------------------------------------------------------------

/// Anything the session isolate's mailbox is expected to consume.
sealed class InboundEvent {
  const InboundEvent();
}

/// Text WebSocket frame received from the peer.
final class InboundText extends InboundEvent {
  const InboundText(this.payload);
  final String payload;
}

/// Binary WebSocket frame received from the peer.
final class InboundBinary extends InboundEvent {
  const InboundBinary(this.bytes);
  final Uint8List bytes;
}

/// WebSocket closed (peer or local). The session must drain any pending
/// state and exit; the bridge has already begun teardown.
final class InboundClosed extends InboundEvent {
  const InboundClosed(this.code, this.reason);
  final int? code;
  final String? reason;
}

/// Cross-isolate message delivered by the EventBus to a session that is
/// currently joined to [topic]. Delivery is best-effort and unordered with
/// respect to other topics, but FIFO within a single (publisher, topic) pair
/// because the bridge processes its outbound mailbox in order.
final class BusDelivery extends InboundEvent {
  const BusDelivery({
    required this.topic,
    required this.kind,
    required this.data,
    required this.fromSessionId,
    required this.atUs,
  });

  /// Topic name the publisher used. Same string the receiver passed to
  /// [BusJoin].
  final String topic;

  /// Free-form discriminator chosen by the publisher (e.g. `chat.say`,
  /// `presence.enter`). The session pattern-matches on this field.
  final String kind;

  /// Structured payload. Must contain only Dart-transferable values
  /// (primitives, lists, maps, Uint8List).
  final Map<String, Object?> data;

  /// Session id of the publisher. Receivers can use this to distinguish
  /// "self" deliveries from "other" deliveries when echoing.
  final String fromSessionId;

  /// Microseconds-since-epoch the bus stamped at fanout time. Useful for
  /// per-topic ordering and basic latency measurements.
  final int atUs;
}

// ---------------------------------------------------------------------------
// Outbound (session isolate → bridge)
// ---------------------------------------------------------------------------

/// Anything a session can post on its outbound SendPort. The bridge uses a
/// type switch to dispatch.
sealed class OutboundFrame {
  const OutboundFrame();
}

/// Raw text frame the bridge will write to the WebSocket peer. HTMX
/// fragments arrive here.
final class OutboundText extends OutboundFrame {
  const OutboundText(this.text);
  final String text;
}

/// Raw binary frame the bridge will write to the WebSocket peer.
final class OutboundBinary extends OutboundFrame {
  const OutboundBinary(this.bytes);
  final Uint8List bytes;
}

/// Asks the bridge to gracefully close the WebSocket. The session emits
/// this when its supervisor decides the conversation is over (idle, panic,
/// etc.).
final class OutboundClose extends OutboundFrame {
  const OutboundClose({this.code = 1000, this.reason = 'session_done'});
  final int code;
  final String reason;
}

/// Telemetry counter mutation. Sessions never write to global metrics
/// directly; they emit these events and the bridge folds them in.
final class MetricEvent extends OutboundFrame {
  const MetricEvent(this.name, [this.delta = 1]);
  final String name;
  final int delta;
}

/// Subscribe the emitting session to [topic] in the EventBus. Idempotent.
/// Matches Erlang's `:pg.join(group, self())`.
final class BusJoin extends OutboundFrame {
  const BusJoin(this.topic);
  final String topic;
}

/// Unsubscribe the emitting session from [topic]. Idempotent. Matches
/// Erlang's `:pg.leave(group, self())`.
final class BusLeave extends OutboundFrame {
  const BusLeave(this.topic);
  final String topic;
}

/// Fan a structured payload out to every session currently joined to
/// [topic]. The publishing session also receives the delivery unless the
/// bus is configured to skip self.
final class BusPublish extends OutboundFrame {
  const BusPublish({
    required this.topic,
    required this.kind,
    this.data = const <String, Object?>{},
    this.includeSelf = true,
  });

  final String topic;
  final String kind;
  final Map<String, Object?> data;
  final bool includeSelf;
}

// ---------------------------------------------------------------------------
// Identity / conversation frames (handled by the supervisor, not the bus)
// ---------------------------------------------------------------------------

/// Tell the supervisor which user owns this session. The supervisor
/// updates the [Presence] index and broadcasts `presence.identified` on
/// the well-known `presence` topic.
final class Identify extends OutboundFrame {
  const Identify({required this.userId, this.displayName = ''});
  final String userId;
  final String displayName;
}

/// Create-or-refresh a conversation in the registry. Doesn't auto-join
/// the calling user; pair with [ConversationJoin] for that.
final class ConversationOpen extends OutboundFrame {
  const ConversationOpen({
    required this.conversationId,
    this.title = '',
    this.kind = 'chat',
  });
  final String conversationId;
  final String title;
  final String kind;
}

/// Add the calling user to a conversation's member list AND bus-join the
/// conversation's underlying topic for this specific session. Other
/// sessions of the same user keep their own bus-join state.
final class ConversationJoin extends OutboundFrame {
  const ConversationJoin(this.conversationId);
  final String conversationId;
}

/// Bus-leave the conversation topic for this session. By default the
/// user remains a member of the conversation so their other sessions
/// (and reconnects) stay subscribed. Pass `dropMembership: true` to
/// fully leave (Phoenix's `unsub` semantics).
final class ConversationLeave extends OutboundFrame {
  const ConversationLeave(
    this.conversationId, {
    this.dropMembership = false,
  });
  final String conversationId;
  final bool dropMembership;
}

/// Append a chat message to [conversationId] and fan it out via the bus.
/// The supervisor records the message in the registry's recent-message
/// cache before publishing.
final class ConversationSay extends OutboundFrame {
  const ConversationSay({
    required this.conversationId,
    required this.text,
  });
  final String conversationId;
  final String text;
}

/// Hard delete a conversation. Currently unrestricted (any session may
/// emit) since there's no auth layer in this deployment; production
/// usage would gate this on the conversation's `createdByUserId`.
final class ConversationDelete extends OutboundFrame {
  const ConversationDelete(this.conversationId);
  final String conversationId;
}

// ---------------------------------------------------------------------------
// Host-pool routing (main isolate ↔ session-host isolate)
// ---------------------------------------------------------------------------
//
// `SessionSupervisor` keeps a pool of "session-host" isolates. Each host
// owns N sessions (N configurable, default 100). Host mailboxes accept
// these three message types. `_Session` instances inside the host never
// see them directly — the host's run loop demultiplexes onto per-session
// inbox streams.

/// Tells a host to instantiate a new in-host `_Session` from [boot] and
/// start its pipelines. The session's `outbound` SendPort is reused from
/// [boot], same as in the legacy 1-isolate-per-session model, so the
/// supervisor's outbound listener doesn't need any changes.
final class AttachSession {
  const AttachSession(this.boot);
  final SessionBootMessage boot;
}

/// Tells a host to dispose the session identified by [sessionId]. Idempotent.
/// Triggers the session's normal `_dispose` path, which emits "left"
/// announcements on the lobby/conv topics before unwinding RxDart.
final class DetachSession {
  const DetachSession(this.sessionId);
  final String sessionId;
}

/// Forwards an [InboundEvent] (raw WS frames AND `BusDelivery`s) to a
/// specific session inside the host. The host looks up the session by
/// id and pushes the event onto its per-session inbox. Unknown ids are
/// silently dropped (covers the race between detach-decided-on-main and
/// in-flight forwards).
final class RouteToSession {
  const RouteToSession({required this.sessionId, required this.event});
  final String sessionId;
  final InboundEvent event;
}
