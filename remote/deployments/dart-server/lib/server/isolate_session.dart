/// One file = the full body of work that runs *inside* a per-connection
/// session isolate.
///
/// The shape is deliberately Phoenix-ish: every connected WebSocket peer is
/// matched 1:1 with a Dart `Isolate` (BEAM process analogue) that owns:
///
///   * a private mutable state record (counters, last-seen, etc.)
///   * a private RxDart broadcast stream of decoded inbound events
///   * an outbound channel back to the main isolate (HTTP/WS bridge)
///   * pg-style topic subscriptions via [BusJoin] / [BusPublish]
///
/// Killing the isolate kills the session. Crashing it is contained: the
/// supervisor on the main isolate observes the exit port and tears down the
/// associated WebSocket. Nothing else in the process is affected.
///
/// HTML for HTMX OOB swaps is produced by **Jaspr components** (see
/// `wss_components.dart`) — never by string concatenation. Each pipeline
/// builds a typed `Component`, which Jaspr then renders to a properly
/// escaped HTML fragment. That gives us composability, unit-testability,
/// and zero manual `htmlEscape` callsites.
library;

import 'dart:async';
import 'dart:isolate';

import 'package:jaspr/jaspr.dart';
import 'package:rxdart/rxdart.dart';

import '../shared/htmx_fragments.dart';
import '../shared/wire_messages.dart';
import 'wss_components.dart';

/// Default topic every session auto-joins on boot. Mirrors a Phoenix
/// `lobby` channel: a global broadcast space where any session can drop
/// a message and every other session sees it.
const String _lobbyTopic = 'lobby';

/// Well-known topics owned by the supervisor. Sessions auto-join both on
/// boot so they see identity churn + conversation directory mutations.
const String _presenceTopic = 'presence';
const String _convListTopic = 'conv-list';

Future<void> isolateSessionEntry(SendPort handshakePort) async {
  ensureJasprInit();

  final mailbox = ReceivePort();
  handshakePort.send(mailbox.sendPort);

  final iter = StreamIterator<dynamic>(mailbox);
  if (!await iter.moveNext()) return;

  final bootRaw = iter.current;
  if (bootRaw is! SessionBootMessage) {
    throw StateError('isolate boot frame was not a SessionBootMessage');
  }
  final boot = bootRaw;
  final outbound = boot.outbound as SendPort;

  final session = _Session(boot, outbound);
  try {
    await session.run(iter);
  } finally {
    await iter.cancel();
    mailbox.close();
  }
}

class _Session {
  _Session(this._boot, this._outbound);

  final SessionBootMessage _boot;
  final SendPort _outbound;

  final _inbound = PublishSubject<HtmxInbound>();
  final _busInbound = PublishSubject<BusDelivery>();

  /// Counter widget value (per-session).
  final _counter = BehaviorSubject<int>.seeded(0);

  /// Per-session echo history (not bus-shared).
  final _history = BehaviorSubject<List<String>>.seeded(const []);

  /// Lobby chat (cross-session bus deliveries on the global lobby topic).
  final _lobby = BehaviorSubject<List<LobbyRow>>.seeded(const []);

  /// Identity state mirrored locally so the session can render a "who
  /// am I" pill without round-tripping through the supervisor for every
  /// rerender. Kicked off as the anonymous default the supervisor binds
  /// at adopt-time.
  late final _identity =
      BehaviorSubject<({String userId, String displayName})>.seeded((
    userId: 'anon-${_boot.sessionId}',
    displayName:
        'anon-${_boot.sessionId.substring(0, _boot.sessionId.length.clamp(0, 4))}',
  ));

  /// Currently-open conversation. Drives the conversation panel and
  /// determines which `conv:<id>` deliveries get rendered into the
  /// chat stream. `''` = none open.
  final _activeConv = BehaviorSubject<String>.seeded('');

  /// `conversationId → recent message list`. Updated on
  /// `BusDelivery(kind: conv.message)`.
  final _convMessages =
      BehaviorSubject<Map<String, List<ConvMessage>>>.seeded(const {});

  /// Snapshot of the conversation directory. Lazily mirrored from
  /// `conv-list` topic; we never round-trip to the registry.
  final _convDirectory =
      BehaviorSubject<Map<String, ConvSummary>>.seeded(const {});

  Timer? _ticker;
  final _subs = <StreamSubscription<dynamic>>[];

