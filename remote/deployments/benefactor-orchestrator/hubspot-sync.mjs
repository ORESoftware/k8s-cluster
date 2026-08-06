#!/usr/bin/env node
// Consent-neutral CRM synchronization for Benefactor-discovered business contacts.
// This process writes standard company/contact fields to HubSpot and records the
// resulting IDs in RDS. It never creates marketing consent or dispatches outreach.
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';

import { parseBoolean, parseBoundedInteger, validateBatchId } from './contact-batch-lib.mjs';
import {
  assertLiveSyncConfig,
  buildCompanyProperties,
  buildContactProperties,
  buildExactSearchBody,
  buildSyncReport,
  domainFromLead,
  hashIdentifier,
  isRoleEmail,
  normalizeEmail,
  safeErrorCode,
} from './hubspot-sync-lib.mjs';

const require = createRequire('/work/package.json');
const pg = require('pg');

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

function validateHubSpotBase(raw, allowedHosts) {
  const url = new URL(raw);
  if (url.protocol !== 'https:') throw new Error('HUBSPOT_API_BASE must use https');
  if (url.username || url.password || url.port || url.pathname !== '/' || url.search || url.hash) {
    throw new Error('HUBSPOT_API_BASE must be an origin without credentials, port, path, query, or fragment');
  }
  if (!allowedHosts.includes(url.hostname.toLowerCase())) {
    throw new Error('HUBSPOT_API_BASE host is not allowlisted');
  }
  return url.origin;
}

const batchIdRaw = env('CONTACT_BATCH_ID');
const allowedHubSpotHosts = env('HUBSPOT_ALLOWED_HOSTS', 'api.hubapi.com')
  .split(',')
  .map((item) => item.trim().toLowerCase())
  .filter(Boolean);

const config = {
  rdsUrl: validateDatabaseUrl(requiredEnv('RDS_URL')),
  pgSslCaFile: requiredEnv('PG_SSL_CA_FILE'),
  dryRun: parseBoolean(process.env.HUBSPOT_DRY_RUN, true),
  accessToken: env('HUBSPOT_ACCESS_TOKEN'),
  writeConfirmation: env('HUBSPOT_WRITE_CONFIRM'),
  batchId: batchIdRaw ? validateBatchId(batchIdRaw) : '',
  lookbackHours: parseBoundedInteger('HUBSPOT_LOOKBACK_HOURS', process.env.HUBSPOT_LOOKBACK_HOURS, {
    defaultValue: 24,
    min: 1,
    max: 168,
  }),
  batchSize: parseBoundedInteger('HUBSPOT_BATCH_SIZE', process.env.HUBSPOT_BATCH_SIZE, {
    defaultValue: 100,
    min: 1,
    max: 300,
  }),
  requestTimeoutMs: parseBoundedInteger(
    'HUBSPOT_REQUEST_TIMEOUT_MS',
    process.env.HUBSPOT_REQUEST_TIMEOUT_MS,
    { defaultValue: 15_000, min: 1_000, max: 60_000 },
  ),
  requestDelayMs: parseBoundedInteger(
    'HUBSPOT_REQUEST_DELAY_MS',
    process.env.HUBSPOT_REQUEST_DELAY_MS,
    { defaultValue: 150, min: 0, max: 5_000 },
  ),
  claimTimeoutMinutes: parseBoundedInteger(
    'HUBSPOT_CLAIM_TIMEOUT_MINUTES',
    process.env.HUBSPOT_CLAIM_TIMEOUT_MINUTES,
    { defaultValue: 30, min: 5, max: 240 },
  ),
  requireRoleEmail: parseBoolean(process.env.HUBSPOT_REQUIRE_ROLE_EMAIL, true),
  apiBase: validateHubSpotBase(env('HUBSPOT_API_BASE', 'https://api.hubapi.com'), allowedHubSpotHosts),
  runLockKey: env('HUBSPOT_RUN_LOCK_KEY', 'benefactor-hubspot-contact-sync'),
};

assertLiveSyncConfig(config);

const db = new pg.Client({
  connectionString: config.rdsUrl,
  ssl: {
    ca: readFileSync(config.pgSslCaFile, 'utf8'),
    rejectUnauthorized: true,
  },
  statement_timeout: 30_000,
  query_timeout: 35_000,
  application_name: 'benefactor-hubspot-contact-sync',
});

function sleep(ms) {
  return ms > 0 ? new Promise((resolve) => setTimeout(resolve, ms)) : Promise.resolve();
}

async function readBodyCapped(response, maxBytes = 1_048_576) {
  if (!response.body) return '';
  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel().catch(() => {});
      throw new Error('hubspot_response_too_large');
    }
    chunks.push(value);
  }
  return Buffer.concat(chunks.map((chunk) => Buffer.from(chunk))).toString('utf8');
}

