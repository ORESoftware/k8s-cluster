/**
 * Business contact extraction — phone numbers and email addresses.
 *
 * This module is intentionally parser-agnostic: it operates on the raw HTML
 * string plus the already-extracted visible text, so the same logic serves the
 * `native-fetch`, `cheerio`, `jsdom`, `linkedom`, and browser strategies. It is
 * a pure module (no I/O, no logging) so the worker can run it and the server can
 * unit-test it in isolation.
 *
 * PII policy: this is opt-in collection. Callers ask for contacts only when the
 * job needs them, and neither this module nor its callers log the extracted
 * values — counts only. See AGENTS.md ("Minimize and protect data").
 */

export type ContactSource = 'tel-href' | 'mailto-href' | 'structured-data' | 'meta' | 'text';

export type PhoneNumber = {
  /** The number as it was found on the page, before normalization. */
  raw: string;
  /** E.164 form (`+15551234567`) when a country code was present or derivable. */
  e164?: string;
  /** Human-readable national format when the region is known. */
  national?: string;
  /** Dial-through extension, when the page advertised one. */
  extension?: string;
  /** Every place this number was seen, most trustworthy first. */
  sources: ContactSource[];
  /** 0..1 heuristic confidence — 1.0 for `tel:`/structured data, lower for free text. */
  confidence: number;
};

export type EmailAddress = {
  /** Normalized (lower-cased, trimmed) address. */
  address: string;
  sources: ContactSource[];
  confidence: number;
};

export type ContactExtraction = {
  phones: PhoneNumber[];
  emails: EmailAddress[];
};

export type ContactExtractionOptions = {
  html: string;
  /** Visible text already extracted by the parser strategy. */
  text: string;
  /** ISO 3166-1 alpha-2 region used to normalize local numbers (e.g. `US`). */
  defaultRegion?: string;
  includePhones: boolean;
  includeEmails: boolean;
  maxPhones: number;
  maxEmails: number;
};

/** Country calling codes for regions we can confidently normalize to E.164. */
const REGION_CALLING_CODE: Record<string, string> = {
  US: '1',
  CA: '1',
  PR: '1',
  GB: '44',
  IE: '353',
  AU: '61',
  NZ: '64',
  DE: '49',
  FR: '33',
  ES: '34',
  IT: '39',
  NL: '31',
  BE: '32',
  CH: '41',
  AT: '43',
  SE: '46',
  NO: '47',
  DK: '45',
  FI: '358',
  PT: '351',
  PL: '48',
  IN: '91',
  SG: '65',
  HK: '852',
  JP: '81',
  MX: '52',
  BR: '55',
  ZA: '27',
  AE: '971',
};

const NANP_CALLING_CODE = '1';

const CONFIDENCE: Record<ContactSource, number> = {
  'tel-href': 1,
  'mailto-href': 1,
  'structured-data': 0.95,
  meta: 0.9,
  text: 0.6,
};

// Cap the HTML we scan so a pathological page can't blow up the regex passes.
const MAX_SCAN_CHARS = 4_000_000;
const MAX_JSONLD_BLOCKS = 40;

// Placeholder/asset domains that are never real business contacts.
const PLACEHOLDER_EMAIL_DOMAINS = new Set([
  'example.com',
  'example.org',
  'example.net',
  'domain.com',
  'yourdomain.com',
  'email.com',
  'sentry.io',
  'wixpress.com',
  'schema.org',
]);

// Image/asset extensions that show up as bogus "emails" like `logo@2x.png`.
const ASSET_TAIL = /\.(png|jpe?g|gif|webp|svg|css|js|woff2?|ttf|ico|mp4|pdf)$/i;

const EMAIL_RE = /[A-Z0-9._%+\-]+@[A-Z0-9.\-]+\.[A-Z]{2,24}/gi;

