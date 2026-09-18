/// Thin repository over `presence_convs` and `presence_conv_members`.
///
/// Demonstrates the intended consumption pattern for pg-defs:
///
///   * SQL strings come straight from pg-defs constants (`*SelectSql`)
///     so we never hand-write a column list. Schema changes flow
///     through the regenerator.
///   * Rows are decoded via the pg-defs `*Row.fromJson` factories after
///     [normalisePgColumnMap] turns Postgres `column_map` keys into the
///     camelCase keys those factories expect.
///   * Validators (`row.validate()`) run on every row pulled out of
///     Postgres, surfacing schema drift as a clear error before the
///     row reaches the rest of the application.
///
/// This is intentionally a tiny surface — the supervisor on the main
/// isolate already owns the in-memory ConversationRegistry; the DB-backed
/// repo is opt-in (only meaningful when `DATABASE_URL` is set) and is
/// here to anchor the pattern for the rest of the service.
library;

import '../server/postgres.dart';
import 'pg_contract.dart';

class PresenceConvsRepo {
  PresenceConvsRepo(this._pool);

  final PgPool _pool;

  /// List active (non-soft-deleted) conversations, newest-first by
  /// `updated_at`. Caller picks the limit; default is conservative.
  Future<List<PresenceConvsRow>> listActive({int limit = 50}) async {
    final sql = '''
$presenceConvsSelectSql
where is_soft_deleted = false
order by updated_at desc
limit @limit
''';
    final rows = await _pool.selectRows<PresenceConvsRow>(
      sql,
      parameters: {'limit': limit},
      decode: PresenceConvsRow.fromJson,
    );
    for (final row in rows) {
      final errors = row.validate();
      if (errors.isNotEmpty) {
        throw FormatException(
          'presence_convs row ${row.id} failed validation: ${errors.join(', ')}',
        );
      }
    }
    return rows;
  }

  /// Look up a conversation by canonical UUID. Returns null when the
  /// row is missing or soft-deleted.
  Future<PresenceConvsRow?> findById(String id) async {
    final sql = '''
$presenceConvsSelectSql
where id = @id::uuid and is_soft_deleted = false
limit 1
''';
    final rows = await _pool.selectRows<PresenceConvsRow>(
      sql,
      parameters: {'id': id},
      decode: PresenceConvsRow.fromJson,
    );
    return rows.isEmpty ? null : rows.first;
  }

  /// Members of a conversation, filtered to currently-active membership
  /// rows only (status = 'active' and not soft-deleted).
  Future<List<PresenceConvMembersRow>> activeMembers(String convId) async {
    final sql = '''
$presenceConvMembersSelectSql
where conv_id = @conv_id::uuid
  and is_soft_deleted = false
  and status = 'active'
order by joined_at asc
''';
    final rows = await _pool.selectRows<PresenceConvMembersRow>(
      sql,
      parameters: {'conv_id': convId},
      decode: PresenceConvMembersRow.fromJson,
    );
    for (final row in rows) {
      final errors = row.validate();
      if (errors.isNotEmpty) {
        throw FormatException(
          'presence_conv_members row ${row.id} failed validation: '
          '${errors.join(', ')}',
        );
      }
    }
    return rows;
  }

  /// Pull the most recent `presence_events` rows for outbox-style
  /// fan-out. Used by the (forthcoming) consumer that pumps remote
  /// changes into the in-memory ConversationRegistry on the main
  /// isolate.
  Future<List<PresenceEventsRow>> recentEvents({
    required int afterSeq,
    int limit = 256,
  }) async {
    final sql = '''
$presenceEventsSelectSql
where seq > @after_seq
order by seq asc
limit @limit
''';
    return _pool.selectRows<PresenceEventsRow>(
      sql,
      parameters: {'after_seq': afterSeq, 'limit': limit},
      decode: PresenceEventsRow.fromJson,
    );
  }

  /// Read the last-applied `seq` for a given consumer id (e.g.
  /// `dart-server:<pod-name>`). Returns 0 when no checkpoint exists.
  Future<int> consumerCheckpoint(String consumerId) async {
    final sql = '''
$presenceConsumerCheckpointsSelectSql
where consumer_id = @consumer_id
limit 1
''';
    final rows = await _pool.selectRows<PresenceConsumerCheckpointsRow>(
      sql,
      parameters: {'consumer_id': consumerId},
      decode: PresenceConsumerCheckpointsRow.fromJson,
    );
    return rows.isEmpty ? 0 : rows.first.lastSeq;
  }
}
