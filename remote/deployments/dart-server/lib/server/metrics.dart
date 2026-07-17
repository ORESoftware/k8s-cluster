/// Tiny Prometheus-text metrics aggregator.
///
/// Lives on the main isolate. Counters are incremented either directly
/// (HTTP request hooks) or via [MetricEvent] frames pushed from session
/// isolates. Gauges are sampled at scrape time from a callback registry.
/// Histograms record latency distributions (e.g. WS adopt + first-frame
/// latency); shards forward observations to the coordinator via
/// `ObserveEvent` so the canonical histogram on the coordinator sums
/// every shard's samples.
library;

import 'package:rxdart/rxdart.dart';

/// Default bucket upper bounds (seconds) for latency histograms. Covers
/// sub-millisecond isolate-pool hits through multi-second cold starts so
/// the MDP autotuner can read a meaningful p99 across the whole range.
const List<double> kDefaultLatencyBucketsSeconds = <double>[
  0.0005,
  0.001,
  0.0025,
  0.005,
  0.01,
  0.025,
  0.05,
  0.1,
  0.25,
  0.5,
  1.0,
  2.5,
  5.0,
  10.0,
];

class Metrics {
  Metrics();

  final _counters = <String, int>{};
  final _gauges = <String, num Function()>{};
  final _histograms = <String, _Histogram>{};

  /// Extra exposition fragments appended verbatim at render time. The
  /// coordinator uses this to splice in summed per-shard histograms that
  /// don't live in this instance's own [_histograms] map.
  final _rawProviders = <String Function()>[];

  /// Hot stream of mutated counter names. Useful for tests / live debugging.
  final _ticker = PublishSubject<String>();
  Stream<String> get ticker => _ticker.stream;

  void inc(String name, [int delta = 1]) {
    _counters.update(name, (v) => v + delta, ifAbsent: () => delta);
    _ticker.add(name);
  }

  /// Current value of a counter (0 if never incremented). Lets the
  /// coordinator's control loop read counter deltas between control ticks.
  int counter(String name) => _counters[name] ?? 0;

  void registerGauge(String name, num Function() sampler) {
    _gauges[name] = sampler;
  }

  /// Current value of a registered gauge, or null if absent. Reads through
  /// the sampler callback (same value `/metrics` would render).
  num? gauge(String name) {
    final sampler = _gauges[name];
    return sampler?.call();
  }

  /// Register a callback whose returned text is appended verbatim to the
  /// `/metrics` exposition. Used for cross-isolate-summed histograms.
  void registerRawExposition(String Function() provider) {
    _rawProviders.add(provider);
  }

  /// Record a latency / size sample (seconds) into a histogram. Creates
  /// the histogram lazily with [bounds] (defaults to latency buckets).
  void observe(String name, double value, {List<double>? bounds}) {
    final hist = _histograms.putIfAbsent(
      name,
      () => _Histogram(bounds ?? kDefaultLatencyBucketsSeconds),
    );
    hist.observe(value);
  }

  /// Approximate quantile [q] (0..1) of a histogram by linear interpolation
  /// within the bucket that crosses the target rank. Returns 0 if the
  /// histogram is empty / unknown. Mirrors Prometheus `histogram_quantile`
  /// closely enough for the autotuner's reward signal.
  double histogramQuantile(String name, double q) {
    final hist = _histograms[name];
    if (hist == null || hist.count == 0) return 0;
    return hist.quantile(q);
  }

