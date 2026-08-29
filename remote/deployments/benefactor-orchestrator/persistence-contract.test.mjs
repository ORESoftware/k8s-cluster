import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const source = await readFile(new URL('./orchestrate.mjs', import.meta.url), 'utf8');

test('persists the first verified business phone without overwriting an existing phone', () => {
  assert.match(source, /UPDATE benefactor\.benefactor_leads/);
  assert.match(source, /primary_phone\s*=\s*CASE/i);
  assert.match(source, /COALESCE\(BTRIM\(primary_phone\), ''\) = ''/i);
  assert.match(source, /record\.phones\[0\]/);
});

test('keeps discovery separate from outbound messaging', () => {
  assert.doesNotMatch(source, /api\.sendgrid\.com\/v3\/mail\/send/i);
  assert.doesNotMatch(source, /gmail\.googleapis\.com\/gmail\/v1\/users\/.+\/messages\/send/i);
});

test('keeps arbitrary page retrieval behind the private scraper', () => {
  assert.match(source, /dd-web-scraper\.default\.svc\.cluster\.local/);
  assert.match(source, /ALLOW_DIRECT_FALLBACK is no longer supported/);
});
