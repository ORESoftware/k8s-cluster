// Real-browser integration test for the persistent browser-agent loop.
// Boots a local fixture site + a fastify instance with the /agent/* routes and a
// real headless Chromium, then drives the full observe -> act -> observe loop:
// validation errors, semantic + ref targeting, revisions, consequential-action
// confirmation, and CAPTCHA blocker detection.
//
// Requires the Playwright Chromium binary. Skips itself (does not fail) when the
// browser is unavailable, so unit-only CI stays green.

// Env must be set before importing the module (agentConfig reads it at load).
process.env.BROWSER_AGENT_ALLOW_INSECURE_HTTP = 'true';
process.env.BROWSER_AGENT_ALLOW_PRIVATE_NETWORKS = 'true';
process.env.BROWSER_AGENT_ALLOWED_DOMAINS = '';

import assert from 'node:assert/strict';
import { test } from 'node:test';

import Fastify, { type FastifyInstance } from 'fastify';
import type { Browser } from 'playwright';

import { startFixture } from './fixtures/test-site.mjs';

// Dynamic import AFTER the env is set above: agentConfig reads env at module load,
// and static imports are hoisted above the process.env assignments.
const { registerBrowserAgentRoutes, closeAllSessions } = await import('../src/browser-agent.js');

let chromium: typeof import('playwright').chromium | null = null;
let launchBrowser: Browser | null = null;
try {
  ({ chromium } = await import('playwright'));
  launchBrowser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--disable-dev-shm-usage'] });
} catch (e) {
  // eslint-disable-next-line no-console
  console.warn('skipping browser-agent integration test: Chromium unavailable ->', (e as Error).message);
}

