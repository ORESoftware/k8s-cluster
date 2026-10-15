# pages

Astro file-based routes — each `.astro` file here becomes a page in the built
site.

- `index.astro` — the fiducia.cloud marketing landing page: hero, the six
  coordination-primitive service cards (locks, rate limiting, cron, config KV,
  leader election, service discovery), AI-agent-fleet positioning,
  how-it-works, and calls to action.
- `404.astro` — the on-brand custom "no quorum" not-found page.

All pages wrap their content in `src/layouts/Layout.astro`.

This file is underscore-prefixed (`_README.md`) so Astro excludes it from
routing — a plain `README.md` here would be built and published as a live
`/README/` page on the site.
