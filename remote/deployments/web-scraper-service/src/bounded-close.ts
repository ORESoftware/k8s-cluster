/**
 * Release a browser resource (context/page) without letting cleanup either
 * hang the request or mask the scrape's real outcome.
 *
 * `context.close()` can reject when Chromium has already crashed, and can
 * stall when the browser is wedged. Awaiting it bare in a `finally` would
 * replace the original navigation error with "Target closed" (breaking failure
 * classification) or pin an in-flight slot indefinitely. This helper bounds
 * the wait and reports — but never throws — the cleanup outcome.
 */

export type CloseOutcome = 'closed' | 'failed' | 'timed-out';

export async function closeWithTimeout(
  close: () => Promise<unknown>,
  timeoutMs: number,
  onProblem?: (outcome: Exclude<CloseOutcome, 'closed'>, error?: unknown) => void,
): Promise<CloseOutcome> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<'timed-out'>((resolve) => {
    // Not unref'd: the timer is always cleared once close settles or times out.
    timer = setTimeout(() => resolve('timed-out'), Math.max(0, timeoutMs));
  });

  let closing: Promise<CloseOutcome>;
  try {
    closing = Promise.resolve(close()).then(
      () => 'closed' as const,
      (error: unknown) => {
        onProblem?.('failed', error);
        return 'failed' as const;
      },
    );
  } catch (error) {
    // close() threw synchronously.
    if (timer) clearTimeout(timer);
    onProblem?.('failed', error);
    return 'failed';
  }

  try {
    const outcome = await Promise.race([closing, timedOut]);
    if (outcome === 'timed-out') onProblem?.('timed-out');
    return outcome;
  } finally {
    if (timer) clearTimeout(timer);
  }
}