test(
  'full observe/act loop: validation, refs, revisions, confirmation, and safety blockers',
  { skip: launchBrowser === null },
  async () => {
    const browser = launchBrowser!;
    const fixture = await startFixture();
    const app: FastifyInstance = Fastify();
    registerBrowserAgentRoutes(app, {
      getBrowser: async () => browser,
      isPrivateIp: () => false,
      isAuthorized: () => true,
      log: app.log,
    });
    await app.ready();

    const act = async (payload: unknown): Promise<Record<string, unknown>> => {
      const r = await app.inject({ method: 'POST', url: '/agent/act', payload });
      return JSON.parse(r.body) as Record<string, unknown>;
    };
    const observe = async (payload: unknown): Promise<Record<string, unknown>> => {
      const r = await app.inject({ method: 'POST', url: '/agent/observe', payload });
      return JSON.parse(r.body) as Record<string, unknown>;
    };

    try {
      // 1) start a session at step 1.
      const started = await act({
        request_id: 'r1',
        intent: 'open registration form',
        actions: [{ type: 'start', initial_url: `${fixture.url}/step1` }],
      });
      assert.equal(started.status, 'completed', JSON.stringify(started));
      const sessionId = started.session_id as string;
      assert.ok(sessionId, 'session_id returned');
      assert.match(started.page ? (started.page as { url: string }).url : '', /\/step1$/);

      // 2) browser state -> forms, typed control buckets, and accessibility refs.
      const obs1 = await observe({
        session_id: sessionId,
        include: [
          'visible_text',
          'interactive_elements',
          'accessibility_snapshot',
          'forms',
          'validation_errors',
          'downloads',
        ],
      });
      const els = (obs1.interactive_elements ?? []) as Array<Record<string, unknown>>;
      assert.ok(els.length >= 3, `expected controls, got ${els.length}`);
      const entity = els.find((e) => e.label === 'Entity name' || e.name === 'Entity name');
      assert.ok(entity, 'entity field observed with a ref');
      assert.ok(Array.isArray(obs1.fields) && (obs1.fields as unknown[]).length > 0);
      assert.ok(Array.isArray(obs1.buttons) && (obs1.buttons as unknown[]).length > 0);
      assert.ok(Array.isArray(obs1.links));
      assert.equal((obs1.accessibility_snapshot as { role: string }).role, 'document');
      assert.ok(obs1.visible_text);
      assert.deepEqual(obs1.downloads, []);

      // 3) click Next with the required field empty -> native validation error.
      const badSubmit = await act({
        request_id: 'r2',
        session_id: sessionId,
        expected_revision: obs1.revision,
        intent: 'attempt submit with empty required field',
        actions: [{ type: 'click', target: { visible_text: 'Next' } }],
      });
      assert.ok(['completed', 'partially_completed'].includes(badSubmit.status as string), JSON.stringify(badSubmit));
      // still on step 1 (native validation blocked navigation)
      assert.match((badSubmit.page as { url: string }).url, /\/step1/);

      const obs2 = await observe({ session_id: sessionId, include: ['validation_errors', 'interactive_elements'] });
      const vErrors = (obs2.validation_errors ?? []) as Array<Record<string, unknown>>;
      assert.ok(vErrors.length >= 1, `expected a validation error, got ${JSON.stringify(vErrors)}`);

      // 4) upload a small file entirely in memory and observe its page-visible
      // filename/size. The worker must not need an upload-token directory.
      const uploaded = await act({
        request_id: 'r2a',
        session_id: sessionId,
        expected_revision: obs2.revision,
        intent: 'attach a harmless text fixture',
        actions: [
          {
            type: 'upload',
            target: { label: 'Attachment' },
            inline_file: {
              file_name: 'fixture.txt',
              mime_type: 'text/plain',
              data_base64: Buffer.from('hello').toString('base64'),
            },
          },
        ],
      });
      assert.equal(uploaded.status, 'completed', JSON.stringify(uploaded));
      const obsUpload = await observe({ session_id: sessionId, include: ['visible_text'] });
      assert.match(
        (obsUpload.visible_text as { untrusted_content: string }).untrusted_content,
        /Selected fixture\.txt \(5 bytes\)/,
      );

      // 5) fill the form correctly (semantic + ref targeting) and advance.
      const filled = await act({
        request_id: 'r3',
        session_id: sessionId,
        expected_revision: obsUpload.revision,
        intent: 'complete the form and continue',
        actions: [
          {
            type: 'type',
            target: { label: 'Entity name' },
            value: { literal: 'ORE Software LLC' },
            clear_first: true,
          },
          { type: 'select', target: { role: 'combobox', name: 'State' }, option: { value: 'CO' } },
          { type: 'check', target: { label: 'I agree to the terms' } },
          { type: 'click', target: { visible_text: 'Next' } },
        ],
      });
      assert.equal(filled.status, 'completed', JSON.stringify(filled));
      assert.match((filled.page as { url: string }).url, /\/step2/);
      assert.ok((filled.revision as number) > (obs2.revision as number), 'revision advanced after navigation');

      // 6) stale revision is rejected.
      const stale = await act({
        request_id: 'r4',
        session_id: sessionId,
        expected_revision: 0,
        intent: 'act on stale state',
        actions: [{ type: 'reload' }],
      });
      assert.equal(stale.status, 'revision_conflict', JSON.stringify(stale));

      // 7) consequential submit -> needs_confirmation with a digest.
      const obs3 = await observe({ session_id: sessionId, include: ['interactive_elements'] });
      const pending = await act({
        request_id: 'r5',
        session_id: sessionId,
        expected_revision: obs3.revision,
        intent: 'submit the filing',
        actions: [{ type: 'submit', target: { visible_text: 'Submit filing' } }],
      });
      assert.equal(pending.status, 'needs_confirmation', JSON.stringify(pending));
      const pa = pending.pending_action as { action_digest: string; revision: number };
      assert.match(pa.action_digest, /^sha256:[0-9a-f]{64}$/);

      // 8) confirmed submit succeeds and navigates to /done.
      const confirmed = await act({
        request_id: 'r6',
        session_id: sessionId,
        expected_revision: pa.revision,
        intent: 'submit the filing (confirmed)',
        actions: [{ type: 'submit', target: { visible_text: 'Submit filing' } }],
        confirmation: { action_digest: pa.action_digest, confirmed_revision: pa.revision, user_explicitly_approved: true },
      });
      assert.equal(confirmed.status, 'completed', JSON.stringify(confirmed));
      assert.match((confirmed.page as { url: string }).url, /\/done/);

      // 7b) screenshot is captured on demand.
      const shot = await observe({ session_id: sessionId, include: ['summary', 'screenshot'] });
      const s = shot.screenshot as { mime_type: string; data_base64: string } | undefined;
      assert.ok(s && s.mime_type === 'image/jpeg' && s.data_base64.length > 100, 'screenshot returned');

      // 7c) action-level scroll, screenshot, and extract return bounded data.
      const captured = await act({
        request_id: 'r6a',
        session_id: sessionId,
        expected_revision: (shot as { revision: number }).revision,
        intent: 'exercise bounded read actions',
        actions: [
          { type: 'scroll', delta_y: 200 },
          { type: 'screenshot' },
          {
            type: 'extract',
            include: ['visible_text', 'interactive_elements', 'accessibility_snapshot'],
            max_visible_text_chars: 2000,
          },
        ],
      });
      assert.equal(captured.status, 'completed', JSON.stringify(captured));
      assert.ok((captured.screenshot as { data_base64?: string }).data_base64);
      assert.equal(
        ((captured.extracted as { accessibility_snapshot: { role: string } }).accessibility_snapshot).role,
        'document',
      );

      // 7d) stop_when halts a batch early (navigation short-circuits the reload).
      const stopped = await act({
        request_id: 'r6b',
        session_id: sessionId,
        expected_revision: captured.revision,
        intent: 'navigate then stop before the extra action',
        actions: [
          { type: 'goto', url: `${fixture.url}/step1` },
          { type: 'reload' },
        ],
        stop_when: { url_matches: '/step1' },
      });
      assert.equal(stopped.status, 'completed', JSON.stringify(stopped));
      const stoppedResults = stopped.action_results as Array<{ type: string; status: string }>;
      const reload = stoppedResults.find((r) => r.type === 'reload');
      assert.equal(reload?.status, 'skipped', 'reload skipped after stop_when satisfied');

      // 8) Marketing copy that mentions MFA must not block a normal,
      // multi-field startup application.
      const beforeStartup = await observe({ session_id: sessionId, include: ['summary'] });
      const startupNav = await act({
        request_id: 'r7',
        session_id: sessionId,
        expected_revision: beforeStartup.revision,
        intent: 'open a normal startup application',
        actions: [{ type: 'goto', url: `${fixture.url}/startup` }],
      });
      assert.equal(startupNav.status, 'completed', JSON.stringify(startupNav));
      const obsStartup = await observe({ session_id: sessionId, include: ['summary', 'interactive_elements'] });
      assert.equal(obsStartup.blocker, undefined, JSON.stringify(obsStartup.blocker));
      const startupFill = await act({
        request_id: 'r8',
        session_id: sessionId,
        expected_revision: obsStartup.revision,
        intent: 'verify the normal form remains fillable',
        actions: [{ type: 'fill', target: { label: 'First name' }, value: { literal: 'Test' } }],
      });
      assert.equal(startupFill.status, 'completed', JSON.stringify(startupFill));

      // 9) A compact verification-code page is still blocked as MFA.
      const beforeMfa = await observe({ session_id: sessionId, include: ['summary'] });
      const mfaNav = await act({
        request_id: 'r9',
        session_id: sessionId,
        expected_revision: beforeMfa.revision,
        intent: 'open a verification challenge',
        actions: [{ type: 'goto', url: `${fixture.url}/mfa` }],
      });
      assert.equal(mfaNav.status, 'completed', JSON.stringify(mfaNav));
      const obsMfa = await observe({ session_id: sessionId, include: ['summary'] });
      assert.equal((obsMfa.blocker as { type: string }).type, 'mfa');
      const blockedMfaFill = await act({
        request_id: 'r10',
        session_id: sessionId,
        expected_revision: obsMfa.revision,
        intent: 'verify code entry remains blocked',
        actions: [{ type: 'fill', target: { label: 'One-time code' }, value: { literal: '123456' } }],
      });
      assert.equal(blockedMfaFill.status, 'blocked', JSON.stringify(blockedMfaFill));

      // 10) A genuine card-entry page remains blocked as payment.
      const beforePayment = await observe({ session_id: sessionId, include: ['summary'] });
      const paymentNav = await act({
        request_id: 'r11',
        session_id: sessionId,
        expected_revision: beforePayment.revision,
        intent: 'open a payment screen',
        actions: [{ type: 'goto', url: `${fixture.url}/payment` }],
      });
      assert.equal(paymentNav.status, 'completed', JSON.stringify(paymentNav));
      const obsPayment = await observe({ session_id: sessionId, include: ['summary'] });
      assert.equal((obsPayment.blocker as { type: string }).type, 'payment');

      // 11) navigate to the CAPTCHA page -> blocker is detected and interaction is refused.
      const obsBeforeNav = await observe({ session_id: sessionId, include: ['summary'] });
      const nav = await act({
        request_id: 'r12',
        session_id: sessionId,
        expected_revision: obsBeforeNav.revision,
        intent: 'go to the verify page',
        actions: [{ type: 'goto', url: `${fixture.url}/captcha` }],
      });
      assert.equal(nav.status, 'completed', JSON.stringify(nav));
      const obsCaptcha = await observe({ session_id: sessionId, include: ['summary'] });
      assert.ok(obsCaptcha.blocker, 'captcha blocker surfaced on observe');
      assert.equal((obsCaptcha.blocker as { type: string }).type, 'captcha');

      // 12) close the session.
      const closed = await act({
        request_id: 'r13',
        session_id: sessionId,
        intent: 'done',
        actions: [{ type: 'close' }],
      });
      assert.equal(closed.status, 'completed');
      const gone = await observe({ session_id: sessionId });
      assert.equal((gone as { error_code?: string }).error_code, 'session_not_found');
    } finally {
      await closeAllSessions();
      await app.close();
      await fixture.close();
    }
  },
);

