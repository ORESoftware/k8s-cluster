/// Registry of conversations / topics / groups. Lives on the main
/// isolate next to [Presence] and [EventBus]. Tracks:
///
///   * `conversationId → ConversationMeta`     (created, kind, title, counts)
///   * `conversationId → Set<userId>`           (members)
///   * `userId → Set<conversationId>`           (reverse index)
///   * `conversationId → recent messages`       (bounded LRU+TTL list,
///                                               backed by [InMemoryCache])
///
/// Messages are *not* part of the conversation row itself — they're held
/// in a separate cache so per-conversation history can have a different
/// TTL/capacity from the conversation metadata. The shipped default is
/// "last 32 messages, 24h TTL", which is plenty for a demo and explicitly
/// not a substitute for durable storage.
///
/// Fanout to actual sockets uses the existing [EventBus]: each
/// conversation owns a topic named `conv:<conversationId>`. The
/// supervisor calls `bus.publish(...)` after registry mutations so all
/// joined sessions get a [BusDelivery].
library;

import 'package:rxdart/rxdart.dart';

import 'in_memory_cache.dart';

class ConversationMeta {
  ConversationMeta({
    required this.id,
    required this.title,
    required this.kind,
    required this.createdAtUs,
    required this.createdByUserId,
    this.lastActivityAtUs = 0,
    this.messageCount = 0,
  });

  final String id;
  String title;
  String kind; // 'chat' | 'broadcast' | 'group' | etc.
  final int createdAtUs;
  final String createdByUserId;
  int lastActivityAtUs;
  int messageCount;

  Map<String, Object?> toJson() => <String, Object?>{
        'id': id,
        'title': title,
        'kind': kind,
        'createdAtUs': createdAtUs,
        'createdByUserId': createdByUserId,
        'lastActivityAtUs': lastActivityAtUs,
        'messageCount': messageCount,
      };
}

class ConversationMessage {
  const ConversationMessage({
    required this.userId,
    required this.text,
    required this.atUs,
  });

  final String userId;
  final String text;
  final int atUs;

  Map<String, Object?> toJson() => <String, Object?>{
        'userId': userId,
        'text': text,
        'atUs': atUs,
      };
}

class ConversationChange {
  const ConversationChange._(
    this.kind,
    this.conversationId, {
    this.userId,
  });

  final String kind;
  final String conversationId;
  final String? userId;

  factory ConversationChange.created(String c, String u) =>
      ConversationChange._('created', c, userId: u);
  factory ConversationChange.deleted(String c) =>
      ConversationChange._('deleted', c);
  factory ConversationChange.userJoined(String c, String u) =>
      ConversationChange._('user_joined', c, userId: u);
  factory ConversationChange.userLeft(String c, String u) =>
      ConversationChange._('user_left', c, userId: u);
  factory ConversationChange.message(String c, String u) =>
      ConversationChange._('message', c, userId: u);

  @override
  String toString() => 'ConversationChange($kind, $conversationId, $userId)';
}

class ConversationRegistry {
  ConversationRegistry({
    int recentCapacityPerConv = 32,
    int maxMembersPerConversation = 10000,
    Duration recentTtl = const Duration(hours: 24),
  }) : _recent = InMemoryCache<List<ConversationMessage>>(
          name: 'conv_recent_messages',
          defaultTtl: recentTtl,
          capacity: 1024, // total distinct conversations cached at once
        ),
        _recentCapacityPerConv = recentCapacityPerConv,
        _maxMembersPerConversation = maxMembersPerConversation;

  final _convs = <String, ConversationMeta>{};
  final _members = <String, Set<String>>{}; // conv → users
  final _byUser = <String, Set<String>>{}; // user → convs

  final InMemoryCache<List<ConversationMessage>> _recent;
  final int _recentCapacityPerConv;

  /// Hard cap on distinct members one conversation will track. The member
  /// set is otherwise unbounded: a single connection can re-`Identify` to
  /// fresh user ids and re-join the same conversation, growing the set (and
  /// the `userId → conversations` reverse index) without limit. 0 disables.
  final int _maxMembersPerConversation;

  final _changes = PublishSubject<ConversationChange>();
  Stream<ConversationChange> get changes => _changes.stream;

  /// Underlying recent-message cache. Exposed so the metrics endpoint can
  /// surface its hit/miss/evict counters.
  InMemoryCache<List<ConversationMessage>> get recentCache => _recent;

  // ---- Read API -----------------------------------------------------------

  int get conversationCount => _convs.length;

  /// Sum of memberships across all conversations.
  int get totalMemberships {
    var n = 0;
    for (final s in _members.values) {
      n += s.length;
    }
    return n;
  }

  ConversationMeta? get(String conversationId) => _convs[conversationId];

  /// Members of [conversationId], or empty when unknown.
  Set<String> members(String conversationId) =>
      _members[conversationId] ?? const <String>{};

  /// Conversations [userId] belongs to.
  Set<String> conversationsForUser(String userId) =>
      _byUser[userId] ?? const <String>{};

  /// Snapshot of every conversation, sorted by `lastActivityAtUs` desc.
  List<ConversationMeta> list() {
    final out = _convs.values.toList(growable: false);
    out.sort((a, b) => b.lastActivityAtUs.compareTo(a.lastActivityAtUs));
    return out;
  }

