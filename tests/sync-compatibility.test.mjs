import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import test from 'node:test';

const syncSource = new URL(
  '../apps/daedalus-sync/typescript/dist/src/index.js',
  import.meta.url,
);
const clientSource = new URL(
  '../apps/daedalus-clients/clients/typescript/daedalus.ts',
  import.meta.url,
);
const interfacesSource = new URL(
  '../apps/daedalus-interfaces/generated/typescript/index.ts',
  import.meta.url,
);
const fleetAvailable = [syncSource, clientSource, interfacesSource].every((url) =>
  existsSync(url),
);

test(
  'daedalus-sync stores actual client and generated-interface documents',
  { skip: fleetAvailable ? false : 'private fleet submodules are not initialized' },
  async () => {
    const { MemorySyncStore } = await import(syncSource.href);
    const { PLANS_SYNC_COLLECTION, planToSyncDocument } = await import(clientSource.href);
    const { daedalusClientLogEntriesTable } = await import(interfacesSource.href);

    const store = new MemorySyncStore('operator-1', 'browser-1');
    const plan = {
      id: 'plan-1',
      title: 'fixture',
      goal: 'machine the integration fixture',
      process_family: 'subtractive',
      status: 'draft',
      created_at: '2026-07-22T12:00:00Z',
      updated_at: '2026-07-22T12:00:00Z',
      owner: 'must-not-sync@example.com',
      token: 'must-not-sync',
    };
    const logEntry = {
      id: 'entry-1',
      environment: 'test',
      level: 'info',
      message: 'plan cached',
      source: 'client',
      metadata: { plan_id: plan.id },
      client_timestamp: '2026-07-22T12:00:01Z',
      created_at: '2026-07-22T12:00:01Z',
      is_soft_deleted: false,
    };

    const planChange = await store.put(
      PLANS_SYNC_COLLECTION,
      plan.id,
      planToSyncDocument(plan),
      1000,
    );
    const interfaceChange = await store.put(
      daedalusClientLogEntriesTable,
      logEntry.id,
      logEntry,
      1001,
    );

    assert.equal(planChange.collection, 'plans');
    assert.equal(planChange.payload.process_family, 'subtractive');
    assert.equal(planChange.payload.owner, undefined);
    assert.equal(planChange.payload.token, undefined);
    assert.equal(interfaceChange.collection, 'daedalus_client_log_entries');
    assert.deepEqual(interfaceChange.payload.metadata, { plan_id: 'plan-1' });
  },
);