test(
  'domain allowlist is enforced: navigation off the allowlist is blocked',
  { skip: launchBrowser === null },
  async () => {
    const browser = launchBrowser!;
    const fixture = await startFixture();
    const app = Fastify();
    registerBrowserAgentRoutes(app, {
      getBrowser: async () => browser,
      isPrivateIp: () => false,
      isAuthorized: () => true,
      log: app.log,
    });
    await app.ready();
    const act = async (payload: unknown): Promise<Record<string, unknown>> =>
      JSON.parse((await app.inject({ method: 'POST', url: '/agent/act', payload })).body) as Record<string, unknown>;
    try {
      // A caller-supplied allowlist that does not include the fixture host must
      // block navigation to it (caller can only narrow, and goto is guarded).
      const res = await act({
        request_id: 'a1',
        intent: 'start restricted to a different domain',
        actions: [{ type: 'start', browser: 'chromium' }],
        allowed_domains: ['example.invalid'],
      });
      const started = res.status === 'completed';
      assert.ok(started, JSON.stringify(res));
      const sessionId = res.session_id as string;
      const blocked = await act({
        request_id: 'a2',
        session_id: sessionId,
        intent: 'try to leave the allowlist',
        actions: [{ type: 'goto', url: `${fixture.url}/step1` }],
      });
      // goto off the allowlist is surfaced as a domain_not_allowed blocker.
      assert.equal(blocked.status, 'blocked', JSON.stringify(blocked));
      assert.equal((blocked.blocker as { type: string }).type, 'domain_not_allowed');
    } finally {
      await closeAllSessions();
      await app.close();
      await fixture.close();
    }
  },
);

