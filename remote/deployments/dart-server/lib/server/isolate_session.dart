/// Session runtime + host isolate entry point.
///
/// The shape used to be deliberately Phoenix-ish: one [Isolate] per
/// connected WebSocket peer. That gave perfect crash isolation but cost
/// ~290 KB per session in heap/event-loop overhead and capped a single
/// pod at ~3K connections before main-isolate spawn churn saturated the
/// liveness probe (see the 20K loadtest writeup).
///
/// New shape: a small pool of "session-host" isolates, each running N
/// sessions side-by-side as plain Dart objects inside one event loop. N
/// is configurable (`SESSIONS_PER_HOST`, default 100, range 1..2000).
/// Each session still owns its private RxDart graph, mutable state
/// record, and outbound SendPort — only the *isolate* boundary is now
/// shared across N peers.
///
/// `Session` (the class) is intentionally isolate-agnostic: instantiate
/// it on any isolate, feed inbound events into [Session.deliver], and
/// it pushes outbound frames through the SendPort it was constructed
/// with. The host loop in [hostIsolateEntry] just multiplexes incoming
/// `AttachSession` / `RouteToSession` / `DetachSession` messages into
/// the right `Session` instance.
///
/// HTML for HTMX OOB swaps is produced by **Jaspr components** (see
/// `wss_components.dart`) — never by string concatenation.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io' show Platform;
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

/// Hard caps on the number of rows a session retains in its per-session
/// lobby / echo buffers. The render pipelines only ever show the last
/// 16 / 8 rows, but the *stored* `BehaviorSubject` list was append-only —
/// so a long-lived session on a busy lobby grew its in-memory buffer without
/// bound (a slow per-session leak). We trim at store time to a small multiple
/// of the render window; output is unchanged, memory is bounded.
const int _maxLobbyRows = 64;
const int _maxHistoryRows = 64;

/// Trim [rows] to at most [max] newest entries (keeps the tail).
List<T> _trimTail<T>(List<T> rows, int max) =>
    rows.length <= max ? rows : rows.sublist(rows.length - max);

// ---------------------------------------------------------------------------
// Shared clock-render cache (perf A/B: WS_CLOCK_SHARED_RENDER).
// ---------------------------------------------------------------------------
//
// The 1 Hz `Clock` fragment is the only steady-state emitter, and its HTML is
// a pure function of the (second-granularity) UTC timestamp — identical for
// every session. The original code re-ran a full Jaspr render per session per
// tick, which is the dominant idle-CPU cost at scale (≈one render × live
// sessions every `clockIntervalSeconds`). This module-level cache, shared by
// every session on a host isolate, collapses that to ONE render per distinct
// second across the whole host: the first session to want a given second
// kicks off the render Future and all others await the same Future. Gated by
// `WS_CLOCK_SHARED_RENDER` (default on) so the per-session-render arm can be
// A/B'd; `dart_clock_renders_total` vs `dart_clock_render_cache_hits_total`
// quantify the win.

/// Whether sessions share the cached clock render. Read once per host isolate.
final bool _clockSharedRender =
    (Platform.environment['WS_CLOCK_SHARED_RENDER']?.toLowerCase().trim() ??
            'true') !=
        'false';

/// iso-second → in-flight/settled render. Bounded to a few entries: across
/// phase-spread tickers at most ~2 distinct seconds are live at any boundary.
final _clockRenderCache = <String, Future<String>>{};

void _clockCachePut(String iso, Future<String> html) {
  if (_clockRenderCache.length >= 4) {
    _clockRenderCache.remove(_clockRenderCache.keys.first);
  }
  _clockRenderCache[iso] = html;
}

/// UTC ISO-8601 timestamp truncated to whole seconds. Stable within a
/// 1-second window so every session ticking in the same second produces the
/// same cache key (`toIso8601String()` would otherwise include milliseconds
/// and defeat the cache). Pure + top-level so it can be unit-tested.
String clockIsoForSecond(DateTime now) {
  final u = now.toUtc();
  return DateTime.utc(u.year, u.month, u.day, u.hour, u.minute, u.second)
      .toIso8601String();
}

