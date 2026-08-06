import { createHash } from 'node:crypto';
import { isIP } from 'node:net';

export const PIPELINE_REPORT_VERSION = 'benefactor.pipeline.dry-run.v1';
export const DEFAULT_MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
export const DEFAULT_RESPONSE_BODY_TIMEOUT_MS = 20_000;

const BLOCKED_HOSTS = new Set([
  'localhost',
  'localhost.localdomain',
  'metadata.google.internal',
  'metadata.azure.internal',
  'instance-data.ec2.internal',
]);

const BLOCKED_HOST_SUFFIXES = [
  '.localhost',
  '.local',
  '.internal',
  '.intranet',
  '.home',
  '.lan',
  '.test',
  '.example',
  '.invalid',
  '.onion',
  '.svc',
  '.svc.cluster.local',
  '.cluster.local',
  '.arpa',
];

const CONSUMER_WEBMAIL = new Set([
  'gmail.com', 'yahoo.com', 'hotmail.com', 'outlook.com', 'aol.com', 'icloud.com',
  'me.com', 'live.com', 'msn.com', 'comcast.net', 'att.net', 'verizon.net',
  'sbcglobal.net', 'bellsouth.net', 'cox.net', 'protonmail.com', 'ymail.com',
]);

const BLOCKED_EMAIL_DOMAINS = new Set([
  'example.com', 'example.org', 'example.net', 'test.com', 'acme.com', 'sample.com',
  'website.com', 'placeholder.com', 'company.com', 'mycompany.com', 'nowhere.net',
  'yourdomain.com', 'domain.com', 'email.com', 'sentry.io', 'wixpress.com', 'wix.com',
  'godaddy.com', 'squarespace.com', 'shopify.com', 'weebly.com', 'wordpress.com',
  'wordpress.org', 'mailchimp.com', 'constantcontact.com', 'hubspot.com',
  'sendgrid.net', 'sendinblue.com', 'googleapis.com', 'cloudflare.com', 'fastly.net',
  'amazonaws.com', 'azurewebsites.net', 'herokuapp.com', 'mailgun.com',
  'sparkpost.com', 'postmarkapp.com', 'mandrillapp.com', 'amazonses.com',
  'gravatar.com', 'disqus.com', 'mailinator.com', 'guerrillamail.com', 'tempmail.com',
  'sharklasers.com', 'dispostable.com', 'throwaway.email', 'yopmail.com',
  'trashmail.com', 'fakeinbox.com', 'grr.la', 'tempail.com', 'temp-mail.org',
  '10minutemail.com', 'porch.com', 'angi.com', 'angieslist.com', 'homeadvisor.com',
  'thumbtack.com', 'yelp.com', 'bbb.org', 'bark.com', 'houzz.com', 'buildzoom.com',
  'networx.com', 'expertise.com', 'fixr.com', 'craftjack.com', 'servicetitan.com',
  'homeguide.com', 'barrons.com', 'benzinga.com', 'nasdaq.com', 'marketwatch.com',
  'fool.com', 'seekingalpha.com', 'investopedia.com', 'cnbc.com', 'bloomberg.com',
  'reuters.com', 'wsj.com', 'finance.yahoo.com', 'threebestrated.com',
  'consumeraffairs.com', 'sitejabber.com', 'bestcompany.com', 'sentry-next.wixpress.com',
  'sentry.wixpress.com', 'facebook.com', 'instagram.com', 'linkedin.com', 'twitter.com',
  'x.com', 'pinterest.com', 'youtube.com', 'neogov.com', 'governmentjobs.com',
  'patch.com', 'scionhealth.com', 'latofonts.com', 'indeed.com', 'ziprecruiter.com',
  'glassdoor.com', 'monster.com', 'careerbuilder.com', 'salary.com', 'simplyhired.com',
  'snagajob.com', 'usajobs.gov', 'google.com', 'gstatic.com', 'schema.org', 'w3.org',
  'jquery.com', 'jsdelivr.net', 'unpkg.com', 'cloudfront.net', 'typekit.com',
  'myfonts.com', 'adobe.com', 'wpengine.com', 'elementor.com', 'cdn-website.com',
  'godaddysites.com', 'duckduckgo.com', 'bing.com',
]);

const BLOCKED_EMAIL_PREFIXES = [
  'no-reply', 'noreply', 'donotreply', 'do-not-reply', 'postmaster', 'mailer-daemon',
  'wordpress', 'example', 'user', 'you', 'your', 'name', 'test', 'root', 'hostmaster',
  'abuse', 'sentry',
];

