import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dd_rust_network_mutex_client/client.dart';

final host = Platform.environment['LIVE_MUTEX_HOST'] ?? '127.0.0.1';
final port = int.parse(Platform.environment['LIVE_MUTEX_PORT'] ?? '6970');
final lang = Platform.environment['LMX_WORKER_LANG'] ?? 'dart';
final worker = Platform.environment['LMX_WORKER_ID'] ?? '$lang-0';
final seed = BigInt.parse(Platform.environment['LMX_WORKER_SEED'] ?? '1');
final ops = int.parse(Platform.environment['LMX_WORKER_OPS'] ?? '50');
final keyPrefix = Platform.environment['LMX_FUZZ_KEY_PREFIX'] ?? 'cross';
final keyCount = int.parse(Platform.environment['LMX_FUZZ_KEY_COUNT'] ?? '5');

const ttl = Duration(milliseconds: 60000);

class Rng {
  Rng(BigInt seed) : _x = seed ^ BigInt.parse('9e3779b97f4a7c15', radix: 16) {
    if (_x == BigInt.zero) _x = BigInt.one;
  }

  BigInt _x;

  int below(int n) {
    _x ^= _x >> 12;
    _x ^= _x << 25;
    _x ^= _x >> 27;
    _x &= (BigInt.one << 64) - BigInt.one;
    return ((_x * BigInt.parse('2545f4914f6cdd1d', radix: 16)) % BigInt.from(n))
        .toInt();
  }
}

final _ackQueue = <String>[];
final _ackWaiters = <Completer<String>>[];

StreamSubscription<String> _startAckReader() {
  return stdin.transform(utf8.decoder).transform(const LineSplitter()).listen((
    line,
  ) {
    if (_ackWaiters.isNotEmpty) {
      _ackWaiters.removeAt(0).complete(line);
    } else {
      _ackQueue.add(line);
    }
  });
}

Future<void> _waitAck() async {
  final line = _ackQueue.isNotEmpty
      ? _ackQueue.removeAt(0)
      : await (() {
          final c = Completer<String>();
          _ackWaiters.add(c);
          return c.future;
        })();
  if (line.trim() != 'ack') {
    throw StateError('expected ack from harness, got ${jsonEncode(line)}');
  }
}

Future<void> emit(Map<String, Object?> event) async {
  stdout.writeln(jsonEncode(event));
  await stdout.flush();
  await _waitAck();
}

List<String> chooseKeys(Rng rng, List<String> keys) {
  final want = 2 + rng.below(keys.length > 3 ? 3 : keys.length - 1);
  final pool = [...keys];
  final out = <String>[];
  for (var i = 0; i < want && pool.isNotEmpty; i++) {
    out.add(pool.removeAt(rng.below(pool.length)));
  }
  out.sort();
  return out.toSet().toList();
}

Future<void> holdBriefly(Rng rng) async {
  final ms = rng.below(4);
  if (ms > 0) await Future<void>.delayed(Duration(milliseconds: ms));
}

Future<void> grantReleaseExclusive(
  NetworkMutexClient client,
  Object handle,
  Rng rng,
) async {
  late final String lockUuid;
  late final List<String> keys;
  late final Map<String, int> tokens;
  if (handle is SingleLockHandle) {
    lockUuid = handle.lockUuid;
    keys = [handle.key];
    tokens = {handle.key: handle.fencingToken};
  } else if (handle is CompositeLockHandle) {
    lockUuid = handle.lockUuid;
    keys = handle.keys;
    tokens = handle.fencingTokens;
  } else {
    throw ArgumentError('unknown handle ${handle.runtimeType}');
  }
  await emit({
    'event': 'grant',
    'lang': lang,
    'worker': worker,
    'lockUuid': lockUuid,
    'kind': 'exclusive',
    'keys': keys,
    'tokens': tokens,
  });
  await holdBriefly(rng);
  await emit({
    'event': 'release',
    'lang': lang,
    'worker': worker,
    'lockUuid': lockUuid,
  });
  await client.release(handle);
}

Future<void> main() async {
  final ackReader = _startAckReader();
  final rng = Rng(seed);
  final keys = List.generate(keyCount, (i) => '$keyPrefix-$i');
  final client = await NetworkMutexClient.connect(host: host, port: port);
  try {
    for (var op = 0; op < ops; op++) {
      final roll = rng.below(100);
      if (roll < 30) {
        final h = await client.tryAcquire(
          keys[rng.below(keys.length)],
          ttl: ttl,
        );
        if (h != null) await grantReleaseExclusive(client, h, rng);
      } else if (roll < 50) {
        final h = await client.acquire(keys[rng.below(keys.length)], ttl: ttl);
        await grantReleaseExclusive(client, h, rng);
      } else if (roll < 65) {
        final h = await client.tryAcquireMany(chooseKeys(rng, keys), ttl: ttl);
        if (h != null) await grantReleaseExclusive(client, h, rng);
      } else if (roll < 75) {
        final h = await client.acquireMany(chooseKeys(rng, keys), ttl: ttl);
        await grantReleaseExclusive(client, h, rng);
      } else if (roll < 92) {
        final key = keys[rng.below(keys.length)];
        final h = await client.acquireRead(key);
        await emit({
          'event': 'grant',
          'lang': lang,
          'worker': worker,
          'lockUuid': h.lockUuid,
          'kind': 'read',
          'keys': [key],
          'tokens': {key: h.fencingToken},
        });
        await holdBriefly(rng);
        await emit({
          'event': 'release',
          'lang': lang,
          'worker': worker,
          'lockUuid': h.lockUuid,
        });
        await client.releaseRead(key);
      } else {
        final key = keys[rng.below(keys.length)];
        final h = await client.acquireWrite(key);
        await emit({
          'event': 'grant',
          'lang': lang,
          'worker': worker,
          'lockUuid': h.lockUuid,
          'kind': 'write',
          'keys': [key],
          'tokens': {key: h.fencingToken},
        });
        await holdBriefly(rng);
        await emit({
          'event': 'release',
          'lang': lang,
          'worker': worker,
          'lockUuid': h.lockUuid,
        });
        await client.releaseWrite(key);
      }
    }
  } finally {
    await client.close();
    await ackReader.cancel();
  }
  stdout.writeln(
    jsonEncode({'event': 'done', 'lang': lang, 'worker': worker, 'ops': ops}),
  );
  await stdout.flush();
}
