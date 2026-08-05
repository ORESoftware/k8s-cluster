#!/usr/bin/env node
// Multi-ICP Benefactor contact-discovery batch runner.
// Runs the existing hardened orchestrator one category at a time, tags only newly
// inserted RDS leads with a batch ID, optionally invokes the consent-neutral
// HubSpot sync, and never dispatches outreach.
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  buildBatchReport,
  makeBatchId,
  normalizeCategoryRows,
  parseBoolean,
  parseBoundedInteger,
  planCategoryTargets,
  shouldIncludeOverflow,
} from './contact-batch-lib.mjs';

const require = createRequire('/work/package.json');
const pg = require('pg');
const here = path.dirname(fileURLToPath(import.meta.url));

function env(name, fallback = '') {
  const value = process.env[name];
  return value == null || String(value).trim() === '' ? fallback : String(value).trim();
}

function requiredEnv(name) {
  const value = env(name);
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function validateDatabaseUrl(raw) {
  const url = new URL(raw);
  if (!['postgres:', 'postgresql:'].includes(url.protocol)) {
    throw new Error('RDS_URL must use postgres or postgresql');
  }
  url.searchParams.delete('sslmode');
  url.searchParams.delete('uselibpqcompat');
  return url.toString();
}

const targetContacts = parseBoundedInteger(
  'BATCH_TARGET_CONTACTS',
  process.env.BATCH_TARGET_CONTACTS,
  { defaultValue: 250, min: 200, max: 300 },
);
const minimumContacts = parseBoundedInteger(
  'BATCH_MINIMUM_CONTACTS',
  process.env.BATCH_MINIMUM_CONTACTS,
  { defaultValue: 200, min: 1, max: 300 },
);
const maximumContacts = parseBoundedInteger(
  'BATCH_MAXIMUM_CONTACTS',
  process.env.BATCH_MAXIMUM_CONTACTS,
  { defaultValue: 300, min: 200, max: 300 },
);
if (minimumContacts > targetContacts || targetContacts > maximumContacts) {
  throw new Error('contact bounds must satisfy minimum <= target <= maximum');
}

const config = {
  rdsUrl: validateDatabaseUrl(requiredEnv('RDS_URL')),
  pgSslCaFile: requiredEnv('PG_SSL_CA_FILE'),
  dryRun: parseBoolean(process.env.BATCH_DRY_RUN, true),
  persistConfirmation: env('BATCH_PERSIST_CONFIRM'),
  targetContacts,
  minimumContacts,
  maximumContacts,
  maxCategories: parseBoundedInteger('BATCH_MAX_CATEGORIES', process.env.BATCH_MAX_CATEGORIES, {
    defaultValue: 20,
    min: 1,
    max: 100,
  }),
  maxPerCategory: parseBoundedInteger(
    'BATCH_MAX_CONTACTS_PER_CATEGORY',
    process.env.BATCH_MAX_CONTACTS_PER_CATEGORY,
    { defaultValue: 50, min: 5, max: 100 },
  ),
  categoryAllowlist: env('ICP_CATEGORIES')
    .split(',')
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean),
  batchId: makeBatchId(),
  runLockKey: env('BATCH_RUN_LOCK_KEY', 'benefactor-contact-discovery-batch'),
  hubspotAfterDiscovery: parseBoolean(process.env.HUBSPOT_SYNC_AFTER_DISCOVERY, true),
};

if (!config.dryRun && config.persistConfirmation !== 'collect-benefactor-contact-batch') {
  throw new Error('live collection requires BATCH_PERSIST_CONFIRM=collect-benefactor-contact-batch');
}

const db = new pg.Client({
  connectionString: config.rdsUrl,
  ssl: {
    ca: readFileSync(config.pgSslCaFile, 'utf8'),
    rejectUnauthorized: true,
  },
  statement_timeout: 30_000,
  query_timeout: 35_000,
  application_name: 'benefactor-contact-batch',
});