const ROLE_EMAIL_PREFIXES = new Set([
  'admin', 'appointments', 'booking', 'business', 'care', 'contact', 'customerservice',
  'estimates', 'hello', 'help', 'info', 'inquiries', 'marketing', 'office', 'operations',
  'owner', 'partnerships', 'quotes', 'reception', 'sales', 'service', 'support', 'team',
]);

const COMMON_TLDS = new Set([
  'com', 'net', 'org', 'biz', 'info', 'pro', 'dev', 'app', 'xyz', 'online', 'tech',
  'site', 'agency', 'services', 'company', 'solutions', 'group', 'team', 'homes',
  'builders', 'construction', 'plumbing', 'llc', 'inc', 'email', 'live', 'store',
  'shop', 'works', 'care', 'build', 'plus', 'life',
]);

const EMAIL_REGEX = /[\w.%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi;
const PHONE_CANDIDATE_REGEX = /(?:\+?\d[\d().\s-]{7,}\d)/g;
const BLOCKED_PATH_EXT = /\.(?:png|jpg|jpeg|gif|webp|svg|css|js|ico|woff2?|ttf|otf|eot)$/i;

export class PipelineConfigError extends Error {
  constructor(message) {
    super(message);
    this.name = 'PipelineConfigError';
  }
}

export class ResponseLimitError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ResponseLimitError';
  }
}

export function parseBoolean(value, defaultValue = false) {
  if (value === undefined || value === null || value === '') return defaultValue;
  const normalized = String(value).trim().toLowerCase();
  if (['1', 'true', 'yes', 'on'].includes(normalized)) return true;
  if (['0', 'false', 'no', 'off'].includes(normalized)) return false;
  throw new PipelineConfigError(`invalid boolean value: ${normalized}`);
}

export function parseBoundedInteger(name, value, {
  defaultValue,
  min,
  max,
} = {}) {
  const raw = value === undefined || value === null || value === '' ? defaultValue : value;
  if (raw === undefined) throw new PipelineConfigError(`${name} is required`);
  if (!/^-?\d+$/.test(String(raw).trim())) {
    throw new PipelineConfigError(`${name} must be an integer`);
  }
  const parsed = Number.parseInt(String(raw), 10);
  if (!Number.isSafeInteger(parsed)) throw new PipelineConfigError(`${name} is outside the safe integer range`);
  if (min !== undefined && parsed < min) throw new PipelineConfigError(`${name} must be >= ${min}`);
  if (max !== undefined && parsed > max) throw new PipelineConfigError(`${name} must be <= ${max}`);
  return parsed;
}

export function sanitizeLogValue(value, maxLength = 160) {
  const text = String(value ?? '').replace(/[\r\n\t]+/g, ' ').replace(/\s+/g, ' ').trim();
  return text.slice(0, maxLength);
}

function normalizeHostname(hostname) {
  return hostname.toLowerCase().replace(/\.$/, '').replace(/^\[(.*)\]$/, '$1');
}

export function assertPublicHostname(hostname) {
  const host = normalizeHostname(hostname);
  if (!host || host.length > 253) throw new PipelineConfigError('candidate URL has an invalid hostname');
  if (isIP(host) !== 0) throw new PipelineConfigError('candidate URL must not use an IP literal');
  if (BLOCKED_HOSTS.has(host)) throw new PipelineConfigError('candidate URL points at a blocked host');
  if (BLOCKED_HOST_SUFFIXES.some((suffix) => host.endsWith(suffix))) {
    throw new PipelineConfigError('candidate URL points at a private or non-public host suffix');
  }
  if (host.includes('..') || host.startsWith('.') || host.endsWith('.')) {
    throw new PipelineConfigError('candidate URL has an invalid hostname');
  }
  if (!host.includes('.')) throw new PipelineConfigError('candidate URL must use a public fully-qualified hostname');
  const labels = host.split('.');
  for (const label of labels) {
    if (!label || label.length > 63 || !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label)) {
      throw new PipelineConfigError('candidate URL has an invalid DNS label');
    }
    if (label.startsWith('xn--')) throw new PipelineConfigError('candidate URL IDN labels are not allowed');
  }
  return host;
}

