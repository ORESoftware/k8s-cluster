// Benefactor lead-scraping orchestrator (ported from dd-next-1 patterns).
// One run targets a single ICP service_category. Flow per query:
//   Serper search -> candidate business URLs -> skip aggregators + recently-scraped domains
//   -> fetch page via web-scraper service (cheerio, escalate to playwright) with direct fallback
//   -> extract+validate emails (dd-next-1 regex/filters) -> follow one contact subpage
//   -> dedupe -> insert benefactor.benefactor_leads -> update domain memory + query stats.
import { createRequire } from 'node:module';
const require = createRequire('/work/package.json');
const pg = require('pg');
const cheerio = require('cheerio');

const RDS = process.env.RDS_URL;
const SERPER_KEY = process.env.SERPER_API_KEY || '';
const BRAVE_KEY = process.env.BRAVE_SEARCH_API_KEY || '';
const SCRAPER_URL = process.env.SCRAPER_URL || 'http://dd-web-scraper.default.svc.cluster.local:8097';
const SCRAPER_AUTH = process.env.SCRAPER_AUTH || '';
const CATEGORY = process.env.ICP_CATEGORY;
const MAX_QUERIES = parseInt(process.env.MAX_QUERIES || '8', 10);
const TARGET_EMAILS = parseInt(process.env.TARGET_EMAILS || '30', 10);
const MAX_PAGES_PER_QUERY = parseInt(process.env.MAX_PAGES_PER_QUERY || '8', 10);
const DOMAIN_SKIP_DAYS = parseInt(process.env.DOMAIN_SKIP_DAYS || '14', 10);
const DEADLINE_MS = Date.now() + parseInt(process.env.DEADLINE_SECONDS || '420', 10) * 1000;
if (!CATEGORY) { console.error('ICP_CATEGORY required'); process.exit(2); }

