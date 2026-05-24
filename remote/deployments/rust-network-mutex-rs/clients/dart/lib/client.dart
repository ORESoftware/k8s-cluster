import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'protocol.dart';

class SingleLockHandle {
  SingleLockHandle({required this.key, required this.lockUuid, required this.fencingToken});
  final String key;
  final String lockUuid;
  final int fencingToken;
}

class CompositeLockHandle {
  CompositeLockHandle({required this.keys, required this.lockUuid, required this.fencingTokens});
  final List<String> keys;
  final String lockUuid;
  final Map<String, int> fencingTokens;
}

class _Inflight {
  _Inflight(this.completer, {required this.multi});
  final Completer<Response> completer;
  bool multi;
  final List<Response> seen = [];
}

/// TCP client. One [NetworkMutexClient] is safe for concurrent use; many
/// async tasks can call [acquire] / [release] simultaneously, all sharing
/// the same socket. Multiplexing is correlated by the `uuid` field.
class NetworkMutexClient {
  NetworkMutexClient._(this._socket);

  final Socket _socket;
  final Map<String, _Inflight> _inflight = {};
  final _random = Random.secure();
  String _buffer = '';

  static Future<NetworkMutexClient> connect({
    String host = '127.0.0.1',
    int port = 6970,
    String? token,
    Duration timeout = const Duration(seconds: 5),
  }) async {
    final sock = await Socket.connect(host, port, timeout: timeout);
    sock.setOption(SocketOption.tcpNoDelay, true);
    final c = NetworkMutexClient._(sock);
    c._listen();
    if (token != null) {
      final resp = await c._send(AuthRequest(uuid: c._newUuid(), token: token));
      if (resp is! AuthResponse || !resp.ok) {
        await c.close();
        throw StateError('auth failed');
      }
    }
    return c;
  }

  Future<void> close() async {
    try {
      await _socket.close();
    } catch (_) {}
  }

  Future<SingleLockHandle> acquire(String key, {Duration ttl = const Duration(seconds: 30)}) async {
    final req = LockRequest(uuid: _newUuid(), key: key, ttl: ttl.inMilliseconds);
    final resp = await _sendUntilGrant(req);
    if (resp is! LockResponse || !resp.acquired || resp.lockUuid == null) {
      throw StateError('acquire($key) failed: $resp');
    }
    return SingleLockHandle(key: key, lockUuid: resp.lockUuid!, fencingToken: resp.fencingToken ?? 0);
  }

  Future<CompositeLockHandle> acquireMany(List<String> keys, {Duration ttl = const Duration(seconds: 30)}) async {
    if (keys.isEmpty || keys.length > 5) {
      throw ArgumentError.value(keys.length, 'keys.length', 'must be 1..=5');
    }
    final req = LockRequest(uuid: _newUuid(), keys: keys, ttl: ttl.inMilliseconds);
    final resp = await _sendUntilGrant(req);
    if (resp is! CompositeLockResponse || !resp.acquired || resp.lockUuid == null) {
      throw StateError('acquireMany($keys) failed: $resp');
    }
    return CompositeLockHandle(keys: keys, lockUuid: resp.lockUuid!, fencingTokens: resp.fencingTokens ?? const {});
  }

  Future<void> release(Object handle) async {
    UnlockRequest req;
    if (handle is SingleLockHandle) {
      req = UnlockRequest(uuid: _newUuid(), key: handle.key, lockUuid: handle.lockUuid);
    } else if (handle is CompositeLockHandle) {
      req = UnlockRequest(uuid: _newUuid(), keys: handle.keys, lockUuid: handle.lockUuid);
    } else {
      throw ArgumentError('release: unknown handle type ${handle.runtimeType}');
    }
    final resp = await _send(req);
    if (resp is! UnlockResponse || !resp.unlocked) {
      throw StateError('release failed: $resp');
    }
  }

  Future<({String lockUuid, int fencingToken})> acquireRead(String key) async {
    final req = RegisterReadRequest(uuid: _newUuid(), key: key);
    final resp = await _sendUntilGrant(req);
    if (resp is! RegisterReadResultResponse) throw StateError('acquireRead: $resp');
    return (lockUuid: resp.lockUuid ?? '', fencingToken: resp.fencingToken ?? 0);
  }

  Future<void> releaseRead(String key) async {
    await _send(EndReadRequest(uuid: _newUuid(), key: key));
  }

  Future<({String lockUuid, int fencingToken})> acquireWrite(String key) async {
    final req = RegisterWriteRequest(uuid: _newUuid(), key: key);
    final resp = await _sendUntilGrant(req);
    if (resp is! RegisterWriteResultResponse) throw StateError('acquireWrite: $resp');
    return (lockUuid: resp.lockUuid ?? '', fencingToken: resp.fencingToken ?? 0);
  }

  Future<void> releaseWrite(String key) async {
    await _send(EndWriteRequest(uuid: _newUuid(), key: key));
  }

  // -- internals ---------------------------------------------------------

  void _listen() {
    _socket.listen(
      (chunk) {
        _buffer += utf8.decode(chunk);
        while (true) {
          final nl = _buffer.indexOf('\n');
          if (nl < 0) break;
          final line = _buffer.substring(0, nl).trim();
          _buffer = _buffer.substring(nl + 1);
          if (line.isEmpty) continue;
          final resp = Response.decode(line);
          _dispatch(resp);
        }
      },
      onError: (Object err) {
        for (final inf in _inflight.values) {
          if (!inf.completer.isCompleted) inf.completer.completeError(err);
        }
      },
      onDone: () {
        for (final inf in _inflight.values) {
          if (!inf.completer.isCompleted) inf.completer.completeError(StateError('connection closed'));
        }
        _inflight.clear();
      },
    );
  }

  void _dispatch(Response resp) {
    final inf = _inflight[resp.uuid];
    if (inf == null) return;
    if (inf.multi && _isIntermediate(resp)) {
      inf.seen.add(resp);
      return;
    }
    _inflight.remove(resp.uuid);
    if (!inf.completer.isCompleted) inf.completer.complete(resp);
  }

  bool _isIntermediate(Response r) {
    return switch (r) {
      LockResponse(:final acquired, :final error) => !acquired && error == null,
      CompositeLockResponse(:final acquired, :final error) => !acquired && error == null,
      RegisterReadResultResponse(:final granted) => !granted,
      RegisterWriteResultResponse(:final granted) => !granted,
      ReelectionResponse() => true,
      _ => false,
    };
  }

  Future<Response> _send(Request req) {
    final completer = Completer<Response>();
    _inflight[req.uuid] = _Inflight(completer, multi: false);
    _socket.write(req.encode());
    return completer.future;
  }

  Future<Response> _sendUntilGrant(Request req) {
    final completer = Completer<Response>();
    _inflight[req.uuid] = _Inflight(completer, multi: true);
    _socket.write(req.encode());
    return completer.future;
  }

  String _newUuid() {
    final bytes = List<int>.generate(16, (_) => _random.nextInt(256));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    String h(int b) => b.toRadixString(16).padLeft(2, '0');
    final s = bytes.map(h).join();
    return '${s.substring(0, 8)}-${s.substring(8, 12)}-${s.substring(12, 16)}-${s.substring(16, 20)}-${s.substring(20)}';
  }
}
