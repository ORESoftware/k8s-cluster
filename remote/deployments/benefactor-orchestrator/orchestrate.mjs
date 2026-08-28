// Benefactor lead-discovery orchestrator.
//
// Security/ownership boundary:
// - arbitrary-domain retrieval is performed only by the private dd-web-scraper service;
// - this process never directly fetches a discovered URL and never sends outreach;
// - provider/search and scraper responses are bounded in bytes and time;
// - dry-run mode performs no database mutations and emits a deterministic, identifier-hashed report.
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import {
  buildDryRunReport,
  canonicalJson,
  confidenceForContact,
  extractEmailsFromText,
  extractPhonesFromText,
  hostOf,
  mergeProviderResults,
  normalizeCandidateUrl,
  normalizeEmail,
  normalizePhone,
  normalizeScraperServiceUrl,
  parseBoolean,
  parseBoundedInteger,
  providerStatuses,
  readJsonCapped,
  sanitizeLogValue,
} from './pipeline-lib.mjs';
import { createSearchProviders } from './providers/index.mjs';

const require = createRequire('/work/package.json');
const pg = require('pg');
const cheerio = require('cheerio');

function requiredEnv(name) {
  const value = String(process.env[name] ?? '').trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

const config = {
  rdsUrl: requiredEnv('RDS_URL'),
  pgSslCaFile: requiredEnv('PG_SSL_CA_FILE'),
  scraperUrl: process.env.SCRAPER_URL || 'http://dd-web-scraper.default.svc.cluster.local:8097',
  scraperAllowedHosts: String(process.env.SCRAPER_ALLOWED_HOSTS || 'dd-web-scraper.default.svc.cluster.local')
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean),
  scraperAuth: requiredEnv('SCRAPER_AUTH'),
  category: requiredEnv('ICP_CATEGORY'),
  serperKey: String(process.env.SERPER_API_KEY || ''),
  braveKey: String(process.env.BRAVE_SEARCH_API_KEY || ''),
  apolloKey: String(process.env.APOLLO_API_KEY || ''),
  hunterKey: String(process.env.HUNTER_API_KEY || ''),
  datalaneKey: String(process.env.DATALANE_API_KEY || ''),
  linkedinExportPath: String(process.env.LINKEDIN_SALES_NAVIGATOR_EXPORT || ''),
  maxQueries: parseBoundedInteger('MAX_QUERIES', process.env.MAX_QUERIES, { defaultValue: 8, min: 1, max: 100 }),
  targetEmails: parseBoundedInteger('TARGET_EMAILS', process.env.TARGET_EMAILS, { defaultValue: 30, min: 1, max: 2_000 }),
  maxPagesPerQuery: parseBoundedInteger('MAX_PAGES_PER_QUERY', process.env.MAX_PAGES_PER_QUERY, { defaultValue: 8, min: 1, max: 50 }),
  domainSkipDays: parseBoundedInteger('DOMAIN_SKIP_DAYS', process.env.DOMAIN_SKIP_DAYS, { defaultValue: 14, min: 0, max: 365 }),
  queryCooldownDays: parseBoundedInteger('QUERY_COOLDOWN_DAYS', process.env.QUERY_COOLDOWN_DAYS, { defaultValue: 30, min: 0, max: 365 }),
  zeroNewRetire: parseBoundedInteger('ZERO_NEW_RETIRE', process.env.ZERO_NEW_RETIRE, { defaultValue: 3, min: 1, max: 100 }),
  scrapeThrottleDays: parseBoundedInteger('SCRAPE_THROTTLE_DAYS', process.env.SCRAPE_THROTTLE_DAYS, { defaultValue: 30, min: 1, max: 365 }),
  deadlineSeconds: parseBoundedInteger('DEADLINE_SECONDS', process.env.DEADLINE_SECONDS, { defaultValue: 420, min: 30, max: 3_600 }),
  providerTimeoutMs: parseBoundedInteger('PROVIDER_TIMEOUT_MS', process.env.PROVIDER_TIMEOUT_MS, { defaultValue: 15_000, min: 1_000, max: 60_000 }),
  scraperTimeoutMs: parseBoundedInteger('SCRAPER_TIMEOUT_MS', process.env.SCRAPER_TIMEOUT_MS, { defaultValue: 45_000, min: 5_000, max: 120_000 }),
  responseBodyTimeoutMs: parseBoundedInteger('RESPONSE_BODY_TIMEOUT_MS', process.env.RESPONSE_BODY_TIMEOUT_MS, { defaultValue: 20_000, min: 1_000, max: 60_000 }),
  maxProviderResponseBytes: parseBoundedInteger('MAX_PROVIDER_RESPONSE_BYTES', process.env.MAX_PROVIDER_RESPONSE_BYTES, { defaultValue: 2 * 1024 * 1024, min: 16_384, max: 8 * 1024 * 1024 }),
  maxScraperResponseBytes: parseBoundedInteger('MAX_SCRAPER_RESPONSE_BYTES', process.env.MAX_SCRAPER_RESPONSE_BYTES, { defaultValue: 3 * 1024 * 1024, min: 64 * 1024, max: 12 * 1024 * 1024 }),
  dbStatementTimeoutMs: parseBoundedInteger('DB_STATEMENT_TIMEOUT_MS', process.env.DB_STATEMENT_TIMEOUT_MS, { defaultValue: 30_000, min: 1_000, max: 120_000 }),
  requireRoleEmail: parseBoolean(process.env.REQUIRE_ROLE_EMAIL, true),
  dryRun: parseBoolean(process.env.PIPELINE_DRY_RUN, false),
  scrapeRequestType: String(process.env.SCRAPE_REQUEST_TYPE || 'scrape_collect').trim(),
};