function retryDelay(response, attempt) {
  const raw = response.headers.get('retry-after');
  const seconds = raw && /^\d+$/.test(raw) ? Number.parseInt(raw, 10) : 0;
  return Math.min(30_000, seconds > 0 ? seconds * 1_000 : 500 * 2 ** attempt);
}

async function hubspotRequest(path, { method = 'GET', body, allow404 = false } = {}) {
  const url = new URL(path, `${config.apiBase}/`);
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), config.requestTimeoutMs);
    let response;
    try {
      response = await fetch(url, {
        method,
        signal: controller.signal,
        headers: {
          Authorization: `Bearer ${config.accessToken}`,
          Accept: 'application/json',
          ...(body ? { 'Content-Type': 'application/json' } : {}),
        },
        ...(body ? { body: JSON.stringify(body) } : {}),
      });
    } finally {
      clearTimeout(timer);
    }

    if (allow404 && response.status === 404) {
      await readBodyCapped(response).catch(() => '');
      return null;
    }

    if ((response.status === 429 || response.status >= 500) && attempt < 2) {
      await readBodyCapped(response).catch(() => '');
      await sleep(retryDelay(response, attempt));
      continue;
    }

    const raw = await readBodyCapped(response);
    if (!response.ok) {
      const error = new Error(`HubSpot request failed with HTTP ${response.status}`);
      error.status = response.status;
      throw error;
    }
    if (!raw) return {};
    try {
      return JSON.parse(raw);
    } catch {
      throw new Error('hubspot_invalid_json');
    }
  }
  throw new Error('hubspot_retry_exhausted');
}

async function searchObject(objectType, propertyName, value) {
  const response = await hubspotRequest(`/crm/v3/objects/${objectType}/search`, {
    method: 'POST',
    body: buildExactSearchBody(propertyName, value),
  });
  return Array.isArray(response.results) && response.results.length ? response.results[0] : null;
}

async function upsertObject(objectType, propertyName, propertyValue, properties) {
  const existing = await searchObject(objectType, propertyName, propertyValue);
  if (existing?.id) {
    const updated = await hubspotRequest(`/crm/v3/objects/${objectType}/${encodeURIComponent(existing.id)}`, {
      method: 'PATCH',
      body: { properties },
    });
    return { id: String(updated.id || existing.id), action: 'updated' };
  }
  const created = await hubspotRequest(`/crm/v3/objects/${objectType}`, {
    method: 'POST',
    body: { properties },
  });
  if (!created.id) throw new Error(`hubspot_${objectType}_create_missing_id`);
  return { id: String(created.id), action: 'created' };
}

async function loadCandidates() {
  const result = await db.query(
    `
SELECT
  id::text AS lead_id,
  LOWER(BTRIM(primary_email)) AS email,
  NULLIF(BTRIM(owner_first_name), '') AS first_name,
  NULLIF(BTRIM(owner_last_name), '') AS last_name,
  NULLIF(BTRIM(business_name), '') AS business_name,
  NULLIF(BTRIM(website_url), '') AS website_url,
  NULLIF(BTRIM(source_url), '') AS source_url,
  NULLIF(BTRIM(service_category), '') AS service_category,
  NULLIF(BTRIM(city), '') AS city,
  NULLIF(BTRIM(state), '') AS state,
  lead_status,
  outreach_status,
  COALESCE(meta_data, '{}'::jsonb) AS meta_data
FROM benefactor.benefactor_leads
WHERE is_soft_deleted = false
  AND primary_email IS NOT NULL
  AND BTRIM(primary_email) <> ''
  AND source_tool = 'orchestrator'
  AND LOWER(COALESCE(lead_status, '')) NOT IN (
    'unsubscribed', 'do_not_contact', 'suppressed', 'bounced', 'complaint'
  )
  AND (
    $1::text IS NOT NULL AND meta_data->>'contactBatchId' = $1
    OR $1::text IS NULL AND created_at >= now() - make_interval(hours => $2::int)
  )
  AND (
    COALESCE(meta_data->'hubspotSync'->>'status', 'pending') IN ('pending', 'failed')
    OR (
      meta_data->'hubspotSync'->>'status' = 'syncing'
      AND CASE
        WHEN COALESCE(meta_data->'hubspotSync'->>'startedAt', '')
          ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}'
        THEN (meta_data->'hubspotSync'->>'startedAt')::timestamptz
        ELSE to_timestamp(0)
      END < now() - make_interval(mins => $3::int)
    )
  )
ORDER BY created_at ASC, id ASC
LIMIT $4;
`,
    [config.batchId || null, config.lookbackHours, config.claimTimeoutMinutes, config.batchSize],
  );
  return result.rows;
}

