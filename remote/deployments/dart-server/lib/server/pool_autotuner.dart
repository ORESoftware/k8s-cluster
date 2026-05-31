/// MDP-style isolate-pool autotuner.
///
/// Frames "how big should the session-host isolate pool be?" as a
/// contextual reinforcement-learning problem and learns the answer online
/// from live telemetry. It is the brain behind `WS_MDP_MODE=local`.
///
/// MDP shape
/// ---------
///   * **State (context)** — a discretised view of load: pool utilisation
///     bucket × arrival-trend bucket. Utilisation is
///     `liveSessions / (liveHosts × sessionsPerHost)`; trend is the
///     sign/magnitude of the session-count delta since the last tick.
///   * **Action** — a *joint* choice over two control variables, decoded
///     from one discrete action index:
///       1. the pod-wide host-isolate target from a small ladder, by default
///          `{20, 30, 40, 50}` (the levels we A/B/n across to hit 50K WS
///          connections without over-provisioning at medium load), and
///       2. the per-host session density (`sessionsPerHost`) from a second
///          ladder, by default `{100, 250, 500, 1000}` — how densely to pack
///          sessions onto each isolate. Low density spreads load across more,
///          quieter event loops (lower per-isolate contention, more base-heap
///          overhead); high density packs fewer, busier isolates (cheaper RAM
///          floor, higher tail latency under contention). The action space is
///          the Cartesian product, so the learner explores e.g. "40 hosts ×
///          250/host" vs "20 hosts × 1000/host" for the *same* offered load.
///   * **Reward** — negative cost:
///       `-(wLatency·p99/target + wColdStart·coldStarts + wRefusal·refusals
///          + wIdle·idleHosts + wSize·targetHosts/maxLevel)`
///     so the learner is pushed toward the *smallest* pool that still
///     keeps p99 adopt/first-frame latency low and cold-starts/refusals at
///     zero. Cold starts (a connection that had to `Isolate.spawn` on its
///     own hot path) and capacity refusals are the latency/availability
///     failures we most want to avoid; idle hosts and raw pool size are
///     the cost we trade against them.
///   * **Update** — tabular Q-learning,
///     `Q[s,a] += α·(r + γ·maxₐ′Q[s′,a′] − Q[s,a])`, with ε-greedy
///     exploration decaying from [AutotunerConfig.epsilonStart] toward
///     [AutotunerConfig.epsilonMin].
///
/// The whole thing is pure Dart with no isolate / IO dependencies so it
/// unit-tests deterministically against a synthetic load environment.
/// Adding a *third* action dimension later (e.g. pool *count* ∈ {2,3} for
/// library-segmented heaps) is a drop-in: widen [_actionCount] and the
/// [decide] decode the same way the density dimension was added here.
library;

import 'dart:math';

/// One control-tick decision: the two pool knobs the policy chose for the
/// upcoming interval. Decoded from a single Q-table action index.
class AutotunerDecision {
  const AutotunerDecision({
    required this.targetHosts,
    required this.sessionsPerHost,
  });

  /// Pod-wide session-host isolate target (a level from
  /// [AutotunerConfig.sizeLevels]).
  final int targetHosts;

  /// Per-host session cap / density (a level from
  /// [AutotunerConfig.densityLevels]).
  final int sessionsPerHost;

  @override
  String toString() =>
      'AutotunerDecision(targetHosts: $targetHosts, '
      'sessionsPerHost: $sessionsPerHost)';
}

/// One control-tick telemetry snapshot handed to the autotuner. All deltas
/// (cold starts, refusals) are measured over the interval that elapsed
/// under the *previously chosen* action, so the reward they produce is
/// correctly attributed to that action.
class AutotunerObservation {
  const AutotunerObservation({
    required this.totalSessions,
    required this.liveHosts,
    required this.sessionsPerHost,
    required this.sessionsDelta,
    required this.coldStarts,
    required this.refusals,
    required this.idleHosts,
    required this.p99AdoptSeconds,
    required this.p99FirstFrameSeconds,
  });

  final int totalSessions;
  final int liveHosts;
  final int sessionsPerHost;
  final int sessionsDelta;
  final int coldStarts;
  final int refusals;
  final int idleHosts;
  final double p99AdoptSeconds;
  final double p99FirstFrameSeconds;
}