  /// Recent messages for [conversationId], oldest first.
  List<ConversationMessage> recentMessages(String conversationId) =>
      _recent.get(conversationId) ?? const <ConversationMessage>[];

  /// Bus topic name owned by this conversation. Centralised so callers
  /// don't sprinkle the `conv:` prefix everywhere.
  static String topicFor(String conversationId) => 'conv:$conversationId';

  // ---- Write API ----------------------------------------------------------

  /// Create or refresh a conversation row. Idempotent. Title and kind
  /// updates pass through.
  ConversationMeta upsert({
    required String conversationId,
    required String title,
    required String kind,
    required String createdByUserId,
  }) {
    final existing = _convs[conversationId];
    if (existing != null) {
      existing.title = title.isEmpty ? existing.title : title;
      existing.kind = kind.isEmpty ? existing.kind : kind;
      return existing;
    }
    final now = DateTime.now().microsecondsSinceEpoch;
    final meta = ConversationMeta(
      id: conversationId,
      title: title.isEmpty ? conversationId : title,
      kind: kind.isEmpty ? 'chat' : kind,
      createdAtUs: now,
      createdByUserId: createdByUserId,
      lastActivityAtUs: now,
    );
    _convs[conversationId] = meta;
    _changes.add(ConversationChange.created(conversationId, createdByUserId));
    return meta;
  }

  /// Drop a conversation entirely. Removes membership rows and recent-
  /// messages cache entry. No bus traffic emitted; the supervisor is
  /// expected to broadcast `conv.deleted` on the lobby topic itself.
  void delete(String conversationId) {
    final meta = _convs.remove(conversationId);
    if (meta == null) return;
    final users = _members.remove(conversationId);
    if (users != null) {
      for (final u in users) {
        _byUser[u]?.remove(conversationId);
        if (_byUser[u]?.isEmpty ?? false) _byUser.remove(u);
      }
    }
    _recent.remove(conversationId);
    _changes.add(ConversationChange.deleted(conversationId));
  }

  /// Add [userId] as a member of [conversationId]. Returns true when the
  /// membership was actually new (false when idempotent no-op).
  bool addMember(String conversationId, String userId) {
    final existing = _members[conversationId];
    if (existing != null && existing.contains(userId)) return false;
    // Enforce the per-conversation member cap before creating the set entry,
    // so a refused add doesn't leave an empty set behind.
    if (_maxMembersPerConversation > 0 &&
        (existing?.length ?? 0) >= _maxMembersPerConversation) {
      return false;
    }
    final set = _members.putIfAbsent(conversationId, () => <String>{});
    if (!set.add(userId)) return false;
    _byUser.putIfAbsent(userId, () => <String>{}).add(conversationId);
    final meta = _convs[conversationId];
    if (meta != null) {
      meta.lastActivityAtUs = DateTime.now().microsecondsSinceEpoch;
    }
    _changes.add(ConversationChange.userJoined(conversationId, userId));
    return true;
  }

  /// Remove [userId] from [conversationId]. Returns true when actually
  /// removed (false when not a member).
  bool removeMember(String conversationId, String userId) {
    final set = _members[conversationId];
    if (set == null || !set.remove(userId)) return false;
    if (set.isEmpty) _members.remove(conversationId);
    _byUser[userId]?.remove(conversationId);
    if (_byUser[userId]?.isEmpty ?? false) _byUser.remove(userId);
    _changes.add(ConversationChange.userLeft(conversationId, userId));
    return true;
  }

  /// Drop every membership owned by [userId]. Returns the conversations
  /// they were in (so the caller can emit user-left events). Used on
  /// hard "log out" actions; ordinary disconnect leaves the user as a
  /// member so they don't lose their conversations across reconnects.
  Set<String> dropAllMembershipsFor(String userId) {
    final set = _byUser.remove(userId) ?? const <String>{};
    for (final c in set) {
      final m = _members[c];
      if (m != null) {
        m.remove(userId);
        if (m.isEmpty) _members.remove(c);
      }
      _changes.add(ConversationChange.userLeft(c, userId));
    }
    return set;
  }

  /// Append a chat message and return the resulting recent list.
  /// Truncates to `recentCapacityPerConv` newest entries.
  List<ConversationMessage> appendMessage({
    required String conversationId,
    required String userId,
    required String text,
  }) {
    final now = DateTime.now().microsecondsSinceEpoch;
    final msg = ConversationMessage(userId: userId, text: text, atUs: now);
    final next = _recent.update(
      conversationId,
      (prev) {
        final list = [...(prev ?? const <ConversationMessage>[]), msg];
        if (list.length > _recentCapacityPerConv) {
          return list.sublist(list.length - _recentCapacityPerConv);
        }
        return list;
      },
    );
    final meta = _convs[conversationId];
    if (meta != null) {
      meta.messageCount++;
      meta.lastActivityAtUs = now;
    }
    _changes.add(ConversationChange.message(conversationId, userId));
    return next;
  }

  Future<void> close() async {
    await _changes.close();
    await _recent.close();
    _convs.clear();
    _members.clear();
    _byUser.clear();
  }
}
