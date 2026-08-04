export function validateOperatorConfig(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('operator configuration must be an object');
  }
  const allowed = new Set(['collectionMode', 'consentRequired', 'maxPages']);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new TypeError(`unsupported operator configuration key: ${key}`);
    }
  }
  if (value.collectionMode !== 'official-api') {
    throw new TypeError('collectionMode must be official-api');
  }
  if (value.consentRequired !== true) {
    throw new TypeError('consentRequired must be true');
  }
  if (!Number.isSafeInteger(value.maxPages) || value.maxPages < 1 || value.maxPages > 100) {
    throw new TypeError('maxPages must be an integer from 1 through 100');
  }
  return Object.freeze({ ...value });
}
