// Persistent-session declarative browser-automation worker.
//
// This module adds a stateful browser-command surface on top of the otherwise
// stateless scraper: an isolated Playwright BrowserContext + Page is held open
// across multiple HTTP calls so an external MCP gateway (dd-browser-mcp-rs) can
// drive a multi-step workflow (navigate -> observe -> fill -> observe -> ...).
//
// Trust model: these routes are internal. They are only reachable from the
// dd-browser-mcp-rs pod (NetworkPolicy) and require the shared SERVER_AUTH_SECRET
// (X-Server-Auth). The MCP gateway is the public trust boundary and does its own
// schema validation; this worker re-validates everything with zod regardless.
//
// The caller NEVER supplies JavaScript, XPath, or CSS-into-evaluate. The only
// script that runs in the page is the fixed EXTRACT_FN below. Targets are
// resolved by semantic strategies (role/name/label/placeholder/text/testid) with
// an optional CSS fallback, and elements are tagged with a `data-dd-ref`
// attribute so a stable short ref (e1, e2, ...) survives minor DOM mutations.

import { randomUUID, createHash, timingSafeEqual } from 'node:crypto';
import { lookup as dnsLookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import { readFile, realpath, stat } from 'node:fs/promises';
import { isAbsolute, relative, resolve } from 'node:path';
import { z } from 'zod';
import {
  SensitiveFieldPolicyError,
  assertSensitiveFieldWriteAllowed,
} from './sensitive-field-policy.js';
import type {
  Browser as PlaywrightBrowser,
  BrowserContext,
  Page,
  Frame,
  Dialog,
  Locator,
} from 'playwright';
import type { FastifyInstance, FastifyBaseLogger } from 'fastify';

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

function envNum(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;
  const value = Number(raw);
  return Number.isFinite(value) && value > 0 ? value : fallback;
}
function envBool(name: string, fallback: boolean): boolean {
  const raw = process.env[name]?.trim().toLowerCase();
  if (raw === undefined || raw === '') return fallback;
  if (['1', 'true', 'yes', 'on'].includes(raw)) return true;
  if (['0', 'false', 'no', 'off'].includes(raw)) return false;
  return fallback;
}
function envList(name: string): string[] {
  return (process.env[name] ?? '')
    .split(',')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
}

export const agentConfig = {
  enabled: envBool('BROWSER_AGENT_ENABLED', true),
  maxSessionsPerOwner: envNum('BROWSER_AGENT_MAX_SESSIONS_PER_OWNER', 3),
  maxSessionsTotal: envNum('BROWSER_AGENT_MAX_SESSIONS_TOTAL', 12),
  idleTtlMs: envNum('BROWSER_AGENT_IDLE_TTL_MS', 30 * 60_000),
  absoluteTtlMs: envNum('BROWSER_AGENT_ABSOLUTE_TTL_MS', 4 * 60 * 60_000),
  defaultTimeoutMs: envNum('BROWSER_AGENT_DEFAULT_TIMEOUT_MS', 30_000),
  maxTimeoutMs: envNum('BROWSER_AGENT_MAX_TIMEOUT_MS', 60_000),
  maxActionsPerBatch: envNum('BROWSER_AGENT_MAX_ACTIONS', 20),
  maxLongPollMs: envNum('BROWSER_AGENT_MAX_LONGPOLL_MS', 25_000),
  maxElements: envNum('BROWSER_AGENT_MAX_ELEMENTS', 500),
  maxVisibleTextChars: envNum('BROWSER_AGENT_MAX_TEXT_CHARS', 30_000),
  // Empty allowlist means "any public https host" (dev). In production the MCP
  // deployment sets BROWSER_AGENT_ALLOWED_DOMAINS to a concrete allowlist.
  allowedDomains: envList('BROWSER_AGENT_ALLOWED_DOMAINS'),
  allowPrivateNetworks: envBool('BROWSER_AGENT_ALLOW_PRIVATE_NETWORKS', false),
  allowInsecureHttp: envBool('BROWSER_AGENT_ALLOW_INSECURE_HTTP', false),
  acceptDownloads: envBool('BROWSER_AGENT_ACCEPT_DOWNLOADS', false),
  uploadsDir: process.env.BROWSER_AGENT_UPLOADS_DIR ?? null,
  // A JSON object mapping secret_ref -> literal value, provided out-of-band via
  // a mounted file. Values are resolved only here and never returned/logged.
  secretsFile: process.env.BROWSER_AGENT_SECRETS_FILE ?? null,
  seleniumUrl: process.env.BROWSER_AGENT_SELENIUM_URL ?? null,
  seleniumAuthSecret: process.env.SERVER_AUTH_SECRET ?? null,
} as const;

function validAllowedDomain(domain: string): boolean {
  return (
    domain.includes('.') &&
    !domain.startsWith('.') &&
    !domain.endsWith('.') &&
    !domain.includes('..') &&
    /^[a-z0-9.-]+$/.test(domain)
  );
}

if (
  process.env.NODE_ENV === 'production' &&
  agentConfig.enabled &&
  (agentConfig.allowedDomains.length === 0 || !agentConfig.allowedDomains.every(validAllowedDomain))
) {
  throw new Error(
    'production browser agent requires BROWSER_AGENT_ALLOWED_DOMAINS hostnames without schemes or ports',
  );
}

// ---------------------------------------------------------------------------
// Wire schemas (mirror of schemas/browser-*.schema.json)
// ---------------------------------------------------------------------------

const TargetSchema = z
  .object({
    ref: z.string().min(1).max(64).optional(),
    role: z.string().min(1).max(64).optional(),
    name: z.string().max(400).optional(),
    label: z.string().max(400).optional(),
    placeholder: z.string().max(400).optional(),
    visible_text: z.string().max(400).optional(),
    title: z.string().max(400).optional(),
    test_id: z.string().max(200).optional(),
    exact: z.boolean().optional(),
    nth: z.number().int().min(0).max(200).optional(),
    frame_ref: z.string().min(1).max(64).optional(),
    css_fallback: z.string().min(1).max(600).optional(),
  })
  .strict();
export type Target = z.infer<typeof TargetSchema>;

const ValueSchema = z.union([
  z.object({ literal: z.string().max(20_000) }).strict(),
  z.object({ secret_ref: z.string().min(1).max(300) }).strict(),
]);

const FileTokenSchema = z
  .string()
  .min(1)
  .max(128)
  .regex(/^[A-Za-z0-9][A-Za-z0-9._-]*$/)
  .refine((value) => !value.includes('..'));

const MAX_INLINE_UPLOAD_BYTES = 256 * 1024;
const MAX_INLINE_UPLOAD_BASE64_CHARS = 4 * Math.ceil(MAX_INLINE_UPLOAD_BYTES / 3);
const BASE64_RE = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const MIME_TYPE_RE = /^[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*\/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*$/;

const InlineUploadSchema = z
  .object({
    file_name: z
      .string()
      .min(1)
      .max(128)
      .refine(
        (value) =>
          value !== '.' &&
          value !== '..' &&
          !value.includes('/') &&
          !value.includes('\\') &&
          !/[\u0000-\u001f\u007f]/.test(value),
        'file_name must be a plain filename without path separators or control characters',
      ),
    mime_type: z.string().min(3).max(100).regex(MIME_TYPE_RE).optional(),
    data_base64: z
      .string()
      .min(4)
      .max(MAX_INLINE_UPLOAD_BASE64_CHARS)
      .regex(BASE64_RE, 'data_base64 must use canonical padded base64')
      .refine(
        (value) => Buffer.from(value, 'base64').byteLength <= MAX_INLINE_UPLOAD_BYTES,
        `decoded file must be no more than ${MAX_INLINE_UPLOAD_BYTES} bytes`,
      ),
  })
  .strict();

const WaitConditionSchema = z.union([
  z.object({ duration_ms: z.number().int().min(0).max(60_000) }).strict(),
  z.object({ url_matches: z.string().min(1).max(600) }).strict(),
  z.object({ text_visible: z.string().min(1).max(400) }).strict(),
  z.object({ element_visible: TargetSchema }).strict(),
  z
    .object({ load_state: z.enum(['domcontentloaded', 'load', 'networkidle']) })
    .strict(),
]);

const ActionSchema = z.union([
  z
    .object({
      type: z.literal('start'),
      initial_url: z.string().url().optional(),
      browser: z.enum(['chromium', 'firefox', 'webkit', 'selenium']).optional(),
      viewport: z
        .object({
          width: z.number().int().min(200).max(4000),
          height: z.number().int().min(200).max(4000),
        })
        .strict()
        .optional(),
      locale: z.string().max(35).optional(),
      timezone: z.string().max(64).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal('goto'),
      url: z.string().url(),
      wait_until: z
        .enum(['commit', 'domcontentloaded', 'load', 'networkidle'])
        .optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal('fill'),
      target: TargetSchema,
      value: ValueSchema,
      clear_first: z.boolean().optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal('type'),
      target: TargetSchema,
      value: ValueSchema,
      clear_first: z.boolean().optional(),
      delay_ms: z.number().int().min(0).max(250).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal('fill_form'),
      fields: z
        .array(z.object({ target: TargetSchema, value: ValueSchema }).strict())
        .min(1)
        .max(50),
    })
    .strict(),
  z
    .object({
      type: z.literal('click'),
      target: TargetSchema,
      button: z.enum(['left', 'middle', 'right']).optional(),
    })
    .strict(),
  z
    .object({
      type: z.literal('submit'),
      target: TargetSchema,
    })
    .strict(),
  z
    .object({
      type: z.literal('select'),
      target: TargetSchema,
      option: z.union([
        z.object({ label: z.string().max(400) }).strict(),
        z.object({ value: z.string().max(400) }).strict(),
        z.object({ index: z.number().int().min(0).max(1000) }).strict(),
      ]),
    })
    .strict(),
  z.object({ type: z.literal('check'), target: TargetSchema }).strict(),
  z.object({ type: z.literal('uncheck'), target: TargetSchema }).strict(),
  z
    .object({
      type: z.literal('press'),
      target: TargetSchema.optional(),
      key: z.string().min(1).max(60),
    })
    .strict(),
  z
    .object({
      type: z.literal('upload'),
      target: TargetSchema,
      file_token: FileTokenSchema.optional(),
      inline_file: InlineUploadSchema.optional(),
    })
    .strict()
    .superRefine((value, ctx) => {
      if ((value.file_token === undefined) === (value.inline_file === undefined)) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          message: 'upload requires exactly one of file_token or inline_file',
        });
      }
    }),
  z
    .object({
      type: z.literal('scroll'),
      target: TargetSchema.optional(),
      delta_x: z.number().int().min(-5000).max(5000).optional(),
      delta_y: z.number().int().min(-5000).max(5000).optional(),
    })
    .strict(),
  z.object({ type: z.literal('screenshot') }).strict(),
  z
    .object({
      type: z.literal('extract'),
      include: z
        .array(
          z.enum([
            'summary',
            'visible_text',
            'interactive_elements',
            'accessibility_snapshot',
            'forms',
            'validation_errors',
            'dialogs',
            'frames',
            'downloads',
            'network_failures',
          ]),
        )
        .max(10)
        .optional(),
      max_visible_text_chars: z
        .number()
        .int()
        .min(200)
        .max(agentConfig.maxVisibleTextChars)
        .optional(),
    })
    .strict(),
  z.object({ type: z.literal('wait'), condition: WaitConditionSchema }).strict(),
  z.object({ type: z.literal('back') }).strict(),
  z.object({ type: z.literal('forward') }).strict(),
  z.object({ type: z.literal('reload') }).strict(),
  z.object({ type: z.literal('close') }).strict(),
]);
export type BrowserAction = z.infer<typeof ActionSchema>;

