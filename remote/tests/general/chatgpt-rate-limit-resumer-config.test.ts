import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';

const repo = [process.cwd(), resolve(process.cwd(), '..', '..')].find((p) => existsSync(resolve(p, 'remote/argocd/clusters/aws/kustomization.yaml')));
assert.ok(repo);
const root = resolve(repo, 'remote/argocd/clusters/aws/chatgpt-rate-limit-resumer');
const read = (path: string): string => readFileSync(resolve(repo, path), 'utf8');
const cron = readFileSync(resolve(root, 'cronjob.yaml'), 'utf8');
const scripts = ['runtime.mjs', 'state.mjs', 'chatgpt-ui.mjs', 'resumer.mjs'].map((n) => readFileSync(resolve(root, n), 'utf8')).join('\n');
const policy = await import(pathToFileURL(resolve(root, 'policy.mjs')).href);

test('03:00 Central recovery job is AWS-only and least-privileged', () => {
  assert.match(cron, /schedule:\s*"0 3 \* \* \*"/);
  assert.match(cron, /timeZone:\s*America\/Chicago/);
  assert.match(cron, /concurrencyPolicy:\s*Forbid/);
  assert.match(cron, /automountServiceAccountToken:\s*false/);
  assert.match(cron, /key:\s*CHATGPT_STORAGE_STATE_JSON/);
  assert.doesNotMatch(cron, /OPENAI_API_KEY|GH_PAT|SERVER_AUTH_SECRET/);
  assert.match(read('remote/argocd/clusters/aws/kustomization.yaml'), /- chatgpt-rate-limit-resumer/);
  assert.doesNotMatch(read('remote/argocd/clusters/gcp/kustomization.yaml'), /chatgpt-rate-limit-resumer/);
  assert.doesNotMatch(read('remote/argocd/clusters/hetzner/kustomization.yaml'), /chatgpt-rate-limit-resumer/);
});

test('rate-limit retry policy is narrow, private, and idempotent', () => {
  assert.equal(policy.isRateLimitErrorText('Too many requests. Please try again later.'), true);
  assert.equal(policy.isRateLimitErrorText('The “too many requests” test now passes.'), false);
  assert.equal(policy.conversationIdentity('https://example.com/c/nope'), null);
  const opts = { cooldownMs: 72_000_000, maxAttempts: 7, attemptWindowMs: 604_800_000 };
  assert.equal(policy.attemptEligibility({ lastAttemptAt: '2026-08-05T01:00:00Z', attempts: 1, attemptWindowStartedAt: '2026-08-04T08:00:00Z' }, Date.parse('2026-08-05T08:00:00Z'), opts).reason, 'cooldown');
  assert.doesNotMatch(scripts, /page\.screenshot|context\.tracing|page\.content\(/);
  assert.match(scripts, /lastOutcome:\s*'attempt_reserved'/);
  assert.ok(scripts.indexOf("lastOutcome: 'attempt_reserved'") < scripts.indexOf('await clickSend'));
});
