/// Tiny self-contained WebSocket load tester for dd-dart-server.
///
/// Opens N concurrent WSS connections, drives each one with a steady
/// stream of HTMX-shaped JSON triggers, and measures throughput +
/// latency. Each connection corresponds to one session isolate on the
/// server, so this also exercises the supervisor / isolate spawn path.
///
/// Defaults are conservative; tune via env or CLI:
///   BENCH_WSS_URL          ws://127.0.0.1:8089/dart/wss
///   BENCH_WSS_CONNS        128
///   BENCH_WSS_RATE         50      (msg/s/connection)
///   BENCH_WSS_DURATION     30      (seconds)
///   BENCH_WSS_WARMUP       3       (seconds)
///   BENCH_WSS_TRIGGER      bump    (one of: bump, echo, say)
///
/// Output is line-delimited JSON on stdout; progress goes to stderr.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

void main(List<String> args) async {
  final cfg = _Config.parse(args);
  stderr.writeln('[wss_loadtest] target=${cfg.url} conns=${cfg.connections} '
      'rate=${cfg.ratePerConn}/s/conn duration=${cfg.duration.inSeconds}s '
      'warmup=${cfg.warmup.inSeconds}s trigger=${cfg.trigger}');

  final results = await _run(cfg);
  stdout.writeln(const JsonEncoder().convert(results));
}

Future<Map<String, Object?>> _run(_Config cfg) async {
  final url = Uri.parse(cfg.url);
  final start = DateTime.now();
  final deadline = start.add(cfg.duration + cfg.warmup);
  final warmupCutoff = start.add(cfg.warmup);

  final connectLatencyUs = <int>[];
  final firstFrameUs = <int>[];
  final messageLatencyUs = <int>[];
  var connected = 0;
  var connectErrors = 0;
  var messagesSent = 0;
  var messagesReceived = 0;
  var bytesReceived = 0;
  var disconnects = 0;

  Future<void> connection(int id) async {
    final connectT0 = DateTime.now();
    WebSocket socket;
    try {
      socket = await WebSocket.connect(url.toString());
    } catch (e) {
      connectErrors++;
      stderr.writeln('[wss_loadtest] conn $id failed: $e');
      return;
    }
    final connectT1 = DateTime.now();
    connected++;
    connectLatencyUs.add(connectT1.difference(connectT0).inMicroseconds);
    var seenFirstFrame = false;

    socket.listen(
      (event) {
        final now = DateTime.now();
        if (!seenFirstFrame) {
          seenFirstFrame = true;
          firstFrameUs.add(now.difference(connectT1).inMicroseconds);
        }
        if (now.isBefore(warmupCutoff)) return;
        messagesReceived++;
        if (event is String) {
          bytesReceived += event.length;
        } else if (event is List<int>) {
          bytesReceived += event.length;
        }
      },
      onDone: () => disconnects++,
      onError: (_) => disconnects++,
      cancelOnError: true,
    );

    final periodMicros = (1e6 / cfg.ratePerConn).round();
    final timer = Timer.periodic(Duration(microseconds: periodMicros), (t) {
      if (DateTime.now().isAfter(deadline)) {
        t.cancel();
        return;
      }
      final payload = _buildTrigger(cfg.trigger, id, messagesSent);
      try {
        socket.add(payload);
        if (DateTime.now().isAfter(warmupCutoff)) {
          messagesSent++;
          // We do not pair sends to receives in this harness — the
          // server multicasts OOB fragments based on the bus + render
          // pipeline. We measure receive-side latency separately via
          // the "first-frame" stat.
          messageLatencyUs.add(0);
        }
      } catch (_) {/* socket closed */}
    });

    await socket.done;
    timer.cancel();
  }

  final futures = <Future<void>>[];
  for (var i = 0; i < cfg.connections; i++) {
    futures.add(connection(i));
    // Stagger connection opens so we don't overwhelm the listen
    // backlog on the server.
    await Future<void>.delayed(Duration(microseconds: cfg.staggerMicros));
  }

  await Future.delayed(cfg.duration + cfg.warmup);
  final wall =
      DateTime.now().difference(warmupCutoff).inMicroseconds / 1e6;

  // Drain anything still hanging by closing all connections.
  // (We let the natural socket.done fire from the server side too.)

  Map<String, Object?> percentiles(List<int> samples) {
    if (samples.isEmpty) return const {};
    final sorted = [...samples]..sort();
    int p(double q) {
      final idx = (sorted.length * q).floor().clamp(0, sorted.length - 1);
      return sorted[idx];
    }

    return {
      'p50_us': p(0.5),
      'p90_us': p(0.9),
      'p95_us': p(0.95),
      'p99_us': p(0.99),
      'max_us': sorted.last,
    };
  }

  return <String, Object?>{
    'kind': 'wss_loadtest',
    'config': cfg.toJson(),
    'wall_seconds': wall,
    'connected': connected,
    'connect_errors': connectErrors,
    'disconnects': disconnects,
    'messages_sent': messagesSent,
    'messages_received': messagesReceived,
    'bytes_received': bytesReceived,
    'send_rps': wall > 0 ? messagesSent / wall : 0,
    'recv_rps': wall > 0 ? messagesReceived / wall : 0,
    'mb_per_s_recv': wall > 0 ? (bytesReceived / wall / 1024 / 1024) : 0,
    'connect_latency': percentiles(connectLatencyUs),
    'first_frame_latency': percentiles(firstFrameUs),
  };
}