  Future<void> run(StreamIterator<dynamic> iter) async {
    _wirePipelines();
    _joinTopics();
    await _emitGreeting();

    while (await iter.moveNext()) {
      final raw = iter.current;
      if (raw is InboundEvent) {
        switch (raw) {
          case InboundText(:final payload):
            _onInboundText(payload);
          case InboundBinary(:final bytes):
            await _emitFragment(StatusPill(
              'received ${bytes.length} binary bytes',
            ));
          case InboundClosed():
            await _dispose();
            return;
          case BusDelivery():
            _busInbound.add(raw);
        }
      } else if (raw == _shutdownSentinel) {
        await _dispose();
        return;
      }
    }
    await _dispose();
  }

  static const _shutdownSentinel = '__shutdown__';

  void _wirePipelines() {
    _subs.add(
      _counter
          .distinct()
          .map((v) => Counter(v))
          .asyncMap(renderFragment)
          .listen(_emitText),
    );

    _subs.add(
      _history
          .map((rows) =>
              rows.length <= 8 ? rows : rows.sublist(rows.length - 8))
          .map((rows) => EchoPanel(rows))
          .asyncMap(renderFragment)
          .listen(_emitText),
    );

    _subs.add(
      _lobby
          .map((rows) =>
              rows.length <= 16 ? rows : rows.sublist(rows.length - 16))
          .map((rows) => LobbyPanel(rows))
          .asyncMap(renderFragment)
          .listen(_emitText),
    );

    // Identity pill re-renders any time our own identity changes.
    _subs.add(
      _identity
          .distinct()
          .map((id) =>
              IdentityPanel(userId: id.userId, displayName: id.displayName))
          .asyncMap(renderFragment)
          .listen(_emitText),
    );

    // Conversation directory + active conversation drive two panels.
    _subs.add(
      Rx.combineLatest2<Map<String, ConvSummary>, String, ConvList>(
        _convDirectory,
        _activeConv,
        (dir, active) => ConvList(
          conversations: dir.values.toList(growable: false),
          activeId: active,
        ),
      ).asyncMap(renderFragment).listen(_emitText),
    );

    _subs.add(
      Rx.combineLatest2<String, Map<String, List<ConvMessage>>, ConvPanel>(
        _activeConv,
        _convMessages,
        (active, msgs) => ConvPanel(
          activeId: active,
          messages: msgs[active] ?? const <ConvMessage>[],
        ),
      ).asyncMap(renderFragment).listen(_emitText),
    );

    // HTMX inbound → state.
    _subs.add(_inbound.listen(_handleHtmxTrigger));

    // Bus inbound → state mutations.
    _subs.add(_busInbound.listen(_handleBusDelivery));

    // Server-driven 1Hz tick.
    _ticker = Timer.periodic(const Duration(seconds: 1), (_) {
      unawaited(_emitFragment(
        Clock(DateTime.now().toUtc().toIso8601String()),
      ));
    });
  }

  void _joinTopics() {
    _send(const BusJoin(_lobbyTopic));
    _send(const BusJoin(_presenceTopic));
    _send(const BusJoin(_convListTopic));

    // Announce arrival to the lobby.
    _send(BusPublish(
      topic: _lobbyTopic,
      kind: 'chat.system',
      data: <String, Object?>{'text': 'session ${_boot.sessionId} joined'},
      includeSelf: false,
    ));
  }

  Future<void> _emitGreeting() async {
    final ageMs =
        (DateTime.now().microsecondsSinceEpoch - _boot.spawnedAtUs) / 1000.0;
    await _emitFragment(SessionMeta(
      sessionId: _boot.sessionId,
      remoteAddr: _boot.remoteAddr,
      handshakeAgeMs: ageMs,
      topics: const [_lobbyTopic, _presenceTopic, _convListTopic],
    ));

    await _emitFragment(const Counter(0));
    await _emitFragment(const EchoPanel(<String>[]));
    await _emitFragment(const LobbyPanel(<LobbyRow>[]));
    await _emitFragment(Clock(DateTime.now().toUtc().toIso8601String()));
    await _emitFragment(IdentityPanel(
      userId: _identity.value.userId,
      displayName: _identity.value.displayName,
    ));
    await _emitFragment(const ConvList(
      conversations: <ConvSummary>[],
      activeId: '',
    ));
    await _emitFragment(const ConvPanel(
      activeId: '',
      messages: <ConvMessage>[],
    ));
    _send(const MetricEvent('dart_sessions_opened_total'));
  }

