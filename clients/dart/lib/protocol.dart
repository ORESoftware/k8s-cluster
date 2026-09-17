// Dart port of `src/protocol.rs`. The Request/Response surface uses *sealed
// classes* so a switch on `e.runtimeType` (or pattern matching) is checked
// for exhaustiveness by the analyzer — the Dart analogue of the Rust serde
// tagged enum and the property the upstream live-mutex library lacks.

import 'dart:convert';

// ---------- Request ------------------------------------------------------

sealed class Request {
  const Request({required this.uuid});
  final String uuid;
  String get type;
  Map<String, dynamic> toJson();

  String encode() => '${jsonEncode(toJson())}\n';
}

class VersionRequest extends Request {
  const VersionRequest({required super.uuid, required this.value});
  final String value;
  @override
  String get type => 'version';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'value': value};
}

class AuthRequest extends Request {
  const AuthRequest({required super.uuid, required this.token});
  final String token;
  @override
  String get type => 'auth';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'token': token};
}

class LockRequest extends Request {
  const LockRequest({
    required super.uuid,
    this.key,
    this.keys,
    this.pid,
    this.ttl = 30000,
    this.max,
    this.force = false,
    this.retryCount = 0,
    this.keepLocksAfterDeath = false,
    this.wait,
  });
  final String? key;
  final List<String>? keys;
  final int? pid;
  final int ttl;
  final int? max;
  final bool force;
  final int retryCount;
  final bool keepLocksAfterDeath;
  final bool? wait;

  @override
  String get type => 'lock';
  @override
  Map<String, dynamic> toJson() {
    final m = <String, dynamic>{
      'type': type,
      'uuid': uuid,
      'force': force,
      'retryCount': retryCount,
      'keepLocksAfterDeath': keepLocksAfterDeath,
      'ttl': ttl,
    };
    if (key != null) m['key'] = key;
    if (keys != null) m['keys'] = keys;
    if (pid != null) m['pid'] = pid;
    if (max != null) m['max'] = max;
    if (wait != null) m['wait'] = wait;
    return m;
  }
}

class UnlockRequest extends Request {
  const UnlockRequest({
    required super.uuid,
    this.key,
    this.keys,
    this.lockUuid,
    this.force = false,
  });
  final String? key;
  final List<String>? keys;
  final String? lockUuid;
  final bool force;

  @override
  String get type => 'unlock';
  @override
  Map<String, dynamic> toJson() {
    final m = <String, dynamic>{'type': type, 'uuid': uuid, 'force': force};
    if (key != null) m['key'] = key;
    if (keys != null) m['keys'] = keys;
    if (lockUuid != null) m['lockUuid'] = lockUuid;
    return m;
  }
}

class RegisterReadRequest extends Request {
  const RegisterReadRequest({required super.uuid, required this.key});
  final String key;
  @override
  String get type => 'registerRead';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'key': key};
}

class RegisterWriteRequest extends Request {
  const RegisterWriteRequest({required super.uuid, required this.key});
  final String key;
  @override
  String get type => 'registerWrite';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'key': key};
}

class EndReadRequest extends Request {
  const EndReadRequest({required super.uuid, required this.key});
  final String key;
  @override
  String get type => 'endRead';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'key': key};
}

class EndWriteRequest extends Request {
  const EndWriteRequest({required super.uuid, required this.key});
  final String key;
  @override
  String get type => 'endWrite';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'key': key};
}

class LockInfoRequest extends Request {
  const LockInfoRequest({required super.uuid, required this.key});
  final String key;
  @override
  String get type => 'lockInfo';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid, 'key': key};
}

class LsRequest extends Request {
  const LsRequest({required super.uuid});
  @override
  String get type => 'ls';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid};
}

class HeartbeatRequest extends Request {
  const HeartbeatRequest({required super.uuid});
  @override
  String get type => 'heartbeat';
  @override
  Map<String, dynamic> toJson() => {'type': type, 'uuid': uuid};
}

// ---------- Response -----------------------------------------------------

sealed class Response {
  const Response({required this.uuid});
  final String uuid;
  String get type;

