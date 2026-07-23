# daedalus-monorepo

Git superproject for the [daedalus-fab](https://github.com/daedalus-fab) application repositories,
following the fiducia-cloud / sonus-auris monorepo pattern.

Each integrated app or library repo is tracked as a git submodule under `apps/`. The
superproject pins each submodule to an exact commit while `.gitmodules` sets
`branch = main`, so updates intentionally follow each repo's main branch.
Marketing/Pages sites are deliberately **not** vendored here.

This repo is private: it is the all-up integration view of the Daedalus fleet.

## Clone

```sh
git clone --recurse-submodules git@github.com:daedalus-fab/daedalus-monorepo.git
```

## Apps

- [`apps/fabrication-server.rs`](https://github.com/daedalus-fab/fabrication-server.rs) — fabrication planning service
- [`apps/daedalus-api-server.rs`](https://github.com/daedalus-fab/daedalus-api-server.rs) — JSON API (MASH A/S tier)
- [`apps/daedalus-web-server.rs`](https://github.com/daedalus-fab/daedalus-web-server.rs) — Maud/htmx UI (MASH M/H tier)
- [`apps/daedalus-clients`](https://github.com/daedalus-fab/daedalus-clients) — official API clients
- [`apps/daedalus-interfaces`](https://github.com/daedalus-fab/daedalus-interfaces) — generated shared data adapters
- [`apps/daedalus-sync`](https://github.com/daedalus-fab/daedalus-sync) — local-first browser, mobile, and Rust synchronization

## Tests

`npm test` runs the monorepo contract tests (submodule inventory ↔
`.gitmodules` ↔ index gitlinks, workflow action-pin auditing, and the
TypeScript sync compatibility contract). CI runs them on every push/PR and
weekly; the `fleet-audit` job additionally initializes the private submodules
and runs the real TypeScript, Rust, and Dart compatibility tests when an
`ORG_SUBMODULE_TOKEN` Actions secret (fine-grained, read-only contents on this
org) is configured.

## Updating pins

`scripts/pin-submodules.sh` fetches every app's `main` and stages the new
pins. See [AGENTS.md](AGENTS.md) — history is append-only in this repo.