/// Hard length caps on client-supplied text fields accepted from an HTMX
/// trigger. Inbound frames are already byte-capped (`WS_MAX_INBOUND_BYTES`,
/// 64 KiB by default) but that limit is per-frame: without a per-field cap a
/// single 64 KiB display name / conversation id / chat line would still be
/// (a) stored verbatim as a map key/value in the shard's Presence /
/// ConversationRegistry / EventBus state, and (b) fanned out to *every*
/// joined session on the shard — a cheap amplification vector. We truncate
/// rather than reject so the demo stays forgiving; identifiers get a short
/// cap, free-text chat a larger one.
const int _maxIdentLen = 128;
const int _maxTextLen = 2000;

/// Truncate [s] to at most [max] code units. Trim first at the call site.
String _cap(String s, int max) => s.length <= max ? s : s.substring(0, max);

/// Entry point for a session-host isolate. Receives a one-time handshake
/// SendPort, replies with its own mailbox, then loops forever multiplexing
/// `AttachSession` / `RouteToSession` / `DetachSession` frames coming in
/// from the supervisor on the main isolate.
///
/// Errors raised inside one session are swallowed here so a panic in
/// session A cannot kill sessions B..N sharing the same host. Spawn the
/// host with `errorsAreFatal: false`; the supervisor still watches the
/// error / exit ports and tears down all attached sessions on a hard
/// host-level failure.
Future<void> hostIsolateEntry(SendPort handshakePort) async {
  // Crash-resilience guard. Any asynchronous error that escapes a
  // session's own try/catch — a Jaspr render throwing inside a detached
  // RxDart pipeline, an error in the 1 Hz Timer callback, a dropped
  // Future — lands in this zone handler instead of propagating to the VM.
  // Without it a single bad render in ONE session would kill the whole
  // host isolate and drop every session sharing it (up to
  // `sessionsPerHost`). We log a rate-limited line and keep the host
  // serving its other sessions. Genuine isolate termination (OOM,
  // explicit kill) is still observed by the supervisor's exit port, which
  // tears the host's sessions down cleanly.
  await runZonedGuarded(() async {
    try {
      await _runHost(handshakePort);
    } catch (e, st) {
      _logHostUncaught(e, st);
    }
  }, _logHostUncaught);
}

Future<void> _runHost(SendPort handshakePort) async {
  ensureJasprInit();

  final mailbox = ReceivePort('dd-dart-host-mailbox');
  handshakePort.send(mailbox.sendPort);

  final sessions = <String, Session>{};

  await for (final raw in mailbox) {
    try {
      if (raw is AttachSession) {
        final session = Session(raw.boot);
        sessions[raw.boot.sessionId] = session;
        unawaited(() async {
          try {
            await session.run();
          } catch (_) {
            // Session-level error — already logged via the outbound
            // MetricEvent path inside Session. Swallow to keep the host
            // alive for sibling sessions.
          } finally {
            sessions.remove(raw.boot.sessionId);
          }
        }());
      } else if (raw is RouteToSession) {
        sessions[raw.sessionId]?.deliver(raw.event);
      } else if (raw is DetachSession) {
        final s = sessions.remove(raw.sessionId);
        s?.requestShutdown();
      } else if (raw == _hostShutdownSentinel) {
        for (final s in sessions.values) {
          s.requestShutdown();
        }
        sessions.clear();
        mailbox.close();
        return;
      }
    } catch (_) {
      // Defensive: never let a malformed frame from main kill the host.
    }
  }
}

int _hostUncaughtErrors = 0;
int _hostUncaughtLoggedAtMs = 0;