async function claimLead(lead) {
  const state = {
    status: 'syncing',
    startedAt: new Date().toISOString(),
    batchId: config.batchId || lead.meta_data?.contactBatchId || null,
    marketingConsent: 'unchanged',
    outreachApproval: 'required',
    runner: 'benefactor-hubspot-contact-sync',
  };
  const result = await db.query(
    `
UPDATE benefactor.benefactor_leads
SET
  meta_data = jsonb_set(COALESCE(meta_data, '{}'::jsonb), '{hubspotSync}', $2::jsonb, true),
  updated_at = now()
WHERE id::text = $1
  AND is_soft_deleted = false
  AND COALESCE(meta_data->'hubspotSync'->>'status', 'pending') <> 'synced'
RETURNING id;
`,
    [lead.lead_id, JSON.stringify(state)],
  );
  return result.rowCount === 1;
}

async function markLead(leadId, state) {
  await db.query(
    `
UPDATE benefactor.benefactor_leads
SET
  meta_data = jsonb_set(COALESCE(meta_data, '{}'::jsonb), '{hubspotSync}', $2::jsonb, true),
  updated_at = now()
WHERE id::text = $1;
`,
    [leadId, JSON.stringify(state)],
  );
}

async function syncLead(lead) {
  const companyProperties = buildCompanyProperties(lead);
  const contactProperties = buildContactProperties(lead);
  let company = null;
  if (companyProperties.domain) {
    company = await upsertObject('companies', 'domain', companyProperties.domain, companyProperties);
    await sleep(config.requestDelayMs);
  }
  const contact = await upsertObject('contacts', 'email', contactProperties.email, contactProperties);
  return { company, contact };
}

async function run() {
  await db.connect();
  const lock = await db.query('SELECT pg_try_advisory_lock(hashtext($1)) AS locked', [config.runLockKey]);
  if (!lock.rows[0]?.locked) {
    console.log('HUBSPOT_SYNC_REPORT ' + JSON.stringify(buildSyncReport({
      batchId: config.batchId,
      dryRun: config.dryRun,
      candidates: 0,
      synced: 0,
      skipped: 0,
      failed: 0,
      companies: 0,
      contacts: 0,
    })));
    return;
  }

  const counters = {
    candidates: 0,
    synced: 0,
    skipped: 0,
    failed: 0,
    companies: 0,
    contacts: 0,
  };

  try {
    const candidates = await loadCandidates();
    counters.candidates = candidates.length;
    for (const lead of candidates) {
      const email = normalizeEmail(lead.email);
      const sampleId = hashIdentifier(email).slice(0, 12);
      if (!email || (config.requireRoleEmail && !isRoleEmail(email))) {
        counters.skipped += 1;
        console.log(JSON.stringify({ event: 'hubspot_sync_skipped', sampleId, reason: 'not_role_business_email' }));
        continue;
      }

      if (config.dryRun) {
        console.log(JSON.stringify({
          event: 'hubspot_sync_candidate',
          sampleId,
          hasCompanyDomain: Boolean(domainFromLead(lead)),
          hasPhone: Boolean(buildContactProperties(lead).phone),
          serviceCategory: lead.service_category || null,
        }));
        continue;
      }

      const claimed = await claimLead(lead);
      if (!claimed) {
        counters.skipped += 1;
        continue;
      }

      try {
        const result = await syncLead(lead);
        counters.contacts += 1;
        if (result.company) counters.companies += 1;
        counters.synced += 1;
        await markLead(lead.lead_id, {
          status: 'synced',
          syncedAt: new Date().toISOString(),
          batchId: config.batchId,
          contactId: result.contact.id,
          companyId: result.company?.id || null,
          contactAction: result.contact.action,
          companyAction: result.company?.action || null,
          marketingConsent: 'unchanged',
          outreachApproval: 'required',
          runner: 'benefactor-hubspot-contact-sync',
        });
        console.log(JSON.stringify({ event: 'hubspot_sync_succeeded', sampleId }));
      } catch (error) {
        counters.failed += 1;
        const errorCode = safeErrorCode(error);
        await markLead(lead.lead_id, {
          status: 'failed',
          failedAt: new Date().toISOString(),
          batchId: config.batchId,
          errorCode,
          marketingConsent: 'unchanged',
          outreachApproval: 'required',
          runner: 'benefactor-hubspot-contact-sync',
        }).catch(() => {});
        console.error(JSON.stringify({ event: 'hubspot_sync_failed', sampleId, errorCode }));
      }
      await sleep(config.requestDelayMs);
    }

    console.log(
      'HUBSPOT_SYNC_REPORT ' +
        JSON.stringify(
          buildSyncReport({
            batchId: config.batchId,
            dryRun: config.dryRun,
            ...counters,
          }),
        ),
    );
  } finally {
    await db.query('SELECT pg_advisory_unlock(hashtext($1))', [config.runLockKey]).catch(() => {});
  }
}

try {
  await run();
} catch (error) {
  console.error(JSON.stringify({ event: 'hubspot_sync_fatal', errorCode: safeErrorCode(error) }));
  process.exitCode = 1;
} finally {
  await db.end().catch(() => {});
}
