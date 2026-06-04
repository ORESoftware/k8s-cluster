/// Server-side hot reload.
///
/// Yes — Dart server processes can hot-load new code while running, without
/// dropping in-flight WebSocket connections, RxDart subscriptions, the
/// EventBus, the conversation cache, or any other in-memory state. This is
/// the same mechanism Flutter uses for hot reload, exposed via the [VM
/// Service Protocol](https://github.com/dart-lang/sdk/blob/main/runtime/vm/service/service.md).
///
/// What gets reloaded
/// ------------------
///
/// We call `reloadSources(isolateId)` for **every** isolate in the process:
///
///   * The main isolate picks up new HTTP routing logic, new metrics
///     gauges, new EventBus / Presence / ConversationRegistry method
///     bodies, new Jaspr render functions.
///   * Each session isolate picks up new render functions, new HTMX
///     trigger handlers, new RxDart pipeline shapes.
///
/// In-flight WebSockets are **not** dropped. The next time a session's
/// pipeline emits a frame (RxDart subject mutation → render fn → outbound
/// SendPort → WS), it goes through the **new** render code. Mid-flight
/// async chains finish with their old code; the new code takes over at
/// the next event-loop turn.
///
/// What's preserved across a reload
/// --------------------------------
///
///   * `BehaviorSubject` values + subscription topology (per session)
///   * Counter values, echo history, lobby chat, conversation membership
///   * EventBus topic membership maps, Presence index, ConversationRegistry
///     metadata + recent-messages cache
///   * Open WebSockets (peers don't see anything; they just get the next
///     frame faster)
///
/// What CANNOT be hot-reloaded
/// ---------------------------
///
///   * AOT binaries (`dart compile exe`) — there's no JIT in the runtime.
///     The deployment ships the AOT path for prod and the JIT path for
///     dev/staging behind `HOT_RELOAD=true`.
///   * Class shape changes: adding/removing fields, changing supertype,
///     changing const-ness. The VM rejects these reloads and you'll see
///     `success: false` in the result. Restart the process.
///   * Static initialisers / top-level finals: their values are kept;
///     re-evaluating requires restart.
///   * Generic type-parameter changes on a live class.
///
/// Method body changes, new methods on existing classes, new top-level
/// functions, new files entirely, and new closures all reload cleanly.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:developer' as dev;
import 'dart:io';

import 'package:rxdart/rxdart.dart';
import 'package:vm_service/utils.dart' as vm_utils;
import 'package:vm_service/vm_service.dart';
import 'package:vm_service/vm_service_io.dart';
import 'package:watcher/watcher.dart';

import 'metrics.dart';

class HotReloadResult {
  HotReloadResult({
    required this.success,
    required this.isolatesReloaded,
    required this.isolatesFailed,
    required this.durationMs,
    required this.atUs,
    required this.message,
    required this.byIsolate,
  });

  final bool success;
  final int isolatesReloaded;
  final int isolatesFailed;
  final int durationMs;
  final int atUs;
  final String message;

  /// `isolateName → either `"ok"` or the failure reason`.
  final Map<String, String> byIsolate;

  Map<String, Object?> toJson() => <String, Object?>{
        'success': success,
        'isolatesReloaded': isolatesReloaded,
        'isolatesFailed': isolatesFailed,
        'durationMs': durationMs,
        'atUs': atUs,
        'message': message,
        'byIsolate': byIsolate,
      };
}

class HotReloader {
  HotReloader({
    required this.metrics,
    required this.watchPaths,
    this.debounce = const Duration(milliseconds: 300),
    this.pollInterval = const Duration(milliseconds: 250),
  });

  final Metrics metrics;

  /// Directories to watch (recursively) for `.dart` changes.
  final List<String> watchPaths;

  /// Coalesce bursts of changes into a single reload.
  final Duration debounce;

  /// Filesystem poll interval for `PollingDirectoryWatcher`. We use
  /// polling instead of native inotify/FSEvents so this works the same
  /// way on Linux, macOS, and Windows (the in-pod EC2 build runs on
  /// Linux + the developer laptops on macOS, and inotify isn't recursive
  /// on Linux without per-directory watchers).
  final Duration pollInterval;

  VmService? _vm;
  String? _serviceUri;
  Timer? _debounceTimer;
  final _watchSubs = <StreamSubscription<dynamic>>[];
  final _resultSubject = PublishSubject<HotReloadResult>();
  HotReloadResult? _last;

  Stream<HotReloadResult> get results => _resultSubject.stream;
  HotReloadResult? get lastResult => _last;
  String? get serviceUri => _serviceUri;
  bool get isRunning => _vm != null;

  int _reloads = 0;
  int _reloadsFailed = 0;
  int _lastDurationMs = 0;

  int get reloads => _reloads;
  int get reloadsFailed => _reloadsFailed;
  int get lastDurationMs => _lastDurationMs;

