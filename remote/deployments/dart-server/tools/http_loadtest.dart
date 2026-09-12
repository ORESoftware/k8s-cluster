/// Tiny self-contained HTTP load tester for dd-dart-server.
///
/// Designed to run with no third-party dependencies — `dart run
/// tools/http_loadtest.dart` is enough. Output is line-delimited JSON so
/// scripts/bench.sh can pipe it through jq.
///
/// Defaults are conservative (16 connections × 60s) so a developer
/// laptop doesn't accidentally DoS itself; tweak with the env vars or
/// CLI flags below.
///
/// Usage:
///   dart run tools/http_loadtest.dart \
///     --url http://127.0.0.1:8089/healthz \
///     --connections 32 \
///     --duration 30
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

void main(List<String> args) async {
  final cfg = _Config.parse(args);
  stderr.writeln('[http_loadtest] target=${cfg.url} '
      'connections=${cfg.connections} duration=${cfg.duration.inSeconds}s '
      'warmup=${cfg.warmup.inSeconds}s');

  final results = await _run(cfg);
  stdout.writeln(const JsonEncoder().convert(results));
}

Future<Map<String, Object?>> _run(_Config cfg) async {
  final url = Uri.parse(cfg.url);
  final scheme = url.scheme;
  if (scheme != 'http' && scheme != 'https') {
    throw ArgumentError('only http/https supported, got $scheme');
  }
  final port = url.hasPort ? url.port : (scheme == 'https' ? 443 : 80);

  final stop = Completer<void>();
  final latenciesUs = <int>[];
  var requests = 0;
  var failures = 0;
  var totalBytes = 0;
  final start = DateTime.now();
  final deadline = start.add(cfg.duration + cfg.warmup);
  final warmupCutoff = start.add(cfg.warmup);

  Future<void> worker(int id) async {
    final client = HttpClient()
      ..connectionTimeout = const Duration(seconds: 5)
      ..idleTimeout = const Duration(seconds: 30);
    while (!stop.isCompleted && DateTime.now().isBefore(deadline)) {
      final t0 = DateTime.now();
      try {
        final req = await client.openUrl(cfg.method, url);
        req.headers.set('host', '${url.host}:$port');
        req.headers.set('connection', 'keep-alive');
        if (cfg.body != null) {
          req.add(utf8.encode(cfg.body!));
        }
        final resp = await req.close();
        final bytes = await resp.fold<int>(0, (a, c) => a + c.length);
        final t1 = DateTime.now();
        if (resp.statusCode >= 400) {
          failures++;
        } else {
          if (DateTime.now().isAfter(warmupCutoff)) {
            requests++;
            totalBytes += bytes;
            latenciesUs.add(t1.difference(t0).inMicroseconds);
          }
        }
      } catch (_) {
        failures++;
      }
    }
    client.close(force: true);
  }

  final workers = [
    for (var i = 0; i < cfg.connections; i++) worker(i),
  ];

  unawaited(Future.delayed(cfg.duration + cfg.warmup, () {
    if (!stop.isCompleted) stop.complete();
  }));

  await Future.wait(workers);
  final wall = DateTime.now().difference(warmupCutoff).inMicroseconds / 1e6;
  latenciesUs.sort();

  Map<String, Object?> percentiles() {
    if (latenciesUs.isEmpty) return const {};
    int p(double q) {
      final idx = (latenciesUs.length * q).floor().clamp(0, latenciesUs.length - 1);
      return latenciesUs[idx];
    }

    return {
      'p50_us': p(0.5),
      'p90_us': p(0.9),
      'p95_us': p(0.95),
      'p99_us': p(0.99),
      'p999_us': p(0.999),
      'max_us': latenciesUs.last,
    };
  }

  return <String, Object?>{
    'kind': 'http_loadtest',
    'config': cfg.toJson(),
    'wall_seconds': wall,
    'requests': requests,
    'failures': failures,
    'bytes': totalBytes,
    'rps': wall > 0 ? requests / wall : 0,
    'mb_per_s': wall > 0 ? (totalBytes / wall / 1024 / 1024) : 0,
    'latency': percentiles(),
  };
}

class _Config {
  _Config({
    required this.url,
    required this.connections,
    required this.duration,
    required this.warmup,
    required this.method,
    required this.body,
  });

  final String url;
  final int connections;
  final Duration duration;
  final Duration warmup;
  final String method;
  final String? body;

  Map<String, Object?> toJson() => {
        'url': url,
        'connections': connections,
        'duration_s': duration.inSeconds,
        'warmup_s': warmup.inSeconds,
        'method': method,
        'body_bytes': body?.length ?? 0,
      };

  static _Config parse(List<String> args) {
    String url = Platform.environment['BENCH_URL'] ??
        'http://127.0.0.1:8089/healthz';
    var connections =
        int.tryParse(Platform.environment['BENCH_CONNS'] ?? '') ?? 16;
    var duration = Duration(
        seconds: int.tryParse(Platform.environment['BENCH_DURATION'] ?? '') ??
            30);
    var warmup = Duration(
        seconds: int.tryParse(Platform.environment['BENCH_WARMUP'] ?? '') ?? 3);
    var method = 'GET';
    String? body;

    for (var i = 0; i < args.length; i++) {
      final a = args[i];
      switch (a) {
        case '--url':
          url = args[++i];
        case '--connections':
        case '-c':
          connections = int.parse(args[++i]);
        case '--duration':
        case '-d':
          duration = Duration(seconds: int.parse(args[++i]));
        case '--warmup':
          warmup = Duration(seconds: int.parse(args[++i]));
        case '--method':
        case '-X':
          method = args[++i].toUpperCase();
        case '--body':
        case '-b':
          body = args[++i];
        case '--help':
        case '-h':
          stderr.writeln('usage: dart run tools/http_loadtest.dart '
              '[--url URL] [-c N] [-d SECS] [--warmup SECS] [-X METHOD] [-b BODY]');
          exit(0);
      }
    }

    return _Config(
      url: url,
      connections: connections,
      duration: duration,
      warmup: warmup,
      method: method,
      body: body,
    );
  }
}
