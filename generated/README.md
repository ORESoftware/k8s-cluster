# `generated/` — generated API documentation (do not hand-edit)

These files are **generated and checked in**, not written by hand. They are produced by
`remote/tools/generate-api-docs.mjs` in the `ores/k8s-cluster` monorepo (see the `generatedBy` field
in `api-docs.json`), which inspects the service's routes. Regenerate from that tool rather than
editing these files directly — manual changes will be overwritten.

The running service serves this content at `GET /docs/api`, `GET /api/docs` (HTML), and
`GET /api/docs.json` (JSON).

## Files

- **`api-docs.json`** — machine-readable route catalog: per-route path, methods, route type
  (`service` vs `user-generated`), auth posture, and purpose, plus summary counts.
- **`api-docs.html`** — self-contained, styled HTML rendering of the same catalog for humans.
