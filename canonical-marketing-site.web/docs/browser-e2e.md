# Marketing-site browser e2e — state, gaps, and how to shore it up

Puppeteer + Playwright specs live in [`tests/`](../tests/) alongside a shared
`site-browser-harness.mjs` (boots `astro preview` on an ephemeral port, resolves
Chromium). They run via the `browser-e2e` CI job (ubuntu-latest, blocking) and
the opt-in `browser-e2e-selfhosted.yml`.

**Covered today (10 tests):** landing-page smoke (both engines) + hero/nav/
services, and the below-the-fold sections — stats, process timeline, frameworks
grid, differentiators, testimonial + contact CTA, SEO/OG metadata, single-`h1`
accessibility, and a 375px mobile no-overflow check.

`test:browser` globs `tests/*-{playwright,puppeteer}.test.mjs`, so new specs are
picked up automatically — add a `tests/<area>-{puppeteer,playwright}.test.mjs`
pair and it just runs.

---

## 1. Assertions are content-exact → brittle to copy edits (MED)

Tests assert literal marketing copy (`"60%"`, `"Lower Cost vs Big 4"`,
`"James Rodriguez"`, the six framework tiers, etc.). A wording change breaks the
suite even when nothing is actually wrong. That's a deliberate contract for the
compliance framework names (they must not silently change), but the softer
marketing copy shouldn't be a tripwire.

**How to shore up.** Split the intent: keep exact assertions for the
must-not-drift set (framework names, mailto CTA, section anchors, `<title>`);
loosen the rest to structural/`data-testid` checks (counts, presence, order)
rather than verbatim text. Consider adding `data-testid` attributes to the
`.stats__item` / `.frameworks__item` / `.process__step` nodes so the specs bind
to stable hooks instead of class names + copy.

## 2. No accessibility or visual-regression gate (MED)

The suite checks a single `h1` and that CTAs are keyboard-reachable elements, but
runs no real a11y audit and no visual diff. A contrast regression, a missing
`alt`, or a broken layout would pass.

**How to shore up.**
- a11y: add `@axe-core/playwright` and assert zero serious/critical violations on
  the landing page (one new Playwright test). Cheap, high-signal.
- visual: Playwright's `toHaveScreenshot()` with a committed baseline per
  viewport (desktop + the 375px mobile we already exercise). Gate it as
  non-blocking first while baselines settle, then promote to blocking.

## 3. Only the happy path / one page (LOW)

There is exactly one route (`/`). When the site grows (pricing, per-framework
pages, a real contact form), each needs its own spec pair, and form submission /
client-side interactivity will need actual interaction coverage, not just static
render assertions.

## 4. CI/runtime notes (LOW)

- The Node-version divergence documented in project memory (CI Node 22 vs the
  Docker client stage's node:26 and `navigator.locks`) does **not** affect this
  static Astro site, but keep it in mind if client-side JS is ever added here.
- `test:browser` rebuilds the site each run (`npm run build && node --test …`).
  Fine at current size; if the build gets slow, cache `dist/` or reuse a running
  server via `CANONICAL_SITE_TEST_URL` (already supported by the harness).
- Double Chromium provisioning: same note as the web-server repo — the
  ubuntu-latest job installs Playwright's Chromium and also lets Puppeteer
  download its own. Unify on one browser to halve setup time (see
  `canonical-web-server.rs/docs/browser-e2e.md` §3).

## 5. What was NOT verified

The self-hosted path (`runs-on: canonical-browser`) has not executed — the runner
scale set isn't deployed. Validated only as YAML + by mirroring the working
ubuntu-latest job. See k8s-cluster `docs/canonical-ci-runners-followups.md`.