String _buildTrigger(String trigger, int connId, int seq) {
  final r = Random(connId ^ seq);
  switch (trigger) {
    case 'bump':
      return jsonEncode({
        'HEADERS': {'HX-Trigger-Name': 'bump'},
      });
    case 'reset':
      return jsonEncode({
        'HEADERS': {'HX-Trigger-Name': 'reset'},
      });
    case 'echo':
      return jsonEncode({
        'message': 'hello-${connId}-${seq}',
        'HEADERS': {'HX-Trigger-Name': 'echo'},
      });
    case 'say':
      return jsonEncode({
        'text': 'msg-${connId}-${seq}-${r.nextInt(1 << 31)}',
        'HEADERS': {'HX-Trigger-Name': 'say'},
      });
    default:
      throw ArgumentError('unknown trigger: $trigger');
  }
}

class _Config {
  _Config({
    required this.url,
    required this.connections,
    required this.ratePerConn,
    required this.duration,
    required this.warmup,
    required this.trigger,
    required this.staggerMicros,
  });

  final String url;
  final int connections;
  final int ratePerConn;
  final Duration duration;
  final Duration warmup;
  final String trigger;
  final int staggerMicros;

  Map<String, Object?> toJson() => {
        'url': url,
        'connections': connections,
        'rate_per_conn': ratePerConn,
        'duration_s': duration.inSeconds,
        'warmup_s': warmup.inSeconds,
        'trigger': trigger,
        'stagger_us': staggerMicros,
      };

  static _Config parse(List<String> args) {
    var url = Platform.environment['BENCH_WSS_URL'] ??
        'ws://127.0.0.1:8089/dart/wss';
    var connections =
        int.tryParse(Platform.environment['BENCH_WSS_CONNS'] ?? '') ?? 128;
    var rate = int.tryParse(Platform.environment['BENCH_WSS_RATE'] ?? '') ?? 50;
    var duration = Duration(
        seconds:
            int.tryParse(Platform.environment['BENCH_WSS_DURATION'] ?? '') ??
                30);
    var warmup = Duration(
        seconds:
            int.tryParse(Platform.environment['BENCH_WSS_WARMUP'] ?? '') ?? 3);
    var trigger = Platform.environment['BENCH_WSS_TRIGGER'] ?? 'bump';
    var stagger =
        int.tryParse(Platform.environment['BENCH_WSS_STAGGER_US'] ?? '') ?? 500;

    for (var i = 0; i < args.length; i++) {
      final a = args[i];
      switch (a) {
        case '--url':
          url = args[++i];
        case '--connections':
        case '-c':
          connections = int.parse(args[++i]);
        case '--rate':
        case '-r':
          rate = int.parse(args[++i]);
        case '--duration':
        case '-d':
          duration = Duration(seconds: int.parse(args[++i]));
        case '--warmup':
          warmup = Duration(seconds: int.parse(args[++i]));
        case '--trigger':
        case '-t':
          trigger = args[++i];
        case '--stagger-us':
          stagger = int.parse(args[++i]);
        case '--help':
        case '-h':
          stderr.writeln('usage: dart run tools/wss_loadtest.dart '
              '[--url URL] [-c N] [-r RATE] [-d SECS] [--warmup SECS] [-t TRIG]');
          exit(0);
      }
    }

    return _Config(
      url: url,
      connections: connections,
      ratePerConn: rate,
      duration: duration,
      warmup: warmup,
      trigger: trigger,
      staggerMicros: stagger,
    );
  }
}
