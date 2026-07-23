import 'package:daedalus_client/daedalus_client.dart';
import 'package:daedalus_interfaces/daedalus_interfaces.dart';
import 'package:daedalus_sync/daedalus_sync.dart';
import 'package:test/test.dart';

void main() {
  test('actual client and interface packages share the sync document boundary',
      () async {
    final store = MemorySyncStore(scope: 'operator-1', actorId: 'flutter-1');
    const plan = Plan(
      id: 'plan-1',
      title: 'fixture',
      goal: 'machine the integration fixture',
      processFamily: 'subtractive',
      status: 'draft',
      createdAt: '2026-07-22T12:00:00Z',
      updatedAt: '2026-07-22T12:00:00Z',
    );
    const logEntry = DaedalusClientLogEntry(
      id: 'entry-1',
      sessionId: 'session-1',
      environment: 'test',
      level: 'info',
      message: 'plan cached',
      source: 'client',
      category: 'sync.compatibility',
      metadata: <String, Object?>{'plan_id': 'plan-1'},
      clientTimestamp: '2026-07-22T12:00:01Z',
      createdAt: '2026-07-22T12:00:01Z',
      isSoftDeleted: false,
    );

    final planChange = await store.putDocument(
      Plan.syncCollection,
      plan.id,
      plan,
      nowMs: 1000,
    );
    final logChange = await store.putDocument(
      daedalusClientLogEntriesTable,
      logEntry.id,
      logEntry,
      nowMs: 1001,
    );

    expect(planChange.collection, 'plans');
    expect(planChange.payload!['process_family'], 'subtractive');
    expect(
      (logChange.payload!['metadata']! as Map<Object?, Object?>)['plan_id'],
      'plan-1',
    );
  });
}
