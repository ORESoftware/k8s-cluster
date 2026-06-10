/// Presence / identity index. Lives on the main isolate next to the
/// EventBus. Two responsibilities:
///
///   1. Bidirectional `userId ↔ sessionId` mapping. A single user can
///      have many concurrent sessions (multiple browser tabs, the
///      Flutter SPA + the HTMX SSR demo, etc.); a single session is
///      always bound to exactly one user.
///
///   2. Display-name registry: `userId → displayName`. Sessions can
///      `Identify` themselves with a friendlier name; the bus publishes
///      an event so other sessions can re-render.
///
/// Sessions are auto-bound to a synthetic `anon-<sessionId>` user on
/// adopt so every code path can treat presence as always-populated.
library;

import 'package:rxdart/rxdart.dart';

class PresenceChange {
  const PresenceChange._(this.kind, this.sessionId, this.userId, this.displayName);

  final String kind;
  final String sessionId;
  final String userId;
  final String displayName;

  factory PresenceChange.bind(String s, String u, String n) =>
      PresenceChange._('bind', s, u, n);
  factory PresenceChange.rebind(String s, String u, String n) =>
      PresenceChange._('rebind', s, u, n);
  factory PresenceChange.unbind(String s, String u, String n) =>
      PresenceChange._('unbind', s, u, n);
  factory PresenceChange.rename(String u, String n) =>
      PresenceChange._('rename', '', u, n);

  @override
  String toString() => 'PresenceChange($kind, sid=$sessionId, uid=$userId, name=$displayName)';
}

class Presence {
  Presence();

  final _userToSessions = <String, Set<String>>{};
  final _sessionToUser = <String, String>{};
  final _displayNames = <String, String>{};
  final _changes = PublishSubject<PresenceChange>();

  Stream<PresenceChange> get changes => _changes.stream;

  /// Number of distinct logged-in users (i.e. with at least one session).
  int get userCount => _userToSessions.length;

  /// Number of session-bindings (typically equal to live session count).
  int get sessionCount => _sessionToUser.length;

  /// Lookup: which user owns this session?
  String? userIdFor(String sessionId) => _sessionToUser[sessionId];

  /// Lookup: all sessions owned by this user. Returns empty set when the
  /// user is offline.
  Set<String> sessionsFor(String userId) =>
      _userToSessions[userId] ?? const <String>{};

  /// Lookup: display name for this user, falling back to the userId.
  String displayNameFor(String userId) =>
      _displayNames[userId] ?? userId;

  /// Snapshot (for /metrics + admin views) of `userId → [displayName, sessionCount]`.
  Map<String, ({String displayName, int sessionCount})> snapshot() {
    final out = <String, ({String displayName, int sessionCount})>{};
    for (final entry in _userToSessions.entries) {
      out[entry.key] = (
        displayName: _displayNames[entry.key] ?? entry.key,
        sessionCount: entry.value.length,
      );
    }
    return out;
  }

  /// Bind a brand-new session to a user. Idempotent: re-binding the same
  /// (session, user) pair is a no-op.
  void bind(String sessionId, String userId, {String displayName = ''}) {
    final existing = _sessionToUser[sessionId];
    if (existing == userId) {
      // Already bound; just refresh display-name when one is supplied.
      if (displayName.isNotEmpty) {
        _displayNames[userId] = displayName;
        _changes.add(PresenceChange.rename(userId, displayName));
      }
      return;
    }
    if (existing != null) {
      // Session is being moved from another user (rare; happens when a
      // session calls Identify with a new userId after a prior bind).
      _userToSessions[existing]?.remove(sessionId);
      if (_userToSessions[existing]?.isEmpty ?? false) {
        _userToSessions.remove(existing);
      }
    }
    _sessionToUser[sessionId] = userId;
    _userToSessions.putIfAbsent(userId, () => <String>{}).add(sessionId);
    if (displayName.isNotEmpty) _displayNames[userId] = displayName;

    if (existing != null) {
      _changes.add(PresenceChange.rebind(sessionId, userId, displayNameFor(userId)));
    } else {
      _changes.add(PresenceChange.bind(sessionId, userId, displayNameFor(userId)));
    }
  }

  /// Unbind a session. Returns the userId it was attached to (so the
  /// caller can decide whether to fire "user went offline" events).
  String? unbind(String sessionId) {
    final user = _sessionToUser.remove(sessionId);
    if (user == null) return null;
    final name = displayNameFor(user);
    final set = _userToSessions[user];
    if (set != null) {
      set.remove(sessionId);
      if (set.isEmpty) {
        _userToSessions.remove(user);
        // Release the display-name entry once the user has no live
        // sessions. Otherwise `_displayNames` grows by one entry per
        // connection for the shard's whole lifetime: every session is
        // auto-bound to a distinct `anon-<sessionId>` user on adopt (which
        // records a name) and never reconnects, so the map leaks ~one entry
        // per WebSocket ever accepted. An identified user that reconnects
        // re-publishes their name on the next Identify, so dropping it here
        // only means an *offline* user renders as their raw id (which
        // `displayNameFor` already falls back to) until they return.
        _displayNames.remove(user);
      }
    }
    _changes.add(PresenceChange.unbind(sessionId, user, name));
    return user;
  }

  /// Rename a user. Pure metadata; doesn't move bindings.
  void rename(String userId, String displayName) {
    _displayNames[userId] = displayName;
    _changes.add(PresenceChange.rename(userId, displayName));
  }

  /// True iff [userId] has any live session right now.
  bool isOnline(String userId) => (_userToSessions[userId] ?? const {}).isNotEmpty;

  Future<void> close() async {
    await _changes.close();
  }
}