  static Response decode(String line) {
    final j = jsonDecode(line) as Map<String, dynamic>;
    final t = j['type'] as String;
    final u = j['uuid'] as String? ?? '';
    switch (t) {
      case 'version':
        return VersionResponse(
          uuid: u,
          brokerVersion: j['brokerVersion'] as String? ?? '',
          ok: j['ok'] as bool? ?? false,
          error: j['error'] as String?,
        );
      case 'auth':
        return AuthResponse(
          uuid: u,
          ok: j['ok'] as bool? ?? false,
          error: j['error'] as String?,
        );
      case 'lock':
        return LockResponse(
          uuid: u,
          key: j['key'] as String? ?? '',
          acquired: j['acquired'] as bool? ?? false,
          lockRequestCount: (j['lockRequestCount'] as num?)?.toInt() ?? 0,
          lockUuid: j['lockUuid'] as String?,
          fencingToken: (j['fencingToken'] as num?)?.toInt(),
          readersCount: (j['readersCount'] as num?)?.toInt(),
          error: j['error'] as String?,
        );
      case 'compositeLock':
        return CompositeLockResponse(
          uuid: u,
          keys: ((j['keys'] as List?) ?? const [])
              .map((e) => e as String)
              .toList(),
          acquired: j['acquired'] as bool? ?? false,
          lockUuid: j['lockUuid'] as String?,
          fencingTokens: (j['fencingTokens'] as Map?)?.map(
            (k, v) => MapEntry(k as String, (v as num).toInt()),
          ),
          error: j['error'] as String?,
        );
      case 'unlock':
        return UnlockResponse(
          uuid: u,
          keys: ((j['keys'] as List?) ?? const [])
              .map((e) => e as String)
              .toList(),
          unlocked: j['unlocked'] as bool? ?? false,
          lockRequestCount: (j['lockRequestCount'] as num?)?.toInt() ?? 0,
          error: j['error'] as String?,
        );
      case 'registerReadResult':
        return RegisterReadResultResponse(
          uuid: u,
          key: j['key'] as String? ?? '',
          readersCount: (j['readersCount'] as num?)?.toInt() ?? 0,
          writerFlag: j['writerFlag'] as bool? ?? false,
          granted: j['granted'] as bool? ?? false,
          lockUuid: j['lockUuid'] as String?,
          fencingToken: (j['fencingToken'] as num?)?.toInt(),
        );
      case 'registerWriteResult':
        return RegisterWriteResultResponse(
          uuid: u,
          key: j['key'] as String? ?? '',
          readersCount: (j['readersCount'] as num?)?.toInt() ?? 0,
          writerFlag: j['writerFlag'] as bool? ?? false,
          granted: j['granted'] as bool? ?? false,
          lockUuid: j['lockUuid'] as String?,
          fencingToken: (j['fencingToken'] as num?)?.toInt(),
        );
      case 'endReadResult':
        return EndReadResultResponse(
          uuid: u,
          key: j['key'] as String? ?? '',
          readersCount: (j['readersCount'] as num?)?.toInt() ?? 0,
        );
      case 'endWriteResult':
        return EndWriteResultResponse(
          uuid: u,
          key: j['key'] as String? ?? '',
          readersCount: (j['readersCount'] as num?)?.toInt() ?? 0,
          writerFlag: j['writerFlag'] as bool? ?? false,
        );
      case 'lockInfo':
        return LockInfoResponse(
          uuid: u,
          key: j['key'] as String? ?? '',
          isLocked: j['isLocked'] as bool? ?? false,
          lockholderUuids: ((j['lockholderUuids'] as List?) ?? const [])
              .map((e) => e as String)
              .toList(),
          lockRequestCount: (j['lockRequestCount'] as num?)?.toInt() ?? 0,
          readersCount: (j['readersCount'] as num?)?.toInt() ?? 0,
          writerFlag: j['writerFlag'] as bool? ?? false,
        );
      case 'lsResult':
        return LsResultResponse(
          uuid: u,
          keys: ((j['keys'] as List?) ?? const [])
              .map((e) => e as String)
              .toList(),
        );
      case 'reelection':
        return ReelectionResponse(uuid: u, key: j['key'] as String? ?? '');
      case 'error':
        return ErrorResponse(
          uuid: u,
          error: j['error'] as String? ?? 'unknown',
        );
      case 'ok':
        return OkResponse(uuid: u);
      default:
        throw FormatException('unknown response type: $t');
    }
  }
}

