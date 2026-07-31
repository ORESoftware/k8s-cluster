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

Desktop alpha buttons are enabled at build time. Use
`PUBLIC_RUST_DESKTOP_DOWNLOAD_BASE_URL` and
`PUBLIC_FLUTTER_DESKTOP_DOWNLOAD_BASE_URL` for the stable R2 alpha prefixes, or
`PUBLIC_DESKTOP_TESTERS_URL` for a restricted
Google Drive folder while R2 is being activated. If neither is set, the site
shows an honest non-clickable “Downloads opening soon” state.

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
    DesktopDownloads.astro  # gated desktop-alpha downloads
    NoSpooks.astro          # tongue-in-cheek "no spooks" / warrant-canary band
    Footer.astro            # link columns + Partners row
    Partners.astro          # neutral capability badges (footer)
    StoreButtons.astro      # App Store + Google Play badges
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

- `StoreButtons.astro` — replace `APPLE_URL` / `GOOGLE_URL` with real listings.
- GitHub links point to `github.com/sonus-auris/sonus-auris` — update if the
  org/repo name differs.
- `astro.config.mjs` `site` — set to the real production domain.
- `Partners.astro` — the footer row now shows **neutral capability badges**
  (Sound matching, Sleep & snore, Music capture, Clear audio), NOT third-party
  endorsements. Do not reintroduce real organisation/brand names without written
  permission — that would imply an affiliation Sonus Auris does not have.
- Legal pages (`privacy.astro`, `account-deletion.astro`) have placeholder
  legal-entity name, contact email, and postal address to fill before store
  submission; a visible "Before publishing" banner shows while placeholders remain.

## Theme

Green + orange, rounded **Baloo 2** type, music / party / driving cartoon
doodles. Fun but professional.