// A phone candidate: optional +, then digits/grouping punctuation, bounded so it
// isn't glued to a longer digit/word run (which would be an ID, not a number).
const PHONE_TEXT_RE = /(?<![\w+])(\+?\(?\d[\d().\- ‐-―\s]{5,20}\d)(?![\w])/g;
const PHONE_EXT_RE = /^[\s ]*(?:ext|extn|extension|x|#)\.?[\s ]*(\d{1,6})/i;

const TEL_HREF_RE = /(?:href|data-href)\s*=\s*["']\s*tel:([^"']+)["']/gi;
const MAILTO_HREF_RE = /(?:href|data-href)\s*=\s*["']\s*mailto:([^"'?]+)/gi;

export function extractContacts(options: ContactExtractionOptions): ContactExtraction {
  const html = options.html.length > MAX_SCAN_CHARS ? options.html.slice(0, MAX_SCAN_CHARS) : options.html;
  const text = options.text.length > MAX_SCAN_CHARS ? options.text.slice(0, MAX_SCAN_CHARS) : options.text;

  const phones = new PhoneCollector(options.defaultRegion);
  const emails = new EmailCollector();

  if (options.includePhones) {
    collectTelHrefs(html, phones);
  }
  if (options.includeEmails) {
    collectMailtoHrefs(html, emails);
  }
  if (options.includePhones || options.includeEmails) {
    collectStructuredData(html, options, phones, emails);
    collectMeta(html, options, phones, emails);
  }
  if (options.includePhones) {
    collectPhonesFromText(text, phones);
  }
  if (options.includeEmails) {
    collectEmailsFromText(text, emails);
  }

  return {
    phones: options.includePhones ? phones.finalize(options.maxPhones) : [],
    emails: options.includeEmails ? emails.finalize(options.maxEmails) : [],
  };
}

function collectTelHrefs(html: string, phones: PhoneCollector): void {
  for (const match of html.matchAll(TEL_HREF_RE)) {
    const value = decodeEntities(match[1] ?? '').trim();
    if (value) {
      phones.add(value, 'tel-href');
    }
  }
}

function collectMailtoHrefs(html: string, emails: EmailCollector): void {
  for (const match of html.matchAll(MAILTO_HREF_RE)) {
    const value = decodeEntities(match[1]).trim();
    emails.add(value, 'mailto-href');
  }
}

function collectPhonesFromText(text: string, phones: PhoneCollector): void {
  for (const match of text.matchAll(PHONE_TEXT_RE)) {
    const candidate = match[1];
    const tail = text.slice(match.index + match[0].length);
    const extMatch = tail.match(PHONE_EXT_RE);
    phones.add(candidate, 'text', extMatch?.[1]);
  }
}

function collectEmailsFromText(text: string, emails: EmailCollector): void {
  for (const match of text.matchAll(EMAIL_RE)) {
    emails.add(match[0], 'text');
  }
}

function collectMeta(
  html: string,
  options: ContactExtractionOptions,
  phones: PhoneCollector,
  emails: EmailCollector,
): void {
  const metaRe = /<meta\b[^>]*>/gi;
  for (const tag of html.matchAll(metaRe)) {
    const el = tag[0];
    const key = (attr(el, 'property') ?? attr(el, 'itemprop') ?? attr(el, 'name') ?? '').toLowerCase();
    const content = attr(el, 'content');
    if (!content) {
      continue;
    }
    const value = decodeEntities(content).trim();
    if (options.includePhones && /(?:phone|telephone|tel|contactpoint)/.test(key)) {
      phones.add(value, 'meta');
    }
    if (options.includeEmails && /email/.test(key)) {
      emails.add(value, 'meta');
    }
  }
}

function collectStructuredData(
  html: string,
  options: ContactExtractionOptions,
  phones: PhoneCollector,
  emails: EmailCollector,
): void {
  const blockRe = /<script\b[^>]*type\s*=\s*["']application\/ld\+json["'][^>]*>([\s\S]*?)<\/script>/gi;
  let blocks = 0;
  for (const match of html.matchAll(blockRe)) {
    if (blocks++ >= MAX_JSONLD_BLOCKS) {
      break;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(decodeEntities(match[1].trim()));
    } catch {
      continue;
    }
    walkJsonLd(parsed, options, phones, emails, 0);
  }
}

function walkJsonLd(
  node: unknown,
  options: ContactExtractionOptions,
  phones: PhoneCollector,
  emails: EmailCollector,
  depth: number,
): void {
  if (depth > 8 || node === null) {
    return;
  }
  if (Array.isArray(node)) {
    for (const item of node) {
      walkJsonLd(item, options, phones, emails, depth + 1);
    }
    return;
  }
  if (typeof node !== 'object') {
    return;
  }
  for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
    const lowered = key.toLowerCase();
    if (options.includePhones && (lowered === 'telephone' || lowered === 'phone' || lowered === 'faxnumber')) {
      addStringOrArray(value, (v) => phones.add(v, 'structured-data'));
    } else if (options.includeEmails && lowered === 'email') {
      addStringOrArray(value, (v) => emails.add(v, 'structured-data'));
    } else if (typeof value === 'object' && value !== null) {
      walkJsonLd(value, options, phones, emails, depth + 1);
    }
  }
}

function addStringOrArray(value: unknown, add: (value: string) => void): void {
  if (typeof value === 'string') {
    add(value.replace(/^mailto:/i, '').replace(/^tel:/i, '').trim());
  } else if (Array.isArray(value)) {
    for (const item of value) {
      if (typeof item === 'string') {
        add(item.replace(/^mailto:/i, '').replace(/^tel:/i, '').trim());
      }
    }
  }
}

class PhoneCollector {
  private readonly byKey = new Map<string, PhoneNumber>();

  constructor(private readonly defaultRegion?: string) {}

  add(raw: string, source: ContactSource, extension?: string): void {
    const normalized = normalizePhoneNumber(raw, this.defaultRegion);
    if (!normalized) {
      return;
    }
    const ext = extension ?? normalized.extension;
    const key = normalized.e164 ?? `nat:${normalized.digits}`;
    const existing = this.byKey.get(key);
    if (existing) {
      if (!existing.sources.includes(source)) {
        existing.sources.push(source);
      }
      existing.confidence = Math.max(existing.confidence, CONFIDENCE[source]);
      if (!existing.extension && ext) {
        existing.extension = ext;
      }
      return;
    }
    this.byKey.set(key, {
      raw: normalized.raw,
      ...(normalized.e164 ? { e164: normalized.e164 } : {}),
      ...(normalized.national ? { national: normalized.national } : {}),
      ...(ext ? { extension: ext } : {}),
      sources: [source],
      confidence: CONFIDENCE[source],
    });
  }

  finalize(max: number): PhoneNumber[] {
    return [...this.byKey.values()]
      .sort((a, b) => b.confidence - a.confidence || rankSource(a) - rankSource(b))
      .slice(0, max)
      .map((phone) => ({ ...phone, sources: sortSources(phone.sources) }));
  }
}

class EmailCollector {
  private readonly byAddress = new Map<string, EmailAddress>();

  add(raw: string, source: ContactSource): void {
    const address = normalizeEmail(raw);
    if (!address) {
      return;
    }
    const existing = this.byAddress.get(address);
    if (existing) {
      if (!existing.sources.includes(source)) {
        existing.sources.push(source);
      }
      existing.confidence = Math.max(existing.confidence, CONFIDENCE[source]);
      return;
    }
    this.byAddress.set(address, { address, sources: [source], confidence: CONFIDENCE[source] });
  }

  finalize(max: number): EmailAddress[] {
    return [...this.byAddress.values()]
      .sort((a, b) => b.confidence - a.confidence || a.address.localeCompare(b.address))
      .slice(0, max)
      .map((email) => ({ ...email, sources: sortSources(email.sources) }));
  }
}

type NormalizedPhone = {
  raw: string;
  digits: string;
  e164?: string;
  national?: string;
  extension?: string;
};

/**
 * Normalize a raw phone string to E.164 when a country code is present or can be
 * inferred from `defaultRegion`. Returns `undefined` for values that don't look
 * like a dialable business number (too short/long, placeholder runs, IDs).
 */
export function normalizePhoneNumber(raw: string, defaultRegion?: string): NormalizedPhone | undefined {
  const trimmed = raw.trim();
  // Split off a trailing extension the source embedded (e.g. "...x123").
  let extension: string | undefined;
  const extMatch = trimmed.match(/[\s,;]*(?:ext|extn|extension|x|#|,)\.?[\s]*(\d{1,6})\s*$/i);
  let core = trimmed;
  if (extMatch) {
    extension = extMatch[1];
    core = trimmed.slice(0, extMatch.index).trim();
  }

  const hasPlus = /^\s*\+/.test(core);
  const digits = core.replace(/\D/g, '');
  const internationalPrefix = !hasPlus && digits.startsWith('00');
  const bareDigits = internationalPrefix ? digits.slice(2) : digits;

  if (!isPlausiblePhone(bareDigits, hasPlus || internationalPrefix)) {
    return undefined;
  }

  if (hasPlus || internationalPrefix) {
    const e164 = `+${bareDigits}`;
    return {
      raw: trimmed,
      digits: bareDigits,
      e164,
      national: formatNational(e164),
      extension,
    };
  }

  const region = (defaultRegion ?? '').toUpperCase();
  const callingCode = REGION_CALLING_CODE[region];

  if (callingCode === NANP_CALLING_CODE) {
    if (digits.length === 10) {
      const e164 = `+1${digits}`;
      return { raw: trimmed, digits, e164, national: formatNational(e164), extension };
    }
    if (digits.length === 11 && digits.startsWith('1')) {
      const e164 = `+${digits}`;
      return { raw: trimmed, digits: digits.slice(1), e164, national: formatNational(e164), extension };
    }
  } else if (callingCode) {
    const national = digits.replace(/^0/, '');
    const e164 = `+${callingCode}${national}`;
    if (e164.length >= 9 && e164.length <= 16) {
      return { raw: trimmed, digits: national, e164, national: undefined, extension };
    }
  }

  // No usable region — keep the national digits so a downstream reviewer can
  // still see the number, but don't fabricate a country code.
  return { raw: trimmed, digits, national: groupDigits(digits), extension };
}

function isPlausiblePhone(digits: string, international: boolean): boolean {
  const len = digits.length;
  if (international) {
    if (len < 8 || len > 15) {
      return false;
    }
  } else if (len < 10 || len > 15) {
    // Free-text numbers without a country code need an area code to be trustworthy;
    // 7-digit locals are too easily confused with IDs and are dropped on purpose.
    return false;
  }
  if (/^(\d)\1+$/.test(digits)) {
    return false; // 5555555555 and friends
  }
  if (digits === '1234567890' || digits === '0123456789' || digits === '11234567890') {
    return false;
  }
  return true;
}

function formatNational(e164: string): string | undefined {
  const nanp = e164.match(/^\+1(\d{3})(\d{3})(\d{4})$/);
  if (nanp) {
    return `(${nanp[1]}) ${nanp[2]}-${nanp[3]}`;
  }
  return undefined;
}

function groupDigits(digits: string): string {
  if (digits.length <= 4) {
    return digits;
  }
  // Group from the right in blocks of 3-4 for readability of unknown formats.
  const head = digits.slice(0, digits.length % 3 === 0 ? 3 : digits.length % 3 || 3);
  const rest = digits.slice(head.length).replace(/(\d{3})(?=\d)/g, '$1 ');
  return `${head} ${rest}`.trim();
}

export function normalizeEmail(raw: string): string | undefined {
  const address = decodeEntities(raw).trim().replace(/^mailto:/i, '').replace(/[.,;:>)\]]+$/, '').toLowerCase();
  if (address.length < 6 || address.length > 254) {
    return undefined;
  }
  const at = address.lastIndexOf('@');
  if (at <= 0 || at === address.length - 1) {
    return undefined;
  }
  const local = address.slice(0, at);
  const domain = address.slice(at + 1);
  if (!/^[a-z0-9.\-]+\.[a-z]{2,24}$/.test(domain) || domain.includes('..')) {
    return undefined;
  }
  if (!/[a-z0-9]/.test(local)) {
    return undefined;
  }
  if (PLACEHOLDER_EMAIL_DOMAINS.has(domain) || ASSET_TAIL.test(address)) {
    return undefined;
  }
  return address;
}

function attr(tag: string, name: string): string | undefined {
  const match = tag.match(new RegExp(`\\b${name}\\s*=\\s*["']([^"']*)["']`, 'i'));
  return match?.[1];
}

function decodeEntities(value: string): string {
  return value
    .replace(/&amp;/gi, '&')
    .replace(/&#0*64;/g, '@')
    .replace(/&#x0*40;/gi, '@')
    .replace(/&lt;/gi, '<')
    .replace(/&gt;/gi, '>')
    .replace(/&quot;/gi, '"')
    .replace(/&#0*32;/g, ' ')
    .replace(/&nbsp;/gi, ' ');
}

function rankSource(phone: PhoneNumber): number {
  return Math.min(...phone.sources.map((s) => SOURCE_ORDER.indexOf(s)));
}

const SOURCE_ORDER: ContactSource[] = ['tel-href', 'mailto-href', 'structured-data', 'meta', 'text'];

function sortSources(sources: ContactSource[]): ContactSource[] {
  return [...sources].sort((a, b) => SOURCE_ORDER.indexOf(a) - SOURCE_ORDER.indexOf(b));
}
