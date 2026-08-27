import { isIP } from 'node:net';

export const DEFAULT_ALLOWED_ORIGINS = Object.freeze([
  'https://athleto.store',
  'https://app.athleto.store',
]);

function parseHttpsOrigin(name, value) {
  let url;
  try {
    url = new URL(value);
  } catch (error) {
    throw new TypeError(`${name} must be an absolute URL: ${error.message}`);
  }

  if (url.protocol !== 'https:') {
    throw new TypeError(`${name} must use HTTPS`);
  }
  if (url.username || url.password) {
    throw new TypeError(`${name} must not contain URL credentials`);
  }
  if (url.port && url.port !== '443') {
    throw new TypeError(`${name} must use the default HTTPS port`);
  }
  if (url.pathname !== '/' || url.search || url.hash) {
    throw new TypeError(`${name} must be an origin without a path, query, or fragment`);
  }

  const hostname = url.hostname.toLowerCase();
  if (
    isIP(hostname) !== 0 ||
    hostname === 'localhost' ||
    hostname.endsWith('.localhost') ||
    hostname.endsWith('.local') ||
    hostname.endsWith('.internal')
  ) {
    throw new TypeError(`${name} must use a public DNS hostname`);
  }

  return url.origin;
}

export function allowedLiveOrigins(
  extraOrigins = process.env.ATHLETO_UI_ALLOWED_ORIGINS ?? '',
) {
  const allowed = new Set(DEFAULT_ALLOWED_ORIGINS);
  for (const [index, value] of extraOrigins.split(',').entries()) {
    const candidate = value.trim();
    if (!candidate) continue;
    allowed.add(parseHttpsOrigin(`ATHLETO_UI_ALLOWED_ORIGINS[${index}]`, candidate));
  }
  return allowed;
}

export function normalizeLiveTarget(name, value, options = {}) {
  const origin = parseHttpsOrigin(name, value);
  const allowedOrigins = options.allowedOrigins ?? allowedLiveOrigins();
  if (!allowedOrigins.has(origin)) {
    throw new TypeError(
      `${name} origin ${origin} is not allow-listed; use ATHLETO_UI_ALLOWED_ORIGINS for an explicit public preview origin`,
    );
  }
  return origin;
}