  /// Render in Prometheus exposition format.
  String render() {
    final sb = StringBuffer();
    final now = DateTime.now().toUtc().toIso8601String();
    sb.writeln('# dd-dart-server prometheus exposition');
    sb.writeln('# scraped_at $now');

    final counterNames = _counters.keys.toList()..sort();
    for (final name in counterNames) {
      sb.writeln('# TYPE $name counter');
      sb.writeln('$name ${_counters[name]}');
    }

    final gaugeNames = _gauges.keys.toList()..sort();
    for (final name in gaugeNames) {
      num value;
      try {
        value = _gauges[name]!();
      } catch (_) {
        // A faulty gauge closure must not throw out of render() and take the
        // whole /metrics scrape down with it (the raw providers below are
        // already guarded; gauges were not). Skip the bad gauge.
        continue;
      }
      // Prometheus exposition can't parse Dart's `Infinity` / `NaN` spellings
      // (it wants `+Inf` / `-Inf` / `NaN`), so a non-finite reading would make
      // the line unparseable and break the scrape for the whole endpoint.
      // Coerce to 0 — these gauges should always be finite anyway.
      if (value is double && !value.isFinite) value = 0;
      sb.writeln('# TYPE $name gauge');
      sb.writeln('$name $value');
    }

    final histNames = _histograms.keys.toList()..sort();
    for (final name in histNames) {
      sb.write(_histograms[name]!.render(name));
    }

    for (final provider in _rawProviders) {
      try {
        sb.write(provider());
      } catch (_) {/* never let a faulty provider break /metrics */}
    }
    return sb.toString();
  }

  Future<void> close() async {
    await _ticker.close();
  }
}

/// Single Prometheus histogram. Buckets are stored as per-bucket counts;
/// cumulative `le` lines are computed at render time. Additive across
/// isolates: summing two histograms with identical bounds is just
/// element-wise count + sum + total addition, which is exactly how the
/// coordinator folds `ObserveEvent`s from every shard.
class _Histogram {
  _Histogram(this.bounds)
      : counts = List<int>.filled(bounds.length + 1, 0, growable: false);

  /// Sorted ascending upper bounds. The implicit final bucket is `+Inf`.
  final List<double> bounds;

  /// counts[i] = samples in `(bounds[i-1], bounds[i]]`; the last slot is
  /// the `+Inf` overflow bucket.
  final List<int> counts;

  double sum = 0;
  int count = 0;

  void observe(double value) {
    sum += value;
    count++;
    for (var i = 0; i < bounds.length; i++) {
      if (value <= bounds[i]) {
        counts[i]++;
        return;
      }
    }
    counts[bounds.length]++;
  }

  double quantile(double q) {
    if (count == 0) return 0;
    final target = (q.clamp(0.0, 1.0)) * count;
    var cumulative = 0;
    var lowerBound = 0.0;
    for (var i = 0; i < bounds.length; i++) {
      final next = cumulative + counts[i];
      if (next >= target) {
        // Linear interpolation inside this bucket's [lowerBound, upper].
        final upper = bounds[i];
        final inBucket = counts[i];
        if (inBucket == 0) return upper;
        final frac = (target - cumulative) / inBucket;
        return lowerBound + (upper - lowerBound) * frac;
      }
      cumulative = next;
      lowerBound = bounds[i];
    }
    // Falls into the +Inf bucket; report the largest finite bound.
    return bounds.isEmpty ? 0 : bounds.last;
  }

  String render(String name) {
    final sb = StringBuffer();
    sb.writeln('# TYPE $name histogram');
    var cumulative = 0;
    for (var i = 0; i < bounds.length; i++) {
      cumulative += counts[i];
      sb.writeln('${name}_bucket{le="${_fmt(bounds[i])}"} $cumulative');
    }
    cumulative += counts[bounds.length];
    sb.writeln('${name}_bucket{le="+Inf"} $cumulative');
    sb.writeln('${name}_sum $sum');
    sb.writeln('${name}_count $count');
    return sb.toString();
  }

  static String _fmt(double b) {
    // Prometheus le labels: avoid trailing ".0" noise on whole numbers but
    // keep small fractional bounds intact.
    if (b == b.roundToDouble() && b.abs() < 1e15) {
      return b.toStringAsFixed(b.abs() >= 1 ? 1 : 4);
    }
    return b.toString();
  }
}