config.scraperUrl = normalizeScraperServiceUrl(config.scraperUrl, config.scraperAllowedHosts);
if (!/^[a-z0-9][a-z0-9._:-]{0,127}$/i.test(config.category)) {
  throw new Error('ICP_CATEGORY contains unsupported characters');
}
if (!/^[a-z0-9][a-z0-9._:-]{0,127}$/i.test(config.scrapeRequestType)) {
  throw new Error('SCRAPE_REQUEST_TYPE contains unsupported characters');
}
const statuses = providerStatuses(config);
const searchProviders = createSearchProviders({
  braveKey: config.braveKey,
  serperKey: config.serperKey,
  fetchJson: providerFetchJson,
});
if (!searchProviders.some((provider) => provider.configured)) {
  throw new Error('at least one search provider must be configured');
}
if (String(process.env.ALLOW_DIRECT_FALLBACK || '').toLowerCase() === 'true') {
  throw new Error('ALLOW_DIRECT_FALLBACK is no longer supported; arbitrary domains must stay behind dd-web-scraper');
}

const deadlineMs = Date.now() + config.deadlineSeconds * 1_000;
const AGGREGATOR = /(?:^|\.)(?:yelp|angi|angieslist|homeadvisor|thumbtack|bbb|houzz|facebook|instagram|linkedin|twitter|x|pinterest|youtube|yellowpages|mapquest|nextdoor|indeed|glassdoor|ziprecruiter|tripadvisor|reddit|wikipedia|amazon|google|bing|duckduckgo|porch|expertise|threebestrated|manta|chamberofcommerce|governmentjobs|neogov|patch|monster|careerbuilder|simplyhired|snagajob|usajobs|salary|scionhealth|ihireconstruction|builtin|wellfound|jobcase|recruit|talent)\.[a-z.]+$/i;

function errorSummary(error) {
  const code = typeof error?.code === 'string' ? ` code=${sanitizeLogValue(error.code, 40)}` : '';
  return `${error?.name || 'Error'}${code}`;
}

function providerLog(provider, message) {
  console.warn(`[benefactor-pipeline] provider=${provider} ${sanitizeLogValue(message)}`);
}

async function fetchJson(url, init, { timeoutMs, maxBytes }) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, { ...init, signal: controller.signal });
    if (!response.ok) throw new Error(`upstream_http_${response.status}`);
    return await readJsonCapped(response, {
      maxBytes,
      timeoutMs: config.responseBodyTimeoutMs,
    });
  } finally {
    clearTimeout(timer);
  }
}

async function providerFetchJson(url, init) {
  return fetchJson(url, init, {
    timeoutMs: config.providerTimeoutMs,
    maxBytes: config.maxProviderResponseBytes,
  });
}