export function normalizeCandidateUrl(value) {
  const raw = String(value ?? '').trim();
  if (!raw || raw.length > 2048) throw new PipelineConfigError('candidate URL is empty or too long');
  let parsed;
  try {
    parsed = new URL(raw);
  } catch {
    throw new PipelineConfigError('candidate URL is invalid');
  }
  if (!['http:', 'https:'].includes(parsed.protocol)) {
    throw new PipelineConfigError('candidate URL must use HTTP or HTTPS');
  }
  if (parsed.username || parsed.password) throw new PipelineConfigError('candidate URL must not contain credentials');
  if (parsed.port && !['80', '443'].includes(parsed.port)) {
    throw new PipelineConfigError('candidate URL must use a standard HTTP port');
  }
  const host = assertPublicHostname(parsed.hostname);
  parsed.hostname = host;
  parsed.hash = '';
  return parsed.toString();
}

export function hostOf(value) {
  try {
    return assertPublicHostname(new URL(normalizeCandidateUrl(value)).hostname).replace(/^www\./, '');
  } catch {
    return null;
  }
}

export function normalizeScraperServiceUrl(value, allowedHosts = ['dd-web-scraper.default.svc.cluster.local']) {
  let parsed;
  try {
    parsed = new URL(String(value ?? '').trim());
  } catch {
    throw new PipelineConfigError('SCRAPER_URL is invalid');
  }
  if (!['http:', 'https:'].includes(parsed.protocol)) {
    throw new PipelineConfigError('SCRAPER_URL must use HTTP or HTTPS');
  }
  if (parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new PipelineConfigError('SCRAPER_URL must not contain credentials, query parameters, or fragments');
  }
  if (parsed.pathname !== '/' && parsed.pathname !== '') {
    throw new PipelineConfigError('SCRAPER_URL must be an origin without a path');
  }
  const host = normalizeHostname(parsed.hostname);
  const normalizedAllowedHosts = new Set(
    allowedHosts.map((item) => normalizeHostname(String(item).trim())).filter(Boolean),
  );
  if (!normalizedAllowedHosts.has(host)) {
    throw new PipelineConfigError('SCRAPER_URL host is not in SCRAPER_ALLOWED_HOSTS');
  }
  if (parsed.protocol === 'http:' && !host.endsWith('.svc.cluster.local')) {
    throw new PipelineConfigError('plain HTTP scraper traffic is restricted to cluster Service DNS');
  }
  parsed.hostname = host;
  parsed.pathname = '/';
  return parsed.toString();
}

function parseContentLength(response) {
  const raw = response.headers?.get?.('content-length');
  if (!raw) return null;
  if (!/^\d+$/.test(raw)) return null;
  const parsed = Number.parseInt(raw, 10);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function readWithDeadline(reader, deadlineMs) {
  const remainingMs = deadlineMs - Date.now();
  if (remainingMs <= 0) {
    return Promise.reject(new ResponseLimitError('upstream response body timed out'));
  }
  let timeout;
  return Promise.race([
    reader.read(),
    new Promise((_, reject) => {
      timeout = setTimeout(() => reject(new ResponseLimitError('upstream response body timed out')), remainingMs);
    }),
  ]).finally(() => clearTimeout(timeout));
}

export async function readBodyCapped(response, {
  maxBytes = DEFAULT_MAX_RESPONSE_BYTES,
  timeoutMs = DEFAULT_RESPONSE_BODY_TIMEOUT_MS,
} = {}) {
  const declared = parseContentLength(response);
  if (declared !== null && declared > maxBytes) {
    throw new ResponseLimitError(`upstream response exceeds ${maxBytes} bytes`);
  }
  if (!response.body) return '';
  const reader = response.body.getReader();
  const deadlineMs = Date.now() + timeoutMs;
  const chunks = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await readWithDeadline(reader, deadlineMs);
      if (done) break;
      if (!value) continue;
      total += value.byteLength;
      if (total > maxBytes) {
        throw new ResponseLimitError(`upstream response exceeds ${maxBytes} bytes`);
      }
      chunks.push(Buffer.from(value));
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks, total).toString('utf8');
}

export async function readJsonCapped(response, options) {
  const body = await readBodyCapped(response, options);
  try {
    return JSON.parse(body);
  } catch {
    throw new ResponseLimitError('upstream returned invalid JSON');
  }
}