  void _onInboundText(String text) {
    final parsed = parseHtmxInboundJson(text);
    if (parsed == null) {
      unawaited(_emitFragment(const StatusPill('non-json frame ignored')));
      return;
    }
    _inbound.add(parsed);
  }

  void _send(OutboundFrame frame) => _outbound.send(frame);
  void _emitText(String html) => _outbound.send(OutboundText(html));

  /// Async helper used for one-shot fragment emissions that aren't
  /// driven by a long-lived RxDart pipeline (status pills, the
  /// initial greeting, the 1Hz clock, etc.).
  Future<void> _emitFragment(Component component) async {
    final html = await renderFragment(component);
    _emitText(html);
  }

  // ---- HTMX trigger handling ---------------------------------------------

  void _handleHtmxTrigger(HtmxInbound msg) {
    switch (msg.triggerName ?? msg.trigger) {
      case 'bump':
        _counter.add(_counter.value + 1);
        _send(const MetricEvent('dart_session_bumps_total'));
      case 'reset':
        _counter.add(0);
        _send(const MetricEvent('dart_session_resets_total'));
      case 'echo':
        final text = msg.stringField('message').trim();
        if (text.isEmpty) return;
        _history.add([..._history.value, text]);
        _send(const MetricEvent('dart_session_echoes_total'));
      case 'say':
        final text = msg.stringField('text').trim();
        if (text.isEmpty) return;
        _send(BusPublish(
          topic: _lobbyTopic,
          kind: 'chat.say',
          data: <String, Object?>{
            'text': text,
            'from': _identity.value.userId,
            'displayName': _identity.value.displayName,
          },
        ));
        _send(const MetricEvent('dart_session_says_total'));
      case 'identify':
        final userId = msg.stringField('user_id').trim();
        final displayName = msg.stringField('display_name').trim();
        if (userId.isEmpty) {
          unawaited(_emitFragment(
            const StatusPill('user_id required to identify'),
          ));
          return;
        }
        _identity.add((userId: userId, displayName: displayName));
        _send(Identify(userId: userId, displayName: displayName));
      case 'open-conv':
        final convId = msg.stringField('conversation_id').trim();
        final title = msg.stringField('title').trim();
        if (convId.isEmpty) return;
        _send(ConversationOpen(
          conversationId: convId,
          title: title,
          kind: msg.stringField('kind', 'chat'),
        ));
      case 'join-conv':
        final convId = msg.stringField('conversation_id').trim();
        if (convId.isEmpty) return;
        _activeConv.add(convId);
        _send(ConversationJoin(convId));
      case 'leave-conv':
        final convId = msg.stringField('conversation_id').trim();
        if (convId.isEmpty) return;
        if (_activeConv.value == convId) _activeConv.add('');
        _send(ConversationLeave(
          convId,
          dropMembership: msg.stringField('drop') == '1',
        ));
      case 'say-conv':
        final convId = msg.stringField('conversation_id').trim();
        final text = msg.stringField('text').trim();
        if (convId.isEmpty || text.isEmpty) return;
        _send(ConversationSay(conversationId: convId, text: text));
      case 'switch-conv':
        // Sets which conversation the local panel renders. No supervisor
        // round-trip needed; the bus deliveries already populate
        // `_convMessages` for any topic this session is bus-joined to.
        _activeConv.add(msg.stringField('conversation_id').trim());
      case 'delete-conv':
        final convId = msg.stringField('conversation_id').trim();
        if (convId.isEmpty) return;
        _send(ConversationDelete(convId));
      default:
        unawaited(_emitFragment(StatusPill(
          'unknown trigger ${msg.triggerName ?? msg.trigger ?? "<none>"}',
        )));
    }
  }

  // ---- Bus delivery handling ---------------------------------------------

