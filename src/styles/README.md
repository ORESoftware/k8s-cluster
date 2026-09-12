# src/styles/

- `global.css` — the site's only stylesheet, imported once by `Base.astro` so it
  applies to every page. Contains:
  - the self-hosted **Baloo 2** `@font-face` (so no Google Fonts request / no
    third-party IP leak, keeping the CSP tight);
  - `:root` **design tokens** — the green + orange brand palette, ink/paper
    colors, shadows, radii, font stack, and max content width;
  - base element resets and typography (`box-sizing`, `body`, headings, links);
  - shared utility/component classes reused across components: `.container`,
    `.eyebrow`, `.section`, `.section-head`, `.btn` variants, `.card`,
    `.wave-divider`;
  - a `prefers-reduced-motion` block that neutralizes animations.

Component-specific styles live in each `.astro` file's scoped `<style>` block,
not here. When adding a component, prefer these tokens/utilities before writing
new CSS.