/// Zone handler for a session-host isolate. Swallows the error (keeping
/// the host alive for its other sessions) and logs at most ~once per
/// second so a render-error storm can't flood stdout / Loki. The running
/// total is included so operators can still see the true error volume.
void _logHostUncaught(Object error, StackTrace stack) {
  _hostUncaughtErrors++;
  final now = DateTime.now().millisecondsSinceEpoch;
  if (now - _hostUncaughtLoggedAtMs < 1000) return;
  _hostUncaughtLoggedAtMs = now;
  // ignore: avoid_print
  print(jsonEncode({
    'event': 'session_host_uncaught_error',
    'total': _hostUncaughtErrors,
    'error': '$error',
    'stack': stack.toString().split('\n').take(3).join(' | '),
  }));
}

/// Sentinel the supervisor can send to a host mailbox to ask it to drain
/// all sessions and exit cleanly.
const String _hostShutdownSentinel = '__host_shutdown__';

/// Asks a host mailbox to gracefully shut down (drain sessions, return).
void requestHostShutdown(SendPort hostMailbox) {
  hostMailbox.send(_hostShutdownSentinel);
}

/// One connected WebSocket peer's worth of state and behaviour. Lives
/// entirely on whichever isolate constructed it; communication with the
/// outside world goes through:
///
///   * [deliver] — push an [InboundEvent] (WS frame OR bus delivery)
///     onto the session's private inbox stream.
///   * `boot.outbound` — a SendPort owned by the supervisor; the session
///     writes [OutboundFrame]s here and the supervisor dispatches them
///     to the WebSocket / metrics / EventBus / presence index.
class Session {
  Session(this._boot);

  final SessionBootMessage _boot;
  late final SendPort _outbound = _boot.outbound as SendPort;

  /// Per-session inbox. Filled by the host's mailbox loop in response
  /// to `RouteToSession` frames; drained by [run].
  final _inbox = StreamController<dynamic>(sync: false);

  /// Microseconds-since-epoch at the last inbound WS frame from the
  /// peer (text or binary). Bus deliveries and server-emitted clock
  /// frames do NOT update this. Used by the per-tick idle check to
  /// gracefully close sessions that have been silent for longer than
  /// `_boot.idleTimeoutSeconds`.
  int _lastInboundUs = DateTime.now().microsecondsSinceEpoch;
  bool _idleClosing = false;

  final _inboundHtmx = PublishSubject<HtmxInbound>();
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
  bool _disposed = false;

  /// Push an inbound event (WS frame or bus delivery) into the session.
  /// Safe to call from the host's mailbox loop. Drops cleanly after
  /// `_dispose()` has run.
  void deliver(dynamic event) {
    if (_inbox.isClosed) return;
    if (event is InboundText || event is InboundBinary) {
      _lastInboundUs = DateTime.now().microsecondsSinceEpoch;
    }
    _inbox.add(event);
  }

  /// Ask the session to drain and exit. The mailbox loop in [run] sees
  /// the [_shutdownSentinel] and breaks. Idempotent.
  void requestShutdown() {
    if (_inbox.isClosed) return;
    _inbox.add(_shutdownSentinel);
  }

  Future<void> run() async {
    _wirePipelines();
    _joinTopics();
    await _emitGreeting();

    await for (final raw in _inbox.stream) {
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
          .listen(_emitText, onError: _onRenderError),
    );

    _subs.add(
      _history
          .map((rows) =>
              rows.length <= 8 ? rows : rows.sublist(rows.length - 8))
          .map((rows) => EchoPanel(rows))
          .asyncMap(renderFragment)
          .listen(_emitText, onError: _onRenderError),
    );

    _subs.add(
      _lobby
          .map((rows) =>
              rows.length <= 16 ? rows : rows.sublist(rows.length - 16))
          .map((rows) => LobbyPanel(rows))
          .asyncMap(renderFragment)
          .listen(_emitText, onError: _onRenderError),
    );

    // Identity pill re-renders any time our own identity changes.
    _subs.add(
      _identity
          .distinct()
          .map((id) =>
              IdentityPanel(userId: id.userId, displayName: id.displayName))
          .asyncMap(renderFragment)
          .listen(_emitText, onError: _onRenderError),
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
      ).asyncMap(renderFragment).listen(_emitText, onError: _onRenderError),
    );