const ActRequestSchema = z
  .object({
    request_id: z.string().min(1).max(200),
    session_id: z.string().min(1).max(200).optional(),
    owner: z.string().min(1).max(200).optional(),
    intent: z.string().min(1).max(2000),
    expected_revision: z.number().int().min(0).optional(),
    actions: z.array(ActionSchema).min(1).max(agentConfig.maxActionsPerBatch),
    stop_when: z
      .object({
        url_matches: z.string().max(600).optional(),
        text_visible: z.string().max(400).optional(),
        element_visible: TargetSchema.optional(),
        navigation_occurs: z.boolean().optional(),
        validation_error_occurs: z.boolean().optional(),
      })
      .strict()
      .optional(),
    confirmation: z
      .object({
        action_digest: z.string().min(1).max(200),
        confirmed_revision: z.number().int().min(0),
        user_explicitly_approved: z.literal(true),
      })
      .strict()
      .optional(),
    timeout_ms: z.number().int().min(1000).max(agentConfig.maxTimeoutMs).optional(),
    allowed_domains: z.array(z.string().max(200)).max(200).optional(),
  })
  .strict();
export type ActRequest = z.infer<typeof ActRequestSchema>;

const ObserveRequestSchema = z
  .object({
    session_id: z.string().min(1).max(200),
    owner: z.string().min(1).max(200).optional(),
    since_revision: z.number().int().min(0).optional(),
    wait_ms: z.number().int().min(0).max(agentConfig.maxLongPollMs).optional(),
    include: z
      .array(
        z.enum([
          'summary',
          'visible_text',
          'interactive_elements',
          'accessibility_snapshot',
          'forms',
          'validation_errors',
          'dialogs',
          'frames',
          'downloads',
          'network_failures',
          'screenshot',
        ]),
      )
      .max(10)
      .optional(),
    max_elements: z.number().int().min(1).max(agentConfig.maxElements).optional(),
    max_visible_text_chars: z
      .number()
      .int()
      .min(200)
      .max(agentConfig.maxVisibleTextChars)
      .optional(),
    redaction: z.enum(['strict', 'standard']).optional(),
  })
  .strict();
export type ObserveRequest = z.infer<typeof ObserveRequestSchema>;

// ---------------------------------------------------------------------------
// Error model
// ---------------------------------------------------------------------------

export type ErrorCode =
  | 'invalid_request'
  | 'session_not_found'
  | 'session_expired'
  | 'session_busy'
  | 'revision_conflict'
  | 'idempotency_conflict'
  | 'domain_not_allowed'
  | 'target_not_found'
  | 'ambiguous_target'
  | 'action_timeout'
  | 'navigation_failed'
  | 'secret_required'
  | 'unsafe_download'
  | 'too_many_sessions'
  | 'upload_not_supported'
  | 'browser_crashed'
  | 'sensitive_field_blocked'
  | 'internal_error';

export class AgentError extends Error {
  constructor(
    public code: ErrorCode,
    message: string,
  ) {
    super(message);
    this.name = 'AgentError';
  }
}

export type BlockerType =
  | 'captcha'
  | 'mfa'
  | 'secret_required'
  | 'payment'
  | 'signature'
  | 'legal_attestation'
  | 'final_submission'
  | 'domain_not_allowed'
  | 'unsafe_download'
  | 'ambiguous_target'
  | 'sensitive_field_blocked'
  | 'other';

// ---------------------------------------------------------------------------
// In-page extraction (the ONLY script that ever runs in a page; fixed source).
// Returns a normalized, model-readable snapshot. Sensitive field values are
// never returned — only a value_state marker. Elements are tagged with
// data-dd-ref so refs are resolvable later.
// ---------------------------------------------------------------------------

interface RawElement {
  fp: string;
  role: string;
  name: string;
  label: string;
  tag: string;
  type: string;
  placeholder: string;
  testId: string;
  required: boolean;
  disabled: boolean;
  readOnly: boolean;
  checked: boolean;
  sensitive: boolean;
  hasValue: boolean;
  selectedOption: string;
  options: { label: string; value: string; selected: boolean; disabled: boolean }[];
  validationMessage: string;
  formFp: string;
  isSubmit: boolean;
  interactive: boolean;
}
interface RawForm {
  fp: string;
  name: string;
  method: string;
  actionOrigin: string;
  fieldFps: string[];
  submitFps: string[];
}
interface RawSnapshot {
  title: string;
  visibleText: string;
  elements: RawElement[];
  forms: RawForm[];
  signals: {
    captcha: boolean;
    mfa: boolean;
    payment: boolean;
    signature: boolean;
    legalAttestation: boolean;
  };
}

