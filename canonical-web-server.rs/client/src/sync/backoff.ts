const MAX_RETRY_DELAY_MS = 5 * 60_000;

export function retryDelayMs(attempt: number, random = Math.random): number {
  const exponent = Math.min(Math.max(attempt - 1, 0), 9);
  const ceiling = Math.min(1_000 * 2 ** exponent, MAX_RETRY_DELAY_MS);
  return Math.floor(random() * ceiling);
}
