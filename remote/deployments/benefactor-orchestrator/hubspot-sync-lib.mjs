import crypto from 'node:crypto';

const GENERIC_EMAIL_DOMAINS = new Set([
  'aol.com',
  'gmail.com',
  'googlemail.com',
  'hotmail.com',
  'icloud.com',
  'live.com',
  'me.com',
  'msn.com',
  'outlook.com',
  'proton.me',
  'protonmail.com',
  'yahoo.com',
]);

const ROLE_LOCAL_PARTS = new Set([
  'admin',
  'billing',
  'business',
  'contact',
  'customerservice',
  'hello',
  'info',
  'office',
  'operations',
  'sales',
  'service',
  'support',
  'team',
]);

export function normalizeEmail(value) {
  const email = String(value || '').trim().toLowerCase();
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) return '';
  return email;
}

export function normalizePhone(value) {
  const raw = String(value || '').trim();
  if (!raw) return '';
  const cleaned = raw.replaceAll(/[^\d+]/g, '');
  if (!/^\+[1-9]\d{7,14}$/.test(cleaned)) return '';
  return cleaned;
}

export function emailDomain(email) {
  const normalized = normalizeEmail(email);
  return normalized ? normalized.slice(normalized.lastIndexOf('@') + 1) : '';
}

export function isGenericEmailDomain(domain) {
  return GENERIC_EMAIL_DOMAINS.has(String(domain || '').trim().toLowerCase());
}

export function isRoleEmail(email) {
  const normalized = normalizeEmail(email);
  if (!normalized) return false;
  const local = normalized.slice(0, normalized.indexOf('@')).replaceAll(/[._+-]/g, '');
  return ROLE_LOCAL_PARTS.has(local) || /^(?:info|sales|support|contact|hello|office|service|team)\d*$/.test(local);
}

export function domainFromLead(lead) {
  const candidates = [lead.website_url, lead.source_url];
  for (const candidate of candidates) {
    try {
      if (!candidate) continue;
      const url = new URL(candidate);
      const host = url.hostname.toLowerCase().replace(/^www\./, '');
      if (host && host.includes('.') && !isGenericEmailDomain(host)) return host;
    } catch {
      // Try the next source.
    }
  }
  const domain = emailDomain(lead.email || lead.primary_email);
  return domain && !isGenericEmailDomain(domain) ? domain : '';
}

export function websiteFromLead(lead) {
  for (const candidate of [lead.website_url, lead.source_url]) {
    try {
      if (!candidate) continue;
      const url = new URL(candidate);
      if (!['http:', 'https:'].includes(url.protocol)) continue;
      return `${url.protocol}//${url.host}`;
    } catch {
      // Try the next source.
    }
  }
  const domain = domainFromLead(lead);
  return domain ? `https://${domain}` : '';
}

export function firstPhone(metaData) {
  const phones = Array.isArray(metaData?.phones) ? metaData.phones : [];
  for (const phone of phones) {
    const normalized = normalizePhone(phone);
    if (normalized) return normalized;
  }
  return '';
}

function add(properties, key, value) {
  const normalized = String(value || '').trim();
  if (normalized) properties[key] = normalized;
}

export function buildCompanyProperties(lead) {
  const properties = {};
  add(properties, 'name', lead.business_name);
  add(properties, 'domain', domainFromLead(lead));
  add(properties, 'website', websiteFromLead(lead));
  add(properties, 'city', lead.city);
  add(properties, 'state', lead.state);
  return properties;
}

export function buildContactProperties(lead) {
  const email = normalizeEmail(lead.email || lead.primary_email);
  if (!email) throw new Error('lead email is invalid');
  const properties = { email };
  add(properties, 'firstname', lead.first_name || lead.owner_first_name);
  add(properties, 'lastname', lead.last_name || lead.owner_last_name);
  add(properties, 'phone', firstPhone(lead.meta_data));
  add(properties, 'company', lead.business_name);
  add(properties, 'website', websiteFromLead(lead));
  add(properties, 'city', lead.city);
  add(properties, 'state', lead.state);
  return properties;
}

export function buildExactSearchBody(propertyName, value) {
  return {
    filterGroups: [
      {
        filters: [
          {
            propertyName,
            operator: 'EQ',
            value: String(value),
          },
        ],
      },
    ],
    limit: 1,
  };
}

export function assertLiveSyncConfig(config) {
  if (config.dryRun) return;
  const missing = [];
  if (!config.accessToken) missing.push('HUBSPOT_ACCESS_TOKEN');
  if (!config.batchId) missing.push('CONTACT_BATCH_ID');
  if (config.writeConfirmation !== 'sync-benefactor-contact-batch') {
    missing.push('HUBSPOT_WRITE_CONFIRM=sync-benefactor-contact-batch');
  }
  if (missing.length) throw new Error(`live HubSpot sync requires ${missing.join(', ')}`);
}

export function safeErrorCode(error) {
  const status = Number(error?.status || error?.statusCode || 0);
  if (status) return `hubspot_http_${status}`;
  const code = String(error?.code || error?.name || 'error').toLowerCase().replaceAll(/[^a-z0-9_-]/g, '_');
  return code.slice(0, 80) || 'error';
}

export function hashIdentifier(value) {
  return crypto.createHash('sha256').update(String(value || '')).digest('hex');
}

export function buildSyncReport({ batchId, dryRun, candidates, synced, skipped, failed, companies, contacts }) {
  return {
    schemaVersion: 1,
    batchDigest: batchId ? hashIdentifier(batchId) : null,
    dryRun: Boolean(dryRun),
    candidates,
    synced,
    skipped,
    failed,
    companies,
    contacts,
    marketingConsentMutated: false,
    outreachDispatched: false,
  };
}