// NOTE: This function is serialized and executed in the browser. It must be
// self-contained (no closure over Node scope) and reference only its argument.
/* eslint-disable */
function EXTRACT_FN(opts: { maxElements: number; maxTextChars: number }): RawSnapshot {
  const SENSITIVE_RE =
    /pass(word|code)|ssn|social.?security|tax.?id|ein|routing|account.?number|\bcvv\b|\bcvc\b|card.?number|\bccnum|security.?code|\botp\b|one.?time|\bpin\b|secret|private.?key/i;
  const CAPTCHA_RE = /captcha|recaptcha|hcaptcha|turnstile|are you (a )?human|i'?m not a robot/i;
  const MFA_RE =
    /(two|multi).?factor|2fa|mfa|authenticator|verification code|one.?time (code|password)|enter the code/i;
  const PAYMENT_RE =
    /card number|credit card|payment method|cardholder|expiration date|billing address|\bcvv\b|\bcvc\b/i;
  const SIGNATURE_RE =
    /\be-?sign\b|electronic signature|sign here|draw your signature|docusign/i;
  const LEGAL_RE =
    /under penalty of perjury|i (hereby )?certify|sworn (statement|declaration)|i attest|legally binding|i declare under/i;

  function computeRole(el: Element): string {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a' && el.hasAttribute('href')) return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'input') {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      if (t === 'checkbox') return 'checkbox';
      if (t === 'radio') return 'radio';
      if (t === 'button' || t === 'submit' || t === 'reset') return 'button';
      if (t === 'range') return 'slider';
      return 'textbox';
    }
    return tag;
  }
  function accName(el: Element): string {
    const al = el.getAttribute('aria-label');
    if (al && al.trim()) return al.trim();
    const labelledby = el.getAttribute('aria-labelledby');
    if (labelledby) {
      const parts = labelledby
        .split(/\s+/)
        .map((id) => (el.ownerDocument.getElementById(id) || {}).textContent || '')
        .join(' ')
        .trim();
      if (parts) return parts;
    }
    const id = el.getAttribute('id');
    if (id) {
      const forLabel = el.ownerDocument.querySelector('label[for="' + CSS.escape(id) + '"]');
      if (forLabel && forLabel.textContent) return forLabel.textContent.trim();
    }
    const parentLabel = el.closest('label');
    if (parentLabel && parentLabel.textContent) return parentLabel.textContent.trim();
    const text = (el.textContent || '').trim();
    if (text && text.length <= 200) return text;
    const ph = el.getAttribute('placeholder');
    if (ph) return ph.trim();
    const title = el.getAttribute('title');
    if (title) return title.trim();
    return '';
  }
  function labelFor(el: Element): string {
    const id = el.getAttribute('id');
    if (id) {
      const forLabel = el.ownerDocument.querySelector('label[for="' + CSS.escape(id) + '"]');
      if (forLabel && forLabel.textContent) return forLabel.textContent.trim();
    }
    const parentLabel = el.closest('label');
    if (parentLabel && parentLabel.textContent) return parentLabel.textContent.trim();
    return '';
  }
  function isVisible(el: Element): boolean {
    const he = el as HTMLElement;
    if (!he.getClientRects || he.getClientRects().length === 0) return false;
    const style = el.ownerDocument.defaultView?.getComputedStyle(he);
    if (!style) return true;
    return style.visibility !== 'hidden' && style.display !== 'none' && style.opacity !== '0';
  }
  function fingerprint(el: Element, ord: number): string {
    const parts = [
      el.tagName.toLowerCase(),
      el.getAttribute('id') || '',
      el.getAttribute('name') || '',
      el.getAttribute('type') || '',
      computeRole(el),
      accName(el).slice(0, 60),
      String(ord),
    ];
    let h = 0;
    const s = parts.join('|');
    for (let i = 0; i < s.length; i++) {
      h = (h * 31 + s.charCodeAt(i)) | 0;
    }
    return 'f' + (h >>> 0).toString(36);
  }

  const roots: (Document | ShadowRoot)[] = [document];
  const elements: RawElement[] = [];
  const forms: RawForm[] = [];
  const seenForms = new Map<Element, string>();
  let ord = 0;

  function processRoot(root: Document | ShadowRoot): void {
    const all = root.querySelectorAll('*');
    for (let i = 0; i < all.length; i++) {
      const el = all[i]!;
      if ((el as HTMLElement).shadowRoot) roots.push((el as HTMLElement).shadowRoot!);
      const tag = el.tagName.toLowerCase();
      const isFormEl = ['input', 'select', 'textarea', 'button'].includes(tag);
      const role = computeRole(el);
      const clickable =
        isFormEl ||
        tag === 'a' ||
        role === 'button' ||
        role === 'link' ||
        role === 'checkbox' ||
        role === 'radio' ||
        role === 'tab' ||
        role === 'menuitem' ||
        el.hasAttribute('onclick');
      if (!clickable) continue;
      if (!isVisible(el)) continue;
      if (elements.length >= opts.maxElements) break;
      ord++;
      const type = (el.getAttribute('type') || '').toLowerCase();
      const name = accName(el);
      const sensitive =
        type === 'password' ||
        SENSITIVE_RE.test(el.getAttribute('name') || '') ||
        SENSITIVE_RE.test(el.getAttribute('id') || '') ||
        SENSITIVE_RE.test(el.getAttribute('autocomplete') || '') ||
        SENSITIVE_RE.test(name);
      const input = el as HTMLInputElement;
      const options: RawElement['options'] = [];
      let selectedOption = '';
      if (tag === 'select') {
        const sel = el as HTMLSelectElement;
        for (let o = 0; o < sel.options.length && o < 200; o++) {
          const opt = sel.options[o]!;
          options.push({
            label: (opt.label || opt.textContent || '').trim().slice(0, 200),
            value: opt.value,
            selected: opt.selected,
            disabled: opt.disabled,
          });
          if (opt.selected) selectedOption = (opt.label || opt.textContent || '').trim();
        }
      }
      const fp = fingerprint(el, ord);
      el.setAttribute('data-dd-fp', fp);
      const parentForm = el.closest('form');
      let formFp = '';
      if (parentForm) {
        formFp = seenForms.get(parentForm) || fingerprint(parentForm, 100000 + seenForms.size);
        if (!seenForms.has(parentForm)) {
          seenForms.set(parentForm, formFp);
          parentForm.setAttribute('data-dd-fp', formFp);
        }
      }
      const isSubmit =
        (tag === 'button' && (input.type || 'submit') !== 'button') ||
        (tag === 'input' && type === 'submit');
      const hasValue = 'value' in el ? Boolean((input.value || '').length) : false;
      elements.push({
        fp,
        role,
        name: name.slice(0, 200),
        label: labelFor(el).slice(0, 200),
        tag,
        type,
        placeholder: (el.getAttribute('placeholder') || '').slice(0, 200),
        testId:
          el.getAttribute('data-testid') ||
          el.getAttribute('data-test-id') ||
          el.getAttribute('data-test') ||
          '',
        required: input.required || el.getAttribute('aria-required') === 'true',
        disabled: input.disabled || el.getAttribute('aria-disabled') === 'true',
        readOnly: input.readOnly || el.getAttribute('aria-readonly') === 'true',
        checked: input.checked || el.getAttribute('aria-checked') === 'true',
        sensitive,
        hasValue,
        selectedOption,
        options,
        validationMessage:
          typeof input.validationMessage === 'string' ? input.validationMessage.slice(0, 300) : '',
        formFp,
        isSubmit,
        interactive: true,
      });
    }
  }

  for (let r = 0; r < roots.length && r < 500; r++) processRoot(roots[r]!);

  const formEls = document.querySelectorAll('form');
  for (let i = 0; i < formEls.length && i < 100; i++) {
    const form = formEls[i]!;
    const fp = seenForms.get(form) || form.getAttribute('data-dd-fp') || fingerprint(form, 200000 + i);
    const fieldFps: string[] = [];
    const submitFps: string[] = [];
    for (const e of elements) {
      if (e.formFp === fp) {
        if (e.isSubmit) submitFps.push(e.fp);
        else fieldFps.push(e.fp);
      }
    }
    let actionOrigin = '';
    try {
      actionOrigin = new URL((form as HTMLFormElement).action, document.baseURI).origin;
    } catch (_e) {
      actionOrigin = '';
    }
    forms.push({
      fp,
      name: form.getAttribute('name') || form.getAttribute('id') || '',
      method: ((form as HTMLFormElement).method || 'get').toLowerCase(),
      actionOrigin,
      fieldFps,
      submitFps,
    });
  }

  const bodyText = (document.body?.innerText || '').replace(/\s+/g, ' ').trim();
  const visibleText = bodyText.slice(0, opts.maxTextChars);
  const haystack = (bodyText + ' ' + (document.title || '')).slice(0, 40000);
  const visibleFormControls = Array.from(
    document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>(
      'input:not([type="hidden"]),textarea,select',
    ),
  ).filter((el) => {
    const style = window.getComputedStyle(el);
    return !el.disabled && style.display !== 'none' && style.visibility !== 'hidden';
  });
  const hasExplicitMfaControl = !!document.querySelector(
    [
      'input[autocomplete="one-time-code"]',
      'input[name*="otp" i]',
      'input[id*="otp" i]',
      'input[name*="one_time" i]',
      'input[id*="one-time" i]',
      'input[name*="verification_code" i]',
      'input[id*="verification-code" i]',
    ].join(','),
  );
  const hasMfaPromptShape =
    MFA_RE.test(haystack) && visibleFormControls.length > 0 && visibleFormControls.length <= 4;
  const hasExplicitPaymentControl = !!document.querySelector(
    [
      'input[autocomplete="cc-number"]',
      'input[autocomplete="cc-exp"]',
      'input[autocomplete="cc-csc"]',
      'input[name*="cardnumber" i]',
      'input[id*="card-number" i]',
      'input[name="cvv" i]',
      'input[name="cvc" i]',
    ].join(','),
  );
  const hasPaymentPromptShape =
    PAYMENT_RE.test(haystack) && visibleFormControls.length > 0 && visibleFormControls.length <= 4;

  return {
    title: document.title || '',
    visibleText,
    elements,
    forms,
    signals: {
      captcha: CAPTCHA_RE.test(haystack) || !!document.querySelector('iframe[src*="captcha" i],iframe[src*="recaptcha" i],iframe[title*="captcha" i],.g-recaptcha,.h-captcha,.cf-turnstile'),
      // A vendor page may mention MFA in marketing, support, or footer copy.
      // Treat it as a blocker only when the page also looks like a compact
      // code-entry challenge or exposes an explicit one-time-code control.
      mfa: hasExplicitMfaControl || hasMfaPromptShape,
      // Pricing/help copy is common on application pages. Require an explicit
      // card control or a compact payment-form shape before blocking actions.
      payment: hasExplicitPaymentControl || hasPaymentPromptShape,
      signature: SIGNATURE_RE.test(haystack) || !!document.querySelector('canvas[class*="sign" i],iframe[src*="docusign" i]'),
      legalAttestation: LEGAL_RE.test(haystack),
    },
  };
}
/* eslint-enable */

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

interface RefEntry {
  fp: string;
  frameRef: string;
}
interface DialogRecord {
  type: 'alert' | 'confirm' | 'prompt' | 'beforeunload';
  message: string;
}
interface DownloadRecord {
  suggestedFilename: string;
  url: string;
}
interface ScreenshotRecord {
  mime_type: 'image/jpeg';
  width: number;
  height: number;
  data_base64: string;
}
interface NetworkFailure {
  method: string;
  urlOrigin: string;
  resourceType: string;
  error: string;
}
interface PendingAction {
  description: string;
  targetRef: string;
  pageUrl: string;
  revision: number;
  actionDigest: string;
  consequences: string[];
}

interface Session {
  id: string;
  owner: string;
  engine: 'chromium' | 'firefox' | 'webkit';
  context: BrowserContext;
  page: Page;
  revision: number;
  createdAt: number;
  lastActivity: number;
  busy: boolean;
  allowedDomains: string[];
  refByFp: Map<string, string>; // fp -> ref (e.g. "e1")
  refEntries: Map<string, RefEntry>; // ref -> {fp, frameRef}
  frameRefByUrl: Map<string, string>;
  refCounter: number;
  frameCounter: number;
  dialogs: DialogRecord[];
  downloads: DownloadRecord[];
  networkFailures: NetworkFailure[];
  lastSnapshot: RawSnapshot | null;
  lastBlocker: { type: BlockerType; message: string } | null;
  pending: PendingAction | null;
  lastActionScreenshot: ScreenshotRecord | null;
  lastExtraction: Record<string, unknown> | null;
  idempotency: Map<string, unknown>;
  waiters: ((rev: number) => void)[];
}

const sessions = new Map<string, Session>();

// Cap concurrent long-poll waiters per session (DoS guard).
const MAX_WAITERS_PER_SESSION = 24;

function ownerCount(owner: string): number {
  let n = 0;
  for (const s of sessions.values()) if (s.owner === owner) n++;
  return n;
}

function bumpRevision(session: Session): void {
  session.revision += 1;
  session.lastActivity = Date.now();
  const waiters = session.waiters;
  session.waiters = [];
  for (const w of waiters) w(session.revision);
}

async function destroySession(session: Session, reason: string): Promise<void> {
  sessions.delete(session.id);
  for (const w of session.waiters) w(session.revision);
  session.waiters = [];
  try {
    await session.context.close();
  } catch {
    /* already gone */
  }
  void reason;
}

// Idle / absolute TTL sweep.
let sweepTimer: NodeJS.Timeout | null = null;
function startSweeper(log: FastifyBaseLogger): void {
  if (sweepTimer) return;
  sweepTimer = setInterval(() => {
    const now = Date.now();
    for (const s of [...sessions.values()]) {
      const idle = now - s.lastActivity > agentConfig.idleTtlMs;
      const aged = now - s.createdAt > agentConfig.absoluteTtlMs;
      if (idle || aged) {
        log.info({ sessionId: s.id, idle, aged }, 'browser-agent: evicting session');
        void destroySession(s, idle ? 'idle_ttl' : 'absolute_ttl');
      }
    }
  }, 30_000);
  sweepTimer.unref();
}

export async function closeAllSessions(): Promise<void> {
  if (sweepTimer) clearInterval(sweepTimer);
  sweepTimer = null;
  await Promise.all([...sessions.values()].map((s) => destroySession(s, 'shutdown')));
}

// ---------------------------------------------------------------------------
// URL / domain policy
// ---------------------------------------------------------------------------

const BLOCKED_SCHEMES = new Set(['file:', 'data:', 'javascript:', 'chrome:', 'about:', 'blob:', 'ftp:']);

function domainAllowed(host: string, allowlist: string[]): boolean {
  if (allowlist.length === 0) return true; // dev: any public host
  const h = host.toLowerCase();
  return allowlist.some((d) => h === d || h.endsWith('.' + d));
}

function requestUrlAllowedByDomain(raw: string, allowlist: string[]): boolean {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return false;
  }
  if (url.username || url.password) return false;
  const secureNetworkProtocol = url.protocol === 'https:';
  const allowedDevelopmentProtocol = agentConfig.allowInsecureHttp && url.protocol === 'http:';
  if (!secureNetworkProtocol && !allowedDevelopmentProtocol) return false;
  const defaultPort =
    url.port === '' ||
    (url.protocol === 'https:' && url.port === '443') ||
    (url.protocol === 'http:' && url.port === '80');
  // Explicit ports are needed by local integration fixtures, but only when the
  // operator has already opted into private-network access. Production keeps
  // that option disabled and therefore remains limited to 80/443 defaults.
  return (defaultPort || agentConfig.allowPrivateNetworks) && domainAllowed(url.hostname, allowlist);
}

// Each entry authorizes that hostname and its subdomains. Intersect the
// caller's requested scope with the process-level production ceiling by keeping
// the more specific entry whenever the two suffix trees overlap.
function effectiveAllowedDomains(requested: string[] | undefined, configured: string[]): string[] {
  const requestDomains = (requested ?? []).map((domain) => domain.toLowerCase());
  if (configured.length === 0) return requestDomains;
  if (requestDomains.length === 0) return configured;

  const effective = new Set<string>();
  for (const requestDomain of requestDomains) {
    for (const configuredDomain of configured) {
      if (requestDomain === configuredDomain || requestDomain.endsWith('.' + configuredDomain)) {
        effective.add(requestDomain);
      } else if (configuredDomain.endsWith('.' + requestDomain)) {
        effective.add(configuredDomain);
      }
    }
  }
  return [...effective];
}