async function searchAllProviders(query, count) {
  const batches = [];
  for (const adapter of searchProviders) {
    if (!adapter.configured) continue;
    const status = statuses.find((entry) => entry.provider === adapter.name);
    status.requests += 1;
    try {
      const results = await adapter.search(query, count);
      status.status = 'ok';
      status.resultCount += results.length;
      batches.push({ provider: adapter.name, results });
    } catch (error) {
      status.status = 'degraded_error';
      status.failures += 1;
      counters.providerFailures += 1;
      providerLog(adapter.name, `search_failed ${errorSummary(error)}`);
      batches.push({ provider: adapter.name, results: [] });
    }
  }
  return mergeProviderResults(batches, Math.max(count, config.maxPagesPerQuery));
}

function extractFromHtml(html, baseUrl) {
  const emails = new Set();
  const phones = new Set();
  let businessName = '';
  let contactUrl = null;
  try {
    const $ = cheerio.load(html);
    $('a[href^="mailto:"]').each((_, element) => {
      let email = ($(element).attr('href') || '').replace(/^mailto:/i, '').split('?')[0];
      try { email = decodeURIComponent(email); } catch {}
      const normalized = normalizeEmail(email.split(/[\s,;<>()]/)[0], {
        requireRoleEmail: config.requireRoleEmail,
      });
      if (normalized) emails.add(normalized);
    });
    $('a[href^="tel:"]').each((_, element) => {
      let phone = ($(element).attr('href') || '').replace(/^tel:/i, '').split('?')[0];
      try { phone = decodeURIComponent(phone); } catch {}
      const normalized = normalizePhone(phone);
      if (normalized) phones.add(normalized);
    });
    const title = ($('title').first().text() || '').trim();
    businessName = title
      .replace(/\s*[-|–—]\s*(?:Home|Contact|About|Services|Welcome).*$/i, '')
      .replace(/\s*[-|–—]\s*$/, '')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 200);
    const contactPattern = /\/(?:contact|about|team|connect|get-in-touch|reach-us)(?:\/|$|[?#])/i;
    $('a[href]').each((_, element) => {
      if (contactUrl) return;
      const href = $(element).attr('href') || '';
      if (!contactPattern.test(href)) return;
      try {
        const candidate = normalizeCandidateUrl(new URL(href, baseUrl).toString());
        if (new URL(candidate).origin === new URL(baseUrl).origin) contactUrl = candidate;
      } catch {}
    });
  } catch {
    // Raw extraction below remains available when the document is malformed.
  }
  for (const email of extractEmailsFromText(html, { requireRoleEmail: config.requireRoleEmail })) emails.add(email);
  for (const phone of extractPhonesFromText(html)) phones.add(phone);
  return {
    emails: [...emails].sort(),
    phones: [...phones].sort(),
    businessName,
    contactUrl,
  };
}

async function scrapeViaPrivateService(candidateUrl, strategy) {
  const url = normalizeCandidateUrl(candidateUrl);
  const endpoint = new URL('/scrape', config.scraperUrl);
  const body = await fetchJson(endpoint, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'x-server-auth': config.scraperAuth,
    },
    body: JSON.stringify({
      url,
      strategy,
      includeHtml: true,
      includeText: true,
      includeLinks: true,
      timeoutMs: Math.min(30_000, config.scraperTimeoutMs - 1_000),
      waitUntil: 'domcontentloaded',
      maxHtmlChars: 800_000,
      respectRobots: true,
      rejectPrivateNetwork: true,
    }),
  }, {
    timeoutMs: config.scraperTimeoutMs,
    maxBytes: config.maxScraperResponseBytes,
  });
  if (!body || body.ok === false) return null;
  const extraction = body.extraction || {};
  return {
    html: typeof extraction.html === 'string' ? extraction.html.slice(0, 800_000) : '',
    text: typeof extraction.text === 'string' ? extraction.text.slice(0, 800_000) : '',
    strategy: sanitizeLogValue(body.strategy || strategy, 32),
  };
}

