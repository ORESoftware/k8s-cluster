# src/layouts/

Shared page shells wrapped around page content via `<slot />`.

- `Base.astro` — the HTML document. Renders `<html><head>…</head><body><slot/>`
  with charset/viewport, `<title>` + description (overridable via props), Open
  Graph tags, theme color, favicon, and a preload for the self-hosted Baloo 2
  font. Public asset URLs are prefixed with the GitHub Pages base path. Used by
  every page.
- `Legal.astro` — layout for the legal pages. Builds on `Base`, adds a narrow
  (`max-width: 46rem`) centered article column with a "← Sonus Auris" back link
  and scoped typography for headings, paragraphs, tables, and the `.updated` /
  `.todo` note styles. Used by `privacy.astro` and `account-deletion.astro`.