export function deobfuscateContactText(text) {
  return String(text ?? '')
    .replace(/\\u[0-9a-fA-F]{4}/g, ' ')
    .replace(/&commat;|&#64;|&#x40;/gi, '@')
    .replace(/&#46;|&#x2e;/gi, '.')
    .replace(/\s*[[({]\s*at\s*[\])}]\s*/gi, '@')
    .replace(/\s*[[({]\s*dot\s*[\])}]\s*/gi, '.')
    .replace(/([A-Z0-9._%+-])\s+at\s+([A-Z0-9.-]+\s+(?:dot|\.))/gi, '$1@$2')
    .replace(/([A-Z0-9_-])\s+dot\s+([A-Z]{2,})/gi, '$1.$2');
}

export function normalizeEmail(value, { requireRoleEmail = true } = {}) {
  const lower = String(value ?? '').toLowerCase().trim().replace(/[.,;:)]+$/, '');
  if (lower.length < 6 || lower.length > 254) return null;
  const at = lower.indexOf('@');
  if (at < 1 || at !== lower.lastIndexOf('@')) return null;
  const local = lower.slice(0, at);
  const domain = lower.slice(at + 1);
  if (!local || !domain || !domain.includes('.')) return null;
  if (local.length > 64 || /\.\./.test(local) || local.startsWith('.') || local.endsWith('.')) return null;
  if (!/^[a-z0-9.!#$%&'*+/=?^_`{|}~-]+$/.test(local)) return null;
  if (!/^[a-z0-9.-]+$/.test(domain) || domain.includes('..') || domain.startsWith('.') || domain.endsWith('.')) return null;
  const tld = domain.slice(domain.lastIndexOf('.') + 1);
  if (tld.length < 2 || tld.length > 24) return null;
  if (!(/^[a-z]{2}$/.test(tld) || COMMON_TLDS.has(tld))) return null;
  if (BLOCKED_PATH_EXT.test(domain) || domain.startsWith('xn--')) return null;
  if (domain.includes('sentry') || domain.endsWith('.wixpress.com')) return null;
  if (CONSUMER_WEBMAIL.has(domain) || BLOCKED_EMAIL_DOMAINS.has(domain)) return null;
  for (const prefix of BLOCKED_EMAIL_PREFIXES) {
    if (local === prefix || local.startsWith(`${prefix}.`) || local.startsWith(`${prefix}+`)) return null;
  }
  const rolePrefix = local.split('+', 1)[0].split(/[._-]/, 1)[0];
  if (requireRoleEmail && !ROLE_EMAIL_PREFIXES.has(rolePrefix)) return null;
  if (/^\d+$/.test(local)) return null;
  return lower;
}

export function extractEmailsFromText(text, options) {
  const out = new Set();
  for (const raw of deobfuscateContactText(text).match(EMAIL_REGEX) ?? []) {
    const email = normalizeEmail(raw, options);
    if (email) out.add(email);
  }
  return [...out].sort();
}

export function normalizePhone(value) {
  const raw = String(value ?? '').trim();
  if (!raw) return null;
  const hasPlus = raw.startsWith('+');
  const digits = raw.replace(/\D/g, '');
  if (digits.length < 10 || digits.length > 15) return null;
  if (/^(\d)\1+$/.test(digits)) return null;
  if (!hasPlus && digits.length === 10) return `+1${digits}`;
  if (!hasPlus && digits.length === 11 && digits.startsWith('1')) return `+${digits}`;
  return hasPlus ? `+${digits}` : null;
}

export function extractPhonesFromText(text) {
  const out = new Set();
  for (const raw of String(text ?? '').match(PHONE_CANDIDATE_REGEX) ?? []) {
    const phone = normalizePhone(raw);
    if (phone) out.add(phone);
  }
  return [...out].sort();
}

export function confidenceForContact({ email, websiteDomain, foundOnContactPage = false }) {
  const emailDomain = String(email ?? '').split('@')[1] ?? '';
  let confidence = foundOnContactPage ? 0.9 : 0.8;
  if (emailDomain === websiteDomain || emailDomain.endsWith(`.${websiteDomain}`)) confidence += 0.05;
  return Math.min(0.99, Number(confidence.toFixed(2)));
}

export function providerStatuses(config) {
  return [
    {
      provider: 'serper',
      kind: 'search',
      enabled: Boolean(config.serperKey),
      status: config.serperKey ? 'configured' : 'disabled_missing_credentials',
      requests: 0,
      resultCount: 0,
      failures: 0,
    },
    {
      provider: 'brave',
      kind: 'search',
      enabled: Boolean(config.braveKey),
      status: config.braveKey ? 'configured' : 'disabled_missing_credentials',
      requests: 0,
      resultCount: 0,
      failures: 0,
    },
    {
      provider: 'apollo',
      kind: 'enrichment',
      enabled: false,
      status: config.apolloKey ? 'disabled_adapter_not_implemented' : 'disabled_missing_credentials',
      requests: 0,
      resultCount: 0,
      failures: 0,
    },
    {
      provider: 'hunter',
      kind: 'verification',
      enabled: false,
      status: config.hunterKey ? 'disabled_adapter_not_implemented' : 'disabled_missing_credentials',
      requests: 0,
      resultCount: 0,
      failures: 0,
    },
    {
      provider: 'datalane',
      kind: 'enrichment',
      enabled: false,
      status: config.datalaneKey ? 'disabled_adapter_not_implemented' : 'disabled_missing_credentials',
      requests: 0,
      resultCount: 0,
      failures: 0,
    },
    {
      provider: 'linkedin-sales-navigator',
      kind: 'authorized_import',
      enabled: false,
      status: config.linkedinExportPath ? 'disabled_adapter_not_implemented' : 'disabled_missing_authorized_export',
      requests: 0,
      resultCount: 0,
      failures: 0,
    },
  ].sort((a, b) => a.provider.localeCompare(b.provider));
}

export function mergeProviderResults(batches, maxResults = 50) {
  const byDomain = new Map();
  for (const batch of batches) {
    const provider = String(batch.provider ?? '').toLowerCase();
    const results = Array.isArray(batch.results) ? batch.results : [];
    for (let index = 0; index < results.length; index += 1) {
      const raw = typeof results[index] === 'string' ? results[index] : results[index]?.url;
      try {
        const url = normalizeCandidateUrl(raw);
        const domain = hostOf(url);
        if (!domain || byDomain.has(domain)) continue;
        byDomain.set(domain, {
          url,
          domain,
          provider,
          providerRank: index + 1,
        });
      } catch {
        // Search providers can return cached/internal/invalid links; reject each candidate independently.
      }
    }
  }
  return [...byDomain.values()]
    .sort((a, b) => a.provider.localeCompare(b.provider) || a.providerRank - b.providerRank || a.url.localeCompare(b.url))
    .slice(0, maxResults);
}

function stableValue(value) {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, child]) => [key, stableValue(child)]),
    );
  }
  return value;
}

