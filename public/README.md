# public/

Static assets copied verbatim into the build output at the site root. Because
the site is served from the GitHub Pages subpath `/sonus-auris-site.web`,
references prefix these with `import.meta.env.BASE_URL`.

- `favicon.svg` — browser tab icon (referenced in `Base.astro`).
- `og.svg` — Open Graph / social-share preview image (referenced in `Base.astro`).
- `fonts/baloo2.woff2` — self-hosted Baloo 2 variable font (weights 500–800),
  preloaded in `Base.astro` and declared via `@font-face` in `styles/global.css`.
- `_headers` — desired security headers (CSP, HSTS, etc.). **Note:** GitHub Pages
  does NOT honor `_headers`, so on the current host this file is published as an
  inert text file and the headers are not actually served. See the comment at the
  top of the file for how to enforce them (Netlify/Cloudflare Pages, or a CDN/proxy
  in front of the origin).
