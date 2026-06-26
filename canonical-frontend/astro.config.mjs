// @ts-check
import { defineConfig } from 'astro/config';

// https://astro.build/config
export default defineConfig({
  site: 'https://canonical.cloud',
  base: '/',
  server: {
    port: 4322,
  },
});