export function canonicalJson(value) {
  return JSON.stringify(stableValue(value));
}

export function sha256(value) {
  return createHash('sha256').update(String(value)).digest('hex');
}

export function buildDryRunReport({ category, providers, records, counters }) {
  const normalizedRecords = records.map((record) => {
    const email = String(record.email ?? '').toLowerCase();
    const emailDomain = email.split('@')[1] ?? '';
    const phones = [...new Set(record.phones ?? [])]
      .map((phone) => sha256(String(phone)))
      .sort();
    return {
      businessNameSha256: sha256(String(record.businessName ?? '').slice(0, 200)),
      confidence: Number(record.confidence ?? 0),
      domainSha256: sha256(String(record.domain ?? '').toLowerCase()),
      emailDomainSha256: sha256(emailDomain),
      emailSha256: sha256(email),
      phoneSha256: phones,
      provider: String(record.provider ?? ''),
      providerRank: Number(record.providerRank ?? 0),
      queryId: String(record.queryId ?? ''),
      querySha256: sha256(String(record.query ?? '').slice(0, 500)),
      sourceUrlSha256: sha256(String(record.sourceUrl ?? '')),
      verificationStatus: String(record.verificationStatus ?? 'unverified'),
    };
  }).sort((a, b) =>
    a.emailSha256.localeCompare(b.emailSha256)
    || a.domainSha256.localeCompare(b.domainSha256)
    || a.sourceUrlSha256.localeCompare(b.sourceUrlSha256));

  const body = {
    categorySha256: sha256(String(category ?? '')),
    counters: stableValue(counters ?? {}),
    providers: (providers ?? []).map((provider) => ({
      enabled: Boolean(provider.enabled),
      failures: Number(provider.failures ?? 0),
      kind: String(provider.kind ?? ''),
      provider: String(provider.provider ?? ''),
      requests: Number(provider.requests ?? 0),
      resultCount: Number(provider.resultCount ?? 0),
      status: String(provider.status ?? ''),
    })).sort((a, b) => a.provider.localeCompare(b.provider)),
    records: normalizedRecords,
    reportVersion: PIPELINE_REPORT_VERSION,
  };
  const canonical = canonicalJson(body);
  return {
    ...body,
    reportDigest: `sha256:${sha256(canonical)}`,
  };
}