/// Tunable hyperparameters + reward weights. Every field is overridable
/// from an env var in `bin/server.dart` so operators can retune without a
/// rebuild.
class AutotunerConfig {
  const AutotunerConfig({
    this.sizeLevels = const <int>[20, 30, 40, 50],
    this.densityLevels = const <int>[100, 250, 500, 1000],
    this.alpha = 0.2,
    this.gamma = 0.6,
    this.epsilonStart = 0.30,
    this.epsilonMin = 0.02,
    this.epsilonDecay = 0.995,
    this.targetLatencySeconds = 0.05,
    this.latencyPenaltyCap = 8.0,
    this.wLatency = 1.0,
    this.wColdStart = 0.05,
    this.wRefusal = 0.5,
    this.wIdle = 0.02,
    this.wSize = 0.2,
    this.seed,
  });

  /// Discrete pod-wide host-isolate targets the policy chooses between.
  final List<int> sizeLevels;

  /// Discrete per-host session densities (`sessionsPerHost` caps) the policy
  /// chooses between. A single-element list collapses the second dimension,
  /// reducing the learner to the pure pool-size experiment.
  final List<int> densityLevels;

  final double alpha;
  final double gamma;
  final double epsilonStart;
  final double epsilonMin;
  final double epsilonDecay;

  /// p99 latency (seconds) considered "good"; the latency penalty is the
  /// observed p99 divided by this, so hitting target costs ~1.0.
  final double targetLatencySeconds;

  /// Clamp on the normalised latency penalty so a single pathological
  /// spike can't dominate the Q-update.
  final double latencyPenaltyCap;

  final double wLatency;
  final double wColdStart;
  final double wRefusal;
  final double wIdle;
  final double wSize;

  /// Optional RNG seed for deterministic tests.
  final int? seed;
}

/// Number of utilisation buckets in the state encoding.
const int _utilBuckets = 4;

/// Number of arrival-trend buckets in the state encoding.
const int _trendBuckets = 3;

class PoolAutotuner {
  PoolAutotuner(this.config)
      : _epsilon = config.epsilonStart,
        _rng = Random(config.seed),
        _actionCount = config.sizeLevels.length * config.densityLevels.length,
        assert(config.sizeLevels.isNotEmpty, 'need at least one size level'),
        assert(config.densityLevels.isNotEmpty,
            'need at least one density level');

  final AutotunerConfig config;
  final Random _rng;

  /// Size of the joint (sizeLevels × densityLevels) action space.
  final int _actionCount;

  /// state index → per-action Q-values.
  final _q = <int, List<double>>{};

  double _epsilon;
  bool _hasPrev = false;
  int _prevState = 0;
  int _prevAction = 0;
  int _prevTargetHosts = 0;
  int _prevSessionsPerHost = 0;

  double _lastReward = 0;
  double _rewardEma = 0;
  bool _emaSeeded = false;
  int _updates = 0;

  // ---- public read-only telemetry ---------------------------------------
  double get epsilon => _epsilon;
  double get lastReward => _lastReward;
  double get rewardEma => _rewardEma;
  int get updates => _updates;
  int get statesVisited => _q.length;
  int get lastTargetHosts => _prevTargetHosts;
  int get lastSessionsPerHost => _prevSessionsPerHost;
  int get lastActionIndex => _prevAction;

  // ---- joint action decode -----------------------------------------------
  // The action index packs (sizeIdx, densityIdx) in row-major order:
  //   action = sizeIdx * densityLevels.length + densityIdx
  int _sizeIndexOf(int action) => action ~/ config.densityLevels.length;
  int _densityIndexOf(int action) => action % config.densityLevels.length;
  int _targetHostsOf(int action) => config.sizeLevels[_sizeIndexOf(action)];
  int _densityOf(int action) => config.densityLevels[_densityIndexOf(action)];