async function assertUrlAllowed(
  raw: string,
  allowlist: string[],
  isPrivateIp: (addr: string) => boolean,
): Promise<URL> {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new AgentError('invalid_request', 'malformed URL');
  }
  if (BLOCKED_SCHEMES.has(url.protocol)) {
    throw new AgentError('domain_not_allowed', `scheme ${url.protocol} is not permitted`);
  }
  if (url.protocol !== 'https:' && !(agentConfig.allowInsecureHttp && url.protocol === 'http:')) {
    throw new AgentError('domain_not_allowed', 'only https:// navigation is permitted');
  }
  if (url.username || url.password) {
    throw new AgentError('domain_not_allowed', 'URL credentials are not permitted');
  }
  if (!requestUrlAllowedByDomain(raw, allowlist)) {
    throw new AgentError(
      'domain_not_allowed',
      `host ${url.hostname} or its explicit port is not on the allowlist`,
    );
  }
  // SSRF: reject IP literals and DNS answers that resolve to private ranges.
  if (isIP(url.hostname)) {
    if (!agentConfig.allowPrivateNetworks && isPrivateIp(url.hostname)) {
      throw new AgentError('domain_not_allowed', 'IP-literal targets to private ranges are blocked');
    }
  } else if (!agentConfig.allowPrivateNetworks) {
    try {
      const answers = await dnsLookup(url.hostname, { all: true });
      for (const a of answers) {
        if (isPrivateIp(a.address)) {
          throw new AgentError('domain_not_allowed', 'host resolves to a private/reserved address');
        }
      }
    } catch (e) {
      if (e instanceof AgentError) throw e;
      throw new AgentError('navigation_failed', 'DNS resolution failed for target host');
    }
  }
  return url;
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

// A secret entry is either a bare string (no domain binding) or an object with
// an explicit `domains` allowlist. On a public endpoint, bind every secret to
// specific domains: a bound secret is only ever typed into a matching origin, so
// a caller cannot steer the browser to an attacker page and harvest it there.
type SecretEntry = string | { value: string; domains?: string[] };
let secretsCache: Record<string, SecretEntry> | null = null;

async function resolveSecret(ref: string, currentHost: string): Promise<string> {
  if (!agentConfig.secretsFile) {
    throw new AgentError('secret_required', 'secret references are not configured on this worker');
  }
  if (!secretsCache) {
    try {
      secretsCache = JSON.parse(await readFile(agentConfig.secretsFile, 'utf8')) as Record<string, SecretEntry>;
    } catch {
      throw new AgentError('secret_required', 'secret store is unavailable');
    }
  }
  const entry = secretsCache[ref];
  if (entry === undefined) {
    throw new AgentError('secret_required', 'no secret bound to that reference');
  }
  if (typeof entry === 'string') {
    throw new AgentError('secret_required', 'secret entries must be domain-bound objects');
  }
  if (typeof entry.value !== 'string') {
    throw new AgentError('secret_required', 'malformed secret entry');
  }
  if (!entry.domains || entry.domains.length === 0) {
    throw new AgentError('secret_required', 'secret entry must declare at least one permitted domain');
  }
  const host = currentHost.toLowerCase();
  const permitted = entry.domains.some(
    (domain) => host === domain.toLowerCase() || host.endsWith('.' + domain.toLowerCase()),
  );
  if (!permitted) {
    throw new AgentError('secret_required', 'secret is not permitted on the current page domain');
  }
  return entry.value;
}

async function resolveUploadToken(token: string): Promise<string> {
  if (!agentConfig.uploadsDir) {
    throw new AgentError('unsafe_download', 'the operator-staged upload token store is not enabled');
  }
  const base = await realpath(agentConfig.uploadsDir).catch(() => {
    throw new AgentError('unsafe_download', 'the upload token store is unavailable');
  });
  const file = await realpath(resolve(base, token)).catch(() => {
    throw new AgentError('unsafe_download', 'file_token is unknown or expired');
  });
  const child = relative(base, file);
  if (!child || child.startsWith('..') || isAbsolute(child)) {
    throw new AgentError('unsafe_download', 'file_token resolved outside the upload token store');
  }
  const metadata = await stat(file);
  if (!metadata.isFile() || metadata.size > 25 * 1024 * 1024) {
    throw new AgentError('unsafe_download', 'upload token must reference a regular file up to 25 MiB');
  }
  return file;
}

