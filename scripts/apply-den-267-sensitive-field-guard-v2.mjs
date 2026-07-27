#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';

const agentPath = 'remote/deployments/web-scraper-service/src/browser-agent.ts';
const readmePath = 'remote/deployments/browser-mcp-rs/readme.md';

function replaceOnce(source, before, after, label) {
  if (source.includes(after)) return source;
  const first = source.indexOf(before);
  if (first < 0) throw new Error(`DEN-267 patch marker not found: ${label}`);
  if (source.indexOf(before, first + before.length) >= 0) {
    throw new Error(`DEN-267 patch marker is ambiguous: ${label}`);
  }
  return source.slice(0, first) + after + source.slice(first + before.length);
}

function insertInCase(source, caseName, needle, insertion, label) {
  const marker = `case '${caseName}': {`;
  const start = source.indexOf(marker);
  if (start < 0) throw new Error(`DEN-267 case marker not found: ${caseName}`);
  const nextCase = source.indexOf("\n        case '", start + marker.length);
  const end = nextCase < 0 ? source.length : nextCase;
  const block = source.slice(start, end);
  if (block.includes(insertion.trim())) return source;
  const relative = block.indexOf(needle);
  if (relative < 0) throw new Error(`DEN-267 case insertion marker not found: ${label}`);
  if (block.indexOf(needle, relative + needle.length) >= 0) {
    throw new Error(`DEN-267 case insertion marker is ambiguous: ${label}`);
  }
  const absolute = start + relative + needle.length;
  return source.slice(0, absolute) + insertion + source.slice(absolute);
}

let source = await readFile(agentPath, 'utf8');

source = replaceOnce(
  source,
  "import { z } from 'zod';\n",
  "import { z } from 'zod';\nimport {\n  SensitiveFieldPolicyError,\n  assertSensitiveFieldWriteAllowed,\n} from './sensitive-field-policy.js';\n",
  'policy import',
);

source = replaceOnce(
  source,
  "  | 'browser_crashed'\n  | 'internal_error';",
  "  | 'browser_crashed'\n  | 'sensitive_field_blocked'\n  | 'internal_error';",
  'error-code union',
);

source = replaceOnce(
  source,
  "  if (typeof entry === 'string') return entry;\n  if (typeof entry.value !== 'string') {\n    throw new AgentError('secret_required', 'malformed secret entry');\n  }\n  if (entry.domains && entry.domains.length > 0) {\n    const host = currentHost.toLowerCase();\n    const permitted = entry.domains.some((d) => host === d.toLowerCase() || host.endsWith('.' + d.toLowerCase()));\n    if (!permitted) {\n      throw new AgentError('secret_required', 'secret is not permitted on the current page domain');\n    }\n  }\n  return entry.value;",
  "  if (typeof entry === 'string') {\n    throw new AgentError('secret_required', 'secret entries must be domain-bound objects');\n  }\n  if (typeof entry.value !== 'string') {\n    throw new AgentError('secret_required', 'malformed secret entry');\n  }\n  if (!entry.domains || entry.domains.length === 0) {\n    throw new AgentError('secret_required', 'secret entry must declare at least one permitted domain');\n  }\n  const host = currentHost.toLowerCase();\n  const permitted = entry.domains.some(\n    (domain) => host === domain.toLowerCase() || host.endsWith('.' + domain.toLowerCase()),\n  );\n  if (!permitted) {\n    throw new AgentError('secret_required', 'secret is not permitted on the current page domain');\n  }\n  return entry.value;",
  'domain-bound secret enforcement',
);

const helper = `\nasync function assertResolvedFieldWriteAllowed(\n  loc: Locator,\n  value: z.infer<typeof ValueSchema>,\n): Promise<void> {\n  const field = await loc.evaluate((element) => {\n    const id = element.getAttribute('id') ?? '';\n    const explicitLabel = id\n      ? element.ownerDocument.querySelector(\`label[for=\"\${CSS.escape(id)}\"]\`)\n      : null;\n    const containingLabel = element.closest('label');\n    return {\n      tagName: element.tagName,\n      type: element.getAttribute('type'),\n      name: element.getAttribute('name'),\n      id,\n      autocomplete: element.getAttribute('autocomplete'),\n      ariaLabel: element.getAttribute('aria-label'),\n      label: (explicitLabel?.textContent ?? containingLabel?.textContent ?? '').trim(),\n      placeholder: element.getAttribute('placeholder'),\n      title: element.getAttribute('title'),\n    };\n  });\n\n  try {\n    assertSensitiveFieldWriteAllowed(field, 'literal' in value ? 'literal' : 'secret_ref');\n  } catch (error) {\n    if (error instanceof SensitiveFieldPolicyError) {\n      throw new AgentError('sensitive_field_blocked', error.message);\n    }\n    throw error;\n  }\n}\n`;

source = replaceOnce(
  source,
  "\n// Returns true if the batch produced a state change.\nasync function runActions(",
  `${helper}\n// Returns true if the batch produced a state change.\nasync function runActions(`,
  'resolved-target guard helper',
);

const locatorNeedle = "          const loc = await resolveTarget(session, action.target);\n";
source = insertInCase(
  source,
  'fill',
  locatorNeedle,
  "          await assertResolvedFieldWriteAllowed(loc, action.value);\n",
  'fill write guard',
);
source = insertInCase(
  source,
  'type',
  locatorNeedle,
  "          await assertResolvedFieldWriteAllowed(loc, action.value);\n",
  'type write guard',
);
source = insertInCase(
  source,
  'fill_form',
  "            const loc = await resolveTarget(session, field.target);\n",
  "            await assertResolvedFieldWriteAllowed(loc, field.value);\n",
  'fill-form write guard',
);

await writeFile(agentPath, source);

let readme = await readFile(readmePath, 'utf8');
readme = replaceOnce(
  readme,
  '* **Uploads are bounded**:',
  '* **Sensitive-field writes fail closed**: SSN/tax identifiers, bank and payment-card fields, and MFA/OTP/PIN controls cannot be filled by the agent. Literal credentials are rejected; credentials may only use a domain-bound `secret_ref`.\n* **Uploads are bounded**:',
  'runbook safety bullet',
);
await writeFile(readmePath, readme);

console.log('DEN-267 sensitive-field guard applied successfully');
