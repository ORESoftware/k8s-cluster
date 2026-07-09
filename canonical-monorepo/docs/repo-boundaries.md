# Repo boundaries

`canonical-monorepo` is a **git superproject**: it stores submodule *pins*
(gitlinks) plus shared config (CI, docs, scripts, `build.sh`). It never vendors
app source directly.

## Apps

- **`canonical-backend.rs`** — Rust (axum) HTTP service. Serves the built Astro
  site from `STATIC_DIR` and exposes `/healthz` + `/api/{health,info}`. Public.
- **`canonical-frontend`** — Astro static marketing site (SOC 2 / FedRAMP /
  HIPAA). Builds to `dist/`, which the backend serves. Public.

Each app is its own repo with its own visibility, CI, Dockerfile, `agents.md`,
and Nix dev shell. The superproject is the all-up integration / GitOps view.

## Where things live

| Concern                         | Home                                             |
| ------------------------------- | ------------------------------------------------ |
| App source                      | the app repo (`apps/<app>`, a submodule)         |
| Cross-repo build (site→backend) | `build.sh` here                                  |
| Submodule pins / branch pins    | `.gitmodules` + gitlinks here                    |
| Shared automation               | `scripts/` here                                  |

## Rules

- Change app code inside the submodule, push it there, **then** update the pin
  here with `scripts/pin-submodules.sh`.
- Do not commit real `.env*` files. Only `.env.example` (placeholder values) is
  tracked; everything else matching `.env*` is gitignored.
- Keep destructive actions manual: the `scripts/*.sh` helpers never `git push`
  and guard with `--dry-run` / `--allow-dirty`.
- Removing a submodule checkout with `rm -rf apps/<app>` corrupts the gitlink
  state — use `git submodule` commands (or `git rm` for a real removal) instead.
