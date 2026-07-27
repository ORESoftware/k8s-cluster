export type SensitiveFieldKind =
  | 'ordinary'
  | 'credential'
  | 'government_identifier'
  | 'banking'
  | 'payment_card'
  | 'mfa';

export interface FieldMetadata {
  tagName?: string | null;
  type?: string | null;
  name?: string | null;
  id?: string | null;
  autocomplete?: string | null;
  ariaLabel?: string | null;
  label?: string | null;
  placeholder?: string | null;
  title?: string | null;
}

export type FieldWriteSource = 'literal' | 'secret_ref';

export interface FieldWriteDecision {
  allowed: boolean;
  kind: SensitiveFieldKind;
  reason: string;
}

export class SensitiveFieldPolicyError extends Error {
  readonly code = 'sensitive_field_blocked' as const;
  readonly kind: SensitiveFieldKind;

  constructor(kind: SensitiveFieldKind, message: string) {
    super(message);
    this.name = 'SensitiveFieldPolicyError';
    this.kind = kind;
  }
}

const NORMALIZE_RE = /[^a-z0-9]+/g;

function normalize(value: string | null | undefined): string {
  return (value ?? '').toLowerCase().replace(NORMALIZE_RE, ' ').trim();
}

function fieldText(field: FieldMetadata): string {
  return [
    field.tagName,
    field.type,
    field.name,
    field.id,
    field.autocomplete,
    field.ariaLabel,
    field.label,
    field.placeholder,
    field.title,
  ]
    .map(normalize)
    .filter(Boolean)
    .join(' | ');
}

function any(text: string, patterns: RegExp[]): boolean {
  return patterns.some((pattern) => pattern.test(text));
}

const MFA_PATTERNS = [
  /\bone time (?:code|password)\b/,
  /\bverification code\b/,
  /\bauthenticator code\b/,
  /\bsecurity code\b/,
  /\b(?:otp|totp|mfa|2fa)\b/,
  /\b(?:pin|passcode)\b/,
];

const GOVERNMENT_IDENTIFIER_PATTERNS = [
  /\bsocial security(?: number)?\b/,
  /\bssn\b/,
  /\btax(?:payer)? (?:id|identification)(?: number)?\b/,
  /\b(?:ein|itin|tin)\b/,
  /\bnational id(?:entification)?(?: number)?\b/,
];

const BANKING_PATTERNS = [
  /\brouting(?: number)?\b/,
  /\bbank account(?: number)?\b/,
  /\bchecking account(?: number)?\b/,
  /\bsavings account(?: number)?\b/,
  /\b(?:iban|swift|bic)\b/,
];

const PAYMENT_CARD_PATTERNS = [
  /\bcredit card(?: number)?\b/,
  /\bdebit card(?: number)?\b/,
  /\bcard number\b/,
  /\bcardholder\b/,
  /\bexpiration date\b/,
  /\bexpiry date\b/,
  /\b(?:cvv|cvc|ccv|cc number|cc exp|cc csc|ccnumber)\b/,
];

const CREDENTIAL_PATTERNS = [
  /\bpassword\b/,
  /\bclient secret\b/,
  /\bapi key\b/,
  /\baccess token\b/,
  /\brefresh token\b/,
  /\bprivate key\b/,
  /\bsecret key\b/,
  /\blogin secret\b/,
];

export function classifySensitiveField(field: FieldMetadata): SensitiveFieldKind {
  const text = fieldText(field);
  const autocomplete = normalize(field.autocomplete);
  const type = normalize(field.type);

  if (autocomplete === 'one time code' || any(text, MFA_PATTERNS)) return 'mfa';
  if (any(text, GOVERNMENT_IDENTIFIER_PATTERNS)) return 'government_identifier';
  if (any(text, BANKING_PATTERNS)) return 'banking';
  if (
    autocomplete.startsWith('cc ') ||
    ['cc-number', 'cc-exp', 'cc-csc'].includes((field.autocomplete ?? '').toLowerCase()) ||
    any(text, PAYMENT_CARD_PATTERNS)
  ) {
    return 'payment_card';
  }
  if (type === 'password' || any(text, CREDENTIAL_PATTERNS)) return 'credential';
  return 'ordinary';
}

export function decideSensitiveFieldWrite(
  field: FieldMetadata,
  source: FieldWriteSource,
): FieldWriteDecision {
  const kind = classifySensitiveField(field);

  if (kind === 'ordinary') {
    return { allowed: true, kind, reason: 'ordinary field' };
  }

  if (kind === 'credential' && source === 'secret_ref') {
    return {
      allowed: true,
      kind,
      reason: 'credential write uses an out-of-band, domain-bound secret reference',
    };
  }

  const reason =
    kind === 'credential'
      ? 'literal credential writes are disabled; use a domain-bound secret_ref'
      : `${kind.replaceAll('_', ' ')} fields require human completion and cannot be written by the browser agent`;

  return { allowed: false, kind, reason };
}

export function assertSensitiveFieldWriteAllowed(
  field: FieldMetadata,
  source: FieldWriteSource,
): SensitiveFieldKind {
  const decision = decideSensitiveFieldWrite(field, source);
  if (!decision.allowed) {
    throw new SensitiveFieldPolicyError(decision.kind, decision.reason);
  }
  return decision.kind;
}