function resolveInlineUpload(inline: z.infer<typeof InlineUploadSchema>): {
  name: string;
  mimeType: string;
  buffer: Buffer;
} {
  const buffer = Buffer.from(inline.data_base64, 'base64');
  if (
    buffer.byteLength > MAX_INLINE_UPLOAD_BYTES ||
    buffer.toString('base64') !== inline.data_base64
  ) {
    throw new AgentError(
      'unsafe_download',
      `inline upload must be canonical base64 encoding no more than ${MAX_INLINE_UPLOAD_BYTES} bytes`,
    );
  }
  return {
    name: inline.file_name,
    mimeType: inline.mime_type ?? 'application/octet-stream',
    buffer,
  };
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

function frameForRef(session: Session, frameRef?: string): Frame | Page {
  if (!frameRef) return session.page;
  for (const [url, ref] of session.frameRefByUrl) {
    if (ref === frameRef) {
      const f = session.page.frames().find((fr) => fr.url() === url);
      if (f) return f;
    }
  }
  return session.page;
}

async function resolveTarget(session: Session, target: Target): Promise<Locator> {
  const scope = frameForRef(session, target.frame_ref);
  // 1) explicit ref -> data-dd-fp tag from the latest snapshot.
  if (target.ref) {
    const entry = session.refEntries.get(target.ref);
    if (!entry) {
      throw new AgentError('target_not_found', `ref ${target.ref} is not in the latest observed state`);
    }
    const scoped = frameForRef(session, entry.frameRef);
    const loc = scoped.locator(`[data-dd-fp="${cssEscape(entry.fp)}"]`);
    if ((await loc.count()) === 0) {
      throw new AgentError('target_not_found', `ref ${target.ref} no longer resolves; re-observe`);
    }
    return loc.first();
  }
  // 2..7) semantic strategies in priority order.
  const candidates: Locator[] = [];
  const exact = target.exact ?? false;
  if (target.role) {
    const opts: { name?: string; exact?: boolean } = {};
    if (target.name) {
      opts.name = target.name;
      opts.exact = exact;
    }
    // getByRole accepts an ARIA role string; validated as free text upstream.
    candidates.push(scope.getByRole(target.role as Parameters<Frame['getByRole']>[0], opts));
  }
  if (target.label) candidates.push(scope.getByLabel(target.label, { exact }));
  if (target.placeholder) candidates.push(scope.getByPlaceholder(target.placeholder, { exact }));
  if (target.visible_text) candidates.push(scope.getByText(target.visible_text, { exact }));
  if (target.title) candidates.push(scope.getByTitle(target.title, { exact }));
  if (target.test_id) candidates.push(scope.getByTestId(target.test_id));
  // Force the CSS engine so a caller cannot smuggle another Playwright selector
  // engine (e.g. `xpath=`, `text=`) through css_fallback — XPath is unsupported.
  if (target.css_fallback) candidates.push(scope.locator(`css=${target.css_fallback}`));

  if (candidates.length === 0) {
    throw new AgentError('invalid_request', 'target must specify at least one selector strategy');
  }
  for (const loc of candidates) {
    const count = await loc.count();
    if (count === 0) continue;
    if (count === 1) return loc;
    if (target.nth !== undefined) return loc.nth(target.nth);
    // Ambiguous: surface candidates rather than guessing.
    throw new AgentError(
      'ambiguous_target',
      `matched ${count} elements; add nth or a more specific selector`,
    );
  }
  throw new AgentError('target_not_found', 'no element matched the target');
}

function cssEscape(value: string): string {
  return value.replace(/["\\]/g, '\\$&');
}

// ---------------------------------------------------------------------------
// Action digest (for consequential-action confirmation)
// ---------------------------------------------------------------------------

function actionDigest(sessionId: string, revision: number, pageUrl: string, detail: string): string {
  const h = createHash('sha256');
  h.update([sessionId, String(revision), pageUrl, detail].join('\u0000'));
  return 'sha256:' + h.digest('hex');
}

function auditHash(value: string): string {
  return createHash('sha256').update(value).digest('hex').slice(0, 16);
}

function actionDestinationHost(action: BrowserAction): string | undefined {
  const raw =
    action.type === 'goto'
      ? action.url
      : action.type === 'start'
        ? action.initial_url
        : undefined;
  if (!raw) return undefined;
  try {
    return new URL(raw).hostname;
  } catch {
    return undefined;
  }
}

// A click/submit is "consequential" when the control looks like a final
// submission (submit button whose label implies filing/paying/agreeing).
const CONSEQUENTIAL_RE =
  /\b(submit|file|pay|purchase|buy|confirm|agree|accept|place order|sign|send|finalize|complete filing|register)\b/i;

// ---------------------------------------------------------------------------
// Snapshot -> response projection + ref assignment
// ---------------------------------------------------------------------------

function assignRefs(session: Session, snap: RawSnapshot): void {
  // Frames: assign stable frame refs by URL.
  for (const fr of session.page.frames()) {
    if (fr === session.page.mainFrame()) continue;
    if (!session.frameRefByUrl.has(fr.url())) {
      session.frameCounter += 1;
      session.frameRefByUrl.set(fr.url(), `frame${session.frameCounter}`);
    }
  }
  // Elements/forms: reuse a ref for a fingerprint we have seen before.
  for (const el of snap.elements) {
    if (!session.refByFp.has(el.fp)) {
      session.refCounter += 1;
      const ref = `e${session.refCounter}`;
      session.refByFp.set(el.fp, ref);
      session.refEntries.set(ref, { fp: el.fp, frameRef: '' });
    }
  }
  for (const f of snap.forms) {
    if (!session.refByFp.has(f.fp)) {
      session.refCounter += 1;
      const ref = `form${session.refCounter}`;
      session.refByFp.set(f.fp, ref);
      session.refEntries.set(ref, { fp: f.fp, frameRef: '' });
    }
  }
}

function detectBlocker(snap: RawSnapshot): { type: BlockerType; message: string } | null {
  if (snap.signals.captcha) return { type: 'captcha', message: 'CAPTCHA challenge detected; requires human handling.' };
  if (snap.signals.mfa) return { type: 'mfa', message: 'MFA / one-time-code entry detected; requires human handling.' };
  if (snap.signals.payment) return { type: 'payment', message: 'Payment page detected; requires human handling.' };
  if (snap.signals.signature) return { type: 'signature', message: 'Electronic signature detected; requires human handling.' };
  if (snap.signals.legalAttestation)
    return { type: 'legal_attestation', message: 'Legal attestation detected; requires explicit confirmation.' };
  return null;
}

async function takeSnapshot(session: Session, maxElements: number, maxText: number): Promise<RawSnapshot> {
  // esbuild/tsx (dev + tests) instruments named functions with a `__name` helper
  // that is not defined once EXTRACT_FN is serialized into the page. Shim it as
  // identity before evaluating; harmless under tsc production builds (no __name).
  await session.page.evaluate('globalThis.__name = globalThis.__name || function (f) { return f; };');
  const snap = (await session.page.evaluate(EXTRACT_FN, {
    maxElements,
    maxTextChars: maxText,
  })) as RawSnapshot;
  session.lastSnapshot = snap;
  // Content blockers (captcha/mfa/payment/...) are recomputed each snapshot.
  // A policy blocker set by the navigation interceptor (domain_not_allowed) is
  // NOT recomputable from page content, so preserve it here; it is cleared by
  // the framenavigated handler once a navigation actually succeeds.
  const contentBlocker = detectBlocker(snap);
  if (contentBlocker) {
    session.lastBlocker = contentBlocker;
  } else if (session.lastBlocker?.type !== 'domain_not_allowed') {
    session.lastBlocker = null;
  }
  assignRefs(session, snap);
  return snap;
}

function pageInfo(session: Session): { url: string; origin: string; title: string; load_state: string } {
  const url = session.page.url();
  let origin = '';
  try {
    origin = new URL(url).origin;
  } catch {
    origin = '';
  }
  return {
    url,
    origin,
    title: session.lastSnapshot?.title ?? '',
    load_state: 'load',
  };
}

// ---------------------------------------------------------------------------
// Session lifecycle helpers
// ---------------------------------------------------------------------------

interface AgentDeps {
  getBrowser: (engine: 'chromium' | 'firefox' | 'webkit') => Promise<PlaywrightBrowser>;
  isPrivateIp: (address: string) => boolean;
  log: FastifyBaseLogger;
}

async function createSession(
  deps: AgentDeps,
  owner: string,
  start: Extract<BrowserAction, { type: 'start' }>,
  allowedDomains: string[],
): Promise<Session> {
  if (ownerCount(owner) >= agentConfig.maxSessionsPerOwner) {
    throw new AgentError('too_many_sessions', 'per-owner session limit reached; close a session first');
  }
  if (sessions.size >= agentConfig.maxSessionsTotal) {
    throw new AgentError('too_many_sessions', 'global session limit reached; retry later');
  }
  if (start.browser === 'selenium') {
    // Selenium is a stateless declarative runner (dd-selenium-server opens and
    // quits a fresh WebDriver session per /run), so it cannot back a persistent
    // multi-call session. Use chromium/firefox/webkit for stateful flows, or the
    // one-shot /agent/selenium-run bridge for a native Selenium scenario.
    throw new AgentError(
      'invalid_request',
      'selenium is stateless; use chromium/firefox/webkit for persistent sessions or the selenium bridge',
    );
  }
  const engine = start.browser ?? 'chromium';
  const browser = await deps.getBrowser(engine);
  const context = await browser.newContext({
    viewport: start.viewport ?? { width: 1280, height: 900 },
    locale: start.locale,
    timezoneId: start.timezone,
    acceptDownloads: agentConfig.acceptDownloads,
    // Service workers can handle requests outside Playwright's route handler.
    // Block them so the context-wide hostname ceiling covers every request.
    serviceWorkers: 'block',
  });
  const page = await context.newPage();
  const id = randomUUID().replace(/-/g, '') + randomUUID().replace(/-/g, '');
  const now = Date.now();
  const session: Session = {
    id,
    owner,
    engine,
    context,
    page,
    revision: 0,
    createdAt: now,
    lastActivity: now,
    busy: false,
    allowedDomains,
    refByFp: new Map(),
    refEntries: new Map(),
    frameRefByUrl: new Map(),
    refCounter: 0,
    frameCounter: 0,
    dialogs: [],
    downloads: [],
    networkFailures: [],
    lastSnapshot: null,
    lastBlocker: null,
    pending: null,
    lastActionScreenshot: null,
    lastExtraction: null,
    idempotency: new Map(),
    waiters: [],
  };

  // Enforce the session ceiling below model actions: clicks, form redirects,
  // iframes, scripts, images, fetch/XHR, and WebSockets cannot escape merely
  // because page code initiated them rather than an explicit `goto`. DNS is
  // checked once per origin; NetworkPolicy is the per-packet DNS-rebinding
  // backstop for private and reserved ranges.
  await context.route('**/*', async (route) => {
    const request = route.request();
    const raw = request.url();
    const reportNavigationBlock = (message: string): void => {
      if (request.resourceType() === 'document' && request.isNavigationRequest()) {
        session.lastBlocker = { type: 'domain_not_allowed', message };
      }
    };
    let url: URL;
    try {
      url = new URL(raw);
    } catch {
      reportNavigationBlock('navigation URL is malformed');
      await route.abort('blockedbyclient');
      return;
    }
    if (!requestUrlAllowedByDomain(raw, allowedDomains)) {
      reportNavigationBlock(`navigation to ${url.hostname} is outside the domain allowlist`);
      await route.abort('blockedbyclient');
      return;
    }
    try {
      // Re-resolve every outbound request. Chromium performs its own DNS lookup
      // after this policy check, so the pod NetworkPolicy remains the
      // per-packet backstop against DNS rebinding to private/reserved ranges.
      await assertUrlAllowed(raw, allowedDomains, deps.isPrivateIp);
      await route.continue();
    } catch {
      reportNavigationBlock(`navigation to ${url.hostname} failed the public-address policy`);
      await route.abort('blockedbyclient');
    }
  });
  await context.routeWebSocket('**', async (webSocket) => {
    await webSocket.close({ code: 1008, reason: 'non-HTTP(S) schemes are disabled' });
  });

  page.on('dialog', (dialog: Dialog) => {
    session.dialogs.push({
      type: dialog.type() as DialogRecord['type'],
      message: dialog.message().slice(0, 500),
    });
    // Never auto-accept: dismiss so the workflow surfaces the dialog to the model.
    void dialog.dismiss().catch(() => undefined);
    bumpRevision(session);
  });
  page.on('download', (dl) => {
    session.downloads.push({ suggestedFilename: dl.suggestedFilename(), url: dl.url() });
    if (!agentConfig.acceptDownloads) void dl.cancel().catch(() => undefined);
    bumpRevision(session);
  });
  page.on('framenavigated', (fr) => {
    if (fr === session.page.mainFrame()) {
      let destinationHost = '';
      try {
        destinationHost = new URL(fr.url()).hostname;
      } catch {
        destinationHost = '';
      }
      deps.log.info(
        {
          event: 'browser_navigation',
          ownerHash: auditHash(owner),
          sessionHash: auditHash(session.id),
          destinationHost,
        },
        'browser-agent audit',
      );
      bumpRevision(session);
    }
  });
  page.on('requestfailed', (req) => {
    if (session.networkFailures.length >= 50) return;
    let origin = '';
    try {
      origin = new URL(req.url()).origin;
    } catch {
      origin = '';
    }
    session.networkFailures.push({
      method: req.method(),
      urlOrigin: origin,
      resourceType: req.resourceType(),
      error: req.failure()?.errorText ?? 'failed',
    });
  });

  sessions.set(id, session);
  return session;
}

function getOwnedSession(sessionId: string, owner: string): Session {
  const session = sessions.get(sessionId);
  if (!session) throw new AgentError('session_not_found', 'no such session (it may have expired)');
  if (session.owner !== owner) throw new AgentError('session_not_found', 'no such session');
  return session;
}

// ---------------------------------------------------------------------------
// Action execution
// ---------------------------------------------------------------------------

interface ActionResult {
  index: number;
  type: string;
  status: 'completed' | 'skipped' | 'blocked' | 'failed';
  message?: string;
  resolved_ref?: string;
}

async function fillValue(session: Session, value: z.infer<typeof ValueSchema>): Promise<string> {
  if ('literal' in value) return value.literal;
  let host = '';
  try {
    host = new URL(session.page.url()).hostname;
  } catch {
    host = '';
  }
  return resolveSecret(value.secret_ref, host);
}

async function assertResolvedFieldWriteAllowed(
  loc: Locator,
  value: z.infer<typeof ValueSchema>,
): Promise<void> {
  const field = await loc.evaluate((element) => {
    const id = element.getAttribute('id') ?? '';
    const explicitLabel = id
      ? element.ownerDocument.querySelector(`label[for="${CSS.escape(id)}"]`)
      : null;
    const containingLabel = element.closest('label');
    return {
      tagName: element.tagName,
      type: element.getAttribute('type'),
      name: element.getAttribute('name'),
      id,
      autocomplete: element.getAttribute('autocomplete'),
      ariaLabel: element.getAttribute('aria-label'),
      label: (explicitLabel?.textContent ?? containingLabel?.textContent ?? '').trim(),
      placeholder: element.getAttribute('placeholder'),
      title: element.getAttribute('title'),
    };
  });

  try {
    assertSensitiveFieldWriteAllowed(field, 'literal' in value ? 'literal' : 'secret_ref');
  } catch (error) {
    if (error instanceof SensitiveFieldPolicyError) {
      throw new AgentError('sensitive_field_blocked', error.message);
    }
    throw error;
  }
}

// Returns true if the batch produced a state change.
async function runActions(
  session: Session,
  deps: AgentDeps,
  req: ActRequest,
  timeoutMs: number,
): Promise<{
  status: string;
  results: ActionResult[];
  blocker: { type: BlockerType; message: string } | null;
  pending: PendingAction | null;
  changed: boolean;
}> {
  const results: ActionResult[] = [];
  let changed = false;
  session.lastActionScreenshot = null;
  session.lastExtraction = null;
  const deadline = Date.now() + timeoutMs;
  const remaining = () => Math.max(1000, deadline - Date.now());

  for (let i = 0; i < req.actions.length; i++) {
    const action = req.actions[i]!;
    if (Date.now() > deadline) {
      results.push({ index: i, type: action.type, status: 'skipped', message: 'batch timeout reached' });
      continue;
    }
    // Pre-action blocker gate: never let a page-interaction action proceed on a
    // sensitive/blocked page (CAPTCHA/MFA/payment/signature/attestation).
    // Navigation and lifecycle actions are allowed so the agent can recover.
    const INTERACTION = new Set([
      'fill',
      'type',
      'fill_form',
      'click',
      'submit',
      'select',
      'check',
      'uncheck',
      'press',
      'upload',
    ]);
    if (session.lastBlocker && INTERACTION.has(action.type)) {
      results.push({ index: i, type: action.type, status: 'blocked', message: session.lastBlocker.message });
      return { status: 'blocked', results, blocker: session.lastBlocker, pending: null, changed };
    }
    const urlBefore = session.page.url();
    try {
      switch (action.type) {
        case 'start': {
          // start is handled by the caller (session already created); no-op here.
          results.push({ index: i, type: 'start', status: 'completed' });
          break;
        }
        case 'goto': {
          const url = await assertUrlAllowed(action.url, session.allowedDomains, deps.isPrivateIp);
          await session.page.goto(url.toString(), {
            waitUntil: action.wait_until ?? 'domcontentloaded',
            timeout: remaining(),
          });
          changed = true;
          results.push({ index: i, type: 'goto', status: 'completed' });
          break;
        }
        case 'fill': {
          const loc = await resolveTarget(session, action.target);
          await assertResolvedFieldWriteAllowed(loc, action.value);
          if (action.clear_first) await loc.fill('', { timeout: remaining() });
          const value = await fillValue(session, action.value);
          await loc.fill(value, { timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'fill', status: 'completed', resolved_ref: action.target.ref });
          break;
        }
        case 'type': {
          const loc = await resolveTarget(session, action.target);
          await assertResolvedFieldWriteAllowed(loc, action.value);
          if (action.clear_first) await loc.fill('', { timeout: remaining() });
          const value = await fillValue(session, action.value);
          await loc.pressSequentially(value, {
            delay: action.delay_ms ?? 0,
            timeout: remaining(),
          });
          changed = true;
          results.push({ index: i, type: 'type', status: 'completed', resolved_ref: action.target.ref });
          break;
        }
        case 'fill_form': {
          for (const field of action.fields) {
            const loc = await resolveTarget(session, field.target);
            await assertResolvedFieldWriteAllowed(loc, field.value);
            const value = await fillValue(session, field.value);
            await loc.fill(value, { timeout: remaining() });
          }
          changed = true;
          results.push({ index: i, type: 'fill_form', status: 'completed' });
          break;
        }
        case 'click': {
          const loc = await resolveTarget(session, action.target);
          // Consequential-action confirmation gate.
          const labelText = (await loc.innerText().catch(() => '')) || (await loc.getAttribute('value').catch(() => '')) || '';
          const consequential = CONSEQUENTIAL_RE.test(labelText) || CONSEQUENTIAL_RE.test(action.target.name ?? '');
          if (consequential) {
            const digest = actionDigest(session.id, session.revision, session.page.url(), `click:${labelText.trim()}`);
            const confirmed =
              req.confirmation &&
              req.confirmation.action_digest === digest &&
              req.confirmation.confirmed_revision === session.revision &&
              req.confirmation.user_explicitly_approved === true;
            if (!confirmed) {
              const pending: PendingAction = {
                description: `Click "${labelText.trim().slice(0, 120)}"`,
                targetRef: action.target.ref ?? '',
                pageUrl: session.page.url(),
                revision: session.revision,
                actionDigest: digest,
                consequences: ['May submit a form or trigger an irreversible action'],
              };
              session.pending = pending;
              results.push({ index: i, type: 'click', status: 'blocked', message: 'needs confirmation' });
              return { status: 'needs_confirmation', results, blocker: null, pending, changed };
            }
          }
          await loc.click({ button: action.button ?? 'left', timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'click', status: 'completed', resolved_ref: action.target.ref });
          break;
        }
        case 'submit': {
          const loc = await resolveTarget(session, action.target);
          const labelText =
            (await loc.innerText().catch(() => '')) ||
            (await loc.getAttribute('value').catch(() => '')) ||
            'submit';
          const digest = actionDigest(
            session.id,
            session.revision,
            session.page.url(),
            `submit:${labelText.trim()}`,
          );
          const confirmed =
            req.confirmation &&
            req.confirmation.action_digest === digest &&
            req.confirmation.confirmed_revision === session.revision &&
            req.confirmation.user_explicitly_approved === true;
          if (!confirmed) {
            const pending: PendingAction = {
              description: `Submit using "${labelText.trim().slice(0, 120)}"`,
              targetRef: action.target.ref ?? '',
              pageUrl: session.page.url(),
              revision: session.revision,
              actionDigest: digest,
              consequences: ['Will submit a form or trigger a potentially irreversible action'],
            };
            session.pending = pending;
            results.push({ index: i, type: 'submit', status: 'blocked', message: 'needs confirmation' });
            return { status: 'needs_confirmation', results, blocker: null, pending, changed };
          }
          await loc.click({ button: 'left', timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'submit', status: 'completed', resolved_ref: action.target.ref });
          break;
        }
        case 'select': {
          const loc = await resolveTarget(session, action.target);
          const opt = action.option;
          if ('label' in opt) await loc.selectOption({ label: opt.label }, { timeout: remaining() });
          else if ('value' in opt) await loc.selectOption({ value: opt.value }, { timeout: remaining() });
          else await loc.selectOption({ index: opt.index }, { timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'select', status: 'completed' });
          break;
        }
        case 'check': {
          const loc = await resolveTarget(session, action.target);
          await loc.check({ timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'check', status: 'completed' });
          break;
        }
        case 'uncheck': {
          const loc = await resolveTarget(session, action.target);
          await loc.uncheck({ timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'uncheck', status: 'completed' });
          break;
        }
        case 'press': {
          if (action.target) {
            const loc = await resolveTarget(session, action.target);
            await loc.press(action.key, { timeout: remaining() });
          } else {
            await session.page.keyboard.press(action.key);
          }
          changed = true;
          results.push({ index: i, type: 'press', status: 'completed' });
          break;
        }
        case 'upload': {
          const loc = await resolveTarget(session, action.target);
          const file =
            action.file_token !== undefined
              ? await resolveUploadToken(action.file_token)
              : resolveInlineUpload(action.inline_file!);
          await loc.setInputFiles(file, { timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'upload', status: 'completed', resolved_ref: action.target.ref });
          break;
        }
        case 'scroll': {
          if (action.target) {
            const loc = await resolveTarget(session, action.target);
            await loc.scrollIntoViewIfNeeded({ timeout: remaining() });
          } else {
            await session.page.mouse.wheel(action.delta_x ?? 0, action.delta_y ?? 600);
          }
          changed = true;
          results.push({ index: i, type: 'scroll', status: 'completed', resolved_ref: action.target?.ref });
          break;
        }
        case 'screenshot': {
          session.lastActionScreenshot = await captureScreenshot(session);
          results.push({
            index: i,
            type: 'screenshot',
            status: session.lastActionScreenshot ? 'completed' : 'failed',
            message: session.lastActionScreenshot ? 'viewport screenshot captured' : 'screenshot capture failed',
          });
          break;
        }
        case 'extract': {
          await takeSnapshot(
            session,
            agentConfig.maxElements,
            action.max_visible_text_chars ?? agentConfig.maxVisibleTextChars,
          );
          session.lastExtraction = projectObserve(
            session,
            {
              session_id: session.id,
              include:
                action.include ??
                [
                  'summary',
                  'visible_text',
                  'interactive_elements',
                  'accessibility_snapshot',
                  'forms',
                  'validation_errors',
                  'downloads',
                ],
              max_visible_text_chars: action.max_visible_text_chars,
            },
            false,
          );
          results.push({ index: i, type: 'extract', status: 'completed' });
          break;
        }
        case 'wait': {
          await runWait(session, action.condition, remaining());
          results.push({ index: i, type: 'wait', status: 'completed' });
          break;
        }
        case 'back': {
          await session.page.goBack({ timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'back', status: 'completed' });
          break;
        }
        case 'forward': {
          await session.page.goForward({ timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'forward', status: 'completed' });
          break;
        }
        case 'reload': {
          await session.page.reload({ timeout: remaining() });
          changed = true;
          results.push({ index: i, type: 'reload', status: 'completed' });
          break;
        }
        case 'close': {
          results.push({ index: i, type: 'close', status: 'completed' });
          await destroySession(session, 'closed_by_action');
          return { status: 'completed', results, blocker: null, pending: null, changed: true };
        }
      }
      // Early-exit when a stop_when clause is satisfied: skip remaining actions.
      if (req.stop_when && (await stopConditionMet(session, req.stop_when, urlBefore))) {
        for (let j = i + 1; j < req.actions.length; j++) {
          results.push({ index: j, type: req.actions[j]!.type, status: 'skipped', message: 'stop_when satisfied' });
        }
        return { status: 'completed', results, blocker: null, pending: null, changed };
      }
    } catch (e) {
      if (
        e instanceof AgentError &&
        (e.code === 'ambiguous_target' ||
          e.code === 'target_not_found' ||
          e.code === 'domain_not_allowed' ||
          e.code === 'sensitive_field_blocked')
      ) {
        const type: BlockerType =
          e.code === 'ambiguous_target'
            ? 'ambiguous_target'
            : e.code === 'domain_not_allowed'
              ? 'domain_not_allowed'
              : e.code === 'sensitive_field_blocked'
                ? 'sensitive_field_blocked'
                : 'other';
        const blocker: { type: BlockerType; message: string } = { type, message: e.message };
        results.push({ index: i, type: action.type, status: 'blocked', message: e.message });
        return { status: 'blocked', results, blocker, pending: null, changed };
      }
      const message = e instanceof Error ? e.message.slice(0, 200) : 'action failed';
      results.push({ index: i, type: action.type, status: 'failed', message });
      const failStatus = results.some((r) => r.status === 'completed') ? 'partially_completed' : 'failed';
      return { status: failStatus, results, blocker: null, pending: null, changed };
    }
  }
  return { status: 'completed', results, blocker: null, pending: null, changed };
}

// Evaluate a stop_when clause after an action. Returns true when the batch
// should halt early (remaining actions skipped). All probes are immediate
// (no waiting) so a stop check never blocks the action batch.
async function stopConditionMet(
  session: Session,
  sw: NonNullable<ActRequest['stop_when']>,
  urlBefore: string,
): Promise<boolean> {
  const url = session.page.url();
  if (sw.navigation_occurs && url !== urlBefore) return true;
  if (sw.url_matches && new RegExp(escapeRegex(sw.url_matches)).test(url)) return true;
  if (sw.text_visible) {
    try {
      if (await session.page.getByText(sw.text_visible).first().isVisible()) return true;
    } catch {
      /* not present yet */
    }
  }
  if (sw.element_visible) {
    try {
      const loc = await resolveTarget(session, sw.element_visible);
      if (await loc.isVisible()) return true;
    } catch {
      /* not resolvable yet */
    }
  }
  if (sw.validation_error_occurs) {
    try {
      const hasError = await session.page.evaluate(
        '(function(){var els=document.querySelectorAll("input,select,textarea");for(var i=0;i<els.length;i++){var e=els[i];if(e.willValidate&&!e.validity.valid&&e.validationMessage)return true;}return false;})()',
      );
      if (hasError === true) return true;
    } catch {
      /* ignore */
    }
  }
  return false;
}

async function runWait(
  session: Session,
  condition: z.infer<typeof WaitConditionSchema>,
  timeout: number,
): Promise<void> {
  if ('duration_ms' in condition) {
    await session.page.waitForTimeout(Math.min(condition.duration_ms, timeout));
  } else if ('url_matches' in condition) {
    await session.page.waitForURL(new RegExp(escapeRegex(condition.url_matches)), { timeout });
  } else if ('text_visible' in condition) {
    await session.page.getByText(condition.text_visible).first().waitFor({ state: 'visible', timeout });
  } else if ('element_visible' in condition) {
    const loc = await resolveTarget(session, condition.element_visible);
    await loc.waitFor({ state: 'visible', timeout });
  } else {
    await session.page.waitForLoadState(condition.load_state, { timeout });
  }
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// ---------------------------------------------------------------------------
// Response builders
// ---------------------------------------------------------------------------

function buildActResponse(
  session: Session | null,
  req: ActRequest,
  outcome: Awaited<ReturnType<typeof runActions>>,
): Record<string, unknown> {
  const closed = session === null || !sessions.has(session.id);
  const revision = session ? session.revision : 0;
  return {
    request_id: req.request_id,
    session_id: session?.id ?? req.session_id ?? '',
    revision,
    status: outcome.status,
    page: session && !closed ? pageInfo(session) : { url: '', origin: '', title: '', load_state: 'closed' },
    action_results: outcome.results,
    ...(outcome.blocker ? { blocker: outcome.blocker } : {}),
    ...(outcome.pending
      ? {
          pending_action: {
            description: outcome.pending.description,
            target_ref: outcome.pending.targetRef,
            page_url: outcome.pending.pageUrl,
            revision: outcome.pending.revision,
            action_digest: outcome.pending.actionDigest,
            consequences: outcome.pending.consequences,
          },
        }
      : {}),
    ...(session?.lastActionScreenshot ? { screenshot: session.lastActionScreenshot } : {}),
    ...(session?.lastExtraction ? { extracted: session.lastExtraction } : {}),
    changed: outcome.changed,
    summary: buildSummary(session, outcome),
  };
}

function buildSummary(session: Session | null, outcome: Awaited<ReturnType<typeof runActions>>): string {
  if (outcome.status === 'needs_confirmation') return 'Consequential action requires explicit confirmation.';
  if (outcome.blocker) return `Blocked: ${outcome.blocker.message}`;
  const done = outcome.results.filter((r) => r.status === 'completed').length;
  // Page title is untrusted (attacker-controlled) and is surfaced via the MCP
  // text content block, so it is deliberately NOT interpolated here. It remains
  // available in the structured page.title field.
  void session;
  return `${done} action(s) completed.`;
}

function projectObserve(session: Session, req: ObserveRequest, timedOut: boolean, previousRevision?: number): Record<string, unknown> {
  const include = new Set(
    req.include ?? [
      'summary',
      'visible_text',
      'interactive_elements',
      'accessibility_snapshot',
      'forms',
      'validation_errors',
      'downloads',
    ],
  );
  const snap = session.lastSnapshot;
  const maxElements = req.max_elements ?? 200;
  const strict = (req.redaction ?? 'standard') === 'strict';
  const info = pageInfo(session);

  const out: Record<string, unknown> = {
    session_id: session.id,
    revision: session.revision,
    ...(previousRevision !== undefined ? { previous_revision: previousRevision } : {}),
    changed: previousRevision !== undefined ? session.revision !== previousRevision : true,
    timed_out: timedOut,
    page: { ...info, secure_context: info.origin.startsWith('https:') },
    summary: snap ? summarizeSnapshot(snap, session) : 'No page loaded yet.',
  };

  if (snap && (include.has('interactive_elements') || include.has('accessibility_snapshot'))) {
    const projectedElements = snap.elements.slice(0, maxElements).map((el) => {
      const ref = session.refByFp.get(el.fp) ?? '';
      const valueState = el.sensitive ? 'redacted' : el.hasValue ? 'present' : 'empty';
      const base: Record<string, unknown> = {
        ref,
        role: el.role,
        ...(el.name ? { name: el.name } : {}),
        ...(el.label ? { label: el.label } : {}),
        ...(el.type ? { type: el.type } : {}),
        ...(el.required ? { required: true } : {}),
        ...(el.disabled ? { disabled: true } : {}),
        ...(el.readOnly ? { read_only: true } : {}),
        ...(el.role === 'checkbox' || el.role === 'radio' ? { checked: el.checked } : {}),
        value_state: valueState,
      };
      if (el.selectedOption && !el.sensitive) base.selected_option = el.selectedOption;
      if (el.options.length && !strict) base.options = el.options.slice(0, 100).map((o) => ({
        label: o.label,
        ...(o.value ? { value: o.value } : {}),
        ...(o.selected ? { selected: true } : {}),
        ...(o.disabled ? { disabled: true } : {}),
      }));
      if (el.validationMessage) base.validation_message = el.validationMessage;
      return base;
    });
    if (include.has('interactive_elements')) {
      out.interactive_elements = projectedElements;
      out.fields = projectedElements.filter((el) =>
        ['textbox', 'combobox', 'checkbox', 'radio', 'slider'].includes(String(el.role)),
      );
      out.buttons = projectedElements.filter((el) => el.role === 'button');
      out.links = projectedElements.filter((el) => el.role === 'link');
    }
    if (include.has('accessibility_snapshot')) {
      out.accessibility_snapshot = {
        format: 'dd-accessibility-v1',
        role: 'document',
        name: snap.title.slice(0, 300),
        children: projectedElements.map((el) => ({
          ref: el.ref,
          role: el.role,
          ...(el.name ? { name: el.name } : {}),
          ...(el.required ? { required: true } : {}),
          ...(el.disabled ? { disabled: true } : {}),
          ...(el.checked !== undefined ? { checked: el.checked } : {}),
          value_state: el.value_state,
        })),
      };
    }
  }
  if (snap && include.has('forms')) {
    out.forms = snap.forms.map((f) => ({
      ref: session.refByFp.get(f.fp) ?? '',
      ...(f.name ? { name: f.name } : {}),
      method: f.method,
      ...(f.actionOrigin ? { action_origin: f.actionOrigin } : {}),
      field_refs: f.fieldFps.map((fp) => session.refByFp.get(fp) ?? '').filter(Boolean),
      submit_refs: f.submitFps.map((fp) => session.refByFp.get(fp) ?? '').filter(Boolean),
    }));
  }
  if (snap && include.has('validation_errors')) {
    out.validation_errors = snap.elements
      .filter((el) => el.validationMessage)
      .map((el) => ({ ref: session.refByFp.get(el.fp) ?? '', message: el.validationMessage }));
  }
  if (include.has('dialogs')) out.dialogs = session.dialogs;
  if (include.has('frames')) {
    out.frames = session.page
      .frames()
      .filter((fr) => fr !== session.page.mainFrame())
      .map((fr) => {
        let origin = '';
        try {
          origin = new URL(fr.url()).origin;
        } catch {
          origin = '';
        }
        return {
          ref: session.frameRefByUrl.get(fr.url()) ?? '',
          url: fr.url(),
          origin,
          accessible: origin === info.origin,
        };
      });
  }
  if (include.has('downloads')) out.downloads = session.downloads;
  if (include.has('network_failures')) out.network_failures = session.networkFailures.slice(-20);
  if (session.lastBlocker) out.blocker = session.lastBlocker;
  if (snap && include.has('visible_text')) {
    const limit = req.max_visible_text_chars ?? 12_000;
    const text = snap.visibleText.slice(0, limit);
    out.visible_text = { untrusted_content: text, truncated: snap.visibleText.length > text.length };
  }
  return out;
}

// Capture a bounded JPEG screenshot of the current viewport. Returned inline as
// base64 (the MCP gateway can re-wrap it as image content). Never full-page and
// never a public URL. Fails soft: returns null so observe still succeeds.
async function captureScreenshot(
  session: Session,
): Promise<ScreenshotRecord | null> {
  try {
    const buffer = await session.page.screenshot({ type: 'jpeg', quality: 55, fullPage: false, timeout: 8_000 });
    const vp = session.page.viewportSize() ?? { width: 0, height: 0 };
    return {
      mime_type: 'image/jpeg',
      width: vp.width,
      height: vp.height,
      data_base64: Buffer.from(buffer).toString('base64'),
    };
  } catch {
    return null;
  }
}

// Build the observe response and attach a screenshot when requested. Split from
// projectObserve (which stays sync) because screenshot capture is async.
async function finishObserve(
  session: Session,
  req: ObserveRequest,
  timedOut: boolean,
  previousRevision?: number,
): Promise<Record<string, unknown>> {
  const out = projectObserve(session, req, timedOut, previousRevision);
  if ((req.include ?? []).includes('screenshot')) {
    const shot = await captureScreenshot(session);
    if (shot) out.screenshot = shot;
  }
  return out;
}

function summarizeSnapshot(snap: RawSnapshot, session: Session): string {
  const blocker = session.lastBlocker ? ` Blocker: ${session.lastBlocker.type}.` : '';
  const forms = snap.forms.length;
  const controls = snap.elements.length;
  // Untrusted page title is NOT interpolated into this summary (it surfaces via
  // the MCP text content block). It stays in the structured page.title field.
  return `Page with ${controls} interactive control(s) and ${forms} form(s).${blocker}`;
}

// ---------------------------------------------------------------------------
// Route registration
// ---------------------------------------------------------------------------

export function registerBrowserAgentRoutes(
  fastify: FastifyInstance,
  deps: AgentDeps & { isAuthorized: (headers: Record<string, string | string[] | undefined>) => boolean },
): void {
  if (!agentConfig.enabled) {
    deps.log.info('browser-agent routes disabled (BROWSER_AGENT_ENABLED=false)');
    return;
  }
  if (agentConfig.secretsFile) {
    // Secrets typed into pages are only as safe as the domains they're bound to.
    // Never provision a secrets file on a publicly reachable, unauthenticated
    // worker unless every secret entry carries a `domains` binding.
    deps.log.warn(
      'browser-agent: BROWSER_AGENT_SECRETS_FILE is set; ensure every secret is domain-bound and the endpoint is authenticated',
    );
  }
  startSweeper(deps.log);

  // All /agent/* routes require the shared server-auth secret.
  fastify.addHook('onRequest', async (request, reply) => {
    const path = request.url.split('?')[0] ?? request.url;
    if (!path.startsWith('/agent/')) return;
    if (path === '/agent/healthz') return;
    if (deps.isAuthorized(request.headers)) return;
    return reply.code(401).send({ ok: false, error: 'unauthorized' });
  });

  fastify.get('/agent/healthz', async () => ({
    ok: true,
    sessions: sessions.size,
    maxSessions: agentConfig.maxSessionsTotal,
    uploadsEnabled: true,
    inlineUploadsEnabled: true,
    tokenUploadsEnabled: Boolean(agentConfig.uploadsDir),
    maxInlineUploadBytes: MAX_INLINE_UPLOAD_BYTES,
  }));

  fastify.get('/agent/tools', async () => ({
    tools: ['browser_act', 'browser_state'],
    engines: ['chromium', 'firefox', 'webkit'],
    uploadModes: agentConfig.uploadsDir ? ['inline', 'file_token'] : ['inline'],
    maxInlineUploadBytes: MAX_INLINE_UPLOAD_BYTES,
    allowlistEnforced: agentConfig.allowedDomains.length > 0,
    allowedDomains: agentConfig.allowedDomains,
  }));

  fastify.post('/agent/act', async (request, reply) => {
    const parsed = ActRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error_code: 'invalid_request', error: parsed.error.format() });
    }
    const req = parsed.data;
    const owner = req.owner ?? 'anonymous';

    let session: Session | null = null;
    try {
      const startAction = req.actions.find((a) => a.type === 'start') as
        | Extract<BrowserAction, { type: 'start' }>
        | undefined;

      if (!req.session_id) {
        if (!startAction) {
          throw new AgentError('invalid_request', 'a request without session_id must include a start action');
        }
        const allow = effectiveAllowedDomains(req.allowed_domains, agentConfig.allowedDomains);
        if (agentConfig.allowedDomains.length > 0 && allow.length === 0) {
          throw new AgentError(
            'domain_not_allowed',
            'requested domains do not intersect the server allowlist',
          );
        }
        session = await createSession(deps, owner, startAction, allow);
        if (startAction.initial_url) {
          const url = await assertUrlAllowed(startAction.initial_url, session.allowedDomains, deps.isPrivateIp);
          await session.page.goto(url.toString(), { waitUntil: 'domcontentloaded', timeout: req.timeout_ms ?? agentConfig.defaultTimeoutMs });
        }
      } else {
        session = getOwnedSession(req.session_id, owner);
        if (session.busy) throw new AgentError('session_busy', 'a mutating action is already running for this session');
        // Idempotency: replay stored result for identical request_id.
        const idemKey = req.request_id;
        if (session.idempotency.has(idemKey)) {
          return session.idempotency.get(idemKey) as Record<string, unknown>;
        }
        // Optimistic concurrency.
        if (req.expected_revision !== undefined && req.expected_revision !== session.revision) {
          return {
            request_id: req.request_id,
            session_id: session.id,
            revision: session.revision,
            status: 'revision_conflict',
            page: pageInfo(session),
            action_results: [],
            changed: false,
            summary: `Stale expected_revision ${req.expected_revision}; current is ${session.revision}. Re-observe.`,
          };
        }
      }

      session.busy = true;
      const timeoutMs = req.timeout_ms ?? agentConfig.defaultTimeoutMs;
      // Refresh snapshot so pre-action blocker gate and refs are current.
      if (sessions.has(session.id)) await takeSnapshot(session, agentConfig.maxElements, 4000);
      const outcome = await runActions(session, deps, req, timeoutMs);
      const stillOpen = sessions.has(session.id);
      if (stillOpen && outcome.changed) {
        await takeSnapshot(session, agentConfig.maxElements, 4000);
        bumpRevision(session);
      }
      const response = buildActResponse(stillOpen ? session : null, req, outcome);
      const ownerHash = auditHash(owner);
      const sessionHash = auditHash(session.id);
      for (const result of outcome.results) {
        const action = req.actions[result.index];
        deps.log.info(
          {
            event: 'browser_action',
            requestId: req.request_id,
            ownerHash,
            sessionHash,
            actionIndex: result.index,
            actionType: result.type,
            status: result.status,
            destinationHost: action ? actionDestinationHost(action) : undefined,
            resolvedRef: result.resolved_ref,
          },
          'browser-agent audit',
        );
      }
      if (stillOpen) {
        session.busy = false;
        session.idempotency.set(req.request_id, response);
        if (session.idempotency.size > 100) {
          const firstKey = session.idempotency.keys().next().value;
          if (firstKey !== undefined) session.idempotency.delete(firstKey);
        }
      }
      return response;
    } catch (e) {
      if (session && sessions.has(session.id)) session.busy = false;
      return replyError(reply, e, deps.log);
    }
  });

  fastify.post('/agent/observe', async (request, reply) => {
    const parsed = ObserveRequestSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error_code: 'invalid_request', error: parsed.error.format() });
    }
    const req = parsed.data;
    const owner = req.owner ?? 'anonymous';
    try {
      const session = getOwnedSession(req.session_id, owner);
      const previousRevision = req.since_revision;

      // Immediate return path.
      if (previousRevision === undefined || session.revision !== previousRevision || !req.wait_ms) {
        await takeSnapshot(session, req.max_elements ?? agentConfig.maxElements, req.max_visible_text_chars ?? agentConfig.maxVisibleTextChars);
        const response = await finishObserve(session, req, false, previousRevision);
        deps.log.info(
          {
            event: 'browser_state',
            ownerHash: auditHash(owner),
            sessionHash: auditHash(session.id),
            revision: session.revision,
          },
          'browser-agent audit',
        );
        return response;
      }

      // Bound concurrent long-polls per session so a caller cannot pin an
      // unbounded number of waiters (and upstream connections) on one session.
      if (session.waiters.length >= MAX_WAITERS_PER_SESSION) {
        return finishObserve(session, req, true, previousRevision);
      }
      // Long-poll: wait for a revision change or timeout. Never holds a browser
      // lock; a concurrent /agent/act can still run.
      const waitMs = Math.min(req.wait_ms, agentConfig.maxLongPollMs);
      const changed = await waitForRevision(session, previousRevision, waitMs, request);
      const sessionStill = sessions.get(req.session_id);
      if (!sessionStill) throw new AgentError('session_expired', 'session ended while waiting');
      if (changed) {
        await takeSnapshot(sessionStill, req.max_elements ?? agentConfig.maxElements, req.max_visible_text_chars ?? agentConfig.maxVisibleTextChars);
      }
      const response = await finishObserve(sessionStill, req, !changed, previousRevision);
      deps.log.info(
        {
          event: 'browser_state',
          ownerHash: auditHash(owner),
          sessionHash: auditHash(sessionStill.id),
          revision: sessionStill.revision,
          longPoll: true,
          changed,
        },
        'browser-agent audit',
      );
      return response;
    } catch (e) {
      return replyError(reply, e, deps.log);
    }
  });

  // One-shot Selenium bridge (the "wrapper" over dd-selenium-server). Playwright
  // backs the stateful browser_act/browser_state loop; this endpoint forwards a
  // native, bounded Selenium scenario (CSS-selector DSL) to the existing
  // dd-selenium-server /run in a single call. Internal-only, same auth gate.
  const SeleniumRunSchema = z
    .object({
      scenario: z.object({}).passthrough(),
      timeout_ms: z.number().int().min(1000).max(120_000).optional(),
    })
    .strict();
  fastify.post('/agent/selenium-run', async (request, reply) => {
    if (!agentConfig.seleniumUrl) {
      return reply.code(503).send({ error_code: 'worker_unavailable', error: 'selenium bridge is not configured' });
    }
    const parsed = SeleniumRunSchema.safeParse(request.body);
    if (!parsed.success) {
      return reply.code(400).send({ error_code: 'invalid_request', error: parsed.error.format() });
    }
    try {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), (parsed.data.timeout_ms ?? 60_000) + 5_000);
      const resp = await fetch(`${agentConfig.seleniumUrl.replace(/\/$/, '')}/run`, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          ...(agentConfig.seleniumAuthSecret ? { 'x-server-auth': agentConfig.seleniumAuthSecret } : {}),
        },
        body: JSON.stringify(parsed.data.scenario),
        signal: controller.signal,
      }).finally(() => clearTimeout(timer));
      const text = await resp.text();
      let body: unknown;
      try {
        body = JSON.parse(text);
      } catch {
        body = { raw: text.slice(0, 4000) };
      }
      return reply.code(resp.ok ? 200 : 502).send(body);
    } catch (e) {
      deps.log.error({ err: e }, 'browser-agent: selenium bridge error');
      return reply.code(502).send({ error_code: 'worker_unavailable', error: 'selenium bridge request failed' });
    }
  });

  fastify.post('/agent/close', async (request, reply) => {
    const body = z.object({ session_id: z.string().min(1).max(200), owner: z.string().max(200).optional() }).safeParse(request.body);
    if (!body.success) return reply.code(400).send({ error_code: 'invalid_request' });
    try {
      const session = getOwnedSession(body.data.session_id, body.data.owner ?? 'anonymous');
      await destroySession(session, 'closed_by_request');
      return { ok: true, session_id: body.data.session_id };
    } catch (e) {
      return replyError(reply, e, deps.log);
    }
  });
}

