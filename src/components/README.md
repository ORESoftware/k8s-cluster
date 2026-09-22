# src/components/

Reusable Astro components. Most are full-width homepage **sections** (each owns
its markup + scoped `<style>`); a few are small shared pieces. No client JS —
animation is inline SVG/CSS only.

## Homepage sections (in page order)

- `Nav.astro` — sticky header: logo, in-page anchor links, GitHub link, "Get the
  app" CTA.
- `Hero.astro` — headline pitch + `StoreButtons`, and an animated inline-SVG
  phone (record ring, waveform bars, cloud-backup badge) with floating emoji
  doodles. Decorative art is `aria-hidden`.
- `Audience.astro` — "who it's for" card grid (`#why`), driven by an `audiences`
  data array (musicians, evidence/peace-of-mind, music lovers, students & pros).
- `Features.astro` — feature card grid (`#features`), driven by a `features`
  data array (rolling buffer, cloud backup, bookmarks, on-device analysis, …).
- `HowItWorks.astro` — numbered 4-step flow (`#how`) from a `steps` array.
- `Privacy.astro` — dark green privacy-first section (`#privacy`): a `promises`
  grid plus a decorative "encrypted vault" card.
- `OpenSource.astro` — "read the code" section (`#open-source`): a `points`
  list, GitHub CTA, and a decorative fake terminal transcript.
- `Download.astro` — final centered CTA card (`#download`) with `StoreButtons`.
- `DesktopDownloads.astro` — environment-driven Rust desktop alpha links for
  R2 or a temporary restricted tester folder.
- `NoSpooks.astro` — tongue-in-cheek "certified spook-free" / warrant-canary
  band. The agency call-outs and membership badge are a joke/satirical
  placeholder (see the comment in the file).

## Shared pieces

- `Footer.astro` — renders `Partners`, then brand blurb + Product/Trust/Legal
  link columns (including the privacy & account-deletion routes) and a copyright
  line.
- `Partners.astro` — footer row of **neutral capability badges** (not third-party
  endorsements). Icons are hard-coded SVG strings rendered via `set:html`; never
  feed that from untrusted data. Do not add real brand names without permission.
- `StoreButtons.astro` — App Store + Google Play badge row (used by Hero and
  Download). `align` prop = `start` | `center`. Listing URLs are placeholders.
- `Logo.astro` — SVG mark + wordmark linking home. `size` scales it; `light`
  switches text colors for dark backgrounds.
