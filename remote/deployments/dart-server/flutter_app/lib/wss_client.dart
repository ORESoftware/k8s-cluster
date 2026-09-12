/// Flutter-side WebSocket client that speaks the same protocol as the
/// HTMX SSR demo at /dart/pages/wss.
///
/// Outbound: ws-send-shaped JSON
/// ```
///   {"text":"hello","HEADERS":{"HX-Request":"true","HX-Trigger-Name":"say"}}
/// ```
///
/// Inbound: server pushes HTML fragments wrapped in `hx-swap-oob` divs.
/// The Flutter app extracts a few labelled payloads with surface-level
/// regexes and projects them onto RxDart streams that Flutter widgets
/// observe via `StreamBuilder`.
///
/// Surfaces exposed (each as its own BehaviorSubject):
///   * connection                      — connecting / connected / disconnected
///   * meta                            — session metadata dict (id, remote, …)
///   * clock                           — server clock string
///   * counter                         — per-session counter
///   * echo                            — per-session echo history
///   * lobby                           — global lobby chat
///   * status                          — last status / log line
///   * identity                        — { userId, displayName }
///   * conversations                   — directory map { id → meta }
///   * activeConversationId            — which conv the panel is showing
///   * conversationMessages            — { convId → list of messages }
library;

import 'dart:async';
import 'dart:convert';

import 'package:rxdart/rxdart.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

class WssClient {
  WssClient(this.url);

  final Uri url;

  WebSocketChannel? _channel;
  StreamSubscription<dynamic>? _sub;

  // ---- Reactive state surface ---------------------------------------------

  final connection = BehaviorSubject<WssConnectionState>.seeded(
    WssConnectionState.disconnected,
  );

  final clock = BehaviorSubject<String>.seeded('');
  final counter = BehaviorSubject<int>.seeded(0);
  final echo = BehaviorSubject<List<String>>.seeded(const []);
  final lobby = BehaviorSubject<List<LobbyEntry>>.seeded(const []);
  final meta = BehaviorSubject<Map<String, String>>.seeded(const {});
  final status = BehaviorSubject<String>.seeded('idle');

  /// Current identity (anon-* until the user identifies).
  final identity = BehaviorSubject<Identity>.seeded(
    const Identity(userId: '', displayName: ''),
  );

  /// Conversation directory snapshot. Keyed by conversation id, value is
  /// a map of metadata (`title`, `messageCount`, `memberCount`,
  /// `lastActivityAtUs`, …).
  final conversations =
      BehaviorSubject<Map<String, Map<String, Object?>>>.seeded(const {});

  /// Currently-open conversation id (`''` = none).
  final activeConversationId = BehaviorSubject<String>.seeded('');

  /// `convId → recent messages` rolling cache. Mirrors the per-conv
  /// list the server fans out via `conv:<id>` topics.
  final conversationMessages =
      BehaviorSubject<Map<String, List<ConversationEntry>>>.seeded(const {});

  // ---- Connect / send / close ---------------------------------------------

  Future<void> connect() async {
    if (connection.value == WssConnectionState.connected ||
        connection.value == WssConnectionState.connecting) {
      return;
    }
    connection.add(WssConnectionState.connecting);
    try {
      final ch = WebSocketChannel.connect(url);
      _channel = ch;
      _sub = ch.stream.listen(
        _onFrame,
        onError: (Object err, StackTrace st) {
          status.add('error: $err');
          connection.add(WssConnectionState.disconnected);
        },
        onDone: () {
          connection.add(WssConnectionState.disconnected);
          status.add('disconnected');
        },
      );
      connection.add(WssConnectionState.connected);
      status.add('connected');
    } catch (e) {
      status.add('connect failed: $e');
      connection.add(WssConnectionState.disconnected);
    }
  }

  void send({
    required String triggerName,
    Map<String, Object?> fields = const {},
  }) {
    final ch = _channel;
    if (ch == null) return;
    final payload = <String, Object?>{
      ...fields,
      'HEADERS': <String, String>{
        'HX-Request': 'true',
        'HX-Trigger': triggerName,
        'HX-Trigger-Name': triggerName,
      },
    };
    ch.sink.add(jsonEncode(payload));
  }

  // ---- Convenience action helpers -----------------------------------------

  void identify({required String userId, String displayName = ''}) {
    if (userId.trim().isEmpty) return;
    send(triggerName: 'identify', fields: {
      'user_id': userId.trim(),
      'display_name': displayName.trim(),
    });
  }

  void openConversation({
    required String conversationId,
    String title = '',
    String kind = 'chat',
  }) {
    if (conversationId.trim().isEmpty) return;
    send(triggerName: 'open-conv', fields: {
      'conversation_id': conversationId.trim(),
      'title': title.trim(),
      'kind': kind,
    });
  }

