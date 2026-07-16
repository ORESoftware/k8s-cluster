import assert from 'node:assert/strict';
import test from 'node:test';

import { captchaAutoSolveAllowed } from '../dist/scrape-policy.js';

test('a request cannot enable CAPTCHA solving when the operator disabled it', () => {
  assert.equal(captchaAutoSolveAllowed(false, true), false);
  assert.equal(captchaAutoSolveAllowed(false, undefined), false);
});

test('a request can opt out of an operator-enabled CAPTCHA solver', () => {
  assert.equal(captchaAutoSolveAllowed(true, false), false);
  assert.equal(captchaAutoSolveAllowed(true, true), true);
  assert.equal(captchaAutoSolveAllowed(true, undefined), true);
});