async function fetchPage(candidateUrl) {
  let result = null;
  try {
    result = await scrapeViaPrivateService(candidateUrl, 'cheerio');
  } catch (error) {
    console.warn(`[benefactor-pipeline] scraper=cheerio result=failed ${errorSummary(error)}`);
  }
  if (!result || (result.html.length < 400 && !result.text)) {
    try {
      const playwright = await scrapeViaPrivateService(candidateUrl, 'playwright');
      if (playwright && (playwright.html || playwright.text)) result = playwright;
    } catch (error) {
      console.warn(`[benefactor-pipeline] scraper=playwright result=failed ${errorSummary(error)}`);
    }
  }
  return result;
}

function validateDatabaseUrl(raw) {
  const url = new URL(raw);
  if (!['postgres:', 'postgresql:'].includes(url.protocol)) {
    throw new Error('RDS_URL must use the postgres or postgresql scheme');
  }
  url.searchParams.delete('sslmode');
  url.searchParams.delete('uselibpqcompat');
  return url.toString();
}

const db = new pg.Client({
  connectionString: validateDatabaseUrl(config.rdsUrl),
  ssl: {
    ca: readFileSync(config.pgSslCaFile, 'utf8'),
    rejectUnauthorized: true,
  },
  statement_timeout: config.dbStatementTimeoutMs,
  query_timeout: config.dbStatementTimeoutMs + 5_000,
  application_name: 'benefactor-orchestrator',
});

const counters = {
  queriesLoaded: 0,
  queriesRun: 0,
  urlsVisited: 0,
  pagesWithEmail: 0,
  contactsCollected: 0,
  phonesCollected: 0,
  leadsInserted: 0,
  duplicateLeads: 0,
  suppressedSkips: 0,
  throttledSkips: 0,
  providerFailures: 0,
  persistenceFailures: 0,
};
const collected = new Map();

async function domainSkip(domain) {
  const result = await db.query(
    `select is_blocked, is_permanently_blocked, last_scraped_at
       from benefactor.benefactor_leads_domains
      where domain=$1 and domain_kind='website'
      limit 1`,
    [domain],
  );
  const row = result.rows[0];
  if (!row) return false;
  if (row.is_blocked || row.is_permanently_blocked) return true;
  return Boolean(row.last_scraped_at
    && Date.now() - new Date(row.last_scraped_at).getTime() < config.domainSkipDays * 86_400_000);
}

async function recordDomain(domain, emailCount) {
  if (config.dryRun) return;
  await db.query(
    `insert into benefactor.benefactor_leads_domains
       (domain, domain_kind, status, source, scrape_count, email_found_count, last_scraped_at, last_email_found_at)
     values ($1, 'website', 'scraped_recently', 'orchestrator', 1, $2, now(), $3)
     on conflict (domain, domain_kind) do update set
       scrape_count = benefactor.benefactor_leads_domains.scrape_count + 1,
       email_found_count = benefactor.benefactor_leads_domains.email_found_count + $2,
       status = 'scraped_recently',
       last_scraped_at = now(),
       last_email_found_at = coalesce($3, benefactor.benefactor_leads_domains.last_email_found_at)`,
    [domain, emailCount, emailCount > 0 ? new Date() : null],
  );
}

async function updateQueryStats(queryId, urlsVisited, emailsFound) {
  if (config.dryRun) return;
  await db.query(
    `update benefactor.benefactor_scrape_queries set
       total_runs = total_runs + 1,
       last_run_at = now(),
       total_urls_visited = total_urls_visited + $2,
       total_emails_found = total_emails_found + $3,
       last_run_emails_found = $3,
       last_run_success = $4,
       last_success_at = case when $4 then now() else last_success_at end,
       cooldown_until = now() + make_interval(days => $5::int),
       consecutive_zero_new_runs = case when $3 > 0 then 0 else consecutive_zero_new_runs + 1 end,
       last_zero_new_run_at = case when $3 > 0 then last_zero_new_run_at else now() end,
       is_active = case when $3 = 0 and consecutive_zero_new_runs + 1 >= $6 then false else is_active end,
       updated_at = now()
     where id = $1`,
    [queryId, urlsVisited, emailsFound, emailsFound > 0, config.queryCooldownDays, config.zeroNewRetire],
  );
}

