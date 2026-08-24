const DEFAULT_REQUEST_TIMEOUT_MS = 15_000;
const DEFAULT_MAX_ATTEMPTS = 4;

function retryAfterMilliseconds(response, attempt) {
  const raw = response?.headers?.get?.('retry-after');
  if (raw) {
    const seconds = Number(raw);
    if (Number.isFinite(seconds) && seconds >= 0) return Math.min(seconds * 1000, 30_000);
    const date = Date.parse(raw);
    if (Number.isFinite(date)) return Math.max(0, Math.min(date - Date.now(), 30_000));
  }
  return Math.min(500 * (2 ** attempt), 8_000);
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export async function fetchJsonWithRetry(url, options = {}, runtime = {}) {
  const fetchImpl = runtime.fetchImpl || globalThis.fetch;
  const attempts = runtime.attempts || DEFAULT_MAX_ATTEMPTS;
  const timeoutMs = runtime.timeoutMs || DEFAULT_REQUEST_TIMEOUT_MS;
  const sleepImpl = runtime.sleepImpl || sleep;
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    let response;
    try {
      response = await fetchImpl(url, {
        ...options,
        signal: AbortSignal.timeout(timeoutMs),
      });
      if (response.ok) return await response.json();
      const body = await response.text().catch(() => '');
      const exhaustedRateLimit = response.status === 403 && response.headers.get('x-ratelimit-remaining') === '0';
      const retryable = response.status === 429 || response.status >= 500 || exhaustedRateLimit;
      if (!retryable || attempt === attempts - 1) {
        const error = new Error(
          `HTTP ${response.status} from ${new URL(url).host}: ${body.slice(0, 300)}`,
        );
        error.status = response.status;
        throw error;
      }
      await sleepImpl(retryAfterMilliseconds(response, attempt));
    } catch (error) {
      lastError = error;
      const retryableNetworkError = error?.name === 'TimeoutError' || error?.name === 'AbortError' || error instanceof TypeError;
      if (!retryableNetworkError || attempt === attempts - 1) throw error;
      await sleepImpl(Math.min(500 * (2 ** attempt), 8_000));
    }
  }
  throw lastError || new Error('Request failed without an error');
}