// ── email extraction (faithful port of dd-next-1) ─────────────────────────────
const EMAIL_REGEX = /[\w.%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi;
const CONSUMER_WEBMAIL = new Set(['gmail.com','yahoo.com','hotmail.com','outlook.com','aol.com','icloud.com','me.com','live.com','msn.com','comcast.net','att.net','verizon.net','sbcglobal.net','bellsouth.net','cox.net','protonmail.com','ymail.com']);
const BLOCKED_EMAIL_DOMAINS = new Set(['example.com','example.org','example.net','test.com','acme.com','sample.com','website.com','placeholder.com','company.com','mycompany.com','nowhere.net','yourdomain.com','domain.com','email.com','sentry.io','wixpress.com','wix.com','godaddy.com','squarespace.com','shopify.com','weebly.com','wordpress.com','wordpress.org','mailchimp.com','constantcontact.com','hubspot.com','sendgrid.net','sendinblue.com','googleapis.com','cloudflare.com','fastly.net','amazonaws.com','azurewebsites.net','herokuapp.com','mailgun.com','sparkpost.com','postmarkapp.com','mandrillapp.com','amazonses.com','gravatar.com','disqus.com','mailinator.com','guerrillamail.com','tempmail.com','sharklasers.com','dispostable.com','throwaway.email','yopmail.com','trashmail.com','fakeinbox.com','grr.la','tempail.com','temp-mail.org','10minutemail.com','porch.com','angi.com','angieslist.com','homeadvisor.com','thumbtack.com','yelp.com','bbb.org','bark.com','houzz.com','buildzoom.com','networx.com','expertise.com','fixr.com','craftjack.com','servicetitan.com','homeguide.com','barrons.com','benzinga.com','nasdaq.com','marketwatch.com','fool.com','seekingalpha.com','investopedia.com','cnbc.com','bloomberg.com','reuters.com','wsj.com','yahoo.com','finance.yahoo.com','threebestrated.com','consumeraffairs.com','sitejabber.com','bestcompany.com','sentry-next.wixpress.com','sentry.wixpress.com','facebook.com','instagram.com','linkedin.com','twitter.com','x.com','pinterest.com','youtube.com','neogov.com','governmentjobs.com','patch.com','scionhealth.com','latofonts.com','indeed.com','ziprecruiter.com','glassdoor.com','monster.com','careerbuilder.com','salary.com','simplyhired.com','snagajob.com','usajobs.gov','google.com','gstatic.com','schema.org','w3.org','jquery.com','jsdelivr.net','unpkg.com','cloudfront.net','typekit.com','myfonts.com','adobe.com','wpengine.com','elementor.com','cdn-website.com','godaddysites.com','duckduckgo.com','bing.com']);
// Allowlisted TLDs: any 2-letter ccTLD/short TLD plus these common business TLDs. Rejecting
// everything else kills word-bleed artifacts (e.g. "...comno", "...aievery") and .edu/.gov/.mil
// addresses (schools/cities/bases are not benefactor lead targets).
const COMMON_TLDS = new Set(['com','net','org','biz','info','pro','dev','app','xyz','online','tech','site','agency','services','company','solutions','group','team','homes','builders','construction','plumbing','llc','inc','email','live','store','shop','works','care','build','plus','life']);
const BLOCKED_EMAIL_PREFIXES = ['no-reply','noreply','donotreply','do-not-reply','postmaster','mailer-daemon','wordpress','example','user','you','your','name','test','root','hostmaster','abuse','sentry'];
const BLOCKED_PATH_EXT = /\.(?:png|jpg|jpeg|gif|webp|svg|css|js|ico|woff2?|ttf|otf|eot)$/i;

function deobfuscate(text) {
  return text
    // Neutralize JSON unicode escapes (> = '>', etc.) so they can't bleed into a local-part.
    .replace(/\\u[0-9a-fA-F]{4}/g, ' ')
    .replace(/&commat;|&#64;|&#x40;/gi, '@').replace(/&#46;|&#x2e;/gi, '.')
    .replace(/\s*[[({]\s*at\s*[\])}]\s*/gi, '@').replace(/\s*[[({]\s*dot\s*[\])}]\s*/gi, '.')
    .replace(/([A-Z0-9._%+-])\s+at\s+([A-Z0-9.-]+\s+(?:dot|\.))/gi, '$1@$2')
    .replace(/([A-Z0-9_-])\s+dot\s+([A-Z]{2,})/gi, '$1.$2');
}
function isValidEmail(email) {
  const lower = email.toLowerCase().trim();
  if (lower.length < 6 || lower.length > 254) return false;
  const at = lower.indexOf('@');
  if (at < 1 || at !== lower.lastIndexOf('@')) return false;
  const local = lower.slice(0, at), domain = lower.slice(at + 1);
  if (!local || !domain || !domain.includes('.')) return false;
  if (local.length > 40 || /\.\./.test(local) || local.startsWith('.') || local.endsWith('.')) return false;
  const tld = domain.slice(domain.lastIndexOf('.') + 1);
  if (tld.length < 2 || tld.length > 24) return false;
  if (!(/^[a-z]{2}$/.test(tld) || COMMON_TLDS.has(tld))) return false;
  if (BLOCKED_PATH_EXT.test(domain)) return false;
  if (/[^\x20-\x7E]/.test(lower) || domain.startsWith('xn--')) return false;
  if (/sentry/.test(domain) || domain.endsWith('.wixpress.com')) return false;
  if (BLOCKED_EMAIL_DOMAINS.has(domain) && !CONSUMER_WEBMAIL.has(domain)) return false;
  for (const p of BLOCKED_EMAIL_PREFIXES) if (local === p || local.startsWith(p + '.') || local.startsWith(p + '+')) return false;
  if (/^\d+$/.test(local)) return false;
  return true;
}
function emailsFromText(text) {
  const out = new Set();
  for (const raw of (deobfuscate(text || '').match(EMAIL_REGEX) || [])) {
    const clean = raw.toLowerCase().replace(/[.,;:)]+$/, '');
    if (isValidEmail(clean)) out.add(clean);
  }
  return out;
}
function extractFromHtml(html, baseUrl) {
  const out = new Set();
  let businessName = '', contactUrl = null;
  try {
    const $ = cheerio.load(html);
    $('a[href^="mailto:"]').each((_, el) => {
      let e = ($(el).attr('href') || '').replace(/^mailto:/i, '').split('?')[0];
      try { e = decodeURIComponent(e); } catch {}
      e = e.split(/[\s,;<>()]/)[0].trim().toLowerCase();
      if (isValidEmail(e)) out.add(e);
    });
    const t = ($('title').first().text() || '').trim();
    businessName = t.replace(/\s*[-|–—]\s*(?:Home|Contact|About|Services|Welcome).*$/i, '').replace(/\s*[-|–—]\s*$/, '').trim().slice(0, 200);
    const re = /\/(?:contact|about|team|connect|get-in-touch|reach-us)(?:\/|$|[?#])/i;
    $('a[href]').each((_, el) => {
      if (contactUrl) return;
      const href = $(el).attr('href') || '';
      if (re.test(href)) { try { const r = new URL(href, baseUrl); if (r.origin === new URL(baseUrl).origin) contactUrl = r.href; } catch {} }
    });
  } catch { /* fall through to raw-html regex below */ }
  // Harvest emails by regexing the RAW html (not cheerio .text()): tag boundaries (`<`, `>`, `"`)
  // delimit the match, so adjacent words/labels can't bleed into the email — this removes the
  // earlier artifacts like `x@gmail.comaddress` and `serviceservice@lameyelectric.com`.
  for (const e of emailsFromText(html)) out.add(e);
  return { emails: [...out], businessName, contactUrl };
}

// ── search ────────────────────────────────────────────────────────────────────
const AGGREGATOR = /(?:^|\.)(?:yelp|angi|angieslist|homeadvisor|thumbtack|bbb|houzz|facebook|instagram|linkedin|twitter|x|pinterest|youtube|yellowpages|mapquest|nextdoor|indeed|glassdoor|ziprecruiter|tripadvisor|reddit|wikipedia|amazon|google|bing|duckduckgo|porch|expertise|threebestrated|manta|chamberofcommerce|governmentjobs|neogov|patch|monster|careerbuilder|simplyhired|snagajob|usajobs|salary|scionhealth|ihireconstruction|builtin|wellfound|jobcase|recruit|talent)\.[a-z.]+$/i;
function hostOf(u) { try { return new URL(u).hostname.replace(/^www\./, '').toLowerCase(); } catch { return null; } }
function normUrl(u) { try { const x = new URL(u); if (!/^https?:$/.test(x.protocol)) return null; x.hash=''; return x.toString(); } catch { return null; } }

async function serper(q, num) {
  if (!SERPER_KEY) return [];
  try {
    const ctl = new AbortController(); const t = setTimeout(() => ctl.abort(), 15000);
    const res = await fetch('https://google.serper.dev/search', { method: 'POST', headers: { 'X-API-KEY': SERPER_KEY, 'Content-Type': 'application/json' }, body: JSON.stringify({ q, num: Math.min(num, 10), gl: 'us', hl: 'en' }), signal: ctl.signal });
    clearTimeout(t);
    if (!res.ok) { const b = await res.text().catch(() => ''); console.log(`  serper HTTP ${res.status} ${b.slice(0, 160)}`); return []; }
    const j = await res.json();
    return (j.organic || []).map((o) => normUrl(o.link)).filter(Boolean);
  } catch (e) { console.log('  serper err', e.message); return []; }
}
async function brave(q, num) {
  if (!BRAVE_KEY) return [];
  try {
    const ctl = new AbortController(); const t = setTimeout(() => ctl.abort(), 15000);
    const res = await fetch(`https://api.search.brave.com/res/v1/web/search?q=${encodeURIComponent(q)}&count=${num}`, { headers: { 'X-Subscription-Token': BRAVE_KEY, Accept: 'application/json' }, signal: ctl.signal });
    clearTimeout(t);
    if (!res.ok) return [];
    const j = await res.json();
    return ((j.web && j.web.results) || []).map((o) => normUrl(o.url)).filter(Boolean);
  } catch { return []; }
}

// ── page fetch via web-scraper service, fallback to direct fetch ───────────────
async function scrapeViaService(url, strategy) {
  const ctl = new AbortController(); const t = setTimeout(() => ctl.abort(), 45000);
  try {
    const res = await fetch(`${SCRAPER_URL}/scrape`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'x-server-auth': SCRAPER_AUTH },
      body: JSON.stringify({ url, strategy, includeHtml: true, includeText: true, includeLinks: true, timeoutMs: 30000, waitUntil: 'domcontentloaded', maxHtmlChars: 800000 }),
      signal: ctl.signal,
    });
    clearTimeout(t);
    if (!res.ok) return null;
    const j = await res.json();
    if (!j || j.ok === false) return null;
    const ex = j.extraction || {};
    return { html: ex.html || '', text: ex.text || '', strategy: j.strategy };
  } catch { clearTimeout(t); return null; }
}
async function scrapeDirect(url) {
  const ctl = new AbortController(); const t = setTimeout(() => ctl.abort(), 15000);
  try {
    const res = await fetch(url, { headers: { 'User-Agent': 'Mozilla/5.0 (compatible; benefactor-leadbot/1.0)', Accept: 'text/html,application/xhtml+xml' }, redirect: 'follow', signal: ctl.signal });
    clearTimeout(t);
    if (!res.ok) return null;
    const ct = res.headers.get('content-type') || '';
    if (ct && !/text|html/.test(ct)) return null;
    const html = (await res.text()).slice(0, 800000);
    return { html, text: '', strategy: 'direct' };
  } catch { clearTimeout(t); return null; }
}
async function fetchPage(url) {
  // cheerio (fast, no browser) -> if thin/no-email escalate to playwright -> direct fallback
  let r = await scrapeViaService(url, 'cheerio');
  if (!r || (r.html.length < 400 && !r.text)) {
    const pw = await scrapeViaService(url, 'playwright');
    if (pw && (pw.html || pw.text)) r = pw;
  }
  if (!r || (!r.html && !r.text)) r = await scrapeDirect(url);
  return r;
}

// ── main ───────────────────────────────────────────────────────────────────────
const db = new pg.Client({ connectionString: RDS, ssl: { rejectUnauthorized: false }, statement_timeout: 0 });
await db.connect();
await db.query(`set search_path = benefactor, public`);

const queries = (await db.query(
  `select id, query_text, query_variant, service_category, target_city, target_state, benefactor_icp_slug, benefactor_icp_name
   from benefactor.benefactor_scrape_queries
   where service_category=$1 and is_active and not is_soft_deleted
     and (cooldown_until is null or cooldown_until <= now())
   order by priority desc, total_runs asc, last_run_at asc nulls first
   limit $2`, [CATEGORY, MAX_QUERIES])).rows;
console.log(`[${CATEGORY}] loaded ${queries.length} queries`);

async function domainSkip(domain) {
  const r = await db.query(`select status, is_blocked, is_permanently_blocked, last_scraped_at from benefactor.benefactor_leads_domains where domain=$1 and domain_kind='website' limit 1`, [domain]);
  const row = r.rows[0]; if (!row) return false;
  if (row.is_blocked || row.is_permanently_blocked) return true;
  if (row.last_scraped_at && (Date.now() - new Date(row.last_scraped_at).getTime()) < DOMAIN_SKIP_DAYS * 86400000) return true;
  return false;
}
async function recordDomain(domain, { found, emails }) {
  await db.query(
    `insert into benefactor.benefactor_leads_domains (domain, domain_kind, status, source, scrape_count, email_found_count, last_scraped_at, last_email_found_at)
     values ($1,'website',$2,'orchestrator',1,$3, now(), $4)
     on conflict (domain, domain_kind) do update set
       scrape_count = benefactor.benefactor_leads_domains.scrape_count + 1,
       email_found_count = benefactor.benefactor_leads_domains.email_found_count + $3,
       status = $2, last_scraped_at = now(),
       last_email_found_at = coalesce($4, benefactor.benefactor_leads_domains.last_email_found_at)`,
    [domain, found ? 'scraped_recently' : 'scraped_recently', emails, found ? new Date() : null]);
}

const collected = new Map(); // email -> lead record
let urlsVisited = 0, pagesWithEmail = 0;

for (const q of queries) {
  if (collected.size >= TARGET_EMAILS || Date.now() > DEADLINE_MS) break;
  let qFound = 0, qVisited = 0;
  let urls = await serper(q.query_text, 12);
  if (urls.length === 0) urls = await brave(q.query_text, 12);
  // unique business domains, skip aggregators
  const seen = new Set(); const pick = [];
  for (const u of urls) {
    const h = hostOf(u); if (!h || AGGREGATOR.test(h) || seen.has(h)) continue;
    // Skip non-business pages: government/edu sites and licensing boards/directories are not leads.
    if (/(\.gov|\.edu|\.mil)$|licens|stateboard|state-board/i.test(h)) continue;
    seen.add(h); pick.push(u); if (pick.length >= MAX_PAGES_PER_QUERY) break;
  }
  console.log(`[${CATEGORY}] q="${q.query_text.slice(0,60)}" -> ${urls.length} results, ${pick.length} business urls`);
  for (const url of pick) {
    if (collected.size >= TARGET_EMAILS || Date.now() > DEADLINE_MS) break;
    const domain = hostOf(url); if (!domain) continue;
    if (await domainSkip(domain)) continue;
    urlsVisited++; qVisited++;
    const page = await fetchPage(url);
    let found = new Set();
    if (page && (page.html || page.text)) {
      const r1 = extractFromHtml(page.html || `<body>${page.text}</body>`, url);
      r1.emails.forEach((e) => found.add(e));
      if (found.size === 0 && r1.contactUrl) {
        const cp = await fetchPage(r1.contactUrl);
        if (cp && (cp.html || cp.text)) extractFromHtml(cp.html || `<body>${cp.text}</body>`, r1.contactUrl).emails.forEach((e) => found.add(e));
      }
      var bizName = r1.businessName;
    }
    await recordDomain(domain, { found: found.size > 0, emails: found.size });
    if (found.size > 0) pagesWithEmail++;
    for (const email of found) {
      qFound++;
      if (collected.has(email)) continue;
      collected.set(email, { email, url, domain, bizName: bizName || '', q });
    }
  }
  await db.query(
    `update benefactor.benefactor_scrape_queries set total_runs=total_runs+1, last_run_at=now(),
       total_urls_visited=total_urls_visited+$2, total_emails_found=total_emails_found+$3,
       last_run_emails_found=$3, last_run_success=$4 where id=$1`,
    [q.id, qVisited, qFound, qFound > 0]);
}

// ── persist leads ───────────────────────────────────────────────────────────────
let inserted = 0;
for (const rec of collected.values()) {
  try {
    const res = await db.query(
      `insert into benefactor.benefactor_leads
         (business_name, primary_email, service_category, city, state, source_url, source_query, source_tool, source_engine, tags, meta_data, lead_status, outreach_status)
       values ($1,$2,$3,$4,$5,$6,$7,'orchestrator','serper',$8,$9,'new','pending')
       on conflict (primary_email) where primary_email <> '' do nothing
       returning id`,
      [rec.bizName, rec.email, rec.q.service_category, rec.q.target_city, rec.q.target_state, rec.url, rec.q.query_text,
       JSON.stringify(['benefactor-scrape','orchestrator', `category:${rec.q.service_category}`, rec.q.benefactor_icp_slug ? `icp:${rec.q.benefactor_icp_slug}` : 'icp:unknown']),
       JSON.stringify({ scrapeSourceUrl: rec.url, scrapeQuery: rec.q.query_text, scrapeQueryRowId: rec.q.id, benefactorIcpSlug: rec.q.benefactor_icp_slug, benefactorIcpName: rec.q.benefactor_icp_name, pipeline: 'benefactor-orchestrator', importedAt: new Date().toISOString() })]);
    if (res.rows.length) inserted++;
  } catch (e) { console.log('  insert skip:', e.message.split('\n')[0]); }
}

console.log(`\n[${CATEGORY}] DONE queries=${queries.length} urlsVisited=${urlsVisited} pagesWithEmail=${pagesWithEmail} emailsCollected=${collected.size} leadsInserted=${inserted}`);
await db.end();
