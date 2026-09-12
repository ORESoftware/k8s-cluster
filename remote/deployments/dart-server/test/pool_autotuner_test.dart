import 'package:dd_dart_server/server/pool_autotuner.dart';
import 'package:test/test.dart';

/// Synthetic load environment for the pool-size dimension: given the pool
/// size currently applied (`target` host isolates), the per-host `density`
/// cap, and the offered load (`sessions`), produce the telemetry the
/// supervisor would have reported over the interval.
///
///   * Under-provisioned (load > target × density) ⇒ cold starts + high p99
///     (connections had to spawn an isolate on their own hot path).
///   * Over-provisioned ⇒ idle hosts (the cost the autotuner trades off).
///
/// With `density == 1000` this reduces exactly to the original fixed-1000
/// environment, so the pool-size tests pin density to `[1000]` and exercise
/// only the host-count lever.
AutotunerObservation _env(int target, int density, int sessions, int prev) {
  final capacity = target * density;
  final neededHosts = (sessions / density).ceil();
  final overflow = sessions - capacity;
  final coldStarts = overflow > 0 ? (overflow / density).ceil() : 0;
  final idle = (target - neededHosts) > 0 ? target - neededHosts : 0;
  final p99 = overflow > 0 ? 0.5 : 0.002;
  return AutotunerObservation(
    totalSessions: sessions,
    liveHosts: target,
    sessionsPerHost: density,
    sessionsDelta: sessions - prev,
    coldStarts: coldStarts,
    refusals: 0,
    idleHosts: idle,
    p99AdoptSeconds: p99,
    p99FirstFrameSeconds: p99 * 0.5,
  );
}

/// Two-dimensional environment with a genuine density optimum. Latency rises
/// with the per-host *cap* (a host told to pack `density` sessions runs that
/// many RxDart graphs per tick), while too-low a cap forces extra hosts the
/// pool ceiling can't supply (cold starts). For ~20K sessions the sweet spot
/// is ~40 hosts × 500/host: enough capacity with no contention.
AutotunerObservation _env2d(int target, int density, int sessions, int prev) {
  final neededHosts = (sessions / density).ceil();
  final coldStarts = neededHosts > target ? neededHosts - target : 0;
  final idle = (target - neededHosts) > 0 ? target - neededHosts : 0;
  // Per-isolate event-loop contention once the cap passes a comfort band.
  final contention = density > 500 ? (density - 500) * 0.0006 : 0.0;
  final p99 = (coldStarts > 0 ? 0.5 : 0.002) + contention;
  return AutotunerObservation(
    totalSessions: sessions,
    liveHosts: target,
    sessionsPerHost: density,
    sessionsDelta: sessions - prev,
    coldStarts: coldStarts,
    refusals: 0,
    idleHosts: idle,
    p99AdoptSeconds: p99,
    p99FirstFrameSeconds: p99 * 0.5,
  );
}

/// Drive the learner against [env] for [steps] ticks, feeding each tick's
/// chosen `(target, density)` back in, and return the mean target + density
/// over the final tail window.
({double target, double density}) _tailMeans(
  PoolAutotuner at,
  int sessions, {
  required int seedTarget,
  required int seedDensity,
  AutotunerObservation Function(int, int, int, int) env = _env,
  int steps = 4000,
  int tailFrom = 3000,
}) {
  var target = seedTarget;
  var density = seedDensity;
  var prev = 0;
  final tTail = <int>[];
  final dTail = <int>[];
  for (var i = 0; i < steps; i++) {
    final obs = env(target, density, sessions, prev);
    prev = sessions;
    final d = at.decide(obs);
    target = d.targetHosts;
    density = d.sessionsPerHost;
    if (i >= tailFrom) {
      tTail.add(target);
      dTail.add(density);
    }
  }
  return (
    target: tTail.reduce((a, b) => a + b) / tTail.length,
    density: dTail.reduce((a, b) => a + b) / dTail.length,
  );
}

