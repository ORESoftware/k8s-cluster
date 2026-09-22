# scripts

Build/deploy helper scripts for the site.

- `sync-static.mjs` — copies the built `dist/` into the sibling
  `../fiducia-customer.rs/static/` directory so the Rust backend can serve the
  site. Run via `npm run sync` (which builds first). It refuses to write anywhere
  outside the workspace and errors if the build output is missing.
