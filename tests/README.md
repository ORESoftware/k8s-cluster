# tests

Automated tests for the marketing site, run with the Node test runner.

- `site-contract.test.mjs` — fast, browser-free source contract checks: asserts
  the landing page, layout, and CSS retain their key marketing copy, CTAs, and
  responsive rules. This is the `npm test` gate.
- `site-browser-harness.mjs` — shared boot recipe that starts `astro preview`
  on an ephemeral port (under the `/fiducia` base) and re-exports Chrome
  discovery from `@fiducia/test-config`; imported by the browser specs.
- `site-playwright.test.mjs` / `site-puppeteer.test.mjs` — full browser E2E that
  render the live landing page and custom 404 and assert title, hero, nav,
  service cards, CTAs, and no page errors. Run via `npm run test:browser`;
  best-effort in CI (needs a real Chrome).