async function persistContact(record) {
  await db.query('begin');
  try {
    const existing = await db.query(
      `select id, lead_status
         from benefactor.benefactor_leads
        where lower(primary_email) = lower($1) and not is_soft_deleted
        order by created_at asc
        limit 1
        for update`,
      [record.email],
    );
    if (existing.rows[0]?.lead_status === 'unsubscribed' || existing.rows[0]?.lead_status === 'do_not_contact') {
      counters.suppressedSkips += 1;
      await db.query('rollback');
      return;
    }

    const throttle = await db.query(
      `select 1
         from benefactor.benefactor_leads_throttling
        where lower(email) = lower($1) and request_type = $2 and not is_soft_deleted
          and (next_allowed_at is null or next_allowed_at > now())
        limit 1
        for update`,
      [record.email, config.scrapeRequestType],
    );
    if (throttle.rows.length) {
      counters.throttledSkips += 1;
      await db.query('rollback');
      return;
    }

    const metadata = {
      benefactorIcpName: record.icpName,
      benefactorIcpSlug: record.icpSlug,
      collectedAt: record.collectedAt,
      confidence: record.confidence,
      pipeline: 'benefactor-orchestrator',
      phones: record.phones,
      provider: record.provider,
      providerRank: record.providerRank,
      scrapeQuery: record.query,
      scrapeQueryRowId: record.queryId,
      scrapeSourceUrl: record.sourceUrl,
      verificationStatus: record.verificationStatus,
    };
    const inserted = await db.query(
      `insert into benefactor.benefactor_leads
         (business_name, primary_email, service_category, city, state, source_url, source_query,
          source_tool, source_engine, tags, meta_data, lead_status, outreach_status)
       values ($1,$2,$3,$4,$5,$6,$7,'orchestrator',$8,$9,$10,'new','pending')
       on conflict (primary_email) where primary_email <> '' do nothing
       returning id`,
      [
        record.businessName,
        record.email,
        record.serviceCategory,
        record.city,
        record.state,
        record.sourceUrl,
        record.query,
        record.provider,
        JSON.stringify([
          'benefactor-scrape',
          'orchestrator',
          `category:${record.serviceCategory}`,
          record.icpSlug ? `icp:${record.icpSlug}` : 'icp:unknown',
          `provider:${record.provider}`,
        ]),
        JSON.stringify(metadata),
      ],
    );
    if (inserted.rows.length) counters.leadsInserted += 1;
    else counters.duplicateLeads += 1;
    const leadId = inserted.rows[0]?.id || existing.rows[0]?.id || null;

    await db.query(
      `insert into benefactor.benefactor_leads_throttling
         (benefactor_lead_id, email, request_type, last_request_at, next_allowed_at,
          request_count, throttle_window_days, last_request_source)
       values ($1,$2,$3,now(),now() + make_interval(days => $4::int),1,$4,'orchestrator')
       on conflict (email, request_type) where is_soft_deleted = false
       do update set
         last_request_at = now(),
         next_allowed_at = now() + make_interval(days => $4::int),
         request_count = benefactor.benefactor_leads_throttling.request_count + 1,
         benefactor_lead_id = coalesce(benefactor.benefactor_leads_throttling.benefactor_lead_id, excluded.benefactor_lead_id),
         updated_at = now()`,
      [leadId, record.email, config.scrapeRequestType, config.scrapeThrottleDays],
    );
    await db.query('commit');
  } catch (error) {
    await db.query('rollback').catch(() => {});
    throw error;
  }
}

