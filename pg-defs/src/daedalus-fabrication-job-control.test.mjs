import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const schemaUrl = new URL('../schema/schema.sql', import.meta.url);

test('Daedalus fabrication execution remains durable, fenced, and resumable', async () => {
  const sql = await readFile(schemaUrl, 'utf8');

  assert.match(sql, /create table if not exists daedalus\.fabrication_job_executions/i);
  assert.match(sql, /checkpoint_version bigint default 0 not null/i);
  assert.match(sql, /checkpoint jsonb default '\{\}'::jsonb not null/i);
  assert.match(sql, /fiducia_fencing_token bigint/i);
  assert.match(sql, /fabrication_job_executions_running_lease_complete/i);
  assert.match(sql, /unique \(tenant_id, idempotency_key\)/i);
  assert.match(sql, /where state in \('queued', 'retry_wait'\)/i);
  assert.match(sql, /where state = 'running'/i);

  assert.match(sql, /create table if not exists daedalus\.fabrication_job_outbox/i);
  assert.match(sql, /subject like 'dd\.remote\.fabrication\.%'/i);
  assert.match(sql, /unique \(message_id\)/i);
  assert.match(sql, /where published_at is null/i);
  assert.match(sql, /references daedalus\.fabrication_job_executions \(job_id\)/i);
});