  void joinConversation(String conversationId) {
    if (conversationId.trim().isEmpty) return;
    activeConversationId.add(conversationId.trim());
    send(triggerName: 'join-conv', fields: {
      'conversation_id': conversationId.trim(),
    });
  }

  void leaveConversation(String conversationId, {bool dropMembership = false}) {
    if (conversationId.trim().isEmpty) return;
    if (activeConversationId.value == conversationId.trim()) {
      activeConversationId.add('');
    }
    send(triggerName: 'leave-conv', fields: {
      'conversation_id': conversationId.trim(),
      if (dropMembership) 'drop': '1',
    });
  }

  void sayInConversation({required String conversationId, required String text}) {
    if (conversationId.trim().isEmpty || text.trim().isEmpty) return;
    send(triggerName: 'say-conv', fields: {
      'conversation_id': conversationId.trim(),
      'text': text.trim(),
    });
  }

  void switchConversation(String conversationId) {
    activeConversationId.add(conversationId.trim());
    send(triggerName: 'switch-conv', fields: {
      'conversation_id': conversationId.trim(),
    });
  }

  void deleteConversation(String conversationId) {
    if (conversationId.trim().isEmpty) return;
    send(triggerName: 'delete-conv', fields: {
      'conversation_id': conversationId.trim(),
    });
  }

  Future<void> close() async {
    await _sub?.cancel();
    await _channel?.sink.close();
    _channel = null;
    connection.add(WssConnectionState.disconnected);
  }

  Future<void> dispose() async {
    await close();
    await connection.close();
    await clock.close();
    await counter.close();
    await echo.close();
    await lobby.close();
    await meta.close();
    await status.close();
    await identity.close();
    await conversations.close();
    await activeConversationId.close();
    await conversationMessages.close();
  }

  // ---- Inbound frame parsing ----------------------------------------------

  static final _idRegex =
      RegExp(r'<div\s+id="([^"]+)"\s+hx-swap-oob="[^"]*">([\s\S]*)<\/div>');
  static final _stripTags = RegExp(r'<[^>]+>');
  static final _counterValueRegex =
      RegExp(r'<span class="value">(\d+)<\/span>');
  static final _liRegex = RegExp(r'<li[^>]*class="([^"]*)"[^>]*>([\s\S]*?)<\/li>');
  static final _liNoClassRegex = RegExp(r'<li>([\s\S]*?)<\/li>');
  static final _codeRegex = RegExp(r'<code[^>]*>([^<]+)<\/code>');
  static final _spanRegex = RegExp(r'<span>([^<]*)<\/span>');
  static final _strongRegex = RegExp(r'<strong>([^<]*)<\/strong>');
  static final _smallRegex = RegExp(r'<small>([^<]*)<\/small>');
  static final _dlRegex = RegExp(r'<dt>([^<]+)<\/dt>\s*<dd>([\s\S]*?)<\/dd>');
  static final _hiddenConvIdRegex = RegExp(
      r'<input\s+type="hidden"\s+name="conversation_id"\s+value="([^"]+)"');
  static final _h4ConvId =
      RegExp(r'<h4>conversation\s*<code>([^<]+)<\/code>\s*<\/h4>');
  static final _identityUidRegex =
      RegExp(r'<code class="uid">([^<]+)<\/code>');
  static final _identityDisplayRegex =
      RegExp(r'<span class="display">([^<]+)<\/span>');
  static final _smallMembersRegex = RegExp(r'(\d+)\s+members\s+·\s+(\d+)\s+msgs');

  void _onFrame(dynamic raw) {
    if (raw is! String) return;
    final match = _idRegex.firstMatch(raw);
    if (match == null) return;
    final id = match.group(1)!;
    final body = match.group(2)!;
    switch (id) {
      case 'session-meta':
        meta.add(_parseMeta(body));
      case 'session-clock':
        clock.add(_unescape(body.replaceAll(_stripTags, '').trim()));
      case 'live-counter':
        final m = _counterValueRegex.firstMatch(body);
        if (m != null) counter.add(int.tryParse(m.group(1)!) ?? 0);
      case 'echo-panel':
        echo.add(_parseEcho(body));
      case 'lobby-panel':
        lobby.add(_parseLobby(body));
      case 'session-status':
        status.add(_unescape(body.replaceAll(_stripTags, '').trim()));
      case 'identity-panel':
        identity.add(_parseIdentity(body));
      case 'conv-list-panel':
        conversations.add(_parseConvDirectory(body));
      case 'conv-panel':
        _applyConvPanel(body);
    }
  }