async function run() {
  console.log(`[benefactor-pipeline] category=${config.category} mode=${config.dryRun ? 'dry-run' : 'persist'} providers=${statuses.map((item) => `${item.provider}:${item.status}`).join(',')}`);
  await db.connect();
  await db.query('set search_path = benefactor, public');
  const queries = (await db.query(
    `select id, query_text, query_variant, service_category, target_city, target_state,
            benefactor_icp_slug, benefactor_icp_name
       from benefactor.benefactor_scrape_queries
      where service_category=$1 and is_active and not is_soft_deleted
        and (cooldown_until is null or cooldown_until <= now())
      order by priority desc, total_runs asc, id asc
      limit $2`,
    [config.category, config.maxQueries],
  )).rows;
  counters.queriesLoaded = queries.length;

  for (const query of queries) {
    if (collected.size >= config.targetEmails || Date.now() > deadlineMs) break;
    counters.queriesRun += 1;
    let queryVisited = 0;
    const queryNewEmails = new Set();
    const candidates = await searchAllProviders(query.query_text, Math.max(12, config.maxPagesPerQuery * 2));
    const selected = candidates
      .filter((candidate) => !AGGREGATOR.test(candidate.domain))
      .filter((candidate) => !/(\.gov|\.edu|\.mil)$|licens|stateboard|state-board/i.test(candidate.domain))
      .slice(0, config.maxPagesPerQuery);
    console.log(`[benefactor-pipeline] query_id=${query.id} provider_candidates=${candidates.length} selected=${selected.length}`);

    for (const candidate of selected) {
      if (collected.size >= config.targetEmails || Date.now() > deadlineMs) break;
      if (await domainSkip(candidate.domain)) continue;
      counters.urlsVisited += 1;
      queryVisited += 1;
      const firstPage = await fetchPage(candidate.url);
      let contacts = { emails: [], phones: [], businessName: '', contactUrl: null };
      let foundOnContactPage = false;
      if (firstPage && (firstPage.html || firstPage.text)) {
        contacts = extractFromHtml(firstPage.html || `<body>${firstPage.text}</body>`, candidate.url);
        if (!contacts.emails.length && contacts.contactUrl) {
          const contactPage = await fetchPage(contacts.contactUrl);
          if (contactPage && (contactPage.html || contactPage.text)) {
            const followup = extractFromHtml(contactPage.html || `<body>${contactPage.text}</body>`, contacts.contactUrl);
            contacts = {
              emails: followup.emails,
              phones: [...new Set([...contacts.phones, ...followup.phones])].sort(),
              businessName: contacts.businessName || followup.businessName,
              contactUrl: contacts.contactUrl,
            };
            foundOnContactPage = contacts.emails.length > 0;
          }
        }
      }
      await recordDomain(candidate.domain, contacts.emails.length);
      if (contacts.emails.length) counters.pagesWithEmail += 1;
      for (const email of contacts.emails) {
        if (collected.has(email)) continue;
        queryNewEmails.add(email);
        const record = {
          businessName: contacts.businessName,
          city: query.target_city,
          collectedAt: new Date().toISOString(),
          confidence: confidenceForContact({
            email,
            websiteDomain: candidate.domain,
            foundOnContactPage,
          }),
          domain: candidate.domain,
          email,
          icpName: query.benefactor_icp_name,
          icpSlug: query.benefactor_icp_slug,
          phones: contacts.phones,
          provider: candidate.provider,
          providerRank: candidate.providerRank,
          query: query.query_text,
          queryId: query.id,
          serviceCategory: query.service_category,
          sourceUrl: foundOnContactPage && contacts.contactUrl ? contacts.contactUrl : candidate.url,
          state: query.target_state,
          verificationStatus: 'syntax_valid',
        };
        collected.set(email, record);
      }
    }
    await updateQueryStats(query.id, queryVisited, queryNewEmails.size);
  }

  counters.contactsCollected = collected.size;
  counters.phonesCollected = new Set([...collected.values()].flatMap((record) => record.phones)).size;
  if (!config.dryRun) {
    for (const record of collected.values()) {
      try {
        await persistContact(record);
      } catch (error) {
        counters.persistenceFailures += 1;
        console.error(`[benefactor-pipeline] persistence_failed ${errorSummary(error)}`);
      }
    }
  }

  const report = buildDryRunReport({
    category: config.category,
    providers: statuses,
    records: [...collected.values()],
    counters,
  });
  console.log(`BENEFACTOR_PIPELINE_REPORT ${canonicalJson(report)}`);
  console.log(`[benefactor-pipeline] done category=${config.category} mode=${config.dryRun ? 'dry-run' : 'persist'} contacts=${collected.size} inserted=${counters.leadsInserted} report=${report.reportDigest}`);
}

try {
  await run();
} catch (error) {
  console.error(`[benefactor-pipeline] fatal ${errorSummary(error)}`);
  process.exitCode = 1;
} finally {
  await db.end().catch(() => {});
}
