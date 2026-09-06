// End-to-end smoke test mirroring clients/ts/src/smoke.ts.
//
//   dart run bin/smoke.dart
//
// Override host/port via LIVE_MUTEX_HOST / LIVE_MUTEX_PORT.
import 'dart:io';

import 'package:dd_rust_network_mutex_client/client.dart';

Future<void> main() async {
  final host = Platform.environment['LIVE_MUTEX_HOST'] ?? '127.0.0.1';
  final port = int.parse(Platform.environment['LIVE_MUTEX_PORT'] ?? '6970');
  final client = await NetworkMutexClient.connect(host: host, port: port);
  // ignore: avoid_print
  print('[smoke-dart] connected $host:$port');

  final ex = await client.acquire('smoke-dart-exclusive');
  // ignore: avoid_print
  print('[smoke-dart] exclusive grant: lockUuid=${ex.lockUuid} fencing=${ex.fencingToken}');
  await client.release(ex);
  // ignore: avoid_print
  print('[smoke-dart] released exclusive');

  final comp = await client.acquireMany(['smoke-dart-a', 'smoke-dart-b', 'smoke-dart-c']);
  // ignore: avoid_print
  print('[smoke-dart] composite grant: lockUuid=${comp.lockUuid} tokens=${comp.fencingTokens}');
  await client.release(comp);
  // ignore: avoid_print
  print('[smoke-dart] released composite');

  final w = await client.acquireWrite('smoke-dart-rw');
  // ignore: avoid_print
  print('[smoke-dart] writer grant: id=${w.lockUuid} fencing=${w.fencingToken}');
  await client.releaseWrite('smoke-dart-rw');

  final r1f = client.acquireRead('smoke-dart-rw');
  final r2f = client.acquireRead('smoke-dart-rw');
  final results = await Future.wait([r1f, r2f]);
  // ignore: avoid_print
  print('[smoke-dart] readers: ${results.map((r) => r.fencingToken).toList()}');
  await client.releaseRead('smoke-dart-rw');
  await client.releaseRead('smoke-dart-rw');

  await client.close();
  // ignore: avoid_print
  print('[smoke-dart] OK');
}