test(
  'off-allowlist navigation via a link click is blocked at the network layer',
  { skip: launchBrowser === null },
  async () => {
    const browser = launchBrowser!;
    const fixture = await startFixture();
    const host = new URL(fixture.url).hostname; // 127.0.0.1
    const app = Fastify();
    registerBrowserAgentRoutes(app, {
      getBrowser: async () => browser,
      isPrivateIp: () => false,
      isAuthorized: () => true,
      log: app.log,
    });
    await app.ready();
    const act = async (payload: unknown): Promise<Record<string, unknown>> =>
      JSON.parse((await app.inject({ method: 'POST', url: '/agent/act', payload })).body) as Record<string, unknown>;
    const observe = async (payload: unknown): Promise<Record<string, unknown>> =>
      JSON.parse((await app.inject({ method: 'POST', url: '/agent/observe', payload })).body) as Record<string, unknown>;
    try {
      // Allowlist admits only the fixture host. The page2 link points at a
      // different origin (example.com); clicking it must be aborted by the
      // request interceptor (not merely by the goto guard), leaving the session
      // with a domain_not_allowed blocker.
      const started = await act({
        request_id: 'n1',
        intent: 'open step 2 within the allowlist',
        actions: [{ type: 'start', initial_url: `${fixture.url}/step2` }],
        allowed_domains: [host],
      });
      assert.equal(started.status, 'completed', JSON.stringify(started));
      const sessionId = started.session_id as string;
      await act({
        request_id: 'n2',
        session_id: sessionId,
        intent: 'click the external link',
        actions: [{ type: 'click', target: { visible_text: 'external site' } }],
      });
      // The click itself completes, but the navigation it triggers is aborted;
      // the session records the blocker (observed on the next observe).
      const obs = await observe({ session_id: sessionId, include: ['summary'] });
      assert.equal((obs.blocker as { type: string } | undefined)?.type, 'domain_not_allowed', JSON.stringify(obs));
      // The blocked target never loaded (aborted -> chrome-error page).
      assert.doesNotMatch((obs.page as { url: string }).url, /example\.com/);
    } finally {
      await closeAllSessions();
      await app.close();
      await fixture.close();
    }
  },
);

test('teardown: close shared browser', { skip: launchBrowser === null }, async () => {
  await launchBrowser!.close();
});
