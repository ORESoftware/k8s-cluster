import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const sql = readFileSync(join(root, 'sql/schema.sql'), 'utf8');

function tableDefinition(tableName) {
  const table = sql.match(
    new RegExp(`create table if not exists ${tableName} \\([\\s\\S]+?\\n\\);`, 'i'),
  );
  assert.ok(table, `missing ${tableName} reference shape`);
  return table[0];
}

test('server session schema preserves durable revocation and process isolation', () => {
  const table = sql.match(/create table if not exists web_session \([\s\S]+?\n\);/i);
  assert.ok(table, 'missing web_session reference shape');

  for (const column of [
    'supabase_session_id',
    'encrypted_access_token',
    'encrypted_refresh_token',
    'access_expires_at',
    'refresh_lease_id',
    'refresh_lease_expires_at',
    'revoked_at',
    'revocation_pending_at',
    'revocation_next_attempt_at',
    'revocation_attempts',
    'upstream_revoked_at',
    'revocation_abandoned_at',
    'revocation_failure_kind',
  ]) {
    assert.match(table[0], new RegExp(`\\b${column}\\b`, 'i'), `missing ${column}`);
  }
  assert.doesNotMatch(table[0], /^\s+(?:access_token|refresh_token)\s/im);
  assert.match(table[0], /^\s+refresh_lease_id\s+uuid,\s*$/im);
  assert.match(table[0], /^\s+refresh_lease_expires_at\s+timestamptz,\s*$/im);

  assert.match(sql, /create index if not exists web_session_user_id_idx\s+on web_session \(user_id\)/i);
  assert.match(
    sql,
    /create index if not exists web_session_revocation_retry_idx\s+on web_session \(revocation_next_attempt_at\)/i,
  );
  assert.match(
    sql,
    /create index if not exists web_session_supabase_revocation_idx\s+on web_session \(supabase_session_id, revoked_at\)/i,
  );
  assert.match(sql, /alter table web_session enable row level security/i);
  assert.match(sql, /alter table web_session force row level security/i);

  const policy = sql.match(
    /create policy web_session_process_boundary on web_session[\s\S]+?;/i,
  );
  assert.ok(policy, 'missing web_session process-boundary policy');
  assert.match(policy[0], /current_user = 'canonical_web_server'/i);
  assert.match(policy[0], /current_user = 'canonical_session_revoker'/i);
  assert.match(
    policy[0],
    /current_setting\('canonical\.system_task', true\)[\s\S]+?= 'session_revocation'/i,
  );
  assert.match(policy[0], /with check/i);
  assert.doesNotMatch(policy[0], /auth\.uid\(\)|\*/i);
  assert.match(sql, /revoke all on table web_session from public, anon, authenticated/i);
});

test('admin RBAC tables are forced-RLS and have no customer policy', () => {
  for (const table of ['admin_role_assignment', 'admin_audit_event']) {
    assert.match(sql, new RegExp(`create table if not exists ${table} \\(`, 'i'));
    assert.match(sql, new RegExp(`alter table ${table} enable row level security`, 'i'));
    assert.match(sql, new RegExp(`alter table ${table} force row level security`, 'i'));
    assert.doesNotMatch(sql, new RegExp(`create policy [^;]+ on ${table}`, 'is'));
  }

  assert.match(
    sql,
    /check \(role in \('support', 'user_admin', 'compliance_admin', 'security_admin'\)\)/i,
  );
  assert.match(sql, /revoke all on table admin_role_assignment, admin_audit_event\s+from public, anon, authenticated/i);
});

test('customer owner policies retain exact auth.uid ownership without admin bypasses', () => {
  for (const table of [
    'audit_engagement',
    'engagement_note',
    'user_profile',
    'sync_clock',
    'sync_record',
    'sync_change',
    'sync_receipt',
  ]) {
    const policy = sql.match(new RegExp(`create policy [^;]+ on ${table}[\\s\\S]+?;`, 'i'));
    assert.ok(policy, `missing owner policy for ${table}`);
    assert.doesNotMatch(policy[0], /canonical_admin|is_admin|\bor\b/i);
    assert.match(policy[0], /auth\.uid\(\)/i);
  }
});

test('sync storage matches the runtime string types and database invariants', () => {
  const profile = tableDefinition('user_profile');
  assert.match(profile, /^\s+email\s+varchar\s+not null,/im);
  assert.match(profile, /^\s+display_name\s+varchar,/im);

  const clock = tableDefinition('sync_clock');
  assert.match(clock, /^\s+cursor\s+bigint\s+not null\s+default 0,/im);
  assert.match(
    clock,
    /^\s+constraint sync_clock_cursor_check check \(cursor >= 0\)$/im,
  );

  const record = tableDefinition('sync_record');
  assert.match(record, /^\s+collection\s+varchar\s+not null,/im);
  assert.match(
    record,
    /^\s+constraint sync_record_version_check check \(version > 0\),$/im,
  );

  const change = tableDefinition('sync_change');
  assert.match(change, /^\s+collection\s+varchar\s+not null,/im);
  assert.match(change, /^\s+operation\s+text\s+not null,/im);
  for (const invariant of [
    'constraint sync_change_cursor_check check \\(cursor > 0\\)',
    "constraint sync_change_operation_check check \\(operation in \\('put', 'delete'\\)\\)",
    'constraint sync_change_version_check check \\(version > 0\\)',
  ]) {
    assert.match(change, new RegExp(`^\\s+${invariant},?$`, 'im'));
  }

  const receipt = tableDefinition('sync_receipt');
  assert.match(receipt, /^\s+request_hash\s+varchar\s+not null,/im);
});

test('admin runtime is restricted to capability lookup and immutable audit append', () => {
  assert.match(sql, /create or replace function canonical_admin_has_capability[\s\S]+security definer/i);
  assert.match(sql, /create or replace function canonical_admin_append_audit[\s\S]+security definer/i);
  assert.match(sql, /set search_path = pg_catalog, public/i);
  assert.match(sql, /canonical_admin_has_capability\(\s*requested_capability text\s*\)/i);
  assert.match(sql, /assignment\.user_id = auth\.uid\(\)/i);
  assert.match(sql, /request\.jwt\.claims[\s\S]+->> 'aal'[\s\S]+\) = 'aal2'/i);
  assert.match(sql, /canonical_admin_has_capability\('audit\.write'\)/i);
  assert.match(sql, /event_id, auth\.uid\(\), requested_capability/i);
  assert.doesNotMatch(
    sql,
    /canonical_admin_append_audit\(\s*event_id uuid,\s*actor_id uuid/i,
  );
  assert.match(
    sql,
    /revoke all on function canonical_admin_has_capability\(text\)\s+from public, anon, authenticated/i,
  );
  assert.doesNotMatch(sql, /canonical_admin_(?:update|delete)_audit/i);
  assert.doesNotMatch(sql, /service[_-]?role/i);
});
