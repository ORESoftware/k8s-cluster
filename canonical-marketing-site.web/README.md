# canonical-marketing-site.web

[Astro](https://astro.build) marketing site for
**[canonical.cloud](https://canonical.cloud)** — SOC 2, FedRAMP & HIPAA
compliance audits. Built to a static `dist/` and served in production by
[`canonical-web-server.rs`](https://github.com/canonical-cloud/canonical-web-server.rs).

Part of the [`canonical-monorepo`](https://github.com/canonical-cloud/canonical-monorepo)
superproject; also usable standalone.

## Structure

```text
src/
  pages/index.astro        # the landing page
  layouts/BaseLayout.astro # head metadata, nav, footer
  styles/global.css        # design tokens + base styles
tests/                     # node --test specs (contract + playwright + puppeteer)
```

## Develop

This repo ships a Nix dev shell and an `.envrc`:

```sh
direnv allow          # or: nix develop ./.nix   (or: ./shell)
npm install
npm run dev           # http://localhost:4321
```

## Build

```sh
npm run build         # -> dist/
npm run preview       # serve the built dist/
```

Wire the build into the web server for a full local stack:

```sh
npm run build
STATIC_DIR=$PWD/dist  (cd ../canonical-web-server.rs && cargo run)
```

## GitHub Pages

Pushes to `main` deploy the static site to
<https://canonical.plus/> through the `pages` workflow. The workflow supplies
Astro's custom-domain `site` and root `base` flags only for that build, so the
normal root deployment at
`https://canonical.cloud` remains unchanged.

To reproduce the Pages artifact locally:

```sh
npm ci
npm test
npm run build -- \
  --site https://canonical.plus \
  --base /
```

## Test

```sh
npm test              # fast static-contract specs (CI gate)
npm run test:browser  # build + Playwright AND Puppeteer e2e (both runners)
```

The browser suite boots `astro preview` on an ephemeral port and drives it with
both Playwright and Puppeteer via `node --test`. It needs a local Chrome/Chromium
(the Nix shell provides one; otherwise Puppeteer downloads one and
`npx playwright install chromium` fetches Playwright's). Point it at an already
running site with `CANONICAL_SITE_TEST_URL=http://localhost:4321`.