void main() {
  group('PoolAutotuner (pool-size lever, density pinned)', () {
    test('decide always returns configured size + density levels', () {
      final at =
          PoolAutotuner(const AutotunerConfig(seed: 1, densityLevels: [1000]));
      for (var i = 0; i < 50; i++) {
        final d = at.decide(_env(30, 1000, 20000, 20000));
        expect(const [20, 30, 40, 50], contains(d.targetHosts));
        expect(d.sessionsPerHost, 1000);
      }
    });

    test('is deterministic for a fixed seed + observation sequence', () {
      final a =
          PoolAutotuner(const AutotunerConfig(seed: 42, densityLevels: [1000]));
      final b =
          PoolAutotuner(const AutotunerConfig(seed: 42, densityLevels: [1000]));
      var ta = 20, tb = 20, pa = 0, pb = 0;
      for (var i = 0; i < 300; i++) {
        ta = a.decide(_env(ta, 1000, 35000, pa)).targetHosts;
        pa = 35000;
        tb = b.decide(_env(tb, 1000, 35000, pb)).targetHosts;
        pb = 35000;
        expect(ta, tb, reason: 'step $i diverged');
      }
    });

    test('scales the pool UP under sustained high load (~35K sessions)', () {
      // Needs ~35 hosts; the smallest adequate level is 40. The learner
      // should avoid the under-provisioned 20/30 (huge latency penalty) and
      // settle near 40.
      final at =
          PoolAutotuner(const AutotunerConfig(seed: 7, densityLevels: [1000]));
      final means = _tailMeans(at, 35000, seedTarget: 20, seedDensity: 1000);
      expect(means.target, greaterThanOrEqualTo(35),
          reason: 'should provision near the ~35-host demand, got ${means.target}');
      expect(at.recommend(_env(40, 1000, 35000, 35000)).targetHosts,
          greaterThanOrEqualTo(40));
    });

    test('scales the pool DOWN under light load (~5K sessions)', () {
      // Every level covers the load, so idle-host + size cost dominate and
      // the smallest level (20) wins.
      final at =
          PoolAutotuner(const AutotunerConfig(seed: 11, densityLevels: [1000]));
      final means = _tailMeans(at, 5000, seedTarget: 50, seedDensity: 1000);
      expect(means.target, lessThanOrEqualTo(26),
          reason: 'should shrink toward the minimum level, got ${means.target}');
    });

    test('reward EMA improves as the policy learns', () {
      final at =
          PoolAutotuner(const AutotunerConfig(seed: 3, densityLevels: [1000]));
      var target = 20;
      var prev = 0;
      at.decide(_env(target, 1000, 35000, prev));
      final early = at.rewardEma;
      for (var i = 0; i < 3000; i++) {
        final obs = _env(target, 1000, 35000, prev);
        prev = 35000;
        target = at.decide(obs).targetHosts;
      }
      expect(at.rewardEma, greaterThan(early),
          reason: 'reward EMA should rise (become less negative) with learning');
      expect(at.epsilon, lessThan(0.30));
    });
  });

  group('PoolAutotuner (joint size × density)', () {
    // The defaults already are sizeLevels [20,30,40,50] ×
    // densityLevels [100,250,500,1000], i.e. the 16-cell joint action space.

    test('decode covers the full Cartesian action space', () {
      final at = PoolAutotuner(const AutotunerConfig(seed: 99));
      final hosts = <int>{};
      final densities = <int>{};
      // ε starts at 0.30 so a few hundred exploratory ticks hit every cell.
      for (var i = 0; i < 4000; i++) {
        final d = at.decide(_env2d(40, 500, 20000, 20000));
        hosts.add(d.targetHosts);
        densities.add(d.sessionsPerHost);
        expect(const [20, 30, 40, 50], contains(d.targetHosts));
        expect(const [100, 250, 500, 1000], contains(d.sessionsPerHost));
        expect(at.lastSessionsPerHost, d.sessionsPerHost);
        expect(at.lastTargetHosts, d.targetHosts);
      }
      expect(hosts, containsAll(const [20, 30, 40, 50]),
          reason: 'every host level should be explored');
      expect(densities, containsAll(const [100, 250, 500, 1000]),
          reason: 'every density level should be explored');
    });

    test('is deterministic for a fixed seed across both levers', () {
      final a = PoolAutotuner(const AutotunerConfig(seed: 21));
      final b = PoolAutotuner(const AutotunerConfig(seed: 21));
      var ta = 20, tb = 20, da = 500, db = 500, pa = 0, pb = 0;
      for (var i = 0; i < 400; i++) {
        final ra = a.decide(_env2d(ta, da, 20000, pa));
        final rb = b.decide(_env2d(tb, db, 20000, pb));
        pa = 20000;
        pb = 20000;
        ta = ra.targetHosts;
        da = ra.sessionsPerHost;
        tb = rb.targetHosts;
        db = rb.sessionsPerHost;
        expect(ta, tb, reason: 'host lever diverged at step $i');
        expect(da, db, reason: 'density lever diverged at step $i');
      }
    });

    test(
        'learns a sensible density: avoids both the cold-start and '
        'contention extremes (~20K sessions)', () {
      // Optimum is ~40 hosts × 500/host: 100/250 force more hosts than the
      // ladder can supply (cold starts), 1000 over-packs (latency). The
      // learner should converge into the comfortable middle.
      final at = PoolAutotuner(const AutotunerConfig(seed: 7));
      final means = _tailMeans(
        at,
        20000,
        seedTarget: 20,
        seedDensity: 100,
        env: _env2d,
        steps: 6000,
        tailFrom: 4500,
      );
      expect(means.density, greaterThan(300),
          reason: 'should avoid the cold-start-prone low densities, '
              'got ${means.density}');
      expect(means.density, lessThan(800),
          reason: 'should avoid the contention-heavy 1000 cap, '
              'got ${means.density}');
      expect(means.target, greaterThanOrEqualTo(35),
          reason: 'needs ~40 hosts at 500/host to cover the load, '
              'got ${means.target}');
    });
  });
}
