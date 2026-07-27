#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';

const path = 'remote/deployments/web-scraper-service/src/browser-agent.ts';
let source = await readFile(path, 'utf8');

function replaceOnce(before, after, label) {
  if (source.includes(after)) return;
  const first = source.indexOf(before);
  if (first < 0) throw new Error(`DEN-267 marker not found: ${label}`);
  if (source.indexOf(before, first + before.length) >= 0) {
    throw new Error(`DEN-267 marker is ambiguous: ${label}`);
  }
  source = source.slice(0, first) + after + source.slice(first + before.length);
}

replaceOnce(
  "  | 'ambiguous_target'\n  | 'other';",
  "  | 'ambiguous_target'\n  | 'sensitive_field_blocked'\n  | 'other';",
  'blocker type',
);

replaceOnce(
  "        (e.code === 'ambiguous_target' || e.code === 'target_not_found' || e.code === 'domain_not_allowed')",
  "        (e.code === 'ambiguous_target' ||\n          e.code === 'target_not_found' ||\n          e.code === 'domain_not_allowed' ||\n          e.code === 'sensitive_field_blocked')",
  'blocked error condition',
);

replaceOnce(
  "            : e.code === 'domain_not_allowed'\n              ? 'domain_not_allowed'\n              : 'other';",
  "            : e.code === 'domain_not_allowed'\n              ? 'domain_not_allowed'\n              : e.code === 'sensitive_field_blocked'\n                ? 'sensitive_field_blocked'\n                : 'other';",
  'blocker type mapping',
);

await writeFile(path, source);
console.log('DEN-267 sensitive-field blocker outcome applied');
