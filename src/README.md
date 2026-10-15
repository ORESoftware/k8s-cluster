# src/

Astro source for the Sonus Auris marketing + compliance site. Everything here is
compiled to static HTML/CSS by `astro build`; the site ships **no client
JavaScript** (the only animation is inline SVG/CSS).

## Layout

- `pages/` — file-based routes; each `.astro` file becomes a page.
  `index.astro` is the single-page homepage (assembles the section components);
  `privacy.astro` (`/privacy`) and `account-deletion.astro` (`/account-deletion`)
  are the legal/compliance pages required for app-store approval, both rendered
  in the `Legal` layout. Note: do **not** add a `README.md` (or any `.md`) inside
  `pages/` — Astro would publish it as a live page.
- `layouts/` — shared page shells. `Base.astro` renders the HTML document
  (`<head>`, meta/OG tags, font preload); `Legal.astro` wraps Base with a narrow,
  readable article column used by the legal pages.
- `components/` — the homepage sections (Hero, Features, Privacy, …) plus small
  reusable pieces (Logo, StoreButtons, Nav, Footer, Partners).
- `lib/` — tiny build-time helpers shared by components. `external-url.ts`
  validates deployment-supplied URLs (store listings, download hosts) as
  absolute https before they can reach an `href`; `unique-id.ts` hands out
  per-render DOM ids for components that appear more than once on a page.
- `styles/global.css` — brand design tokens (colors, radius, shadows), base
  element styles, shared utility classes (`.container`, `.btn`, `.card`,
  `.eyebrow`, `.section`), and the self-hosted `@font-face`.
- `env.d.ts` — Astro TypeScript type reference; do not edit.

## Notes

- Production is served at the custom-domain root `https://sonusauris.app/`, not
  the GitHub Pages repository subpath. Internal links and public-asset URLs still
  use `import.meta.env.BASE_URL` so explicit preview/subpath builds remain
  possible; the release workflow sets the base to `/`.
- The deployment workflow builds `dist/` once, runs all generated-output gates
  against that tree, rejects symlinks and special files, creates a deterministic
  `artifact.tar`, extracts it again, and compares the round-trip SHA-256 file
  inventory before upload. Do not reintroduce a second framework build between
  verification and publication.
- `/.well-known/security.txt` is release-critical. The Pages archive is packaged
  explicitly because the current `actions/upload-pages-artifact` composite
  excludes top-level dot-directories; using it directly would omit that route.
- The publisher identity the legal pages render (`pages/privacy.astro`,
  `pages/account-deletion.astro`) comes from `data/publisher.ts` — one frozen
  object, no placeholders. Store listing and download URLs are still supplied by
  the deployment environment; see the root `README.md` "Things to wire up before
  launch".
