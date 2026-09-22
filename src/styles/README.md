# styles

Global CSS for the site.

- `global.css` — imported once by `src/layouts/Layout.astro` and thus applied
  everywhere. Defines the navy + purple design tokens (CSS custom properties),
  base element styles, shared component classes (nav, hero, buttons, cards,
  grids, badges), and the responsive breakpoints the contract tests guard
  (e.g. mobile nav collapse at `max-width: 880px`).