    _subs.add(
      Rx.combineLatest2<String, Map<String, List<ConvMessage>>, ConvPanel>(
        _activeConv,
        _convMessages,
        (active, msgs) => ConvPanel(
          activeId: active,
          messages: msgs[active] ?? const <ConvMessage>[],
        ),
      ).asyncMap(renderFragment).listen(_emitText, onError: _onRenderError),
    );

    // HTMX inbound → state. A throw in the trigger handler is contained to
    // this session (counted + dropped) rather than escaping to the host.
    _subs.add(_inboundHtmx.listen(
      (msg) {
        try {
          _handleHtmxTrigger(msg);
        } catch (_) {
          _send(const MetricEvent('dart_session_render_errors_total'));
        }
      },
      onError: _onRenderError,
    ));

    // Bus inbound → state mutations. Same per-session containment.
    _subs.add(_busInbound.listen(
      (delivery) {
        try {
          _handleBusDelivery(delivery);
        } catch (_) {
          _send(const MetricEvent('dart_session_render_errors_total'));
        }
      },
      onError: _onRenderError,
    ));

    // Server-driven 1Hz tick. The idle-disconnect check fires on
    // every tick (it's cheap — just two int subtractions + a metric
    // emit on the rare timeout path). The Clock OOB fragment fires
    // every `clockIntervalSeconds` ticks so a benchmark profile can
    // dial the per-session jaspr render rate way down without losing
    // the lifecycle gates.
    var tickCount = 0;
    _ticker = Timer.periodic(const Duration(seconds: 1), (_) {
      // A throw here would otherwise be an uncaught error on the host's
      // event loop (no surrounding await) — contain it to this session.
      try {
        tickCount++;
        final clockInterval = _boot.clockIntervalSeconds;
        if (clockInterval > 0 && tickCount % clockInterval == 0) {
          unawaited(_emitClock());
        }
        _checkIdle();
      } catch (_) {
        _send(const MetricEvent('dart_session_tick_errors_total'));
      }
    });
  }

  /// `onError` for every per-session render pipeline. A render/pipeline
  /// failure for one widget must never tear down the session — let alone
  /// the host isolate it shares with up to `sessionsPerHost` peers. Count
  /// it and drop the frame; the next state change re-renders.
  void _onRenderError(Object error, StackTrace stack) {
    _send(const MetricEvent('dart_session_render_errors_total'));
  }

  /// Per-tick lifecycle gate. Two independent reasons can decide to
  /// gracefully close the session:
  ///
  ///   1. **Idle timeout** (`idleTimeoutSeconds`) — peer has gone fully
  ///      silent for too long. Fires `4001 idle_timeout_<N>s`.
  ///
  ///   2. **Age-based eviction** (`maxAgeSeconds` + `ageBasedIdleSeconds`) —
  ///      session is older than `maxAgeSeconds` AND peer has been idle
  ///      at least `ageBasedIdleSeconds`. Fires `4003 session_aged`.
  ///      Lets a long-running session-host isolate retire its slots
  ///      naturally instead of being kept alive forever by a single
  ///      chatty client.
  ///
  /// Idempotent: the `_idleClosing` flag prevents repeat closes once
  /// either trigger fires.
  void _checkIdle() {
    if (_idleClosing) return;
    final nowUs = DateTime.now().microsecondsSinceEpoch;
    final idleUs = nowUs - _lastInboundUs;

    // (1) Hard idle timeout.
    final timeoutSec = _boot.idleTimeoutSeconds;
    if (timeoutSec > 0 && idleUs >= timeoutSec * 1000000) {
      _idleClosing = true;
      _send(const MetricEvent('dart_session_idle_timeout_total'));
      _send(OutboundClose(
        code: 4001,
        reason: 'idle_timeout_${timeoutSec}s',
      ));
      requestShutdown();
      return;
    }

    // (2) Age-based eviction.
    final maxAgeSec = _boot.maxAgeSeconds;
    final ageIdleSec = _boot.ageBasedIdleSeconds;
    if (maxAgeSec > 0 && ageIdleSec >= 0) {
      final ageUs = nowUs - _boot.spawnedAtUs;
      if (ageUs >= maxAgeSec * 1000000 &&
          idleUs >= ageIdleSec * 1000000) {
        _idleClosing = true;
        _send(const MetricEvent('dart_session_aged_out_total'));
        _send(OutboundClose(
          code: 4003,
          reason: 'session_aged_${maxAgeSec}s',
        ));
        requestShutdown();
        return;
      }
    }
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
    // Benchmark fast-path: clients that follow the akka pipeline shape
    // ({"id":"...","payload":"..."}) — what `ws-loadtest-rs LOAD_MODE=
    // pipeline` and `dd-rust-wss-server` already speak — get an
    // immediate `{ok:true,result:{id}}` reply without parsing JSON,
    // routing through HTMX, or rendering Jaspr fragments. Lets the
    // existing pipeline loader measure RTT against the Dart server in
    // the same way it measures it against rust-wss-server, so the
    // head-to-head Dart/Gleam/Rust comparison is apples-to-apples.
    if (_handleBenchmarkPing(text)) return;

    final parsed = parseHtmxInboundJson(text);
    if (parsed == null) {
      unawaited(_emitFragment(const StatusPill('non-json frame ignored')));
      return;
    }
    _inboundHtmx.add(parsed);
  }

  /// Cheap substring scan: if [text] looks like a pipeline-mode frame
  /// ({"id":"..."} and not HTMX), reply with the akka envelope and
  /// return true. No `jsonDecode` on the hot path.
  bool _handleBenchmarkPing(String text) {
    if (text.contains('"HEADERS"')) return false;
    final id = _extractStringField(text, 'id');
    if (id == null) return false;
    final ts = DateTime.now().millisecondsSinceEpoch;
    _emitText(
      '{"ok":true,"result":{"id":"${_jsonEscape(id)}"},"ts":$ts}',
    );
    return true;
  }

  static String? _extractStringField(String text, String key) {
    final needle = '"$key"';
    final keyPos = text.indexOf(needle);
    if (keyPos < 0) return null;
    var i = keyPos + needle.length;
    while (i < text.length) {
      final c = text.codeUnitAt(i);
      if (c == 0x20 || c == 0x09) {
        i++;
        continue;
      }
      break;
    }
    if (i >= text.length || text.codeUnitAt(i) != 0x3a) return null;
    i++;
    while (i < text.length) {
      final c = text.codeUnitAt(i);
      if (c == 0x20 || c == 0x09) {
        i++;
        continue;
      }
      break;
    }
    if (i >= text.length || text.codeUnitAt(i) != 0x22) return null;
    i++;
    final start = i;
    while (i < text.length && text.codeUnitAt(i) != 0x22) {
      i++;
    }
    if (i >= text.length) return null;
    return text.substring(start, i);
  }

  static String _jsonEscape(String s) =>
      s.replaceAll('\\', '\\\\').replaceAll('"', '\\"');

  void _send(OutboundFrame frame) => _outbound.send(frame);
  void _emitText(String html) => _outbound.send(OutboundText(html));

  /// Async helper used for one-shot fragment emissions that aren't
  /// driven by a long-lived RxDart pipeline (status pills, the
  /// initial greeting, the 1Hz clock, etc.).
  Future<void> _emitFragment(Component component) async {
    // Used in many `unawaited(...)` contexts (greeting, clock, status
    // pills). A Jaspr render throwing here would become an uncaught async
    // error on the host loop, so swallow + count instead.
    try {
      final html = await renderFragment(component);
      _emitText(html);
    } catch (_) {
      _send(const MetricEvent('dart_session_render_errors_total'));
    }
  }

  /// Emit the 1 Hz clock fragment, sharing one Jaspr render per second across
  /// every session on this host when `WS_CLOCK_SHARED_RENDER` is on (the
  /// default). See the cache notes at the top of this file. Errors are
  /// swallowed + counted exactly like [_emitFragment] so a render failure
  /// drops one clock frame instead of escaping to the host loop.
  Future<void> _emitClock() async {
    try {
      final iso = clockIsoForSecond(DateTime.now());
      final String html;
      if (_clockSharedRender) {
        final pending = _clockRenderCache[iso];
        if (pending != null) {
          _send(const MetricEvent('dart_clock_render_cache_hits_total'));
          html = await pending;
        } else {
          final fut = renderFragment(Clock(iso));
          _clockCachePut(iso, fut);
          _send(const MetricEvent('dart_clock_renders_total'));
          html = await fut;
        }
      } else {
        _send(const MetricEvent('dart_clock_renders_total'));
        html = await renderFragment(Clock(iso));
      }
      _emitText(html);
    } catch (_) {
      _send(const MetricEvent('dart_session_render_errors_total'));
    }
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
        final text = _cap(msg.stringField('message').trim(), _maxTextLen);
        if (text.isEmpty) return;
        _history.add(_trimTail([..._history.value, text], _maxHistoryRows));
        _send(const MetricEvent('dart_session_echoes_total'));
      case 'say':
        final text = _cap(msg.stringField('text').trim(), _maxTextLen);
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
        final userId = _cap(msg.stringField('user_id').trim(), _maxIdentLen);
        final displayName =
            _cap(msg.stringField('display_name').trim(), _maxIdentLen);
        if (userId.isEmpty) {
          unawaited(_emitFragment(
            const StatusPill('user_id required to identify'),
          ));
          return;
        }
        _identity.add((userId: userId, displayName: displayName));
        _send(Identify(userId: userId, displayName: displayName));
      case 'open-conv':
        final convId =
            _cap(msg.stringField('conversation_id').trim(), _maxIdentLen);
        final title = _cap(msg.stringField('title').trim(), _maxIdentLen);
        if (convId.isEmpty) return;
        _send(ConversationOpen(
          conversationId: convId,
          title: title,
          kind: _cap(msg.stringField('kind', 'chat'), _maxIdentLen),
        ));
      case 'join-conv':
        final convId =
            _cap(msg.stringField('conversation_id').trim(), _maxIdentLen);
        if (convId.isEmpty) return;
        _activeConv.add(convId);
        _send(ConversationJoin(convId));
      case 'leave-conv':
        final convId =
            _cap(msg.stringField('conversation_id').trim(), _maxIdentLen);
        if (convId.isEmpty) return;
        if (_activeConv.value == convId) _activeConv.add('');
        _send(ConversationLeave(
          convId,
          dropMembership: msg.stringField('drop') == '1',
        ));
      case 'say-conv':
        final convId =
            _cap(msg.stringField('conversation_id').trim(), _maxIdentLen);
        final text = _cap(msg.stringField('text').trim(), _maxTextLen);
        if (convId.isEmpty || text.isEmpty) return;
        _send(ConversationSay(conversationId: convId, text: text));
      case 'switch-conv':
        // Sets which conversation the local panel renders. No supervisor
        // round-trip needed; the bus deliveries already populate
        // `_convMessages` for any topic this session is bus-joined to.
        _activeConv.add(
            _cap(msg.stringField('conversation_id').trim(), _maxIdentLen));
      case 'delete-conv':
        final convId =
            _cap(msg.stringField('conversation_id').trim(), _maxIdentLen);
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
      _lobby.add(_trimTail([
        ..._lobby.value,
        LobbyRow(
          name: (delivery.data['displayName'] as String?) ??
              (delivery.data['from'] as String?) ??
              delivery.fromSessionId,
          text: text,
          self: delivery.fromSessionId == _boot.sessionId,
        ),
      ], _maxLobbyRows));
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
    if (_disposed) return;
    _disposed = true;
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
    await _inboundHtmx.close();
    await _busInbound.close();
    await _counter.close();
    await _history.close();
    await _lobby.close();
    await _identity.close();
    await _activeConv.close();
    await _convMessages.close();
    await _convDirectory.close();
    if (!_inbox.isClosed) await _inbox.close();
    _send(const MetricEvent('dart_sessions_closed_total'));
  }
}
