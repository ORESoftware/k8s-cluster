/// Tiny in-memory cache primitive used by the conversation registry,
/// presence index, and any per-isolate hot-path that needs an LRU + TTL
/// store. Pure Dart, no dependencies, single-isolate (the main isolate).
///
/// Two eviction axes:
///
///   * **TTL** — entries become stale after [defaultTtl] (per-entry
///     overridable in [put]). Stale entries are returned as `null` from
///     [get] and removed lazily on access plus actively by a periodic
///     sweep timer.
///
///   * **Capacity** — when [capacity] is set and the cache exceeds it on
///     insert, the least-recently-used entry is evicted. LRU order is
///     maintained as a `LinkedHashMap` insertion-order list, with [get]
///     bumping the touched key to the tail.
///
/// The cache emits an observable `Stream<CacheEvent>` so tests and the
/// metrics endpoint can observe hit/miss/evict churn without poking
/// internals.
library;

import 'dart:async';
import 'dart:collection';

import 'package:rxdart/rxdart.dart';

class CacheEvent {
  const CacheEvent._(this.kind, this.key);

  final String kind;
  final String key;

  factory CacheEvent.hit(String k) => CacheEvent._('hit', k);
  factory CacheEvent.miss(String k) => CacheEvent._('miss', k);
  factory CacheEvent.put(String k) => CacheEvent._('put', k);
  factory CacheEvent.evict(String k) => CacheEvent._('evict', k);
  factory CacheEvent.expire(String k) => CacheEvent._('expire', k);

  @override
  String toString() => 'CacheEvent($kind, $key)';
}

class _Entry<V> {
  _Entry(this.value, this.expiresAtUs);
  V value;
  int expiresAtUs;
}

class InMemoryCache<V> {
  InMemoryCache({
    this.defaultTtl = const Duration(minutes: 30),
    this.capacity,
    this.sweepInterval = const Duration(seconds: 30),
    this.name = 'cache',
  }) {
    if (sweepInterval.inMilliseconds > 0) {
      _sweepTimer = Timer.periodic(sweepInterval, (_) => _sweep());
    }
  }

  /// Default TTL applied to entries that don't override on put.
  final Duration defaultTtl;

  /// Optional bound on the number of live entries. Null disables LRU
  /// eviction (TTL still applies).
  final int? capacity;

  /// How often to actively evict expired entries. Set to `Duration.zero`
  /// to disable the sweeper (lazy eviction on access still applies).
  final Duration sweepInterval;

  /// Human label, used in CacheEvent and metrics.
  final String name;

  final _entries = LinkedHashMap<String, _Entry<V>>();
  final _events = PublishSubject<CacheEvent>();
  Timer? _sweepTimer;

  // Stats: rolling counters since startup.
  int _hits = 0;
  int _misses = 0;
  int _evicts = 0;
  int _expires = 0;

  Stream<CacheEvent> get events => _events.stream;
  int get size => _entries.length;
  int get hits => _hits;
  int get misses => _misses;
  int get evicts => _evicts;
  int get expires => _expires;

  /// Get a key. Returns null when missing or expired.
  V? get(String key) {
    final e = _entries[key];
    if (e == null) {
      _misses++;
      _events.add(CacheEvent.miss(key));
      return null;
    }
    final nowUs = DateTime.now().microsecondsSinceEpoch;
    if (e.expiresAtUs <= nowUs) {
      _entries.remove(key);
      _expires++;
      _events.add(CacheEvent.expire(key));
      return null;
    }
    // LRU bump.
    _entries.remove(key);
    _entries[key] = e;
    _hits++;
    _events.add(CacheEvent.hit(key));
    return e.value;
  }

  /// Insert / overwrite. Returns the previous value if any (without
  /// counting as a hit/miss).
  V? put(String key, V value, {Duration? ttl}) {
    final ttlUs = (ttl ?? defaultTtl).inMicroseconds;
    final expiresAtUs = DateTime.now().microsecondsSinceEpoch + ttlUs;
    final prev = _entries.remove(key);
    _entries[key] = _Entry<V>(value, expiresAtUs);
    _events.add(CacheEvent.put(key));

    final cap = capacity;
    if (cap != null && _entries.length > cap) {
      final oldest = _entries.keys.first;
      _entries.remove(oldest);
      _evicts++;
      _events.add(CacheEvent.evict(oldest));
    }
    return prev?.value;
  }

  /// Update an existing entry's value via [updater] without changing TTL.
  /// If the key is absent, [updater] is called with `null` and the result
  /// is inserted using [defaultTtl]. Useful for atomic list-append.
  V update(String key, V Function(V? prev) updater, {Duration? ttl}) {
    final prev = _entries[key]?.value;
    final next = updater(prev);
    put(key, next, ttl: ttl);
    return next;
  }

  /// Drop a key. Returns the previous value if any.
  V? remove(String key) {
    final prev = _entries.remove(key);
    if (prev != null) {
      _evicts++;
      _events.add(CacheEvent.evict(key));
    }
    return prev?.value;
  }

  /// Snapshot of currently-live keys (post-sweep). Stable order = LRU,
  /// oldest first.
  List<String> keys() {
    _sweep();
    return _entries.keys.toList(growable: false);
  }

  /// Snapshot of currently-live values, oldest first.
  List<V> values() {
    _sweep();
    return _entries.values.map((e) => e.value).toList(growable: false);
  }

  void clear() {
    final n = _entries.length;
    _entries.clear();
    _evicts += n;
  }

  Future<void> close() async {
    _sweepTimer?.cancel();
    _sweepTimer = null;
    await _events.close();
    _entries.clear();
  }

  void _sweep() {
    final nowUs = DateTime.now().microsecondsSinceEpoch;
    final dead = <String>[];
    for (final entry in _entries.entries) {
      if (entry.value.expiresAtUs <= nowUs) dead.add(entry.key);
    }
    for (final k in dead) {
      _entries.remove(k);
      _expires++;
      _events.add(CacheEvent.expire(k));
    }
  }
}
