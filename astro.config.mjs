// @ts-check
import { defineConfig } from 'astro/config';

const requestedPort = Number.parseInt(process.env.SONUS_AURIS_SITE_PORT ?? '', 10);
const port = Number.isInteger(requestedPort) && requestedPort >= 1 && requestedPort <= 65_535
  ? requestedPort
  : 4321;

const productionSite = 'https://sonusauris.app';
const requestedSite = process.env.SONUS_AURIS_SITE_URL?.trim();
const requestedBase = process.env.SONUS_AURIS_SITE_BASE?.trim();

export default defineConfig({
  // The public release surface is the custom-domain root. Preview builds may
  // override both values, but production builds must never silently fall back
  // to the old GitHub Pages project subpath.
  site: requestedSite || productionSite,
  base: requestedBase || '/',
  server: {
    port,
    host: process.env.SONUS_AURIS_SITE_HOST ?? true,
  },
});
