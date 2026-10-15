<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/deployments/fiducia-marketing.web` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/fiducia.cloud/fiducia-marketing.web` · submodule checkout `~/codes/ores/k8s-cluster/remote/deployments/fiducia-marketing.web`.
<!-- END k8s-cluster-submodule-notice -->

# fiducia-marketing

The [Astro](https://astro.build) front-end for **fiducia.cloud** — the marketing
homepage for Raft-based consensus & coordination as a service.

> [!NOTE]
> **Naming.** This repository was renamed from `fiducia-ui.web` to
> `fiducia-marketing.web` (2026-07) — it is the static marketing site, not a UI
> for the customer or admin apps. GitHub redirects the old repo URL.

This repository is static marketing only. Customer account/API-key workflows
belong to the Rust MASH server in `fiducia-customer.rs`; operator workflows belong
to the separate Rust MASH server in `fiducia-admin.rs`. Neither application is
compiled into this Astro site.

- Purple + navy theme, fully static output.
- Built behind a gateway path prefix, so `base` is `/fiducia` (override with
  `PUBLIC_BASE=/` to serve at a domain root).

## Develop

```bash
npm ci --ignore-scripts
npm run dev        # http://localhost:4321/fiducia/
npm run build      # -> dist/
npm run sync       # build + copy dist/ into ../fiducia-customer.rs/static/
```

`npm ci --ignore-scripts` installs the exact dependency graph in
`package-lock.json` without running dependency lifecycle hooks. Its local
packages are supplied as sibling repositories: `@fiducia/interfaces` at
`487e470c45ab5851e8f6f3b1dc048fe067fbf408` and `@fiducia/test-config` at
`825220281fdc16bbf47a035177001d2fe29bdabf`. CI checks out those full commits
explicitly rather than following moving branches.

## Container build

The Docker builder fetches those same two public sibling repositories at the
exact commits above, verifies their resolved HEADs, and then runs
`npm ci --ignore-scripts` and the Astro production build. Build inputs are
explicit and can be advanced only to other full commit IDs:

```bash
docker build \
  --build-arg INTERFACES_REF=487e470c45ab5851e8f6f3b1dc048fe067fbf408 \
  --build-arg TEST_CONFIG_REF=825220281fdc16bbf47a035177001d2fe29bdabf \
  --build-arg PUBLIC_BASE=/fiducia \
  -t fiducia-marketing .
```

`PUBLIC_BASE` is the only application build-time setting. The builder uses
digest-pinned `node:26-slim`; the final digest-pinned `nginx:1.31-alpine` stage
contains only the static
`dist/` output and nginx configuration—no Git client, source checkout, Node
toolchain, or sibling repositories. Nginx runs as its unprivileged `nginx` user,
listens on port `8080`, writes temporary files only under `/tmp`, and adds
baseline static-site security headers. CI builds this image from an isolated
checkout, so missing sibling inputs fail closed. When the gateway uses `/fiducia`, it
must continue stripping that prefix before proxying to the container; use
`PUBLIC_BASE=/` for root-hosted deployments.

The Rust backend (`../fiducia-customer.rs`) serves the built site. `static/` is
gitignored in the backend repo — deployment builds this site in-pod (node
initContainer) from this repo's main and hands it to the backend via
`STATIC_DIR`, so nothing built is ever committed there. `npm run sync` only
refreshes the local dev copy; run it after changing anything here so local
serving matches source.

Everything in `public/` is copied verbatim into `dist/` and served, and every
non-underscore-prefixed file in `src/pages/` becomes a live route — don't put
anything in either place that shouldn't be published (directory intent docs in
`src/pages/` are named `_README.md` for this reason).

## Security posture

- **Fully static, no secrets in the bundle.** Output is static HTML/CSS/JS. The
  only build-time env is `PUBLIC_BASE` (a URL path prefix) — no API keys, tokens,
  or credentials are inlined. `npm audit --omit=dev` reports 0 vulnerabilities.
- **No user data / no request-derived XSS sinks.** The marketing pages render
  only static, author-controlled content; there is no
  `innerHTML`/`dangerouslySetInnerHTML` and no request-derived interpolation.
  The single `set:html` (the hero terminal snippet in `src/pages/index.astro`)
  renders a build-time string constant — keep it that way: never pass
  request- or user-derived values to `set:html`.
- **HTTP security headers are set by the serving layer.**
  `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
  `Referrer-Policy: strict-origin-when-cross-origin`, a permissions policy, and
  a CSP are applied by `fiducia-customer.rs` in front of `dist/`; the standalone
  nginx image applies the first four headers directly. Note the backend CSP is
  currently just `upgrade-insecure-requests` (no source restrictions) so its
  docs/diagram pages can load their CDN scripts — see the comment in
  `fiducia-customer.rs/src/main.rs`. The shell also carries
  `<meta name="referrer" content="strict-origin-when-cross-origin">`
  as defense-in-depth so full URLs don't leak to third-party origins.
