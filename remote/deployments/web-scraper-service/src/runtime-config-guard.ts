const APIFY_API_ORIGIN = 'https://api.apify.com';
const APIFY_API_PATH = '/v2';

/**
 * Credential-bearing provider endpoints are code-owned trust boundaries.
 * APIFY_TOKEN must never be redirected to an arbitrary HTTPS endpoint merely
 * because configuration supplied another base URL.
 */
export function assertTrustedProviderEndpoints(env: NodeJS.ProcessEnv = process.env): void {
  const configured = (env.APIFY_API_BASE_URL ?? `${APIFY_API_ORIGIN}${APIFY_API_PATH}`).trim();
  let url: URL;
  try {
    url = new URL(configured);
  } catch {
    throw new TypeError('APIFY_API_BASE_URL must be the canonical Apify v2 endpoint');
  }

  const normalizedPath = url.pathname.replace(/\/+$/, '') || '/';
  if (
    url.origin !== APIFY_API_ORIGIN ||
    normalizedPath !== APIFY_API_PATH ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new TypeError('APIFY_API_BASE_URL must be exactly https://api.apify.com/v2');
  }
}