function runChild(script, extraEnv) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(here, script)], {
      cwd: here,
      env: { ...process.env, ...extraEnv },
      stdio: 'inherit',
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${script} exited with ${code ?? signal ?? 'unknown'}`));
    });
  });
}

async function loadCategories() {
  const result = await db.query(
    `
SELECT
  service_category,
  COUNT(*)::int AS query_count,
  MAX(priority)::int AS max_priority
FROM benefactor.benefactor_scrape_queries
WHERE is_active = true
  AND is_soft_deleted = false
  AND service_category IS NOT NULL
  AND BTRIM(service_category) <> ''
GROUP BY service_category
ORDER BY MAX(priority) DESC, COUNT(*) DESC, service_category ASC
LIMIT $1;
`,
    [config.maxCategories],
  );
  return normalizeCategoryRows(result.rows, config.categoryAllowlist);
}

async function tagNewLeads(category, categoryStartedAt) {
  if (config.dryRun) return 0;
  const pendingHubSpotState = {
    status: 'pending',
    queuedAt: new Date().toISOString(),
    batchId: config.batchId,
    marketingConsent: 'unchanged',
    outreachApproval: 'required',
  };
  const result = await db.query(
    `
UPDATE benefactor.benefactor_leads
SET
  meta_data = COALESCE(meta_data, '{}'::jsonb)
    || jsonb_build_object(
      'contactBatchId', $1,
      'contactBatchStartedAt', $2,
      'marketingConsent', 'unknown',
      'outreachApproval', 'required',
      'hubspotSync', $5::jsonb
    ),
  updated_at = now()
WHERE is_soft_deleted = false
  AND source_tool = 'orchestrator'
  AND service_category = $3
  AND created_at >= $4::timestamptz
  AND COALESCE(meta_data->>'contactBatchId', '') = ''
RETURNING id;
`,
    [
      config.batchId,
      categoryStartedAt.toISOString(),
      category,
      categoryStartedAt.toISOString(),
      JSON.stringify(pendingHubSpotState),
    ],
  );
  return result.rowCount;
}

async function countTaggedContacts() {
  if (config.dryRun) return 0;
  const result = await db.query(
    `
SELECT COUNT(*)::int AS count
FROM benefactor.benefactor_leads
WHERE is_soft_deleted = false
  AND meta_data->>'contactBatchId' = $1;
`,
    [config.batchId],
  );
  return Number(result.rows[0]?.count || 0);
}

async function countApprovedForOutreach() {
  if (config.dryRun) return 0;
  const exists = await db.query("SELECT to_regclass('public.benefactor_marketing_contacts') AS table_name");
  if (!exists.rows[0]?.table_name) return 0;
  const result = await db.query(
    `
SELECT COUNT(DISTINCT LOWER(bl.primary_email))::int AS count
FROM benefactor.benefactor_leads bl
JOIN public.benefactor_marketing_contacts mc
  ON LOWER(mc.email) = LOWER(bl.primary_email)
WHERE bl.is_soft_deleted = false
  AND bl.meta_data->>'contactBatchId' = $1
  AND mc.status = 'active'
  AND mc.consent_status = 'opted_in';
`,
    [config.batchId],
  );
  return Number(result.rows[0]?.count || 0);
}

async function run() {
  await db.connect();
  const lock = await db.query('SELECT pg_try_advisory_lock(hashtext($1)) AS locked', [config.runLockKey]);
  if (!lock.rows[0]?.locked) throw new Error('another contact-discovery batch holds the advisory lock');

  let categoriesRun = 0;
  let contactsTagged = 0;
  let hubspot = { attempted: false };
  try {
    const categories = await loadCategories();
    if (!categories.length) throw new Error('no active ICP categories are available');
    const plan = planCategoryTargets(categories, config.targetContacts, config.maxPerCategory, {
      includeOverflow: shouldIncludeOverflow(config.dryRun),
    });

    console.log(
      JSON.stringify({
        event: 'benefactor_contact_batch_start',
        batchId: config.batchId,
        dryRun: config.dryRun,
        targetContacts: config.targetContacts,
        categories: categories.length,
        plannedRuns: plan.length,
      }),
    );

    for (const item of plan) {
      if (!config.dryRun && contactsTagged >= config.targetContacts) break;
      const remaining = config.dryRun ? item.target : config.targetContacts - contactsTagged;
      const categoryTarget = Math.min(item.target, remaining);
      if (categoryTarget <= 0) break;
      const categoryStartedAt = new Date();
      await runChild('orchestrate.mjs', {
        ICP_CATEGORY: item.category,
        TARGET_EMAILS: String(categoryTarget),
        PIPELINE_DRY_RUN: config.dryRun ? 'true' : 'false',
      });
      categoriesRun += 1;
      const tagged = await tagNewLeads(item.category, categoryStartedAt);
      contactsTagged = config.dryRun ? contactsTagged : await countTaggedContacts();
      console.log(
        JSON.stringify({
          event: 'benefactor_contact_batch_category_done',
          category: item.category,
          requested: categoryTarget,
          tagged,
          batchTotal: contactsTagged,
        }),
      );
    }

    if (config.hubspotAfterDiscovery) {
      hubspot = { attempted: true, dryRun: config.dryRun || parseBoolean(process.env.HUBSPOT_DRY_RUN, true) };
      await runChild('hubspot-sync.mjs', {
        CONTACT_BATCH_ID: config.batchId,
        HUBSPOT_DRY_RUN: hubspot.dryRun ? 'true' : 'false',
        HUBSPOT_BATCH_SIZE: String(config.maximumContacts),
      });
    }

    const approvedForOutreach = await countApprovedForOutreach();
    const status = config.dryRun
      ? 'dry_run_complete'
      : contactsTagged < config.minimumContacts
        ? 'below_minimum'
        : contactsTagged > config.maximumContacts
          ? 'above_maximum'
          : 'collected_and_synced';
    const report = buildBatchReport({
      batchId: config.batchId,
      dryRun: config.dryRun,
      targetContacts: config.targetContacts,
      minimumContacts: config.minimumContacts,
      maximumContacts: config.maximumContacts,
      categoriesPlanned: plan.length,
      categoriesRun,
      contactsTagged,
      hubspot,
      approvedForOutreach,
      status,
    });
    console.log(`BENEFACTOR_CONTACT_BATCH_REPORT ${JSON.stringify(report)}`);

    if (!config.dryRun && contactsTagged < config.minimumContacts) process.exitCode = 2;
    if (!config.dryRun && contactsTagged > config.maximumContacts) process.exitCode = 3;
  } finally {
    await db.query('SELECT pg_advisory_unlock(hashtext($1))', [config.runLockKey]).catch(() => {});
  }
}

try {
  await run();
} catch (error) {
  console.error(JSON.stringify({
    event: 'benefactor_contact_batch_fatal',
    error: String(error?.message || error).slice(0, 240),
  }));
  process.exitCode = 1;
} finally {
  await db.end().catch(() => {});
}
