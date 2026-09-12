import 'package:dd_dart_server/server/postgres.dart';
import 'package:dd_pg_defs/pg_defs.dart' as pg;
import 'package:test/test.dart';

void main() {
  group('normalisePgColumnMap', () {
    test('strips _json suffix introduced by pg-defs SELECT casts', () {
      final raw = <String, dynamic>{
        'value_json': '{"feature":"hot-reload"}',
        'meta_data_json': '{"shard":2}',
        'labels_json': '["a","b"]',
      };
      final out = normalisePgColumnMap(raw);
      expect(out, containsPair('value', '{"feature":"hot-reload"}'));
      expect(out, containsPair('metaData', '{"shard":2}'));
      expect(out, containsPair('labels', '["a","b"]'));
    });

    test('camelCases snake_case columns', () {
      final raw = <String, dynamic>{
        'is_soft_deleted': false,
        'created_at': '2026-05-22T22:00:00Z',
        'created_by': 'op',
      };
      final out = normalisePgColumnMap(raw);
      expect(out['isSoftDeleted'], isFalse);
      expect(out['createdAt'], '2026-05-22T22:00:00Z');
      expect(out['createdBy'], 'op');
    });

    test('produces the exact JSON shape PresenceConvsRow.fromJson expects', () {
      final raw = <String, dynamic>{
        'id': '11111111-1111-4111-8111-111111111111',
        'slug': 'general',
        'display_name': 'General',
        'status': 'active',
        'meta_data_json': '{}',
        'is_soft_deleted': false,
        'created_at': '2026-05-22T22:00:00Z',
        'updated_at': '2026-05-22T22:01:00Z',
        'created_by': null,
        'updated_by': null,
      };
      final row = pg.PresenceConvsRow.fromJson(normalisePgColumnMap(raw));
      expect(row.id, '11111111-1111-4111-8111-111111111111');
      expect(row.slug, 'general');
      expect(row.displayName, 'General');
      expect(row.status, 'active');
      expect(row.metaData, isEmpty);
      expect(row.isSoftDeleted, isFalse);
      expect(row.createdBy, isNull);
      expect(row.validate(), isEmpty);
    });

    test('PresenceConvMembersRow validates role + status enums', () {
      final raw = <String, dynamic>{
        'id': '22222222-2222-4222-8222-222222222222',
        'conv_id': '11111111-1111-4111-8111-111111111111',
        'user_id': '33333333-3333-4333-8333-333333333333',
        'role': 'member',
        'status': 'active',
        'meta_data_json': '{}',
        'is_soft_deleted': false,
        'joined_at': '2026-05-22T22:00:00Z',
        'left_at': null,
        'created_at': '2026-05-22T22:00:00Z',
        'updated_at': '2026-05-22T22:00:00Z',
        'created_by': null,
        'updated_by': null,
      };
      final row = pg.PresenceConvMembersRow.fromJson(normalisePgColumnMap(raw));
      expect(row.validate(), isEmpty);

      final bogus = pg.PresenceConvMembersRow.fromJson({
        ...row.toJson(),
        'role': 'overlord',
        'status': 'haunted',
      });
      expect(bogus.validate(), isNotEmpty);
    });
  });
}
