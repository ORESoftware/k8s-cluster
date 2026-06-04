import 'package:dd_dart_server/server/metrics.dart';
import 'package:test/test.dart';

void main() {
  group('Metrics histograms', () {
    test('observe renders Prometheus histogram lines', () {
      final m = Metrics();
      for (final v in <double>[0.001, 0.001, 0.01, 0.2, 2.0]) {
        m.observe('dart_ws_adopt_latency_seconds', v);
      }
      final out = m.render();

      expect(out, contains('# TYPE dart_ws_adopt_latency_seconds histogram'));
      expect(out, contains('dart_ws_adopt_latency_seconds_count 5'));
      // A `_sum` line is emitted (exact float text left unasserted to avoid
      // IEEE-754 shortest-repr fragility).
      expect(out, contains('dart_ws_adopt_latency_seconds_sum '));
      // +Inf bucket is cumulative and equals the count.
      expect(out, contains('dart_ws_adopt_latency_seconds_bucket{le="+Inf"} 5'));
      // The le="0.001" bucket should hold the two 0.001 samples cumulatively.
      expect(out, contains('dart_ws_adopt_latency_seconds_bucket{le="0.001"} 2'));
    });

    test('histogramQuantile is monotone and within range', () {
      final m = Metrics();
      for (var i = 0; i < 100; i++) {
        m.observe('lat', 0.01 + i * 0.001); // 0.01 .. 0.109
      }
      final p50 = m.histogramQuantile('lat', 0.5);
      final p99 = m.histogramQuantile('lat', 0.99);
      expect(p50, greaterThan(0));
      expect(p99, greaterThanOrEqualTo(p50));
      expect(m.histogramQuantile('does_not_exist', 0.9), 0);
    });

    test('counter and gauge accessors read back current values', () {
      final m = Metrics();
      m.inc('dart_test_total');
      m.inc('dart_test_total', 4);
      expect(m.counter('dart_test_total'), 5);
      expect(m.counter('never_set'), 0);

      var live = 7;
      m.registerGauge('dart_test_gauge', () => live);
      expect(m.gauge('dart_test_gauge'), 7);
      live = 9;
      expect(m.gauge('dart_test_gauge'), 9);
      expect(m.gauge('missing_gauge'), isNull);
    });

    test('raw exposition providers are appended to render', () {
      final m = Metrics();
      m.registerRawExposition(() => '# custom\ndart_custom_metric 1\n');
      expect(m.render(), contains('dart_custom_metric 1'));
    });
  });
}
