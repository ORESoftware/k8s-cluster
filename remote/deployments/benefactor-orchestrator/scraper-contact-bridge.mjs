import { normalizeEmail, normalizePhone } from './pipeline-lib.mjs';

const DEFAULT_SCRAPER_URL = 'http://dd-web-scraper.default.svc.cluster.local:8097';
const MAX_BRIDGE_RESPONSE_BYTES = 4 * 1024 * 1024;
const BRIDGE_INSTALLED = Symbol.for('benefactor.scraperContactBridge.installed');

function asUrl(input) {
  try {
    if (input instanceof URL) return input;
    if (typeof input === 'string') return new URL(input);
    if (input && typeof input.url === 'string') return new URL(input.url);
  } catch {
    return null;
  }
  return null;
}

function parseScraperOrigin(value = process.env.SCRAPER_URL || DEFAULT_SCRAPER_URL) {
  try {
    return new URL(value);
  } catch {
    return new URL(DEFAULT_SCRAPER_URL);
  }
}

function isScrapeRequest(input, scraperUrl) {
  const url = asUrl(input);
  if (!url) return false;
  return url.origin === scraperUrl.origin && url.pathname.replace(/\/+$/, '') === '/scrape';
}

function parseJsonBody(body) {
  if (typeof body === 'string') {
    try {
      const parsed = JSON.parse(body);
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : null;
    } catch {
      return null;
    }
  }
  if (body instanceof Uint8Array || Buffer.isBuffer(body)) {
    try {
      const parsed = JSON.parse(Buffer.from(body).toString('utf8'));
      return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : null;
    } catch {
      return null;
    }
  }
  return null;
}

function clampPositiveInteger(value, fallback, maximum) {
  const parsed = Number.parseInt(String(value ?? ''), 10);
  if (!Number.isSafeInteger(parsed) || parsed < 1) return fallback;
  return Math.min(parsed, maximum);
}

export function enrichScraperRequest(payload) {
  return {
    ...payload,
    includeContacts: true,
    includeEmails: true,
    includePhones: true,
    maxEmails: clampPositiveInteger(payload.maxEmails, 50, 50),
    maxPhones: clampPositiveInteger(payload.maxPhones, 50, 50),
  };
}

export function normalizeStructuredContacts(extraction, { requireRoleEmail = true } = {}) {
  const contacts = extraction && typeof extraction === 'object' ? extraction.contacts : null;
  const emails = new Set();
  const phones = new Set();

  for (const candidate of Array.isArray(contacts?.emails) ? contacts.emails : []) {
    const raw = typeof candidate === 'string' ? candidate : candidate?.address;
    const email = normalizeEmail(raw, { requireRoleEmail });
    if (email) emails.add(email);
  }

  for (const candidate of Array.isArray(contacts?.phones) ? contacts.phones : []) {
    const values = typeof candidate === 'string'
      ? [candidate]
      : [candidate?.e164, candidate?.raw, candidate?.national];
    for (const raw of values) {
      const phone = normalizePhone(raw);
      if (phone) {
        phones.add(phone);
        break;
      }
    }
  }

  return {
    emails: [...emails].sort(),
    phones: [...phones].sort(),
  };
}

function escapeHtmlAttribute(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('"', '&quot;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;');
}

export function augmentScraperBody(body, options = {}) {
  if (!body || typeof body !== 'object') return body;
  const extraction = body.extraction && typeof body.extraction === 'object'
    ? { ...body.extraction }
    : {};
  const normalized = normalizeStructuredContacts(extraction, options);
  if (!normalized.emails.length && !normalized.phones.length) {
    return { ...body, extraction };
  }

  const anchors = [
    ...normalized.emails.map((email) => `<a href="mailto:${escapeHtmlAttribute(email)}">contact</a>`),
    ...normalized.phones.map((phone) => `<a href="tel:${escapeHtmlAttribute(phone)}">phone</a>`),
  ].join('');
  const structuredMarkup = `<section data-benefactor-structured-contacts="true" hidden>${anchors}</section>`;
  const textSuffix = [
    ...normalized.emails,
    ...normalized.phones,
  ].join('\n');

  extraction.html = `${typeof extraction.html === 'string' ? extraction.html : ''}${structuredMarkup}`;
  extraction.text = [
    typeof extraction.text === 'string' ? extraction.text : '',
    textSuffix,
  ].filter(Boolean).join('\n');

  return { ...body, extraction };
}

