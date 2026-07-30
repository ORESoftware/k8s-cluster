<!-- BEGIN k8s-cluster-submodule-notice -->
> [!NOTE]
> **Canonical source.** This repository is the source of truth for its code. It
> is also vendored as a **secondary** git submodule of
> [ORESoftware/k8s-cluster](https://github.com/ORESoftware/k8s-cluster) at
> `remote/submodules/sonus-auris-site.web` — make changes here, not in that submodule checkout.
>
> On disk: source clone `~/codes/sonus-auris/sonus-auris-site.web` · submodule checkout `~/codes/ores/k8s-cluster/remote/submodules/sonus-auris-site.web`.
<!-- END k8s-cluster-submodule-notice -->

# sonus-auris-site.web

Marketing website for **Sonus Auris** — *a dashcam for audio*. An always-on
recording app that keeps a rolling buffer of your audio and backs it up,
encrypted, to the cloud. Built for musicians who don't want to lose the riff,
and for anyone who wants an honest record when memory isn't enough.

Built with [Astro](https://astro.build). No client framework — just fast,
static HTML/CSS with a little inline SVG animation.

## Develop

```bash
npm install
npm run dev      # http://localhost:4321
```

[`flags-2-env`](https://github.com/ORESoftware/flags-2-env) can validate and
override Astro's host, port, canonical site URL, and base path:

```sh
scripts/with-flags help
scripts/with-flags audit
scripts/with-flags --host=127.0.0.1 --port=4321 -- npm run dev
scripts/with-flags --site=https://example.test --base=/preview -- npm run build
```

The wrapper builds the monorepo's pinned native source into a commit-keyed user
cache, or uses `FLAGS2ENV_BIN` in a standalone checkout.

## Build

```bash
npm run build    # outputs to ./dist
npm run preview  # preview the production build
```

## Release links

The site is static and never proxies installer bytes. Mobile store URLs and
desktop installer URLs are injected at build time:

```sh
cp .env.example .env
# Set PUBLIC_APP_STORE_URL / PUBLIC_PLAY_STORE_URL after listings are public.
# Set PUBLIC_DOWNLOAD_BASE_URL to the Cloudflare R2 custom domain.
npm run build
```

`PUBLIC_DOWNLOAD_BASE_URL` expands to these stable aliases:

- `latest/sonus-auris-windows-x64.exe`
- `latest/sonus-auris-macos-universal.dmg`
- `latest/sonus-auris-linux-x86_64.deb`

A per-platform `PUBLIC_DOWNLOAD_*_URL` overrides its alias. The release pipeline
must upload immutable, versioned objects first, then replace the `/latest`
aliases with `Cache-Control: no-store`. Large binaries go directly from the R2
custom domain to the user; the Rust server may expose release metadata or issue
redirects, but it must not become a bandwidth proxy for public installers.

## Structure

```
src/
  layouts/
    Base.astro              # <head>, fonts, OG tags, <body> slot
    Legal.astro             # readable article column for /privacy & deletion
  components/
    Nav.astro               # sticky header
    Hero.astro              # animated phone + waveform
    Audience.astro          # musicians + peace-of-mind
    Features.astro          # feature grid
    HowItWorks.astro        # 4-step flow
    Privacy.astro           # privacy-first promises + vault
    OpenSource.astro        # "read the code" + terminal
    Download.astro          # final CTA
    NoSpooks.astro          # tongue-in-cheek "no spooks" / warrant-canary band
    Footer.astro            # link columns + Partners row
    Partners.astro          # neutral capability badges (footer)
    StoreButtons.astro      # mobile stores + optional desktop installers
    Logo.astro
  pages/
    index.astro             # single-page marketing assembly
    privacy.astro           # /privacy — app-store privacy policy
    account-deletion.astro  # /account-deletion — data-deletion page
  styles/global.css         # design tokens, base styles, @font-face
public/                     # favicon.svg, og.svg, fonts/, _headers
```

Each directory also has its own `README.md` describing what lives there.

## Things to wire up before launch

Still open:

- Configure the store listing and download URLs in the site deployment
  environment — `PUBLIC_APP_STORE_URL`, `PUBLIC_PLAY_STORE_URL`, and
  `PUBLIC_DOWNLOAD_{BASE,WINDOWS,MACOS,LINUX}_URL`, all read by
  `StoreButtons.astro`. Never link to unsigned or unreviewed artifacts.

Standing rules, not tasks:

- `Partners.astro` — the footer row shows **neutral capability badges** (Sound
  matching, Sleep & snore, Music capture, Clear audio), NOT third-party
  endorsements. Do not reintroduce real organisation/brand names without written
  permission — that would imply an affiliation Sonus Auris does not have.
- The publisher identity that the legal pages render lives in one place,
  `src/data/publisher.ts`. Keep it accurate: App Review and the Play Console
  both check that a privacy policy names a real controller and a reachable
  contact.

Already done (this section previously said otherwise):

- Legal-entity name, contact email, and postal address are filled in via
  `src/data/publisher.ts`, and the "Before publishing" banner is gone. The
  `tests/site/legal.test.mjs` release gate in `sonus-auris-e2e` no longer has
  placeholders to find — verified against the built `dist/` HTML.
- `astro.config.mjs` `site` resolves to the production domain
  (`requestedSite || productionSite`), overridable per build.
- GitHub links point at `github.com/sonus-auris/sonus-auris-ui.dart`, which
  exists; the old note here named a repo that does not.

## Theme

Green + orange, rounded **Baloo 2** type, music / party / driving cartoon
doodles. Fun but professional.
