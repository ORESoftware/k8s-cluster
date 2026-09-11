const UNKNOWN_OPTIONS_ENV = 'DD_WEB_SCRAPER_UNKNOWN_OPTIONS';
const PARSE_ERRORS_ENV = 'DD_WEB_SCRAPER_PARSE_ERRORS';

export function parseFlags2EnvOutput(stdout: string): Record<string, string> {
  let value: unknown;
  try {
    value = JSON.parse(stdout);
  } catch (error) {
    throw new Error(
      `flags2env returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`,
    );
  }

  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('flags2env returned invalid JSON: expected an object');
  }

  const resolved = value as Record<string, unknown>;
  assertEmptyDiagnosticChannel(resolved, UNKNOWN_OPTIONS_ENV, 'unknown option');
  assertEmptyDiagnosticChannel(resolved, PARSE_ERRORS_ENV, 'invalid value');

  const overrides: Record<string, string> = {};
  for (const [key, rawValue] of Object.entries(resolved)) {
    if (key === UNKNOWN_OPTIONS_ENV || key === PARSE_ERRORS_ENV) continue;
    if (!/^[A-Z_][A-Z0-9_]*$/.test(key)) {
      throw new TypeError(`flags2env returned unexpected environment key ${JSON.stringify(key)}`);
    }
    if (rawValue === null || typeof rawValue === 'object') {
      throw new TypeError(`flags2env returned unexpected non-scalar value for ${key}`);
    }
    overrides[key] = String(rawValue);
  }
  return overrides;
}

function assertEmptyDiagnosticChannel(
  resolved: Record<string, unknown>,
  key: string,
  label: string,
): void {
  const raw = resolved[key];
  if (raw === undefined || raw === '') return;
  if (typeof raw !== 'string') {
    throw new TypeError(`flags2env ${key} diagnostic channel must be a JSON-array string`);
  }

  let items: unknown;
  try {
    items = JSON.parse(raw);
  } catch {
    throw new TypeError(`flags2env ${key} diagnostic channel is not valid JSON`);
  }
  if (!Array.isArray(items)) {
    throw new TypeError(`flags2env ${key} diagnostic channel must contain a JSON array`);
  }
  if (items.length > 0) {
    const summary = items.slice(0, 8).map(String).join(', ');
    throw new TypeError(`flags2env rejected ${items.length} ${label}(s): ${summary}`);
  }
}