  Map<String, String> _parseMeta(String body) {
    final out = <String, String>{};
    for (final m in _dlRegex.allMatches(body)) {
      final k = _unescape(m.group(1)!.trim());
      final v = _unescape(m.group(2)!.replaceAll(_stripTags, '').trim());
      out[k] = v;
    }
    return out;
  }

  List<String> _parseEcho(String body) {
    final out = <String>[];
    // First: try the class-aware regex (skips muted entries).
    for (final m in _liRegex.allMatches(body)) {
      final cls = m.group(1) ?? '';
      if (cls.contains('muted')) continue;
      out.add(_unescape(m.group(2)!.replaceAll(_stripTags, '').trim()));
    }
    if (out.isEmpty) {
      for (final m in _liNoClassRegex.allMatches(body)) {
        out.add(_unescape(m.group(1)!.replaceAll(_stripTags, '').trim()));
      }
    }
    return out;
  }

  List<LobbyEntry> _parseLobby(String body) {
    final out = <LobbyEntry>[];
    for (final liMatch in _liRegex.allMatches(body)) {
      final cls = liMatch.group(1) ?? '';
      if (cls.contains('muted')) continue;
      final inner = liMatch.group(2)!;
      final from = _codeRegex.firstMatch(inner)?.group(1) ?? '';
      final span = _spanRegex.firstMatch(inner)?.group(1) ?? '';
      out.add(LobbyEntry(
        from: _unescape(from.trim()),
        text: _unescape(span.trim()),
        self: cls.contains('self'),
      ));
    }
    return out;
  }

  Identity _parseIdentity(String body) {
    final uid = _identityUidRegex.firstMatch(body)?.group(1) ?? '';
    final name = _identityDisplayRegex.firstMatch(body)?.group(1) ?? '';
    return Identity(userId: _unescape(uid.trim()), displayName: _unescape(name.trim()));
  }

  Map<String, Map<String, Object?>> _parseConvDirectory(String body) {
    final out = <String, Map<String, Object?>>{};
    for (final m in _liRegex.allMatches(body)) {
      final cls = m.group(1) ?? '';
      final inner = m.group(2)!;
      if (cls.contains('muted')) continue;
      final id = _hiddenConvIdRegex.firstMatch(inner)?.group(1);
      if (id == null) continue;
      final title = _strongRegex.firstMatch(inner)?.group(1) ?? id;
      final small = _smallRegex.firstMatch(inner)?.group(1) ?? '';
      final stats = _smallMembersRegex.firstMatch(small);
      final memberCount = stats != null ? int.tryParse(stats.group(1)!) ?? 0 : 0;
      final messageCount = stats != null ? int.tryParse(stats.group(2)!) ?? 0 : 0;
      out[_unescape(id)] = <String, Object?>{
        'id': _unescape(id),
        'title': _unescape(title.trim()),
        'memberCount': memberCount,
        'messageCount': messageCount,
        'selected': cls.contains('selected'),
      };
    }
    return out;
  }

  void _applyConvPanel(String body) {
    final h4 = _h4ConvId.firstMatch(body);
    if (h4 == null) {
      // Empty / no conversation selected.
      activeConversationId.add('');
      return;
    }
    final convId = _unescape(h4.group(1)!.trim());
    activeConversationId.add(convId);
    final entries = <ConversationEntry>[];
    for (final liMatch in _liRegex.allMatches(body)) {
      final cls = liMatch.group(1) ?? '';
      if (cls.contains('muted')) continue;
      final inner = liMatch.group(2)!;
      final from = _codeRegex.firstMatch(inner)?.group(1) ?? '';
      final span = _spanRegex.firstMatch(inner)?.group(1) ?? '';
      entries.add(ConversationEntry(
        from: _unescape(from.trim()),
        text: _unescape(span.trim()),
        self: cls.contains('self'),
      ));
    }
    final next =
        Map<String, List<ConversationEntry>>.from(conversationMessages.value);
    next[convId] = entries;
    conversationMessages.add(next);
  }

  String _unescape(String s) => s
      .replaceAll('&amp;', '&')
      .replaceAll('&lt;', '<')
      .replaceAll('&gt;', '>')
      .replaceAll('&quot;', '"')
      .replaceAll('&#39;', "'");
}

enum WssConnectionState { disconnected, connecting, connected }

class LobbyEntry {
  const LobbyEntry({required this.from, required this.text, required this.self});
  final String from;
  final String text;
  final bool self;
}

class ConversationEntry {
  const ConversationEntry({
    required this.from,
    required this.text,
    required this.self,
  });
  final String from;
  final String text;
  final bool self;
}

class Identity {
  const Identity({required this.userId, required this.displayName});
  final String userId;
  final String displayName;
}
