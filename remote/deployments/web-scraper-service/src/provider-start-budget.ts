export const PROVIDER_START_WINDOW_MS = 60_000;

export function pruneProviderStarts(
  starts: number[],
  nowMs: number,
  windowMs = PROVIDER_START_WINDOW_MS,
): number {
  assertFiniteTimestamp(nowMs, 'nowMs');
  if (!Number.isInteger(windowMs) || windowMs <= 0) {
    throw new TypeError('provider start budget windowMs must be a positive integer');
  }

  const cutoff = nowMs - windowMs;
  let stale = 0;
  while (stale < starts.length && starts[stale]! <= cutoff) stale += 1;
  if (stale > 0) starts.splice(0, stale);
  return starts.length;
}

export function tryConsumeProviderStartBudget(
  starts: number[],
  nowMs: number,
  maxStarts: number,
  windowMs = PROVIDER_START_WINDOW_MS,
): boolean {
  if (!Number.isInteger(maxStarts) || maxStarts <= 0) {
    throw new TypeError('provider start budget maxStarts must be a positive integer');
  }
  pruneProviderStarts(starts, nowMs, windowMs);
  if (starts.length >= maxStarts) return false;
  starts.push(nowMs);
  return true;
}

function assertFiniteTimestamp(value: number, name: string): void {
  if (!Number.isFinite(value)) throw new TypeError(`${name} must be finite`);
}
