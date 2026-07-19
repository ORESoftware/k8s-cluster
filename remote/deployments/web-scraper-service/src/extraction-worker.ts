import { parentPort } from 'node:worker_threads';

import * as cheerio from 'cheerio';

import { extractContacts, type ContactExtraction } from './contacts.js';

type ParserName = 'native-fetch' | 'cheerio' | 'jsdom' | 'linkedom';

type ExtractionInput = {
  parser: ParserName;
  html: string;
  baseUrl: string;
  selector?: string;
  selectors?: Record<string, string>;
  includeHtml?: boolean;
  includeText?: boolean;
  includeLinks?: boolean;
  includePhones?: boolean;
  includeEmails?: boolean;
  contactRegion?: string;
  maxHtmlChars: number;
  maxTextChars: number;
  maxLinks: number;
  maxPhones: number;
  maxEmails: number;
};

type ExtractionResult = {
  parser: ParserName;
  title?: string;
  text?: string;
  html?: string;
  selection?: {
    selector: string;
    count: number;
    text?: string;
    html?: string;
  };
  fields?: Record<string, string>;
  links?: string[];
  contacts?: ContactExtraction;
};

type WorkerResponse =
  | { ok: true; extraction: ExtractionResult }
  | { ok: false; error: string };

parentPort?.on('message', (input: ExtractionInput) => {
  extractDocument(input)
    .then((extraction) => post({ ok: true, extraction }))
    .catch((error) => {
      post({
        ok: false,
        error: error instanceof Error ? error.message : String(error),
      });
    });
});

async function extractDocument(input: ExtractionInput): Promise<ExtractionResult> {
  const result = await runParser(input);
  applyContactExtraction(result, input);
  return result;
}

function runParser(input: ExtractionInput): Promise<ExtractionResult> | ExtractionResult {
  switch (input.parser) {
    case 'native-fetch':
      return extractNative(input);
    case 'jsdom':
      return extractWithJsdom(input);
    case 'linkedom':
      return extractWithLinkedom(input);
    case 'cheerio':
      return extractWithCheerio(input);
    default:
      return assertNever(input.parser);
  }
}

/**
 * Contact scanning is opt-in and runs against the full (untruncated) HTML plus
 * the parser's visible text. `result.text` is clamped for the response payload,
 * so when the caller suppressed or truncated text we re-derive a full-page text
 * for scanning — otherwise a footer phone number past the clamp would be missed.
 */
function applyContactExtraction(result: ExtractionResult, input: ExtractionInput): void {
  if (!input.includePhones && !input.includeEmails) {
    return;
  }
  const textIsComplete =
    typeof result.text === 'string' && result.text.length < input.maxTextChars;
  result.contacts = extractContacts({
    html: input.html,
    text: textIsComplete ? (result.text as string) : stripHtml(input.html),
    defaultRegion: input.contactRegion,
    includePhones: input.includePhones === true,
    includeEmails: input.includeEmails === true,
    maxPhones: input.maxPhones,
    maxEmails: input.maxEmails,
  });
}

function extractNative(input: ExtractionInput): ExtractionResult {
  const result: ExtractionResult = {
    parser: 'native-fetch',
    title: firstMatch(input.html, /<title[^>]*>([\s\S]*?)<\/title>/i),
  };
  if (input.includeText !== false) {
    result.text = clampText(stripHtml(input.html), input.maxTextChars);
  }
  if (input.includeHtml) {
    result.html = trimToMax(input.html, input.maxHtmlChars);
  }
  return result;
}

