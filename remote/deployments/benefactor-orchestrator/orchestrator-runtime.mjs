import {
  createProviderDiagnosticsFetch,
  createProviderState,
  formatProviderDiagnosticsLine,
  recordProviderWarning,
} from './provider-diagnostics-bridge.mjs';
import { createScraperContactFetch } from './scraper-contact-bridge.mjs';

function optionalScraperUrl(value) {
  if (value instanceof URL) return value;
  if (typeof value === 'string' && value.trim()) return new URL(value);
  return undefined;
}

export function createBenefactorRuntime({
  fetchImpl = globalThis.fetch?.bind(globalThis),
  scraperUrl,
  requireRoleEmail,
} = {}) {
  if (typeof fetchImpl !== 'function') throw new TypeError('fetchImpl must be a function');

  const providerState = createProviderState();
  const providerFetch = createProviderDiagnosticsFetch(fetchImpl, providerState);
  const contactOptions = {};
  const parsedScraperUrl = optionalScraperUrl(scraperUrl);
  if (parsedScraperUrl) contactOptions.scraperUrl = parsedScraperUrl;
  if (typeof requireRoleEmail === 'boolean') contactOptions.requireRoleEmail = requireRoleEmail;

  const fetch = createScraperContactFetch(providerFetch, contactOptions);
  return Object.freeze({
    fetch,
    recordProviderWarning(message) {
      return recordProviderWarning(providerState, message);
    },
    providerDiagnosticsLine() {
      return formatProviderDiagnosticsLine(providerState);
    },
  });
}
