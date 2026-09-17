# layouts

Reusable Astro layout components that wrap page content.

- `Layout.astro` — the base HTML shell for every page. Sets `<head>` metadata
  (title, description, Open Graph tags, theme color, favicon), normalizes the
  `/fiducia` base URL, imports the global stylesheet, and renders page content
  through a `<slot />`.
