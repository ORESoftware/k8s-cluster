/// Thin HTTP client for the cluster's `dd-mdp-optimizer` service.
///
/// When `WS_MDP_MODE=remote`, the coordinator delegates the pool decision to
/// the sanctioned Rust MDP/POMDP optimizer
/// (`dd-mdp-optimizer.default.svc.cluster.local:8096`) via its
/// `POST /telemetry/learn` endpoint instead of the local Q-learner. The
/// coordinator tunes two independent levers, so we ask the optimizer about
/// each as its own candidate ladder over the same bounded telemetry (pool
/// utilisation, cold-start pressure, refusals, p99 adopt latency):
///
///   * **pool size** — candidate actions `pool-20 … pool-50`; the optimizer's
///     `recommendedAction` maps back to a host-isolate target.
///   * **host density** — candidate actions `density-100 … density-1000`;
///     the recommendation maps back to a per-host session cap.
///
/// Everything here is best-effort: any network error, timeout, or unmappable
/// response leaves that lever's field `null` so the caller holds the previous
/// setpoint. This keeps the feature working with zero hard runtime dependency
/// on the optimizer being reachable.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// Snapshot the coordinator hands the client each control tick.
class OptimizerSignals {
  const OptimizerSignals({
    required this.utilization,
    required this.coldStartRate,
    required this.refusalRate,
    required this.p99AdoptSeconds,
    required this.windowMs,
    required this.sizeLevels,
    required this.densityLevels,
  });

  /// Pool utilisation in `[0, 1+]` (sessions / capacity).
  final double utilization;

  /// Cold-start spawns per second over the window.
  final double coldStartRate;

  /// Capacity refusals per second over the window.
  final double refusalRate;

  final double p99AdoptSeconds;
  final int windowMs;

  /// Host-isolate ladder, e.g. `[20, 30, 40, 50]`.
  final List<int> sizeLevels;

  /// Per-host density ladder, e.g. `[100, 250, 500, 1000]`.
  final List<int> densityLevels;
}

/// The optimizer's recommendation for one control tick. Either field is
/// `null` when that lever could not be resolved (optimizer unreachable or a
/// generic `observe`/`hold` action), in which case the caller holds the
/// previous setpoint for that lever.
class OptimizerDecision {
  const OptimizerDecision({this.targetHosts, this.sessionsPerHost});
  final int? targetHosts;
  final int? sessionsPerHost;
}

class OptimizerClient {
  OptimizerClient({
    required this.baseUrl,
    this.timeout = const Duration(seconds: 2),
  }) : _http = (HttpClient()..connectionTimeout = timeout);

  /// e.g. `http://dd-mdp-optimizer.default.svc.cluster.local:8096`.
  final String baseUrl;
  final Duration timeout;
  final HttpClient _http;

  /// Ask the optimizer for both levers (pool size + density) over the same
  /// telemetry window. The two requests run concurrently; each lever falls
  /// back to `null` independently on any failure.
  Future<OptimizerDecision> recommend(OptimizerSignals s) async {
    final results = await Future.wait<int?>([
      _askLadder(
        scope: 'dart-wss-pool',
        prefix: 'pool',
        levels: s.sizeLevels,
        signals: _poolSignals(s),
        windowMs: s.windowMs,
      ),
      _askLadder(
        scope: 'dart-wss-density',
        prefix: 'density',
        levels: s.densityLevels,
        signals: _densitySignals(s),
        windowMs: s.windowMs,
      ),
    ]);
    return OptimizerDecision(
      targetHosts: results[0],
      sessionsPerHost: results[1],
    );
  }

  // ---- per-lever signal/impact shaping ----------------------------------

  /// Pool-size hints: as load/latency risk rises, push toward the largest
  /// pool; negatively impact the smallest pool when saturated.
  List<Map<String, Object?>> _poolSignals(OptimizerSignals s) {
    return <Map<String, Object?>>[
      {
        'name': 'ws_pool_utilization',
        'service': 'dd-dart-server',
        'layer': 'app',
        'value': s.utilization,
        'warning': 0.8,
        'critical': 0.95,
        'weight': 1.0,
        'actionImpacts': [
          {'action': 'pool-${s.sizeLevels.last}', 'delta': 0.5, 'confidence': 0.8},
          {'action': 'pool-${s.sizeLevels.first}', 'delta': -0.4, 'confidence': 0.8},
        ],
      },
      _coldStartSignal(s),
      _refusalSignal(s),
      {
        'name': 'ws_p99_adopt_latency_seconds',
        'service': 'dd-dart-server',
        'layer': 'app',
        'value': s.p99AdoptSeconds,
        'warning': 0.05,
        'critical': 0.5,
        'weight': 1.0,
        'actionImpacts': [
          {'action': 'pool-${s.sizeLevels.last}', 'delta': 0.3, 'confidence': 0.6},
        ],
      },
    ];
  }

