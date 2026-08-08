const PROVIDER_DIAGNOSTICS_VERSION = 'benefactor.provider-diagnostics.v1';
const INSTALL_MARKER = Symbol.for('benefactor.providerDiagnostics.installed');
const STATE_MARKER = Symbol.for('benefactor.providerDiagnostics.state');

const PROVIDERS = Object.freeze({
  brave: {
    origin: 'https://api.search.brave.com',
    pathname: '/res/v1/web/search',
  },
  serper: {
    origin: 'https://google.serper.dev',
    pathname: '/search',
  },
});

const NETWORK_ERROR_CODES = new Set([
  'ECONNREFUSED',
  'ECONNRESET',
  'EHOSTUNREACH',
  'ENETUNREACH',
  'ENOTFOUND',
  'EAI_AGAIN',
]);

function createProviderRecord() {
  return {
    requests: 0,
    failures: 0,
    failureCodes: {},
    pendingFailureWarnings: 0,
  };
}

function createProviderState() {
  return {
    brave: createProviderRecord(),
    serper: createProviderRecord(),
  };
}

function providerStateFor(state, provider) {
  return state[provider] || (state[provider] = createProviderRecord());
}

function asUrl(input) {
  try {
    if (input instanceof URL) return input;
    if (typeof input === 'string') return new URL(input);
    if (input && typeof input.url === 'string') return new URL(input.url);
  } catch {
    return null;
  }
  return null;
}

export function providerForRequest(input) {
  const url = asUrl(input);
  if (!url) return null;
  for (const [provider, contract] of Object.entries(PROVIDERS)) {
    if (url.origin === contract.origin && url.pathname === contract.pathname) return provider;
  }
  return null;
}

function normalizedErrorCode(error) {
  return String(error?.code || '')
    .trim()
    .toUpperCase()
    .replace(/[^A-Z0-9_]/g, '')
    .slice(0, 40);
}

export function providerFailureCode(error, responseStatus = null) {
  if (Number.isInteger(responseStatus) && responseStatus >= 400 && responseStatus <= 599) {
    return `http_${responseStatus}`;
  }

  const name = String(error?.name || '').toLowerCase();
  const message = String(error?.message || '').toLowerCase();
  const code = normalizedErrorCode(error);

  if (name === 'aborterror' || name === 'timeouterror' || /\b(?:abort|timed?\s*out|timeout)\b/.test(message)) {
    return 'timeout';
  }
  if (
    name === 'responselimiterror'
    || /response (?:body )?(?:exceeds|timed out)|invalid json/.test(message)
  ) {
    return 'response_limit';
  }
  if (/^upstream_http_[45][0-9]{2}$/.test(message)) {
    return message.replace('upstream_', '');
  }
  if (
    NETWORK_ERROR_CODES.has(code)
    || /\b(?:dns|network|socket|connection reset|fetch failed)\b/.test(message)
  ) {
    return 'network';
  }
  return 'unknown';
}

function incrementFailure(state, code, { expectWarning = false } = {}) {
  state.failures += 1;
  state.failureCodes[code] = Number(state.failureCodes[code] || 0) + 1;
  if (expectWarning) state.pendingFailureWarnings += 1;
}

export function createProviderDiagnosticsFetch(originalFetch, state = createProviderState()) {
  if (typeof originalFetch !== 'function') throw new TypeError('originalFetch must be a function');

  return async function providerDiagnosticsFetch(input, init) {
    const provider = providerForRequest(input);
    if (!provider) return originalFetch(input, init);

    const providerState = providerStateFor(state, provider);
    providerState.requests += 1;

    try {
      const response = await originalFetch(input, init);
      if (!response?.ok) {
        incrementFailure(
          providerState,
          providerFailureCode(null, Number(response?.status)),
          { expectWarning: true },
        );
      }
      return response;
    } catch (error) {
      incrementFailure(providerState, providerFailureCode(error), { expectWarning: true });
      throw error;
    }
  };
}

function warningFailureCode(errorName, errorCode) {
  const name = String(errorName || '').toLowerCase();
  const code = String(errorCode || '').toUpperCase();
  if (name === 'responselimiterror') return 'response_limit';
  if (name === 'aborterror' || name === 'timeouterror') return 'timeout';
  if (NETWORK_ERROR_CODES.has(code)) return 'network';
  return 'unknown';
}

export function recordProviderWarning(state, message) {
  const match = String(message || '').match(
    /^\[benefactor-pipeline\] provider=(brave|serper) search_failed ([A-Za-z][A-Za-z0-9]*)(?: code=([A-Za-z0-9_]+))?/,
  );
  if (!match) return false;

  const providerState = providerStateFor(state, match[1]);
  if (providerState.pendingFailureWarnings > 0) {
    providerState.pendingFailureWarnings -= 1;
    return true;
  }

  incrementFailure(providerState, warningFailureCode(match[2], match[3]));
  return true;
}

function stableFailureCodes(value) {
  return Object.fromEntries(
    Object.entries(value || {})
      .filter(([code, count]) => /^[a-z0-9_]{1,48}$/.test(code) && Number.isSafeInteger(count) && count > 0)
      .sort(([left], [right]) => left.localeCompare(right)),
  );
}

export function buildProviderDiagnostics(state = createProviderState()) {
  return {
    reportVersion: PROVIDER_DIAGNOSTICS_VERSION,
    providers: Object.keys(PROVIDERS).sort().map((provider) => {
      const value = state[provider] || {};
      const requests = Number(value.requests || 0);
      const failures = Number(value.failures || 0);
      return {
        provider,
        requests,
        successes: Math.max(0, requests - failures),
        failures,
        failureCodes: stableFailureCodes(value.failureCodes),
      };
    }),
  };
}

export function installProviderDiagnostics({ target = globalThis } = {}) {
  if (target[INSTALL_MARKER]) return false;
  const state = createProviderState();
  const originalFetch = target.fetch;
  if (typeof originalFetch !== 'function') throw new TypeError('global fetch is unavailable');
  target.fetch = createProviderDiagnosticsFetch(originalFetch.bind(target), state);

  const originalWarn = target.console?.warn?.bind(target.console);
  if (typeof originalWarn === 'function') {
    target.console.warn = (...args) => {
      originalWarn(...args);
      recordProviderWarning(state, args.map((value) => String(value)).join(' '));
    };
  }

  const originalLog = target.console?.log?.bind(target.console);
  if (typeof originalLog === 'function') {
    target.console.log = (...args) => {
      originalLog(...args);
      if (typeof args[0] === 'string' && args[0].startsWith('BENEFACTOR_PIPELINE_REPORT ')) {
        originalLog(`BENEFACTOR_PROVIDER_DIAGNOSTICS ${JSON.stringify(buildProviderDiagnostics(state))}`);
      }
    };
  }

  target[STATE_MARKER] = state;
  target[INSTALL_MARKER] = true;
  return true;
}