  void _handleBusDelivery(BusDelivery delivery) {
    if (delivery.topic == _lobbyTopic && delivery.kind == 'chat.say') {
      final text = delivery.data['text'] as String? ?? '';
      if (text.isEmpty) return;
      _lobby.add([
        ..._lobby.value,
        LobbyRow(
          name: (delivery.data['displayName'] as String?) ??
              (delivery.data['from'] as String?) ??
              delivery.fromSessionId,
          text: text,
          self: delivery.fromSessionId == _boot.sessionId,
        ),
      ]);
      _send(const MetricEvent('dart_session_lobby_deliveries_total'));
      return;
    }
    if (delivery.topic == _lobbyTopic && delivery.kind == 'chat.system') {
      final text = delivery.data['text'] as String? ?? '';
      unawaited(_emitFragment(StatusPill(text)));
      return;
    }

    if (delivery.topic == _presenceTopic) {
      final user = delivery.data['userId'] as String? ?? '?';
      final name = delivery.data['displayName'] as String? ?? user;
      switch (delivery.kind) {
        case 'presence.identified':
          unawaited(_emitFragment(StatusPill('$name identified as $user')));
        case 'presence.session_left':
          final off = delivery.data['userOffline'] as bool? ?? false;
          unawaited(_emitFragment(StatusPill(
            off ? '$name went offline' : 'session of $name closed',
          )));
      }
      return;
    }

    if (delivery.topic == _convListTopic) {
      final next = Map<String, ConvSummary>.from(_convDirectory.value);
      switch (delivery.kind) {
        case 'conv.deleted':
          final id = delivery.data['conversationId'] as String? ?? '';
          next.remove(id);
        case 'conv.created':
        case 'conv.updated':
        case 'conv.user_joined':
        case 'conv.user_left':
        case 'conv.bumped':
          final id = delivery.data['id'] as String? ??
              delivery.data['conversationId'] as String? ??
              '';
          if (id.isEmpty) return;
          next[id] = _mergeConvSummary(next[id], delivery.data, id);
      }
      _convDirectory.add(next);
      return;
    }

    // Per-conversation topics: `conv:<id>`.
    if (delivery.topic.startsWith('conv:') &&
        delivery.kind == 'conv.message') {
      final convId = delivery.topic.substring('conv:'.length);
      final msgs = Map<String, List<ConvMessage>>.from(_convMessages.value);
      final list = [
        ...?msgs[convId],
        ConvMessage(
          name: (delivery.data['displayName'] as String?) ??
              (delivery.data['userId'] as String?) ??
              '?',
          text: delivery.data['text'] as String? ?? '',
          self: delivery.data['userId'] == _identity.value.userId,
        ),
      ];
      // Cap the local view at 32; supervisor's authoritative cache is
      // larger and outlives any single session.
      msgs[convId] = list.length <= 32 ? list : list.sublist(list.length - 32);
      _convMessages.add(msgs);
      _send(const MetricEvent('dart_session_conv_deliveries_total'));
      return;
    }
    if (delivery.topic.startsWith('conv:') &&
        delivery.kind == 'conv.user_joined') {
      final name = delivery.data['displayName'] as String? ??
          delivery.data['userId'] as String? ??
          '?';
      unawaited(_emitFragment(StatusPill('$name joined ${delivery.topic}')));
      return;
    }
  }

  ConvSummary _mergeConvSummary(
    ConvSummary? prev,
    Map<String, Object?> patch,
    String id,
  ) {
    int? readInt(String key) {
      final v = patch[key];
      if (v is int) return v;
      if (v is num) return v.toInt();
      if (v is String) return int.tryParse(v);
      return null;
    }

    final title = (patch['title'] as String?) ?? prev?.title ?? id;
    final memberCount = readInt('memberCount') ?? prev?.memberCount;
    final messageCount = readInt('messageCount') ?? prev?.messageCount ?? 0;
    final lastActivityAtUs = readInt('lastActivityAtUs') ??
        prev?.lastActivityAtUs ??
        DateTime.now().microsecondsSinceEpoch;
    return ConvSummary(
      id: id,
      title: title,
      memberCount: memberCount,
      messageCount: messageCount,
      lastActivityAtUs: lastActivityAtUs,
    );
  }

  Future<void> _dispose() async {
    _ticker?.cancel();
    _send(BusPublish(
      topic: _lobbyTopic,
      kind: 'chat.system',
      data: <String, Object?>{'text': 'session ${_boot.sessionId} left'},
      includeSelf: false,
    ));
    _send(const BusLeave(_lobbyTopic));
    _send(const BusLeave(_presenceTopic));
    _send(const BusLeave(_convListTopic));
    for (final sub in _subs) {
      try {
        await sub.cancel();
      } catch (_) {/* swallow */}
    }
    await _inbound.close();
    await _busInbound.close();
    await _counter.close();
    await _history.close();
    await _lobby.close();
    await _identity.close();
    await _activeConv.close();
    await _convMessages.close();
    await _convDirectory.close();
    _send(const MetricEvent('dart_sessions_closed_total'));
  }
}

/// Convenience: the supervisor calls this to wake an isolate's mailbox loop
/// in its dispose path. Sends a sentinel object the loop knows to break on.
void requestSessionShutdown(SendPort sessionPort) {
  sessionPort.send('__shutdown__');
}
