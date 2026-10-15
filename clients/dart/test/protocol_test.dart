import 'package:dd_rust_network_mutex_client/protocol.dart';
import 'package:test/test.dart';

void main() {
  test('LockRequest encodes camelCase fields', () {
    final req = LockRequest(
      uuid: 'u',
      keys: ['a', 'b'],
      ttl: 1000,
      wait: false,
    );
    final s = req.encode();
    expect(s, contains('"type":"lock"'));
    expect(s, contains('"keys":["a","b"]'));
    expect(s, contains('"keepLocksAfterDeath":false'));
    expect(s, contains('"retryCount":0'));
    expect(s, contains('"wait":false'));
  });

  test('LockRequest preserves wait true and omits absent wait', () {
    final waitTrue = LockRequest(uuid: 'u1', key: 'k', wait: true).encode();
    final omitted = LockRequest(uuid: 'u2', key: 'k').encode();

    expect(waitTrue, contains('"wait":true'));
    expect(omitted, isNot(contains('"wait"')));
  });

  test('Response.decode parses compositeLock', () {
    final r = Response.decode(
      '{"type":"compositeLock","uuid":"u","keys":["a","b"],"acquired":true,"lockUuid":"L","fencingTokens":{"a":1,"b":2}}',
    );
    expect(r, isA<CompositeLockResponse>());
    final c = r as CompositeLockResponse;
    expect(c.lockUuid, 'L');
    expect(c.fencingTokens?['b'], 2);
  });

  test('exhaustive switch on Response', () {
    String dispatch(Response r) {
      return switch (r) {
        VersionResponse() => 'version',
        AuthResponse() => 'auth',
        LockResponse() => 'lock',
        CompositeLockResponse() => 'compositeLock',
        UnlockResponse() => 'unlock',
        RegisterReadResultResponse() => 'registerReadResult',
        RegisterWriteResultResponse() => 'registerWriteResult',
        EndReadResultResponse() => 'endReadResult',
        EndWriteResultResponse() => 'endWriteResult',
        LockInfoResponse() => 'lockInfo',
        LsResultResponse() => 'lsResult',
        ReelectionResponse() => 'reelection',
        ErrorResponse() => 'error',
        OkResponse() => 'ok',
      };
    }

    expect(dispatch(const OkResponse(uuid: 'x')), 'ok');
  });
}
