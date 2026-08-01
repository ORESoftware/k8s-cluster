import { createHash } from 'node:crypto';

const SECRET_PATTERNS = [
  {
    kind: 'aws_access_key_id',
    pattern: /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/g,
  },
  {
    kind: 'aws_secret_access_key',
    pattern: /\b(?:aws_)?secret(?:_?access)?_?key\b["']?\s*[=:]\s*["']?([A-Za-z0-9/+=]{32,})["']?/gi,
    capture: 1,
  },
  {
    kind: 'github_token',
    pattern: /\b(?:github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{20,})\b/g,
  },
  {
    kind: 'linear_api_key',
    pattern: /\blin_api_[A-Za-z0-9_-]{16,}\b/g,
  },
  {
    kind: 'google_chat_bridge_token',
    pattern: /\bCHAT_BRIDGE_TOKEN\b\s*[=:]\s*["']?([^\s"']{24,})["']?/gi,
    capture: 1,
  },
  {
    kind: 'slack_token',
    pattern: /\bxox[baprs]-[A-Za-z0-9-]{20,}\b/g,
  },
  {
    kind: 'google_api_key',
    pattern: /\bAIza[0-9A-Za-z_-]{30,}\b/g,
  },
  {
    kind: 'bearer_token',
    pattern: /\bAuthorization\s*:\s*Bearer\s+([A-Za-z0-9._~+/=-]{20,})/gi,
    capture: 1,
  },
  {
    kind: 'assigned_secret',
    pattern:
      /\b(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|BEARER_TOKEN|CLIENT_SECRET|PRIVATE_TOKEN|REFRESH_TOKEN|PASSWORD)\b\s*[=:]\s*["']?([^\s"']{16,})["']?/gi,
    capture: 1,
  },
  {
    kind: 'private_key',
    pattern:
      /-----BEGIN (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----/g,
  },
];

const CONTACT_ONLY_PATTERN = /^\s*(?:\+?\d[\d\s().-]{6,}\d)(?:\s+[\p{L}\p{M}][\p{L}\p{M}\s_.-]{0,80})?\s*$/u;
const NAMED_CONTACT_PATTERN = /^\s*[\p{L}\p{M}][\p{L}\p{M}\s_.-]{0,80}\s+(?:\+?\d[\d\s().-]{6,}\d)\s*$/u;

function fingerprint(value) {
  return createHash('sha256').update(value).digest('hex').slice(0, 16);
}

function clonePattern(pattern) {
  return new RegExp(pattern.source, pattern.flags);
}

export function scanSensitiveContent(value) {
  const text = String(value ?? '');
  const findings = [];

  for (const definition of SECRET_PATTERNS) {
    const pattern = clonePattern(definition.pattern);
    let match;
    while ((match = pattern.exec(text)) !== null) {
      const matchedValue = definition.capture ? match[definition.capture] : match[0];
      const valueOffset = definition.capture ? match[0].indexOf(matchedValue) : 0;
      const start = match.index + Math.max(0, valueOffset);
      const end = start + matchedValue.length;
      findings.push({
        kind: definition.kind,
        start,
        end,
        fingerprint: fingerprint(matchedValue),
      });
      if (match[0].length === 0) pattern.lastIndex += 1;
    }
  }

  findings.sort((left, right) => left.start - right.start || right.end - left.end);
  const deduplicated = [];
  for (const finding of findings) {
    const previous = deduplicated.at(-1);
    if (
      previous &&
      previous.start === finding.start &&
      previous.end === finding.end &&
      previous.kind === finding.kind
    ) {
      continue;
    }
    deduplicated.push(finding);
  }
  return deduplicated;
}

export function redactSensitiveContent(value, findings = scanSensitiveContent(value)) {
  const text = String(value ?? '');
  if (findings.length === 0) return text;

  const byStart = [...findings].sort((left, right) => right.start - left.start || left.end - right.end);
  let redacted = text;
  let coveredStart = Infinity;
  for (const finding of byStart) {
    if (finding.end > coveredStart) continue;
    redacted = `${redacted.slice(0, finding.start)}[REDACTED:${finding.kind}]${redacted.slice(finding.end)}`;
    coveredStart = finding.start;
  }
  return redacted;
}

export function isHighConfidenceContactOnly(value) {
  const text = String(value ?? '').trim();
  if (!text || text.length > 140 || /https?:\/\//i.test(text)) return false;
  if (/^(?:\d{1,3}\.){3}\d{1,3}$/.test(text)) return false;
  return CONTACT_ONLY_PATTERN.test(text) || NAMED_CONTACT_PATTERN.test(text);
}

export function classifySensitiveMessage(message) {
  const fields = ['text', 'formattedText', 'argumentText', 'fallbackText'];
  const findings = [];
  for (const field of fields) {
    const value = message?.[field];
    if (!value) continue;
    for (const finding of scanSensitiveContent(value)) {
      findings.push({ field, ...finding });
    }
  }

  if (findings.length > 0) {
    return {
      classification: 'sensitive-secret',
      findings,
      kinds: [...new Set(findings.map((finding) => finding.kind))].sort(),
    };
  }

  const primaryText =
    message?.text || message?.formattedText || message?.argumentText || message?.fallbackText || '';
  if (isHighConfidenceContactOnly(primaryText)) {
    return { classification: 'private-contact', findings: [], kinds: [] };
  }

  return { classification: 'safe', findings: [], kinds: [] };
}

export function quarantineMessage(message, classification) {
  if (!classification || classification.classification === 'safe') return structuredClone(message);

  const output = structuredClone(message);
  for (const field of ['text', 'formattedText', 'argumentText', 'fallbackText']) {
    if (field in output) output[field] = '';
  }
  output.attachments = [];
  output.attachedGifs = [];
  output.annotations = [];
  output.safety = {
    classification: classification.classification,
    kinds: classification.kinds,
    quarantined: true,
  };
  return output;
}
