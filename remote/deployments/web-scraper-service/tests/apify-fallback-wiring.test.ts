import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';

const root = new URL('../', import.meta.url);

async function read(relative: string): Promise<string> {
  return readFile(new URL(relative, root), 'utf8');
}

test('Apify fallback stays opt-in, bounded, and policy preserving', async () => {
  const gateway = await read('src/gateway.ts');

  assert.match(gateway, /SCRAPER_APIFY_FALLBACK/);
  assert.match(gateway, /upstreamStatus < 500 \|\| upstreamStatus > 599/);
  assert.match(gateway, /error\.includes\('robots\.txt'\)/);
  assert.match(gateway, /error\.includes\('blocked by scraper'\)/);
  assert.match(gateway, /error\.includes\('captcha'\)/);
  assert.match(gateway, /body\.includeContacts \|\| body\.includePhones \|\| body\.includeEmails/);
  assert.match(gateway, /body\.proxy \|\| body\.useProxy === true \|\| body\.solveCaptcha === true/);
  assert.match(gateway, /maxCrawlDepth: 0/);
  assert.match(gateway, /maxCrawlPages: 1/);
  assert.match(gateway, /SCRAPER_APIFY_MAX_CONCURRENT/);
  assert.match(gateway, /SCRAPER_APIFY_MAX_PER_MINUTE/);
  assert.match(gateway, /SCRAPER_APIFY_MAX_RESPONSE_BYTES/);
  assert.match(gateway, /authorization: `Bearer \$\{apifyConfig\.token\}`/);
  assert.doesNotMatch(gateway, /searchParams\.set\(['"]token['"]/);
});

test('flags2env owns non-secret CLI configuration and secrets stay env-only', async () => {
  const config = await read('.cli-flags.toml');
  const entrypoint = await read('src/entrypoint.ts');
  const dockerfile = await read('Dockerfile');

  assert.match(config, /env = "SCRAPER_APIFY_FALLBACK"/);
  assert.match(config, /env = "SCRAPER_APIFY_TIMEOUT_MS"/);
  assert.match(config, /env = "SCRAPER_APIFY_MAX_CONCURRENT"/);
  assert.match(config, /env = "SCRAPER_APIFY_MAX_PER_MINUTE"/);
  assert.doesNotMatch(config, /APIFY_API_TOKEN/);
  assert.doesNotMatch(config, /SERVER_AUTH_SECRET/);

  assert.match(entrypoint, /spawnSync\('flags2env'/);
  assert.match(entrypoint, /env: \{ \.\.\.process\.env, \.\.\.overrides \}/);
  assert.match(dockerfile, /npm install --global @oresoftware\/f2e@0\.3\.0/);
  assert.match(dockerfile, /flags2env audit \.cli-flags\.toml/);
  assert.match(dockerfile, /CMD \["node", "dist\/entrypoint\.js"\]/);
});