  /// Run one control step: fold the reward earned by the previous action
  /// into Q, then choose (and remember) the action for the current state.
  /// Returns the chosen pod-wide host-isolate target AND per-host density.
  AutotunerDecision decide(AutotunerObservation obs) {
    final state = _encodeState(obs);
    final reward = _reward(obs);
    _lastReward = reward;
    _rewardEma = _emaSeeded ? (_rewardEma * 0.9 + reward * 0.1) : reward;
    _emaSeeded = true;

    if (_hasPrev) {
      _update(_prevState, _prevAction, reward, state);
    }

    final action = _selectAction(state);
    _prevState = state;
    _prevAction = action;
    _prevTargetHosts = _targetHostsOf(action);
    _prevSessionsPerHost = _densityOf(action);
    _hasPrev = true;
    _decayEpsilon();
    return AutotunerDecision(
      targetHosts: _prevTargetHosts,
      sessionsPerHost: _prevSessionsPerHost,
    );
  }

  /// Greedy (no-exploration) recommendation for a state, for operators /
  /// dashboards that want "what would it do right now".
  AutotunerDecision recommend(AutotunerObservation obs) {
    final state = _encodeState(obs);
    final action = _argmax(_row(state));
    return AutotunerDecision(
      targetHosts: _targetHostsOf(action),
      sessionsPerHost: _densityOf(action),
    );
  }

  // ---- reward ------------------------------------------------------------
  double _reward(AutotunerObservation obs) {
    final p99 = max(obs.p99AdoptSeconds, obs.p99FirstFrameSeconds);
    final latPenalty = config.targetLatencySeconds <= 0
        ? 0.0
        : (p99 / config.targetLatencySeconds).clamp(0.0, config.latencyPenaltyCap);
    final maxLevel = config.sizeLevels.reduce(max);
    final sizeCost = maxLevel <= 0 ? 0.0 : _prevTargetHosts / maxLevel;
    return -(config.wLatency * latPenalty +
        config.wColdStart * obs.coldStarts +
        config.wRefusal * obs.refusals +
        config.wIdle * obs.idleHosts +
        config.wSize * sizeCost);
  }

  // ---- Q-learning core ---------------------------------------------------
  List<double> _row(int state) =>
      _q.putIfAbsent(state, () => List<double>.filled(_actionCount, 0));

  void _update(int s, int a, double reward, int sNext) {
    final row = _row(s);
    final maxNext = _maxValue(_row(sNext));
    row[a] += config.alpha * (reward + config.gamma * maxNext - row[a]);
    _updates++;
  }

  int _selectAction(int state) {
    if (_rng.nextDouble() < _epsilon) {
      return _rng.nextInt(_actionCount);
    }
    return _argmax(_row(state));
  }

  int _argmax(List<double> row) {
    var best = 0;
    var bestVal = row[0];
    var ties = 1;
    for (var i = 1; i < row.length; i++) {
      if (row[i] > bestVal) {
        bestVal = row[i];
        best = i;
        ties = 1;
      } else if (row[i] == bestVal) {
        ties++;
        // Reservoir tie-break so we don't always bias toward the low index.
        if (_rng.nextInt(ties) == 0) best = i;
      }
    }
    return best;
  }

  double _maxValue(List<double> row) {
    var m = row[0];
    for (var i = 1; i < row.length; i++) {
      if (row[i] > m) m = row[i];
    }
    return m;
  }

  void _decayEpsilon() {
    _epsilon = max(config.epsilonMin, _epsilon * config.epsilonDecay);
  }

  // ---- state encoding ----------------------------------------------------
  int _encodeState(AutotunerObservation obs) {
    return _utilBucket(obs) * _trendBuckets + _trendBucket(obs);
  }

  int _utilBucket(AutotunerObservation obs) {
    final capacity = obs.liveHosts * obs.sessionsPerHost;
    final util = capacity > 0
        ? obs.totalSessions / capacity
        : (obs.totalSessions > 0 ? 1.0 : 0.0);
    if (util < 0.5) return 0;
    if (util < 0.8) return 1;
    if (util < 0.95) return 2;
    return 3;
  }

  int _trendBucket(AutotunerObservation obs) {
    final cap = obs.sessionsPerHost <= 0 ? 1 : obs.sessionsPerHost;
    final threshold = (0.1 * cap).ceil();
    if (obs.sessionsDelta < -threshold) return 0; // falling
    if (obs.sessionsDelta <= threshold) return 1; // flat
    return 2; // rising
  }
}