function extractWithCheerio(input: ExtractionInput): ExtractionResult {
  const $ = cheerio.load(input.html);
  const result: ExtractionResult = {
    parser: 'cheerio',
    title: normalizeText($('title').first().text()),
  };
  if (input.includeText !== false) {
    result.text = clampText(normalizeText($('body').text() || $.root().text()), input.maxTextChars);
  }
  if (input.includeHtml) {
    result.html = trimToMax(input.html, input.maxHtmlChars);
  }
  if (input.selector) {
    const selected = $(input.selector);
    result.selection = {
      selector: input.selector,
      count: selected.length,
      text: clampText(normalizeText(selected.first().text()), input.maxTextChars),
      html: input.includeHtml ? selected.first().html() ?? undefined : undefined,
    };
  }
  if (input.selectors) {
    result.fields = Object.fromEntries(
      Object.entries(input.selectors).map(([name, selector]) => [
        name,
        clampText(normalizeText($(selector).first().text()), input.maxTextChars),
      ]),
    );
  }
  if (input.includeLinks) {
    result.links = $('a[href]')
      .map((_index, element) => resolveHref($(element).attr('href'), input.baseUrl))
      .get()
      .filter((href): href is string => Boolean(href))
      .slice(0, input.maxLinks);
  }
  return result;
}

async function extractWithJsdom(input: ExtractionInput): Promise<ExtractionResult> {
  const { JSDOM } = await import('jsdom');
  const dom = new JSDOM(input.html, { url: input.baseUrl });
  const { document } = dom.window;
  const result: ExtractionResult = {
    parser: 'jsdom',
    title: normalizeText(document.querySelector('title')?.textContent ?? ''),
  };
  applyDomExtraction(result, document, input);
  dom.window.close();
  return result;
}

async function extractWithLinkedom(input: ExtractionInput): Promise<ExtractionResult> {
  const { parseHTML } = await import('linkedom');
  const { document } = parseHTML(input.html);
  const result: ExtractionResult = {
    parser: 'linkedom',
    title: normalizeText(document.querySelector('title')?.textContent ?? ''),
  };
  applyDomExtraction(result, document, input);
  return result;
}

function applyDomExtraction(
  result: ExtractionResult,
  document: Document,
  input: ExtractionInput,
): void {
  if (input.includeText !== false) {
    result.text = clampText(
      normalizeText(document.body?.textContent ?? document.textContent ?? ''),
      input.maxTextChars,
    );
  }
  if (input.includeHtml) {
    result.html = trimToMax(input.html, input.maxHtmlChars);
  }
  if (input.selector) {
    const selected = Array.from(document.querySelectorAll(input.selector));
    const first = selected[0];
    result.selection = {
      selector: input.selector,
      count: selected.length,
      text: first ? clampText(normalizeText(first.textContent ?? ''), input.maxTextChars) : '',
      html: input.includeHtml && first ? first.innerHTML : undefined,
    };
  }
  if (input.selectors) {
    result.fields = Object.fromEntries(
      Object.entries(input.selectors).map(([name, selector]) => [
        name,
        clampText(normalizeText(document.querySelector(selector)?.textContent ?? ''), input.maxTextChars),
      ]),
    );
  }
  if (input.includeLinks) {
    result.links = Array.from(document.querySelectorAll('a[href]'))
      .map((element) => resolveHref(element.getAttribute('href') ?? undefined, input.baseUrl))
      .filter((href): href is string => Boolean(href))
      .slice(0, input.maxLinks);
  }
}

function post(message: WorkerResponse): void {
  parentPort?.postMessage(message);
}

function normalizeText(value: string): string {
  return value.replace(/\s+/g, ' ').trim();
}

function stripHtml(value: string): string {
  return normalizeText(value.replace(/<script[\s\S]*?<\/script>/gi, ' ').replace(/<style[\s\S]*?<\/style>/gi, ' ').replace(/<[^>]+>/g, ' '));
}

function clampText(value: string, maxChars: number): string {
  return value.length > maxChars ? `${value.slice(0, maxChars)}...` : value;
}

function trimToMax(value: string, maxChars: number): string {
  return value.length > maxChars ? value.slice(0, maxChars) : value;
}

function firstMatch(value: string, pattern: RegExp): string | undefined {
  const match = value.match(pattern);
  return match?.[1] ? normalizeText(stripHtml(match[1])) : undefined;
}

function resolveHref(value: string | undefined, baseUrl: string): string | undefined {
  if (!value) {
    return undefined;
  }
  try {
    return new URL(value, baseUrl).toString();
  } catch {
    return value;
  }
}

function assertNever(value: never): never {
  throw new Error(`unsupported parser: ${String(value)}`);
}