function hasDocumentContent(body) {
  const extraction = body?.extraction;
  return Boolean(
    (typeof extraction?.html === 'string' && extraction.html.trim())
      || (typeof extraction?.text === 'string' && extraction.text.trim()),
  );
}

export function shouldEscalateToBrowser(payload, body, options = {}) {
  const strategy = String(payload?.strategy || '').toLowerCase();
  if (strategy === 'playwright' || strategy === 'puppeteer' || strategy === 'browserless') {
    return false;
  }
  if (!body || body.ok === false) return false;
  const contacts = normalizeStructuredContacts(body.extraction, options);
  return contacts.emails.length === 0 && contacts.phones.length === 0;
}

async function readJsonCloneCapped(response, maxBytes = MAX_BRIDGE_RESPONSE_BYTES) {
  const declared = Number.parseInt(response.headers.get('content-length') || '0', 10);
  if (Number.isFinite(declared) && declared > maxBytes) return null;
  const clone = response.clone();
  if (!clone.body) return null;
  const reader = clone.body.getReader();
  const chunks = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value) continue;
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel().catch(() => {});
        return null;
      }
      chunks.push(Buffer.from(value));
    }
  } finally {
    reader.releaseLock();
  }
  try {
    const parsed = JSON.parse(Buffer.concat(chunks, total).toString('utf8'));
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function jsonResponseLike(response, body) {
  const headers = new Headers(response.headers);
  headers.delete('content-length');
  headers.delete('content-encoding');
  headers.delete('transfer-encoding');
  headers.set('content-type', 'application/json; charset=utf-8');
  return new Response(JSON.stringify(body), {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

function browserPayload(payload) {
  return {
    ...payload,
    strategy: 'playwright',
    renderJavaScript: true,
    waitUntil: payload.waitUntil || 'domcontentloaded',
  };
}

export function createScraperContactFetch(originalFetch, {
  scraperUrl = parseScraperOrigin(),
  requireRoleEmail = String(process.env.REQUIRE_ROLE_EMAIL ?? 'true').toLowerCase() !== 'false',
} = {}) {
  if (typeof originalFetch !== 'function') throw new TypeError('originalFetch must be a function');

  return async function benefactorScraperContactFetch(input, init = {}) {
    if (!isScrapeRequest(input, scraperUrl)) return originalFetch(input, init);
    const payload = parseJsonBody(init?.body);
    if (!payload) return originalFetch(input, init);

    const enriched = enrichScraperRequest(payload);
    const requestInit = { ...init, body: JSON.stringify(enriched) };
    const staticResponse = await originalFetch(input, requestInit);
    if (!staticResponse.ok) return staticResponse;
    const staticBody = await readJsonCloneCapped(staticResponse);
    if (!staticBody) return staticResponse;

    let selectedResponse = staticResponse;
    let selectedBody = staticBody;
    if (shouldEscalateToBrowser(enriched, staticBody, { requireRoleEmail })) {
      try {
        const fallback = browserPayload(enriched);
        const browserResponse = await originalFetch(input, { ...init, body: JSON.stringify(fallback) });
        if (browserResponse.ok) {
          const browserBody = await readJsonCloneCapped(browserResponse);
          if (browserBody) {
            const browserContacts = normalizeStructuredContacts(
              browserBody.extraction,
              { requireRoleEmail },
            );
            if (
              hasDocumentContent(browserBody)
              || browserContacts.emails.length > 0
              || browserContacts.phones.length > 0
            ) {
              selectedResponse = browserResponse;
              selectedBody = browserBody;
            }
          }
        }
      } catch {
        // Keep the successful static response. The existing orchestrator remains
        // responsible for bounded warnings and run-level diagnostics.
      }
    }

    return jsonResponseLike(
      selectedResponse,
      augmentScraperBody(selectedBody, { requireRoleEmail }),
    );
  };
}

export function installScraperContactBridge(options = {}) {
  if (globalThis[BRIDGE_INSTALLED]) return false;
  globalThis.fetch = createScraperContactFetch(globalThis.fetch, options);
  globalThis[BRIDGE_INSTALLED] = true;
  return true;
}
