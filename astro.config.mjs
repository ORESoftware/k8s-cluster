// @ts-check
// Astro build config for the fiducia.cloud marketing site: static output served
// by the Rust backend, under the gateway `/fiducia` base path (see comments below).
import { defineConfig } from 'astro/config';

// https://astro.build/config
export default defineConfig({
  site: 'https://fiducia.cloud',
  // Served behind the cluster gateway under the /fiducia/ path prefix, which is
  // stripped before reaching the backend. `base` makes every emitted asset URL
  // carry the /fiducia prefix so it round-trips through the gateway. Overridable
  // via PUBLIC_BASE (e.g. set to "/" when serving fiducia.cloud at its own root).
  base: process.env.PUBLIC_BASE ?? '/fiducia',
  // Static output — the Rust backend serves the generated `dist/`.
  output: 'static',
});
