import { createBraveProvider } from './brave.mjs';
import { createSerperProvider } from './serper.mjs';

export function createSearchProviders({ braveKey, serperKey, fetchJson }) {
  return [
    createSerperProvider({ apiKey: serperKey, fetchJson }),
    createBraveProvider({ apiKey: braveKey, fetchJson }),
  ];
}