  /// Open a connection to our own VM service. Returns `false` when the
  /// VM service is disabled (the AOT case) — caller should log + skip.
  Future<bool> start() async {
    final info = await dev.Service.getInfo();
    final base = info.serverUri;
    if (base == null) {
      // VM service is not running. This is the normal case for AOT.
      stderr.writeln(
        '[hot_reloader] vm-service not enabled — hot reload disabled. '
        'Pass --enable-vm-service to dart run for JIT-mode hot reload.',
      );
      return false;
    }
    _serviceUri = base.toString();
    final ws = vm_utils.convertToWebSocketUrl(serviceProtocolUrl: base);
    _vm = await vmServiceConnectUri(ws.toString());

    for (final dir in watchPaths) {
      final watcher = PollingDirectoryWatcher(
        dir,
        pollingDelay: pollInterval,
      );
      _watchSubs.add(watcher.events.listen(_onWatch));
      // Wait for the watcher to "start" before returning. This is
      // optional but makes startup logs deterministic.
      try {
        await watcher.ready;
      } catch (_) {/* swallow */}
    }

    metrics.registerGauge('dart_hot_reloads_total', () => reloads);
    metrics.registerGauge('dart_hot_reloads_failed_total', () => reloadsFailed);
    metrics.registerGauge('dart_hot_reload_last_ms', () => lastDurationMs);
    return true;
  }

  void _onWatch(WatchEvent e) {
    if (!e.path.endsWith('.dart')) return;
    if (e.path.contains('${Platform.pathSeparator}.dart_tool${Platform.pathSeparator}')) {
      return;
    }
    if (e.path.contains('${Platform.pathSeparator}build${Platform.pathSeparator}')) {
      return;
    }
    _debounceTimer?.cancel();
    _debounceTimer = Timer(debounce, () {
      unawaited(reloadAll(reason: 'watch:${e.type.toString().split('.').last}:${e.path}'));
    });
  }

  /// Trigger a reload of every isolate group in the VM. Safe to call
  /// manually (e.g. from `/dart/admin/reload`).
  ///
  /// `reloadSources(isolateId)` reloads sources for **all** isolates in
  /// the same isolate group as the target. Because session isolates are
  /// spawned via `Isolate.spawn` from the main isolate, they share an
  /// isolate group with the main isolate — so one reload call covers
  /// every active WebSocket session at once.
  Future<HotReloadResult> reloadAll({
    bool force = false,
    String reason = 'manual',
  }) async {
    final vm = _vm;
    if (vm == null) {
      return _capture(HotReloadResult(
        success: false,
        isolatesReloaded: 0,
        isolatesFailed: 0,
        durationMs: 0,
        atUs: DateTime.now().microsecondsSinceEpoch,
        message: 'vm-service not connected',
        byIsolate: const {},
      ));
    }
    final t0 = DateTime.now();
    final vmInfo = await vm.getVM();
    final isolates = vmInfo.isolates ?? const <IsolateRef>[];
    final byIsolate = <String, String>{};
    var failed = 0;

    // Pick one isolate per group as the reload target. Then label every
    // isolate in that group with the result.
    final groupTargets = <String, IsolateRef>{};
    for (final iso in isolates) {
      final group = iso.isolateGroupId;
      if (group == null) continue;
      groupTargets.putIfAbsent(group, () => iso);
    }

    for (final entry in groupTargets.entries) {
      final groupId = entry.key;
      final target = entry.value;
      final id = target.id;
      if (id == null) continue;
      String? failureReason;
      try {
        final report = await vm.reloadSources(id, force: force);
        if (report.success != true) {
          // The exact failure shape depends on the kind of reload error
          // (class-shape mismatch vs. compile error vs. cancel). Serialise
          // the full json so the operator can see what the VM said.
          final raw = report.json;
          failureReason = raw == null
              ? 'reload reported failure (no details)'
              : jsonEncode(raw);
        }
      } catch (e) {
        failureReason = 'exception: $e';
      }

      final groupIsolates =
          isolates.where((i) => i.isolateGroupId == groupId);
      for (final iso in groupIsolates) {
        final label = iso.name ?? iso.id ?? '?';
        if (failureReason == null) {
          byIsolate[label] = 'ok';
        } else {
          byIsolate[label] = failureReason;
          failed++;
        }
      }
    }

    final reloaded = byIsolate.length - failed;
    final result = HotReloadResult(
      success: failed == 0 && byIsolate.isNotEmpty,
      isolatesReloaded: reloaded,
      isolatesFailed: failed,
      durationMs: DateTime.now().difference(t0).inMilliseconds,
      atUs: DateTime.now().microsecondsSinceEpoch,
      message: byIsolate.isEmpty
          ? 'no isolates'
          : (failed == 0
              ? 'reloaded $reloaded isolates across ${groupTargets.length} group(s) ($reason)'
              : '$failed of ${byIsolate.length} isolates failed across ${groupTargets.length} group(s) ($reason)'),
      byIsolate: byIsolate,
    );

    _reloads++;
    if (failed > 0) _reloadsFailed++;
    _lastDurationMs = result.durationMs;
    metrics.inc('dart_hot_reload_attempt_total');
    metrics.inc(
      result.success ? 'dart_hot_reload_success_total' : 'dart_hot_reload_failure_total',
    );
    return _capture(result);
  }

  HotReloadResult _capture(HotReloadResult r) {
    _last = r;
    if (!_resultSubject.isClosed) _resultSubject.add(r);
    return r;
  }

  Future<void> close() async {
    _debounceTimer?.cancel();
    for (final s in _watchSubs) {
      try {
        await s.cancel();
      } catch (_) {/* swallow */}
    }
    try {
      await _vm?.dispose();
    } catch (_) {/* swallow */}
    _vm = null;
    if (!_resultSubject.isClosed) await _resultSubject.close();
  }
}
