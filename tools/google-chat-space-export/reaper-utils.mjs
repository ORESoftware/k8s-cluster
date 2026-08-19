import { createHash } from 'node:crypto';

export const CANDIDATE_KEY_PATTERN = /^google-chat:[A-Za-z0-9_-]+:[0-9a-f]{24}$/;
export const LINEAR_IDENTIFIER_PATTERN = /^[A-Z][A-Z0-9]+-[1-9][0-9]*$/;
export const PR_REFERENCE_PATTERN = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+#[1-9][0-9]*$/;
export const COMMIT_REFERENCE_PATTERN = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+@[0-9a-f]{40}$/;
export const GITHUB_OWNER_PATTERN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
export const GITHUB_REPOSITORY_PATTERN = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

const SECRET_PATTERNS = [
  /\bgh[pousr]_[A-Za-z0-9_]{20,}\b/g,
  /\blin_api_[A-Za-z0-9]{20,}\b/g,
  /\bSG\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{20,}\b/g,
  /\b(?:cfat|sk|xox[baprs])[-_.][A-Za-z0-9_-]{16,}\b/gi,
  /\bAKIA[0-9A-Z]{16}\b/g,
  /\bBearer\s+[A-Za-z0-9._~-]{20,}\b/gi,
  /\b[A-Za-z0-9_-]{24,}\.[A-Za-z0-9_-]{24,}\.[A-Za-z0-9_-]{24,}\b/g,
];
const EMAIL_PATTERN = /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi;
const PHONE_PATTERN = /(?<![A-Za-z0-9])\+?[0-9][0-9().\s-]{7,}[0-9](?![A-Za-z0-9])/g;

export function assertPlainObject(value, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
}

export function sha256(value) {
  return createHash('sha256').update(String(value)).digest('hex');
}

function stableValue(value) {
  if (Array.isArray(value)) return value.map(stableValue);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.keys(value).sort().map((key) => [key, stableValue(value[key])]),
    );
  }
  return value;
}

export function stableStringify(value) {
  return JSON.stringify(stableValue(value));
}

export function uniqueSorted(values) {
  return [...new Set(values.filter(Boolean).map(String))].sort();
}

export function integerFromEnv(value, fallback, label, minimum = 1, maximum = 1000) {
  if (value === undefined || value === null || value === '') return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new Error(`${label} must be an integer between ${minimum} and ${maximum}`);
  }
  return parsed;
}

export function canonicalInstant(value, label) {
  if (typeof value !== 'string') throw new Error(`${label} must be a canonical RFC-3339 instant`);
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed) || new Date(parsed).toISOString() !== value) {
    throw new Error(`${label} must be a canonical RFC-3339 instant`);
  }
  return value;
}

export function redactSensitiveText(value) {
  let output = String(value ?? '');
  for (const pattern of SECRET_PATTERNS) output = output.replace(pattern, '[REDACTED_SECRET]');
  output = output.replace(EMAIL_PATTERN, '[REDACTED_EMAIL]');
  output = output.replace(PHONE_PATTERN, '[REDACTED_PHONE]');
  return output;
}

export function safeIssueTitle(value, prefix = '') {
  const redacted = redactSensitiveText(value)
    .replace(/[\r\n\t]+/g, ' ')
    .replace(/\s{2,}/g, ' ')
    .trim();
  const fallback = 'Review a sanitized Google Chat work item';
  const base = redacted.length >= 5 ? redacted : fallback;
  const combined = `${prefix}${base}`.trim();
  return combined.length <= 120 ? combined : `${combined.slice(0, 119).trim()}…`;
}

export function sourceKeyDigest(sourceKey) {
  return `sha256:${sha256(sourceKey)}`;
}

export function candidateMarker(candidateKey) {
  return `<!-- google-chat-reaper:${candidateKey} -->`;
}
