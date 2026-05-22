import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import test from 'node:test';

function findRepoRoot(): string {
  for (const candidate of [process.cwd(), resolve(process.cwd(), '..', '..')]) {
    if (existsSync(resolve(candidate, 'remote/deployments/dev-server/src/server.ts'))) {
      return candidate;
    }
  }
  throw new Error(`Unable to locate repo root from ${process.cwd()}`);
}

const repoRoot = findRepoRoot();

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(resolve(repoRoot, relativePath), 'utf8');
}

test('postgres schema declares the agent_remote_dev_breadcrumbs source of truth', async () => {
  const schemaSql = await readRepoFile('remote/libs/pg-defs/schema/schema.sql');

  assert.match(
    schemaSql,
    /create table if not exists agent_remote_dev_breadcrumbs \(\s*id bigserial primary key,\s*thread_id uuid not null,\s*task_id uuid,\s*kind varchar\(80\) not null,\s*payload jsonb default '\{\}'::jsonb not null,\s*emitted_at timestamptz default now\(\) not null,/,
  );
  assert.match(schemaSql, /agent_remote_dev_breadcrumbs_thread_id_emitted_at_idx/);
  assert.match(schemaSql, /agent_remote_dev_breadcrumbs_task_id_emitted_at_idx[\s\S]*where task_id is not null/);
  assert.match(schemaSql, /agent_remote_dev_breadcrumbs_emitted_at_idx/);
  assert.match(schemaSql, /agent_remote_dev_breadcrumbs_kind_format_chk/);
  assert.match(schemaSql, /agent_remote_dev_breadcrumbs_payload_object_chk/);
});

test('pg-defs codegen surfaces the breadcrumb table in every generated language', async () => {
  const drizzle = await readRepoFile('remote/libs/pg-defs/generated/typescript/drizzle.ts');
  const rust = await readRepoFile('remote/libs/pg-defs/generated/rust/src/lib.rs');
  const gleam = await readRepoFile('remote/libs/pg-defs/generated/gleam/src/pg_defs.gleam');
  const prisma = await readRepoFile('remote/libs/pg-defs/generated/prisma/schema.prisma');
  const sqlcSchema = await readRepoFile('remote/libs/pg-defs/generated/go/sqlc/schema.sql');

  assert.match(drizzle, /export const agentRemoteDevBreadcrumbs = pgTable\(\s*"agent_remote_dev_breadcrumbs"/);
  assert.match(rust, /pub const AGENT_REMOTE_DEV_BREADCRUMBS_TABLE: &str = "agent_remote_dev_breadcrumbs";/);
  assert.match(rust, /pub struct AgentRemoteDevBreadcrumbRow/);
  assert.match(gleam, /pub const agent_remote_dev_breadcrumbs_table = "agent_remote_dev_breadcrumbs"/);
  assert.match(gleam, /pub type AgentRemoteDevBreadcrumbRow/);
  assert.match(prisma, /model AgentRemoteDevBreadcrumb \{/);
  assert.match(prisma, /@@map\("agent_remote_dev_breadcrumbs"\)/);
  assert.match(sqlcSchema, /create table if not exists agent_remote_dev_breadcrumbs/);
});

test('rest-api exposes breadcrumb ingest + tail endpoints backed by postgres', async () => {
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');

  assert.match(restServer, /struct AgentBreadcrumbIngestRequest/);
  assert.match(restServer, /struct AgentBreadcrumbRow/);
  assert.match(restServer, /struct AgentBreadcrumbTailResponse/);
  assert.match(restServer, /async fn persist_agent_breadcrumb_to_postgres\(/);
  assert.match(restServer, /async fn fetch_agent_breadcrumb_tail_from_postgres\(/);
  assert.match(restServer, /async fn ingest_agent_breadcrumb\(/);
  assert.match(restServer, /async fn agent_thread_breadcrumb_tail\(/);
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/breadcrumbs",\s*post\(ingest_agent_breadcrumb\),\s*\)/,
  );
  assert.match(
    restServer,
    /\.route\(\s*"\/api\/agents\/threads\/:thread_id\/breadcrumbs\/tail",\s*get\(agent_thread_breadcrumb_tail\),\s*\)/,
  );
  assert.match(restServer, /insert into agent_remote_dev_breadcrumbs/);
  assert.match(restServer, /from agent_remote_dev_breadcrumbs/);
  assert.match(
    restServer,
    /and \(task_id is null or task_id <> \$2::text::uuid\)/,
  );
});

test('dev-server uses the rest-api breadcrumb endpoints instead of writing tmp/convos in the workspace', async () => {
  const server = await readRepoFile('remote/deployments/dev-server/src/server.ts');

  assert.match(server, /async function postBreadcrumb\(/);
  assert.match(server, /async function postSessionBreadcrumb\(/);
  assert.match(server, /async function postTaskBreadcrumb\(/);
  assert.match(
    server,
    /\/api\/agents\/threads\/\$\{encodeURIComponent\(input\.threadId\)\}\/breadcrumbs/,
  );
  assert.match(server, /breadcrumbWriteTimeoutMs:/);
  assert.match(server, /sanitizeBreadcrumbPayload/);

  // No file-system breadcrumb writes inside the workspace.
  assert.doesNotMatch(server, /THREAD_LOG_RELATIVE_PATH/);
  assert.doesNotMatch(server, /threadLogRelativePath/);
  assert.doesNotMatch(server, /getSessionLogPath/);
  assert.doesNotMatch(server, /appendThreadLog/);
  assert.doesNotMatch(server, /readLocalThreadContext/);

  // Auto-tail fetch is gone — breadcrumbs only enter the prompt via explicit
  // picker selection in `contextBlobs`.
  assert.doesNotMatch(server, /async function fetchThreadBreadcrumbTail\(/);
  assert.doesNotMatch(server, /\/breadcrumbs\/tail/);
});

test('breadcrumbs ride the context picker as checkbox-selectable candidates', async () => {
  const restServer = await readRepoFile('remote/deployments/rest-api-rs/src/main.rs');
  const devServer = await readRepoFile('remote/deployments/dev-server/src/server.ts');
  const webHome = await readRepoFile('remote/deployments/web-home-rs/src/main.rs');

  // 1. AgentContextCandidate carries a `kind` discriminator; breadcrumbs are
  //    fetched alongside agent_context_blobs in the same response.
  assert.match(restServer, /kind: String,?\s*\}/);
  assert.match(restServer, /CONTEXT_KIND_BREADCRUMB: &str = "breadcrumb"/);
  assert.match(restServer, /CONTEXT_KIND_BLOB: &str = "context-blob"/);
  assert.match(restServer, /BREADCRUMB_CANDIDATE_PREFIX: &str = "breadcrumb:"/);
  assert.match(restServer, /async fn fetch_breadcrumb_candidates_for_thread\(/);
  assert.match(restServer, /fn breadcrumb_row_to_candidate\(/);

  // 2. Selected-context fetch splits ids into blob + breadcrumb buckets so a
  //    `breadcrumb:<n>` selection resolves to an agent_remote_dev_breadcrumbs
  //    row, not a missing context-blob.
  assert.match(restServer, /if id\.starts_with\(BREADCRUMB_CANDIDATE_PREFIX\)/);
  assert.match(
    restServer,
    /from agent_remote_dev_breadcrumbs[\s\S]*?where thread_id = \$1::text::uuid[\s\S]*?and id = any\(\$2\)/,
  );

  // 3. dev-server treats kind: 'breadcrumb' rows as the source of
  //    <thread_breadcrumb_tail>; kind: 'context-blob' stays in
  //    <selected_context_blobs>. Unchecked rows never reach the worker.
  assert.match(devServer, /kind: z\.enum\(\[[^\]]*'breadcrumb'[^\]]*\]\)\.optional\(\)/);
  assert.match(devServer, /function isBreadcrumbContextItem\(/);
  assert.match(devServer, /function formatSelectedBreadcrumbs\(/);
  assert.match(devServer, /<thread_breadcrumb_tail>/);
  assert.match(devServer, /thread-context:selected-breadcrumbs/);

  // 4. Picker UI shows breadcrumbs with a distinct badge/class so operators
  //    can tell what they're un-checking.
  assert.match(webHome, /context-row-breadcrumb/);
  assert.match(webHome, /context-badge-breadcrumb/);
  assert.match(webHome, /item\.kind === "breadcrumb"/);
});

test('redis interfaces package exposes the agent breadcrumb cache schema for cross-runtime consumers', async () => {
  const tsIndex = await readRepoFile('remote/libs/interfaces/redis/generated/typescript/index.ts');
  const rustLib = await readRepoFile('remote/libs/interfaces/redis/generated/rust/src/lib.rs');
  const pyModule = await readRepoFile('remote/libs/interfaces/redis/generated/python/dd_redis_interfaces.py');
  const gleamModule = await readRepoFile('remote/libs/interfaces/redis/generated/gleam/src/dd_redis_interfaces.gleam');
  const indexJson = await readRepoFile('remote/libs/interfaces/redis/schema/index.json');

  for (const schemaName of [
    'agent-thread-breadcrumb-cache.schema.json',
    'container-pool-affinity-lock.schema.json',
    'runtime-config-redis.schema.json',
  ]) {
    assert.match(indexJson, new RegExp(schemaName), `index.json must list ${schemaName}`);
  }

  assert.match(tsIndex, /export type AgentThreadBreadcrumb = \{/);
  assert.match(tsIndex, /export type AgentThreadBreadcrumbTail = \{/);
  assert.match(tsIndex, /export function agentThreadBreadcrumbTailKey\(prefix: string, threadId: string\): string/);
  assert.match(tsIndex, /export function containerPoolAffinityLockKey\(prefix: string, poolSlug: string, threadId: string\): string/);
  assert.match(tsIndex, /export function runtimeConfigEntryKey\(prefix: string, env: string, scope: string, key: string\): string/);

  assert.match(rustLib, /pub fn agent_thread_breadcrumb_tail_key\(prefix: &str, thread_id: &str\) -> String/);
  assert.match(rustLib, /pub fn container_pool_affinity_lock_key\(prefix: &str, pool_slug: &str, thread_id: &str\) -> String/);
  assert.match(rustLib, /pub fn runtime_config_entry_key\(prefix: &str, env: &str, scope: &str, key: &str\) -> String/);
  assert.match(rustLib, /pub struct AgentThreadBreadcrumb \{/);
  assert.match(rustLib, /pub struct AgentThreadBreadcrumbTail \{/);

  assert.match(pyModule, /def agent_thread_breadcrumb_tail_key\(prefix: str, thread_id: str\) -> str:/);
  assert.match(pyModule, /class AgentThreadBreadcrumb:/);
  assert.match(pyModule, /class AgentThreadBreadcrumbTail:/);

  assert.match(gleamModule, /pub fn agent_thread_breadcrumb_tail_key\(/);
  assert.match(gleamModule, /pub type AgentThreadBreadcrumbTail \{/);
});