function waitForRevision(
  session: Session,
  since: number,
  waitMs: number,
  request: { raw: { on: (event: string, cb: () => void) => void } },
): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const done = (changed: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(changed);
    };
    const timer = setTimeout(() => done(false), waitMs);
    session.waiters.push((rev) => done(rev !== since));
    // Client disconnect cancels the wait.
    request.raw.on('close', () => done(session.revision !== since));
  });
}

// Pure, browser-free surface exposed for unit tests (see tests/browser-agent.test.ts).
export const __test = {
  ActRequestSchema,
  ObserveRequestSchema,
  domainAllowed,
  requestUrlAllowedByDomain,
  effectiveAllowedDomains,
  validAllowedDomain,
  actionDigest,
  assertUrlAllowed,
  CONSEQUENTIAL_RE,
  BLOCKED_SCHEMES,
};

function replyError(
  reply: { code: (n: number) => { send: (body: unknown) => unknown } },
  e: unknown,
  log: FastifyBaseLogger,
): unknown {
  if (e instanceof AgentError) {
    const statusByCode: Partial<Record<ErrorCode, number>> = {
      invalid_request: 400,
      session_not_found: 404,
      session_expired: 410,
      session_busy: 409,
      revision_conflict: 409,
      idempotency_conflict: 409,
      too_many_sessions: 429,
      domain_not_allowed: 403,
      target_not_found: 422,
      ambiguous_target: 422,
      secret_required: 422,
      upload_not_supported: 422,
      unsafe_download: 422,
      action_timeout: 504,
      navigation_failed: 502,
      browser_crashed: 502,
      internal_error: 500,
    };
    return reply.code(statusByCode[e.code] ?? 500).send({ error_code: e.code, error: e.message });
  }
  log.error({ err: e }, 'browser-agent: unhandled error');
  return reply.code(500).send({ error_code: 'internal_error', error: 'internal error' });
}