  /// Density hints: high utilisation favours packing more per host (absorb
  /// load without forking new isolates → push the top density); high p99
  /// adopt latency favours spreading thinner to cut per-isolate contention
  /// (→ push the bottom density). Cold starts also favour denser hosts (each
  /// host covers more of the offered load before a new one is needed).
  List<Map<String, Object?>> _densitySignals(OptimizerSignals s) {
    return <Map<String, Object?>>[
      {
        'name': 'ws_pool_utilization',
        'service': 'dd-dart-server',
        'layer': 'app',
        'value': s.utilization,
        'warning': 0.8,
        'critical': 0.95,
        'weight': 1.0,
        'actionImpacts': [
          {'action': 'density-${s.densityLevels.last}', 'delta': 0.4, 'confidence': 0.7},
        ],
      },
      _coldStartSignal(s),
      _refusalSignal(s),
      {
        'name': 'ws_p99_adopt_latency_seconds',
        'service': 'dd-dart-server',
        'layer': 'app',
        'value': s.p99AdoptSeconds,
        'warning': 0.05,
        'critical': 0.5,
        'weight': 1.0,
        'actionImpacts': [
          {'action': 'density-${s.densityLevels.first}', 'delta': 0.4, 'confidence': 0.7},
          {'action': 'density-${s.densityLevels.last}', 'delta': -0.3, 'confidence': 0.7},
        ],
      },
    ];
  }

  Map<String, Object?> _coldStartSignal(OptimizerSignals s) => {
        'name': 'ws_cold_start_rate',
        'service': 'dd-dart-server',
        'layer': 'app',
        'value': s.coldStartRate,
        'warning': 0.5,
        'critical': 5.0,
        'weight': 1.0,
      };

  Map<String, Object?> _refusalSignal(OptimizerSignals s) => {
        'name': 'ws_capacity_refusal_rate',
        'service': 'dd-dart-server',
        'layer': 'app',
        'value': s.refusalRate,
        'warning': 0.01,
        'critical': 0.5,
        'weight': 1.0,
      };

  // ---- transport ---------------------------------------------------------

  /// POST one ladder of `'$prefix-$level'` candidate actions and map the
  /// optimizer's `recommendedAction` back to its integer level. Returns
  /// `null` on any failure or an unmappable (e.g. `observe`/`hold`) action.
  Future<int?> _askLadder({
    required String scope,
    required String prefix,
    required List<int> levels,
    required List<Map<String, Object?>> signals,
    int windowMs = 0,
  }) async {
    if (levels.isEmpty) return null;
    final actions = [for (final n in levels) '$prefix-$n'];
    final body = <String, Object?>{
      'requestId': '$scope-${DateTime.now().millisecondsSinceEpoch}',
      'scope': scope,
      'windowMs': windowMs,
      'actions': actions,
      'signals': signals,
    };

    try {
      final uri = Uri.parse('$baseUrl/telemetry/learn');
      final req = await _http.postUrl(uri).timeout(timeout);
      req.headers.contentType = ContentType.json;
      req.add(utf8.encode(jsonEncode(body)));
      final resp = await req.close().timeout(timeout);
      if (resp.statusCode != 200) {
        await resp.drain<void>();
        return null;
      }
      // Bound the reply we'll buffer. The optimizer is internal, but a
      // misbehaving/compromised endpoint could stream an unbounded body and
      // OOM the coordinator; `.join()` would buffer it all. Accumulate up to a
      // hard ceiling and bail otherwise (treated as a miss, so the lever holds
      // its previous setpoint).
      const maxReplyBytes = 1 << 20; // 1 MiB
      final bytes = <int>[];
      await for (final chunk in resp.timeout(timeout)) {
        bytes.addAll(chunk);
        if (bytes.length > maxReplyBytes) {
          unawaited(resp.drain<void>().catchError((_) {}));
          return null;
        }
      }
      final decoded = jsonDecode(utf8.decode(bytes, allowMalformed: true));
      if (decoded is! Map) return null;
      final action = decoded['recommendedAction'];
      if (action is! String) return null;
      return _parseLevel(action, '$prefix-', levels);
    } catch (_) {
      return null;
    }
  }

  /// Maps `pool-30` → `30` (or `density-500` → `500`) if the level is known;
  /// otherwise null (e.g. the optimizer fell back to a generic action).
  static int? _parseLevel(String action, String prefix, List<int> levels) {
    if (!action.startsWith(prefix)) return null;
    final n = int.tryParse(action.substring(prefix.length));
    if (n == null || !levels.contains(n)) return null;
    return n;
  }

  void close() {
    _http.close(force: true);
  }
}
