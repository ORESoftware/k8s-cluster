import crypto from 'node:crypto';

const BATCH_ID_RE = /^benefactor-[0-9]{8}T[0-9]{6}Z-[a-f0-9]{8}$/;

export function parseBoolean(value, fallback = false) {
  if (value == null || String(value).trim() === '') return fallback;
  const normalized = String(value).trim().toLowerCase();
  if (['1', 'true', 'yes', 'on'].includes(normalized)) return true;
  if (['0', 'false', 'no', 'off'].includes(normalized)) return false;
  throw new Error(`invalid boolean value: ${value}`);
}

export function parseBoundedInteger(name, value, { defaultValue, min, max }) {
  const raw = value == null || String(value).trim() === '' ? String(defaultValue) : String(value).trim();
  if (!/^-?\d+$/.test(raw)) throw new Error(`${name} must be an integer`);
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${name} must be between ${min} and ${max}`);
  }
  return parsed;
}

export function makeBatchId(now = new Date(), randomBytes = crypto.randomBytes) {
  const stamp = now.toISOString().replaceAll('-', '').replaceAll(':', '').replace('.000', '');
  return `benefactor-${stamp}-${randomBytes(4).toString('hex')}`;
}

export function validateBatchId(value) {
  const normalized = String(value || '').trim();
  if (!BATCH_ID_RE.test(normalized)) {
    throw new Error('CONTACT_BATCH_ID has an unsupported format');
  }
  return normalized;
}

export function normalizeCategoryRows(rows, allowlist = []) {
  const allowed = new Set(allowlist.map((item) => String(item).trim().toLowerCase()).filter(Boolean));
  const seen = new Set();
  return rows
    .map((row) => ({
      category: String(row.service_category || '').trim().toLowerCase(),
      queryCount: Number(row.query_count || 0),
      priority: Number(row.max_priority || 0),
    }))
    .filter((row) => row.category)
    .filter((row) => allowed.size === 0 || allowed.has(row.category))
    .filter((row) => {
      if (seen.has(row.category)) return false;
      seen.add(row.category);
      return true;
    })
    .sort((a, b) => b.priority - a.priority || b.queryCount - a.queryCount || a.category.localeCompare(b.category));
}

export function planCategoryTargets(categories, targetContacts, maxPerCategory, { includeOverflow = false } = {}) {
  if (!Array.isArray(categories) || categories.length === 0) return [];
  if (!Number.isInteger(targetContacts) || targetContacts < 1) throw new Error('targetContacts must be positive');
  if (!Number.isInteger(maxPerCategory) || maxPerCategory < 1) throw new Error('maxPerCategory must be positive');

  const plans = [];
  let remaining = targetContacts;
  let pass = 0;
  while (remaining > 0) {
    let allocatedThisPass = 0;
    for (const category of categories) {
      if (remaining <= 0) break;
      const amount = Math.min(maxPerCategory, remaining);
      plans.push({
        category: category.category,
        target: amount,
        pass,
        queryCount: category.queryCount,
        priority: category.priority,
      });
      remaining -= amount;
      allocatedThisPass += amount;
    }
    if (allocatedThisPass === 0) break;
    pass += 1;
  }

  if (includeOverflow) {
    const scheduled = new Set(plans.map((item) => item.category));
    for (const category of categories) {
      if (scheduled.has(category.category)) continue;
      plans.push({
        category: category.category,
        target: maxPerCategory,
        pass,
        queryCount: category.queryCount,
        priority: category.priority,
        overflow: true,
      });
    }
  }
  return plans;
}

export function hashIdentifier(value) {
  return crypto.createHash('sha256').update(String(value || '')).digest('hex');
}

export function buildBatchReport({
  batchId,
  dryRun,
  targetContacts,
  minimumContacts,
  maximumContacts,
  categoriesPlanned,
  categoriesRun,
  contactsTagged,
  hubspot,
  approvedForOutreach,
  status,
}) {
  return {
    schemaVersion: 1,
    batchId,
    batchDigest: hashIdentifier(batchId),
    dryRun: Boolean(dryRun),
    targetContacts,
    minimumContacts,
    maximumContacts,
    categoriesPlanned,
    categoriesRun,
    contactsTagged,
    hubspot: hubspot || { attempted: false },
    approvedForOutreach: Number(approvedForOutreach || 0),
    outreachDispatchRequested: false,
    status,
  };
}
