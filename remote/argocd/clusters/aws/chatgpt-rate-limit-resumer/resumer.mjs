import { chromium } from 'playwright';
import process from 'node:process';
import { attemptEligibility, conversationKey } from './policy.mjs';
import {
  RUN_ID,
  RUN_STATE_PATH,
  config,
  errorCode,
  installRequestBoundary,
  log,
} from './runtime.mjs';
import {
  acquireRunLock,
  loadRunState,
  prepareStorageState,
  pruneRunState,
  saveBrowserState,
  writeJsonAtomic,
} from './state.mjs';
import {
  clickSend,
  collectCandidates,
  fillComposer,
  inspectConversation,
  visibleComposer,
  waitForOutcome,
  waitForSubmission,
} from './chatgpt-ui.mjs';

async function main() {
  log('run_started', {
    schedule_timezone: 'America/Chicago',
    max_chat_scan: config.maxChatScan,
    max_resumes: config.maxResumes,
  });

  const releaseRunLock = await acquireRunLock();
  let browser;
  let context;
  let blockedRequestCount = () => 0;
  const counters = {
    discovered: 0,
    inspected: 0,
    rate_limited: 0,
    attempted: 0,
    resumed: 0,
    still_rate_limited: 0,
    submitted_in_progress: 0,
    response_timeout: 0,
    skipped_cooldown: 0,
    skipped_attempt_cap: 0,
    errors: 0,
  };

  try {
    const storageState = await prepareStorageState();
    const runState = await loadRunState();
    const nowMs = Date.now();
    pruneRunState(runState, nowMs);

    browser = await chromium.launch({ headless: true });
    context = await browser.newContext({
      storageState,
      viewport: { width: 1440, height: 1000 },
      locale: 'en-US',
      timezoneId: 'America/Chicago',
      acceptDownloads: false,
      serviceWorkers: 'block',
    });
    context.setDefaultTimeout(config.pageTimeoutMs);
    blockedRequestCount = await installRequestBoundary(context);

    const page = await context.newPage();
    const candidates = await collectCandidates(page);
    counters.discovered = candidates.length;
    log('candidate_scan_complete', { count: candidates.length });

    for (const candidate of candidates) {
      if (counters.attempted >= config.maxResumes) break;

      const key = conversationKey(candidate.id);
      const existing = runState.conversations[key] ?? null;
      const seenAt = new Date().toISOString();

      try {
        await page.goto(candidate.url, {
          waitUntil: 'domcontentloaded',
          timeout: config.pageTimeoutMs,
        });
        await page.waitForTimeout(1_500);
        counters.inspected += 1;

        const snapshot = await inspectConversation(page);
        if (snapshot.authProblem) {
          const error = new Error(`ChatGPT authentication is unavailable: ${snapshot.authProblem}`);
          error.exitCode = 78;
          throw error;
        }

        runState.conversations[key] = {
          ...(runState.conversations[key] ?? existing ?? {}),
          lastSeenAt: seenAt,
          lastOutcome: snapshot.blocked ? existing?.lastOutcome ?? 'detected' : 'not_rate_limited',
        };

        if (!snapshot.blocked) continue;
        counters.rate_limited += 1;

        const eligibility = attemptEligibility(existing, Date.now(), {
          cooldownMs: config.cooldownMs,
          maxAttempts: config.maxAttempts,
          attemptWindowMs: config.attemptWindowMs,
        });
        if (!eligibility.eligible) {
          if (eligibility.reason === 'cooldown') counters.skipped_cooldown += 1;
          if (eligibility.reason === 'attempt_cap') counters.skipped_attempt_cap += 1;
          log('conversation_skipped', { conversation: key, reason: eligibility.reason });
          continue;
        }

        const composer = await visibleComposer(page);
        if (!composer) throw new Error('The ChatGPT composer is not available.');

        const baselineUserTurns = snapshot.userTurnCount;
        counters.attempted += 1;
        runState.conversations[key] = {
          ...runState.conversations[key],
          attempts: eligibility.attempts + 1,
          attemptWindowStartedAt: new Date(eligibility.windowStartedAt).toISOString(),
          lastAttemptAt: new Date().toISOString(),
          lastOutcome: 'attempt_reserved',
        };
        await writeJsonAtomic(RUN_STATE_PATH, runState);

        await fillComposer(composer, config.continuationPrompt);
        const submitMethod = await clickSend(page, composer);
        await waitForSubmission(page, baselineUserTurns);

        runState.conversations[key] = {
          ...runState.conversations[key],
          lastOutcome: 'submitted',
        };
        await writeJsonAtomic(RUN_STATE_PATH, runState);
        log('continuation_submitted', {
          conversation: key,
          rate_reason: snapshot.rateReason,
          submit_method: submitMethod,
        });

        const outcome = await waitForOutcome(page, baselineUserTurns);
        counters[outcome] = (counters[outcome] ?? 0) + 1;
        runState.conversations[key].lastOutcome = outcome;
        runState.conversations[key].lastSeenAt = new Date().toISOString();
        await writeJsonAtomic(RUN_STATE_PATH, runState);
        log('conversation_outcome', { conversation: key, outcome });
      } catch (error) {
        if (error?.exitCode === 78) throw error;
        counters.errors += 1;
        runState.conversations[key] = {
          ...(runState.conversations[key] ?? existing ?? {}),
          lastSeenAt: seenAt,
          lastOutcome: 'error',
          lastErrorAt: new Date().toISOString(),
          lastErrorClass: error?.name ?? 'Error',
        };
        await writeJsonAtomic(RUN_STATE_PATH, runState);
        log('conversation_error', {
          conversation: key,
          error_class: error?.name ?? 'Error',
          error_code: errorCode(error),
        });
      }
    }

    runState.last_run = {
      runId: RUN_ID,
      finishedAt: new Date().toISOString(),
      counters,
    };
    await writeJsonAtomic(RUN_STATE_PATH, runState);
    await saveBrowserState(context);

    log('run_completed', {
      ...counters,
      blocked_requests: blockedRequestCount(),
    });
  } finally {
    await context?.close().catch(() => {});
    await browser?.close().catch(() => {});
    await releaseRunLock();
  }
}

main().catch((error) => {
  log('run_failed', {
    error_class: error?.name ?? 'Error',
    error_code: errorCode(error),
  });
  process.exitCode = Number.isInteger(error?.exitCode) ? error.exitCode : 1;
});
