/// Tiny Prometheus-text metrics aggregator.
///
/// Lives on the main isolate. Counters are incremented either directly
/// (HTTP request hooks) or via [MetricEvent] frames pushed from session
/// isolates. Gauges are sampled at scrape time from a callback registry.
library;

import 'package:rxdart/rxdart.dart';

class Metrics {
  Metrics();

  final _counters = <String, int>{};
  final _gauges = <String, num Function()>{};

  /// Hot stream of mutated counter names. Useful for tests / live debugging.
  final _ticker = PublishSubject<String>();
  Stream<String> get ticker => _ticker.stream;

  void inc(String name, [int delta = 1]) {
    _counters.update(name, (v) => v + delta, ifAbsent: () => delta);
    _ticker.add(name);
  }

  void registerGauge(String name, num Function() sampler) {
    _gauges[name] = sampler;
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
      sb.writeln('# TYPE $name gauge');
      sb.writeln('$name ${_gauges[name]!()}');
    }
    return sb.toString();
  }

  Future<void> close() async {
    await _ticker.close();
  }
}
