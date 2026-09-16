import assert from 'node:assert/strict';
import test from 'node:test';

import { parseFlags2EnvOutput } from '../src/flags-runtime.js';

test('accepts scalar env overrides when diagnostic channels are empty', () => {
  assert.deepEqual(
    parseFlags2EnvOutput(
      JSON.stringify({
        PORT: '8098',
        APIFY_FALLBACK_ENABLED: 'true',
        DD_WEB_SCRAPER_UNKNOWN_OPTIONS: '[]',
        DD_WEB_SCRAPER_PARSE_ERRORS: '[]',
      }),
    ),
    { PORT: '8098', APIFY_FALLBACK_ENABLED: 'true' },
  );
});

test('rejects unknown options even when the flags2env process itself exited successfully', () => {
  assert.throws(
    () =>
      parseFlags2EnvOutput(
        JSON.stringify({ DD_WEB_SCRAPER_UNKNOWN_OPTIONS: '["--typo"]' }),
      ),
    /rejected 1 unknown option/,
  );
});

test('rejects parser value errors and malformed diagnostic channels', () => {
  assert.throws(
    () => parseFlags2EnvOutput(JSON.stringify({ DD_WEB_SCRAPER_PARSE_ERRORS: '["PORT"]' })),
    /rejected 1 invalid value/,
  );
  assert.throws(
    () => parseFlags2EnvOutput(JSON.stringify({ DD_WEB_SCRAPER_PARSE_ERRORS: 'not-json' })),
    /not valid JSON/,
  );
});

test('rejects unexpected env names and structured values', () => {
  assert.throws(() => parseFlags2EnvOutput(JSON.stringify({ 'bad-key': 'x' })), /unexpected environment key/);
  assert.throws(() => parseFlags2EnvOutput(JSON.stringify({ PORT: { nested: true } })), /non-scalar/);
});
