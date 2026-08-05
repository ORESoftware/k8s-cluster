import { createHash } from 'node:crypto';

export const DEFAULT_CONTINUATION_PROMPT =
  'Continue from exactly where the prior response stopped because of a rate or request limit. ' +
  'Read the full thread first, preserve all completed work, do not repeat finished sections, ' +
  'and complete only the unfinished work. Perform the requested work now rather than only ' +
  'describing a plan.';

const RATE_LIMIT_PATTERNS = [
  /^(?:chatgpt(?: said)?:?\s*)?(?:error(?:\s+\d+)?[:\s-]*)?too many requests(?:[.!]|$)/i,
  /^(?:error(?:\s+\d+)?[:\s-]*)?(?:rate|request|usage|message) limit (?:reached|exceeded)\b/i,
  /^you(?:'|’)ve reached\b.{0,160}\blimit\b/i,
  /^you have reached\b.{0,160}\blimit\b/i,
  /^(?:please )?try again in \d+\s*(?:seconds?|minutes?|hours?)\b/i,
  /^(?:something went wrong[.!:\s-]*)?please try again later\b/i,
  /^(?:error[\s:-]*)?429\b[\s\S]{0,160}\b(?:rate|request|limit)\b/i,
  /^(?:our systems are|we are) (?:currently )?(?:busy|overloaded)\b/i,
];

export function normalizeText(value) {
  return String(value ?? '')
    .replace(/\u00a0/g, ' ')
    .replace(/[\t\r ]+/g, ' ')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

export function isRateLimitErrorText(value) {
  const text = normalizeText(value);
  if (!text || text.length > 800 || text.split('\n').length > 12) return false;
  return RATE_LIMIT_PATTERNS.some((pattern) => pattern.test(text));
}

export function conversationIdentity(rawHref, baseUrl = 'https://chatgpt.com/') {
  if (!rawHref) return null;

  let url;
  try {
    url = new URL(rawHref, baseUrl);
  } catch {
    return null;
  }

  if (url.protocol !== 'https:' || url.hostname !== 'chatgpt.com') return null;
  const match = url.pathname.match(/(?:^|\/)c\/([A-Za-z0-9-]{16,})(?:\/|$)/);
  if (!match) return null;

  return {
    id: match[1],
    url: `${url.origin}${url.pathname}`,
  };
}

export function conversationKey(conversationId) {
  return createHash('sha256').update(String(conversationId)).digest('hex').slice(0, 20);
}

export function attemptEligibility(
  record,
  nowMs,
  { cooldownMs, maxAttempts, attemptWindowMs },
) {
  if (!record) return { eligible: true, attempts: 0, windowStartedAt: nowMs };

  const lastAttemptMs = Date.parse(record.lastAttemptAt ?? '');
  if (Number.isFinite(lastAttemptMs) && nowMs - lastAttemptMs < cooldownMs) {
    return {
      eligible: false,
      reason: 'cooldown',
      attempts: Number(record.attempts ?? 0),
      windowStartedAt: Date.parse(record.attemptWindowStartedAt ?? '') || nowMs,
    };
  }

  const windowStartedMs = Date.parse(record.attemptWindowStartedAt ?? '');
  if (!Number.isFinite(windowStartedMs) || nowMs - windowStartedMs >= attemptWindowMs) {
    return { eligible: true, attempts: 0, windowStartedAt: nowMs };
  }

  const attempts = Number(record.attempts ?? 0);
  if (attempts >= maxAttempts) {
    return {
      eligible: false,
      reason: 'attempt_cap',
      attempts,
      windowStartedAt: windowStartedMs,
    };
  }

  return { eligible: true, attempts, windowStartedAt: windowStartedMs };
}