class VersionResponse extends Response {
  const VersionResponse({
    required super.uuid,
    required this.brokerVersion,
    required this.ok,
    this.error,
  });
  final String brokerVersion;
  final bool ok;
  final String? error;
  @override
  String get type => 'version';
}

class AuthResponse extends Response {
  const AuthResponse({required super.uuid, required this.ok, this.error});
  final bool ok;
  final String? error;
  @override
  String get type => 'auth';
}

class LockResponse extends Response {
  const LockResponse({
    required super.uuid,
    required this.key,
    required this.acquired,
    required this.lockRequestCount,
    this.lockUuid,
    this.fencingToken,
    this.readersCount,
    this.error,
  });
  final String key;
  final bool acquired;
  final int lockRequestCount;
  final String? lockUuid;
  final int? fencingToken;
  final int? readersCount;
  final String? error;
  @override
  String get type => 'lock';
}

class CompositeLockResponse extends Response {
  const CompositeLockResponse({
    required super.uuid,
    required this.keys,
    required this.acquired,
    this.lockUuid,
    this.fencingTokens,
    this.error,
  });
  final List<String> keys;
  final bool acquired;
  final String? lockUuid;
  final Map<String, int>? fencingTokens;
  final String? error;
  @override
  String get type => 'compositeLock';
}

class UnlockResponse extends Response {
  const UnlockResponse({
    required super.uuid,
    required this.keys,
    required this.unlocked,
    required this.lockRequestCount,
    this.error,
  });
  final List<String> keys;
  final bool unlocked;
  final int lockRequestCount;
  final String? error;
  @override
  String get type => 'unlock';
}

class RegisterReadResultResponse extends Response {
  const RegisterReadResultResponse({
    required super.uuid,
    required this.key,
    required this.readersCount,
    required this.writerFlag,
    required this.granted,
    this.lockUuid,
    this.fencingToken,
  });
  final String key;
  final int readersCount;
  final bool writerFlag;
  final bool granted;
  final String? lockUuid;
  final int? fencingToken;
  @override
  String get type => 'registerReadResult';
}

class RegisterWriteResultResponse extends Response {
  const RegisterWriteResultResponse({
    required super.uuid,
    required this.key,
    required this.readersCount,
    required this.writerFlag,
    required this.granted,
    this.lockUuid,
    this.fencingToken,
  });
  final String key;
  final int readersCount;
  final bool writerFlag;
  final bool granted;
  final String? lockUuid;
  final int? fencingToken;
  @override
  String get type => 'registerWriteResult';
}

class EndReadResultResponse extends Response {
  const EndReadResultResponse({
    required super.uuid,
    required this.key,
    required this.readersCount,
  });
  final String key;
  final int readersCount;
  @override
  String get type => 'endReadResult';
}

class EndWriteResultResponse extends Response {
  const EndWriteResultResponse({
    required super.uuid,
    required this.key,
    required this.readersCount,
    required this.writerFlag,
  });
  final String key;
  final int readersCount;
  final bool writerFlag;
  @override
  String get type => 'endWriteResult';
}

class LockInfoResponse extends Response {
  const LockInfoResponse({
    required super.uuid,
    required this.key,
    required this.isLocked,
    required this.lockholderUuids,
    required this.lockRequestCount,
    required this.readersCount,
    required this.writerFlag,
  });
  final String key;
  final bool isLocked;
  final List<String> lockholderUuids;
  final int lockRequestCount;
  final int readersCount;
  final bool writerFlag;
  @override
  String get type => 'lockInfo';
}

class LsResultResponse extends Response {
  const LsResultResponse({required super.uuid, required this.keys});
  final List<String> keys;
  @override
  String get type => 'lsResult';
}

class ReelectionResponse extends Response {
  const ReelectionResponse({required super.uuid, required this.key});
  final String key;
  @override
  String get type => 'reelection';
}

class ErrorResponse extends Response {
  const ErrorResponse({required super.uuid, required this.error});
  final String error;
  @override
  String get type => 'error';
}

class OkResponse extends Response {
  const OkResponse({required super.uuid});
  @override
  String get type => 'ok';
}
